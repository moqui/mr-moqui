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

//! The lazy catalog (plans/LLM-A2A-Assist.md 4.3, 4.6, 7.3): the tools a run reaches only through `find_tools`,
//! `describe_tools` and `run_tool`, so the visible tool list of a conversation never changes.
//!
//! - **Service tools**: only the services an `AgentToolExposure` row opts in (`allow-remote` plays no part). The row
//!   gives the tool name, a title and description the model reads, a schema that replaces the one made from the
//!   service's parameters (as the OpenAPI document makes it), a classification that replaces the service's own, whether
//!   the assistant (`exposeAssistant`) and the MCP server (`exposeMcp`, phase 13.11) offer it, the preview mode of its
//!   writes and a `lockVersion` each edit moves on. The service's own class comes from its authorization action: view
//!   is READ, delete is DESTRUCTIVE, anything else WRITE, and a service declared `read-only="true"` is READ
//!   ([`service_classification`]). A READ runs fenced, so the claim is checked as it runs.
//! - **Add-on tools** that set [`super::ToolSpec::lazy`] and that the agent does not pin.
//! - **MCP client tools** (`mcp__<server>__<tool>`, [`super::mcp_client`]) and **A2A delegates** (`a2a__<remote>`,
//!   [`super::delegate`]), from the admin's rows for the run's agent ([`Catalog::with_remotes`]); their prefixes are
//!   reserved, so no exposure may take them ([`reserved`]). They are EXTERNAL unless an admin marks an MCP tool
//!   otherwise, so they ask for approvals as writes do.
//!
//! A run's catalog ([`Catalog::for_run`]) keeps every entry with whether the agent may run it (its policy for the
//! entry's name and class, its role group for the service), so `find_tools` lists what it may run and `run_tool` says
//! why it may not run the rest. An exposure that does not read, or names a service that is not defined, is logged and
//! left out; the others stay.

use std::sync::Arc;

use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use super::{registered, AgentTool, Classification, ClientCaps, ToolPolicy};
use crate::entity::{EntityFacade, EntityValue};
use crate::llm::approval::PreviewMode;
use crate::llm::{flag_of, int_of, text_of};
use crate::service::call::ToolService;

pub(crate) const EXPOSURE: &str = "moqui.agent.AgentToolExposure";

/// Name prefixes kept for the MCP client tools and the A2A delegates.
pub const RESERVED_PREFIXES: [&str; 2] = ["mcp__", "a2a__"];

/// The most entries `find_tools` lists.
pub const FIND_MAX: usize = 8;

