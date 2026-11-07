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

//! A Constitution document (plans/LLM-A2A-Assist.md 7.10, "Document and localization" and "Validation and failures"):
//! parsing, the checks a document passes before it is saved or used, its hash, and the text it is written as.
//!
//! The document stays a `serde_json::Value`, so keys this code does not know (extension metadata) survive every edit.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use super::locale;

/// The schema version this code reads and writes.
pub const SCHEMA_VERSION: u64 = 1;

/// A problem with one field of a document: its JSON path, a reason code and a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldError {
    pub path: String,
    pub code: &'static str,
    pub message: String,
}

impl FieldError {
    pub fn new(path: impl Into<String>, code: &'static str, message: impl Into<String>) -> Self {
        Self { path: path.into(), code, message: message.into() }
    }

    /// `path: message`, or the message alone for the whole document.
    pub fn describe(&self) -> String {
        if self.path.is_empty() {
            self.message.clone()
        } else {
            format!("{}: {}", self.path, self.message)
        }
    }
}

/// What a reference URL points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UrlKind {
    /// An absolute `https://` URL; its origin (`https://host` or `https://host:port`).
    External { origin: String },
    /// A root-relative path on this server.
    Internal,
}

/// The JSON of `text`, or the error naming where it fails.
///
/// # Errors
///
/// Returns a field error when the text is not a JSON object.
pub fn parse(text: &str) -> Result<Value, FieldError> {
    match serde_json::from_str::<Value>(text) {
        Ok(value @ Value::Object(_)) => Ok(value),
        Ok(_) => Err(FieldError::new("", "notObject", "the Constitution must be a JSON object")),
        Err(error) => Err(FieldError::new("", "json", format!("the Constitution is not valid JSON: {error}"))),
    }
}

/// The SHA-256 of the document's RFC 8785 text: two documents equal as JSON have the same hash.
pub fn hash(document: &Value) -> String {
    moqui_lib::json::canonical_sha256(document)
}

/// The document's default locale, normalized (`en` when missing, which validation refuses).
pub fn default_locale(document: &Value) -> String {
    document
        .get("defaultLocale")
        .and_then(Value::as_str)
        .and_then(locale::normalize)
        .unwrap_or_else(|| "en".to_string())
}

