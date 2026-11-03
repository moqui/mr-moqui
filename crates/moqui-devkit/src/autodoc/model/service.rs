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

//! A service's doc (its settings, parameters, what it implements, the SECA rules on it and, when asked, its definition)
//! and a service file's (its services).

use moqui_lib::error::Error;
use serde_json::{json, Value as Json};

use super::{capped, Run, SOURCE_LINES};
use crate::autodoc::plan::DocEntry;

pub(super) fn data(run: &mut Run<'_>, doc: &DocEntry) -> Result<Json, Error> {
    let detail = moqui_core::service::info::service_detail(&doc.name)
        .ok_or_else(|| Error::new(format!("service {} is not defined", doc.name)))?;
    let detail = moqui_lib::json::to_json(&moqui_lib::object::Value::Map(detail));
    let description = run.description_of(&doc.file, doc.line);
    let mut map = run.header(doc, description);
    let text = |key: &str| detail.get(key).and_then(Json::as_str).unwrap_or_default().to_string();
    let flag = |key: &str| detail.get(key).and_then(Json::as_bool).unwrap_or(false);
    let transaction = if flag("txIgnore") {
        "ignore"
    } else if flag("txForceNew") {
        "force-new"
    } else {
        "use-or-begin"
    };
    let implements: Vec<Json> = detail
        .get("implements")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .map(|name| json!({"name": name, "href": run.link_id(doc, &format!("service:{name}"))}))
        .collect();
    let name = doc.name.clone();
    let on_it: Vec<crate::eca::EcaRuleInfo> = run
        .eca_rules()
        .iter()
        .filter(|rule| rule.kind == "seca" && rule.target == name)
        .cloned()
        .collect();
    let rules: Vec<Json> = on_it
        .into_iter()
        .map(|rule| {
            let file = run.plan.of_file(&canonical(&rule.file));
            json!({
                "id": rule.id,
                "when": rule.when,
                "condition": rule.condition,
                "actions": rule.actions.join(", "),
                "component": rule.component,
                "href": file.and_then(|found| run.link(doc, found)),
            })
        })
        .collect();
    let source = run
        .options
        .include_source
        .then(|| capped(&text("serviceXml"), SOURCE_LINES));
    let (used_by, uses, usage) = run.usage_data(doc, &[&doc.id])?;
    map.insert(
        "service".into(),
        json!({
            "verb": text("verb"),
            "noun": text("noun"),
            "type": text("serviceType"),
            "kind": text("kind"),
            "location": text("location"),
            "method": text("method"),
            "authenticate": text("authenticate"),
            "transaction": transaction,
            "readOnly": flag("readOnly"),
            "validate": flag("validate"),
            "semaphore": text("semaphore"),
            "inParameters": flatten(detail.get("inParameters")),
            "outParameters": flatten(detail.get("outParameters")),
            "implements": implements,
            "secaRules": rules,
            "source": source,
        }),
    );
    map.insert("usedBy".into(), used_by);
    map.insert("uses".into(), uses);
    map.insert("diagrams".into(), json!({"usage": usage}));
    Ok(Json::Object(map))
}

/// A path as the index keeps it: canonical, links resolved.
pub(super) fn canonical(path: &str) -> String {
    std::fs::canonicalize(path).map_or_else(|_| path.to_string(), |found| found.display().to_string())
}

/// Nested parameters as rows, a nested one named by its dotted path.
fn flatten(parameters: Option<&Json>) -> Vec<Json> {
    fn walk(prefix: &str, list: &[Json], out: &mut Vec<Json>) {
        for parameter in list {
            let name = parameter.get("name").and_then(Json::as_str).unwrap_or_default();
            let full = if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{prefix}.{name}")
            };
            let required = parameter.get("required").and_then(Json::as_str).unwrap_or("false");
            out.push(json!({
                "name": full,
                "type": parameter.get("type").cloned().unwrap_or(Json::Null),
                "required": required == "true",
                "default": parameter.get("default").cloned().unwrap_or(Json::Null),
                "allowHtml": parameter.get("allowHtml").cloned().unwrap_or(Json::Null),
                "description": parameter.get("description").and_then(Json::as_str).map(crate::autodoc::describe::paragraphs),
                "implied": parameter.get("implied").and_then(Json::as_bool).unwrap_or(false),
            }));
            if let Some(children) = parameter.get("children").and_then(Json::as_array) {
                walk(&full, children, out);
            }
        }
    }
    let mut out = Vec::new();
    if let Some(list) = parameters.and_then(Json::as_array) {
        walk("", list, &mut out);
    }
    out
}

/// A service file's doc: its services in file order, and the file's comment.
pub(super) fn file_data(run: &mut Run<'_>, doc: &DocEntry) -> Result<Json, Error> {
    let description = run.descriptions.lead(&doc.file);
    let mut map = run.header(doc, description);
    let services: Vec<Json> = run
        .plan
        .services_of(&doc.file)
        .into_iter()
        .map(|service| {
            json!({
                "title": service.title,
                "name": service.name,
                "summary": service.summary,
                "line": service.line,
                "href": run.link(doc, service),
            })
        })
        .collect();
    map.insert("services".into(), Json::Array(services));
    Ok(Json::Object(map))
}
