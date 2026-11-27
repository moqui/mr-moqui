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

//! ECA rules against recorded contexts (plans/archive/design-dev-aide.md 10): the contexts Try the condition takes (a
//! service's last Try, a record of an EECA's entity, a push of the push log) saved as named fixtures,
//! `test/fixtures/<name>.json` of the rules file's component (or the site's, D17), and Save as spec on a rule: a
//! feature that runs the rule's trigger with a fixture and checks what the rule did, appended to the rules file's spec
//! (`test/<RulesFile>.spec.js`) as the service writer's Save as spec appends a Try's (`crate::services`), and run by
//! its Run spec in the sim. A SECA's trigger is a call of its service with the fixture as the parameters, an EECA's
//! the entity operation of its event on the fixture as the record; the feature's checks are what the trigger did in a
//! run in the sim when it was saved (the messages it left and the rows it wrote, for the developer to trim). A
//! gitECA rule's push is not replayed by a spec (the testkit has no push to run its event with), so its feature checks
//! the condition alone against the fixture, as a spec evaluates it (`ec.resource.condition`), and says so.

use std::path::Path;

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use crate::services::{in_sim, js, Over};
use crate::tier::Tier;
use crate::workspace::{json_value, text, Context};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(save_rule_fixture), save_rule_fixture)?;
    registry.service_function(moqui_core::service_location!(list_rule_fixtures), list_rule_fixtures)?;
    registry.service_function(moqui_core::service_location!(save_rule_spec), save_rule_spec)?;
    registry.service_function(moqui_core::service_location!(try_rule), try_rule)
}

/// The most fixtures listed, and the largest one read.
const MAX_FIXTURES: usize = 200;
const MAX_FIXTURE_BYTES: u64 = 256 * 1024;

/// A fixture's name: letters, digits, `-` and `_`, at most 64.
fn fixture_name(name: &str) -> Result<String, LibError> {
    let name = name.trim();
    if name.is_empty()
        || name.len() > 64
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(LibError::new(
            "a fixture's name is letters, digits, - and _ (at most 64)",
        ));
    }
    Ok(name.to_string())
}

/// The rules file a call names (`rootId`, `path`): its full path.
fn rules_file(context: &Context, parameters: &Parameters) -> Result<std::path::PathBuf, LibError> {
    let root = context.root(&text(parameters, "rootId"))?;
    Ok(context.resolve(root, &text(parameters, "path"))?.path)
}

/// The spec of a rules file: its name less `.xml`, its parts joined (`MySiteSecas` of `MySite.secas.xml`).
pub(crate) fn rules_spec_name(file: &str) -> String {
    let base = file.rsplit(['/', '\\']).next().unwrap_or(file);
    let base = base.strip_suffix(".xml").unwrap_or(base);
    base.split('.')
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + chars.as_str()
            })
        })
        .collect::<String>()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect()
}

/// Where a fixture of a rules file is: `test/fixtures/<name>.json` of its component, else of the site's.
fn fixture_file(context: &Context, rules: &Path, name: &str) -> Result<(String, String), LibError> {
    crate::services::own_or_site(
        context,
        Some(rules),
        &format!("test/fixtures/{name}.json"),
        "the rules file is upstream and there is no site component to keep its fixtures in",
    )
}

/// The `component://` location of a file of a root.
fn location(context: &Context, root_id: &str, path: &str) -> Result<String, LibError> {
    let root = context.root(root_id)?;
    crate::screens::location_of(root, path).ok_or_else(|| LibError::new(format!("{path} is not in a component")))
}

/// Write the fixture `name` of `rules` (its context, a JSON object) and answer where it is.
fn write_fixture(
    context: &Context,
    rules: &Path,
    name: &str,
    fixture: &serde_json::Map<String, Json>,
) -> Result<Json, LibError> {
    let (root_id, path) = fixture_file(context, rules, name)?;
    let root = context.root(&root_id)?;
    let resolved = context.resolve(root, &path)?;
    let base = std::fs::read(&resolved.path)
        .map(|bytes| crate::workspace::sha256(&bytes))
        .unwrap_or_default();
    let body = serde_json::to_string_pretty(&Json::Object(fixture.clone())).unwrap_or_else(|_| "{}".to_string()) + "\n";
    crate::workspace::save_text(
        context,
        root,
        &resolved,
        &body,
        &base,
        Some(json!({ "ruleFixture": name })),
    )?;
    Ok(json!({ "name": name, "rootId": root_id, "location": location(context, &root_id, &path)?, "path": path }))
}

