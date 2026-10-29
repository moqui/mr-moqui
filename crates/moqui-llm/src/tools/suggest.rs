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

//! Did-you-mean (plans/LLM-A2A-Assist.md 4.6 and 7.3): the nearest names by edit distance with a bonus for a shared
//! prefix ([`nearest`]), for elements, tools, skills, lists and paths; the synonym map that routes common wrong tool
//! names to the tool that does the job; and the answer to a call of a tool that is not offered ([`unknown_tool`]),
//! which turns a direct call of a lazy catalog name (phase 13.4) into guidance to call `run_tool`.

use super::format::ToolError;

/// Common wrong tool names, the tool that does their job, and how.
const SYNONYMS: &[(&str, &str, &str)] = &[
    ("navigate", "screen", "Call screen with go to open a screen."),
    ("open", "screen", "Call screen with go to open a screen."),
    ("goto", "screen", "Call screen with go to open a screen."),
    ("go", "screen", "Call screen with go to open a screen."),
    ("open_screen", "screen", "Call screen with go to open a screen."),
    ("click", "screen", "Call screen with do to press a button, link or row."),
    ("press", "screen", "Call screen with do to press a button, link or row."),
    ("submit", "screen", "Call screen with do to press the form's button."),
    ("fill", "screen", "Call screen with set to fill fields."),
    ("type", "screen", "Call screen with set to fill fields."),
    (
        "browse",
        "search",
        "Call search with a few words, or data with a screen path and list.",
    ),
    ("find", "search", "Call search with a few plain words or an id."),
    ("lookup", "search", "Call search with a few plain words or an id."),
    ("request", "data", "Call data with a screen path and list to read rows."),
    (
        "query",
        "data",
        "Call data with a screen path and list to read rows, or search to find a record.",
    ),
    ("list", "data", "Call data with a screen path and list to read rows."),
    ("read", "data", "Call data with a screen path and list to read rows."),
    ("question", "ask", "Call ask with the question and 2 to 4 options."),
    ("load_skill", "skill", "Call skill with the skill's name."),
];

/// The edit distance of two names, case ignored.
pub fn distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.to_lowercase().chars().collect();
    let right: Vec<char> = right.to_lowercase().chars().collect();
    let mut row: Vec<usize> = (0..=right.len()).collect();
    for (i, l) in left.iter().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, r) in right.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = if l == r {
                previous
            } else {
                1 + previous.min(above).min(row[j])
            };
            previous = above;
        }
    }
    row[right.len()]
}

fn common_prefix(left: &str, right: &str) -> usize {
    left.to_lowercase()
        .chars()
        .zip(right.to_lowercase().chars())
        .take_while(|(l, r)| l == r)
        .count()
}

/// Up to `max` of `candidates` nearest `wanted`, best first: the edit distance less a bonus for a shared prefix (up to
/// 4) and for one name holding the other; a candidate too far for its length is left out. Ties go by name.
pub fn nearest<'a>(wanted: &str, candidates: impl IntoIterator<Item = &'a str>, max: usize) -> Vec<String> {
    let wanted = wanted.trim();
    if wanted.is_empty() || max == 0 {
        return Vec::new();
    }
    let lower = wanted.to_lowercase();
    let mut scored: Vec<(i64, String)> = Vec::new();
    for candidate in candidates {
        if candidate.is_empty() || scored.iter().any(|(_, known)| known == candidate) {
            continue;
        }
        let other = candidate.to_lowercase();
        let distance = i64::try_from(distance(&lower, &other)).unwrap_or(i64::MAX);
        let prefix = i64::try_from(common_prefix(&lower, &other).min(4)).unwrap_or(0);
        let holds = lower.len() >= 3 && (other.contains(&lower) || lower.contains(&other));
        let score = distance - prefix - if holds { 3 } else { 0 };
        let longest = i64::try_from(lower.chars().count().max(other.chars().count())).unwrap_or(i64::MAX);
        let near = distance <= (longest / 3).max(2) || prefix >= 3 || holds;
        if near {
            scored.push((score, candidate.to_string()));
        }
    }
    scored.sort();
    scored.into_iter().take(max).map(|(_, name)| name).collect()
}