/// The rules of the document, in order.
pub fn rules(document: &Value) -> &[Value] {
    document.get("rules").and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

/// The references of a rule, in order.
pub fn references(rule: &Value) -> &[Value] {
    rule.get("references").and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

/// The `id` of a rule or reference.
pub fn id_of(value: &Value) -> &str {
    value.get("id").and_then(Value::as_str).unwrap_or("")
}

/// Every locale the document's text maps use, normalized and sorted, the default locale among them.
pub fn locales(document: &Value) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    out.insert(default_locale(document));
    let mut add = |map: Option<&Value>| {
        if let Some(entries) = map.and_then(Value::as_object) {
            out.extend(entries.keys().filter_map(|key| locale::normalize(key)));
        }
    };
    for field in ["title", "provenance", "instructions"] {
        add(document.get(field));
    }
    for rule in rules(document) {
        for field in ["title", "description", "instructions"] {
            add(rule.get(field));
        }
        for reference in references(rule) {
            add(reference.get("title"));
        }
    }
    out
}

/// What a reference URL is, or why it is refused: an absolute `https://` URL without credentials, or a root-relative
/// path (not `//`) without `..` segments, neither with control characters, white space or a backslash.
///
/// # Errors
///
/// Returns a reason code and a message.
pub fn check_url(text: &str) -> Result<UrlKind, (&'static str, String)> {
    if text.is_empty() {
        return Err(("urlEmpty", "a reference needs a URL".to_string()));
    }
    if text.chars().any(|c| c.is_control() || c.is_whitespace() || c == '\\') {
        return Err(("urlCharacters", "the URL has white space, a control character or a backslash".to_string()));
    }
    if let Some(path) = text.strip_prefix('/') {
        if path.starts_with('/') {
            return Err(("urlAmbiguous", "a URL starting with // names another host; use https://".to_string()));
        }
        let path_part = text.split(['?', '#']).next().unwrap_or(text);
        if path_part.split('/').any(|segment| segment == ".." || segment == ".") {
            return Err(("urlAmbiguous", "a path with . or .. segments is ambiguous".to_string()));
        }
        return Ok(UrlKind::Internal);
    }
    let parsed = url::Url::parse(text).map_err(|error| ("urlInvalid", format!("the URL is not valid: {error}")))?;
    if parsed.scheme() != "https" {
        return Err(("urlScheme", "a reference URL is https:// or a path starting with /".to_string()));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(("urlCredentials", "a reference URL must not carry credentials".to_string()));
    }
    let Some(host) = parsed.host_str().filter(|host| !host.is_empty()) else {
        return Err(("urlInvalid", "the URL has no host".to_string()));
    };
    let origin = match parsed.port() {
        Some(port) => format!("https://{host}:{port}"),
        None => format!("https://{host}"),
    };
    Ok(UrlKind::External { origin })
}

/// The problems of a document; none means it may be saved and used.
pub fn validate(document: &Value) -> Vec<FieldError> {
    let mut errors = Vec::new();
    let Some(root) = document.as_object() else {
        errors.push(FieldError::new("", "notObject", "the Constitution must be a JSON object"));
        return errors;
    };
    match root.get("schemaVersion").and_then(Value::as_u64) {
        Some(SCHEMA_VERSION) => {}
        Some(other) => errors.push(FieldError::new(
            "schemaVersion",
            "schemaVersion",
            format!("schema version {other} is not supported (this server reads {SCHEMA_VERSION})"),
        )),
        None => errors.push(FieldError::new("schemaVersion", "schemaVersion", "schemaVersion must be 1")),
    }
    let default_locale = match root.get("defaultLocale").and_then(Value::as_str).map(locale::normalize) {
        Some(Some(locale)) => locale,
        _ => {
            errors.push(FieldError::new("defaultLocale", "locale", "defaultLocale must be a locale such as en"));
            "en".to_string()
        }
    };
    for field in ["title", "provenance", "instructions"] {
        check_text_map(root.get(field), field, &default_locale, &mut errors);
    }
    let Some(rules) = root.get("rules").and_then(Value::as_array) else {
        errors.push(FieldError::new("rules", "rules", "rules must be a list"));
        return errors;
    };
    let mut rule_ids = BTreeSet::new();
    let mut reference_ids = BTreeSet::new();
    for (index, rule) in rules.iter().enumerate() {
        let path = format!("rules[{index}]");
        if !rule.is_object() {
            errors.push(FieldError::new(path, "rule", "a rule must be an object"));
            continue;
        }
        check_id(rule, &path, &mut rule_ids, "rule", &mut errors);
        for field in ["title", "description", "instructions"] {
            check_text_map(rule.get(field), &format!("{path}.{field}"), &default_locale, &mut errors);
        }
        match rule.get("references") {
            None => {}
            Some(Value::Array(list)) => {
                for (ref_index, reference) in list.iter().enumerate() {
                    let ref_path = format!("{path}.references[{ref_index}]");
                    if !reference.is_object() {
                        errors.push(FieldError::new(ref_path, "reference", "a reference must be an object"));
                        continue;
                    }
                    check_id(reference, &ref_path, &mut reference_ids, "reference", &mut errors);
                    check_text_map(reference.get("title"), &format!("{ref_path}.title"), &default_locale, &mut errors);
                    match reference.get("url").and_then(Value::as_str) {
                        Some(url) => {
                            if let Err((code, message)) = check_url(url) {
                                errors.push(FieldError::new(format!("{ref_path}.url"), code, message));
                            }
                        }
                        None => errors.push(FieldError::new(format!("{ref_path}.url"), "urlEmpty", "a reference needs a URL")),
                    }
                }
            }
            Some(_) => errors.push(FieldError::new(format!("{path}.references"), "references", "references must be a list")),
        }
    }
    errors
}

fn check_id(value: &Value, path: &str, seen: &mut BTreeSet<String>, kind: &str, errors: &mut Vec<FieldError>) {
    match value.get("id").and_then(Value::as_str).map(str::trim) {
        Some(id) if !id.is_empty() => {
            if id.chars().any(|c| c.is_control() || c.is_whitespace()) {
                errors.push(FieldError::new(format!("{path}.id"), "id", format!("the {kind} id {id:?} has white space")));
            } else if !seen.insert(id.to_string()) {
                errors.push(FieldError::new(format!("{path}.id"), "duplicateId", format!("the {kind} id {id} is used twice")));
            }
        }
        _ => errors.push(FieldError::new(format!("{path}.id"), "id", format!("a {kind} needs an id"))),
    }
}

fn check_text_map(map: Option<&Value>, path: &str, default_locale: &str, errors: &mut Vec<FieldError>) {
    let Some(entries) = map.and_then(Value::as_object) else {
        errors.push(FieldError::new(path, "textMap", "this text must be a map of locale to text"));
        return;
    };
    let mut seen: BTreeMap<String, &str> = BTreeMap::new();
    let mut has_default = false;
    for (key, value) in entries {
        let Some(normalized) = locale::normalize(key) else {
            errors.push(FieldError::new(format!("{path}.{key}"), "locale", format!("{key:?} is not a locale")));
            continue;
        };
        if let Some(previous) = seen.insert(normalized.clone(), key) {
            errors.push(FieldError::new(
                format!("{path}.{key}"),
                "duplicateLocale",
                format!("{key} and {previous} are the same locale"),
            ));
        }
        match value.as_str() {
            Some(text) if !text.trim().is_empty() => {
                if normalized == default_locale {
                    has_default = true;
                }
            }
            _ => errors.push(FieldError::new(
                format!("{path}.{key}"),
                "emptyText",
                "a translation must be text that is not empty; leave the locale out to fall back",
            )),
        }
    }
    if !has_default && !errors.iter().any(|error| error.path.starts_with(path) && error.code == "emptyText") {
        errors.push(FieldError::new(
            path,
            "missingDefault",
            format!("the text in the default locale ({default_locale}) is required"),
        ));
    }
}

/// The document as indented JSON in its usual key order (the known keys first, in the order the format describes;
/// the default locale first in each locale map; other keys after, sorted), with a final newline.
pub fn to_pretty(document: &Value) -> String {
    let default_locale = default_locale(document);
    let mut out = String::new();
    write_value(document, Level::Document, &default_locale, 0, &mut out);
    out.push('\n');
    out
}

#[derive(Clone, Copy)]
enum Level {
    Document,
    Rule,
    Reference,
    TextMap,
    Other,
}

const DOCUMENT_KEYS: &[&str] = &["schemaVersion", "defaultLocale", "title", "provenance", "instructions", "rules"];
const RULE_KEYS: &[&str] = &["id", "title", "description", "instructions", "references"];
const REFERENCE_KEYS: &[&str] = &["id", "title", "url"];

fn child_level(level: Level, key: &str) -> Level {
    match (level, key) {
        (Level::Document, "title" | "provenance" | "instructions")
        | (Level::Rule, "title" | "description" | "instructions")
        | (Level::Reference, "title") => Level::TextMap,
        _ => Level::Other,
    }
}

fn ordered_keys<'a>(map: &'a Map<String, Value>, level: Level, default_locale: &str) -> Vec<&'a String> {
    let known: &[&str] = match level {
        Level::Document => DOCUMENT_KEYS,
        Level::Rule => RULE_KEYS,
        Level::Reference => REFERENCE_KEYS,
        Level::TextMap | Level::Other => &[],
    };
    let mut keys: Vec<&String> = Vec::new();
    for name in known {
        if let Some((key, _)) = map.get_key_value(*name) {
            keys.push(key);
        }
    }
    if matches!(level, Level::TextMap) {
        if let Some(key) = map.keys().find(|key| locale::normalize(key).as_deref() == Some(default_locale)) {
            keys.push(key);
        }
    }
    for key in map.keys() {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}