/// `save#RuleFixture`: a context (`context`, a JSON object) saved as the fixture `name` of the rules file (`rootId`,
/// `path`), replacing one of the name. The extend tier.
fn save_rule_fixture(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let name = fixture_name(&text(parameters, "name"))?;
    let rules = rules_file(&context, parameters)?;
    let saved = write_fixture(
        &context,
        &rules,
        &name,
        &crate::services::object_in(parameters, "context"),
    )?;
    let mut out = Parameters::new();
    out.insert("fixture".into(), json_value(&saved));
    Ok(out)
}

/// `list#RuleFixtures`: the fixtures of the rules file (`rootId`, `path`): each JSON file of `test/fixtures` of where
/// its fixtures go, with its context; one that does not read as a JSON object is listed with why.
fn list_rule_fixtures(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let rules = rules_file(&context, parameters)?;
    let mut fixtures: Vec<Json> = Vec::new();
    // the directory of a fixture named for the listing, not written
    if let Ok((root_id, path)) = fixture_file(&context, &rules, "x") {
        let dir = Path::new(&path)
            .parent()
            .map(|dir| dir.to_string_lossy().into_owned())
            .unwrap_or_default();
        let root = context.root(&root_id)?;
        let resolved = context.resolve(root, &dir)?;
        let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(&resolved.path)
            .map(|read| read.filter_map(Result::ok).map(|entry| entry.path()).collect())
            .unwrap_or_default();
        entries.sort();
        for file in entries
            .iter()
            .filter(|file| file.extension().is_some_and(|ext| ext == "json"))
            .take(MAX_FIXTURES)
        {
            let Some(name) = file.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let relative = format!("{dir}/{name}.json");
            // confined as an open is: a link out of the root is not read
            let confined = match context.resolve(root, &relative) {
                Ok(confined) => confined,
                Err(err) => {
                    tracing::warn!("fixture {relative} is not read: {err}");
                    continue;
                }
            };
            let read = std::fs::metadata(&confined.path)
                .ok()
                .filter(|meta| meta.len() <= MAX_FIXTURE_BYTES)
                .and_then(|_| std::fs::read_to_string(&confined.path).ok());
            let mut entry = json!({ "name": name, "rootId": root_id, "path": relative,
                "location": location(&context, &root_id, &relative).ok() });
            match read.as_deref().map(serde_json::from_str::<Json>) {
                Some(Ok(Json::Object(map))) => entry["context"] = Json::Object(map),
                Some(Ok(_)) => entry["problem"] = json!("not a JSON object"),
                Some(Err(err)) => entry["problem"] = json!(format!("does not read: {err}")),
                None => entry["problem"] = json!("too large or not readable"),
            }
            fixtures.push(entry);
        }
    }
    let mut out = Parameters::new();
    out.insert("fixtures".into(), json_value(&fixtures));
    Ok(out)
}

/// The trigger of a rule in JavaScript, with the fixture in `fixture`: a SECA's service called with it, an EECA's
/// entity operation on it as the record (`operation` create, update or delete; an update or a delete of a record not
/// there makes it from the fixture first). `call` is the spec's helper; the sim's run calls the service itself.
fn trigger_js(kind: &str, target: &str, operation: &str, in_spec: bool) -> Result<String, LibError> {
    let target = js(&json!(target));
    Ok(match kind {
        "seca" if in_spec => format!("const out = call({target}, fixture)"),
        "seca" => format!("ec.service.sync().name({target}).parameters(fixture).call()"),
        "eeca" => match operation {
            "create" => format!("ec.entity.makeValue({target}).setAll(fixture).create()"),
            "update" => format!(
                "const row = ec.entity.makeValue({target}).setAll(fixture)\n        const held = row.cloneValue()\n        \
                 if (held.refresh()) held.setAll(fixture).update()\n        else row.create()"
            ),
            "delete" => format!(
                "const row = ec.entity.makeValue({target}).setAll(fixture)\n        if (!row.cloneValue().refresh()) row.create()\n        \
                 ec.entity.makeValue({target}).setAll(row.getPrimaryKeys()).delete()"
            ),
            other => return Err(LibError::new(format!("an EECA rule's operation is create, update or delete, not {other}"))),
        },
        other => return Err(LibError::new(format!("a spec runs the trigger of a seca or eeca rule, not {other}"))),
    })
}

/// The feature of a SECA or EECA rule: its trigger with the fixture, and what it did in the sim when saved.
fn trigger_feature(title: &str, fixture_location: &str, trigger: &str, messages: &[Json], rows: &[Json]) -> String {
    let mut text = format!(
        "\nfeature({}, {{\n    when() {{\n        const fixture = JSON.parse(ec.resource.getLocationText({}))\n        {trigger}\n        \
         return {{ messages: Array.from(ec.message.messages) }}\n    }},\n    then({{ messages }}) {{\n",
        js(&json!(title)),
        js(&json!(fixture_location)),
    );
    if messages.is_empty() && rows.is_empty() {
        text.push_str(
            "        // the trigger left no message and wrote no row when it was saved\n        messages.length == 0\n",
        );
    }
    for message in messages.iter().filter_map(Json::as_str) {
        text.push_str(&format!("        messages.includes({})\n", js(&json!(message))));
    }
    if let Some(check) = crate::services::check_data(rows) {
        text.push_str(&check);
    }
    text.push_str("    },\n})\n");
    text
}

