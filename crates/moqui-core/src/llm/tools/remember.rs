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

//! `remember` (plans/LLM-A2A-Assist.md 7.3 and 7.6): keep a fact about the user for later conversations, only when the
//! user asks; with `about` (a skill, a screen path or a service) it is a lesson for that procedure instead, Proposed and
//! the user's own until an admin approves it ([`crate::llm::memory`]). It is offered where someone can be asked (the
//! drawer, the terminal, chat; not A2A, MCP or a job) and in the standard variant, so the tool list of a conversation
//! never changes: when the user turned memory creation off, permanently or for the conversation, the call answers
//! `memory_off` and nothing is kept. A fact saved now is already in this conversation's history; it reaches the prefix
//! of the next one.
//!
//! Text a tool read (an email, a note, a page) can say "remember …", so the call keeps the fact at once only when the
//! request is plainly the user's: on the drawer or the terminal, with no tool output read since the user's last
//! message ([`crate::llm::memory::needs_confirmation`]). Otherwise it is a confirm card (an approval the user accepts,
//! inline or in the room), and the fact is kept only when they do. Each kept fact is a `memory.saved` event, which the
//! drawer shows as "Remembered: … [Forget]". The write is the user's own record, so the read fence lifts for it.

use serde_json::{json, Value as Json};

use super::format::ToolError;
use super::schema::{object, Property};
use super::{caps, AgentTool, Classification, ToolInput, ToolOutcome, ToolSpec};
use crate::llm::approval::{self, ApprovalRequest, ChangeRow, PreviewMode, Requested, Resumed, TargetKind};
use crate::llm::conf::PromptVariant;
use crate::llm::memory::{self, Anchor};
use crate::llm::run::context::TurnContext;

pub struct RememberTool(ToolSpec);

impl Default for RememberTool {
    fn default() -> Self {
        let mut spec = ToolSpec::new(
            "remember",
            "Keep a fact the user asks you to remember for later conversations. With about (a skill, /apps path or \
             service) it is a lesson.",
            object(&[
                Property::required("fact", "string", ""),
                Property::optional("days", "integer", ""),
                Property::optional("about", "string", ""),
            ]),
            Classification::Read,
        )
        .needs_any(&[caps::APPROVAL_INLINE, caps::QUESTION_INLINE]);
        spec.variants = vec![PromptVariant::Standard];
        // a fact from text a tool read waits for the user's confirmation
        spec.may_pause = true;
        Self(spec)
    }
}

/// What a call asks to keep, checked.
struct Keep {
    fact: String,
    days: Option<i64>,
    /// `about` as the call gave it, and what it names.
    about: Option<String>,
    anchor: Option<Anchor>,
}

impl Keep {
    fn of(args: &Json) -> Result<Self, ToolError> {
        let fact = args.get("fact").and_then(Json::as_str).unwrap_or_default();
        let days = args.get("days").and_then(|days| days.as_i64().or_else(|| days.as_str()?.trim().parse().ok()));
        let about = args.get("about").and_then(Json::as_str).map(str::trim).filter(|about| !about.is_empty());
        let anchor = match about {
            Some(about) => Some(Anchor::parse(about).ok_or_else(|| {
                ToolError::new("about", "about is a skill's name, a screen path (/apps/…) or a service (package.verb#Noun).")
            })?),
            None => None,
        };
        let fact = match &anchor {
            Some(_) => memory::lesson_text(fact),
            None => memory::fact_text(fact),
        }
        .map_err(|problem| ToolError::new("not_kept", problem))?;
        Ok(Self {
            fact,
            days,
            about: about.map(str::to_string),
            anchor,
        })
    }

    fn args(&self) -> Json {
        json!({"fact": self.fact, "days": self.days, "about": self.about})
    }
}

impl AgentTool for RememberTool {
    fn spec(&self) -> &ToolSpec {
        &self.0
    }

    fn call(&self, ctx: &mut TurnContext<'_>, args: Json) -> ToolOutcome {
        let keep = match Keep::of(&args) {
            Ok(keep) => keep,
            Err(error) => return error.outcome(),
        };
        if let Err(error) = gate_open(ctx) {
            return error.outcome();
        }
        if !memory::needs_confirmation(ctx) {
            return save(ctx, &keep);
        }
        let (label, summary) = match &keep.anchor {
            Some(anchor) => ("Lesson", format!("Keep a lesson for {}: {}", anchor.label(), keep.fact)),
            None => ("Remember", format!("Remember: {}", keep.fact)),
        };
        let request = ApprovalRequest {
            target_kind: TargetKind::Tool,
            target_name: if keep.anchor.is_some() { "lesson".to_string() } else { "memory".to_string() },
            classification: Classification::Write,
            args: keep.args(),
            summary,
            changes: vec![ChangeRow {
                field: "fact".to_string(),
                label: label.to_string(),
                current: String::new(),
                proposed: keep.fact.clone(),
            }],
            preview: PreviewMode::None,
            ..ApprovalRequest::default()
        };
        match approval::request(ctx, &request) {
            Requested::Pause(pause) => ToolOutcome::Pause(pause),
            // a policy is no person: a fact from tool text is kept only when the user confirms it
            Requested::Approved(_) | Requested::Refused(_) => ToolError::new(
                "needs_confirmation",
                "This came from text a tool read, so it is kept only when the user confirms it, and nobody can confirm \
                 it here. Nothing was kept.",
            )
            .outcome(),
        }
    }

