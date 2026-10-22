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

//! Transcripts and replays. A transcript is JSON lines, `<run>/<spec>/<task>-<n>.jsonl`, one object a line by `type`:
//!
//! - `header`: the spec, the task, the run number, the profile, the model's settings (`LlmModel` fields), the seed.
//! - `user`: a turn's message. `model`: one model reply (`text`, `toolCalls` of `{id, name, arguments}` or `raw`,
//!   `finish`, `usage`). `tool`: a call and its full result. `question`: a question the agent asked and the scripted
//!   answer. `approval`: a held write, what became of it and how it was decided. `answer`: the turn's answer and run state.
//! - `result`: passed or not, the failures and the metrics.
//!
//! A transcript is also a recording: [`Recording::read`] takes its header's model and its `model` and `tool` lines, and
//! a replay ([`Recording::script`]) queues the replies on the fake endpoint, so the tools run live on what the model
//! said then. The `tool` lines are compared with what the tools answer now ([`drift`]), dates and long ids left out.

use std::io::Write;
use std::path::{Path, PathBuf};

use moqui_lib::error::Error;
use serde_json::{json, Value as Json};

use super::observe::{ModelReply, TurnSeen};
use crate::llm::provider::fake::{self, Reply, Step};

/// The text the fake answers once a replay's recorded replies are used up.
pub const REPLAY_END: &str = "[the recording has no more model replies]";

/// A transcript being written.
pub struct Transcript {
    path: PathBuf,
    out: Option<std::io::BufWriter<std::fs::File>>,
}

impl Transcript {
    /// Start `path`, its directories made. A file that cannot be written is logged once, and the run goes on.
    pub fn create(path: PathBuf) -> Self {
        let out = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::File::create(&path))
            .map(std::io::BufWriter::new);
        let out = match out {
            Ok(out) => Some(out),
            Err(error) => {
                tracing::error!("the agent transcript {} could not be written: {error}", path.display());
                None
            }
        };
        Self { path, out }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn line(&mut self, value: &Json) {
        if let Some(out) = &mut self.out {
            if let Err(error) = writeln!(out, "{value}") {
                tracing::error!("the agent transcript {} could not be written: {error}", self.path.display());
                self.out = None;
            }
        }
    }

    /// The lines of a turn: the user's message, each model reply followed by its calls and their results, the held
    /// writes, the questions and the answer.
    pub fn turn(&mut self, index: usize, turn: &TurnSeen) {
        self.line(&json!({"type": "user", "turn": index, "text": turn.say}));
        let mut calls = turn.calls.iter();
        for (step, reply) in turn.replies.iter().enumerate() {
            let mut line = serde_json::to_value(reply).unwrap_or(Json::Null);
            line["type"] = json!("model");
            line["turn"] = json!(index);
            line["step"] = json!(step);
            self.line(&line);
            // a step's calls are stored in the order of its reply's calls
            for call in calls.by_ref().take(reply.tool_calls.len()) {
                self.call(index, step, call);
            }
        }
        for call in calls {
            self.call(index, turn.replies.len(), call);
        }
        for held in &turn.held {
            self.line(&json!({"type": "approval", "turn": index, "transition": held.transition, "text": held.text,
                "status": held.status, "channel": held.channel}));
        }
        for question in &turn.questions {
            self.line(&json!({"type": "question", "turn": index, "question": question}));
        }
        self.line(&json!({"type": "answer", "turn": index, "state": turn.state, "text": turn.answer,
            "invented": turn.invented, "error": turn.error, "screen": turn.screen, "wallMs": turn.wall_ms}));
    }

    fn call(&mut self, turn: usize, step: usize, call: &super::observe::CallSeen) {
        self.line(&json!({"type": "tool", "turn": turn, "step": step, "name": call.tool, "arguments": call.arguments,
            "status": call.status, "error": call.error, "repaired": call.repaired, "result": call.result}));
    }

    pub fn finish(mut self) {
        if let Some(mut out) = self.out.take() {
            if let Err(error) = out.flush() {
                tracing::error!("the agent transcript {} could not be written: {error}", self.path.display());
            }
        }
    }
}

/// What a replay plays: the recorded model's settings, its replies and the tool results then.
#[derive(Clone, Debug, Default)]
pub struct Recording {
    pub model: Json,
    pub replies: Vec<ModelReply>,
    /// `(tool, result)` in order.
    pub results: Vec<(String, String)>,
}

impl Recording {
    /// Read a transcript (or a hand-written recording in the same form).
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or a line is not JSON.
    pub fn read(path: &Path) -> Result<Self, Error> {
        let text = std::fs::read_to_string(path)
            .map_err(|err| Error::new(format!("could not read the recording {}", path.display())).with_source(err))?;
        Self::parse(&text).map_err(|err| Error::new(format!("{}: {err}", path.display())))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut recording = Recording::default();
        for (number, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let value: Json =
                serde_json::from_str(line).map_err(|err| format!("line {} is not JSON: {err}", number + 1))?;
            match value.get("type").and_then(Json::as_str) {
                Some("header") => recording.model = value.get("model").cloned().unwrap_or(Json::Null),
                Some("model") => recording.replies.push(
                    serde_json::from_value(value)
                        .map_err(|err| format!("line {} is not a model reply: {err}", number + 1))?,
                ),
                Some("tool") => recording.results.push((
                    value.get("name").and_then(Json::as_str).unwrap_or_default().to_string(),
                    value.get("result").and_then(Json::as_str).unwrap_or_default().to_string(),
                )),
                _ => {}
            }
        }
        if recording.replies.is_empty() {
            return Err("it has no model replies".to_string());
        }
        Ok(recording)
    }

