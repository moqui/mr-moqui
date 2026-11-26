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

//! `org.moqui.devaide.ServiceWriterServices`: the service writer's and the ECA rules' server side (plans/design-dev-aide.md
//! 9, 10). The services of the index with their SECA rules and the calls of the last spec run; a service as its view
//! reads it (the framework's `service_detail`, its file and line, its rules, what uses it); Try (9.4): a run in the sim
//! overlay with the rows it would write, nothing kept (the agent's `try_service` runs the same), or for real behind a
//! confirm, audited; a snippet of XML Actions or JavaScript run the same way; Save as spec, a feature of `call`, `then`
//! and `checkData` from a Try, written into the service's component or the site's; Run spec, the spec run in process
//! inside the sim, so it writes nothing either; the JavaScript the framework runs for an actions block; and the active
//! ECA rules of each kind with their files and the rules a later one replaced.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_devkit::index::{self, Direction};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;
use serde_json::{json, Value as Json};

use crate::artifacts::shown;
use crate::audit::{self, Write};
use crate::site;
use crate::tier::Tier;
use crate::workspace::{flag, json_value, text, Context};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(list_services), list_services)?;
    registry.service_function(moqui_core::service_location!(get_service_view), get_service_view)?;
    registry.service_function(moqui_core::service_location!(try_service), try_service)?;
    registry.service_function(moqui_core::service_location!(try_snippet), try_snippet)?;
    registry.service_function(moqui_core::service_location!(save_try_spec), save_try_spec)?;
    registry.service_function(moqui_core::service_location!(run_spec), run_spec)?;
    registry.service_function(moqui_core::service_location!(get_generated_js), get_generated_js)?;
    registry.service_function(moqui_core::service_location!(list_eca_rules), list_eca_rules)?;
    registry.service_function(moqui_core::service_location!(eval_condition), eval_condition)?;
    registry.service_function(moqui_core::service_location!(get_push_contexts), get_push_contexts)
}

/// The actions that try a rule's condition: the context set, then whether the condition holds as a message. An
/// expression goes in `if`'s attribute; a condition element (`compare`, `and`, ...) in its `condition`.
pub(crate) fn condition_actions(condition: &str, context: &serde_json::Map<String, Json>) -> String {
    let escape = crate::workspace::xml_attr;
    let mut out = String::from("<actions>");
    for (key, value) in context {
        if !key.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
            continue;
        }
        out.push_str(&format!(
            "<set field=\"{key}\" from=\"{}\"/>",
            escape(&value.to_string())
        ));
    }
    let condition = condition.trim();
    let (open, inner) = if condition.starts_with('<') {
        ("<if>".to_string(), format!("<condition>{condition}</condition><then>"))
    } else {
        (format!("<if condition=\"{}\">", escape(condition)), String::new())
    };
    out.push_str(&open);
    out.push_str(&inner);
    out.push_str("<message>the condition holds</message>");
    if !inner.is_empty() {
        out.push_str("</then>");
    }
    out.push_str("<else><message>the condition does not hold</message></else></if></actions>");
    out
}

/// `eval#Condition`: a rule's condition (an expression, or a condition element) against `against` (a JSON object: the
/// service's last Try, a record), run in the sim (10's "later": a SECA's condition against its service's last Try, an
/// EECA's against a record). Answers whether it holds, and its messages and log. The extend tier, as `try#Snippet`: a
/// condition is an expression evaluated in the server, which the view tier's browse and read do not include (SEC-DEV-02).
fn eval_condition(
    ec: &ExecutionContext,
    service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    if !moqui_llm::approval::preview::available(&context.facade) {
        return Err(LibError::new(
            "a condition is tried in the sim, which this server has not (it needs embedded Postgres)",
        ));
    }
    let condition = text(parameters, "condition");
    if condition.trim().is_empty() {
        let mut out = Parameters::new();
        out.insert("holds".into(), Value::Bool(true));
        return Ok(out);
    }
    let actions = condition_actions(&condition, &object_in(parameters, "against"));
    moqui_core::script::check_actions(&actions)?;
    let answer = in_sim(
        &context,
        service.name,
        "Try a rule's condition",
        Over::default(),
        move || {
            moqui_core::script::run_actions(&actions).map_err(|err| moqui_lib::error::Error::new(err.to_string()))?;
            Ok(BTreeMap::new())
        },
    );
    let holds = answer["messages"]
        .as_array()
        .is_some_and(|messages| messages.iter().any(|text| text == "the condition holds"));
    let mut out = Parameters::new();
    out.insert("holds".into(), Value::Bool(holds));
    out.insert("result".into(), json_value(&answer));
    Ok(out)
}

const GIT_SERVICES: &str = "org.moqui.git.GitServices.";

