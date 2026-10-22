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

//! Deterministic argument repair (plans/LLM-A2A-Assist.md 7.9), after the lenient parse of the provider
//! ([`crate::llm::provider::parse_arguments`]: code fences, trailing commas, single quotes, Python literals, a
//! stringified object): each argument is fitted to its property in the tool's schema. Numbers, booleans, arrays and
//! objects written as text are converted, an enum value in the wrong case takes the schema's case, a synonym or a
//! case variant of a property name is renamed, and a null is left out; an argument the schema does not have is dropped
//! and reported as `ignored`. When this still does not fit, the step makes one schema-constrained repair call of a
//! model whose `json-schema` verdict is yes (`run::step`, plan 7.9).

use serde_json::{Map, Number, Value as Json};

/// Wrong argument names and the property they mean, tried in order: the first whose property the schema has and the
/// call does not.
const SYNONYMS: &[(&str, &str)] = &[
    ("query", "text"),
    ("q", "text"),
    ("search", "text"),
    ("term", "text"),
    ("keywords", "text"),
    ("query", "find"),
    ("search", "find"),
    ("url", "path"),
    ("screen", "path"),
    ("screenPath", "path"),
    ("form", "list"),
    ("formName", "list"),
    ("table", "entity"),
    ("entityName", "entity"),
    ("filter", "where"),
    ("filters", "where"),
    ("conditions", "where"),
    ("columns", "fields"),
    ("select", "fields"),
    ("sort", "order"),
    ("orderBy", "order"),
    ("order_by", "order"),
    ("max", "limit"),
    ("count", "limit"),
    ("size", "limit"),
    ("pageSize", "limit"),
    ("skip", "offset"),
    ("start", "offset"),
    ("artifactId", "artifact"),
    ("match", "grep"),
    ("text", "question"),
    ("prompt", "question"),
    ("choices", "options"),
    ("answers", "options"),
    ("multiple", "multi"),
    ("skill", "name"),
    ("section", "name"),
    ("group", "groups"),
];

/// Arguments fitted to a schema.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Repaired {
    pub args: Json,
    /// The arguments left out, by name.
    pub ignored: Vec<String>,
    /// What was changed, for the log.
    pub fixed: Vec<String>,
}

/// `args` fitted to `schema` (an object schema; `null` takes the arguments as they are).
pub fn repair(args: &Json, schema: &Json) -> Repaired {
    let Some(given) = args.as_object() else {
        return Repaired {
            args: args.clone(),
            ..Repaired::default()
        };
    };
    let Some(properties) = schema.get("properties").and_then(Json::as_object) else {
        return Repaired {
            args: args.clone(),
            ..Repaired::default()
        };
    };
    let mut out = Map::new();
    let mut ignored = Vec::new();
    let mut fixed = Vec::new();
    // the names as given first, so a synonym never takes the place of a property the call names
    let mut later: Vec<(&String, &Json)> = Vec::new();
    for (key, value) in given {
        if properties.contains_key(key) {
            if !value.is_null() {
                out.insert(key.clone(), value.clone());
            }
        } else {
            later.push((key, value));
        }
    }
    for (key, value) in later {
        if value.is_null() {
            continue;
        }
        let target = properties
            .keys()
            .find(|name| name.eq_ignore_ascii_case(key) && !out.contains_key(*name))
            .cloned()
            .or_else(|| {
                SYNONYMS
                    .iter()
                    .filter(|(from, _)| from.eq_ignore_ascii_case(key))
                    .map(|(_, to)| (*to).to_string())
                    .find(|to| properties.contains_key(to) && !out.contains_key(to) && !given.contains_key(to))
            });
        match target {
            Some(target) => {
                fixed.push(format!("{key} as {target}"));
                out.insert(target, value.clone());
            }
            None => ignored.push(key.clone()),
        }
    }
    for (key, value) in &mut out {
        if let Some(property) = properties.get(key) {
            if let Some(coerced) = coerce(value, property) {
                fixed.push(format!("{key} converted"));
                *value = coerced;
            }
        }
    }
    Repaired {
        args: Json::Object(out),
        ignored,
        fixed,
    }
}