/// Whether `name` takes a prefix kept for MCP client tools or A2A delegates.
pub fn reserved(name: &str) -> bool {
    RESERVED_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

/// One `AgentToolExposure` row.
#[derive(Clone, Debug, PartialEq)]
pub struct Exposure {
    pub tool_name: String,
    pub service_name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    /// A JSON schema object that replaces the one made from the service's parameters.
    pub schema_override: Option<Json>,
    /// The admin's class, over the service's own.
    pub classification: Option<Classification>,
    pub expose_assistant: bool,
    pub expose_mcp: bool,
    pub preview: PreviewMode,
    pub lock_version: i64,
    pub disabled: bool,
}

impl Exposure {
    /// The row as an exposure, or why it does not read.
    ///
    /// # Errors
    ///
    /// What is wrong with the row: no tool or service name, a reserved name, an unknown class or preview mode, a
    /// schema override that is not a JSON object.
    pub fn of(row: &EntityValue) -> Result<Self, String> {
        let tool_name = text_of(row, "toolName").ok_or("it has no toolName")?;
        if reserved(&tool_name) {
            return Err(format!("{tool_name} takes a prefix kept for MCP and A2A tools"));
        }
        let service_name = text_of(row, "serviceName").ok_or("it has no serviceName")?;
        let classification = match text_of(row, "classificationEnumId") {
            None => None,
            Some(text) => Some(Classification::parse(&text).ok_or(format!("{text} is not a classification"))?),
        };
        let preview = match text_of(row, "previewMode") {
            None => PreviewMode::OnDemand,
            Some(text) => PreviewMode::parse(&text).ok_or(format!("{text} is not none, on-demand or always"))?,
        };
        let schema_override = match text_of(row, "schemaOverrideJson") {
            None => None,
            Some(text) => match serde_json::from_str::<Json>(&text) {
                Ok(schema) if schema.is_object() => Some(schema),
                Ok(_) => return Err("its schemaOverrideJson is not a JSON object".to_string()),
                Err(error) => return Err(format!("its schemaOverrideJson does not read: {error}")),
            },
        };
        Ok(Self {
            tool_name,
            service_name,
            title: text_of(row, "title"),
            description: text_of(row, "description"),
            schema_override,
            classification,
            // a row opts a service in for the assistant unless it says otherwise
            expose_assistant: flag_of(row, "exposeAssistant") != Some(false),
            expose_mcp: flag_of(row, "exposeMcp") == Some(true),
            preview,
            lock_version: int_of(row, "lockVersion").unwrap_or(0),
            disabled: flag_of(row, "disabled") == Some(true),
        })
    }
}

/// The exposure rows that read, by tool name; one that does not is logged and left out.
///
/// # Errors
///
/// Returns an error when the rows cannot be read.
pub fn exposures(facade: &EntityFacade) -> Result<Vec<Exposure>, moqui_lib::error::Error> {
    let rows = facade
        .find(EXPOSURE)?
        .order_by("toolName", false)
        .use_cache(true)
        .disable_authz()
        .list()?;
    Ok(rows
        .values()
        .iter()
        .filter_map(|row| match Exposure::of(row) {
            Ok(exposure) => Some(exposure),
            Err(why) => {
                let name = text_of(row, "toolName").unwrap_or_default();
                tracing::error!("the tool exposure {name} is left out of the catalog: {why}");
                None
            }
        })
        .collect())
}

/// Whether the current user may run `service` as a tool call, which has no screen above it to inherit from: the
/// service itself when it needs authorization, and for an entity-auto service the entity with the verb's action (an
/// implicit one is not authorized itself, its entity operation is).
///
/// # Errors
///
/// The authorization error, naming the user, the artifact and the action.
pub(crate) fn user_permitted(service: &ToolService) -> Result<(), crate::context::Error> {
    if service.needs_authz() {
        crate::user::authorize("AT_SERVICE", service.name(), service.authz_action())?;
    }
    if let Some(entity) = service.entity_noun() {
        let action = match service.verb() {
            "create" => "create",
            "delete" => "delete",
            _ => "update",
        };
        crate::user::authorize("AT_ENTITY", &entity, action)?;
    }
    Ok(())
}

/// The class of a service as a tool: READ when it is declared `read-only`, else by its authorization action (view is
/// READ, delete is DESTRUCTIVE, anything else is a WRITE).
pub(crate) fn service_classification(service: &ToolService) -> Classification {
    if service.read_only() {
        return Classification::Read;
    }
    match service.authz_action() {
        "view" => Classification::Read,
        "delete" => Classification::Destructive,
        _ => Classification::Write,
    }
}

/// What an entry runs.
#[derive(Clone)]
pub enum Target {
    Service(Exposure),
    AddOn(Arc<dyn AgentTool>),
}

/// One tool of the catalog.
#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub title: String,
    pub description: String,
    pub classification: Classification,
    pub target: Target,
    /// The agent may run it: its policy offers the entry's name in its class, and its role group permits the service.
    pub allowed: bool,
    /// Why not, when it may not.
    pub refusal: Option<String>,
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry")
            .field("name", &self.name)
            .field("classification", &self.classification)
            .field("allowed", &self.allowed)
            .finish_non_exhaustive()
    }
}

impl Entry {
    pub fn service_name(&self) -> Option<&str> {
        match &self.target {
            Target::Service(exposure) => Some(&exposure.service_name),
            Target::AddOn(_) => None,
        }
    }

    pub fn exposure(&self) -> Option<&Exposure> {
        match &self.target {
            Target::Service(exposure) => Some(exposure),
            Target::AddOn(_) => None,
        }
    }