/// A git hosting service as the user (so the repository's rights apply): its out-parameters, or its errors as the
/// refusal.
fn git_hosting(name: &str, parameters: &[(&str, Value)]) -> Result<Parameters, LibError> {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    let out = moqui_core::service::sync(&format!("{GIT_SERVICES}{name}"), &parameters);
    let errors = ExecutionContext::with_current(|ec| ec.message().errors().to_vec()).unwrap_or_default();
    if !errors.is_empty() {
        ExecutionContext::with_current(|ec| ec.message().clear_errors());
        return Err(LibError::new(errors.join("; ")));
    }
    out.map_err(|err| LibError::new(format!("the git hosting ({name}): {err}")))
}

/// What of a gitECA rule tells whether it runs on an event's context, as `GitRule::matches` (moqui-core
/// `service::git_eca`) decides it past the event and the repository: its sources, its `ref-pattern` (the whole ref name,
/// and only an event of a ref), its `ref-type` (branch, tag or other) and `default-branch-only`.
pub(crate) struct GitRuleFilter {
    pub ref_pattern: Option<moqui_lib::object::CompiledRegex>,
    pub ref_type: Option<String>,
    pub default_branch_only: bool,
    pub sources: Vec<String>,
}

/// A rule's `source`, comma separated; empty is a push, a fetch and an import, as a rule without one runs on.
pub(crate) fn sources_of(text: &str) -> Vec<String> {
    let given: Vec<String> = text
        .split(',')
        .map(|source| source.trim().to_string())
        .filter(|source| !source.is_empty())
        .collect();
    if given.is_empty() {
        ["push", "fetch", "import"].map(str::to_string).to_vec()
    } else {
        given
    }
}

impl GitRuleFilter {
    /// Whether the rule runs on the event of `context` (its `source`, `refName` and `isDefaultBranch`).
    pub(crate) fn takes(&self, context: &moqui_lib::object::ValueMap) -> bool {
        let field = |name: &str| context.get(name).map(moqui_lib::object::to_plain_string);
        let source = field("source").unwrap_or_default();
        if !self.sources.contains(&source) {
            return false;
        }
        let ref_name = field("refName").filter(|name| !name.is_empty());
        if self.default_branch_only && field("isDefaultBranch").as_deref() != Some("true") {
            return false;
        }
        let name = ref_name.as_deref().unwrap_or_default();
        if let Some(ref_type) = &self.ref_type {
            let kind = if name.starts_with("refs/heads/") {
                "branch"
            } else if name.starts_with("refs/tags/") {
                "tag"
            } else {
                "other"
            };
            if ref_type != kind {
                return false;
            }
        }
        !self
            .ref_pattern
            .as_ref()
            .is_some_and(|pattern| ref_name.is_none() || !pattern.is_full_match(name))
    }
}

/// `get#PushContexts`: what a gitECA rule's condition is tried against (10's "later": a rule against a push from the
/// push log): the hosted repositories the user may see whose id the rule's `repository-pattern` takes, the latest
/// changes of the one chosen (`find#GitUpdates`, 20 a page), and with `updateId` the contexts the rule's `event` had
/// from that change (`get#GitUpdateEventContexts`, built as the events build them), each marked whether the rule
/// runs on it ([`GitRuleFilter`]: its sources, ref-pattern, ref-type, default-branch-only), and `truncated` when the
/// replay left some out. Run as the user, so a repository's rights apply (the contexts are for who may push there).
/// The extend tier, as `eval#Condition`.
fn get_push_contexts(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    Context::of(ec, Tier::Extend)?;
    let compile = |name: &str| -> Result<Option<moqui_lib::object::CompiledRegex>, LibError> {
        let pattern = text(parameters, name);
        if pattern.trim().is_empty() {
            return Ok(None);
        }
        moqui_lib::object::CompiledRegex::new(pattern.trim())
            .map(Some)
            .map_err(|err| LibError::new(format!("{name} {pattern}: {}", err.message())))
    };
    let repository_pattern = compile("repositoryPattern")?;
    let rule = GitRuleFilter {
        ref_pattern: compile("refPattern")?,
        ref_type: Some(text(parameters, "refType")).filter(|kind| !kind.trim().is_empty()),
        default_branch_only: flag(parameters, "defaultBranchOnly"),
        sources: sources_of(&text(parameters, "source")),
    };
    let listed = git_hosting("find#GitRepositories", &[("pageNoLimit", Value::Bool(true))])?;
    let repositories: Vec<String> = match listed.get("repositoryList") {
        Some(Value::List(rows)) => rows
            .iter()
            .filter_map(|row| match row {
                Value::Map(row) => row.get("repositoryId").map(moqui_lib::object::to_plain_string),
                _ => None,
            })
            .filter(|id| {
                repository_pattern
                    .as_ref()
                    .is_none_or(|pattern| pattern.is_full_match(id))
            })
            .collect(),
        _ => Vec::new(),
    };
    let mut out = Parameters::new();
    out.insert("repositories".into(), json_value(&repositories));
    let asked = text(parameters, "repositoryId");
    let Some(repository_id) = (if asked.is_empty() {
        repositories.first().cloned()
    } else {
        Some(asked)
    }) else {
        return Ok(out);
    };
    out.insert("repositoryId".into(), Value::string(&repository_id));
    let page = text(parameters, "pageIndex");
    let updates = git_hosting(
        "find#GitUpdates",
        &[
            ("repositoryId", Value::string(&repository_id)),
            (
                "pageIndex",
                Value::string(if page.is_empty() { "0" } else { page.as_str() }),
            ),
            ("pageSize", Value::I64(20)),
        ],
    )?;
    out.insert(
        "updates".into(),
        updates.get("updateList").cloned().unwrap_or(Value::List(Vec::new())),
    );
    let update_id = text(parameters, "updateId");
    if update_id.is_empty() {
        return Ok(out);
    }
    let event = text(parameters, "event");
    let answer = git_hosting(
        "get#GitUpdateEventContexts",
        &[
            ("repositoryId", Value::string(&repository_id)),
            ("updateId", Value::string(&update_id)),
            (
                "event",
                Value::string(if event.is_empty() {
                    "post-receive"
                } else {
                    event.as_str()
                }),
            ),
        ],
    )?;
    let contexts: Vec<Value> = match answer.get("contextList") {
        Some(Value::List(rows)) => rows
            .iter()
            .map(|row| {
                let mut row = match row {
                    Value::Map(row) => row.clone(),
                    _ => Default::default(),
                };
                let matches = match row.get("context") {
                    Some(Value::Map(context)) => rule.takes(context),
                    _ => false,
                };
                row.insert("matches".into(), Value::Bool(matches));
                Value::Map(row)
            })
            .collect(),
        _ => Vec::new(),
    };
    out.insert("contexts".into(), Value::List(contexts));
    out.insert(
        "truncated".into(),
        Value::Bool(matches!(answer.get("truncated"), Some(Value::Bool(true)))),
    );
    Ok(out)
}

