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

//! The drawer's writes: a turn (a new run, or a message queued to the active one), run control (cancel, retry), follow
//! (claim it, report a navigation, report a hand-off's outcome), the cards (decide, preview, answer) and feedback.
//! Each acts on the caller's own rows only.

use moqui_lib::object::Value;
use serde_json::{Value as Json, json};

use super::state::{DrawerState, user_now, week_start};
use super::{Answer, Caller, DrawerError, Params};
use crate::entity::EntityValue;
use crate::llm::approval::{self, Channel, DecideError, Decider, Decision};
use crate::llm::conf::Tier;
use crate::llm::prompt::{Surface, build_tail};
use crate::llm::run::state::RunState;
use crate::llm::run::{self, AgentDef, StartRequest, Started, in_tx, quietly};
use crate::llm::skills;
use crate::llm::tools::PauseKind;
use crate::llm::{int_of, text_of};

/// A conversation's title is the first line of its first message, cut here.
const TITLE_CHARS: usize = 80;
/// `searchText` keeps the user's messages up to this many characters.
const SEARCH_TEXT_CHARS: usize = 16_000;
/// A feedback note is cut here.
const NOTE_CHARS: usize = 2000;

fn decide_error(error: DecideError, what: &str, id: &str) -> DrawerError {
    match error {
        DecideError::NotFound => DrawerError::not_found(what, id),
        DecideError::Refused(text) => DrawerError::conflict(text),
        DecideError::Failed(text) => DrawerError {
            status: 500,
            code: "failed",
            message: text,
            retry_after: None,
        },
    }
}

/// The state of the `state` parameter, if any, with the conf's secret keys dropped.
fn state_of(caller: &Caller, params: &Params) -> Option<DrawerState> {
    let extra = crate::llm::facade(&caller.factory)
        .conf()
        .content_log
        .redact_keys
        .clone();
    params.json("state").map(|value| DrawerState::of(&value, &extra))
}

/// Make `client_id` the conversation's follow owner (or, with `release`, give it up when it is), and raise its page
/// version to `version`. Out: the owner and the version now.
pub(super) fn claim_follow(
    caller: &Caller,
    conversation_id: &str,
    client_id: &str,
    version: Option<i64>,
    release: bool,
) -> Result<(Option<String>, i64), DrawerError> {
    claim_follow_if(caller, conversation_id, client_id, version, release, false)
        .map(|(owner, stored, _)| (owner, stored))
}

/// [`claim_follow`], and with `if_free` only when no other tab owns follow, checked under the same lock: the third of
/// the answer says whether it changed anything.
fn claim_follow_if(
    caller: &Caller,
    conversation_id: &str,
    client_id: &str,
    version: Option<i64>,
    release: bool,
    if_free: bool,
) -> Result<(Option<String>, i64, bool), DrawerError> {
    let facade = &caller.facade;
    Ok(quietly(|| {
        in_tx(facade, || {
            let mut row = facade
                .find(run::CONVERSATION)?
                .eq("conversationId", Value::string(conversation_id))
                .eq("userId", Value::string(&caller.user_id))
                .for_update(true)
                .disable_authz()
                .one()?
                .ok_or_else(|| moqui_lib::error::Error::new(format!("there is no conversation {conversation_id}")))?;
            let mut owner = text_of(&row, "clientId");
            let mut stored = int_of(&row, "contextVersion").unwrap_or(0);
            if if_free && owner.as_deref().is_some_and(|owner| owner != client_id) {
                return Ok((owner, stored, false));
            }
            if release {
                if owner.as_deref() == Some(client_id) {
                    owner = None;
                }
            } else {
                owner = Some(client_id.to_string());
                stored = stored.max(version.unwrap_or(0));
            }
            row.set("clientId", owner.as_ref().map_or(Value::Null, Value::string));
            row.set("contextVersion", Value::I64(stored));
            row.update(facade)?;
            Ok((owner, stored, true))
        })
    })?)
}

