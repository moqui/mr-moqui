/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! The bodies of `org.moqui.admin.ConstitutionServices` (docs/ai/Constitution.md): reading the document that applies,
//! the pure helpers both the global editor and an organization's editor use (validate, edit, prompt preview), the
//! global document's store, customize and reset (the sealed `ADMIN_CONSTITUTION_GLOBAL` permission), the record of a
//! change, the trusted reference origins, and the reference reads of the Assist Rules view and the
//! `get#ConstitutionReferences` agent tool.

use std::cell::RefCell;
use std::collections::BTreeMap;

use indexmap::IndexMap;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};
use serde_json::Value as Json;

use super::document::{self, FieldError, UrlKind};
use super::edit::{self, Edit};
use super::origins::{self, Policy};
use super::resolve::{self, Resolved, Scope};
use super::snapshot::{self, RunConstitution};
use super::{locale, render};
use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_core::llm_hooks::RustServiceFn;
use moqui_core::rust_service;

/// The sealed permission of the global document's changes and the trusted origins.
pub const GLOBAL_PERMISSION: &str = "ADMIN_CONSTITUTION_GLOBAL";

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, get_constitution)?;
    rust_service!(map, validate_constitution)?;
    rust_service!(map, edit_constitution_document)?;
    rust_service!(map, preview_constitution_prompt)?;
    rust_service!(map, store_constitution)?;
    rust_service!(map, create_constitution_customization)?;
    rust_service!(map, reset_constitution)?;
    rust_service!(map, record_constitution_change)?;
    rust_service!(map, get_constitution_references)?;
    rust_service!(map, get_constitution_snapshot)?;
    rust_service!(map, resolve_constitution_reference)?;
    rust_service!(map, get_reference_origins)?;
    rust_service!(map, store_reference_origins)?;
    Ok(())
}

type Parameters = BTreeMap<String, Value>;
type Out = Result<BTreeMap<String, Value>, LibError>;

