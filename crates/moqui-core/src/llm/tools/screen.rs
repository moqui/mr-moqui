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

//! The `screen` tool (plans/LLM-A2A-Assist.md 7.3, 7.4, 7.7): the apps as the user, through the run's screen session
//! ([`crate::llm::screen`]). One call runs its steps in order, `go`, `set`, `find`, `do` and `key`, and a failed step
//! stops the rest; with none it looks again. The answer is the screen as text, `--- ok` or `--- error <why>` last, with
//! a `did you mean:` clause for a name that finds no element.
//!
//! Every render of the session is fenced and its writes are held. A press that writes becomes an approval
//! ([`crate::llm::approval`], plan 4.8): the precheck (the agent's policy lets it write, its role group permits the
//! transition, the user may run it, since the gate holds a write before the transition's own check), the snapshot
//! (the changed fields, current against proposed, titled from the tui labels), the `AgentApproval` row bound to the
//! held request's canonical hash, and a pause. The call resumes with the decision: approved, the screen is rendered
//! again and compared with the originals (a change meanwhile is a conflict), and the exact request is sent again
//! under the approval's grant ([`ScreenSessions::execute_approved`]); rejected, the feedback is the result; edited in
//! the screen, the user's own save is. When the user's page follows (the drawer's `screen.follow`), the session's
//! follow events are stored events of the run. The session's state is kept on the conversation after each call, so a
//! later run or another node opens where this one stood.

use std::sync::Arc;

use serde_json::{json, Value as Json};

use super::schema::{object, Property};
use super::{caps, AgentTool, Classification, ToolInput, ToolOutcome, ToolResult, ToolSpec};
use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::llm::approval::{self, ApprovalRequest, ChangeRow, HandoffTarget, PreviewMode, Requested, Resumed, TargetKind};
use crate::llm::assist::FollowStamp;
use crate::llm::run::context::{ScreenHandle, TurnContext};
use crate::llm::screen::{
    self, ApprovalGrant, OpenSpec, PendingWrite, ScreenAnswer, ScreenCommand, ScreenError, ScreenSessions,
    ScreenState, StateWriter,
};

const NAME: &str = "screen";

pub struct ScreenTool(ToolSpec);

impl Default for ScreenTool {
    fn default() -> Self {
        let mut spec = ToolSpec::new(
            NAME,
            "Use the apps as the user: open a screen, fill fields, press buttons, links and rows. Steps run in order go, \
             set, find, do; with none it looks again. The answer is the screen as text, the focused element in [[ ]], \
             then --- ok or --- error and why. A press that \
             writes waits for the user's approval. To filter a list: do Find, set its find fields, do Find; a dialog's \
             fields are set after do on its button.",
            object(&[
                Property::optional("go", "string", "A screen path, with ?query"),
                Property::optional("set", "object", "{element: value}; an element is its label, id or #n"),
                Property::optional("find", "string", "Text to move to"),
                Property::optional("do", "string", "The element to press: a button, link, list row or menu"),
                Property::optional("key", "string", "Keys (Enter, F10) when no element fits"),
                Property::optional("elements", "boolean", "Add the list of elements"),
                Property::optional("rows", "integer", "Screen rows, default 30, at most 100"),
            ]),
            Classification::Read,
        )
        .needs(&[caps::SCREEN]);
        // A held write becomes an approval the run waits for, so it never runs beside other reads.
        spec.may_pause = true;
        Self(spec)
    }
}

impl AgentTool for ScreenTool {
    fn spec(&self) -> &ToolSpec {
        &self.0
    }