/// Bring the conversation's screen to the user's page; a failure is logged and the call goes on (the session reopens
/// from the kept state next time).
fn sync(caller: &Caller, conversation_id: &str, state: &DrawerState) {
    let Some(browser) = state.browser_state() else {
        return;
    };
    if let Err(error) = crate::llm::screen::sync_in(&caller.facade, &caller.user_id, conversation_id, &browser) {
        tracing::warn!("the page of agent conversation {conversation_id} did not sync in: {error}");
    }
}

/// The tail of a message sent from `state`'s page now, with the skills that may fit it when the conversation's frozen
/// catalog does not list them (plan 7.5, answered question 7).
pub(crate) fn tail_of(
    caller: &Caller,
    state: Option<&DrawerState>,
    agent: &AgentDef,
    conversation_id: Option<&str>,
    text: &str,
    surface: Surface,
) -> String {
    let locale = crate::user::locale().ok().flatten().unwrap_or_default();
    let mut spec = state
        .cloned()
        .unwrap_or_default()
        .tail_spec(Some(user_now()), week_start(&locale));
    if let Some(id) = conversation_id {
        // the conversation's canvas as the server has it, whatever the page said
        spec.canvas = super::canvas::tail_canvas(caller, id);
    }
    // a first message's tail names only the learned skills that fit it (they never enter the catalog built from it)
    let index = skills::SkillIndex::for_audience(
        &caller.factory,
        &caller.facade,
        &skills::Audience {
            user_id: Some(&caller.user_id),
            agent_id: Some(&agent.agent_id),
            surface: Some(surface),
        },
    );
    spec.skills = skills::tail_skills(&caller.facade, &index, conversation_id, text);
    build_tail(&spec)
}

/// A conversation with no title gets the first line of `text`; the user's text joins its `searchText`. A failure is
/// logged: neither is needed for the turn.
fn note_text(caller: &Caller, conversation_id: &str, text: &str) {
    let facade = &caller.facade;
    let noted = quietly(|| {
        in_tx(facade, || {
            let Some(mut row) = facade
                .find(run::CONVERSATION)?
                .eq("conversationId", Value::string(conversation_id))
                .for_update(true)
                .disable_authz()
                .one()?
            else {
                return Ok(());
            };
            if text_of(&row, "title").is_none() {
                let first = text
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .unwrap_or_default();
                let mut title: String = first.chars().take(TITLE_CHARS).collect();
                if first.chars().count() > TITLE_CHARS {
                    title.push('…');
                }
                row.set("title", Value::string(title));
            }
            let search = text_of(&row, "searchText").unwrap_or_default();
            if search.chars().count() < SEARCH_TEXT_CHARS {
                let joined = if search.is_empty() {
                    text.to_string()
                } else {
                    format!("{search}\n{text}")
                };
                row.set(
                    "searchText",
                    Value::string(joined.chars().take(SEARCH_TEXT_CHARS).collect::<String>()),
                );
            }
            row.update(facade)
        })
    });
    if let Err(error) = noted {
        tracing::warn!("the title and search text of agent conversation {conversation_id} were not kept: {error}");
    }
}

/// The answer of a turn or a retry.
fn turn_answer(caller: &Caller, started: &Started) -> Answer {
    let run = quietly(|| run::load_run(&caller.facade, &started.run_id))?;
    let run_row = quietly(|| {
        caller
            .facade
            .find(run::RUN)?
            .eq("runId", Value::string(&started.run_id))
            .disable_authz()
            .one()
    })?;
    let conversation = caller.conversation(&started.conversation_id)?;
    let state = run.as_ref().map_or(started.state, |run| run.state);
    Ok(json!({
        "conversationId": started.conversation_id,
        "runId": started.run_id,
        "state": state.wire(),
        "statusId": state.status_id(),
        "queued": started.queued,
        "existing": started.existing,
        "lastEventSeq": run_row.as_ref().and_then(|row| int_of(row, "lastEventSeq")).unwrap_or(0),
        "follow": {
            "clientId": text_of(&conversation, "clientId"),
            "contextVersion": int_of(&conversation, "contextVersion").unwrap_or(0),
        },
        "eventsPath": format!("/assist/runs/{}/events", started.run_id),
    }))
}