/// The feature of a gitECA rule: its condition against the fixture, as it held when saved.
fn condition_feature(title: &str, fixture_location: &str, expression: &str, holds: bool) -> String {
    format!(
        "\nfeature({}, {{\n    expect() {{\n        const fixture = JSON.parse(ec.resource.getLocationText({}))\n        \
         ec.resource.condition({}, null, fixture) == {holds}\n    }},\n}})\n",
        js(&json!(title)),
        js(&json!(fixture_location)),
        js(&json!(expression)),
    )
}

/// A rule's feature, made once the fixture's location is known.
type FeatureOf = Box<dyn Fn(&str) -> String>;

/// A rule as Save as spec and Try the rule take it: its kind, its service or entity, an EECA rule's operation, a gitECA
/// rule's condition, the fixture as the trigger's context, and the name a feature calls the fixture by.
struct RuleAsked<'a> {
    kind: &'a str,
    target: &'a str,
    operation: &'a str,
    condition: &'a str,
    fixture: &'a serde_json::Map<String, Json>,
    title: &'a str,
}

/// A rule's trigger run in the sim with a fixture: what the run came to (with its trace when asked), the feature a
/// spec of it is, and what to say of the feature.
struct Trial {
    feature: FeatureOf,
    run: Json,
    note: Option<&'static str>,
}

/// The run in the sim of a rule's trigger with its fixture (a SECA's service call, an EECA's entity operation, a gitECA
/// rule's condition alone), within `caller`'s frame, traced when `trace`.
fn rule_trial(context: &Context, caller: &str, asked: &RuleAsked<'_>, trace: bool) -> Result<Trial, LibError> {
    let (kind, target, name, fixture) = (asked.kind, asked.target, asked.title, asked.fixture);
    match kind {
        "seca" | "eeca" => {
            if target.trim().is_empty() {
                return Err(LibError::new("name the rule's service or entity"));
            }
            // a SECA's service runs as an agent's try_service would: not the Dev AIDE's own writes, nor the Terminal's
            if kind == "seca" {
                if let Some(why) = crate::agent::try_service::sim_refusal(target) {
                    return Err(LibError::new(why));
                }
            }
            let in_sim_js = trigger_js(kind, target, asked.operation, false)?;
            let in_spec_js = trigger_js(kind, target, asked.operation, true)?;
            let body = format!("const fixture = {};\n{in_sim_js}", Json::Object(fixture.clone()));
            let actions = crate::services::snippet_actions("javascript", &body, &serde_json::Map::new());
            moqui_core::script::check_actions(&actions)?;
            let label = format!("A {kind} rule's trigger on {target}");
            let over = Over { overlay: None, trace };
            let run = in_sim(context, caller, &label, over, move || {
                moqui_core::script::run_actions(&actions).map_err(|err| LibError::new(err.to_string()))?;
                Ok(std::collections::BTreeMap::new())
            });
            let messages: Vec<Json> = run["messages"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|message| message.as_str().is_some_and(|text| !text.starts_with("result: ")))
                .collect();
            let rows: Vec<Json> = run["rows"].as_array().cloned().unwrap_or_default();
            let title = format!("the {kind} rule on {target} against {name}");
            Ok(Trial {
                feature: Box::new(move |location: &str| {
                    trigger_feature(&title, location, &in_spec_js, &messages, &rows)
                }),
                run,
                note: None,
            })
        }
        "giteca" => {
            let condition = asked.condition.trim().to_string();
            let expression = if condition.is_empty() {
                "true".to_string()
            } else if condition.starts_with('<') {
                moqui_core::script::condition_source(&format!("<condition>{condition}</condition>"))?
            } else {
                condition.clone()
            };
            let actions = crate::services::condition_actions(&condition, fixture);
            moqui_core::script::check_actions(&actions)?;
            let run = in_sim(
                context,
                caller,
                "A gitECA rule's condition",
                Over::default(),
                move || {
                    moqui_core::script::run_actions(&actions).map_err(|err| LibError::new(err.to_string()))?;
                    Ok(std::collections::BTreeMap::new())
                },
            );
            let holds = condition.is_empty()
                || run["messages"]
                    .as_array()
                    .is_some_and(|messages| messages.iter().any(|text| text == "the condition holds"));
            let title = format!("the giteca rule's condition against {name} (a spec does not replay a push)");
            Ok(Trial {
                feature: Box::new(move |location: &str| condition_feature(&title, location, &expression, holds)),
                run,
                note: Some("A spec does not replay a push, so the feature checks the rule's condition against the fixture only."),
            })
        }
        "emeca" => Err(LibError::new(
            "an EmECA rule runs on a received message, which a spec does not make: try its condition",
        )),
        other => Err(LibError::new(format!("no rule kind {other}"))),
    }
}