    fn call(&self, ctx: &mut TurnContext<'_>, args: Json) -> ToolOutcome {
        let Some(sessions) = screen::sessions() else {
            return ToolOutcome::Failed("screens are not available on this server".to_string());
        };
        let Some(factory) = ExecutionContext::with_current(|ec| Arc::clone(ec.factory())) else {
            return ToolOutcome::Failed("the screen tool runs only in a run's context".to_string());
        };
        let mut command = match command_of(&args) {
            Ok(command) => command,
            Err(problem) => return ToolOutcome::Failed(problem),
        };
        command.agent_id.clone_from(&ctx.agent.agent_id);
        command.run_id = ctx.run.map(|run| run.run_id.clone());
        // a go that is not a screen the user may open runs nothing: the screen as it is, and why
        let refused = command
            .go
            .as_deref()
            .and_then(|path| screen::check_screen_path(&factory, path).err());
        if refused.is_some() {
            command = ScreenCommand {
                agent_id: command.agent_id,
                run_id: command.run_id,
                rows: command.rows,
                ..ScreenCommand::default()
            };
        }
        let follow = ctx.caps.has(caps::SCREEN_FOLLOW);
        // the follow owner and its page version as the command starts: a page the user moved on from since drops what
        // this command makes
        let stamp = follow.then(|| follow_stamp_of(&factory, ctx)).flatten();
        let Some(handle) = ctx.screen.as_deref_mut() else {
            return ToolOutcome::Failed("this run has no screen session".to_string());
        };
        let mut answer = match run_command(
            &factory,
            sessions.as_ref(),
            handle,
            ctx.user,
            ctx.agent,
            follow,
            &command,
        ) {
            Ok(answer) => answer,
            Err(error) => return ToolOutcome::Failed(error.to_string()),
        };
        if let Some(problem) = refused {
            answer.ok = false;
            answer.text = replace_outcome(&answer.text, &format!("--- error {problem}"));
        }
        let conversation_id = handle.conversation_id.clone();
        keep_state(ctx, &factory, &conversation_id, &answer.state);
        // the lessons of the screen it went to (plan 7.5): the approved ones and the user's own, wrapped as untrusted
        if command.go.is_some() && answer.held.is_none() {
            add_lessons(ctx, &mut answer.text, &answer.path);
        }
        if follow {
            emit_follow(ctx, &answer.follow, stamp.as_ref());
        }
        match answer.held.take() {
            Some(write) => held_write(ctx, &factory, &answer, write),
            None => ToolOutcome::Done(ToolResult {
                text: answer.text.clone(),
                data: Some(json!({"ok": answer.ok, "path": answer.path, "parameters": answer.parameters})),
            }),
        }
    }

    /// The decision came: an approved write runs, anything else is what the model reads.
    fn resume(&self, ctx: &mut TurnContext<'_>, _args: Json, _input: &ToolInput) -> ToolOutcome {
        let view = match approval::for_call(ctx) {
            Ok(Some(view)) => view,
            Ok(None) => return ToolOutcome::Failed("this call has no approval to go on with".to_string()),
            Err(error) => return ToolOutcome::Failed(format!("the approval could not be read: {error}")),
        };
        let Some(factory) = ExecutionContext::with_current(|ec| Arc::clone(ec.factory())) else {
            return ToolOutcome::Failed("the screen tool runs only in a run's context".to_string());
        };
        match approval::resumed(&view) {
            Ok(Resumed::Approved) => execute(ctx, &factory, &view),
            Ok(Resumed::Waiting(pause)) => ToolOutcome::Pause(pause),
            Ok(Resumed::HandedOff(text)) => {
                // the user's own save changed the screen the session shows: it reads it again
                if let Ok(write) = serde_json::from_value::<PendingWrite>(view.args.clone()) {
                    refresh(ctx, &factory, &write);
                }
                ToolOutcome::text(text)
            }
            Ok(Resumed::Done(text)) => ToolOutcome::text(text),
            Err(error) => ToolOutcome::Failed(format!("the approval could not be read: {error}")),
        }
    }
}

