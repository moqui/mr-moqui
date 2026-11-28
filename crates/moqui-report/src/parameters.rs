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

//! A dataset's parameters (docs/tools/Monitoring.md, A dataset's parameters; plans/IronCalc-DB2.md CG4.1): the values a
//! panel query over a service dataset passes to its service (`parameters`), declared in the dataset's body so that the
//! panel editors (the browser's `m-report-definition-editor`, the terminal's report editor) show a field for each, a
//! select where the declaration names its choices. A declaration is presentation: the dataset's service still checks
//! every value it is given, as the viewer, and a dataset without `parameters` is read as before.
//!
//! A declaration is `{name, label, kind, optional, choices}`: `kind` is `text` (the default) or `number`, and `choices`
//! names a service the viewer calls for them, `{service, parameters, list, value, label}` (the out-parameter that holds
//! the list, and each item's value and label fields), which the viewer's own grant must call. A saved dataset names
//! only a choices service a shipped dataset names there with the same parameters (definitions.rs `service_refusal`,
//! as its `serviceName`).

use moqui_core::context::ExecutionContext;
use moqui_core::service::call::{sync_checked, CallOptions};
use serde_json::{json, Value as Json};

use super::{input, text};

/// The most parameters one dataset declares.
pub(crate) const MAX_PARAMETERS: usize = 16;
/// The most choices one parameter offers; a service that answers more is cut.
pub(crate) const MAX_CHOICES: usize = 500;
/// The kinds of a parameter's value.
const KINDS: &[&str] = &["text", "number"];

/// Whether `name` is a parameter name: letters, digits and `_`, starting with a letter, at most 64.
fn is_name(name: &str) -> bool {
    name.len() <= 64
        && name.chars().next().is_some_and(|first| first.is_ascii_alphabetic())
        && name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

/// Why `body`'s `parameters` is not a declaration, for `save#Definition`; None when it is one or there is none.
pub(crate) fn refusal(body: &Json) -> Option<String> {
    let declared = match body.get("parameters") {
        None | Some(Json::Null) => return None,
        Some(Json::Array(declared)) => declared,
        Some(_) => return Some("a dataset's parameters is a list".into()),
    };
    if declared.len() > MAX_PARAMETERS {
        return Some(format!("a dataset declares at most {MAX_PARAMETERS} parameters"));
    }
    let mut names = std::collections::BTreeSet::new();
    for parameter in declared {
        let name = text(parameter, "name", "");
        if !is_name(name) {
            return Some(format!(
                "a dataset parameter's name is letters, digits and _, starting with a letter; {name:?} is not"
            ));
        }
        if !names.insert(name) {
            return Some(format!("a dataset declares the parameter {name} once"));
        }
        if !parameter["label"].is_null() && !parameter["label"].is_string() {
            return Some(format!("dataset parameter {name}: its label is a text"));
        }
        if !KINDS.contains(&text(parameter, "kind", "text")) {
            return Some(format!("dataset parameter {name}: its kind is text or number"));
        }
        if !parameter["optional"].is_null() && !parameter["optional"].is_boolean() {
            return Some(format!("dataset parameter {name}: optional is true or false"));
        }
        let choices = &parameter["choices"];
        if choices.is_null() {
            continue;
        }
        let named = |key: &str| choices[key].as_str().is_some_and(|value| !value.is_empty());
        if !choices.is_object() || !named("service") || !named("list") || !named("value") {
            return Some(format!(
                "dataset parameter {name}: its choices name a service, the list it answers and each item's value"
            ));
        }
        if !choices["label"].is_null() && !choices["label"].is_string() {
            return Some(format!("dataset parameter {name}: its choices' label is a field name"));
        }
        if !choices["parameters"].is_null() && !choices["parameters"].is_object() {
            return Some(format!("dataset parameter {name}: its choices' parameters are a map"));
        }
    }
    None
}

/// What `body`'s parameters' choices call: each service with the parameters it is given (none as `{}`).
pub(crate) fn choice_sources(body: &Json) -> Vec<(&str, Json)> {
    body["parameters"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|parameter| {
            let choices = &parameter["choices"];
            let service = choices["service"].as_str()?;
            let parameters = Some(&choices["parameters"]).filter(|given| !given.is_null()).cloned();
            Some((service, parameters.unwrap_or_else(|| json!({}))))
        })
        .collect()
}

/// The answers of the choices services one listing called, by service and parameters: every saved dataset's choices
/// are a shipped dataset's (definitions.rs `service_refusal`), so a listing calls each of those once, whatever number
/// of datasets name it. None when the service gave none (refused, failed, not the viewer's).
#[derive(Default)]
pub(crate) struct ChoiceAnswers(std::collections::BTreeMap<(String, String), Option<Json>>);

/// The parameters `body` declares as the editors take them: `name`, `label` (the name when it has none), `kind` and
/// `optional`, in their order; an entry that is not a declaration is left out. With `answers`, each that names a
/// choices service also has them, `{value, label}` each ([`choices`]); without, none has.
pub(crate) fn declared(
    ec: &ExecutionContext,
    dataset_id: &str,
    body: &Json,
    mut answers: Option<&mut ChoiceAnswers>,
) -> Vec<Json> {
    body["parameters"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|parameter| is_name(text(parameter, "name", "")))
        .take(MAX_PARAMETERS)
        .map(|parameter| {
            let name = text(parameter, "name", "");
            let mut out = json!({
                "name": name,
                "label": parameter["label"].as_str().filter(|label| !label.is_empty()).unwrap_or(name),
                "kind": Some(text(parameter, "kind", "text")).filter(|kind| KINDS.contains(kind)).unwrap_or("text"),
                "optional": parameter["optional"] == true,
            });
            if let (Some(answers), true) = (answers.as_deref_mut(), parameter["choices"].is_object()) {
                out["choices"] = Json::Array(choices(ec, dataset_id, name, &parameter["choices"], answers));
            }
            out
        })
        .collect()
}

/// A parameter's choices, from its service called as the viewer, once a listing for each service and parameters
/// (`answers`): each item of the out-parameter `list` as its `value` and `label` fields, at most [`MAX_CHOICES`].
fn choices(ec: &ExecutionContext, dataset_id: &str, name: &str, spec: &Json, answers: &mut ChoiceAnswers) -> Vec<Json> {
    let service = text(spec, "service", "");
    let key = (service.to_string(), spec["parameters"].to_string());
    let answered = answers
        .0
        .entry(key)
        .or_insert_with(|| answer(ec, dataset_id, name, spec))
        .clone();
    let Some(answered) = answered else {
        return Vec::new();
    };
    let (value_field, label_field) = (text(spec, "value", ""), text(spec, "label", ""));
    let shown = |value: &Json| match value {
        Json::String(text) => Some(text.clone()),
        Json::Number(number) => Some(number.to_string()),
        _ => None,
    };
    answered[text(spec, "list", "")]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let value = shown(&item[value_field])?;
            let label = shown(&item[label_field])
                .filter(|label| !label.is_empty())
                .unwrap_or_else(|| value.clone());
            Some(json!({"value": value, "label": label}))
        })
        .take(MAX_CHOICES)
        .collect()
}