    /// `read` or `write` (a DESTRUCTIVE or EXTERNAL one says so), as `find_tools` lists it.
    pub fn class_word(&self) -> &'static str {
        match self.classification {
            Classification::Read => "read",
            Classification::Write => "write",
            Classification::Destructive => "write, destructive",
            Classification::External => "write, external",
        }
    }

    /// One line: the name, its class and what it does.
    pub fn line(&self) -> String {
        let what = if self.description.is_empty() {
            &self.title
        } else {
            &self.description
        };
        let what = what.split(['\n', '\r']).next().unwrap_or_default().trim();
        format!(
            "{} ({}): {}",
            self.name,
            self.class_word(),
            super::format::cut(what, 160)
        )
    }

    /// The JSON schema of its parameters: the exposure's override, else the service's in-parameters, else the add-on
    /// tool's own.
    ///
    /// # Errors
    ///
    /// Why it cannot be made (the service's parameters do not expand).
    pub fn schema(&self, facade: &EntityFacade) -> Result<Json, String> {
        match &self.target {
            Target::Service(exposure) => {
                if let Some(schema) = &exposure.schema_override {
                    return Ok(schema.clone());
                }
                let service = ToolService::of(&exposure.service_name).map_err(|error| error.to_string())?;
                service
                    .input_schema(Some(facade))
                    .map(|schema| moqui_lib::json::to_json(&schema))
                    .map_err(|error| error.to_string())
            }
            Target::AddOn(tool) => Ok(tool.spec().parameters.clone()),
        }
    }

    /// The JSON schema of its results, when it has one (a service's out-parameters).
    pub fn results(&self, facade: &EntityFacade) -> Option<Json> {
        match &self.target {
            Target::Service(exposure) => ToolService::of(&exposure.service_name)
                .ok()
                .map(|service| moqui_lib::json::to_json(&service.output_schema(Some(facade)))),
            Target::AddOn(_) => None,
        }
    }

    /// Whether the user may run it ([`user_permitted`]); an add-on tool checks its own when it runs.
    pub fn user_may_run(&self) -> bool {
        let Some(name) = self.service_name() else {
            return true;
        };
        ToolService::of(name).is_ok_and(|service| user_permitted(&service).is_ok())
    }
}

/// The catalog of a run, sorted by name.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub entries: Vec<Entry>,
}

impl Catalog {
    /// The catalog a run of an agent sees: the exposures the assistant offers whose service is defined, and the lazy
    /// add-on tools whose capabilities `caps` has and that `policy` does not pin; each marked with whether `policy`
    /// offers its name in its class and `role_groups` permit its service. A row or a permission check that cannot be
    /// read is logged and left out.
    pub fn for_run(facade: &EntityFacade, caps: &ClientCaps, policy: &ToolPolicy, role_groups: &[String]) -> Self {
        Self::of(facade, false, caps, policy, role_groups)
    }

    /// [`Catalog::for_run`] of the MCP server: the exposures the MCP server offers (`exposeMcp`) instead of the
    /// assistant's, so its allowlist is separate.
    pub fn for_mcp(facade: &EntityFacade, caps: &ClientCaps, policy: &ToolPolicy, role_groups: &[String]) -> Self {
        Self::of(facade, true, caps, policy, role_groups)
    }

