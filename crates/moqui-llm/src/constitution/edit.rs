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

//! Edits of a Constitution document (docs/ai/Constitution.md, "Editing"), one operation at a time, with no storage:
//! the editor of System › Constitution and of an organization's document both apply them and then save the whole
//! document. Rule and reference ids stay with their rules and references; new ones take ids not used yet, and a
//! removal renumbers nothing.

use serde_json::{json, Map, Value};

use super::{document, locale};

/// Which way a move goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
}

/// One edit of a document. Text is set in one locale; an empty text removes that locale's translation (the default
/// locale's text cannot be removed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    /// The document's title, provenance or shared instructions in a locale (`None` leaves a field as it is).
    SetDocumentText { locale: String, title: Option<String>, provenance: Option<String>, instructions: Option<String> },
    SetDefaultLocale { locale: String },
    /// A new rule at the end, with its text in the default locale; out of `apply` with the new id.
    AddRule { title: String, description: String, instructions: String },
    SetRuleText {
        rule_id: String,
        locale: String,
        title: Option<String>,
        description: Option<String>,
        instructions: Option<String>,
    },
    RemoveRule { rule_id: String },
    MoveRule { rule_id: String, direction: Direction },
    /// A new reference at the end of a rule's list, its title in the default locale.
    AddReference { rule_id: String, title: String, url: String },
    SetReference { reference_id: String, locale: String, title: Option<String>, url: Option<String> },
    RemoveReference { reference_id: String },
    MoveReference { reference_id: String, direction: Direction },
    /// Every translation in a locale other than the default.
    RemoveLocale { locale: String },
}

impl Edit {
    /// The edit named `operation` from text parameters (`get` by name), as the services take it.
    ///
    /// # Errors
    ///
    /// Returns a message for an unknown operation or a missing parameter.
    pub fn from_parameters(operation: &str, get: &dyn Fn(&str) -> Option<String>) -> Result<Self, String> {
        let need = |name: &str| get(name).ok_or_else(|| format!("{operation} needs {name}"));
        let direction = || match get("direction").as_deref() {
            Some("up") => Ok(Direction::Up),
            Some("down") => Ok(Direction::Down),
            _ => Err(format!("{operation} needs direction up or down")),
        };
        // A text parameter given empty clears that translation; one left out is unchanged.
        let optional = |name: &str| get(name);
        Ok(match operation {
            "setDocumentText" => Self::SetDocumentText {
                locale: need("locale")?,
                title: optional("title"),
                provenance: optional("provenance"),
                instructions: optional("instructions"),
            },
            "setDefaultLocale" => Self::SetDefaultLocale { locale: need("locale")? },
            "addRule" => Self::AddRule {
                title: need("title")?,
                description: need("description")?,
                instructions: need("instructions")?,
            },
            "setRuleText" => Self::SetRuleText {
                rule_id: need("ruleId")?,
                locale: need("locale")?,
                title: optional("title"),
                description: optional("description"),
                instructions: optional("instructions"),
            },
            "removeRule" => Self::RemoveRule { rule_id: need("ruleId")? },
            "moveRule" => Self::MoveRule { rule_id: need("ruleId")?, direction: direction()? },
            "addReference" => Self::AddReference { rule_id: need("ruleId")?, title: need("title")?, url: need("url")? },
            "setReference" => Self::SetReference {
                reference_id: need("referenceId")?,
                locale: need("locale")?,
                title: optional("title"),
                url: optional("url"),
            },
            "removeReference" => Self::RemoveReference { reference_id: need("referenceId")? },
            "moveReference" => Self::MoveReference { reference_id: need("referenceId")?, direction: direction()? },
            "removeLocale" => Self::RemoveLocale { locale: need("locale")? },
            other => return Err(format!("unknown Constitution edit {other}")),
        })
    }
}

/// What an edit made: the new document, and the id of a rule or reference it added.
#[derive(Clone, Debug, PartialEq)]
pub struct Applied {
    pub document: Value,
    pub new_id: Option<String>,
}