/// What a choices service answers the viewer, or None. A service that fails or is not the viewer's to call gives
/// none, logged, and its errors are not the caller's: the field is then a text field (AGENTS.md failure tolerance).
fn answer(ec: &ExecutionContext, dataset_id: &str, name: &str, spec: &Json) -> Option<Json> {
    let service = text(spec, "service", "");
    // the viewer's own grant of the service, as a dataset's serviceName needs (lib.rs resolve_dataset): a screen's
    // inherited grant does not call it for them. Asked, not used: a refusal records no ArtifactAuthzFailure, so a panel
    // editor's listing by a viewer without it writes no row
    if let Err(error) = moqui_core::user::authorize_own_unrecorded("AT_SERVICE", service, "view") {
        tracing::info!(dataset = dataset_id, parameter = name, service, %error, "A dataset parameter's choices are not the viewer's to read");
        return None;
    }
    let parameters = match moqui_lib::json::from_json(&spec["parameters"]) {
        moqui_lib::object::Value::Map(map) => map.into_iter().collect(),
        _ => moqui_core::native::Parameters::new(),
    };
    // the service's refusal is this field's, not the listing's, as a panel's is its own (lib.rs execute_query)
    ec.message().push_errors();
    let called = sync_checked(
        service,
        &parameters,
        &CallOptions {
            require_new_transaction: Some(true),
            ..Default::default()
        },
    );
    {
        let mut message = ec.message();
        message.clear_errors();
        message.pop_errors();
    }
    match called {
        Ok(out) => Some(input(&out)),
        Err(error) => {
            tracing::warn!(dataset = dataset_id, parameter = name, service, %error, "A dataset parameter's choices were not read");
            None
        }
    }
}

/// `parameters` of a dataset's listing: [`declared`] for `body`, or None when it declares none.
pub(crate) fn listed(
    ec: &ExecutionContext,
    dataset_id: &str,
    body: &Json,
    answers: Option<&mut ChoiceAnswers>,
) -> Option<Json> {
    let declared = declared(ec, dataset_id, body, answers);
    (!declared.is_empty()).then_some(Json::Array(declared))
}

/// A dataset item of a listing with its declared parameters added, when it has some.
pub(crate) fn add_to(item: &mut Json, parameters: Option<Json>) {
    if let (Some(parameters), Some(object)) = (parameters, item.as_object_mut()) {
        object.insert("parameters".into(), parameters);
    }
}

#[cfg(test)]
#[path = "parameters_tests.rs"]
mod tests;