    fn of(facade: &EntityFacade, mcp: bool, caps: &ClientCaps, policy: &ToolPolicy, role_groups: &[String]) -> Self {
        let mut entries = Vec::new();
        let rows = crate::llm::run::quietly(|| exposures(facade)).unwrap_or_else(|error| {
            tracing::error!("the tool exposures could not be read, so the catalog has no services: {error}");
            Vec::new()
        });
        for exposure in rows {
            let offered = if mcp {
                exposure.expose_mcp
            } else {
                exposure.expose_assistant
            };
            if exposure.disabled || !offered {
                continue;
            }
            let service = match ToolService::of(&exposure.service_name) {
                Ok(service) => service,
                Err(error) => {
                    tracing::error!(
                        "the tool {} is left out of the catalog: its service {} is not usable: {error}",
                        exposure.tool_name,
                        exposure.service_name
                    );
                    continue;
                }
            };
            let classification = exposure
                .classification
                .unwrap_or_else(|| service_classification(&service));
            let mut refusal = policy_refusal(policy, &exposure.tool_name, classification);
            if refusal.is_none() && !role_groups.is_empty() {
                match crate::user::group_permits(role_groups, "AT_SERVICE", service.name(), service.authz_action()) {
                    Ok(true) => {}
                    Ok(false) => refusal = Some("this agent's role does not permit it".to_string()),
                    Err(error) => {
                        tracing::error!(
                            "whether the role group may run {} could not be read, so it is refused: {error}",
                            service.name()
                        );
                        refusal = Some("whether this agent may run it could not be checked".to_string());
                    }
                }
            }
            entries.push(Entry {
                name: exposure.tool_name.clone(),
                title: exposure.title.clone().unwrap_or_default(),
                description: exposure.description.clone().unwrap_or_default(),
                classification,
                allowed: refusal.is_none(),
                refusal,
                target: Target::Service(exposure),
            });
        }
        for tool in registered() {
            let spec = tool.spec();
            if !spec.lazy
                || policy.pins(&spec.name)
                || !spec.needs.iter().all(|need| caps.has(need))
                || !(spec.needs_any.is_empty() || spec.needs_any.iter().any(|need| caps.has(need)))
                || entries.iter().any(|entry: &Entry| entry.name == spec.name)
            {
                continue;
            }
            let mut refusal = policy_refusal(policy, &spec.name, spec.classification);
            if let (Some(artifact), None, false) = (&spec.artifact, &refusal, role_groups.is_empty()) {
                match crate::user::group_permits(role_groups, &artifact.artifact_type, &artifact.name, &artifact.action)
                {
                    Ok(true) => {}
                    Ok(false) => refusal = Some("this agent's role does not permit it".to_string()),
                    Err(error) => {
                        tracing::error!(
                            "whether the role group may use tool {} could not be read: {error}",
                            spec.name
                        );
                        refusal = Some("whether this agent may run it could not be checked".to_string());
                    }
                }
            }
            entries.push(Entry {
                name: spec.name.clone(),
                title: spec.title.clone(),
                description: spec.description.clone(),
                classification: spec.classification,
                allowed: refusal.is_none(),
                refusal,
                target: Target::AddOn(Arc::clone(tool)),
            });
        }
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        Self { entries }
    }

    /// This catalog with the remote tools of a run of `agent_id` for `user_id`: the A2A delegates and the MCP client
    /// tools the admin's rows allow the agent ([`super::delegate::entries`], [`super::mcp_client::entries`]), each
    /// marked with whether `policy` offers it in its class. The MCP server's catalog has none: its clients call the
    /// remotes themselves.
    #[must_use]
    pub fn with_remotes(mut self, facade: &EntityFacade, agent_id: &str, user_id: &str, policy: &ToolPolicy) -> Self {
        self.entries.extend(super::delegate::entries(facade, agent_id, policy));
        self.entries.extend(super::mcp_client::entries(facade, agent_id, user_id, policy));
        self.entries.sort_by(|left, right| left.name.cmp(&right.name));
        self
    }

    /// The entry of a tool name, else of a service name (a skill may name the service).
    pub fn get(&self, name: &str) -> Option<&Entry> {
        let name = name.trim();
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .or_else(|| self.entries.iter().find(|entry| entry.service_name() == Some(name)))
    }

