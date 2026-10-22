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

//! The shared result formatter of the tools (plans/LLM-A2A-Assist.md 7.4 and 7.7): errors that teach
//! ([`ToolError`]), whole-value shrinking of JSON results with priority keys ([`shrink`]), cut strings with their
//! marker ([`cut`]), compact pipe tables ([`table`]), and the record references of an answer, which may only be
//! paths a tool returned ([`seen_in`], [`check_answer`]).

use std::collections::BTreeSet;

use serde_json::{Map, Value as Json};

use super::ToolOutcome;

/// Keys a shrink keeps whole and cuts last.
pub const PRIORITY_KEYS: [&str; 11] = [
    "error",
    "instruction",
    "hint",
    "approval",
    "feedback",
    "ok",
    "status",
    "message",
    "total",
    "resolved",
    "artifact",
];

/// The keys of an error that are never trimmed, in the order they are written.
const ERROR_KEYS: [&str; 7] = [
    "error",
    "instruction",
    "hint",
    "did_you_mean",
    "skill",
    "rule",
    "ignored",
];

/// Cells of a table are cut here.
pub const CELL_CAP: usize = 60;

/// An error the model reads: what went wrong as a code, the next step in one or two sentences, and what helps it take
/// that step. Written as JSON in a fixed key order ([`ToolError::to_text`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolError {
    pub code: String,
    pub instruction: String,
    pub did_you_mean: Vec<String>,
    /// The keys most errors do not have, boxed to keep a `Result` small.
    pub more: Box<ErrorMore>,
}

/// The less common keys of a [`ToolError`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ErrorMore {
    pub hint: Option<String>,
    /// `name#section` of the skill that has the rule.
    pub skill: Option<String>,
    pub rule: Option<String>,
    pub ignored: Vec<String>,
    /// More keys, after the fixed ones (`line`, `snippet`).
    pub extra: Vec<(String, Json)>,
}

impl ToolError {
    pub fn new(code: &str, instruction: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            instruction: instruction.into(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.more.hint = Some(hint.into());
        self
    }

    #[must_use]
    pub fn did_you_mean(mut self, names: Vec<String>) -> Self {
        self.did_you_mean = names;
        self
    }

    #[must_use]
    pub fn skill(mut self, skill: impl Into<String>) -> Self {
        self.more.skill = Some(skill.into());
        self
    }

    #[must_use]
    pub fn rule(mut self, rule: impl Into<String>) -> Self {
        self.more.rule = Some(rule.into());
        self
    }

    #[must_use]
    pub fn ignored(mut self, ignored: Vec<String>) -> Self {
        self.more.ignored = ignored;
        self
    }

    #[must_use]
    pub fn with(mut self, key: &str, value: Json) -> Self {
        self.more.extra.push((key.to_string(), value));
        self
    }

    /// `{"error": …, "instruction": …, "hint"?: …, "did_you_mean"?: […], "skill"?: …, "rule"?: …, "ignored"?: […]}`
    /// and then the extra keys, empty ones left out.
    pub fn to_text(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut push = |key: &str, value: Json| parts.push(format!("{}:{value}", Json::String(key.to_string())));
        let more = &self.more;
        push(ERROR_KEYS[0], Json::String(self.code.clone()));
        push(ERROR_KEYS[1], Json::String(self.instruction.clone()));
        if let Some(hint) = &more.hint {
            push(ERROR_KEYS[2], Json::String(hint.clone()));
        }
        if !self.did_you_mean.is_empty() {
            push(ERROR_KEYS[3], strings(&self.did_you_mean));
        }
        if let Some(skill) = &more.skill {
            push(ERROR_KEYS[4], Json::String(skill.clone()));
        }
        if let Some(rule) = &more.rule {
            push(ERROR_KEYS[5], Json::String(rule.clone()));
        }
        if !more.ignored.is_empty() {
            push(ERROR_KEYS[6], strings(&more.ignored));
        }
        for (key, value) in &more.extra {
            push(key, value.clone());
        }
        format!("{{{}}}", parts.join(","))
    }

    /// The failure the run stores: the JSON as it is, with no `Error:` before it.
    pub fn outcome(&self) -> ToolOutcome {
        ToolOutcome::Failed(self.to_text())
    }
}

fn strings(list: &[String]) -> Json {
    Json::Array(list.iter().cloned().map(Json::String).collect())
}

/// Whether a failure text is a [`ToolError`] (it is stored as it is).
pub fn is_error_json(text: &str) -> bool {
    text.starts_with("{\"error\":")
}

/// `text` within `max` characters: cut with `…(+N chars)` when longer.
pub fn cut(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max).collect();
    format!("{kept}…(+{} chars)", count - kept.chars().count())
}

/// A table cell: one line, `|` written `/`, cut at [`CELL_CAP`].
pub fn cell(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|ch| match ch {
            '\n' | '\r' | '\t' => ' ',
            '|' => '/',
            other => other,
        })
        .collect();
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    cut(&flat, CELL_CAP)
}