/// The agent a turn runs: the conversation's, else the one asked for (`assist` by default); it must serve the surface.
fn turn_agent(
    caller: &Caller,
    conversation: Option<&EntityValue>,
    asked: Option<String>,
    surface: Surface,
) -> Result<AgentDef, DrawerError> {
    let agent_id = conversation
        .and_then(|row| text_of(row, "agentId"))
        .or(asked)
        .unwrap_or_else(|| "assist".to_string());
    quietly(|| AgentDef::load(&caller.facade, &agent_id))?
        .filter(|agent| !agent.disabled && agent.serves(surface))
        .ok_or_else(|| DrawerError::not_found("agent", &agent_id))
}

/// Start the run of a turn, or find the one a send at the same moment with the same client message id made.
fn start(caller: &Caller, request: &StartRequest) -> Result<Started, DrawerError> {
    match run::start_run(&caller.factory, request) {
        Ok(started) => Ok(started),
        Err(error) => {
            let again = quietly(|| {
                run::existing_message(&caller.facade, &caller.user_id, request.client_message_id.as_deref())
            })?;
            again.ok_or_else(|| DrawerError::of(&error))
        }
    }
}

/// `POST turn`: the user's message. A new conversation, or one of the user's; while its run is active the message is
/// queued for that run (`queued`), else it starts a run. The same `clientMessageId` again answers what the first send
/// did (`existing`). The sending tab becomes the follow owner, and its page syncs into the conversation's screen.
pub(super) fn turn(caller: &Caller, params: &Params) -> Answer {
    let text = params.required("text")?;
    if text.chars().count() > super::MAX_MESSAGE_CHARS {
        return Err(DrawerError::invalid(format!(
            "the message is over {} characters",
            super::MAX_MESSAGE_CHARS
        )));
    }
    let client_message_id = params.client_id("clientMessageId")?;
    let client_id = params.client_id("clientId")?;
    let tier =
        match params.text("tier") {
            Some(name) => Some(Tier::parse(&name).ok_or_else(|| {
                DrawerError::invalid(format!("{name} is not a tier; it is fast, balanced or thorough"))
            })?),
            None => None,
        };
    let version = params.int("contextVersion");
    // the same send again changes nothing
    if let Some(existing) =
        quietly(|| run::existing_message(&caller.facade, &caller.user_id, Some(&client_message_id)))?
    {
        return turn_answer(caller, &existing);
    }
    let conversation_id = params.text("conversationId");
    let conversation = match &conversation_id {
        Some(id) => Some(caller.conversation(id)?),
        None => None,
    };
    // the terminal's pane is a client of these calls too: its conversations are the terminal's (plan 4.12)
    let surface = match params.text("surface").as_deref() {
        None | Some("drawer") => Surface::Drawer,
        Some("tui") => Surface::Tui,
        Some(other) => return Err(DrawerError::invalid(format!("surface {other} is not drawer or tui"))),
    };
    let agent = turn_agent(caller, conversation.as_ref(), params.text("agentId"), surface)?;
    let state = state_of(caller, params);
    let attachment_ids: Vec<String> = match params.json("attachments") {
        Some(Json::Array(ids)) => ids
            .iter()
            .filter_map(|id| id.as_str().map(str::to_string).or_else(|| id.as_i64().map(|n| n.to_string())))
            .collect(),
        Some(_) => return Err(DrawerError::invalid("attachments is a list of attachment ids")),
        None => params.text("attachments").map(|id| vec![id]).unwrap_or_default(),
    };
    let attachments = super::files::for_turn(caller, &attachment_ids, conversation_id.as_deref())?;
    let request = StartRequest {
        user_id: caller.user_id.clone(),
        agent_id: agent.agent_id.clone(),
        conversation_id: conversation_id.clone(),
        surface,
        text: text.clone(),
        tail: Some(tail_of(caller, state.as_ref(), &agent, conversation_id.as_deref(), &text, surface)),
        client_message_id: Some(client_message_id),
        tier,
        screen_state: state.as_ref().and_then(DrawerState::screen_state),
        defer_submit: true,
        attachments: attachments.clone(),
        ..StartRequest::default()
    };
    // a turn refused (the tarpit, a bad conversation) moves neither follow nor the page; one taken makes the tab the follow
    // owner and syncs its page in before the new run works (a run rejected for the quota does not work)
    let started = start(caller, &request)?;
    if !started.existing && started.state != RunState::Rejected {
        let claimed = claim_follow(caller, &started.conversation_id, &client_id, version, false);
        if let (Some(id), Some(state)) = (&conversation_id, &state) {
            sync(caller, id, state);
        }
        if let Err(error) = run::submit_started(&caller.factory, &started) {
            tracing::error!("agent run {} was not handed to the pool: {error}", started.run_id);
        }
        claimed?;
    }
    if !started.existing {
        note_text(caller, &started.conversation_id, &text);
        super::files::bind(caller, &attachments, &started.conversation_id);
    }
    turn_answer(caller, &started)
}