/// `value` converted to fit `property`, or `None` when it fits or cannot be made to.
fn coerce(value: &Json, property: &Json) -> Option<Json> {
    let kind = property.get("type").and_then(Json::as_str).unwrap_or("");
    let converted = match (kind, value) {
        ("integer", Json::String(text)) => integer(text.trim()),
        ("integer", Json::Number(number)) if number.as_i64().is_none() => number
            .as_f64()
            .filter(|float| float.fract() == 0.0 && float.abs() < 9e15)
            .map(|float| Json::Number(Number::from(float as i64))),
        ("number", Json::String(text)) => text
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(Number::from_f64)
            .map(Json::Number)
            .or_else(|| integer(text.trim())),
        ("boolean", Json::String(text)) => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "y" | "1" | "on" => Some(Json::Bool(true)),
            "false" | "no" | "n" | "0" | "off" | "" => Some(Json::Bool(false)),
            _ => None,
        },
        ("boolean", Json::Number(number)) => number.as_i64().map(|n| Json::Bool(n != 0)),
        ("array", Json::String(text)) => {
            let text = text.trim();
            match serde_json::from_str::<Json>(text) {
                Ok(Json::Array(items)) => Some(Json::Array(items)),
                _ if text.is_empty() => Some(Json::Array(Vec::new())),
                _ => Some(Json::Array(list_of(text).into_iter().map(Json::String).collect())),
            }
        }
        ("array", Json::Array(_)) => None,
        ("array", other) => Some(Json::Array(vec![other.clone()])),
        ("object", Json::String(text)) => object_of(text.trim()),
        ("string", Json::Number(number)) => Some(Json::String(number.to_string())),
        ("string", Json::Bool(flag)) => Some(Json::String(flag.to_string())),
        ("string", Json::Array(items)) if items.len() == 1 && items[0].is_string() => Some(items[0].clone()),
        _ => None,
    };
    let base = converted.as_ref().unwrap_or(value);
    // an array's items, and an enum value in the wrong case
    let itemized = match (base, property.get("items")) {
        (Json::Array(items), Some(item_schema)) => {
            let mut changed = false;
            let items: Vec<Json> = items
                .iter()
                .map(|item| match coerce(item, item_schema) {
                    Some(fixed) => {
                        changed = true;
                        fixed
                    }
                    None => item.clone(),
                })
                .collect();
            changed.then_some(Json::Array(items))
        }
        _ => None,
    };
    let base = itemized.clone().or(converted.clone());
    let current = base.as_ref().unwrap_or(value);
    if let (Json::String(text), Some(Json::Array(allowed))) = (current, property.get("enum")) {
        if !allowed.iter().any(|known| known.as_str() == Some(text)) {
            if let Some(known) = allowed
                .iter()
                .filter_map(Json::as_str)
                .find(|known| known.eq_ignore_ascii_case(text.trim()))
            {
                return Some(Json::String(known.to_string()));
            }
        }
    }
    base
}

/// A list written as text: one item a line; else items split on commas only when each is one word (`People, Screens`),
/// so a label with a comma in it (`Yes, cancel it`) stays one item.
fn list_of(text: &str) -> Vec<String> {
    let trimmed = |parts: Vec<&str>| -> Vec<String> {
        parts
            .into_iter()
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect()
    };
    if text.contains('\n') {
        return trimmed(text.lines().collect());
    }
    let parts = trimmed(text.split(',').collect());
    if parts.len() > 1 && parts.iter().all(|part| !part.contains(char::is_whitespace)) {
        parts
    } else {
        vec![text.to_string()]
    }
}

