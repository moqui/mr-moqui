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

//! `search` (plans/LLM-A2A-Assist.md 7.3 and 7.7): what the search box finds, as the user. The provider groups come
//! from the webroot's `shellSearch` transition (`sri.getShellSearch`, the `palette-source` services of the apps the
//! user may open, each in its app's context); Screens and Actions come from the shell data (the menus to depth two and
//! the palette actions) ranked with Java's browse weights: a title word 80, the screen's name 40, a parent's title 10,
//! the whole text in the title +200, a component alias of a word half of its weight ([`SkillIndex::alias_words`]).
//! The answer is one line a hit, at most `limit` (5, up to 10) a group, with a `more:` line where a group has more.

use std::collections::BTreeMap;

use moqui_lib::object::Value;
use serde_json::Value as Json;

use super::apps::{self, ShellEntry};
use super::format::ToolError;
use super::schema::{object, Property};
use super::{AgentTool, Classification, ToolOutcome, ToolSpec};
use crate::llm::run::context::TurnContext;
use crate::llm::skills::SkillIndex;

const DEFAULT_LIMIT: usize = 5;
const MAX_LIMIT: usize = 10;

pub struct Search(ToolSpec);

impl Default for Search {
    fn default() -> Self {
        let mut schema = object(&[
            Property::required("text", "string", "A few plain words or an id"),
            Property::optional("groups", "array", "Only these groups, e.g. Screens"),
            Property::optional("limit", "integer", "Hits a group, at most 10"),
        ]);
        schema["properties"]["groups"]["items"] = serde_json::json!({"type": "string"});
        Self(ToolSpec::new(
            "search",
            "Find records, people, messages, screens, actions and saved finds the user may open, as the search box does. At most 5 hits a group.",
            schema,
            Classification::Read,
        ))
    }
}

impl AgentTool for Search {
    fn spec(&self) -> &ToolSpec {
        &self.0
    }

    fn call(&self, ctx: &mut TurnContext<'_>, args: Json) -> ToolOutcome {
        let text = args
            .get("text")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if text.is_empty() {
            return ToolError::new("no_text", "Call search with a few plain words or an id in text.").outcome();
        }
        let limit = args
            .get("limit")
            .and_then(Json::as_u64)
            .and_then(|limit| usize::try_from(limit).ok())
            .filter(|limit| *limit > 0)
            .map_or(DEFAULT_LIMIT, |limit| limit.min(MAX_LIMIT));
        let wanted: Vec<String> = args
            .get("groups")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
            .map(|group| group.trim().to_lowercase())
            .filter(|group| !group.is_empty())
            .collect();
        let wants = |group: &str, label: &str| {
            wanted.is_empty()
                || wanted
                    .iter()
                    .any(|name| name == &group.to_lowercase() || name == &label.to_lowercase())
        };
        let index = apps::factory()
            .map(|factory| SkillIndex::with_aliases(&factory))
            .unwrap_or_default();
        let mut lines: Vec<String> = Vec::new();
        if wants("screens", "Screens") || wants("actions", "Actions") {
            let entries = apps::shell_entries(ctx);
            if wants("screens", "Screens") {
                add_group(
                    &mut lines,
                    "Screens",
                    rank(&entries.screens, &text, &index),
                    limit,
                    &text,
                );
            }
            if wants("actions", "Actions") {
                add_group(
                    &mut lines,
                    "Actions",
                    rank(&entries.actions, &text, &index),
                    limit,
                    &text,
                );
            }
        }
        let mut parameters = BTreeMap::new();
        parameters.insert("text".to_string(), Value::string(&text));
        parameters.insert("limit".to_string(), Value::string((limit + 1).to_string()));
        match apps::render_json(ctx, "/shellSearch", parameters) {
            Ok(found) => {
                for group in found.get("groups").and_then(Json::as_array).into_iter().flatten() {
                    let name = group.get("group").and_then(Json::as_str).unwrap_or_default();
                    let label = group.get("label").and_then(Json::as_str).unwrap_or(name);
                    if !wants(name, label) {
                        continue;
                    }
                    let hits: Vec<Hit> = group
                        .get("rows")
                        .and_then(Json::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|row| {
                            let url = row.get("url").and_then(Json::as_str)?;
                            Some(Hit {
                                title: row.get("title").and_then(Json::as_str).unwrap_or(url).to_string(),
                                note: row
                                    .get("subtitle")
                                    .and_then(Json::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                                path: apps::app_path(url),
                            })
                        })
                        .collect();
                    let more = group.get("moreUrl").and_then(Json::as_str).map(apps::app_path);
                    let shown = hits.len().min(limit);
                    add_hits(&mut lines, label, &hits[..shown]);
                    match more {
                        Some(more) => lines.push(format!("more: {label} → {more}")),
                        None if hits.len() > limit => {
                            lines.push(format!("more: search text=\"{text}\" groups=[\"{name}\"] limit=10"));
                        }
                        None => {}
                    }
                }
            }
            Err(error) => tracing::error!("the palette sources could not be searched for an agent: {error}"),
        }
        if lines.is_empty() {
            return ToolOutcome::text(format!(
                "No hits for \"{text}\". Try other words, an id, or fewer groups."
            ));
        }
        ToolOutcome::text(lines.join("\n"))
    }
}

/// One hit: its title, a short note (the trail, a subtitle) and its path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub title: String,
    pub note: String,
    pub path: String,
}