/// `POST cancel`: stop one of the user's runs; the partial answer is kept.
pub(super) fn cancel(caller: &Caller, params: &Params) -> Answer {
    let run_id = params.required("runId")?;
    caller.run(&run_id)?;
    let state = run::cancel_run(&caller.factory, &run_id, Some(&caller.user_id))?;
    Ok(json!({"runId": run_id, "state": state.wire(), "statusId": state.status_id()}))
}

/// The text of a conversation's last user message, with the tail it was sent with.
fn last_user_message(caller: &Caller, conversation_id: &str) -> Result<Option<(String, Option<String>)>, DrawerError> {
    let facade = &caller.facade;
    let Some(message) = quietly(|| {
        facade
            .find(run::MESSAGE)?
            .eq("conversationId", Value::string(conversation_id))
            .eq("role", Value::string("user"))
            .order_by("messageSeq", true)
            .limit(1)
            .disable_authz()
            .one()
    })?
    else {
        return Ok(None);
    };
    let message_id = text_of(&message, "messageId").unwrap_or_default();
    let parts = quietly(|| {
        facade
            .find(run::PART)?
            .eq("messageId", Value::string(&message_id))
            .eq("partKindEnumId", Value::string("AgpkText"))
            .order_by("partSeq", false)
            .disable_authz()
            .list()
    })?;
    let text: Vec<String> = parts
        .values()
        .iter()
        .filter_map(|part| text_of(part, "textValue"))
        .collect();
    let tail = text_of(&message, "metadataJson")
        .and_then(|text| serde_json::from_str::<Json>(&text).ok())
        .and_then(|metadata| metadata.get("tail").and_then(Json::as_str).map(str::to_string));
    Ok(Some((text.join("\n"), tail)))
}

/// `POST retry`: run the conversation's last user message again, after its run failed or was stopped. Refused while
/// a run is active. The message is sent again (it shows twice in the history) with a new `clientMessageId`.
pub(super) fn retry(caller: &Caller, params: &Params) -> Answer {
    let conversation_id = params.required("conversationId")?;
    let client_message_id = params.client_id("clientMessageId")?;
    let client_id = params.client_id("clientId")?;
    if let Some(existing) =
        quietly(|| run::existing_message(&caller.facade, &caller.user_id, Some(&client_message_id)))?
    {
        return turn_answer(caller, &existing);
    }
    let conversation = caller.conversation(&conversation_id)?;
    if let Some(active) = text_of(&conversation, "activeRunId") {
        if quietly(|| run::load_run(&caller.facade, &active))?.is_some_and(|run| !run.state.is_terminal()) {
            return Err(DrawerError::conflict(
                "a run of this conversation is active; stop it first",
            ));
        }
    }
    let (text, stored_tail) = last_user_message(caller, &conversation_id)?
        .filter(|(text, _)| !text.trim().is_empty())
        .ok_or_else(|| DrawerError::conflict("this conversation has no message to retry"))?;
    // a retry stays on the conversation's surface (the drawer's, or the terminal's)
    let surface = text_of(&conversation, "surfaceEnumId")
        .filter(|surface| surface == Surface::Tui.enum_id())
        .map_or(Surface::Drawer, |_| Surface::Tui);
    let agent = turn_agent(caller, Some(&conversation), None, surface)?;
    let state = state_of(caller, params);
    let version = params.int("contextVersion");
    let tail = match &state {
        Some(state) => Some(tail_of(caller, Some(state), &agent, Some(&conversation_id), &text, surface)),
        None => stored_tail,
    };
    let tier = params.text("tier").and_then(|name| Tier::parse(&name));
    let request = StartRequest {
        user_id: caller.user_id.clone(),
        agent_id: agent.agent_id,
        conversation_id: Some(conversation_id.clone()),
        surface,
        text,
        tail,
        client_message_id: Some(client_message_id),
        tier,
        defer_submit: true,
        ..StartRequest::default()
    };
    let started = start(caller, &request)?;
    if !started.existing && started.state != RunState::Rejected {
        let claimed = claim_follow(caller, &conversation_id, &client_id, version, false);
        if let Some(state) = &state {
            sync(caller, &conversation_id, state);
        }
        if let Err(error) = run::submit_started(&caller.factory, &started) {
            tracing::error!("agent run {} was not handed to the pool: {error}", started.run_id);
        }
        claimed?;
    }
    turn_answer(caller, &started)
}

