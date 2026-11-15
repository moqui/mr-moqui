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

//! Completion for the `code` component. In XML the choices come from a schema digest (`get#SchemaDigest`, its
//! `elements` with their `children` and `attributes`): where the cursor is in the text before it says whether an
//! element, an attribute of the tag, or one of an attribute's values goes there. Any other answer is a list of choices.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::highlight::{is_name_char, starts};

/// One choice of the completion list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Choice {
    pub label: String,
    /// What replaces the letters typed.
    pub insert: String,
    pub detail: String,
    /// How far the cursor goes back from the end of the insert: into an attribute's quotes.
    pub back: usize,
}

/// Where the cursor is in an XML text.
#[derive(Debug, PartialEq, Eq)]
enum Spot {
    /// Between tags: an element goes here, written with its `<`.
    Text,
    /// Just after a `<`: an element's name.
    Name,
    /// Among a tag's attributes: the ones it does not have yet.
    Attribute { element: String, present: Vec<String> },
    /// Inside an attribute's quotes.
    Value { element: String, attribute: String },
    /// In a comment, CDATA, a declaration or a closing tag: nothing.
    Nothing,
}

/// The name of a text's root element: its first tag that is not a declaration or a comment.
pub(crate) fn root_element(text: &str) -> String {
    let mut rest = text;
    while let Some((_, after)) = rest.split_once('<') {
        if after.starts_with(|ch: char| ch.is_alphabetic() || ch == '_') {
            return after.chars().take_while(|ch| is_name_char(*ch)).collect();
        }
        rest = after;
    }
    String::new()
}

fn find(chars: &[char], from: usize, what: &str) -> Option<usize> {
    (from..chars.len()).find(|at| starts(chars, *at, what))
}

fn name_end(chars: &[char], from: usize) -> usize {
    let mut at = from;
    while at < chars.len() && is_name_char(chars[at]) {
        at += 1;
    }
    at
}

/// Where the end of `before` is, and the elements open there, outermost first.
fn spot(before: &[char]) -> (Spot, Vec<String>) {
    let len = before.len();
    let mut open: Vec<String> = Vec::new();
    let mut at = 0;
    while at < len {
        if before[at] != '<' {
            at += 1;
            continue;
        }
        let close = if starts(before, at, "<!--") {
            find(before, at + 4, "-->").map(|end| end + 3)
        } else if starts(before, at, "<![CDATA[") {
            find(before, at + 9, "]]>").map(|end| end + 3)
        } else if starts(before, at, "<?") {
            find(before, at + 2, "?>").map(|end| end + 2)
        } else if starts(before, at, "<!") {
            find(before, at + 2, ">").map(|end| end + 1)
        } else if starts(before, at, "</") {
            let name: String = before[at + 2..name_end(before, at + 2)].iter().collect();
            let end = find(before, at + 2, ">").map(|end| end + 1);
            if end.is_some() {
                if let Some(found) = open.iter().rposition(|element| *element == name) {
                    open.truncate(found);
                }
            }
            end
        } else {
            let start = at + 1;
            let end = name_end(before, start);
            if end == len {
                return (Spot::Name, open);
            }
            if end == start {
                // a `<` that starts no tag
                at += 1;
                continue;
            }
            let element: String = before[start..end].iter().collect();
            match tag(before, end, &element) {
                Ok((next, closed)) => {
                    if !closed {
                        open.push(element);
                    }
                    Some(next)
                }
                Err(spot) => return (spot, open),
            }
        };
        let Some(next) = close else {
            return (Spot::Nothing, open);
        };
        at = next;
    }
    (Spot::Text, open)
}

/// The attributes of a tag from `at`, after its name: where the tag ends and whether it closed itself, or the spot when
/// the text ends inside it.
fn tag(chars: &[char], mut at: usize, element: &str) -> Result<(usize, bool), Spot> {
    let len = chars.len();
    let mut present = Vec::new();
    loop {
        while at < len && chars[at].is_whitespace() {
            at += 1;
        }
        if at >= len {
            return Err(Spot::Attribute {
                element: element.to_string(),
                present,
            });
        }
        if chars[at] == '>' {
            return Ok((at + 1, false));
        }
        if starts(chars, at, "/>") {
            return Ok((at + 2, true));
        }
        if !is_name_char(chars[at]) {
            at += 1;
            continue;
        }
        let end = name_end(chars, at);
        if end == len {
            return Err(Spot::Attribute {
                element: element.to_string(),
                present,
            });
        }
        let attribute: String = chars[at..end].iter().collect();
        at = end;
        while at < len && chars[at].is_whitespace() {
            at += 1;
        }
        if at < len && chars[at] == '=' {
            at += 1;
            while at < len && chars[at].is_whitespace() {
                at += 1;
            }
            if at < len && (chars[at] == '"' || chars[at] == '\'') {
                let quote = chars[at];
                match chars[at + 1..].iter().position(|ch| *ch == quote) {
                    Some(found) => at += found + 2,
                    None => {
                        return Err(Spot::Value {
                            element: element.to_string(),
                            attribute,
                        })
                    }
                }
            }
        }
        present.push(attribute);
    }
}