/// The calls of each service in the last spec run here, by service name: the coverage column.
fn coverage() -> &'static Mutex<HashMap<String, u64>> {
    static COVERAGE: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    COVERAGE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn json_of(value: &Value) -> Json {
    moqui_lib::json::to_json(value)
}

/// A parameter given as a JSON object or its text.
pub(crate) fn object_in(parameters: &Parameters, name: &str) -> serde_json::Map<String, Json> {
    match parameters.get(name) {
        Some(Value::String(raw)) => serde_json::from_str(raw).ok(),
        Some(value) => match json_of(value) {
            Json::Object(map) => Some(map),
            _ => None,
        },
        None => None,
    }
    .unwrap_or_default()
}

/// The SECA rules of each service, by service name.
fn seca_counts(rules: &[moqui_devkit::eca::EcaRuleInfo]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for rule in rules
        .iter()
        .filter(|rule| rule.kind == "seca" && rule.replaced_by.is_none())
    {
        *counts.entry(rule.target.clone()).or_insert(0) += 1;
    }
    counts
}

/// `list#Services`: the services of the index by name, with their component, file, SECA count and the calls of the
/// last spec run here.
fn list_services(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let filter = text(parameters, "filter").to_lowercase();
    let components: Vec<String> = text(parameters, "component")
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    let rows = index::of_kinds(&context.factory, &["service".to_string()], &components, 50_000)?;
    let secas = seca_counts(&moqui_devkit::eca::rule_list(&context.factory));
    let calls = lock(coverage()).clone();
    let services: Vec<Json> = rows
        .iter()
        .filter(|row| filter.is_empty() || row.name.to_lowercase().contains(&filter))
        .map(|row| {
            let mut out = shown(&context, row);
            out["secaCount"] = json!(secas.get(&row.name).copied().unwrap_or(0));
            out["calls"] = calls.get(&row.name).map_or(Json::Null, |count| json!(count));
            out["editUrl"] = json!(format!("/qapps/dev/Services/Edit?artifact={}", row.id));
            out
        })
        .collect();
    let mut out = Parameters::new();
    out.insert("services".into(), json_value(&services));
    Ok(out)
}

/// The service's name from `artifactId` (`service:<name>`) or `serviceName`.
fn service_name_in(parameters: &Parameters) -> String {
    let id = text(parameters, "artifactId");
    match id.strip_prefix("service:") {
        Some(name) => name.to_string(),
        None if !id.is_empty() => id,
        None => text(parameters, "serviceName"),
    }
}