/// Keep the session's state for the conversation: a step writes it only while its node holds the run's lease, a
/// call with no run only for its own user. A state with no path (a write run with no session) is not kept.
fn keep_state(ctx: &TurnContext<'_>, factory: &Arc<ExecutionContextFactory>, conversation_id: &str, state: &ScreenState) {
    if state.path.is_empty() {
        return;
    }
    let Some(facade) = factory.entity() else {
        return;
    };
    let writer = match ctx.run {
        Some(run) => StateWriter::Run {
            run_id: &run.run_id,
            node: ctx.events.node(),
        },
        None => StateWriter::Owner(&ctx.user.user_id),
    };
    if let Err(error) = screen::store_state(&facade, conversation_id, writer, state) {
        tracing::error!("the screen state of conversation {conversation_id} was not kept: {error}");
    }
}

/// The conversation's live session on this node, if it has one.
fn session_of(ctx: &TurnContext<'_>, sessions: &dyn ScreenSessions) -> String {
    match ctx.screen.as_deref() {
        Some(handle) if !handle.session_id.is_empty() => handle.session_id.clone(),
        Some(handle) => sessions
            .find(&ctx.user.user_id, &handle.conversation_id)
            .unwrap_or_default(),
        None => String::new(),
    }
}

/// The text of a held write that will not become an approval: the screen as it is, and why, with nothing saved.
fn refused(answer: &ScreenAnswer, why: &str) -> ToolOutcome {
    ToolOutcome::Done(ToolResult {
        text: replace_outcome(&answer.text, &format!("--- error {why}")),
        data: Some(json!({"ok": false, "path": answer.path, "parameters": answer.parameters, "approval": "refused"})),
    })
}

/// A write the gate held: its precheck, then its approval. A write that would be refused is an error the model can
/// act on, never a card the user cannot approve.
fn held_write(
    ctx: &mut TurnContext<'_>,
    factory: &Arc<ExecutionContextFactory>,
    answer: &ScreenAnswer,
    write: PendingWrite,
) -> ToolOutcome {
    let transition = write.transition().to_string();
    let location = write.held["screenLocation"].as_str().unwrap_or_default().to_string();
    let path = write.url.split('?').next().unwrap_or(&write.url).to_string();
    // the agent's role group bounds what it may do, the transition included
    let groups = ctx.agent.role_groups();
    if !groups.is_empty() {
        let action = write.held["authzAction"].as_str().unwrap_or("all").to_string();
        let name = format!("{location}/{transition}");
        match crate::user::group_permits(&groups, "AT_XML_SCREEN_TRANS", &name, &action) {
            Ok(true) => {}
            Ok(false) => {
                return refused(
                    answer,
                    &format!("this agent's role does not permit {transition}, so nothing was saved"),
                )
            }
            Err(error) => {
                tracing::error!("whether the role group permits {name} could not be read: {error}");
                return refused(answer, &format!("{transition} could not be checked, so nothing was saved"));
            }
        }
    }
    // the gate held the write before the transition's own permission check: the user's permission is checked here
    match crate::screen::path_permitted(factory, &screen::root_screen(factory), &path) {
        Ok(true) => {}
        Ok(false) => {
            return refused(
                answer,
                &format!(
                    "the user is not allowed to run {transition}; that is the answer about permission, and nothing                      was saved"
                ),
            )
        }
        Err(error) => {
            return refused(answer, &format!("{transition} could not be checked ({error}), so nothing was saved"))
        }
    }
    let request = approval_request(answer, &write);
    match approval::request(ctx, &request) {
        Requested::Pause(pause) => ToolOutcome::Pause(pause),
        Requested::Refused(why) => refused(answer, &why),
        Requested::Approved(grant) => {
            // a background agent's policy approved it: it runs now
            let view = factory
                .entity()
                .and_then(|facade| crate::llm::run::quietly(|| approval::load(&facade, &grant.approval_id)).ok())
                .flatten();
            match view {
                Some(view) => execute(ctx, factory, &view),
                None => refused(answer, "the approval could not be read, so nothing was saved"),
            }
        }
    }
}