/// `try#Rule`: a rule's trigger run in the sim with a context (`context`), traced, nothing written to the database (its
/// writes are the sim's): what the run came
/// to, its messages and rows, and the trace the ECA view marks the rule's actions with (a gitECA rule's condition
/// alone). The extend tier, as Save as spec.
fn try_rule(ec: &ExecutionContext, service: &ServiceInfo<'_>, parameters: &Parameters) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    if !moqui_llm::approval::preview::available(&context.facade) {
        return Err(LibError::new(
            "a rule's trigger runs in the sim, which this server has not (it needs embedded Postgres)",
        ));
    }
    let fixture = crate::services::object_in(parameters, "context");
    let trial = rule_trial(
        &context,
        service.name,
        &RuleAsked {
            kind: &text(parameters, "kind"),
            target: &text(parameters, "target"),
            operation: &text(parameters, "operation"),
            condition: &text(parameters, "condition"),
            fixture: &fixture,
            title: "the context",
        },
        true,
    )?;
    let mut out = Parameters::new();
    out.insert("result".into(), json_value(&trial.run));
    out.insert("note".into(), trial.note.map_or(Value::Null, Value::string));
    Ok(out)
}

/// `save#RuleSpec`: Save as spec on a rule (10) of the rules file (`rootId`, `path`, saved: the trigger runs the rules
/// the server holds): `kind` seca, eeca or giteca; `target` its service or entity; `operation` an EECA's (create,
/// update or delete); `condition` a gitECA rule's (an expression or a condition element); `fixture` the name its
/// `context` is saved under. The trigger runs in the sim first, which writes nothing to the database, and a failure
/// there writes no file; then the fixture and the feature are written. Answers the spec file and name, the fixture, the sim run, and `note` for a gitECA
/// rule's feature. The extend tier.
fn save_rule_spec(
    ec: &ExecutionContext,
    service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    if !moqui_llm::approval::preview::available(&context.facade) {
        return Err(LibError::new(
            "a rule's spec is made from a run in the sim, which this server has not (it needs embedded Postgres)",
        ));
    }
    let kind = text(parameters, "kind");
    let target = text(parameters, "target");
    let name = fixture_name(&text(parameters, "fixture"))?;
    let rules = rules_file(&context, parameters)?;
    let fixture = crate::services::object_in(parameters, "context");
    let rules_name = rules
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();
    let spec = rules_spec_name(&rules_name);
    if spec.is_empty() {
        return Err(LibError::new(format!("no spec name for {rules_name}")));
    }
    let trial = rule_trial(
        &context,
        service.name,
        &RuleAsked {
            kind: &kind,
            target: &target,
            operation: &text(parameters, "operation"),
            condition: &text(parameters, "condition"),
            fixture: &fixture,
            title: &name,
        },
        false,
    )?;
    if let Some(failed) = trial.run["failed"].as_str() {
        return Err(LibError::new(format!(
            "the trigger failed in the sim, so nothing was written: {failed}"
        )));
    }
    let Trial {
        feature: feature_of,
        run: trial,
        note,
    } = trial;
    let _authz = ec.disable_authz();
    let saved = write_fixture(&context, &rules, &name, &fixture)?;
    let location = saved["location"].as_str().unwrap_or_default().to_string();
    let (root_id, file) = crate::services::own_or_site(
        &context,
        Some(&rules),
        &format!("test/{spec}.spec.js"),
        "the rules file is upstream and there is no site component to write its spec in",
    )?;
    let header = format!(
        "// Specs of the rules of {}, from Save as spec in Dev AIDE (plans/archive/design-dev-aide.md 10): each runs a\n\
         // rule's trigger with a fixture of test/fixtures and checks what the rule did when it was saved; trim what a run\n\
         // makes anew (generated ids, stamps) before relying on them.\n// Run: ./moq test <component> --spec {spec}\n",
        crate::services::comment_text(&rules_name)
    );
    let mut answer = crate::services::append_feature(
        &context,
        &root_id,
        &file,
        &header,
        &feature_of(&location),
        json!({ "ruleSpec": kind, "target": target, "fixture": name }),
    )?;
    answer.insert("spec".into(), Value::string(spec));
    answer.insert("fixture".into(), json_value(&saved));
    answer.insert("trial".into(), json_value(&trial));
    answer.insert("note".into(), note.map_or(Value::Null, Value::string));
    Ok(answer)
}

#[cfg(test)]
#[path = "eca_specs_tests.rs"]
mod tests;