    /// The names the agent may run.
    pub fn names(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|entry| entry.allowed)
            .map(|entry| entry.name.as_str())
            .collect()
    }

    /// The nearest names to `wanted` the agent may run, tool and service names both.
    pub fn did_you_mean(&self, wanted: &str) -> Vec<String> {
        let names: Vec<&str> = self
            .entries
            .iter()
            .filter(|entry| entry.allowed)
            .flat_map(|entry| std::iter::once(entry.name.as_str()).chain(entry.service_name()))
            .collect();
        let near = super::suggest::nearest(wanted, names, 3);
        // a service name found is given as its tool's name
        let mut out: Vec<String> = Vec::new();
        for name in near {
            let tool = self.get(&name).map_or(name, |entry| entry.name.clone());
            if !out.contains(&tool) {
                out.push(tool);
            }
        }
        out
    }

    /// The entries the agent and the user may run that match the words of `text`, best first, and how many matched: a word in the
    /// name counts most, then the title, then the description, each by its plural and simple-suffix forms; the whole
    /// text in the name or title adds more. No words is every entry.
    pub fn find(&self, text: &str) -> (Vec<&Entry>, usize) {
        let query: Vec<Vec<String>> = words(text)
            .into_iter()
            .filter(|word| word.chars().count() >= 3 && !STOP_WORDS.contains(&word.as_str()))
            .map(|word| super::search::word_forms(&word))
            .collect();
        let whole = text.trim().to_lowercase();
        let mut scored: Vec<(i64, &Entry)> = Vec::new();
        // what the agent and the user may run, before the ranking takes its first ones
        for entry in self
            .entries
            .iter()
            .filter(|entry| entry.allowed && entry.user_may_run())
        {
            if query.is_empty() {
                scored.push((0, entry));
                continue;
            }
            let name_words = words(&format!("{} {}", entry.name, entry.service_name().unwrap_or_default()));
            let title_words = words(&entry.title);
            let description_words = words(&entry.description);
            let mut score = 0;
            for forms in &query {
                let has = |list: &[String]| list.iter().any(|word| forms.iter().any(|form| word == form));
                score += if has(&name_words) {
                    4
                } else if has(&title_words) {
                    3
                } else if has(&description_words) {
                    1
                } else {
                    0
                };
            }
            if score > 0
                && whole.len() >= 3
                && (entry.name.to_lowercase().contains(&whole) || entry.title.to_lowercase().contains(&whole))
            {
                score += 10;
            }
            if score > 0 {
                scored.push((score, entry));
            }
        }
        scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.name.cmp(&right.1.name)));
        let total = scored.len();
        (
            scored.into_iter().take(FIND_MAX).map(|(_, entry)| entry).collect(),
            total,
        )
    }
}

/// Words too common to match a tool by.
const STOP_WORDS: [&str; 12] = [
    "the", "and", "for", "with", "from", "into", "that", "this", "tool", "tools", "service", "services",
];

/// The lower-case words of `text`, a camel-case name split (`createUserAccount` is create, user, account).
fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut last_lower = false;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            if ch.is_uppercase() && last_lower && !word.is_empty() {
                out.push(std::mem::take(&mut word).to_lowercase());
            }
            last_lower = ch.is_lowercase() || ch.is_ascii_digit();
            word.push(ch);
        } else {
            if !word.is_empty() {
                out.push(std::mem::take(&mut word).to_lowercase());
            }
            last_lower = false;
        }
    }
    if !word.is_empty() {
        out.push(word.to_lowercase());
    }
    out
}

/// Why the policy does not let the agent run `name` of `classification`, if it does not.
pub(crate) fn policy_refusal(policy: &ToolPolicy, name: &str, classification: Classification) -> Option<String> {
    let rule = policy.rule_as(name, classification);
    (!rule.offered).then(|| format!("this agent may not make {} calls with it", classification.as_str()))
}

/// Every catalog name of the site, for the guidance a direct call of one gets: each exposure's tool and service name,
/// every lazy add-on tool's, and the remote tools' of the admin's rows (no remote is asked).
pub fn all_names(facade: &EntityFacade) -> Vec<String> {
    let mut names: Vec<String> = crate::llm::run::quietly(|| exposures(facade))
        .unwrap_or_default()
        .into_iter()
        .filter(|exposure| !exposure.disabled)
        .flat_map(|exposure| [exposure.tool_name, exposure.service_name])
        .collect();
    names.extend(
        registered()
            .iter()
            .filter(|tool| tool.spec().lazy)
            .map(|tool| tool.spec().name.clone()),
    );
    names.extend(super::delegate::names(facade).into_iter().map(|(name, _)| name));
    names.extend(super::mcp_client::names(facade).into_iter().map(|(name, ..)| name));
    names.sort();
    names.dedup();
    names
}