    fn resume(&self, ctx: &mut TurnContext<'_>, _args: Json, _input: &ToolInput) -> ToolOutcome {
        let view = match approval::for_call(ctx) {
            Ok(Some(view)) => view,
            Ok(None) => return ToolOutcome::Failed("this call has no confirmation to go on with".to_string()),
            Err(error) => return ToolOutcome::Failed(format!("the confirmation could not be read: {error}")),
        };
        match approval::resumed(&view) {
            Ok(Resumed::Approved) => {
                let keep = match Keep::of(&view.args) {
                    Ok(keep) => keep,
                    Err(error) => return error.outcome(),
                };
                let grant = match approval::begin(ctx, &view.approval_id, &view.args) {
                    Ok(grant) => grant,
                    Err(why) => return ToolOutcome::text(why),
                };
                let outcome = save(ctx, &keep);
                let ok = matches!(outcome, ToolOutcome::Done(_));
                approval::finish(ctx, &grant.approval_id, ok, &json!({"ok": ok}));
                outcome
            }
            Ok(Resumed::Waiting(pause)) => ToolOutcome::Pause(pause),
            Ok(Resumed::Done(text) | Resumed::HandedOff(text)) => ToolOutcome::text(text),
            Err(error) => ToolOutcome::Failed(format!("the confirmation could not be read: {error}")),
        }
    }
}

/// The memory gate of the run's user and conversation, open or the `memory_off` error.
fn gate_open(ctx: &TurnContext<'_>) -> Result<(), ToolError> {
    let (Ok(factory), Some(facade)) = (super::apps::factory(), crate::llm::skills::facade()) else {
        return Err(ToolError::new("unavailable", "Memory is not available on this server."));
    };
    let default = crate::llm::facade(&factory).conf().assist.memory_default;
    let conversation_id = ctx.run.map(|run| run.conversation_id.as_str());
    let gate = memory::gate(&facade, &ctx.user.user_id, conversation_id, default)
        .map_err(|error| ToolError::new("read_failed", format!("The memory setting could not be read: {error}")))?;
    if gate.effective() {
        Ok(())
    } else {
        Err(ToolError::new(
            "memory_off",
            "The user turned memory off, so nothing was kept. Tell them; they can turn it on in Assist's memory settings.",
        ))
    }
}

/// Keep the fact (or the lesson) and tell the clients (`memory.saved`, `lesson.proposed`).
fn save(ctx: &mut TurnContext<'_>, keep: &Keep) -> ToolOutcome {
    let Some(facade) = crate::llm::skills::facade() else {
        return ToolError::new("unavailable", "Memory is not available on this server.").outcome();
    };
    let user_id = ctx.user.user_id.clone();
    let conversation_id = ctx.run.map(|run| run.conversation_id.clone());
    if let Some(anchor) = &keep.anchor {
        return match memory::propose_lesson(&facade, &user_id, anchor, &keep.fact, None, conversation_id.as_deref()) {
            Ok(lesson_id) => {
                ctx.events.emit(
                    "lesson.proposed",
                    json!({"lessonId": lesson_id, "text": keep.fact, "anchor": anchor.label()}),
                );
                ToolOutcome::text(format!(
                    "Kept as a lesson for {}: it applies to the user's own work at once, and to others once an admin approves it.",
                    anchor.label()
                ))
            }
            Err(error) => ToolError::new("not_kept", error.message().to_string()).outcome(),
        };
    }
    match memory::add(&facade, &user_id, None, &keep.fact, keep.days, conversation_id.as_deref()) {
        Ok(row) => {
            ctx.events.emit("memory.saved", memory::memory_json(&row));
            ToolOutcome::text(match keep.days {
                Some(days) => format!("Remembered for {days} days. The user can see and change it in Assist's memory."),
                None => "Remembered. The user can see and change it in Assist's memory.".to_string(),
            })
        }
        Err(error) => ToolError::new("not_kept", error.message().to_string()).outcome(),
    }
}

crate::submit_tool!(RememberTool);