/// `POST follow`: this tab follows the conversation's agent (its `ui.*` events are for it), or with `release` gives
/// it up.
pub(super) fn follow(caller: &Caller, params: &Params) -> Answer {
    let conversation_id = params.required("conversationId")?;
    let client_id = params.client_id("clientId")?;
    caller.conversation(&conversation_id)?;
    let release = params.flag("release").unwrap_or(false);
    let (owner, version) = claim_follow(
        caller,
        &conversation_id,
        &client_id,
        params.int("contextVersion"),
        release,
    )?;
    Ok(json!({
        "conversationId": conversation_id,
        "follow": {"clientId": owner, "contextVersion": version},
        "following": owner.as_deref() == Some(client_id.as_str()),
    }))
}

/// `POST context`: the user navigated (or changed the page) in a tab. From the follow owner (or any tab when none
/// follows, which then does), the page version is raised and the page syncs into the conversation's screen, so the
/// agent goes on from where the user went; from another tab it changes nothing (`applied` false).
pub(super) fn context(caller: &Caller, params: &Params) -> Answer {
    let conversation_id = params.required("conversationId")?;
    let client_id = params.client_id("clientId")?;
    let version = params
        .int("contextVersion")
        .ok_or_else(|| DrawerError::invalid("contextVersion is required"))?;
    caller.conversation(&conversation_id)?;
    // the owner checked and claimed under one lock: another tab's claim between the two is not taken over
    let (owner, stored, applied) = claim_follow_if(caller, &conversation_id, &client_id, Some(version), false, true)?;
    if !applied {
        return Ok(json!({
            "conversationId": conversation_id,
            "applied": false,
            "follow": {"clientId": owner, "contextVersion": stored},
        }));
    }
    if let Some(state) = state_of(caller, params) {
        sync(caller, &conversation_id, &state);
    }
    Ok(json!({
        "conversationId": conversation_id,
        "applied": true,
        "follow": {"clientId": owner, "contextVersion": stored},
    }))
}

/// The status code of a hand-off status id.
fn handoff_status(status_id: Option<&str>) -> &'static str {
    match status_id {
        Some("AghWatching") => "watching",
        Some("AghSaved") => "saved",
        Some("AghFailed") => "failed",
        Some("AghAbandoned") => "abandoned",
        _ => "unknown",
    }
}