fn write_value(value: &Value, level: Level, default_locale: &str, indent: usize, out: &mut String) {
    let pad = |depth: usize, out: &mut String| out.extend(std::iter::repeat_n(' ', depth * 2));
    match value {
        Value::Object(map) if map.is_empty() => out.push_str("{}"),
        Value::Object(map) => {
            out.push_str("{\n");
            let keys = ordered_keys(map, level, default_locale);
            for (index, key) in keys.iter().enumerate() {
                pad(indent + 1, out);
                out.push_str(&serde_json::to_string(key).unwrap_or_default());
                out.push_str(": ");
                let child = &map[key.as_str()];
                let next = if matches!(level, Level::Document) && key.as_str() == "rules" {
                    Level::Rule
                } else if matches!(level, Level::Rule) && key.as_str() == "references" {
                    Level::Reference
                } else {
                    child_level(level, key)
                };
                write_value(child, next, default_locale, indent + 1, out);
                if index + 1 < keys.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            pad(indent, out);
            out.push('}');
        }
        Value::Array(items) if items.is_empty() => out.push_str("[]"),
        Value::Array(items) => {
            out.push_str("[\n");
            for (index, item) in items.iter().enumerate() {
                pad(indent + 1, out);
                write_value(item, level, default_locale, indent + 1, out);
                if index + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            pad(indent, out);
            out.push(']');
        }
        other => out.push_str(&serde_json::to_string(other).unwrap_or_default()),
    }
}