fn integer(text: &str) -> Option<Json> {
    if let Ok(number) = text.parse::<i64>() {
        return Some(Json::Number(Number::from(number)));
    }
    text.parse::<f64>()
        .ok()
        .filter(|float| float.fract() == 0.0 && float.abs() < 9e15)
        .map(|float| Json::Number(Number::from(float as i64)))
}

/// An object written as text: JSON, else `a=1&b=2` or `a=1, b=2` pairs.
fn object_of(text: &str) -> Option<Json> {
    if let Ok(Json::Object(map)) = serde_json::from_str::<Json>(text) {
        return Some(Json::Object(map));
    }
    if let Ok(Json::Object(map)) = crate::llm::provider::parse_arguments(text) {
        return Some(Json::Object(map));
    }
    if text.is_empty() {
        return Some(Json::Object(Map::new()));
    }
    let mut out = Map::new();
    for pair in text.split(['&', ',']) {
        let (key, value) = pair.split_once('=').or_else(|| pair.split_once(':'))?;
        let key = key.trim().trim_matches(['"', '\'']);
        if key.is_empty() {
            return None;
        }
        out.insert(
            key.to_string(),
            Json::String(value.trim().trim_matches(['"', '\'']).to_string()),
        );
    }
    Some(Json::Object(out))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn schema() -> Json {
        json!({"type": "object", "properties": {
            "text": {"type": "string"},
            "limit": {"type": "integer"},
            "multi": {"type": "boolean"},
            "groups": {"type": "array", "items": {"type": "string"}},
            "where": {"type": "object"},
            "kind": {"type": "string", "enum": ["openui", "vue-sfc"]},
        }})
    }

    #[test]
    fn numbers_booleans_arrays_and_objects_written_as_text_are_converted() {
        let fixed = repair(
            &json!({"text": 42, "limit": "10", "multi": "yes", "groups": "People, Screens", "where": "{\"a\": 1}"}),
            &schema(),
        );
        assert_eq!(
            fixed.args,
            json!({"text": "42", "limit": 10, "multi": true, "groups": ["People", "Screens"], "where": {"a": 1}})
        );
        assert!(fixed.ignored.is_empty());
        let more = repair(
            &json!({"limit": 5.0, "groups": "Screens", "where": "statusId=Open&type=X", "multi": 0}),
            &schema(),
        );
        assert_eq!(
            more.args,
            json!({"limit": 5, "groups": ["Screens"], "where": {"statusId": "Open", "type": "X"}, "multi": false})
        );
        assert_eq!(
            repair(&json!({"groups": "[\"a\",\"b\"]"}), &schema()).args,
            json!({"groups": ["a", "b"]})
        );
        // one label with a comma in it stays one item; a line a label splits
        assert_eq!(
            repair(&json!({"groups": "Yes, cancel it"}), &schema()).args,
            json!({"groups": ["Yes, cancel it"]})
        );
        assert_eq!(
            repair(&json!({"groups": "Acme Corp\nAcme, Inc."}), &schema()).args,
            json!({"groups": ["Acme Corp", "Acme, Inc."]})
        );
    }

    #[test]
    fn synonyms_and_case_are_mapped_and_unknown_keys_reported_as_ignored() {
        let fixed = repair(
            &json!({"query": "acme", "Limit": 3, "kind": "OpenUI", "frob": true, "nothing": null}),
            &schema(),
        );
        assert_eq!(fixed.args, json!({"text": "acme", "limit": 3, "kind": "openui"}));
        assert_eq!(fixed.ignored, ["frob"]);
        // a property the call names wins over a synonym of it
        let both = repair(&json!({"text": "a", "query": "b"}), &schema());
        assert_eq!(both.args, json!({"text": "a"}));
        assert_eq!(both.ignored, ["query"]);
        // no schema leaves the arguments as they are
        assert_eq!(repair(&json!({"x": 1}), &Json::Null).args, json!({"x": 1}));
    }
}
