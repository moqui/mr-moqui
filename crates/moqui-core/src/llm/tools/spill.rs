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

//! Spill (plans/LLM-A2A-Assist.md 4.6 and 7.7): a tool result over the model's result cap is stored whole at
//! `dbresource://agent/<runId>/<toolCallId>` (the call's `spillLocation`), and the model gets a preview with the first
//! lines (a table's header with every column name among them), or the JSON shrunk by whole values, and the line that
//! says how to read on: `artifact A12 (312 rows, 48,210 chars): data artifact=A12 offset=40, or grep=text`. The run
//! writes it after the tool returns, outside the read fence. `data` reads an artifact back by lines
//! ([`read_lines`]), only one of the same conversation.

use moqui_lib::error::Error;
use moqui_lib::object::Value;
use serde_json::Value as Json;

use super::format;
use crate::entity::EntityFacade;

/// Where the artifacts of the runs are kept.
pub const ROOT: &str = "dbresource://agent";

/// The location of a call's artifact.
pub fn location(run_id: &str, tool_call_id: &str) -> String {
    format!("{ROOT}/{run_id}/{tool_call_id}")
}

/// The name the model reads for the artifact of a call.
pub fn artifact_id(tool_call_id: &str) -> String {
    format!("A{tool_call_id}")
}

/// The tool call of an artifact name (`A12` or `12`).
pub fn tool_call_of(artifact: &str) -> &str {
    let artifact = artifact.trim();
    artifact
        .strip_prefix('A')
        .or_else(|| artifact.strip_prefix('a'))
        .unwrap_or(artifact)
}

/// `1234567` as `1,234,567`.
pub fn grouped(number: usize) -> String {
    let digits = number.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// The preview of `text` within `cap` characters, for the artifact `artifact`.
pub fn preview(text: &str, cap: usize, artifact: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let chars = text.chars().count();
    let footer_of = |offset: usize| {
        format!(
            "artifact {artifact} ({} rows, {} chars): data artifact={artifact} offset={offset}, or grep=text",
            grouped(lines.len()),
            grouped(chars)
        )
    };
    let budget = cap.saturating_sub(footer_of(lines.len()).chars().count() + 1).max(200);
    if let Ok(value @ (Json::Object(_) | Json::Array(_))) = serde_json::from_str::<Json>(text) {
        return format!("{}\n{}", format::shrink(&value, budget), footer_of(0));
    }
    let mut kept: Vec<String> = Vec::new();
    let mut used = 0;
    for line in &lines {
        let size = line.chars().count() + 1;
        if used + size > budget {
            if kept.is_empty() {
                kept.push(format::cut(line, budget.saturating_sub(20)));
            }
            break;
        }
        used += size;
        kept.push((*line).to_string());
    }
    let shown = kept.len();
    format!("{}\n{}", kept.join("\n"), footer_of(shown))
}

/// A result to be stored as a call's artifact: where, and the whole text. The run writes it with the result, in the
/// transaction that checks its lease ([`Artifact::write`]), and removes it when the result is not stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artifact {
    pub tool_call_id: String,
    pub location: String,
    pub text: String,
}

impl Artifact {
    pub fn new(run_id: &str, tool_call_id: &str, text: &str) -> Self {
        Self {
            tool_call_id: tool_call_id.to_string(),
            location: location(run_id, tool_call_id),
            text: text.to_string(),
        }
    }

    /// Write the artifact and the call's `spillLocation`; the caller holds the run's lease in its transaction.
    ///
    /// # Errors
    ///
    /// Returns an error when the artifact or the row cannot be written.
    pub fn write(&self, facade: &EntityFacade) -> Result<(), Error> {
        facade.write_db_resource(&self.location, self.text.as_bytes())?;
        facade
            .find("moqui.agent.AgentToolCall")?
            .eq("toolCallId", Value::string(&self.tool_call_id))
            .disable_authz()
            .update_all([("spillLocation".to_string(), Value::string(&self.location))])?;
        Ok(())
    }

    /// Remove an artifact whose result was not stored (a content blob of a large one stays for the blob sweep). A
    /// failure is logged.
    pub fn remove(&self, facade: &EntityFacade) {
        if let Err(error) = facade.delete_db_resource(&self.location) {
            tracing::error!(
                "the artifact {} of a result not stored could not be removed: {error}",
                self.location
            );
        }
    }
}