fn element<'a>(digest: &'a Value, name: &str) -> Option<&'a Value> {
    digest["elements"]
        .as_array()?
        .iter()
        .find(|spec| spec["name"].as_str() == Some(name))
}

/// The first line of a doc, short.
fn summary(doc: &str) -> String {
    let first = doc.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("");
    let short: String = first.chars().take(60).collect();
    if short.len() < first.len() {
        format!("{short}…")
    } else {
        short
    }
}

/// The choices for the cursor at `cursor` (a char offset) in an XML text, from a schema digest, and the char offset
/// where the letters they replace start.
pub(crate) fn xml(digest: &Value, text: &str, cursor: usize) -> (usize, Vec<Choice>) {
    let before: Vec<char> = text.chars().take(cursor).collect();
    let (spot, open) = spot(&before);
    let typed = before.iter().rev().take_while(|ch| is_name_char(**ch)).count();
    let from = before.len() - typed;
    let elements = |with_bracket: bool| -> Vec<Choice> {
        let names: Vec<String> = match open.last() {
            Some(parent) => element(digest, parent)
                .and_then(|spec| spec["children"].as_array())
                .map(|children| children.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default(),
            None => digest["elements"]
                .as_array()
                .map(|specs| {
                    specs
                        .iter()
                        .filter(|spec| spec["top"] == true)
                        .filter_map(|spec| spec["name"].as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        };
        names
            .into_iter()
            .map(|name| Choice {
                detail: element(digest, &name)
                    .and_then(|spec| spec["doc"].as_str())
                    .map(summary)
                    .unwrap_or_default(),
                insert: if with_bracket { format!("<{name}") } else { name.clone() },
                label: name,
                back: 0,
            })
            .collect()
    };
    let choices = match spot {
        Spot::Text => elements(true),
        Spot::Name => elements(false),
        Spot::Attribute { element: name, present } => element(digest, &name)
            .and_then(|spec| spec["attributes"].as_array())
            .map(|attributes| {
                attributes
                    .iter()
                    .filter_map(|attribute| {
                        let name = attribute["name"].as_str()?;
                        if present.iter().any(|have| have == name) {
                            return None;
                        }
                        let detail = if attribute["required"] == true {
                            "required".to_string()
                        } else if let Some(default) = attribute["default"].as_str() {
                            format!("default {default}")
                        } else {
                            summary(attribute["doc"].as_str().unwrap_or(""))
                        };
                        Some(Choice {
                            label: name.to_string(),
                            insert: format!("{name}=\"\""),
                            detail,
                            back: 1,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        Spot::Value {
            element: name,
            attribute,
        } => {
            let typed = before.iter().rev().take_while(|ch| !matches!(ch, '"' | '\'')).count();
            let values: Vec<Choice> = element(digest, &name)
                .and_then(|spec| spec["attributes"].as_array())
                .and_then(|attributes| attributes.iter().find(|spec| spec["name"].as_str() == Some(&attribute)))
                .and_then(|spec| spec["values"].as_array())
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|value| Choice {
                            label: value.to_string(),
                            insert: value.to_string(),
                            ..Choice::default()
                        })
                        .collect()
                })
                .unwrap_or_default();
            return (before.len() - typed, values);
        }
        Spot::Nothing => Vec::new(),
    };
    (from, choices)
}

/// A completion answer that is a list: `items` or `completions` of the answer, or the answer itself; each a string, or
/// `{label, insert, detail}`.
pub(crate) fn items(value: &Value) -> Option<Vec<Choice>> {
    let list = value["items"]
        .as_array()
        .or_else(|| value["completions"].as_array())
        .or_else(|| value.as_array())?;
    let text = |item: &Value, keys: &[&str]| {
        keys.iter()
            .find_map(|key| item[*key].as_str())
            .unwrap_or("")
            .to_string()
    };
    Some(
        list.iter()
            .filter_map(|item| {
                if let Some(word) = item.as_str() {
                    return Some(Choice {
                        label: word.to_string(),
                        insert: word.to_string(),
                        ..Choice::default()
                    });
                }
                let label = text(item, &["label", "name", "value"]);
                if label.is_empty() {
                    return None;
                }
                let insert = text(item, &["insert", "insertText"]);
                Some(Choice {
                    insert: if insert.is_empty() { label.clone() } else { insert },
                    detail: text(item, &["detail", "doc"]),
                    label,
                    back: 0,
                })
            })
            .collect(),
    )
}

/// The char offset where the word before `cursor` starts, for a list's choices.
pub(crate) fn word_start(text: &str, cursor: usize) -> usize {
    let before: Vec<char> = text.chars().take(cursor).collect();
    let typed = before
        .iter()
        .rev()
        .take_while(|ch| is_name_char(**ch) || **ch == '$')
        .count();
    before.len() - typed
}
