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
use moqui_core::devtools::index::{self, Direction};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
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
    registry.service_function(moqui_core::service_location!(eval_condition), eval_condition)
}

/// The actions that try a rule's condition: the context set, then whether the condition holds as a message. An
/// expression goes in `if`'s attribute; a condition element (`compare`, `and`, ...) in its `condition`.
fn condition_actions(condition: &str, context: &serde_json::Map<String, Json>) -> String {
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
/// EECA's against a record). Answers whether it holds, and its messages and log.
fn eval_condition(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    if !moqui_core::llm::approval::preview::available(&context.facade) {
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
    let answer = in_sim(&context, "Try a rule's condition", move || {
        moqui_core::script::run_actions(&actions).map_err(|err| moqui_lib::error::Error::new(err.to_string()))?;
        Ok(BTreeMap::new())
    });
    let holds = answer["messages"]
        .as_array()
        .is_some_and(|messages| messages.iter().any(|text| text == "the condition holds"));
    let mut out = Parameters::new();
    out.insert("holds".into(), Value::Bool(holds));
    out.insert("result".into(), json_value(&answer));
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
fn object_in(parameters: &Parameters, name: &str) -> serde_json::Map<String, Json> {
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
fn seca_counts(rules: &[moqui_core::devtools::eca::EcaRuleInfo]) -> HashMap<String, usize> {
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
    let secas = seca_counts(&moqui_core::devtools::eca::rule_list(&context.factory));
    let calls = coverage()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
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
/// file and line, its script's file, its SECA rules, how many artifacts use it, and the calls of the last spec run.
fn get_service_view(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let name = service_name_in(parameters);
    let detail =
        moqui_core::service::info::service_detail(&name).ok_or_else(|| LibError::new(format!("no service {name}")))?;
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
    let secas: Vec<Json> = moqui_core::devtools::eca::rule_list(&context.factory)
        .into_iter()
        .filter(|rule| rule.kind == "seca" && rule.target == name)
        .map(|rule| {
            let mut out = serde_json::to_value(&rule).unwrap_or(Json::Null);
            out["open"] = site::located(&context, &rule.file);
            out
        })
        .collect();
    let calls = coverage()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&name)
        .copied();
    let view = json!({
        "service": detail,
        "artifact": artifact,
        "script": script,
        "secas": secas,
        "usedByCount": used_by,
        "calls": calls,
        "sim": moqui_core::llm::approval::preview::available(&context.facade),
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

/// A run of `body` in the sim as the current user: its out-parameters, messages and errors, the rows it would write
/// and its time, or why it failed (its own error, not that the sim was rolled back).
fn in_sim(
    context: &Context,
    label: &str,
    body: impl FnOnce() -> Result<BTreeMap<String, Value>, moqui_lib::error::Error> + Send,
) -> Json {
    let username = moqui_core::user::current_username().ok().flatten().unwrap_or_default();
    let started = std::time::Instant::now();
    let ran = moqui_core::llm::approval::preview::run_in_sim(&context.factory, &username, label, || {
        let (out, log, more) = moqui_core::devtools::log_capture::with(body);
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
    match ran {
        Ok(run) => {
            let (out, messages, errors, (log, more_log)) = run.value;
            json!({"sim": true, "out": out, "messages": messages, "errors": errors, "rows": run.rows,
                "moreRows": run.more, "log": log, "moreLog": more_log, "millis": millis})
        }
        Err(error) => json!({"sim": true, "failed": error.to_string(), "millis": millis}),
    }
}

/// `try#Service`: run a service with `parameters` (a JSON object) in the sim, nothing kept (9.4), or with `real` for
/// real: the extend tier, `confirm` set, audited.
fn try_service(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let name = service_name_in(parameters);
    if !moqui_core::service::is_service_defined(&name, Some(&context.facade)) {
        return Err(LibError::new(format!("no service {name}")));
    }
    let given = object_in(parameters, "parameters");
    let call = call_parameters(&given);
    let mut out = Parameters::new();
    if flag(parameters, "real") {
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
    if !moqui_core::llm::approval::preview::available(&context.facade) {
        return Err(LibError::new(
            "this server has no sim (it needs embedded Postgres): run it for real",
        ));
    }
    let service = name.clone();
    let answer = in_sim(&context, &format!("Try in sim of {name}"), move || {
        moqui_core::service::sync(&service, &call).map_err(|err| moqui_lib::error::Error::new(err.to_string()))
    });
    out.insert("result".into(), json_value(&answer));
    Ok(out)
}

/// The actions of a snippet: XML Actions as they are (an `<actions>` element or its content), or JavaScript as a
/// `script` action; each given parameter set first.
fn snippet_actions(language: &str, body: &str, given: &serde_json::Map<String, Json>) -> String {
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
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    if !moqui_core::llm::approval::preview::available(&context.facade) {
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
    let answer = in_sim(&context, "Try in sim of a snippet", move || {
        moqui_core::script::run_actions(&actions).map_err(|err| moqui_lib::error::Error::new(err.to_string()))?;
        Ok(BTreeMap::new())
    });
    let mut out = Parameters::new();
    out.insert("result".into(), json_value(&answer));
    Ok(out)
}

/// A JavaScript literal of a JSON value.
fn js(value: &Json) -> String {
    value.to_string()
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
    let escape = crate::workspace::xml_attr;
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
    if !records.is_empty() {
        text.push_str(&format!(
            "        checkData(`<entity-facade-xml>\n{}\n        </entity-facade-xml>`).length == 0\n",
            records.join("\n").replace('`', "\\`").replace("${", "\\${")
        ));
    }
    text.push_str("    },\n})\n");
    text
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
    let own = index::describe(&context.factory, &format!("service:{name}"))?
        .and_then(|row| {
            crate::roots::locate(&context.roots, Path::new(&row.path)).map(|(root, relative)| (root.clone(), relative))
        })
        .and_then(|(root, relative)| {
            let component = root.component_of(&relative)?.clone();
            let file = if component.path.is_empty() {
                format!("test/{spec}.spec.js")
            } else {
                format!("{}/test/{spec}.spec.js", component.path)
            };
            context.may_write(&root, &file).ok().map(|()| (root.id.clone(), file))
        });
    let (root_id, file) = match own {
        Some(found) => found,
        None => {
            let site = site::site_path(&context, &format!("test/{spec}.spec.js")).ok_or_else(|| {
                LibError::new("the service is upstream and there is no site component to write its spec in")
            })?;
            (site.root_id, site.path)
        }
    };
    let root = context.root(&root_id)?;
    let resolved = context.resolve(root, &file)?;
    let (before, base) = match std::fs::read(&resolved.path) {
        Ok(bytes) => (String::from_utf8_lossy(&bytes).into_owned(), crate::workspace::sha256(&bytes)),
        Err(_) => (
            format!(
                "// Specs of {name} and its neighbours, from Tries in Dev AIDE (plans/design-dev-aide.md 9.4): trim what a\n// run makes anew (generated ids, stamps) before relying on them.\n// Run: ./moq test <component> --spec {spec}\nspec({{}})\n"
            ),
            String::new(),
        ),
    };
    let feature = spec_feature(&name, &given, &out, &rows);
    let mut text_after = before.trim_end().to_string();
    text_after.push('\n');
    text_after.push_str(&feature);
    let saved = crate::workspace::save_text(
        &context,
        root,
        &resolved,
        &text_after,
        &base,
        Some(json!({ "trySpec": name })),
    )?;
    let mut answer = saved;
    answer.insert("rootId".into(), Value::string(root_id));
    answer.insert("path".into(), Value::string(file));
    answer.insert("spec".into(), Value::string(spec));
    Ok(answer)
}

/// `run#Spec`: a spec file (`rootId` and `path`) run in process inside the sim, so nothing it writes is kept: each
/// feature's outcome, and the calls of each service, which the coverage column shows.
fn run_spec(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
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
    if !moqui_core::llm::approval::preview::available(&context.facade) {
        return Err(LibError::new(
            "a spec runs in the sim here, which this server has not (it needs embedded Postgres)",
        ));
    }
    let spec = moqui_core::testkit::SpecFile {
        name: name.clone(),
        display: resolved.relative.clone(),
        source,
    };
    let filter = Some(text(parameters, "feature")).filter(|text| !text.is_empty());
    let username = moqui_core::user::current_username().ok().flatten().unwrap_or_default();
    let ran = moqui_core::llm::approval::preview::run_in_sim(
        &context.factory,
        &username,
        &format!("Run spec {name}"),
        || {
            moqui_core::service::coverage::enable();
            let result = moqui_core::testkit::run_spec(
                &spec,
                &moqui_core::testkit::RunOptions {
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
        let mut kept = coverage().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        for (service, count) in &calls {
            kept.insert(service.clone(), *count);
        }
    }
    let cases: Vec<Json> = result
        .cases
        .iter()
        .map(|case| {
            let (outcome, message) = match &case.outcome {
                moqui_core::testkit::Outcome::Passed => ("passed", String::new()),
                moqui_core::testkit::Outcome::Failed(failure) => {
                    ("failed", format!("{}: {}", failure.block, failure.message))
                }
                moqui_core::testkit::Outcome::Ignored(why) => ("ignored", why.clone()),
                moqui_core::testkit::Outcome::Skipped(why) => ("skipped", why.clone()),
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
    let rules: Vec<Json> = moqui_core::devtools::eca::rule_list(&context.factory)
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
mod tests {
    use super::*;

    #[test]
    fn a_condition_is_tried_with_its_context_set() {
        let context = serde_json::from_value(json!({"amount": 5, "status": "A<B"})).unwrap();
        assert_eq!(
            condition_actions("amount > 3", &context),
            "<actions><set field=\"amount\" from=\"5\"/><set field=\"status\" from=\"&quot;A&lt;B&quot;\"/>\
             <if condition=\"amount &gt; 3\"><message>the condition holds</message><else><message>the condition does not hold</message></else></if></actions>"
                .replace("&gt;", ">")
        );
        assert!(condition_actions("<compare field=\"a\" value=\"1\"/>", &serde_json::Map::new())
            .contains("<if><condition><compare field=\"a\" value=\"1\"/></condition><then><message>the condition holds</message></then><else>"));
    }

    #[test]
    fn a_spec_is_named_by_its_services_file_or_entity() {
        assert_eq!(
            spec_name("moqui.example.ExampleServices.create#Example"),
            "ExampleServices"
        );
        assert_eq!(spec_name("create#moqui.example.Example"), "Example");
    }

    #[test]
    fn a_try_becomes_a_feature_with_its_rows() {
        let given = serde_json::from_value(json!({"exampleName": "A `b`"})).unwrap();
        let out = serde_json::from_value(json!({"exampleId": "100000", "list": [1]})).unwrap();
        let rows = vec![
            json!({"entity": "moqui.example.Example", "action": "create", "fields": [
                {"field": "exampleId", "after": "100000"}, {"field": "exampleName", "after": "A ${x}"}, {"field": "secret", "after": "(hidden)"}]}),
            json!({"entity": "moqui.example.Example", "action": "delete", "fields": []}),
        ];
        let feature = spec_feature("create#moqui.example.Example", &given, &out, &rows);
        assert!(
            feature.contains("return call(\"create#moqui.example.Example\", {\"exampleName\":\"A `b`\"})"),
            "{feature}"
        );
        assert!(feature.contains("out[\"exampleId\"] == \"100000\""), "{feature}");
        assert!(!feature.contains("out[\"list\"]"), "{feature}");
        assert!(
            feature.contains("<moqui.example.Example exampleId=\"100000\" exampleName=\"A \\${x}\"/>"),
            "{feature}"
        );
        assert!(!feature.contains("secret"), "{feature}");
    }

    const RESULT: &str = "<script><![CDATA[if (typeof result !== 'undefined') ec.message.addMessage('result: ' + JSON.stringify(result))]]></script>";

    #[test]
    fn a_snippet_sets_its_parameters_then_runs() {
        let given = serde_json::from_value(json!({"a": "1 < 2", "bad name": "x"})).unwrap();
        assert_eq!(
            snippet_actions("xml", "<actions><set field=\"result\" from=\"a\"/></actions>", &given),
            "<actions><set field=\"a\" value=\"1 &lt; 2\"/><set field=\"result\" from=\"a\"/>".to_string()
                + RESULT
                + "</actions>"
        );
        assert_eq!(
            snippet_actions("javascript", "result = 1", &serde_json::Map::new()),
            "<actions><script><![CDATA[result = 1]]></script>".to_string() + RESULT + "</actions>"
        );
    }
}