/// `get#ServiceView`: a service as its view shows it: its definition (parameters nested, the implied ones marked), its
/// file and line, its script's file, its SECA rules, how many artifacts use it, and the calls of the last spec run. With
/// `files` (the unsaved service buffers, as `try#Service` takes them) the definition is the buffers' (F14), one they
/// define anew too; nothing of them runs, they are only read.
fn get_service_view(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let name = service_name_in(parameters);
    let overlay = crate::try_overlay::overlay_of(&crate::try_overlay::files_of(&context, parameters)?)?;
    let detail = crate::try_overlay::over(overlay, || moqui_core::service::info::service_detail(&name))
        .ok_or_else(|| LibError::new(format!("no service {name}")))?;
    let detail = json_of(&Value::Map(detail));
    let id = format!("service:{name}");
    let artifact = index::describe(&context.factory, &id)?.map(|row| shown(&context, &row));
    let used_by = index::refs(&context.factory, &id, Direction::In, &[])?.len();
    let script = (detail["kind"] == "script")
        .then(|| {
            detail["location"]
                .as_str()
                .map(|location| location_file(&context, location))
        })
        .flatten();
    let secas: Vec<Json> = moqui_devkit::eca::rule_list(&context.factory)
        .into_iter()
        .filter(|rule| rule.kind == "seca" && rule.target == name)
        .map(|rule| {
            let mut out = serde_json::to_value(&rule).unwrap_or(Json::Null);
            out["open"] = site::located(&context, &rule.file);
            out
        })
        .collect();
    let calls = lock(coverage()).get(&name).copied();
    let view = json!({
        "service": detail,
        "artifact": artifact,
        "script": script,
        "secas": secas,
        "usedByCount": used_by,
        "calls": calls,
        "sim": moqui_llm::approval::preview::available(&context.facade),
        "tier": context.tier.name(),
        "site": context.settings.default_site(),
        // where Add puts a SECA rule: the site's own rules file (D17), a companion buffer made on its first save
        "secasFile": context
            .settings
            .default_site()
            .and_then(|name| site::site_path(&context, &format!("service/{}.secas.xml", site::class_of(name))))
            .map(|file| file.json()),
    });
    let mut out = Parameters::new();
    out.insert("view".into(), json_value(&view));
    Ok(out)
}

/// A script's `component://` location as the Workspace opens it.
fn location_file(context: &Context, location: &str) -> Json {
    let path = location.strip_prefix("component://").and_then(|rest| {
        let (name, rest) = rest.split_once('/')?;
        context
            .factory
            .components()
            .iter()
            .find(|component| component.name == name)
            .map(|component| component.location.join(rest))
    });
    match path {
        Some(path) => site::located(
            context,
            &std::fs::canonicalize(&path).unwrap_or(path).display().to_string(),
        ),
        None => json!({ "location": location }),
    }
}

/// The messages and errors of the entered context.
fn messages() -> (Vec<String>, Vec<String>) {
    ExecutionContext::with_current(|ec| {
        let message = ec.message();
        (message.messages().to_vec(), message.errors().to_vec())
    })
    .unwrap_or_default()
}

fn out_json(out: &BTreeMap<String, Value>) -> Json {
    Json::Object(out.iter().map(|(key, value)| (key.clone(), json_of(value))).collect())
}

/// The parameters of a call from a JSON object.
fn call_parameters(object: &serde_json::Map<String, Json>) -> BTreeMap<String, Value> {
    object
        .iter()
        .filter(|(_, value)| !value.is_null() && value.as_str() != Some(""))
        .map(|(key, value)| (key.clone(), moqui_lib::json::from_json(value)))
        .collect()
}

/// The Dev AIDE service a sim run of `caller` goes within: its own, at the extend tier and above, so the run has what
/// that service's group passes on (`DEV_AIDE_APP`, `inheritAuthz`) as a real run of it has, and a Try in sim may run
/// what a Run for real may (9.4); none at the view tier, which has no real run and keeps only the user's own grants.
fn sim_within(tier: Tier, caller: &str) -> Option<&str> {
    (tier >= Tier::Extend).then_some(caller)
}

/// What a sim run runs over: the service overlay of unsaved or proposed text (F14, `crate::try_overlay`), and whether
/// its XML actions are traced (9.2's marks on the composer's blocks, `moqui_core::script::traced`).
#[derive(Default)]
pub(crate) struct Over {
    pub overlay: Option<std::sync::Arc<moqui_core::service::overlay::ServiceOverlay>>,
    pub trace: bool,
}

/// A trace as a Try answers it: each block's file also as the Workspace opens it (its root and path).
fn located_trace(context: &Context, mut trace: Json) -> Json {
    for block in trace["blocks"].as_array_mut().into_iter().flatten() {
        if let Some(file) = block["file"].as_str().map(str::to_string) {
            let open = site::located(context, &file);
            block["rootId"] = open["rootId"].clone();
            block["path"] = open["path"].clone();
        }
    }
    if let Some(file) = trace["failed"]["file"].as_str().map(str::to_string) {
        let open = site::located(context, &file);
        trace["failed"]["rootId"] = open["rootId"].clone();
        trace["failed"]["path"] = open["path"].clone();
    }
    trace
}