thread_local! {
    /// The agent run whose READ tool is calling, set by the tool runner (never a parameter a model can give).
    static RUN: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// `body` with `run_id` as the calling run, for the READ services an agent's tool runs.
pub(crate) fn with_run<T>(run_id: Option<&str>, body: impl FnOnce() -> T) -> T {
    let previous = RUN.with(|run| run.replace(run_id.map(str::to_string)));
    let out = body();
    RUN.with(|run| run.replace(previous));
    out
}

fn current_run() -> Option<String> {
    RUN.with(|run| run.borrow().clone())
}

fn text(parameters: &Parameters, key: &str) -> Option<String> {
    parameters
        .get(key)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// A parameter as given, empty text kept (an editor clears a translation with it); `None` when left out.
fn raw(parameters: &Parameters, key: &str) -> Option<String> {
    parameters.get(key).filter(|value| !value.is_null()).map(to_plain_string)
}

fn flag(parameters: &Parameters, key: &str) -> bool {
    matches!(text(parameters, key).as_deref(), Some("Y" | "y" | "true"))
}

/// A list parameter: a list, or text split on commas, white space and new lines.
fn list(parameters: &Parameters, key: &str) -> Vec<String> {
    match parameters.get(key) {
        Some(Value::List(items)) => items.iter().map(to_plain_string).map(|item| item.trim().to_string()).filter(|item| !item.is_empty()).collect(),
        Some(value) if !is_empty(value) => to_plain_string(value)
            .split(|c: char| c == ',' || c.is_whitespace())
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn add_error(text: impl Into<String>) {
    let text = text.into();
    ExecutionContext::with_current(|ctx| ctx.message().add_error(text));
}

fn out(entries: impl IntoIterator<Item = (&'static str, Value)>) -> BTreeMap<String, Value> {
    entries.into_iter().map(|(key, value)| (key.to_string(), value)).collect()
}

fn map(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Map(entries.into_iter().map(|(key, value)| (key.to_string(), value)).collect::<IndexMap<_, _>>())
}

fn opt(value: Option<&str>) -> Value {
    value.map_or(Value::Null, Value::string)
}

fn yes(value: bool) -> Value {
    Value::string(if value { "Y" } else { "N" })
}

fn count(value: usize) -> Value {
    Value::I64(i64::try_from(value).unwrap_or(i64::MAX))
}

fn factory() -> Option<std::sync::Arc<ExecutionContextFactory>> {
    ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory()))
}

fn user_id() -> Option<String> {
    moqui_core::user::current_user_id().ok().flatten().filter(|id| !id.is_empty())
}

fn signed_in() -> Option<String> {
    let user = user_id();
    if user.is_none() {
        add_error("reading the Constitution needs a signed-in user");
    }
    user
}

fn has(permission: &str) -> bool {
    matches!(moqui_core::user::has_permission(permission), Ok(true))
}

fn permitted(permission: &str) -> bool {
    if has(permission) {
        true
    } else {
        add_error(format!("this needs the {permission} permission"));
        false
    }
}

/// The locale asked for, else the user's, normalized.
fn reading_locale(parameters: &Parameters) -> String {
    text(parameters, "locale")
        .and_then(|tag| locale::normalize(&tag))
        .or_else(|| locale::normalize(&moqui_core::l10n::locale()))
        .unwrap_or_else(|| "en".to_string())
}

fn error_list(errors: &[FieldError]) -> Value {
    Value::List(
        errors
            .iter()
            .map(|error| {
                map([
                    ("path", Value::string(&error.path)),
                    ("code", Value::string(error.code)),
                    ("message", Value::string(error.describe())),
                ])
            })
            .collect(),
    )
}

/// The external URLs of `next` that are new or changed against `previous` and not trusted now.
fn origin_errors(next: &Json, previous: Option<&Json>, policy: &Policy) -> Vec<FieldError> {
    let old_urls: Vec<&str> = previous
        .map(|previous| {
            document::rules(previous)
                .iter()
                .flat_map(|rule| document::references(rule).iter())
                .filter_map(|reference| reference.get("url").and_then(Json::as_str))
                .collect()
        })
        .unwrap_or_default();
    let mut errors = Vec::new();
    for (index, rule) in document::rules(next).iter().enumerate() {
        for (ref_index, reference) in document::references(rule).iter().enumerate() {
            let Some(url) = reference.get("url").and_then(Json::as_str) else {
                continue;
            };
            if old_urls.contains(&url) {
                continue;
            }
            if let Ok(UrlKind::External { origin }) = document::check_url(url) {
                if !policy.allows(&origin) {
                    errors.push(FieldError::new(
                        format!("rules[{index}].references[{ref_index}].url"),
                        "originNotTrusted",
                        format!("{origin} is not a trusted reference origin; an instance editor adds it under Origins first"),
                    ));
                }
            }
        }
    }
    errors
}

/// The human reading view of a document in `locale`: document text with the locale each came from, the rules with
/// their descriptions and references (agent instructions only for editors).
fn reading_view(doc: &Json, requested: &str, policy: &Policy, with_instructions: bool) -> BTreeMap<String, Value> {
    let default_locale = document::default_locale(doc);
    let localized = |map: Option<&Json>| -> (Value, Value) {
        match map.and_then(|map| locale::text(map, requested, &default_locale)) {
            Some((text, from)) => (Value::string(text), Value::string(from)),
            None => (Value::Null, Value::Null),
        }
    };
    let (title, title_locale) = localized(doc.get("title"));
    let (provenance, provenance_locale) = localized(doc.get("provenance"));
    let (instructions, instructions_locale) = localized(doc.get("instructions"));
    let mut fallback_count = 0usize;
    let mut note = |from: &Value| {
        let from = to_plain_string(from);
        if from != requested && from != locale::language(requested) {
            fallback_count += 1;
        }
    };
    note(&title_locale);
    let rules: Vec<Value> = document::rules(doc)
        .iter()
        .map(|rule| {
            let (rule_title, rule_title_locale) = localized(rule.get("title"));
            let (description, description_locale) = localized(rule.get("description"));
            note(&rule_title_locale);
            note(&description_locale);
            let references: Vec<Value> = document::references(rule)
                .iter()
                .map(|reference| {
                    let (ref_title, ref_title_locale) = localized(reference.get("title"));
                    let url = reference.get("url").and_then(Json::as_str).unwrap_or("");
                    let (internal, trusted) = match document::check_url(url) {
                        Ok(UrlKind::Internal) => (true, true),
                        Ok(UrlKind::External { origin }) => (false, policy.allows(&origin)),
                        Err(_) => (false, false),
                    };
                    map([
                        ("referenceId", Value::string(document::id_of(reference))),
                        ("title", ref_title),
                        ("titleLocale", ref_title_locale),
                        ("url", Value::string(url)),
                        ("internal", yes(internal)),
                        ("trusted", yes(trusted)),
                    ])
                })
                .collect();
            let mut entries = vec![
                ("ruleId", Value::string(document::id_of(rule))),
                ("title", rule_title),
                ("titleLocale", rule_title_locale),
                ("description", description),
                ("descriptionLocale", description_locale),
                ("referenceCount", count(references.len())),
                ("referenceList", Value::List(references)),
            ];
            if with_instructions {
                let (rule_instructions, rule_instructions_locale) = localized(rule.get("instructions"));
                entries.push(("instructions", rule_instructions));
                entries.push(("instructionsLocale", rule_instructions_locale));
            }
            map(entries)
        })
        .collect();
    let locales: Vec<Value> = document::locales(doc).into_iter().map(Value::string).collect();
    let mut result = out([
        ("locale", Value::string(requested)),
        ("defaultLocale", Value::string(&default_locale)),
        ("localeList", Value::List(locales)),
        ("title", title),
        ("titleLocale", title_locale),
        ("provenance", provenance),
        ("provenanceLocale", provenance_locale),
        ("ruleCount", count(rules.len())),
        ("ruleList", Value::List(rules)),
        ("fallbackCount", count(fallback_count)),
    ]);
    if with_instructions {
        result.insert("instructions".to_string(), instructions);
        result.insert("instructionsLocale".to_string(), instructions_locale);
    }
    result
}

/// The hash a store or reset of the global document must name: the instance file's document, the raw file's SHA-256
/// (`file:` and hex) when the file is invalid, else the shipped default's.
fn global_hash(factory: &ExecutionContextFactory) -> Result<String, String> {
    let instance = resolve::instance_path(factory);
    match resolve::read_file(&instance) {
        Ok(Some((_, hash))) => Ok(hash),
        Ok(None) => resolve::read_file(&resolve::shipped_path(factory))?
            .map(|(_, hash)| hash)
            .ok_or_else(|| "the shipped Constitution is missing".to_string()),
        Err(_) => {
            use sha2::Digest as _;
            let bytes = std::fs::read(&instance).unwrap_or_default();
            Ok(format!("file:{}", moqui_lib::codec::hex_encode(&sha2::Sha256::digest(&bytes))))
        }
    }
}

fn scope_of<'a>(parameters: &'a Parameters, user_id: &'a str, scope_text: Option<&'a str>, org: Option<&'a str>) -> Scope<'a> {
    let _ = parameters;
    match (scope_text, org) {
        (Some("shipped"), _) => Scope::Shipped,
        (Some("global"), _) => Scope::Global,
        (_, Some(party_id)) => Scope::Organization { user_id, party_id },
        _ => Scope::ActiveOf(user_id),
    }
}

fn resolved_fields(resolved: &Resolved) -> Vec<(&'static str, Value)> {
    vec![
        ("source", Value::string(resolved.source.as_str())),
        ("organizationPartyId", opt(resolved.organization_id.as_deref())),
        ("organizationName", opt(resolved.organization_name.as_deref())),
        ("contentHash", Value::string(&resolved.hash)),
        ("diagnosticList", Value::List(resolved.diagnostics.iter().map(Value::string).collect())),
    ]
}

/// `get#Constitution`: the document that applies (the user's active organization's, else the global one), or with
/// `scope` global or shipped that one, or with `organizationPartyId` that organization's when the user may read it;
/// localized for reading in `locale`. `includeDocument` adds the whole JSON. Out also `globalHash` (what a global store or reset names), `customized`, `canEdit`.
fn get_constitution(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let Some(user_id) = signed_in() else {
        return Ok(BTreeMap::new());
    };
    let Some(factory) = factory() else {
        return Ok(BTreeMap::new());
    };
    let scope_text = text(parameters, "scope");
    let org = text(parameters, "organizationPartyId");
    let scope = scope_of(parameters, &user_id, scope_text.as_deref(), org.as_deref());
    let can_edit = has(GLOBAL_PERMISSION);
    let policy = origins::load(factory.runtime_dir());
    let customized = resolve::instance_path(&factory).exists();
    let mut result = match resolve::resolve(&factory, scope) {
        Ok(resolved) => {
            let requested = reading_locale(parameters);
            let mut result = reading_view(&resolved.document, &requested, &policy, can_edit);
            result.extend(resolved_fields(&resolved).into_iter().map(|(key, value)| (key.to_string(), value)));
            // the document is readable by whoever may read it here; editors start their changes from it
            if flag(parameters, "includeDocument") {
                result.insert("documentText".to_string(), Value::string(document::to_pretty(&resolved.document)));
            }
            result.insert("available".to_string(), yes(true));
            result
        }
        Err(diagnostics) => out([
            ("available", yes(false)),
            ("diagnosticList", Value::List(diagnostics.into_iter().map(Value::string).collect())),
        ]),
    };
    result.insert("customized".to_string(), yes(customized));
    result.insert("canEdit".to_string(), yes(can_edit));
    result.insert("globalHash".to_string(), opt(global_hash(&factory).ok().as_deref()));
    result.insert("originsTrusted".to_string(), Value::List(policy.origins.iter().map(Value::string).collect()));
    Ok(result)
}

/// `validate#Constitution`: the problems of `documentText` (with `previousText`, also the new or changed external URLs
/// of untrusted origins). Out `valid`, `errorList` and the document's `contentHash` when it is JSON; nothing is
/// written.
fn validate_constitution(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let Some(factory) = factory() else {
        return Ok(BTreeMap::new());
    };
    let mut content_hash = None;
    let errors = match document::parse(&text(parameters, "documentText").unwrap_or_default()) {
        Ok(doc) => {
            content_hash = Some(document::hash(&doc));
            let mut errors = document::validate(&doc);
            let previous = text(parameters, "previousText").and_then(|text| document::parse(&text).ok());
            errors.extend(origin_errors(&doc, previous.as_ref(), &origins::load(factory.runtime_dir())));
            errors
        }
        Err(error) => vec![error],
    };
    Ok(out([
        ("valid", yes(errors.is_empty())),
        ("errorList", error_list(&errors)),
        ("contentHash", opt(content_hash.as_deref())),
    ]))
}

/// `edit#ConstitutionDocument`: one edit (`operation` and its parameters) of `documentText`; nothing is stored. Out the
/// new `documentText`, `newId` for an added rule or reference, and the new document's `errorList`.
fn edit_constitution_document(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let doc = match document::parse(&text(parameters, "documentText").unwrap_or_default()) {
        Ok(doc) => doc,
        Err(error) => {
            add_error(error.describe());
            return Ok(BTreeMap::new());
        }
    };
    let operation = text(parameters, "operation").unwrap_or_default();
    let get = |name: &str| match name {
        "title" | "description" | "instructions" | "provenance" | "url" => raw(parameters, name),
        _ => text(parameters, name),
    };
    let applied = Edit::from_parameters(&operation, &get).and_then(|edit| edit::apply(&doc, &edit));
    match applied {
        Ok(applied) => {
            let errors = document::validate(&applied.document);
            Ok(out([
                ("documentText", Value::string(document::to_pretty(&applied.document))),
                ("newId", opt(applied.new_id.as_deref())),
                ("errorList", error_list(&errors)),
            ]))
        }
        Err(message) => {
            add_error(message);
            Ok(BTreeMap::new())
        }
    }
}

/// `preview#ConstitutionPrompt`: the system-prompt section of `documentText` (else of the document that applies) in
/// `locale`: its text, the fields that fell back to another locale, and its size.
fn preview_constitution_prompt(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let Some(user_id) = signed_in() else {
        return Ok(BTreeMap::new());
    };
    let Some(factory) = factory() else {
        return Ok(BTreeMap::new());
    };
    let doc = match text(parameters, "documentText") {
        Some(text) => match resolve::checked(&text) {
            Ok((doc, _)) => doc,
            Err(problem) => {
                add_error(format!("the Constitution cannot be previewed: {problem}"));
                return Ok(BTreeMap::new());
            }
        },
        None => {
            let scope_text = text(parameters, "scope");
            let org = text(parameters, "organizationPartyId");
            match resolve::resolve(&factory, scope_of(parameters, &user_id, scope_text.as_deref(), org.as_deref())) {
                Ok(resolved) => resolved.document,
                Err(diagnostics) => {
                    add_error(diagnostics.join(" "));
                    return Ok(BTreeMap::new());
                }
            }
        }
    };
    let rendered = render::prompt_section(&doc, &reading_locale(parameters));
    let fallbacks: Vec<Value> = rendered
        .fallbacks
        .iter()
        .map(|(path, from)| map([("path", Value::string(path)), ("locale", Value::string(from))]))
        .collect();
    Ok(out([
        ("promptText", Value::string(&rendered.text)),
        ("locale", Value::string(&rendered.locale)),
        ("fallbackList", Value::List(fallbacks)),
        ("characters", count(rendered.text.chars().count())),
        ("bytes", count(rendered.text.len())),
    ]))
}

/// Record a change of a document: the before and after documents kept as snapshots, and a line in the audit log
/// naming the actor, the scope and both hashes.
pub(crate) fn record(facade: &EntityFacade, action: &str, scope: &str, before: Option<&Json>, after: Option<&Json>) {
    let hash_of = |doc: Option<&Json>| {
        doc.map(|doc| {
            let hash = document::hash(doc);
            snapshot::keep(facade, &hash, doc);
            hash
        })
    };
    let before_hash = hash_of(before);
    let after_hash = hash_of(after);
    let actor = user_id().unwrap_or_else(|| "(none)".to_string());
    tracing::warn!(
        target: moqui_core::live_settings::AUDIT,
        "Constitution {action} of {scope} by user {actor}: {} to {}",
        before_hash.as_deref().unwrap_or("(none)"),
        after_hash.as_deref().unwrap_or("(none)")
    );
}

/// `store#Constitution`: save `documentText` as the instance customization, when `expectedHash` is the global
/// document's hash now (`globalHash` of `get#Constitution`). Refuses an invalid document, a new or changed URL of an
/// untrusted origin, or a stale hash, leaving the file as it was. Out the new `contentHash`.
fn store_constitution(parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !permitted(GLOBAL_PERMISSION) {
        return Ok(BTreeMap::new());
    }
    let Some(factory) = factory() else {
        return Ok(BTreeMap::new());
    };
    let doc = match document::parse(&text(parameters, "documentText").unwrap_or_default()) {
        Ok(doc) => doc,
        Err(error) => {
            add_error(error.describe());
            return Ok(out([("errorList", error_list(&[error]))]));
        }
    };
    let current = global_hash(&factory).unwrap_or_default();
    if text(parameters, "expectedHash").as_deref() != Some(current.as_str()) {
        add_error("the Constitution changed since it was read; reload it and make the change again");
        return Ok(BTreeMap::new());
    }
    let previous = resolve::resolve(&factory, Scope::Global).ok().map(|resolved| resolved.document);
    let mut errors = document::validate(&doc);
    errors.extend(origin_errors(&doc, previous.as_deref(), &origins::load(factory.runtime_dir())));
    if !errors.is_empty() {
        for error in &errors {
            add_error(error.describe());
        }
        return Ok(out([("errorList", error_list(&errors))]));
    }
    if let Err(problem) = super::write_atomic(&resolve::instance_path(&factory), &document::to_pretty(&doc)) {
        add_error(format!("the Constitution could not be saved: {problem}"));
        return Ok(BTreeMap::new());
    }
    record(facade, "store", "global", previous.as_deref(), Some(&doc));
    Ok(out([("contentHash", Value::string(document::hash(&doc)))]))
}

/// `create#ConstitutionCustomization`: start the instance customization as a copy of the shipped default. Refused
/// when there is one already.
fn create_constitution_customization(_parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !permitted(GLOBAL_PERMISSION) {
        return Ok(BTreeMap::new());
    }
    let Some(factory) = factory() else {
        return Ok(BTreeMap::new());
    };
    let instance = resolve::instance_path(&factory);
    if instance.exists() {
        add_error("this instance has its own Constitution already");
        return Ok(BTreeMap::new());
    }
    let shipped = match resolve::read_file(&resolve::shipped_path(&factory)) {
        Ok(Some((doc, _))) => doc,
        Ok(None) => {
            add_error("the shipped Constitution is missing");
            return Ok(BTreeMap::new());
        }
        Err(problem) => {
            add_error(format!("the shipped Constitution cannot be used: {problem}"));
            return Ok(BTreeMap::new());
        }
    };
    if let Err(problem) = super::write_atomic(&instance, &document::to_pretty(&shipped)) {
        add_error(format!("the Constitution could not be saved: {problem}"));
        return Ok(BTreeMap::new());
    }
    record(facade, "customize", "global", None, Some(&shipped));
    Ok(out([("contentHash", Value::string(document::hash(&shipped)))]))
}

/// `reset#Constitution`: remove the instance customization (the shipped default applies again), when `expectedHash`
/// is its hash now. The trusted origins stay as they are.
fn reset_constitution(parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !permitted(GLOBAL_PERMISSION) {
        return Ok(BTreeMap::new());
    }
    let Some(factory) = factory() else {
        return Ok(BTreeMap::new());
    };
    let instance = resolve::instance_path(&factory);
    if !instance.exists() {
        add_error("this instance uses the shipped Constitution already");
        return Ok(BTreeMap::new());
    }
    let current = global_hash(&factory).unwrap_or_default();
    if text(parameters, "expectedHash").as_deref() != Some(current.as_str()) {
        add_error("the Constitution changed since it was read; reload it and reset again");
        return Ok(BTreeMap::new());
    }
    let before = resolve::read_file(&instance).ok().flatten().map(|(doc, _)| doc);
    if let Err(error) = std::fs::remove_file(&instance) {
        add_error(format!("the instance Constitution could not be removed: {error}"));
        return Ok(BTreeMap::new());
    }
    let after = resolve::read_file(&resolve::shipped_path(&factory)).ok().flatten().map(|(doc, _)| doc);
    record(facade, "reset", "global", before.as_deref(), after.as_deref());
    Ok(out([("contentHash", opt(after.map(|doc| document::hash(&doc)).as_deref()))]))
}

/// `record#ConstitutionChange` (internal): what an organization's store or reset records (Mantle calls it after its
/// write): `action`, `organizationPartyId`, and the documents `beforeText` and `afterText`.
fn record_constitution_change(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let parse = |key: &str| text(parameters, key).and_then(|text| document::parse(&text).ok());
    let scope = text(parameters, "organizationPartyId").map_or_else(|| "global".to_string(), |org| format!("organization {org}"));
    let action = text(parameters, "action").unwrap_or_else(|| "change".to_string());
    record(facade, &action, &scope, parse("beforeText").as_ref(), parse("afterText").as_ref());
    Ok(BTreeMap::new())
}

/// The run's document: the run must be the user's (or the user an AgentAdmin), and its snapshot readable.
fn run_document(facade: &EntityFacade, run_id: &str, user_id: &str) -> Result<(RunConstitution, std::sync::Arc<Json>), String> {
    let row = crate::run::quietly(|| facade.find("moqui.agent.AgentRun").and_then(|find| find.eq("runId", Value::string(run_id)).disable_authz().one()))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("there is no run {run_id}"))?;
    let owner = crate::text_of(&row, "userId").unwrap_or_default();
    if owner != user_id && !has("AgentAdmin") {
        return Err(format!("run {run_id} is not yours"));
    }
    let picked = RunConstitution::of_run(&row).ok_or_else(|| format!("run {run_id} has no Constitution"))?;
    let hash = picked.hash.clone().ok_or_else(|| format!("run {run_id} ran without a Constitution"))?;
    let doc = crate::run::quietly(|| snapshot::load(facade, &hash))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("the Constitution of run {run_id} is no longer stored"))?;
    Ok((picked, doc))
}

/// Whether the user may still read the organization's document a run used (the provider answers for it).
fn still_readable(factory: &ExecutionContextFactory, picked: &RunConstitution, user_id: &str) -> bool {
    match (picked.source.as_str(), picked.organization_id.as_deref()) {
        ("organization", Some(party_id)) => resolve::resolve(factory, Scope::Organization { user_id, party_id })
            .map(|resolved| resolved.organization_id.as_deref() == Some(party_id))
            .unwrap_or(false),
        _ => true,
    }
}

/// `get#ConstitutionReferences` (READ, an agent tool): the references of `ruleIds` (all when none), each rule's in
/// order with their localized titles and exact URLs. In an agent run, from the document that run uses; otherwise from
/// the document that applies to the user. An unknown rule id is an error, never a guessed link.
fn get_constitution_references(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(user_id) = signed_in() else {
        return Ok(BTreeMap::new());
    };
    let Some(factory) = factory() else {
        return Ok(BTreeMap::new());
    };
    let (doc, hash, source, org, requested) = if let Some(run_id) = current_run() {
        match run_document(facade, &run_id, &user_id) {
            Ok((picked, doc)) => {
                let hash = picked.hash.clone().unwrap_or_default();
                (doc, hash, picked.source.clone(), picked.organization_id.clone(), picked.locale.clone())
            }
            Err(problem) => {
                add_error(problem);
                return Ok(BTreeMap::new());
            }
        }
    } else {
        match resolve::resolve(&factory, Scope::ActiveOf(&user_id)) {
            Ok(resolved) => {
                let requested = reading_locale(parameters);
                (resolved.document, resolved.hash, resolved.source.as_str().to_string(), resolved.organization_id, requested)
            }
            Err(diagnostics) => {
                add_error(diagnostics.join(" "));
                return Ok(BTreeMap::new());
            }
        }
    };
    let wanted = list(parameters, "ruleIds");
    let rules = document::rules(&doc);
    let unknown: Vec<&String> = wanted.iter().filter(|id| !rules.iter().any(|rule| document::id_of(rule) == id.as_str())).collect();
    if !unknown.is_empty() {
        let names: Vec<&str> = unknown.iter().map(|id| id.as_str()).collect();
        add_error(format!("the Constitution has no rule {}", names.join(", ")));
        return Ok(BTreeMap::new());
    }
    let default_locale = document::default_locale(&doc);
    let rule_list: Vec<Value> = rules
        .iter()
        .filter(|rule| wanted.is_empty() || wanted.iter().any(|id| id == document::id_of(rule)))
        .map(|rule| {
            let title = rule.get("title").and_then(|map| locale::text(map, &requested, &default_locale)).map(|(text, _)| text);
            let references: Vec<Value> = document::references(rule)
                .iter()
                .map(|reference| {
                    let (ref_title, from) = reference
                        .get("title")
                        .and_then(|map| locale::text(map, &requested, &default_locale))
                        .map_or((Value::Null, Value::Null), |(text, from)| (Value::string(text), Value::string(from)));
                    map([
                        ("referenceId", Value::string(document::id_of(reference))),
                        ("title", ref_title),
                        ("titleLocale", from),
                        ("url", Value::string(reference.get("url").and_then(Json::as_str).unwrap_or(""))),
                    ])
                })
                .collect();
            map([
                ("ruleId", Value::string(document::id_of(rule))),
                ("title", opt(title)),
                ("referenceList", Value::List(references)),
            ])
        })
        .collect();
    Ok(out([
        ("contentHash", Value::string(hash)),
        ("source", Value::string(source)),
        ("organizationPartyId", opt(org.as_deref())),
        ("locale", Value::string(requested)),
        ("ruleList", Value::List(rule_list)),
    ]))
}

/// `get#ConstitutionSnapshot`: a run's document for reading ("Rules used for this run"), for the run's user or an
/// AgentAdmin, while the user may still read its organization's document.
fn get_constitution_snapshot(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(user_id) = signed_in() else {
        return Ok(BTreeMap::new());
    };
    let Some(factory) = factory() else {
        return Ok(BTreeMap::new());
    };
    let Some(run_id) = text(parameters, "runId") else {
        add_error("get#ConstitutionSnapshot needs runId");
        return Ok(BTreeMap::new());
    };
    let (picked, doc) = match run_document(facade, &run_id, &user_id) {
        Ok(found) => found,
        Err(problem) => {
            add_error(problem);
            return Ok(BTreeMap::new());
        }
    };
    if !still_readable(&factory, &picked, &user_id) {
        add_error("you may no longer read the Constitution this run used");
        return Ok(BTreeMap::new());
    }
    let requested = reading_locale(parameters);
    let mut result = reading_view(&doc, &requested, &origins::load(factory.runtime_dir()), false);
    result.insert("runId".to_string(), Value::string(&run_id));
    result.insert("source".to_string(), Value::string(&picked.source));
    result.insert("organizationPartyId".to_string(), opt(picked.organization_id.as_deref()));
    result.insert("contentHash".to_string(), opt(picked.hash.as_deref()));
    result.insert("runLocale".to_string(), Value::string(&picked.locale));
    result.insert("available".to_string(), yes(true));
    Ok(result)
}

/// `resolve#ConstitutionReference`: the URL and title of a reference for the dialog, checked at each open: it must be
/// in the document (the run's with `runId`, else the one that applies), the user must still be able to read that
/// document, and an external URL's origin must be trusted now. Out `allowed` and, when not, `reasonCode`
/// (`unknownReference`, `notReadable`, `originNotAllowed`, `invalidUrl`).
fn resolve_constitution_reference(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(user_id) = signed_in() else {
        return Ok(BTreeMap::new());
    };
    let Some(factory) = factory() else {
        return Ok(BTreeMap::new());
    };
    let refused = |reason: &str, message: &str| out([("allowed", yes(false)), ("reasonCode", Value::string(reason)), ("message", Value::string(message))]);
    let Some(reference_id) = text(parameters, "referenceId") else {
        return Ok(refused("unknownReference", "No reference was named."));
    };
    let doc = if let Some(run_id) = text(parameters, "runId") {
        match run_document(facade, &run_id, &user_id) {
            Ok((picked, doc)) => {
                if !still_readable(&factory, &picked, &user_id) {
                    return Ok(refused("notReadable", "You may no longer read the Constitution this reference is from."));
                }
                doc
            }
            Err(_) => return Ok(refused("notReadable", "The Constitution this reference is from cannot be read.")),
        }
    } else {
        let org = text(parameters, "organizationPartyId");
        let scope = match org.as_deref() {
            Some(party_id) => Scope::Organization { user_id: &user_id, party_id },
            None => Scope::ActiveOf(&user_id),
        };
        match resolve::resolve(&factory, scope) {
            Ok(resolved) => resolved.document,
            Err(_) => return Ok(refused("notReadable", "No Constitution is available.")),
        }
    };
    let found = document::rules(&doc).iter().find_map(|rule| {
        document::references(rule)
            .iter()
            .find(|reference| document::id_of(reference) == reference_id)
            .map(|reference| (document::id_of(rule).to_string(), reference.clone()))
    });
    let Some((rule_id, reference)) = found else {
        return Ok(refused("unknownReference", "This reference is not in the Constitution."));
    };
    let url = reference.get("url").and_then(Json::as_str).unwrap_or("").to_string();
    let default_locale = document::default_locale(&doc);
    let requested = reading_locale(parameters);
    let title = reference.get("title").and_then(|map| locale::text(map, &requested, &default_locale)).map(|(text, _)| text.to_string());
    let (internal, origin) = match document::check_url(&url) {
        Ok(UrlKind::Internal) => (true, None),
        Ok(UrlKind::External { origin }) => {
            if !origins::load(factory.runtime_dir()).allows(&origin) {
                return Ok(refused("originNotAllowed", &format!("{origin} is not trusted for reading in the app.")));
            }
            (false, Some(origin))
        }
        Err(_) => return Ok(refused("invalidUrl", "This reference's URL is not valid.")),
    };
    Ok(out([
        ("allowed", yes(true)),
        ("referenceId", Value::string(&reference_id)),
        ("ruleId", Value::string(rule_id)),
        ("title", opt(title.as_deref())),
        ("url", Value::string(&url)),
        ("internal", yes(internal)),
        ("origin", opt(origin.as_deref())),
    ]))
}

/// `get#ReferenceOrigins`: the trusted reference origins, their revision, and whether the user may change them.
fn get_reference_origins(_parameters: &Parameters, _facade: &EntityFacade) -> Out {
    if signed_in().is_none() {
        return Ok(BTreeMap::new());
    }
    let Some(factory) = factory() else {
        return Ok(BTreeMap::new());
    };
    let policy = origins::load(factory.runtime_dir());
    Ok(out([
        ("originList", Value::List(policy.origins.iter().map(Value::string).collect())),
        ("revision", Value::string(&policy.revision)),
        ("customized", yes(policy.customized)),
        ("problem", opt(policy.problem.as_deref())),
        ("canEdit", yes(has(GLOBAL_PERMISSION))),
    ]))
}

/// `store#ReferenceOrigins`: replace the trusted origins with `origins` (a list, or text with one a line) when
/// `expectedRevision` is the current revision. Exact `https://` origins, no wildcards, the default port.
fn store_reference_origins(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    if !permitted(GLOBAL_PERMISSION) {
        return Ok(BTreeMap::new());
    }
    let Some(factory) = factory() else {
        return Ok(BTreeMap::new());
    };
    let before = origins::load(factory.runtime_dir());
    let wanted = list(parameters, "origins");
    match origins::store(factory.runtime_dir(), &wanted, &text(parameters, "expectedRevision").unwrap_or_default()) {
        Ok(policy) => {
            let actor = user_id().unwrap_or_default();
            tracing::warn!(
                target: moqui_core::live_settings::AUDIT,
                "Constitution reference origins changed by user {actor}: [{}] to [{}]",
                before.origins.join(", "),
                policy.origins.join(", ")
            );
            Ok(out([("revision", Value::string(&policy.revision))]))
        }
        Err(problem) => {
            add_error(problem);
            Ok(BTreeMap::new())
        }
    }
}