fn add_hits(lines: &mut Vec<String>, label: &str, hits: &[Hit]) {
    for hit in hits {
        let note = if hit.note.is_empty() {
            String::new()
        } else {
            format!(" ({})", super::format::cut(&hit.note, 60))
        };
        lines.push(format!(
            "{label}: {}{note} → {}",
            super::format::cut(&hit.title, 80),
            hit.path
        ));
    }
}

fn add_group(lines: &mut Vec<String>, label: &str, hits: Vec<Hit>, limit: usize, text: &str) {
    add_hits(lines, label, &hits[..hits.len().min(limit)]);
    if hits.len() > limit {
        lines.push(format!(
            "more: search text=\"{text}\" groups=[\"{label}\"] limit={MAX_LIMIT}"
        ));
    }
}

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// A query word and its stem (Carbon's query-only stemming, plan 3): plurals (`accounts`, `entries`, `boxes`) and the
/// simple suffixes `-ing` and `-ed`. Short words are left as they are.
pub fn word_forms(word: &str) -> Vec<String> {
    let mut forms = vec![word.to_string()];
    let length = word.chars().count();
    let stem = if length > 4 && word.ends_with("ies") {
        Some(format!("{}y", &word[..word.len() - 3]))
    } else if length > 4 && ["ches", "shes", "sses", "uses", "xes", "zes"].iter().any(|end| word.ends_with(end)) {
        Some(word[..word.len() - 2].to_string())
    } else if length > 3 && word.ends_with('s') && !word.ends_with("ss") && !word.ends_with("us") {
        Some(word[..word.len() - 1].to_string())
    } else if length > 5 && word.ends_with("ing") {
        Some(undouble(&word[..word.len() - 3]))
    } else if length > 4 && word.ends_with("ed") {
        Some(undouble(&word[..word.len() - 2]))
    } else {
        None
    };
    forms.extend(stem.filter(|stem| stem != word));
    forms
}

/// A stem whose last consonant was doubled for the suffix (`runn`, `stopp`) without the second one.
fn undouble(stem: &str) -> String {
    let chars: Vec<char> = stem.chars().collect();
    match chars.as_slice() {
        [.., a, b] if a == b && !"aeiouls".contains(*b) => chars[..chars.len() - 1].iter().collect(),
        _ => stem.to_string(),
    }
}

/// The weight of one query word in an entry, 0 when it is not there.
fn word_weight(word: &str, title_words: &[String], name: &str, trail: &str) -> i64 {
    if title_words.iter().any(|known| known.starts_with(word)) {
        80
    } else if name.contains(word) {
        40
    } else if trail.contains(word) {
        10
    } else {
        0
    }
}