/// A run of `body` in the sim as the current user, within `caller`'s frame ([`sim_within`]), over `over`: its
/// out-parameters, messages and errors, the rows it would write and its time, or why it failed (its own error, not that
/// the sim was rolled back), and with `over.trace` the trace of its actions, which a failure answers too.
pub(crate) fn in_sim(
    context: &Context,
    caller: &str,
    label: &str,
    over: Over,
    body: impl FnOnce() -> Result<BTreeMap<String, Value>, moqui_lib::error::Error> + Send,
) -> Json {
    let username = moqui_core::user::current_username().ok().flatten().unwrap_or_default();
    let started = std::time::Instant::now();
    let within = sim_within(context.tier, caller);
    let traced = Mutex::new(Json::Null);
    let ran = moqui_llm::approval::preview::run_in_sim_within(&context.factory, &username, label, within, || {
        let Over { overlay, trace } = over;
        let ((out, log, more), trace) = crate::try_overlay::over(overlay, || {
            let run = || moqui_core::devtools::log_capture::with(body);
            if trace {
                moqui_core::script::traced(run)
            } else {
                (run(), Json::Null)
            }
        });
        *lock(&traced) = trace;
        let log = (log, more);
        let (messages, errors) = messages();
        match out {
            Err(error) => Err(moqui_lib::error::Error::new(
                format!("{error} {}", errors.join("; ")).trim().to_string(),
            )),
            Ok(_) if !errors.is_empty() => Err(moqui_lib::error::Error::new(errors.join("; "))),
            Ok(out) => Ok((out_json(&out), messages, errors, log)),
        }
    });
    let millis = started.elapsed().as_millis();
    let mut answer = match ran {
        Ok(run) => {
            let (out, messages, errors, (log, more_log)) = run.value;
            json!({"sim": true, "out": out, "messages": messages, "errors": errors, "rows": run.rows,
                "moreRows": run.more, "log": log, "moreLog": more_log, "millis": millis})
        }
        Err(error) => json!({"sim": true, "failed": error.to_string(), "millis": millis}),
    };
    let trace = std::mem::take(&mut *lock(&traced));
    if !trace.is_null() {
        answer["trace"] = located_trace(context, trace);
    }
    answer
}

/// `try#Service`: run a service with `parameters` (a JSON object) in the sim, nothing kept (9.4), or with `real` for
/// real: the extend tier, `confirm` set, audited. In the sim it runs over the unsaved or proposed text of `files` (a
/// JSON list of `{rootId, path, text}`) and `preview` (`buffer:<id>`, `approval:<id>`), F14 for services: the service
/// as those files define it (one they define anew too), and the services it calls that they define; text the caller
/// gives runs at the extend tier only, as a snippet does. With `trace`, the trace of its XML actions (each element's
/// count, the values it set and the one that failed).
fn try_service(
    ec: &ExecutionContext,
    service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let name = service_name_in(parameters);
    let files = crate::try_overlay::files_of(&context, parameters)?;
    let overlay = crate::try_overlay::overlay_of(&files)?;
    // text the caller gives is code run in the server: the extend tier, as a snippet (SEC-DEV-02)
    if overlay.is_some() {
        Context::of(ec, Tier::Extend)?;
    }
    let defined_here = overlay.as_ref().is_some_and(|overlay| overlay.defines(&name));
    if !defined_here && !moqui_core::service::is_service_defined(&name, Some(&context.facade)) {
        return Err(LibError::new(format!("no service {name}")));
    }
    let given = object_in(parameters, "parameters");
    let call = call_parameters(&given);
    let mut out = Parameters::new();
    if flag(parameters, "real") {
        if overlay.is_some() {
            return Err(LibError::new(
                "unsaved or proposed text runs in the sim only: save it, then run it for real",
            ));
        }
        let context = Context::of(ec, Tier::Extend)?;
        if !flag(parameters, "confirm") {
            return Err(LibError::new("a real run needs its confirm"));
        }
        let started = std::time::Instant::now();
        let result = moqui_core::service::sync(&name, &call);
        let (messages, errors) = messages();
        let answer = match &result {
            Ok(result) => json!({"sim": false, "out": out_json(result), "messages": messages, "errors": errors,
                "millis": started.elapsed().as_millis()}),
            Err(error) => json!({"sim": false, "failed": error.to_string(), "errors": errors}),
        };
        audit::record(
            &context.facade,
            &Write {
                action: "DAA_TRY",
                path: &name,
                detail: Some(json!({ "parameters": given, "ok": result.is_ok() && errors.is_empty() })),
                ..Write::default()
            },
        );
        out.insert("result".into(), json_value(&answer));
        return Ok(out);
    }
    if !moqui_llm::approval::preview::available(&context.facade) {
        return Err(LibError::new(
            "this server has no sim (it needs embedded Postgres): run it for real",
        ));
    }
    let called = name.clone();
    let shown = overlay.as_deref().map(crate::try_overlay::overlay_json);
    let over = Over {
        overlay,
        trace: flag(parameters, "trace"),
    };
    let mut answer = in_sim(
        &context,
        service.name,
        &format!("Try in sim of {name}"),
        over,
        move || moqui_core::service::sync(&called, &call).map_err(|err| moqui_lib::error::Error::new(err.to_string())),
    );
    if let Some(shown) = shown {
        answer["overlay"] = shown;
    }
    out.insert("result".into(), json_value(&answer));
    Ok(out)
}