/// The approval a held write asks for: the held request as its arguments, the changed fields as the snapshot, the
/// screen and the form for an Edit in screen.
fn approval_request(answer: &ScreenAnswer, write: &PendingWrite) -> ApprovalRequest {
    let transition = write.transition().to_string();
    let location = write.held["screenLocation"].as_str().unwrap_or_default().to_string();
    let changes: Vec<ChangeRow> = write
        .changes
        .iter()
        .map(|change| ChangeRow {
            field: change.field.clone(),
            label: change.label.clone(),
            current: change.original.clone(),
            proposed: change.proposed.clone(),
        })
        .collect();
    let screen_title = answer.title.rsplit(" ▸ ").next().unwrap_or_default().trim().to_string();
    let summary = if screen_title.is_empty() {
        transition.clone()
    } else {
        format!("{transition} on {screen_title}")
    };
    let extra_path: Vec<String> = write.held["extraPath"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .map(str::to_string)
        .collect();
    let handoff = (!write.form.is_empty() && !changes.is_empty()).then(|| HandoffTarget {
        path: write.path.clone(),
        parameters: write.parameters.clone(),
        form: write.form.clone(),
        screen_location: location.clone(),
        transition: transition.clone(),
        extra_path,
        fields: write.fields.keys().cloned().collect(),
    });
    ApprovalRequest {
        target_kind: TargetKind::Screen,
        target_name: format!("{location}#{transition}"),
        classification: Classification::Write,
        args: serde_json::to_value(write).unwrap_or(Json::Null),
        summary,
        changes,
        snapshot: json!({"path": write.path, "parameters": write.parameters, "element": write.element,
            "service": write.held.get("service")}),
        record_refs: json!({"path": write.path, "parameters": write.parameters}),
        preview: PreviewMode::OnDemand,
        handoff,
    }
}

/// Run an approved held write: the conflict check first (the screen rendered again and its values compared with the
/// originals), then the exact request under the approval's grant, then the result recorded on the approval.
fn execute(
    ctx: &mut TurnContext<'_>,
    factory: &Arc<ExecutionContextFactory>,
    view: &approval::ApprovalView,
) -> ToolOutcome {
    let write: PendingWrite = match serde_json::from_value(view.args.clone()) {
        Ok(write) => write,
        Err(error) => return ToolOutcome::Failed(format!("the approved write does not read: {error}")),
    };
    let Some(sessions) = screen::sessions() else {
        return ToolOutcome::Failed("screens are not available on this server, so nothing was saved".to_string());
    };
    let transition = write.transition().to_string();
    let owner = ctx.user.user_id.clone();
    let session_id = session_of(ctx, sessions.as_ref());
    match sessions.recheck(&session_id, &owner, &write) {
        Ok(conflicts) if !conflicts.is_empty() => {
            let changed: Vec<String> = conflicts
                .iter()
                .map(|conflict| {
                    format!(
                        "{} is {} now (it was {}, you proposed {})",
                        if conflict.label.is_empty() {
                            &conflict.field
                        } else {
                            &conflict.label
                        },
                        conflict.current,
                        conflict.original,
                        conflict.proposed
                    )
                })
                .collect();
            approval::conflict(ctx, &view.approval_id, &serde_json::to_value(&conflicts).unwrap_or(Json::Null));
            return ToolOutcome::text(format!(
                "The record changed after {transition} was proposed, so nothing was saved: {}. Read the screen again                  and propose the write anew if it is still wanted.",
                changed.join(", ")
            ));
        }
        Ok(_) => {}
        Err(error) => {
            return ToolOutcome::text(format!(
                "The screen of {transition} could not be checked again ({error}), so nothing was saved."
            ))
        }
    }
    let grant = match approval::begin(ctx, &view.approval_id, &view.args) {
        Ok(grant) => grant,
        Err(why) => return ToolOutcome::text(why),
    };
    let screen_grant = ApprovalGrant {
        approval_id: grant.approval_id.clone(),
        args_hash: write.args_hash.clone(),
        agent_id: ctx.agent.agent_id.clone(),
        run_id: ctx.run.map(|run| run.run_id.clone()),
    };
    match sessions.execute_approved(&session_id, &owner, &write, &screen_grant) {
        Ok(answer) => {
            approval::finish(
                ctx,
                &grant.approval_id,
                answer.ok,
                &json!({"ok": answer.ok, "path": answer.path, "text": super::format::cut(&answer.text, 2000)}),
            );
            if let Some(conversation_id) = ctx.screen.as_deref().map(|handle| handle.conversation_id.clone()) {
                keep_state(ctx, factory, &conversation_id, &answer.state);
            }
            if ctx.caps.has(caps::SCREEN_FOLLOW) {
                let stamp = follow_stamp_of(factory, ctx);
                emit_follow(ctx, &answer.follow, stamp.as_ref());
            }
            let line = if answer.ok {
                format!("The user approved {transition}, and it was saved.")
            } else {
                format!("The user approved {transition}, but the save reported an error, below.")
            };
            ToolOutcome::Done(ToolResult {
                text: format!("{line}\n{}", answer.text),
                data: Some(json!({"ok": answer.ok, "path": answer.path, "parameters": answer.parameters,
                    "approval": if answer.ok { "executed" } else { "failed" }, "approvalId": grant.approval_id})),
            })
        }
        Err(error) => {
            approval::finish(ctx, &grant.approval_id, false, &json!({"error": error.to_string()}));
            ToolOutcome::text(format!(
                "The user approved {transition}, but it did not run ({error}), so nothing was saved."
            ))
        }
    }
}

/// Lessons a screen answer shows: up to this many.
const SCREEN_LESSONS: usize = 2;

/// Append the `lesson:` lines of `path` (and the screens above it) for the run's user to a screen answer.
fn add_lessons(ctx: &TurnContext<'_>, text: &mut String, path: &str) {
    let Some(anchor) = crate::llm::memory::Anchor::parse(path).filter(|anchor| matches!(anchor, crate::llm::memory::Anchor::Screen(_))) else {
        return;
    };
    let Some(facade) = crate::llm::skills::facade() else {
        return;
    };
    let lessons = crate::llm::memory::active_lessons(&facade, &ctx.user.user_id, &anchor, SCREEN_LESSONS);
    for line in crate::llm::memory::lesson_lines(&lessons) {
        text.push('\n');
        text.push_str(&line);
    }
}

/// The session reads its screen again after the user saved the write themselves.
fn refresh(ctx: &mut TurnContext<'_>, factory: &Arc<ExecutionContextFactory>, write: &PendingWrite) {
    let Some(sessions) = screen::sessions() else {
        return;
    };
    let session_id = session_of(ctx, sessions.as_ref());
    if session_id.is_empty() {
        return;
    }
    let url = ScreenState {
        path: write.path.clone(),
        parameters: write.parameters.clone(),
        values: std::collections::BTreeMap::new(),
    }
    .url();
    let command = ScreenCommand {
        agent_id: ctx.agent.agent_id.clone(),
        run_id: ctx.run.map(|run| run.run_id.clone()),
        go: Some(url),
        ..ScreenCommand::default()
    };
    match sessions.command(&session_id, &ctx.user.user_id, &command) {
        Ok(answer) => {
            if let Some(conversation_id) = ctx.screen.as_deref().map(|handle| handle.conversation_id.clone()) {
                keep_state(ctx, factory, &conversation_id, &answer.state);
            }
        }
        Err(error) => tracing::warn!("the screen session did not read its screen again after a hand-off: {error}"),
    }
}

crate::submit_tool!(ScreenTool);

/// The command of a call's arguments.
fn command_of(args: &Json) -> Result<ScreenCommand, String> {
    let text = |name: &str| {
        args.get(name)
            .and_then(Json::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    let mut set = Vec::new();
    match args.get("set") {
        None | Some(Json::Null) => {}
        Some(Json::Object(values)) => {
            for (element, value) in values {
                let value = match value {
                    Json::String(text) => text.clone(),
                    Json::Null => String::new(),
                    Json::Bool(flag) => if *flag { "Y" } else { "N" }.to_string(),
                    other => other.to_string(),
                };
                set.push((element.clone(), value));
            }
        }
        Some(_) => {
            return Err("set is an object of element names and their values, as {\"Name\": \"Ann\"}".to_string())
        }
    }
    let rows = match args.get("rows") {
        None | Some(Json::Null) => None,
        Some(value) => {
            let rows = value
                .as_u64()
                .and_then(|rows| u16::try_from(rows).ok())
                .filter(|rows| (10..=screen::MAX_ROWS).contains(rows))
                .ok_or_else(|| format!("rows is a number from 10 to {}", screen::MAX_ROWS))?;
            Some(rows)
        }
    };
    Ok(ScreenCommand {
        go: text("go"),
        set,
        find: text("find"),
        press: text("do"),
        key: text("key"),
        elements: args.get("elements").and_then(Json::as_bool).unwrap_or(false),
        rows,
        ..ScreenCommand::default()
    })
}

/// Run `command` in the conversation's session, opening it (or opening it again, when this node no longer has it)
/// from the state the conversation kept.
fn run_command(
    factory: &Arc<ExecutionContextFactory>,
    sessions: &dyn ScreenSessions,
    handle: &mut ScreenHandle,
    user: &crate::llm::run::context::UserRef,
    agent: &crate::llm::run::context::AgentDef,
    follow: bool,
    command: &ScreenCommand,
) -> Result<ScreenAnswer, ScreenError> {
    for attempt in 0..2 {
        if handle.session_id.is_empty() {
            handle.session_id = match sessions.find(&user.user_id, &handle.conversation_id) {
                Some(id) => id,
                None => sessions.open(&open_spec(factory, handle, user, agent, follow, command))?,
            };
        }
        match sessions.command(&handle.session_id, &user.user_id, command) {
            Err(ScreenError::NotFound) if attempt == 0 => handle.session_id.clear(),
            other => return other,
        }
    }
    Err(ScreenError::NotFound)
}

fn open_spec(
    factory: &Arc<ExecutionContextFactory>,
    handle: &ScreenHandle,
    user: &crate::llm::run::context::UserRef,
    agent: &crate::llm::run::context::AgentDef,
    follow: bool,
    command: &ScreenCommand,
) -> OpenSpec {
    let state = factory
        .entity()
        .and_then(|facade| match screen::load_state(&facade, &handle.conversation_id) {
            Ok(state) => state,
            Err(error) => {
                tracing::error!(
                    "the screen state of conversation {} could not be read: {error}",
                    handle.conversation_id
                );
                None
            }
        });
    OpenSpec {
        user_id: user.user_id.clone(),
        username: user.username.clone(),
        conversation_id: handle.conversation_id.clone(),
        agent_id: agent.agent_id.clone(),
        run_id: command.run_id.clone(),
        start_path: agent
            .start_path
            .clone()
            .filter(|path| screen::path_problem(path).is_none())
            .unwrap_or_else(|| screen::DEFAULT_START_PATH.to_string()),
        columns: handle.size.map_or(screen::DEFAULT_COLUMNS, |(columns, _)| columns),
        rows: command
            .rows
            .unwrap_or_else(|| handle.size.map_or(screen::DEFAULT_ROWS, |(_, rows)| rows)),
        follow,
        state,
    }
}

/// `text` with its last line, the outcome, replaced by `outcome`.
fn replace_outcome(text: &str, outcome: &str) -> String {
    let kept = match text.rfind("\n--- ") {
        Some(at) => &text[..at],
        None if text.starts_with("--- ") => "",
        None => text,
    };
    if kept.is_empty() {
        outcome.to_string()
    } else {
        format!("{kept}\n{outcome}")
    }
}

/// The follow stamp of the run's conversation ([`crate::llm::assist::follow_stamp`]), when it has one.
fn follow_stamp_of(factory: &Arc<ExecutionContextFactory>, ctx: &TurnContext<'_>) -> Option<FollowStamp> {
    let conversation_id = ctx.screen.as_deref().map(|handle| handle.conversation_id.clone())?;
    let facade = factory.entity()?;
    Some(crate::llm::assist::follow_stamp(&facade, &conversation_id))
}

/// The session's follow events as stored events of the run, each stamped with the follow owner and its page version.
fn emit_follow(ctx: &mut TurnContext<'_>, events: &[screen::FollowEvent], stamp: Option<&FollowStamp>) {
    for event in events {
        let mut payload = event.payload.clone();
        if let Some(stamp) = stamp {
            stamp.apply(&mut payload);
        }
        ctx.events.emit(&event.event_type, payload);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arguments_become_one_command_in_order() {
        let command = command_of(
            &json!({"go": " /apps/x?a=1 ", "set": {"Name": "Ann", "Count": 3, "Done": true},
            "do": "Save", "elements": true, "rows": 40}),
        )
        .unwrap();
        assert_eq!(command.go.as_deref(), Some("/apps/x?a=1"));
        assert_eq!(
            command.set,
            [
                ("Count".to_string(), "3".to_string()),
                ("Done".to_string(), "Y".to_string()),
                ("Name".to_string(), "Ann".to_string())
            ]
        );
        assert_eq!(command.press.as_deref(), Some("Save"));
        assert!(command.elements);
        assert_eq!(command.rows, Some(40));
        assert!(command_of(&json!({"rows": 500})).is_err());
        assert!(command_of(&json!({"set": ["Name", "Ann"]})).is_err());
        assert_eq!(command_of(&json!({})).unwrap(), ScreenCommand::default());
    }

    #[test]
    fn an_outcome_line_is_replaced_and_a_held_write_asks_for_its_approval() {
        assert_eq!(
            replace_outcome("path /a\n--- ok", "--- error no"),
            "path /a\n--- error no"
        );
        assert_eq!(replace_outcome("--- ok", "--- error no"), "--- error no");
        let answer = ScreenAnswer {
            ok: true,
            text: "path /apps/x\nerror held\n--- ok".to_string(),
            path: "/apps/x".to_string(),
            title: "apps ▸ Thing Detail".to_string(),
            ..ScreenAnswer::default()
        };
        let write = PendingWrite {
            held: json!({"transition": "updateThing", "screenLocation": "component://x/Thing.xml", "argsHash": "h",
                "extraPath": ["T1"]}),
            args_hash: "h".to_string(),
            url: "/apps/x/updateThing/T1".to_string(),
            path: "/apps/x".to_string(),
            form: "EditThing".to_string(),
            fields: std::collections::BTreeMap::from([("description".to_string(), "New".to_string())]),
            changes: vec![crate::llm::screen::FieldChange {
                field: "description".to_string(),
                label: "Description".to_string(),
                original: "Old".to_string(),
                proposed: "New".to_string(),
            }],
            ..PendingWrite::default()
        };
        let request = approval_request(&answer, &write);
        assert_eq!(request.target_kind, TargetKind::Screen);
        assert_eq!(request.target_name, "component://x/Thing.xml#updateThing");
        assert_eq!(request.summary, "updateThing on Thing Detail");
        assert_eq!(
            request.changes,
            [ChangeRow {
                field: "description".to_string(),
                label: "Description".to_string(),
                current: "Old".to_string(),
                proposed: "New".to_string(),
            }]
        );
        assert_eq!(request.args["argsHash"], "h", "the held request is what runs");
        let handoff = request.handoff.expect("a form's write may be edited in the screen");
        assert_eq!((handoff.form.as_str(), handoff.transition.as_str()), ("EditThing", "updateThing"));
        assert_eq!(handoff.extra_path, ["T1"]);
        assert_eq!(handoff.fields, ["description"]);
        let refused = refused(&answer, "no");
        let ToolOutcome::Done(result) = refused else {
            panic!("a refusal is a result");
        };
        assert!(result.text.ends_with("error held\n--- error no"), "{}", result.text);
    }
}