/// The entries that match every word of `text` (or an alias of it), best first; ties keep the menu order.
pub fn rank(entries: &[ShellEntry], text: &str, index: &SkillIndex) -> Vec<Hit> {
    let query = words(text);
    if query.is_empty() {
        return Vec::new();
    }
    let phrase = text.trim().to_lowercase();
    let mut scored: Vec<(i64, usize, Hit)> = Vec::new();
    for (at, entry) in entries.iter().enumerate() {
        let title_words = words(&entry.title);
        let name = entry
            .path
            .split('?')
            .next()
            .unwrap_or_default()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_lowercase();
        let trail = entry.trail.to_lowercase();
        let mut score = 0;
        let mut all = true;
        for word in &query {
            // the word, else its stem (the query's only: the entries are matched by prefix and containment)
            let forms = word_forms(word);
            let own = forms
                .iter()
                .map(|form| word_weight(form, &title_words, &name, &trail))
                .max()
                .unwrap_or(0);
            let alias = forms
                .iter()
                .flat_map(|form| index.alias_words(form))
                .map(|alias| word_weight(&alias, &title_words, &name, &trail) / 2)
                .max()
                .unwrap_or(0);
            let weight = own.max(alias);
            if weight == 0 {
                all = false;
                break;
            }
            score += weight;
        }
        if !all {
            continue;
        }
        if entry.title.to_lowercase().contains(&phrase) {
            score += 200;
        }
        scored.push((
            score,
            at,
            Hit {
                title: entry.title.clone(),
                note: entry.trail.clone(),
                path: entry.path.clone(),
            },
        ));
    }
    scored.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(&right.1)));
    scored.into_iter().map(|(_, _, hit)| hit).collect()
}

crate::submit_tool!(Search);

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str, trail: &str, path: &str) -> ShellEntry {
        ShellEntry {
            title: title.to_string(),
            trail: trail.to_string(),
            path: path.to_string(),
        }
    }

    #[test]
    fn screens_rank_by_title_name_and_alias_words() {
        let entries = vec![
            entry("Security", "System", "/apps/system/Security"),
            entry(
                "User Accounts",
                "System › Security",
                "/apps/system/Security/UserAccount",
            ),
            entry("Users", "System › Security", "/apps/system/Security/UserGroup"),
            entry("Customers", "Sales", "/apps/sales/Party"),
        ];
        let mut index = SkillIndex::default();
        let hits = rank(&entries, "user accounts", &index);
        assert_eq!(hits[0].path, "/apps/system/Security/UserAccount");
        assert_eq!(hits.len(), 1, "{hits:?}");
        let users = rank(&entries, "user", &index);
        assert_eq!(users.len(), 2);
        // a parent's title is the weakest match
        let security = rank(&entries, "security", &index);
        assert_eq!(security[0].path, "/apps/system/Security");
        assert!(rank(&entries, "client", &index).is_empty());
        index.aliases.insert("customer".to_string(), vec!["client".to_string()]);
        assert_eq!(rank(&entries, "client", &index)[0].path, "/apps/sales/Party");
        assert_eq!(rank(&entries, "clients", &index)[0].path, "/apps/sales/Party", "an alias of the stem");
    }

    #[test]
    fn a_plural_or_a_suffix_of_the_query_finds_the_singular_screen() {
        let entries = vec![
            entry("Users", "System › Security", "/apps/system/Security/UserAccount"),
            entry("Status Flows", "Tools", "/apps/tools/StatusFlows"),
            entry("Service Job Runs", "System", "/apps/system/ServiceJob/JobRuns"),
        ];
        let index = SkillIndex::default();
        // "accounts" is in no title; its stem is in the screen's name
        assert_eq!(rank(&entries, "user accounts", &index)[0].path, "/apps/system/Security/UserAccount");
        assert_eq!(rank(&entries, "statuses", &index)[0].path, "/apps/tools/StatusFlows");
        assert_eq!(rank(&entries, "running jobs", &index)[0].path, "/apps/system/ServiceJob/JobRuns");
        assert_eq!(word_forms("entries"), ["entries", "entry"]);
        assert_eq!(word_forms("boxes"), ["boxes", "box"]);
        assert_eq!(word_forms("statuses"), ["statuses", "status"]);
        assert_eq!(word_forms("running"), ["running", "run"]);
        assert_eq!(word_forms("filled"), ["filled", "fill"]);
        assert_eq!(word_forms("status"), ["status"], "not a plural");
        assert_eq!(word_forms("class"), ["class"]);
        assert_eq!(word_forms("is"), ["is"]);
    }
}