/// The actions of a snippet: XML Actions as they are (an `<actions>` element or its content), or JavaScript as a
/// `script` action; each given parameter set first.
pub(crate) fn snippet_actions(language: &str, body: &str, given: &serde_json::Map<String, Json>) -> String {
    let escape = crate::workspace::xml_attr;
    let mut out = String::from("<actions>");
    for (key, value) in given {
        if !key.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
            continue;
        }
        let text = value.as_str().map_or_else(|| value.to_string(), str::to_string);
        out.push_str(&format!("<set field=\"{key}\" value=\"{}\"/>", escape(&text)));
    }
    let body = body.trim();
    if language == "javascript" {
        out.push_str(&format!(
            "<script><![CDATA[{}]]></script>",
            body.replace("]]>", "]]]]><![CDATA[>")
        ));
    } else {
        let inner = body
            .strip_prefix("<actions>")
            .and_then(|rest| rest.strip_suffix("</actions>"))
            .unwrap_or(body);
        out.push_str(inner);
    }
    // the snippet's `result`, when it sets one, as its last message
    out.push_str("<script><![CDATA[if (typeof result !== 'undefined') ec.message.addMessage('result: ' + JSON.stringify(result))]]></script>");
    out.push_str("</actions>");
    out
}

/// `try#Snippet`: a scratch inline service (XML Actions or JavaScript, `language`) run in the sim with `parameters`,
/// answering its messages (a `result` it sets is the last, as JSON) and the rows it would write; kept in the browser
/// only (9.4).
fn try_snippet(
    ec: &ExecutionContext,
    service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    if !moqui_llm::approval::preview::available(&context.facade) {
        return Err(LibError::new(
            "a snippet runs in the sim, which this server has not (it needs embedded Postgres)",
        ));
    }
    let actions = snippet_actions(
        &text(parameters, "language"),
        &text(parameters, "body"),
        &object_in(parameters, "parameters"),
    );
    moqui_core::script::check_actions(&actions)?;
    let answer = in_sim(
        &context,
        service.name,
        "Try in sim of a snippet",
        Over::default(),
        move || {
            moqui_core::script::run_actions(&actions).map_err(|err| moqui_lib::error::Error::new(err.to_string()))?;
            Ok(BTreeMap::new())
        },
    );
    let mut out = Parameters::new();
    out.insert("result".into(), json_value(&answer));
    Ok(out)
}

/// A JavaScript literal of a JSON value.
pub(crate) fn js(value: &Json) -> String {
    value.to_string()
}

/// The `checkData` condition of the rows a sim run wrote (created and updated, with their key and new values, hidden
/// ones left out), none when it wrote none.
pub(crate) fn check_data(rows: &[Json]) -> Option<String> {
    let escape = crate::workspace::xml_attr;
    let records: Vec<String> = rows
        .iter()
        .filter(|row| matches!(row["action"].as_str(), Some("create" | "update")))
        .filter_map(|row| {
            let entity = row["entity"].as_str()?;
            let mut attrs: Vec<String> = Vec::new();
            for field in row["fields"].as_array().into_iter().flatten() {
                let (Some(name), Some(after)) = (field["field"].as_str(), field.get("after")) else {
                    continue;
                };
                if after.is_null() || after.as_str() == Some("(hidden)") {
                    continue;
                }
                let value = after.as_str().map_or_else(|| after.to_string(), str::to_string);
                attrs.push(format!("{name}=\"{}\"", escape(&value)));
            }
            Some(format!("            <{entity} {}/>", attrs.join(" ")))
        })
        .collect();
    (!records.is_empty()).then(|| {
        format!(
            "        checkData(`<entity-facade-xml>\n{}\n        </entity-facade-xml>`).length == 0\n",
            records.join("\n").replace('`', "\\`").replace("${", "\\${")
        )
    })
}

/// The feature a Try makes (9.4): a call of the service with its parameters, a `then` comparing the out-parameters,
/// and a `checkData` of the rows the sim wrote (created and updated, with their key and new values), for the developer
/// to trim (generated ids and stamps differ in another run).
pub(crate) fn spec_feature(
    service: &str,
    given: &serde_json::Map<String, Json>,
    out: &serde_json::Map<String, Json>,
    rows: &[Json],
) -> String {
    let mut text = format!(
        "\nfeature({}, {{\n    when() {{\n",
        js(&json!(format!("{service} as it ran in a Try")))
    );
    text.push_str(&format!(
        "        return call({}, {})\n    }},\n",
        js(&json!(service)),
        js(&Json::Object(given.clone()))
    ));
    text.push_str("    then(out) {\n");
    for (key, value) in out {
        if value.is_object() || value.is_array() {
            continue;
        }
        text.push_str(&format!("        out[{}] == {}\n", js(&json!(key)), js(value)));
    }
    if let Some(check) = check_data(rows) {
        text.push_str(&check);
    }
    text.push_str("    },\n})\n");
    text
}