/// `edit` applied to a copy of `doc`. The result is not validated here; the caller validates before saving.
///
/// # Errors
///
/// Returns a message when the edit names a rule, reference or locale that is not there, or would remove the default
/// locale's text.
pub fn apply(doc: &Value, edit: &Edit) -> Result<Applied, String> {
    let mut document = doc.clone();
    let default_locale = document::default_locale(&document);
    let mut new_id = None;
    match edit {
        Edit::SetDocumentText { locale, title, provenance, instructions } => {
            let locale = normalized(locale)?;
            let root = object(&mut document)?;
            for (field, text) in [("title", title), ("provenance", provenance), ("instructions", instructions)] {
                if let Some(text) = text {
                    set_text(root, field, &locale, text, &default_locale)?;
                }
            }
        }
        Edit::SetDefaultLocale { locale } => {
            let locale = normalized(locale)?;
            object(&mut document)?.insert("defaultLocale".to_string(), Value::String(locale));
        }
        Edit::AddRule { title, description, instructions } => {
            let id = next_rule_id(&document);
            let rule = json!({
                "id": id,
                "title": {default_locale.clone(): title.trim()},
                "description": {default_locale.clone(): description.trim()},
                "instructions": {default_locale.clone(): instructions.trim()},
                "references": [],
            });
            rules_mut(&mut document)?.push(rule);
            new_id = Some(id);
        }
        Edit::SetRuleText { rule_id, locale, title, description, instructions } => {
            let locale = normalized(locale)?;
            let rule = rule_mut(&mut document, rule_id)?;
            for (field, text) in [("title", title), ("description", description), ("instructions", instructions)] {
                if let Some(text) = text {
                    set_text(rule, field, &locale, text, &default_locale)?;
                }
            }
        }
        Edit::RemoveRule { rule_id } => {
            let rules = rules_mut(&mut document)?;
            let index = rules.iter().position(|rule| document::id_of(rule) == rule_id).ok_or_else(|| no_rule(rule_id))?;
            rules.remove(index);
        }
        Edit::MoveRule { rule_id, direction } => {
            let rules = rules_mut(&mut document)?;
            let index = rules.iter().position(|rule| document::id_of(rule) == rule_id).ok_or_else(|| no_rule(rule_id))?;
            move_in(rules, index, *direction);
        }
        Edit::AddReference { rule_id, title, url } => {
            let id = next_reference_id(&document, rule_id);
            let rule = rule_mut(&mut document, rule_id)?;
            let reference = json!({"id": id, "title": {default_locale.clone(): title.trim()}, "url": url.trim()});
            match rule.get_mut("references") {
                Some(Value::Array(list)) => list.push(reference),
                _ => {
                    rule.insert("references".to_string(), Value::Array(vec![reference]));
                }
            }
            new_id = Some(id);
        }
        Edit::SetReference { reference_id, locale, title, url } => {
            let locale = normalized(locale)?;
            let (list, index) = reference_list(&mut document, reference_id)?;
            let reference = list[index].as_object_mut().ok_or_else(|| no_reference(reference_id))?;
            if let Some(title) = title {
                set_text(reference, "title", &locale, title, &default_locale)?;
            }
            if let Some(url) = url {
                reference.insert("url".to_string(), Value::String(url.trim().to_string()));
            }
        }
        Edit::RemoveReference { reference_id } => {
            let (list, index) = reference_list(&mut document, reference_id)?;
            list.remove(index);
        }
        Edit::MoveReference { reference_id, direction } => {
            let (list, index) = reference_list(&mut document, reference_id)?;
            move_in(list, index, *direction);
        }
        Edit::RemoveLocale { locale } => {
            let locale = normalized(locale)?;
            if locale == default_locale {
                return Err(format!("{locale} is the default locale; choose another default locale first"));
            }
            remove_locale(&mut document, &locale);
        }
    }
    Ok(Applied { document, new_id })
}

fn normalized(tag: &str) -> Result<String, String> {
    locale::normalize(tag).ok_or_else(|| format!("{tag:?} is not a locale"))
}

fn object(value: &mut Value) -> Result<&mut Map<String, Value>, String> {
    value.as_object_mut().ok_or_else(|| "the Constitution must be a JSON object".to_string())
}

fn rules_mut(document: &mut Value) -> Result<&mut Vec<Value>, String> {
    let root = object(document)?;
    if !matches!(root.get("rules"), Some(Value::Array(_))) {
        root.insert("rules".to_string(), Value::Array(Vec::new()));
    }
    match root.get_mut("rules") {
        Some(Value::Array(rules)) => Ok(rules),
        _ => Err("the Constitution has no rules list".to_string()),
    }
}

