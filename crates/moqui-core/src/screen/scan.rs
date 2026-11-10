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

//! The `scanResolve` transition every screen has: it runs the resolver service of a `scan-input` field of the screen.
//!
//! The request names the field by `scanForm` and `scanField`; the service, the allowed identity types and the context
//! names the resolver takes come from that `scan-input` in the screen definition, never from the request, so a page can
//! run no service the screen does not name. The resolver gets `scanText`, `allowedTypes`, `scanId` and the values of
//! the `parameter` and `depends-on` names; the answer keeps the matches of the allowed types, and adds the GS1 fields
//! of the text when the resolver answered none and the text parses (`moqui_lib::barcode::parse_gs1`).
//!
//! The answer is JSON: `scanId`, `scanText`, `matches` (`identityType`, `id`, `label`, `detail`), `parsed`, and the same
//! matches as drop-down `options` (`value`, `label`) for the clients that search the resolver as a drop-down (tui).

use std::collections::BTreeMap;

use indexmap::IndexMap;
use moqui_lib::node::MNode;
use moqui_lib::object::{to_plain_string, Value};

use super::cache::ScreenFacade;
use super::definition::ScreenDef;
use crate::context::{Error, ExecutionContext, ExecutionContextFactory};
use crate::web;

/// The most text a scan may have: longer is not a scan (a GS1 element string is at most a few hundred characters).
const MAX_TEXT: usize = 4096;
/// The most matches an answer carries; a resolver that finds more should ask for a longer code.
const MAX_MATCHES: usize = 50;

/// A `scan-input` as the transition reads it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ScanInput {
    pub resolver: String,
    pub allowed_types: Vec<String>,
    /// The `parameter` names and the `depends-on` parameter names: the only request values passed on.
    pub context_names: Vec<String>,
}

impl ScanInput {
    pub(crate) fn of(node: &MNode) -> Option<Self> {
        let resolver = node
            .attribute("resolver")
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())?;
        let allowed_types = node
            .attribute("allowed-types")
            .unwrap_or_default()
            .split(',')
            .map(|part| part.trim().to_string())
            .filter(|part| !part.is_empty())
            .collect();
        let mut context_names = Vec::new();
        for parameter in node.children_named("parameter") {
            if let Some(name) = parameter.attribute("name").filter(|name| !name.is_empty()) {
                context_names.push(name);
            }
        }
        for depends in node.children_named("depends-on") {
            let name = depends
                .attribute("parameter")
                .filter(|name| !name.is_empty())
                .or_else(|| depends.attribute("field"))
                .unwrap_or_default();
            if !name.is_empty() {
                context_names.push(name);
            }
        }
        Some(Self {
            resolver,
            allowed_types,
            context_names,
        })
    }
}

/// The `scan-input` of field `field_name` in the form element `form`: in any of its sub-fields.
pub(crate) fn find_in_form(form: &MNode, field_name: &str) -> Option<MNode> {
    let field =
        form.first_where(|node| node.name() == "field" && node.attribute("name").as_deref() == Some(field_name))?;
    field.children().iter().find_map(|sub| sub.first_named("scan-input"))
}

/// The fields of `form` whose `scan-input`s post their scan id under the same name, or None when each has its own. Two
/// scans of one form under one name would post one operation key for both, so the key would no longer name one scan.
pub(crate) fn shared_scan_id(form: &MNode) -> Option<(String, Vec<String>)> {
    let mut by_name: indexmap::IndexMap<String, Vec<String>> = indexmap::IndexMap::default();
    for field in form.children_named("field") {
        for sub in field.children() {
            if let Some(input) = sub.first_named("scan-input") {
                let name = input
                    .attribute("scan-id-field")
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| "scanId".to_string());
                let fields = by_name.entry(name).or_default();
                let field_name = field.attribute("name").unwrap_or_default();
                if !fields.contains(&field_name) {
                    fields.push(field_name);
                }
            }
        }
    }
    by_name.into_iter().find(|(_, fields)| fields.len() > 1)
}

/// Log, when the form is assembled (the definition), the fields of a form whose scans share a scan id name.
pub(crate) fn check_form(form: &MNode, location: &str) {
    if let Some((name, fields)) = shared_scan_id(form) {
        tracing::error!(
            "Form {} of {location}: the scan-input fields {} post their scan id as the same {name}; give each its own scan-id-field",
            form.attribute("name").unwrap_or_default(),
            fields.join(", ")
        );
    }
}