/// The values of a JSON schema's properties as `describe_tools` writes them, one line each: `- name: type, required,
/// format, one of a|b|c (+N), default x: description`.
pub fn parameter_lines(schema: &Json) -> Vec<String> {
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Json::as_array)
        .map(|list| list.iter().filter_map(Json::as_str).collect())
        .unwrap_or_default();
    let Some(properties) = schema.get("properties").and_then(Json::as_object) else {
        return Vec::new();
    };
    properties
        .iter()
        .map(|(name, property)| {
            let mut parts = vec![property.get("type").and_then(Json::as_str).unwrap_or("any").to_string()];
            if required.contains(&name.as_str()) {
                parts.push("required".to_string());
            }
            if let Some(format) = property.get("format").and_then(Json::as_str) {
                parts.push(format.to_string());
            }
            if let Some(options) = property.get("enum").and_then(Json::as_array) {
                let shown: Vec<String> = options.iter().take(10).map(plain).collect();
                let more = options.len().saturating_sub(10);
                parts.push(if more > 0 {
                    format!("one of {} (+{more})", shown.join("|"))
                } else {
                    format!("one of {}", shown.join("|"))
                });
            }
            if let Some(default) = property.get("default") {
                parts.push(format!("default {}", plain(default)));
            }
            let description = property
                .get("description")
                .and_then(Json::as_str)
                .map(|text| {
                    format!(
                        ": {}",
                        super::format::cut(text.split('\n').next().unwrap_or_default().trim(), 120)
                    )
                })
                .unwrap_or_default();
            format!("- {name}: {}{description}", parts.join(", "))
        })
        .collect()
}

fn plain(value: &Json) -> String {
    match value {
        Json::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The current values of the record an entity-auto write names, by its primary key in `parameters`: the key, the
/// fields (encrypted ones and those named like secrets left out) and its `lastUpdatedStamp` as text. `None` when the key
/// is not all there or the record is not found.
///
/// # Errors
///
/// Returns an error when the entity is not defined or the row cannot be read.
pub fn entity_record(
    facade: &EntityFacade,
    entity: &str,
    parameters: &std::collections::BTreeMap<String, Value>,
) -> Result<Option<EntityRecord>, moqui_lib::error::Error> {
    let (pk, hidden) = facade.with_entity(entity, |definition, _, _| {
        let pk: Vec<String> = definition.pk_field_names().into_iter().map(str::to_string).collect();
        let hidden: Vec<String> = definition
            .fields
            .iter()
            .filter(|field| field.encrypt || super::data::secret_name(&field.name))
            .map(|field| field.name.clone())
            .collect();
        Ok((pk, hidden))
    })?;
    let mut find = facade.find(entity)?.disable_authz();
    let mut key = serde_json::Map::new();
    for name in &pk {
        let Some(value) = parameters.get(name).filter(|value| !moqui_lib::object::is_empty(value)) else {
            return Ok(None);
        };
        key.insert(name.clone(), moqui_lib::json::to_json(value));
        find = find.eq(name, value.clone());
    }
    let Some(row) = find.one()? else {
        return Ok(None);
    };
    let fields: serde_json::Map<String, Json> = row
        .fields()
        .iter()
        .filter(|(name, value)| !hidden.iter().any(|field| field.as_str() == &***name) && !value.is_null())
        .map(|(name, value)| (name.to_string(), moqui_lib::json::to_json(value)))
        .collect();
    let stamp = row
        .get("lastUpdatedStamp")
        .filter(|value| !value.is_null())
        .map(moqui_lib::object::to_plain_string);
    Ok(Some(EntityRecord {
        entity: entity.to_string(),
        key: Json::Object(key),
        fields: Json::Object(fields),
        stamp,
        hidden,
    }))
}

/// The primary key field names of `entity`.
///
/// # Errors
///
/// Returns an error when the entity is not defined.
pub fn entity_pk(facade: &EntityFacade, entity: &str) -> Result<Vec<String>, moqui_lib::error::Error> {
    facade.with_entity(entity, |definition, _, _| {
        Ok(definition.pk_field_names().into_iter().map(str::to_string).collect())
    })
}

/// A record an entity-auto write changes, as it is before the write.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityRecord {
    pub entity: String,
    pub key: Json,
    pub fields: Json,
    pub stamp: Option<String>,
    /// Field names never shown (encrypted, or named like secrets).
    pub hidden: Vec<String>,
}

impl EntityRecord {
    /// The snapshot an approval keeps: `{entity, pk, record, lastUpdatedStamp}`.
    pub fn snapshot(&self) -> Json {
        json!({"entity": self.entity, "pk": self.key, "record": self.fields, "lastUpdatedStamp": self.stamp})
    }
}