/// The file at `relative` (`test/X.spec.js`) in the component of `artifact` (an artifact's file) when the user may
/// write there, else in the site's (D17): its root and its path in the root.
pub(crate) fn own_or_site(
    context: &Context,
    artifact: Option<&Path>,
    relative: &str,
    upstream: &str,
) -> Result<(String, String), LibError> {
    let own = artifact
        .and_then(|path| crate::roots::locate(&context.roots, path).map(|(root, relative)| (root.clone(), relative)))
        .and_then(|(root, in_root)| {
            let component = root.component_of(&in_root)?.clone();
            let file = if component.path.is_empty() {
                relative.to_string()
            } else {
                format!("{}/{relative}", component.path)
            };
            context.may_write(&root, &file).ok().map(|()| (root.id.clone(), file))
        });
    match own {
        Some(found) => Ok(found),
        None => {
            let site = site::site_path(context, relative).ok_or_else(|| LibError::new(upstream.to_string()))?;
            Ok((site.root_id, site.path))
        }
    }
}

/// Append `feature` to the spec file `file` of `root_id`, made with `header` and `spec({})` when it is not there;
/// saved through the save pipeline (audited with `detail`). Answers the save's out-parameters with the root and path.
pub(crate) fn append_feature(
    context: &Context,
    root_id: &str,
    file: &str,
    header: &str,
    feature: &str,
    detail: Json,
) -> Result<Parameters, LibError> {
    let root = context.root(root_id)?;
    let resolved = context.resolve(root, file)?;
    let (before, base) = match std::fs::read(&resolved.path) {
        Ok(bytes) => (
            String::from_utf8_lossy(&bytes).into_owned(),
            crate::workspace::sha256(&bytes),
        ),
        Err(_) => (format!("{header}spec({{}})\n"), String::new()),
    };
    let mut text_after = before.trim_end().to_string();
    text_after.push('\n');
    text_after.push_str(feature);
    let mut answer = crate::workspace::save_text(context, root, &resolved, &text_after, &base, Some(detail))?;
    answer.insert("rootId".into(), Value::string(root_id));
    answer.insert("path".into(), Value::string(file));
    Ok(answer)
}

/// The spec file of a service: `test/<Services file>.spec.js` (`ExampleServices` of
/// `moqui.example.ExampleServices.create#Example`), or for an entity-auto service the entity's name.
fn spec_name(service: &str) -> String {
    let (prefix, verb_noun) = service
        .split_once('#')
        .map_or(("", service), |(prefix, noun)| (prefix, noun));
    let base = if prefix.contains('.') {
        prefix
            .rsplit('.')
            .nth(1)
            .filter(|part| !part.is_empty())
            .unwrap_or(prefix)
    } else {
        verb_noun.rsplit('.').next().unwrap_or(verb_noun)
    };
    base.chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect()
}

/// `save#TrySpec`: the feature of a Try appended to the service's spec file (made with `spec({})` when missing), in the
/// service's component when the user may write there, else in the site's (D17); answers the file and its spec name.
fn save_try_spec(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let name = service_name_in(parameters);
    let given = object_in(parameters, "parameters");
    let out = object_in(parameters, "out");
    let rows: Vec<Json> = match parameters.get("rows") {
        Some(Value::String(raw)) => serde_json::from_str(raw).unwrap_or_default(),
        Some(value) => serde_json::from_value(json_of(value)).unwrap_or_default(),
        None => Vec::new(),
    };
    let spec = spec_name(&name);
    if spec.is_empty() {
        return Err(LibError::new(format!("no spec name for {name}")));
    }
    // the service's own component when it may be written, else the site's
    let own = index::describe(&context.factory, &format!("service:{name}"))?.map(|row| row.path);
    let (root_id, file) = own_or_site(
        &context,
        own.as_deref().map(Path::new),
        &format!("test/{spec}.spec.js"),
        "the service is upstream and there is no site component to write its spec in",
    )?;
    let header = format!(
        "// Specs of {name} and its neighbours, from Tries in Dev AIDE (plans/design-dev-aide.md 9.4): trim what a\n// run makes anew (generated ids, stamps) before relying on them.\n// Run: ./moq test <component> --spec {spec}\n"
    );
    let feature = spec_feature(&name, &given, &out, &rows);
    let mut answer = append_feature(&context, &root_id, &file, &header, &feature, json!({ "trySpec": name }))?;
    answer.insert("spec".into(), Value::string(spec));
    Ok(answer)
}