fn rule_mut<'a>(document: &'a mut Value, rule_id: &str) -> Result<&'a mut Map<String, Value>, String> {
    rules_mut(document)?
        .iter_mut()
        .find(|rule| document::id_of(rule) == rule_id)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| no_rule(rule_id))
}

fn reference_list<'a>(document: &'a mut Value, reference_id: &str) -> Result<(&'a mut Vec<Value>, usize), String> {
    for rule in rules_mut(document)? {
        if let Some(Value::Array(list)) = rule.get_mut("references") {
            if let Some(index) = list.iter().position(|reference| document::id_of(reference) == reference_id) {
                return Ok((list, index));
            }
        }
    }
    Err(no_reference(reference_id))
}

fn no_rule(rule_id: &str) -> String {
    format!("the Constitution has no rule {rule_id}")
}

fn no_reference(reference_id: &str) -> String {
    format!("the Constitution has no reference {reference_id}")
}

fn move_in(list: &mut [Value], index: usize, direction: Direction) {
    match direction {
        Direction::Up if index > 0 => list.swap(index, index - 1),
        Direction::Down if index + 1 < list.len() => list.swap(index, index + 1),
        _ => {}
    }
}

/// Set (or with empty text, remove) the `locale` text of the locale map `field` of `owner`.
fn set_text(owner: &mut Map<String, Value>, field: &str, locale: &str, text: &str, default_locale: &str) -> Result<(), String> {
    let text = text.trim();
    if !matches!(owner.get(field), Some(Value::Object(_))) {
        owner.insert(field.to_string(), Value::Object(Map::new()));
    }
    let Some(Value::Object(map)) = owner.get_mut(field) else {
        return Err(format!("{field} is not a locale map"));
    };
    let existing = map.keys().find(|key| locale::normalize(key).as_deref() == Some(locale)).cloned();
    if text.is_empty() {
        if locale == default_locale {
            return Err(format!("the {field} text in the default locale ({default_locale}) is required"));
        }
        if let Some(key) = existing {
            map.remove(&key);
        }
    } else {
        let key = existing.unwrap_or_else(|| locale.to_string());
        map.insert(key, Value::String(text.to_string()));
    }
    Ok(())
}

fn remove_locale(value: &mut Value, locale: &str) {
    fn strip(map: &mut Map<String, Value>, field: &str, locale: &str) {
        if let Some(Value::Object(texts)) = map.get_mut(field) {
            texts.retain(|key, _| locale::normalize(key).as_deref() != Some(locale));
        }
    }
    let Some(root) = value.as_object_mut() else {
        return;
    };
    for field in ["title", "provenance", "instructions"] {
        strip(root, field, locale);
    }
    if let Some(Value::Array(rules)) = root.get_mut("rules") {
        for rule in rules.iter_mut().filter_map(Value::as_object_mut) {
            for field in ["title", "description", "instructions"] {
                strip(rule, field, locale);
            }
            if let Some(Value::Array(references)) = rule.get_mut("references") {
                for reference in references.iter_mut().filter_map(Value::as_object_mut) {
                    strip(reference, "title", locale);
                }
            }
        }
    }
}

/// `C{nn}` one past the highest `C` number in use (`C14` after `C13`), never an id in use.
fn next_rule_id(document: &Value) -> String {
    let used: Vec<&str> = document::rules(document).iter().map(document::id_of).collect();
    let highest = used
        .iter()
        .filter_map(|id| id.strip_prefix('C').and_then(|n| n.parse::<u32>().ok()))
        .max()
        .unwrap_or(0);
    let mut number = highest + 1;
    loop {
        let id = format!("C{number:02}");
        if !used.contains(&id.as_str()) {
            return id;
        }
        number += 1;
    }
}

/// `{rule}-R{nn}` one past the highest of that rule's numbers, unused anywhere in the document.
fn next_reference_id(document: &Value, rule_id: &str) -> String {
    let used: Vec<&str> = document::rules(document)
        .iter()
        .flat_map(|rule| document::references(rule).iter().map(document::id_of))
        .collect();
    let prefix = format!("{rule_id}-R");
    let highest = used
        .iter()
        .filter_map(|id| id.strip_prefix(&prefix).and_then(|n| n.parse::<u32>().ok()))
        .max()
        .unwrap_or(0);
    let mut number = highest + 1;
    loop {
        let id = format!("{prefix}{number:02}");
        if !used.contains(&id.as_str()) {
            return id;
        }
        number += 1;
    }
}