    /// Queue the replies on the fake endpoint, then a few that say the recording ended (a run that asks for more
    /// gets those, not the fake's own answers).
    pub fn script(&self, endpoint_id: &str) {
        fake::clear(endpoint_id);
        let mut replies: Vec<Reply> = self.replies.iter().map(reply_of).collect();
        replies.extend((0..3).map(|_| Reply::text(REPLAY_END)));
        fake::script(endpoint_id, replies);
    }
}

/// The fake's reply for a recorded one: its text, its calls with their arguments in one piece, and `tool_calls` or
/// `stop`; a JSON tool mode reply's own text (`raw`) and `stop`.
pub fn reply_of(reply: &ModelReply) -> Reply {
    // a JSON tool mode reply is played as the text the model wrote
    if let Some(raw) = &reply.raw {
        return Reply::steps(vec![Step::Text(raw.clone()), Step::Finish("stop".to_string())]);
    }
    let mut steps = Vec::new();
    if !reply.text.is_empty() {
        steps.push(Step::Text(reply.text.clone()));
    }
    for call in &reply.tool_calls {
        let arguments = match &call.raw {
            Some(raw) => raw.clone(),
            None if call.arguments.is_null() => "{}".to_string(),
            None => call.arguments.to_string(),
        };
        steps.push(Step::Tool {
            id: call.id.clone(),
            name: call.name.clone(),
            arguments,
            pieces: 1,
        });
    }
    // the reply as it was kept: a length or filter finish was already settled by the engine then
    let finish = if reply.tool_calls.is_empty() { "stop" } else { "tool_calls" };
    steps.push(Step::Finish(finish.to_string()));
    Reply::steps(steps)
}

/// Where the tool results of a replay differ from the recording's, dates and long numbers left out: one line each.
pub fn drift(recorded: &[(String, String)], now: &[(String, String)]) -> Vec<String> {
    if recorded.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    for (index, (tool, result)) in now.iter().enumerate() {
        match recorded.get(index) {
            Some((was_tool, was)) if was_tool == tool => {
                if normalize(was) != normalize(result) {
                    let (old, new) = first_difference(&normalize(was), &normalize(result));
                    lines.push(format!("result {} ({tool}) changed: {old:?} is now {new:?}", index + 1));
                }
            }
            Some((was_tool, _)) => lines.push(format!("call {} is {tool}, the recording's was {was_tool}", index + 1)),
            None => lines.push(format!("call {} ({tool}) is not in the recording", index + 1)),
        }
    }
    if recorded.len() > now.len() {
        lines.push(format!("the recording has {} tool calls, the replay made {}", recorded.len(), now.len()));
    }
    lines
}

/// A result with what changes from run to run left out: dates (`2026-10-21`, with a time), clock times (`22:49`,
/// and the weekday before one, as the terminal's status line shows them) and numbers of five or more digits
/// (sequenced ids, timestamps), each replaced by a placeholder.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut digits = String::new();
    let flush = |digits: &mut String, out: &mut String| {
        if digits.len() >= 5 {
            out.push('#');
        } else {
            out.push_str(digits);
        }
        digits.clear();
    };
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        let after_digit = index > 0 && chars[index - 1].is_ascii_digit();
        if !after_digit && digits.is_empty() {
            if let Some(length) = date_at(&chars[index..]) {
                out.push_str("<date>");
                index += length;
                continue;
            }
            if let Some(length) = time_at(&chars[index..]) {
                out.push_str("<time>");
                index += length;
                continue;
            }
        }
        let ch = chars[index];
        if ch.is_ascii_digit() {
            digits.push(ch);
        } else {
            flush(&mut digits, &mut out);
            out.push(ch);
        }
        index += 1;
    }
    flush(&mut digits, &mut out);
    for day in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"] {
        out = out.replace(&format!("{day} <time>"), "<day> <time>");
    }
    out
}

/// The length of a clock time at the start of `chars`: `H:MM` or `HH:MM`, not followed by another digit.
fn time_at(chars: &[char]) -> Option<usize> {
    let digit = |at: usize| chars.get(at).is_some_and(char::is_ascii_digit);
    let hour = if digit(0) && digit(1) { 2 } else if digit(0) { 1 } else { return None };
    if chars.get(hour) == Some(&':') && digit(hour + 1) && digit(hour + 2) && !digit(hour + 3) {
        Some(hour + 3)
    } else {
        None
    }
}

/// The length of a date at the start of `chars`: `YYYY-MM-DD`, with a time `HH:MM` or `HH:MM:SS` after a space or `T`.
fn date_at(chars: &[char]) -> Option<usize> {
    let digit = |at: usize| chars.get(at).is_some_and(char::is_ascii_digit);
    let is = |at: usize, ch: char| chars.get(at) == Some(&ch);
    if !((0..4).all(digit) && is(4, '-') && digit(5) && digit(6) && is(7, '-') && digit(8) && digit(9)) {
        return None;
    }
    let mut length = 10;
    if (is(10, ' ') || is(10, 'T')) && digit(11) && digit(12) && is(13, ':') && digit(14) && digit(15) {
        length = 16;
        if is(16, ':') && digit(17) && digit(18) {
            length = 19;
        }
    }
    Some(length)
}

/// The first line where two texts differ, cut short.
fn first_difference(old: &str, new: &str) -> (String, String) {
    let cut = |text: &str| text.chars().take(120).collect::<String>();
    let mut old_lines = old.lines();
    let mut new_lines = new.lines();
    loop {
        match (old_lines.next(), new_lines.next()) {
            (Some(a), Some(b)) if a == b => continue,
            (a, b) => return (cut(a.unwrap_or("")), cut(b.unwrap_or(""))),
        }
    }
}