/// Run the transition for the request's parameters and send its JSON answer: 400 for a request that names no field or
/// has no text, 404 when the screen has no such `scan-input`, 400 with the resolver's errors when it fails.
///
/// # Errors
///
/// Returns an error when the form cannot be assembled.
pub(crate) fn resolve(
    facade: &ScreenFacade,
    factory: &ExecutionContextFactory,
    screen: &ScreenDef,
    parameters: &BTreeMap<String, Value>,
) -> Result<(), Error> {
    let text = |name: &str| parameters.get(name).map(to_plain_string).unwrap_or_default();
    let form_name = text("scanForm");
    let field_name = text("scanField");
    // the drop-down clients (tui) send what was typed as term
    let mut scan_text = text("scanText");
    if scan_text.trim().is_empty() {
        scan_text = text("term");
    }
    let scan_text = scan_text
        .trim_matches(|ch: char| ch == '\r' || ch == '\n' || ch == ' ')
        .to_string();
    if form_name.is_empty() || field_name.is_empty() {
        web::send_json_error(400, "A scan names its form and field (scanForm, scanField)");
        return Ok(());
    }
    if !screen.forms.contains_key(&form_name) {
        web::send_json_error(404, &format!("No form {form_name} in this screen"));
        return Ok(());
    }
    let form = super::form::assemble(facade, factory, screen, &form_name)?;
    let Some(input) = find_in_form(&form.shared_node()?, &field_name)
        .as_ref()
        .and_then(ScanInput::of)
    else {
        web::send_json_error(
            404,
            &format!("No scan-input {field_name} with a resolver in form {form_name}"),
        );
        return Ok(());
    };
    if let Some((name, fields)) = shared_scan_id(&form.shared_node()?) {
        web::send_json_error(
            400,
            &format!(
                "The scan-input fields {} of form {form_name} share the scan id {name}",
                fields.join(", ")
            ),
        );
        return Ok(());
    }
    if scan_text.is_empty() || scan_text.chars().count() > MAX_TEXT {
        web::send_json_error(400, "Nothing was scanned");
        return Ok(());
    }
    let scan_id = text("scanId");
    let mut call = BTreeMap::new();
    call.insert("scanText".to_string(), Value::string(&scan_text));
    call.insert(
        "allowedTypes".to_string(),
        Value::List(input.allowed_types.iter().map(Value::string).collect()),
    );
    call.insert("scanId".to_string(), Value::string(&scan_id));
    for name in &input.context_names {
        if let Some(value) = parameters.get(name) {
            call.insert(name.clone(), value.clone());
        }
    }
    // the resolver's own errors (a refusal it names) are the answer, with 400; another failure is the error's own status
    let mark = ExecutionContext::with_current(|ec| ec.message().error_mark());
    let result = crate::service::call::sync_with(&input.resolver, &call, &crate::service::call::CallOptions::default());
    let errors: Vec<String> = mark
        .and_then(|mark| ExecutionContext::with_current(|ec| ec.message().take_errors_since(mark)))
        .unwrap_or_default();
    if !errors.is_empty() {
        web::send_json_error(400, &errors.join("; "));
        return Ok(());
    }
    let answer = match result {
        Ok(answer) => answer,
        Err(err) => {
            tracing::info!("The scan resolver {} failed: {err}", input.resolver);
            web::send_json_error_of(&err);
            return Ok(());
        }
    };
    let matches = matches_of(answer.get("matches"), &input.allowed_types);
    let parsed = match answer.get("parsed") {
        Some(Value::Map(map)) if !map.is_empty() => Value::Map(map.clone()),
        _ => moqui_lib::barcode::parse_gs1(&scan_text).map_or(Value::Null, |data| data.to_value()),
    };
    let options = matches
        .iter()
        .filter_map(|item| match item {
            Value::Map(map) => {
                let mut option = map.clone();
                option.insert("value".to_string(), map.get("id").cloned().unwrap_or(Value::Null));
                let label = option_label(map);
                option.insert("label".to_string(), Value::string(label));
                Some(Value::Map(option))
            }
            _ => None,
        })
        .collect();
    let mut out = IndexMap::default();
    out.insert("scanId".to_string(), Value::string(&scan_id));
    out.insert("scanText".to_string(), Value::string(&scan_text));
    out.insert("matches".to_string(), Value::List(matches));
    out.insert("parsed".to_string(), parsed);
    out.insert("options".to_string(), Value::List(options));
    web::send_json_response(Value::Map(out));
    Ok(())
}

/// The matches the resolver answered as `{identityType, id, label, detail}`: one with no id left out, one of a type not
/// allowed left out when types are given, at most [`MAX_MATCHES`].
pub(crate) fn matches_of(value: Option<&Value>, allowed: &[String]) -> Vec<Value> {
    let Some(Value::List(list)) = value else {
        return Vec::new();
    };
    let field = |map: &moqui_lib::object::ValueMap, name: &str| {
        map.get(name)
            .map(to_plain_string)
            .filter(|text| text != "null")
            .unwrap_or_default()
    };
    list.iter()
        .filter_map(|item| {
            let Value::Map(map) = item else { return None };
            let id = field(map, "id");
            let identity_type = field(map, "identityType");
            if id.is_empty() || (!allowed.is_empty() && !allowed.contains(&identity_type)) {
                return None;
            }
            let label = field(map, "label");
            let mut out = IndexMap::default();
            out.insert("identityType".to_string(), Value::string(&identity_type));
            out.insert("id".to_string(), Value::string(&id));
            out.insert(
                "label".to_string(),
                Value::string(if label.is_empty() { &id } else { &label }),
            );
            out.insert("detail".to_string(), Value::string(field(map, "detail")));
            Some(Value::Map(out))
        })
        .take(MAX_MATCHES)
        .collect()
}

fn option_label(map: &moqui_lib::object::ValueMap) -> String {
    let text = |name: &str| map.get(name).map(to_plain_string).unwrap_or_default();
    let mut label = text("label");
    let detail = text("detail");
    if !detail.is_empty() {
        label = format!("{label} · {detail}");
    }
    let kind = text("identityType");
    if kind.is_empty() {
        label
    } else {
        format!("{label} ({kind})")
    }
}

#[cfg(test)]
#[path = "scan_tests.rs"]
mod tests;