/// A compact pipe table: the header line and a line a row, empty cells for nulls.
pub fn table(header: &[String], rows: &[Vec<String>]) -> String {
    let mut lines = Vec::with_capacity(rows.len() + 1);
    lines.push(header.iter().map(|name| cell(name)).collect::<Vec<_>>().join(" | "));
    for row in rows {
        lines.push(row.iter().map(|value| cell(value)).collect::<Vec<_>>().join(" | "));
    }
    lines.join("\n")
}

/// `value` as JSON text within `cap` characters, shrunk by whole values as Java's `ToolResultTrim`: lists keep whole
/// elements and say how many more there were, maps keep their keys, strings are cut with `…(+N chars)`; the
/// [`PRIORITY_KEYS`] of an object are shrunk last. Text that is still too long is cut as a last resort.
pub fn shrink(value: &Json, cap: usize) -> String {
    let whole = value.to_string();
    if whole.chars().count() <= cap || cap == 0 {
        return whole;
    }
    for (string_cap, list_cap) in [(400, 20), (200, 10), (80, 5), (40, 3), (20, 1)] {
        let shrunk = shrink_value(value, string_cap, list_cap, false);
        let text = shrunk.to_string();
        if text.chars().count() <= cap {
            return text;
        }
        // the priority values whole, the rest smaller still
        let tighter = shrink_value(value, string_cap / 2, list_cap.max(2) / 2, true);
        let text = tighter.to_string();
        if text.chars().count() <= cap {
            return text;
        }
    }
    cut(&shrink_value(value, 20, 1, true).to_string(), cap)
}

fn shrink_value(value: &Json, string_cap: usize, list_cap: usize, spare_priority: bool) -> Json {
    match value {
        Json::String(text) => Json::String(cut(text, string_cap)),
        Json::Array(items) => {
            let keep = list_cap.max(1).min(items.len());
            let mut out: Vec<Json> = items[..keep]
                .iter()
                .map(|item| shrink_value(item, string_cap, list_cap, spare_priority))
                .collect();
            if items.len() > keep {
                out.push(Json::String(format!("…(+{} more)", items.len() - keep)));
            }
            Json::Array(out)
        }
        Json::Object(map) => {
            let mut out = Map::new();
            for (key, item) in map {
                let shrunk = if spare_priority && PRIORITY_KEYS.contains(&key.as_str()) {
                    shrink_value(item, 400, 20, false)
                } else {
                    shrink_value(item, string_cap, list_cap, spare_priority)
                };
                out.insert(key.clone(), shrunk);
            }
            Json::Object(out)
        }
        other => other.clone(),
    }
}

/// The record references in tool output: the `/apps/` paths it names (a `/qapps/` path as its `/apps/` twin), without
/// their query, and the words that may be ids.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    pub paths: BTreeSet<String>,
    pub words: BTreeSet<String>,
    /// Other links tool output named whole: `http(s)://…`, `mailto:…` and other paths from the root.
    pub urls: BTreeSet<String>,
}

impl Seen {
    pub fn add(&mut self, text: &str) {
        self.urls.extend(other_urls(text));
        for url in app_urls(text) {
            let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
            self.paths.insert(path.to_string());
            for pair in query.split('&') {
                if let Some((_, value)) = pair.split_once('=') {
                    self.words.insert(decode(value));
                }
            }
        }
        for word in text.split(|ch: char| !(ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'))) {
            let word = word.trim_matches('.');
            if !word.is_empty() {
                self.words.insert(word.to_string());
            }
        }
    }

    /// Whether a link is one tool output gave: an `/apps/` path with its ids, or another link whole.
    pub fn knows_link(&self, url: &str) -> bool {
        if url.starts_with("/apps/") || url.starts_with("/qapps/") {
            self.knows(url)
        } else {
            self.urls.contains(url)
        }
    }

    /// Whether a link names a path a tool returned with ids a tool returned.
    pub fn knows(&self, url: &str) -> bool {
        let url = app_url(url);
        let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
        self.paths.contains(path)
            && query
                .split('&')
                .filter_map(|pair| pair.split_once('=').map(|(_, value)| decode(value)))
                .all(|value| value.is_empty() || self.words.contains(&value))
    }
}

/// [`Seen`] of every text in `texts`.
pub fn seen_in<'a>(texts: impl IntoIterator<Item = &'a str>) -> Seen {
    let mut seen = Seen::default();
    for text in texts {
        seen.add(text);
    }
    seen
}

fn decode(text: &str) -> String {
    crate::web::percent_decode(text)
}

/// A `/qapps/` url as its `/apps/` twin.
fn app_url(url: &str) -> String {
    url.strip_prefix("/qapps/")
        .map_or_else(|| url.to_string(), |rest| format!("/apps/{rest}"))
}

/// The `/apps/` and `/qapps/` urls in a text, each as an `/apps/` url, trailing punctuation dropped.
fn app_urls(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for start in ["/apps/", "/qapps/"] {
        let mut from = 0;
        while let Some(at) = text[from..].find(start) {
            let begin = from + at;
            let preceded = text[..begin]
                .chars()
                .next_back()
                .is_some_and(|ch| ch.is_alphanumeric() || ch == '/');
            let end = text[begin..]
                .find(|ch: char| {
                    ch.is_whitespace() || matches!(ch, ')' | ']' | '"' | '\'' | '|' | ',' | '<' | '>' | '`')
                })
                .map_or(text.len(), |len| begin + len);
            if !preceded {
                let url = text[begin..end].trim_end_matches(['.', ';', ':']);
                out.push(app_url(url));
            }
            from = end.max(begin + start.len());
        }
    }
    out
}