/// `POST submitted`: the outcome of a hand-off's form in the browser. It is a signal only: the user's own save is
/// observed on the server, so this answers the watch's status as the server has it. With `abandoned` (the user
/// navigated away or pressed Skip) the watch is abandoned and the run goes on with that; only from the browser
/// session with its token.
pub(super) fn submitted(caller: &Caller, params: &Params) -> Answer {
    let handoff_id = params.required("handoffId")?;
    let read = |caller: &Caller| -> Result<EntityValue, DrawerError> {
        quietly(|| {
            caller
                .facade
                .find("moqui.agent.AgentHandoff")?
                .eq("handoffId", Value::string(&handoff_id))
                .disable_authz()
                .one()
        })?
        .filter(|row| text_of(row, "userId").as_deref() == Some(caller.user_id.as_str()))
        .ok_or_else(|| DrawerError::not_found("hand-off", &handoff_id))
    };
    read(caller)?;
    if let Some(reason) = params.text("abandoned") {
        let decider = Decider::from_request(Some(Channel::Drawer)).map_err(DrawerError::refused)?;
        approval::handoff::abandon_handoff(&caller.factory, &handoff_id, &decider, &reason)
            .map_err(|error| decide_error(error, "hand-off", &handoff_id))?;
    }
    let row = read(caller)?;
    Ok(json!({
        "handoffId": handoff_id,
        "approvalId": text_of(&row, "approvalId"),
        "runId": text_of(&row, "runId"),
        "status": handoff_status(text_of(&row, "statusId").as_deref()),
    }))
}

/// `POST decide`: the user's decision on an approval of theirs (approve, with edited arguments for a service or tool
/// call; reject with feedback; edit, which hands the write to the page: the deciding tab becomes the follow owner).
/// Only from the browser session by its cookie with the session token: a header login is refused (403).
pub(super) fn decide(caller: &Caller, params: &Params) -> Answer {
    let approval_id = params.required("approvalId")?;
    let decider = Decider::from_request(Some(Channel::Drawer)).map_err(DrawerError::refused)?;
    let decision = match params.required("decision")?.as_str() {
        "approve" => Decision::Approve {
            arguments: params.json("arguments").filter(Json::is_object),
        },
        "reject" => Decision::Reject {
            feedback: params.text("feedback").unwrap_or_default(),
        },
        "edit" => Decision::Edit,
        other => {
            return Err(DrawerError::invalid(format!(
                "{other} is not a decision; it is approve, reject or edit"
            )));
        }
    };
    let view = quietly(|| approval::load_for_owner(&caller.facade, &approval_id, &caller.user_id))?
        .ok_or_else(|| DrawerError::not_found("approval", &approval_id))?;
    // Edit in screen: the deciding tab owns follow at its page version (the hand-off's page events are stamped with both),
    // and the answer gives them back so the tab applies them
    let mut follow = Json::Null;
    if decision == Decision::Edit {
        if let Some(client_id) = params.text("clientId") {
            let client_id = params.client_id("clientId").map(|_| client_id)?;
            if let Some(run) = quietly(|| run::load_run(&caller.facade, &view.run_id))? {
                let (owner, stored) = claim_follow(
                    caller,
                    &run.conversation_id,
                    &client_id,
                    params.int("contextVersion"),
                    false,
                )?;
                follow = json!({"clientId": owner, "contextVersion": stored});
            }
        }
    }
    let decided = approval::decide(&caller.factory, &approval_id, &decider, &decision)
        .map_err(|error| decide_error(error, "approval", &approval_id))?;
    Ok(json!({
        "approvalId": decided.approval_id,
        "status": decided.status.as_str(),
        "statusId": decided.status.status_id(),
        "handoffId": decided.handoff_id,
        "follow": follow,
    }))
}

/// `POST preview`: the effects of a pending approval of the user's, run in the sim overlay (kept on the approval).
pub(super) fn preview(caller: &Caller, params: &Params) -> Answer {
    let approval_id = params.required("approvalId")?;
    let preview = approval::preview::preview_approval(&caller.factory, &approval_id, &caller.user_id)
        .map_err(|error| decide_error(error, "approval", &approval_id))?;
    Ok(json!({"approvalId": approval_id, "preview": preview}))
}