/// `run#Spec`: a spec file (`rootId` and `path`) run in process inside the sim, so nothing it writes is kept: each
/// feature's outcome, and the calls of each service, which the coverage column shows.
fn run_spec(ec: &ExecutionContext, service: &ServiceInfo<'_>, parameters: &Parameters) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let root = context.root(&text(parameters, "rootId"))?;
    let resolved = context.resolve(root, &text(parameters, "path"))?;
    let source = std::fs::read_to_string(&resolved.path)
        .map_err(|err| LibError::new(format!("{}: {err}", resolved.relative)))?;
    let name = resolved
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(".spec.js"))
        .ok_or_else(|| LibError::new("a spec file is named <Name>.spec.js"))?
        .to_string();
    if !moqui_llm::approval::preview::available(&context.facade) {
        return Err(LibError::new(
            "a spec runs in the sim here, which this server has not (it needs embedded Postgres)",
        ));
    }
    let spec = moqui_devkit::testkit::SpecFile {
        name: name.clone(),
        display: resolved.relative.clone(),
        source,
    };
    let filter = Some(text(parameters, "feature")).filter(|text| !text.is_empty());
    let username = moqui_core::user::current_username().ok().flatten().unwrap_or_default();
    let within = sim_within(context.tier, service.name);
    let ran = moqui_llm::approval::preview::run_in_sim_within(
        &context.factory,
        &username,
        &format!("Run spec {name}"),
        within,
        || {
            moqui_core::service::coverage::enable();
            let result = moqui_devkit::testkit::run_spec(
                &spec,
                &moqui_devkit::testkit::RunOptions {
                    feature_filter: filter,
                    ..Default::default()
                },
                &mut |_| {},
            );
            let calls = moqui_core::service::coverage::finish();
            Ok((result, calls))
        },
    );
    let (result, calls) = ran?.value;
    {
        let mut kept = lock(coverage());
        for (service, count) in &calls {
            kept.insert(service.clone(), *count);
        }
    }
    let cases: Vec<Json> = result
        .cases
        .iter()
        .map(|case| {
            let (outcome, message) = match &case.outcome {
                moqui_devkit::testkit::Outcome::Passed => ("passed", String::new()),
                moqui_devkit::testkit::Outcome::Failed(failure) => {
                    ("failed", format!("{}: {}", failure.block, failure.message))
                }
                moqui_devkit::testkit::Outcome::Ignored(why) => ("ignored", why.clone()),
                moqui_devkit::testkit::Outcome::Skipped(why) => ("skipped", why.clone()),
            };
            json!({ "name": case.name, "outcome": outcome, "message": message, "millis": case.duration.as_millis() })
        })
        .collect();
    let mut out = Parameters::new();
    out.insert("passed".into(), Value::Bool(!result.failed()));
    out.insert("cases".into(), json_value(&cases));
    out.insert(
        "setupFailure".into(),
        result.setup_failure.as_ref().map_or(Value::Null, |failure| {
            Value::string(format!("{}: {}", failure.block, failure.message))
        }),
    );
    out.insert("calls".into(), json_value(&calls));
    Ok(out)
}

/// `get#GeneratedJs`: the JavaScript the framework runs for an actions block (`xml`, an `<actions>` element or its
/// content), each statement after the element it comes from (9.2's Generated view).
fn get_generated_js(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let _context = Context::of(ec, Tier::View)?;
    let xml = text(parameters, "xml");
    let trimmed = xml.trim();
    let actions = if trimmed.starts_with("<actions") {
        trimmed.to_string()
    } else {
        format!("<actions>{trimmed}</actions>")
    };
    let mut out = Parameters::new();
    match moqui_core::script::generated_source(&actions, true) {
        Ok(source) => {
            out.insert("source".into(), Value::string(source));
        }
        Err(err) => {
            out.insert("error".into(), Value::string(err.to_string()));
        }
    }
    Ok(out)
}

/// `list#EcaRules`: the active ECA rules (`kind` seca, eeca, emeca, giteca, or all), each with its file and line, those
/// a later rule replaced under it marked so; `component` and `target` (a service, entity or event, part of its name)
/// narrow.
fn list_eca_rules(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let kind = text(parameters, "kind");
    let component = text(parameters, "component");
    let target = text(parameters, "target").to_lowercase();
    let when = text(parameters, "when");
    let rules: Vec<Json> = moqui_devkit::eca::rule_list(&context.factory)
        .into_iter()
        .filter(|rule| kind.is_empty() || rule.kind == kind)
        .filter(|rule| component.is_empty() || rule.component == component)
        .filter(|rule| target.is_empty() || rule.target.to_lowercase().contains(&target))
        .filter(|rule| when.is_empty() || rule.when.contains(&when))
        .map(|rule| {
            let mut out = serde_json::to_value(&rule).unwrap_or(Json::Null);
            let open = site::located(&context, &rule.file);
            out["site"] = open["site"].clone();
            out["url"] = match (open["rootId"].as_str(), open["path"].as_str()) {
                (Some(root), Some(path)) => json!(format!(
                    "/qapps/dev/Workspace?rootId={root}&path={path}&line={}&view=eca",
                    rule.line
                )),
                _ => Json::Null,
            };
            out["open"] = open;
            out["actionsText"] = json!(rule.actions.join(", "));
            out
        })
        .collect();
    let mut out = Parameters::new();
    out.insert("rules".into(), json_value(&rules));
    Ok(out)
}

#[cfg(test)]
#[path = "services_tests.rs"]
mod tests;