/// The links in a text that are not `/apps/` ones: `http://`, `https://` and `mailto:` urls and other paths from the
/// root, each as written, trailing punctuation dropped.
fn other_urls(text: &str) -> Vec<String> {
    text.split(|ch: char| {
        ch.is_whitespace() || matches!(ch, '(' | ')' | '[' | ']' | '<' | '>' | '"' | '\'' | '`' | '|')
    })
    .map(|word| word.trim_end_matches(['.', ',', ';', ':', '!', '?']))
    .filter(|word| {
        word.starts_with("http://")
            || word.starts_with("https://")
            || word.starts_with("mailto:")
            || (word.starts_with('/') && word.len() > 1 && !word.starts_with("/apps/") && !word.starts_with("/qapps/"))
    })
    .map(str::to_string)
    .collect()
}

/// What [`check_answer`] made of an answer: its text with the links no tool returned made plain, and those links.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckedAnswer {
    pub text: String,
    pub invented: Vec<String>,
}

/// The Markdown links (and images) of `answer` that `seen` does not know become their label as plain text: an `/apps/`
/// path or id no tool returned, an external or other link that did not come from tool output (`invented_reference`,
/// plan 7.7). Brackets in a label pair up, as CommonMark reads them. A bare or `<…>` external url no tool returned is
/// listed too, and left as it is for the client to sanitize.
pub fn check_answer(answer: &str, seen: &Seen) -> CheckedAnswer {
    let mut out = String::with_capacity(answer.len());
    let mut invented = Vec::new();
    let mut rest = answer;
    while let Some(open) = rest.find('[') {
        let (before, from_open) = rest.split_at(open);
        out.push_str(before);
        let Some(close) = from_open.find("](") else {
            out.push_str(from_open);
            rest = "";
            break;
        };
        // brackets inside a label pair up; an unpaired [ starts the link
        let mut unpaired: Vec<usize> = Vec::new();
        for (at, ch) in from_open[1..close].char_indices() {
            match ch {
                '[' => unpaired.push(at + 1),
                ']' => {
                    unpaired.pop();
                }
                _ => {}
            }
        }
        if let Some(start) = unpaired.last() {
            out.push_str(&from_open[..*start]);
            rest = &from_open[*start..];
            continue;
        }
        let label = &from_open[1..close];
        let after = &from_open[close + 2..];
        let Some(end) = after.find(')') else {
            out.push_str(from_open);
            rest = "";
            break;
        };
        let target = after[..end].trim();
        let url = target
            .split_whitespace()
            .next()
            .unwrap_or(target)
            .trim_matches(['<', '>']);
        if seen.knows_link(url) {
            out.push_str(&from_open[..close + 2 + end + 1]);
        } else {
            if out.ends_with('!') {
                out.pop();
            }
            out.push_str(label);
            invented.push(url.to_string());
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    for url in other_urls(&out) {
        let external = url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:");
        if external && !seen.urls.contains(&url) && !invented.contains(&url) {
            invented.push(url);
        }
    }
    CheckedAnswer { text: out, invented }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_no_tool_returned_are_made_plain_external_ones_too() {
        let seen = seen_in([
            "Things on /apps/x/Things\nE01 | One\nrows link to /apps/x/Thing?enumId=<enumId>",
            "docs at https://example.com/guide and /rest/s1/x",
        ]);
        let checked = check_answer(
            "See [E01](/apps/x/Thing?enumId=E01), [E99](/apps/x/Thing?enumId=E99), [the guide](https://example.com/guide), \
             [a [nested] label](https://evil.example/steal), [x [y](/apps/x/Thing?enumId=E01) ![pixel](https://evil.example/p.gif \"t\"), \
             [rest](/rest/s1/x) and <https://evil.example/raw>.",
            &seen,
        );
        assert_eq!(
            checked.text,
            "See [E01](/apps/x/Thing?enumId=E01), E99, [the guide](https://example.com/guide), a [nested] label, [x [y](/apps/x/Thing?enumId=E01) pixel, \
             [rest](/rest/s1/x) and <https://evil.example/raw>."
        );
        assert_eq!(
            checked.invented,
            [
                "/apps/x/Thing?enumId=E99",
                "https://evil.example/steal",
                "https://evil.example/p.gif",
                "https://evil.example/raw"
            ]
        );
    }

    #[test]
    fn a_shrink_keeps_whole_values_and_priority_keys() {
        let value = serde_json::json!({"error": "x", "rows": (0..100).collect::<Vec<_>>(), "note": "y".repeat(500)});
        let text = shrink(&value, 300);
        assert!(text.chars().count() <= 300, "{text}");
        assert!(
            text.contains("\"error\":\"x\"") && text.contains("more)") && text.contains("chars)"),
            "{text}"
        );
    }
}