/// The text of an artifact of the conversation `conversation_id`, `None` when there is no such artifact in it.
///
/// # Errors
///
/// Returns an error when the rows cannot be read.
pub fn read(facade: &EntityFacade, conversation_id: &str, artifact: &str) -> Result<Option<String>, Error> {
    let tool_call_id = tool_call_of(artifact);
    if tool_call_id.is_empty() {
        return Ok(None);
    }
    let Some(call) = facade
        .find("moqui.agent.AgentToolCall")?
        .eq("toolCallId", Value::string(tool_call_id))
        .disable_authz()
        .one()?
    else {
        return Ok(None);
    };
    let (Some(run_id), Some(location)) = (
        crate::llm::text_of(&call, "runId"),
        crate::llm::text_of(&call, "spillLocation"),
    ) else {
        return Ok(None);
    };
    let same_conversation = facade
        .find("moqui.agent.AgentRun")?
        .eq("runId", Value::string(&run_id))
        .disable_authz()
        .one()?
        .and_then(|run| crate::llm::text_of(&run, "conversationId"))
        .is_some_and(|id| id == conversation_id);
    if !same_conversation || !location.starts_with(ROOT) {
        return Ok(None);
    }
    Ok(facade
        .read_db_resource_bytes(&location)?
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
}

/// Lines `offset..offset+limit` of an artifact, or the lines that hold `grep` (case ignored) from the `offset`th match,
/// numbered from 0; a table's header line comes first when the lines start past it.
pub fn read_lines(text: &str, artifact: &str, offset: usize, limit: usize, grep: Option<&str>) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let header = lines
        .iter()
        .take(3)
        .position(|line| line.contains(" | "))
        .map(|at| lines[at]);
    let mut out = Vec::new();
    let (picked, total, what): (Vec<(usize, &str)>, usize, String) = match grep.map(str::trim).filter(|g| !g.is_empty())
    {
        Some(grep) => {
            let wanted = grep.to_lowercase();
            let matches: Vec<(usize, &str)> = lines
                .iter()
                .enumerate()
                .filter(|(_, line)| line.to_lowercase().contains(&wanted))
                .map(|(at, line)| (at, *line))
                .collect();
            let total = matches.len();
            (
                matches.into_iter().skip(offset).take(limit).collect(),
                total,
                format!("lines with \"{grep}\""),
            )
        }
        None => (
            lines
                .iter()
                .enumerate()
                .skip(offset)
                .take(limit)
                .map(|(at, line)| (at, *line))
                .collect(),
            lines.len(),
            "lines".to_string(),
        ),
    };
    out.push(format!(
        "artifact {artifact} {what} (showing {} of {total}, from {offset})",
        picked.len()
    ));
    if let Some(header) = header {
        if picked.first().is_some_and(|(at, line)| *at > 0 && *line != header) {
            out.push(header.to_string());
        }
    }
    for (at, line) in &picked {
        if grep.is_some() {
            out.push(format!("{at}: {line}"));
        } else {
            out.push((*line).to_string());
        }
    }
    if offset + picked.len() < total {
        let grep = grep.map(|grep| format!(" grep={grep}")).unwrap_or_default();
        out.push(format!(
            "more: data artifact={artifact} offset={}{grep}",
            offset + picked.len()
        ));
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn big_table() -> String {
        let mut lines = vec!["enumId | description | enumTypeId".to_string()];
        for n in 0..300 {
            lines.push(format!("E{n} | Enumeration number {n} | TestType"));
        }
        lines.join("\n")
    }

    #[test]
    fn a_preview_keeps_the_header_and_says_how_to_read_on() {
        let text = big_table();
        let preview = preview(&text, 1000, "A7");
        assert!(preview.chars().count() <= 1000, "{}", preview.chars().count());
        assert!(
            preview.starts_with("enumId | description | enumTypeId\nE0 |"),
            "{preview}"
        );
        let footer = preview.lines().last().unwrap();
        assert!(
            footer.starts_with("artifact A7 (301 rows, ") && footer.contains("data artifact=A7 offset="),
            "{footer}"
        );
        let json = serde_json::json!({"error": "x", "rows": (0..500).collect::<Vec<_>>()}).to_string();
        let shrunk = super::preview(&json, 600, "A8");
        assert!(
            shrunk.contains("\"error\":\"x\"") && shrunk.contains("more)"),
            "{shrunk}"
        );
        assert_eq!(grouped(48210), "48,210");
        assert_eq!(tool_call_of("A12"), "12");
    }

    #[test]
    fn lines_are_read_by_offset_and_grep_with_the_header_first() {
        let text = big_table();
        let page = read_lines(&text, "A7", 100, 3, None);
        let lines: Vec<&str> = page.lines().collect();
        assert_eq!(lines[0], "artifact A7 lines (showing 3 of 301, from 100)");
        assert_eq!(lines[1], "enumId | description | enumTypeId");
        assert_eq!(lines[2], "E99 | Enumeration number 99 | TestType");
        assert_eq!(lines.last().unwrap(), &"more: data artifact=A7 offset=103");
        let found = read_lines(&text, "A7", 0, 10, Some("number 25 |"));
        assert!(found.contains("(showing 1 of 1, from 0)"), "{found}");
        assert!(found.contains("26: E25 | Enumeration number 25"), "{found}");
        assert!(!found.contains("more:"), "{found}");
    }
}