/// The answer to a call of `name`, which is not offered: a lazy catalog tool called directly is told to go through
/// `run_tool` (when the run has it); a synonym of an offered tool says how to use that one; else the nearest offered
/// names.
pub fn unknown_tool(name: &str, offered: &[&str], lazy: &[String]) -> ToolError {
    if lazy.iter().any(|known| known == name) {
        return lazy_call(name, offered);
    }
    let lower = name.trim().to_lowercase();
    if let Some((_, tool, how)) = SYNONYMS
        .iter()
        .find(|(synonym, tool, _)| *synonym == lower && offered.contains(tool))
    {
        return ToolError::new("unknown_tool", format!("There is no tool named {name}. {how}"))
            .did_you_mean(vec![(*tool).to_string()]);
    }
    let near = nearest(name, offered.iter().copied(), 3);
    let instruction = if near.is_empty() {
        format!("There is no tool named {name}. Use one of the tools you were given.")
    } else {
        format!(
            "There is no tool named {name}. Call {} instead if it does the job.",
            near[0]
        )
    };
    let error = ToolError::new("unknown_tool", instruction);
    if near.is_empty() {
        error.hint(format!("The tools are: {}.", offered.join(", ")))
    } else {
        error.did_you_mean(near)
    }
}

/// A lazy tool (a service, an MCP or A2A tool of the catalog) called by its own name: the call goes through
/// `run_tool`. Until phase 13.4 offers `run_tool` there is no way to it, and the model is told so.
fn lazy_call(name: &str, offered: &[&str]) -> ToolError {
    if offered.contains(&"run_tool") {
        ToolError::new(
            "lazy_tool",
            format!(
                "{name} is in the tool catalog, not a tool of its own. Call run_tool with name \"{name}\" and its parameters; describe_tools shows them."
            ),
        )
        .did_you_mean(vec!["run_tool".to_string()])
    } else {
        ToolError::new(
            "lazy_tool",
            format!("{name} is in the tool catalog, which this agent cannot run. Do the job on a screen instead."),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_nearest_names_come_by_edit_distance_with_a_prefix_bonus() {
        assert_eq!(distance("kitten", "sitting"), 3);
        assert_eq!(distance("Search", "search"), 0);
        let names = ["screen", "search", "data", "skill", "ask"];
        assert_eq!(nearest("serch", names, 2), ["search"]);
        assert_eq!(nearest("scren", names, 1), ["screen"]);
        assert_eq!(nearest("skills", names, 3), ["skill"]);
        assert!(nearest("zzzzzzzz", names, 3).is_empty());
        // a shared prefix beats a closer distance
        let paths = ["UserAccountList", "UserGroupList", "AccountList"];
        assert_eq!(nearest("UserAcountList", paths, 1), ["UserAccountList"]);
    }

    #[test]
    fn an_unknown_tool_is_routed_by_synonym_or_nearest_name_and_a_lazy_one_to_run_tool() {
        let offered = ["ask", "data", "screen", "search", "skill"];
        let navigate = unknown_tool("navigate", &offered, &[]).to_text();
        assert!(navigate.starts_with("{\"error\":\"unknown_tool\""), "{navigate}");
        assert!(
            navigate.contains("screen with go") && navigate.contains("\"did_you_mean\":[\"screen\"]"),
            "{navigate}"
        );
        let typo = unknown_tool("serach", &offered, &[]).to_text();
        assert!(typo.contains("Call search instead"), "{typo}");
        let none = unknown_tool("frobnicate", &offered, &[]).to_text();
        assert!(none.contains("The tools are: ask, data"), "{none}");
        let lazy = vec!["example.Things.create#Thing".to_string()];
        let direct = unknown_tool("example.Things.create#Thing", &["run_tool", "screen"], &lazy).to_text();
        assert!(
            direct.contains("\"lazy_tool\"") && direct.contains("run_tool with name"),
            "{direct}"
        );
        let without = unknown_tool("example.Things.create#Thing", &offered, &lazy).to_text();
        assert!(without.contains("cannot run"), "{without}");
    }
}