/// `POST answer`: the user's answer to the question a run of theirs waits on (an `ask` question or form, or the
/// budget's "continue?"): `choices` (or `choice`), `values` for a form, `text` for free text. A form's values are
/// checked first (400 with the problems).
pub(super) fn answer(caller: &Caller, params: &Params) -> Answer {
    let run_id = params.required("runId")?;
    let run = caller.run(&run_id)?;
    // a remote's link (Auth) is answered Done or Cancel as a question is
    let waiting = run.state.is_paused()
        && matches!(run.pause_kind, Some(PauseKind::Question | PauseKind::Budget | PauseKind::Auth));
    if !waiting {
        return Err(DrawerError::conflict("this run does not wait for an answer"));
    }
    if let Some(question_id) = params.text("questionId") {
        if run.pause_ref.as_deref() != Some(question_id.as_str()) {
            return Err(DrawerError::conflict(format!(
                "question {question_id} was answered or has expired"
            )));
        }
    }
    let mut data = serde_json::Map::new();
    for key in ["choices", "values"] {
        if let Some(value) = params.json(key) {
            data.insert(key.to_string(), value);
        }
    }
    if let Some(choice) = params.text("choice") {
        data.insert("choice".to_string(), json!(choice));
    }
    let data = (!data.is_empty()).then_some(Json::Object(data));
    let text = params.text("text").unwrap_or_default();
    if data.is_none() && text.is_empty() {
        return Err(DrawerError::invalid("an answer needs choices, values or text"));
    }
    if run.pause_kind == Some(PauseKind::Question) {
        if let Some(question_id) = &run.pause_ref {
            crate::llm::tools::ask::check_answer(&caller.facade, question_id, data.as_ref())
                .map_err(DrawerError::invalid)?;
        }
    }
    let state = run::submit_input(
        &caller.factory,
        &run_id,
        &caller.user_id,
        "answer",
        &text,
        data.as_ref(),
    )?;
    Ok(json!({"runId": run_id, "state": state.wire(), "statusId": state.status_id()}))
}

/// `POST feedback`: thumbs (`rating` up or down, empty clears) and a note on an assistant message of the user's,
/// kept in the message's `metadataJson.feedback`.
pub(super) fn feedback(caller: &Caller, params: &Params) -> Answer {
    let message_id = params.required("messageId")?;
    let rating = params.text("rating");
    if rating
        .as_deref()
        .is_some_and(|rating| rating != "up" && rating != "down")
    {
        return Err(DrawerError::invalid("rating is up, down or empty"));
    }
    let note: Option<String> = params.text("note").map(|note| note.chars().take(NOTE_CHARS).collect());
    let facade = &caller.facade;
    let kept = quietly(|| {
        in_tx(facade, || {
            let Some(mut message) = facade
                .find(run::MESSAGE)?
                .eq("messageId", Value::string(&message_id))
                .for_update(true)
                .disable_authz()
                .one()?
            else {
                return Ok(None);
            };
            let owned = match text_of(&message, "conversationId") {
                Some(conversation_id) => {
                    facade
                        .find(run::CONVERSATION)?
                        .eq("conversationId", Value::string(conversation_id))
                        .eq("userId", Value::string(&caller.user_id))
                        .disable_authz()
                        .count()?
                        > 0
                }
                None => false,
            };
            if !owned || text_of(&message, "role").as_deref() != Some("assistant") {
                return Ok(None);
            }
            let mut metadata = text_of(&message, "metadataJson")
                .and_then(|text| serde_json::from_str::<Json>(&text).ok())
                .filter(Json::is_object)
                .unwrap_or_else(|| json!({}));
            let feedback = if rating.is_none() && note.is_none() {
                Json::Null
            } else {
                json!({"rating": rating, "note": note, "userId": caller.user_id, "date": run::now_millis()})
            };
            if feedback.is_null() {
                if let Some(map) = metadata.as_object_mut() {
                    map.remove("feedback");
                }
            } else {
                metadata["feedback"] = feedback.clone();
            }
            message.set("metadataJson", Value::string(metadata.to_string()));
            message.update(facade)?;
            Ok(Some(feedback))
        })
    })?;
    let feedback = kept.ok_or_else(|| DrawerError::not_found("assistant message", &message_id))?;
    Ok(json!({"messageId": message_id, "feedback": feedback}))
}
