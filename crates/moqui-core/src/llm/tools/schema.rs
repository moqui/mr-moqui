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

//! Tool input schemas: a small builder for the JSON schema objects tools declare, the check of a call's arguments
//! against its tool's schema, and the `*` patterns of `AgentToolPolicy`.

use serde_json::{json, Map, Value as Json};

/// One property of an object schema.
pub struct Property<'a> {
    pub name: &'a str,
    pub kind: &'a str,
    pub description: &'a str,
    pub required: bool,
}

impl<'a> Property<'a> {
    pub fn required(name: &'a str, kind: &'a str, description: &'a str) -> Self {
        Self {
            name,
            kind,
            description,
            required: true,
        }
    }

    pub fn optional(name: &'a str, kind: &'a str, description: &'a str) -> Self {
        Self {
            name,
            kind,
            description,
            required: false,
        }
    }
}

/// `{"type":"object","properties":{…},"required":[…]}` with the properties in the order given; an empty description
/// is left out, and so is an empty `required`.
pub fn object(properties: &[Property<'_>]) -> Json {
    let mut props = Map::new();
    let mut required = Vec::new();
    for property in properties {
        let mut entry = Map::new();
        entry.insert("type".to_string(), json!(property.kind));
        if !property.description.is_empty() {
            entry.insert("description".to_string(), json!(property.description));
        }
        props.insert(property.name.to_string(), Json::Object(entry));
        if property.required {
            required.push(json!(property.name));
        }
    }
    let mut schema = json!({"type": "object", "properties": props});
    if !required.is_empty() {
        schema["required"] = Json::Array(required);
    }
    schema
}

/// Whether `arguments` fit the tool's `schema` (the subset [`crate::llm::provider::check_schema`] checks); `null` is
/// an object with no properties, which any object fits.
///
/// # Errors
///
/// Returns where and why they do not, for the model to read.
pub fn check_arguments(arguments: &Json, schema: &Json) -> Result<(), String> {
    if !arguments.is_object() {
        return Err("the arguments are not a JSON object".to_string());
    }
    if schema.is_null() {
        return Ok(());
    }
    crate::llm::provider::check_schema(arguments, schema)
}

/// Whether a tool name matches a policy pattern: the name itself, or a pattern with `*` standing for any run of
/// characters (`*` alone is every tool).
pub fn name_matches(pattern: &str, name: &str) -> bool {
    let pattern = pattern.trim();
    if !pattern.contains('*') {
        return pattern == name;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let (first, rest) = parts.split_first().expect("split yields one part");
    let Some(mut left) = name.strip_prefix(first) else {
        return false;
    };
    let (last, middle) = rest.split_last().expect("a pattern with * has two parts");
    for part in middle {
        match left.find(part) {
            Some(at) => left = &left[at + part.len()..],
            None => return false,
        }
    }
    left.len() >= last.len() && left.ends_with(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_match_names_with_stars() {
        assert!(name_matches("screen", "screen"));
        assert!(!name_matches("screen", "screens"));
        assert!(name_matches("*", "anything"));
        assert!(name_matches("mcp__*", "mcp__files__read"));
        assert!(!name_matches("mcp__*", "a2a__files"));
        assert!(name_matches("*__read", "mcp__files__read"));
        assert!(name_matches("mcp__*__read", "mcp__files__read"));
        assert!(!name_matches("mcp__*__read", "mcp__files__write"));
        assert!(!name_matches("ab*ba", "aba"));
    }

    #[test]
    fn an_object_schema_lists_its_required_properties_and_checks_arguments() {
        let schema = object(&[
            Property::required("path", "string", "The screen path"),
            Property::optional("limit", "integer", ""),
        ]);
        assert_eq!(
            schema,
            json!({"type": "object", "properties": {"path": {"type": "string", "description": "The screen path"},
                "limit": {"type": "integer"}}, "required": ["path"]})
        );
        assert!(check_arguments(&json!({"path": "/apps"}), &schema).is_ok());
        let missing = check_arguments(&json!({"limit": 3}), &schema).unwrap_err();
        assert!(missing.contains("path"), "{missing}");
        assert!(check_arguments(&json!([1]), &Json::Null).is_err());
        assert!(check_arguments(&json!({"x": 1}), &Json::Null).is_ok());
    }
}
