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

//! `@name` and `@name#section` in a user's message (plan 7.5): each that names a skill of the index appends a
//! synthetic `skill` call and its result right after the message, as if the model had asked, so the frozen prefix
//! and the cache stay as they are. At most [`MAX_MENTIONS`] a message; an `@` that names no skill (an agent, a person,
//! an address) is left as text.

use moqui_lib::error::Error;
use moqui_lib::object::Value;
use serde_json::json;

use super::{uses, SkillIndex};
use crate::entity::EntityFacade;
use crate::llm::prompt::Surface;
use crate::llm::run::step::{append_message, Part};
use crate::llm::run::{now_millis, stamp, TOOL_CALL};
use crate::llm::text_of;
use crate::llm::tools::skill as skill_tool;

/// Skills one message loads by mention.
pub const MAX_MENTIONS: usize = 3;

/// The `name` or `name#section` of each `@` in `text` that names a skill of `index`, in order, once each.
pub fn mentioned(index: &SkillIndex, text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut at = 0;
    while at < chars.len() {
        let starts = chars[at] == '@' && (at == 0 || chars[at - 1].is_whitespace() || chars[at - 1] == '(');
        if !starts {
            at += 1;
            continue;
        }
        let mut end = at + 1;
        while end < chars.len() && (chars[end].is_ascii_alphanumeric() || matches!(chars[end], '-' | '#' | '_')) {
            end += 1;
        }
        let token: String = chars[at + 1..end].iter().collect();
        let token = token.trim_end_matches(['-', '#']).to_lowercase();
        let name = token.split('#').next().unwrap_or_default();
        if !name.is_empty() && index.get(name).is_some() && !out.contains(&token) {
            out.push(token);
            if out.len() == MAX_MENTIONS {
                break;
            }
        }
        at = end.max(at + 1);
    }
    out
}

/// After the user message just appended, the synthetic calls of the skills `text` mentions: an assistant message with
/// one `skill` call each (done, step 0) and a tool message with each result; each load is recorded. Inside the
/// caller's transaction. Nothing when no skill is mentioned.
///
/// # Errors
///
/// Returns an error when a row cannot be written.
#[allow(clippy::too_many_arguments)]
pub(crate) fn append(
    facade: &EntityFacade,
    index: &SkillIndex,
    conversation_id: &str,
    run_id: &str,
    user_id: &str,
    surface: Surface,
    text: &str,
) -> Result<usize, Error> {
    let wanted = mentioned(index, text);
    if wanted.is_empty() {
        return Ok(0);
    }
    let now = now_millis();
    let mut calls: Vec<(String, String, String, String, bool)> = Vec::new();
    let mut parts = Vec::new();
    for (number, name) in wanted.iter().enumerate() {
        let arguments = json!({"name": name});
        let (result, is_error) = match skill_tool::load(index, name) {
            Ok(loaded) => {
                uses::record(facade, Some(run_id), Some(conversation_id), &loaded.skill, &loaded.section);
                (loaded.text, false)
            }
            Err(error) => (error.to_text(), true),
        };
        let mut row = facade.make_value(TOOL_CALL)?;
        row.set_sequenced_id_primary(facade)?;
        let tool_call_id = text_of(&row, "toolCallId").unwrap_or_default();
        let provider_id = format!("mention-{tool_call_id}-{}", number + 1);
        row.set("providerCallId", Value::string(&provider_id));
        row.set("runId", Value::string(run_id));
        row.set("stepSeq", Value::I64(0));
        row.set("toolName", Value::string("skill"));
        row.set("classificationEnumId", Value::string("AgclRead"));
        row.set("argumentsJson", Value::string(arguments.to_string()));
        row.set("argumentsHash", Value::string(moqui_lib::json::canonical_sha256(&arguments)));
        row.set("statusId", Value::string(if is_error { "AgtcFailed" } else { "AgtcDone" }));
        row.set("surfaceEnumId", Value::string(surface.enum_id()));
        row.set("userId", Value::string(user_id));
        row.set("startedDate", stamp(now));
        row.set("endedDate", stamp(now));
        row.set("resultSummary", Value::string(result.chars().take(4000).collect::<String>()));
        row.create(facade)?;
        parts.push(Part {
            kind: "AgpkToolCall",
            data: Some(json!({"id": provider_id, "name": "skill", "arguments": arguments})),
            tool_call_id: Some(tool_call_id.clone()),
            ..Part::default()
        });
        calls.push((tool_call_id, provider_id, name.clone(), result, is_error));
    }
    append_message(
        facade,
        conversation_id,
        Some(run_id),
        "assistant",
        None,
        None,
        &parts,
        Some(&json!({"mention": true})),
        None,
    )?;
    for (tool_call_id, provider_id, _name, result, is_error) in &calls {
        append_message(
            facade,
            conversation_id,
            Some(run_id),
            "tool",
            None,
            None,
            &[Part {
                kind: "AgpkToolResult",
                text: Some(result.clone()),
                tool_call_id: Some(tool_call_id.clone()),
                metadata: Some(json!({"providerCallId": provider_id, "name": "skill", "isError": is_error})),
                ..Part::default()
            }],
            None,
            None,
        )?;
    }
    Ok(calls.len())
}
