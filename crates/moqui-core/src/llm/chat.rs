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

//! Chat agent members (plans/LLM-A2A-Assist.md 4.13). An agent is a room member: its `AgentDefinition.chatUserId` is a
//! service user with role `CmrAgent` and the `agentRef` `org.moqui.agent.AgentServices.handle#ChatMessage`, so
//! `send#Message` hands it each message of the room through `deliver#AgentMessage`.
//!
//! [`handle_chat_message`] reads the message from the room (never from its parameters, so the sender is the one who
//! posted it), and answers a message that mentions the agent, any message of a direct room with it, and a sender's next
//! message in a thread where the agent already works for them. It maps `(chatRoomId, thread root, sender)` to a
//! conversation and starts a run of the agent **as the sender** on the chat surface (a sender without `AgentUse` gets a
//! short reply instead, and so does a message in a room an external user, `EXTERNAL_USER`, is in; nor does an external
//! user decide a card); the thread's earlier messages go with the first message, as untrusted text. Messages of other
//! agent members never start a run, so two agents do not talk to each other.
//!
//! The run's events come back through the observer worker ([`observe`], [`crate::llm::run::observe`]): while it works
//! the agent shows as typing every 3 seconds, with what it does as the status ("Using search…"); an approval or a
//! question is a card message (`content.assist`, `send#AssistCard`), changed in place as it is decided
//! (`update#AssistCard`, pushed as an update); the answer goes into the thread of the triggering message (in a direct
//! room, the timeline) as the agent, with the run named so a client shows "View run". Steps stay out of the room.
//!
//! Who decides a room's approval card (answered question 12): any human user who may post in the room, through the
//! card's button ([`decide_chat_card`], `chatAssistCard` in webroot.xml), with the decision rules of
//! [`crate::llm::approval::Decider`] otherwise: only a browser session by its cookie with the session token, never a
//! header login, an agent's own context or internal code; an agent member, or the chat or run-as user of any agent,
//! never decides. The write still runs as the requester. Edit in screen (a hand-off to the deciding user's own page)
//! and a question's answer stay the requester's. This reconciles plan section 1 ("the approval card … appears in the
//! room for the sender only"): the card is a room message every member sees, which names the requester; it is the
//! decision that question 12 opens to the room.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_lib::error::Error;
use moqui_lib::object::{is_empty, to_plain_string, Value};
use serde_json::{json, Value as Json};

use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::EntityFacade;
use crate::llm::approval::{self, ApprovalView, Channel, Decider, Decision};
use crate::llm::prompt::Surface;
use crate::llm::run::observe::RunEvents;
use crate::llm::run::{self, quietly, RunRow, RunState};
use crate::llm::{int_of, text_of};
use crate::service::rust_services::{rust_service, RustServiceFn};

const CHAT: &str = "org.moqui.impl.ChatServices";
const EVENT: &str = "moqui.chat.ChatEvent";
const MEMBER: &str = "moqui.chat.ChatRoomMember";
const ROOM: &str = "moqui.chat.ChatRoom";
const AGENT: &str = "moqui.agent.AgentDefinition";

/// The agent's typing status is sent this often while it works.
const TYPING_EVERY: Duration = Duration::from_secs(3);
/// The earlier messages of a thread the first message of a conversation takes, and their characters in all.
const THREAD_MESSAGES: usize = 10;
const THREAD_CHARS: usize = 2000;

/// Capabilities the tests add to the chat and trigger runs of an agent (their test tools): `(agentId, capability)`.
#[cfg(test)]
pub(crate) static TEST_CAPS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// The test capabilities of `agent_id`'s runs.
#[cfg(test)]
pub(crate) fn test_caps(agent_id: &str) -> Vec<String> {
    moqui_lib::sync::lock(&TEST_CAPS)
        .iter()
        .filter(|(agent, _)| agent == agent_id)
        .map(|(_, cap)| cap.clone())
        .collect()
}

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, handle_chat_message)?;
    rust_service!(map, decide_chat_card)?;
    Ok(())
}

type Parameters = BTreeMap<String, Value>;
type Out = Result<BTreeMap<String, Value>, Error>;

fn param(parameters: &Parameters, key: &str) -> Option<String> {
    parameters
        .get(key)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn add_error(text: impl Into<String>) {
    let text = text.into();
    ExecutionContext::with_current(|ctx| ctx.message().add_error(text));
}

fn factory() -> Option<Arc<ExecutionContextFactory>> {
    ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
}

fn out(entries: impl IntoIterator<Item = (&'static str, Value)>) -> BTreeMap<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

/// Call a chat service with `parameters`; its error messages are taken back out and returned as the error.
fn chat_call(verb_noun: &str, parameters: Json) -> Result<BTreeMap<String, Value>, Error> {
    let name = format!("{CHAT}.{verb_noun}");
    let parameters: BTreeMap<String, Value> = match moqui_lib::json::from_json(&parameters) {
        Value::Map(map) => map.into_iter().collect(),
        _ => BTreeMap::new(),
    };
    let mark = ExecutionContext::with_current(|ec| ec.message().error_mark());
    let result = crate::service::sync(&name, &parameters);
    let errors: Vec<String> = mark
        .and_then(|mark| ExecutionContext::with_current(|ec| ec.message().take_errors_since(mark)))
        .unwrap_or_default();
    match result {
        Err(error) => Err(Error::new(format!("{name}: {}", error.message()))),
        Ok(_) if !errors.is_empty() => Err(Error::new(format!("{name}: {}", errors.join("; ")))),
        Ok(results) => Ok(results),
    }
}

/// A room message as stored.
#[derive(Clone, Debug)]
struct RoomMessage {
    sender: String,
    body_text: String,
    content: Json,
    thread_root: Option<i64>,
    /// When it was sent, epoch milliseconds.
    sent: Option<i64>,
}

fn room_message(facade: &EntityFacade, room_id: &str, seq: i64) -> Result<Option<RoomMessage>, Error> {
    let Some(row) = facade
        .find(EVENT)?
        .eq("chatRoomId", Value::string(room_id))
        .eq("eventSeq", Value::I64(seq))
        .disable_authz()
        .one()?
    else {
        return Ok(None);
    };
    if text_of(&row, "eventTypeEnumId").as_deref() != Some("CetMessage") || text_of(&row, "redactedDate").is_some() {
        return Ok(None);
    }
    Ok(Some(RoomMessage {
        sender: text_of(&row, "senderUserId").unwrap_or_default(),
        body_text: text_of(&row, "bodyText").unwrap_or_default(),
        content: text_of(&row, "contentJson")
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_else(|| json!({})),
        thread_root: int_of(&row, "threadRootEventSeq"),
        sent: row.get("sentDate").and_then(crate::llm::millis_of),
    }))
}

/// A message older than this is answered only when it is its sender's newest in the room.
const STALE_MILLIS: i64 = 5 * 60_000;

/// Whether the agent may answer message `seq` now: not one sent before the agent joined the room or before the
/// conversation it would go on began, not one at or before the last message that conversation answered, and, when
/// older than a few minutes, only the sender's newest message of the room. So handing the agent an old message (its
/// entry point takes any) starts no fresh run on it.
fn answerable(
    facade: &EntityFacade,
    room_id: &str,
    seq: i64,
    message: &RoomMessage,
    agent_user_id: &str,
    conversation_id: Option<&str>,
) -> Result<bool, Error> {
    let Some(sent) = message.sent else {
        return Ok(false);
    };
    let joined = facade
        .find(MEMBER)?
        .eq("chatRoomId", Value::string(room_id))
        .eq("userId", Value::string(agent_user_id))
        .disable_authz()
        .one()?
        .and_then(|row| row.get("joinedDate").and_then(crate::llm::millis_of));
    if joined.is_some_and(|joined| sent < joined) {
        return Ok(false);
    }
    if let Some(conversation_id) = conversation_id {
        let conversation = facade
            .find(run::CONVERSATION)?
            .eq("conversationId", Value::string(conversation_id))
            .disable_authz()
            .one()?;
        if conversation
            .and_then(|row| row.get("createdDate").and_then(crate::llm::millis_of))
            .is_some_and(|created| sent < created)
        {
            return Ok(false);
        }
        // the messages its runs answered: chat:<room>:<seq>:<agent user>
        let answered = facade
            .find(run::RUN)?
            .eq("conversationId", Value::string(conversation_id))
            .disable_authz()
            .list()?
            .values()
            .iter()
            .filter_map(|row| text_of(row, "clientRequestId"))
            .filter_map(|id| {
                let rest = id.strip_prefix(&format!("chat:{room_id}:"))?;
                let (answered, agent) = rest.split_once(':')?;
                (agent == agent_user_id).then(|| answered.parse::<i64>().ok()).flatten()
            })
            .max();
        if answered.is_some_and(|answered| seq <= answered) {
            return Ok(false);
        }
    }
    if run::now_millis() - sent > STALE_MILLIS {
        let newer = facade
            .find(EVENT)?
            .eq("chatRoomId", Value::string(room_id))
            .eq("senderUserId", Value::string(&message.sender))
            .eq("eventTypeEnumId", Value::string("CetMessage"))
            .condition(moqui_db::sql::Condition::greater("eventSeq", Value::I64(seq)))
            .disable_authz()
            .count()?;
        if newer > 0 {
            return Ok(false);
        }
    }
    Ok(true)
}

/// [`answerable`] of message `seq` of the room as stored (for the tests).
#[cfg(test)]
pub(crate) fn may_answer(
    facade: &EntityFacade,
    room_id: &str,
    seq: i64,
    agent_user_id: &str,
    conversation_id: Option<&str>,
) -> Result<bool, Error> {
    match room_message(facade, room_id, seq)? {
        Some(message) => answerable(facade, room_id, seq, &message, agent_user_id, conversation_id),
        None => Ok(false),
    }
}

/// The role of a user's current membership of a room (`None` when not a member, or left a room that is not direct).
fn member_role(facade: &EntityFacade, room_id: &str, user_id: &str) -> Result<Option<String>, Error> {
    Ok(facade
        .find(MEMBER)?
        .eq("chatRoomId", Value::string(room_id))
        .eq("userId", Value::string(user_id))
        .disable_authz()
        .one()?
        .filter(|row| text_of(row, "leftDate").is_none())
        .and_then(|row| text_of(&row, "roleEnumId")))
}

/// Whether a current member of the room is external ([`crate::user::is_external_user`]).
fn external_members(facade: &EntityFacade, room_id: &str) -> Result<bool, Error> {
    let members = facade
        .find(MEMBER)?
        .eq("chatRoomId", Value::string(room_id))
        .disable_authz()
        .list()?;
    let external = members
        .iter()
        .filter(|row| text_of(row, "leftDate").is_none())
        .filter_map(|row| text_of(row, "userId"))
        .any(|user_id| crate::user::is_external_user(&user_id));
    Ok(external)
}

/// Whether `user_id` is an agent: the chat or run-as user of an agent. A room's `CmrAgent` membership alone proves
/// nothing (only an agent's chat user may hold it, `add#Member` checks, but a row may predate that check).
fn is_agent_user(facade: &EntityFacade, user_id: &str) -> Result<bool, Error> {
    for field in ["chatUserId", "runAsUserId"] {
        if facade.find(AGENT)?.eq(field, Value::string(user_id)).disable_authz().count()? > 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The agent whose chat member `agent_user_id` is, when it is not disabled.
fn agent_of_chat_user(facade: &EntityFacade, agent_user_id: &str) -> Result<Option<String>, Error> {
    Ok(facade
        .find(AGENT)?
        .eq("chatUserId", Value::string(agent_user_id))
        .disable_authz()
        .list()?
        .values()
        .iter()
        .filter(|row| crate::llm::flag_of(row, "disabled") != Some(true))
        .find_map(|row| text_of(row, "agentId")))
}

fn display_name(facade: &EntityFacade, user_id: &str) -> String {
    facade
        .find("moqui.security.UserAccount")
        .and_then(|find| find.eq("userId", Value::string(user_id)).disable_authz().one())
        .ok()
        .flatten()
        .and_then(|row| text_of(&row, "userFullName").or_else(|| text_of(&row, "username")))
        .unwrap_or_else(|| user_id.to_string())
}

/// The name of the chat room of a conversation, for the chat addendum's `${roomName}`; `None` for one of no room.
pub fn room_name(facade: &EntityFacade, conversation_id: &str) -> Option<String> {
    let room_id = quietly(|| {
        facade.find(run::CONVERSATION).and_then(|find| {
            find.eq("conversationId", Value::string(conversation_id))
                .disable_authz()
                .one()
        })
    })
    .ok()
    .flatten()
    .and_then(|row| text_of(&row, "chatRoomId"))?;
    let room = quietly(|| {
        facade
            .find(ROOM)
            .and_then(|find| find.eq("chatRoomId", Value::string(&room_id)).disable_authz().one())
    })
    .ok()
    .flatten()?;
    Some(text_of(&room, "roomName").unwrap_or_else(|| "a direct conversation".to_string()))
}

/// The earlier messages of a thread (its root and the replies before `before`), oldest first, as untrusted text with
/// each sender's name; the agent's own are left out. Empty when there are none.
fn thread_context(
    facade: &EntityFacade,
    room_id: &str,
    root: i64,
    before: i64,
    agent_user_id: &str,
) -> Result<String, Error> {
    let rows = facade
        .find(EVENT)?
        .eq("chatRoomId", Value::string(room_id))
        .condition(moqui_db::sql::Condition::or(vec![
            moqui_db::sql::Condition::equals("eventSeq", Value::I64(root)),
            moqui_db::sql::Condition::equals("threadRootEventSeq", Value::I64(root)),
        ]))
        .condition(moqui_db::sql::Condition::less("eventSeq", Value::I64(before)))
        .eq("eventTypeEnumId", Value::string("CetMessage"))
        .order_by("eventSeq", true)
        .limit(i64::try_from(THREAD_MESSAGES).unwrap_or(10))
        .disable_authz()
        .list()?;
    let mut lines: Vec<String> = Vec::new();
    for row in rows.values().iter().rev() {
        let sender = text_of(row, "senderUserId").unwrap_or_default();
        if sender == agent_user_id || text_of(row, "redactedDate").is_some() {
            continue;
        }
        let text = text_of(row, "bodyText").unwrap_or_default();
        if text.is_empty() {
            continue;
        }
        lines.push(format!("{}: {}", display_name(facade, &sender), text));
    }
    let mut joined = lines.join("\n");
    if joined.chars().count() > THREAD_CHARS {
        let skip = joined.chars().count() - THREAD_CHARS;
        joined = format!("…{}", joined.chars().skip(skip).collect::<String>());
    }
    Ok(joined)
}

/// The conversation of `(room, thread root, sender)` with the agent, if there is one.
fn conversation_of(
    facade: &EntityFacade,
    room_id: &str,
    thread_root: Option<i64>,
    sender: &str,
    agent_id: &str,
) -> Result<Option<String>, Error> {
    let mut find = facade
        .find(run::CONVERSATION)?
        .eq("chatRoomId", Value::string(room_id))
        .eq("userId", Value::string(sender))
        .eq("agentId", Value::string(agent_id));
    find = match thread_root {
        Some(root) => find.eq("threadRootEventSeq", Value::I64(root)),
        None => find.condition(moqui_db::sql::Condition::is_null("threadRootEventSeq")),
    };
    Ok(find
        .order_by("createdDate", true)
        .limit(1)
        .disable_authz()
        .list()?
        .values()
        .first()
        .and_then(|row| text_of(row, "conversationId")))
}

/// Escape text for the safe HTML of a chat body: paragraphs at blank lines, line breaks kept.
pub(crate) fn html_of(text: &str) -> String {
    let escape = |line: &str| {
        line.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    text.trim()
        .split("\n\n")
        .filter(|paragraph| !paragraph.trim().is_empty())
        .map(|paragraph| {
            let lines: Vec<String> = paragraph.trim().lines().map(escape).collect();
            format!("<p>{}</p>", lines.join("<br>"))
        })
        .collect::<Vec<_>>()
        .join("")
}

/// Post a plain reply of the agent in a thread (failure tolerant: logged).
fn post_reply(room_id: &str, agent_user_id: &str, thread_root: Option<i64>, text: &str, assist: Option<Json>) {
    let mut parameters = json!({"chatRoomId": room_id, "senderUserId": agent_user_id, "body": html_of(text),
        "threadRootEventSeq": thread_root});
    let verb = match assist {
        Some(assist) => {
            parameters["assist"] = assist;
            parameters["bodyText"] = json!(text);
            "send#AssistCard"
        }
        None => "send#Message",
    };
    if let Err(error) = quietly(|| chat_call(verb, parameters)) {
        tracing::error!("agent {agent_user_id} could not post in chat room {room_id}: {error}");
    }
}

/// `handle#ChatMessage`: the agent member `agentUserId` got message `eventSeq` of room `chatRoomId`: a run of the
/// agent as the message's sender when the message is for it. Out: `runId` and `conversationId` of the run it started
/// (or gave the message to), nothing when the message was not for the agent.
fn handle_chat_message(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let (Some(room_id), Some(agent_user_id), Some(factory)) = (
        param(parameters, "chatRoomId"),
        param(parameters, "agentUserId"),
        factory(),
    ) else {
        add_error("handle#ChatMessage needs chatRoomId and agentUserId");
        return Ok(BTreeMap::new());
    };
    let Some(seq) = param(parameters, "eventSeq").and_then(|text| text.parse::<i64>().ok()) else {
        add_error("handle#ChatMessage needs the eventSeq of the message");
        return Ok(BTreeMap::new());
    };
    if !crate::llm::facade(&factory).enabled() {
        return Ok(BTreeMap::new());
    }
    let prepared = quietly(|| prepare(facade, &room_id, seq, &agent_user_id))?;
    let Some(prepared) = prepared else {
        return Ok(BTreeMap::new());
    };
    // the run is the sender's: they must be allowed to use the agents
    let may_use = crate::user::user_has_permission(&prepared.sender, "AgentUse").unwrap_or(false);
    if !may_use {
        post_reply(
            &room_id,
            &agent_user_id,
            prepared.thread_root,
            &format!(
                "{} may not use the agents here (the AgentUse permission), so I did not act.",
                prepared.sender_name
            ),
            None,
        );
        return Ok(BTreeMap::new());
    }
    // nor does an agent act in a room an external user (EXTERNAL_USER: a client contact) is in: its replies, cards and
    // the data its tools read with the sender's authority would reach them
    let outsiders = match quietly(|| external_members(facade, &room_id)) {
        Ok(outsiders) => outsiders,
        Err(error) => {
            tracing::error!(
                "agent {agent_user_id} did not answer in room {room_id}: its members could not be read: {error}"
            );
            return Ok(BTreeMap::new());
        }
    };
    if outsiders {
        post_reply(
            &room_id,
            &agent_user_id,
            prepared.thread_root,
            "This room has members from outside the organization, so I do not act here.",
            None,
        );
        return Ok(BTreeMap::new());
    }
    let request = run::StartRequest {
        user_id: prepared.sender.clone(),
        agent_id: prepared.agent_id.clone(),
        conversation_id: prepared.conversation_id.clone(),
        surface: Surface::Chat,
        text: prepared.text.clone(),
        client_request_id: Some(format!("chat:{room_id}:{seq}:{agent_user_id}")),
        client_message_id: Some(format!("chat:{room_id}:{seq}:{agent_user_id}")),
        #[cfg(test)]
        extra_caps: test_caps(&prepared.agent_id),
        defer_submit: true,
        ..run::StartRequest::default()
    };
    let started = match run::start_run(&factory, &request) {
        Ok(started) => started,
        Err(error) => {
            tracing::error!(
                "agent {} could not answer chat message {room_id}::{seq}: {error}",
                prepared.agent_id
            );
            post_reply(
                &room_id,
                &agent_user_id,
                prepared.thread_root,
                &format!(
                    "I could not start on that: {}",
                    crate::llm::tools::format::cut(&error.to_string(), 300)
                ),
                None,
            );
            return Ok(BTreeMap::new());
        }
    };
    if prepared.conversation_id.is_none() && !started.existing && !started.queued {
        let attached = quietly(|| {
            run::in_tx(facade, || {
                let Some(mut row) = facade
                    .find(run::CONVERSATION)?
                    .eq("conversationId", Value::string(&started.conversation_id))
                    .for_update(true)
                    .disable_authz()
                    .one()?
                else {
                    return Ok(());
                };
                row.set("chatRoomId", Value::string(&room_id));
                row.set(
                    "threadRootEventSeq",
                    prepared.thread_root.map_or(Value::Null, Value::I64),
                );
                if text_of(&row, "title").is_none() {
                    row.set(
                        "title",
                        Value::string(crate::llm::tools::format::cut(&prepared.body_text, 80)),
                    );
                }
                row.update(facade)
            })
        });
        if let Err(error) = attached {
            tracing::error!(
                "the conversation {} could not be tied to chat room {room_id}: {error}",
                started.conversation_id
            );
        }
    }
    run::submit_started(&factory, &started)?;
    Ok(out([
        ("runId", Value::string(&started.run_id)),
        ("conversationId", Value::string(&started.conversation_id)),
    ]))
}

struct Prepared {
    sender: String,
    sender_name: String,
    agent_id: String,
    conversation_id: Option<String>,
    thread_root: Option<i64>,
    text: String,
    body_text: String,
}

/// Whether the message is for the agent, and what its run is: `None` when it is not (no mention in a group room and no
/// conversation of the sender in its thread, a message of an agent, a member that is no agent).
fn prepare(facade: &EntityFacade, room_id: &str, seq: i64, agent_user_id: &str) -> Result<Option<Prepared>, Error> {
    let Some(message) = room_message(facade, room_id, seq)? else {
        return Ok(None);
    };
    if message.sender.is_empty() || message.sender == agent_user_id {
        return Ok(None);
    }
    if member_role(facade, room_id, agent_user_id)?.as_deref() != Some("CmrAgent") {
        tracing::info!("chat user {agent_user_id} is no agent member of room {room_id}, so it does not answer");
        return Ok(None);
    }
    // agents never start each other's runs
    if member_role(facade, room_id, &message.sender)?.as_deref() == Some("CmrAgent")
        || is_agent_user(facade, &message.sender)?
    {
        return Ok(None);
    }
    let Some(agent_id) = agent_of_chat_user(facade, agent_user_id)? else {
        tracing::info!(
            "no agent has chat user {agent_user_id} (AgentDefinition.chatUserId), so room {room_id} gets no answer"
        );
        return Ok(None);
    };
    let room = facade
        .find(ROOM)?
        .eq("chatRoomId", Value::string(room_id))
        .disable_authz()
        .one()?;
    let direct = room
        .as_ref()
        .and_then(|room| text_of(room, "roomTypeEnumId"))
        .as_deref()
        == Some("CrtDirect");
    // the reply's thread: a direct room's timeline, else the message's thread, or a new one on the message
    let thread_root = match (direct, message.thread_root) {
        (true, root) => root,
        (false, Some(root)) => Some(root),
        (false, None) => Some(seq),
    };
    let mentioned = message
        .content
        .get("mentions")
        .and_then(Json::as_array)
        .is_some_and(|mentions| mentions.iter().any(|id| id.as_str() == Some(agent_user_id)));
    let conversation_id = conversation_of(facade, room_id, thread_root, &message.sender, &agent_id)?;
    if !mentioned && !direct && conversation_id.is_none() {
        return Ok(None);
    }
    if !answerable(facade, room_id, seq, &message, agent_user_id, conversation_id.as_deref())? {
        tracing::info!("chat message {room_id}::{seq} is older than what agent {agent_id} may answer now, so it is left");
        return Ok(None);
    }
    let mut text = message.body_text.clone();
    if text.trim().is_empty() {
        text = "(a message with no text)".to_string();
    }
    // the first message of a conversation in a thread brings the thread so far, as untrusted text
    if conversation_id.is_none() {
        if let Some(root) = message.thread_root {
            let context = thread_context(facade, room_id, root, seq, agent_user_id)?;
            if !context.is_empty() {
                text = format!(
                    "Earlier in this thread:\n{}\n\n{text}",
                    crate::llm::prompt::untrusted("chat", &context)
                );
            }
        }
    }
    Ok(Some(Prepared {
        sender_name: display_name(facade, &message.sender),
        sender: message.sender,
        agent_id,
        conversation_id,
        thread_root,
        text,
        body_text: message.body_text,
    }))
}

// ---- the room side of a run's events ----

/// Where a chat run's output goes.
#[derive(Clone, Debug)]
struct Room {
    room_id: String,
    thread_root: Option<i64>,
    agent_user_id: String,
}

fn room_of(facade: &EntityFacade, run: &RunRow) -> Result<Option<Room>, Error> {
    if run.surface != Surface::Chat {
        return Ok(None);
    }
    let Some(conversation) = facade
        .find(run::CONVERSATION)?
        .eq("conversationId", Value::string(&run.conversation_id))
        .disable_authz()
        .one()?
    else {
        return Ok(None);
    };
    let Some(room_id) = text_of(&conversation, "chatRoomId") else {
        return Ok(None);
    };
    let agent_user_id = facade
        .find(AGENT)?
        .eq("agentId", Value::string(&run.agent_id))
        .disable_authz()
        .one()?
        .and_then(|row| text_of(&row, "chatUserId"));
    let Some(agent_user_id) = agent_user_id else {
        return Ok(None);
    };
    Ok(Some(Room {
        room_id,
        thread_root: int_of(&conversation, "threadRootEventSeq"),
        agent_user_id,
    }))
}

/// The card message of an approval or a question (`content.assist.itemId`), the newest first.
fn card_of(facade: &EntityFacade, room: &Room, item_id: &str) -> Result<Option<i64>, Error> {
    Ok(facade
        .find(EVENT)?
        .eq("chatRoomId", Value::string(&room.room_id))
        .eq("senderUserId", Value::string(&room.agent_user_id))
        .condition(moqui_db::sql::Condition::like(
            "contentJson",
            Value::string(format!("%\"itemId\":\"{item_id}\"%")),
        ))
        .order_by("eventSeq", true)
        .limit(1)
        .disable_authz()
        .list()?
        .values()
        .first()
        .and_then(|row| int_of(row, "eventSeq")))
}

/// The runs whose agent shows as typing, by run.
#[derive(Default)]
struct Typing {
    runs: Mutex<HashMap<String, TypingRun>>,
}

struct TypingRun {
    room_id: String,
    agent_user_id: String,
    status: String,
    sent: Option<Instant>,
}

fn typing(factory: &ExecutionContextFactory) -> Arc<Typing> {
    factory.extension(|_| Typing::default())
}

fn signal_typing(room_id: &str, agent_user_id: &str, on: bool, status: &str) {
    let mut parameters = json!({"chatRoomId": room_id, "userId": agent_user_id, "typing": on});
    if on && !status.is_empty() {
        parameters["statusText"] = json!(status);
    }
    if let Err(error) = quietly(|| chat_call("signal#Typing", parameters)) {
        tracing::warn!("the typing status of agent {agent_user_id} in room {room_id} was not sent: {error}");
    }
}

fn start_typing(factory: &ExecutionContextFactory, run_id: &str, room: &Room, status: &str) {
    let typing = typing(factory);
    let mut runs = moqui_lib::sync::lock(&typing.runs);
    let entry = runs.entry(run_id.to_string()).or_insert_with(|| TypingRun {
        room_id: room.room_id.clone(),
        agent_user_id: room.agent_user_id.clone(),
        status: String::new(),
        sent: None,
    });
    if !status.is_empty() && entry.status != status {
        entry.status = status.to_string();
        // a new status goes at once
        entry.sent = None;
    }
}

fn stop_typing(factory: &ExecutionContextFactory, run_id: &str) {
    let stopped = moqui_lib::sync::lock(&typing(factory).runs).remove(run_id);
    if let Some(stopped) = stopped {
        if stopped.sent.is_some() {
            signal_typing(&stopped.room_id, &stopped.agent_user_id, false, "");
        }
    }
}

/// The typing statuses due at the observer worker's tick: each working run's that has not been sent for 3 seconds, as
/// `(room, agent user, status)`, marked sent.
pub(crate) fn typing_due(factory: &ExecutionContextFactory) -> Vec<(String, String, String)> {
    let typing = typing(factory);
    let mut runs = moqui_lib::sync::lock(&typing.runs);
    runs.values_mut()
        .filter(|run| run.sent.is_none_or(|sent| sent.elapsed() >= TYPING_EVERY))
        .map(|run| {
            run.sent = Some(Instant::now());
            (run.room_id.clone(), run.agent_user_id.clone(), run.status.clone())
        })
        .collect()
}

/// Send the typing statuses [`typing_due`] gave, in the execution context in place.
pub(crate) fn send_typing(due: &[(String, String, String)]) {
    for (room_id, agent_user_id, status) in due {
        signal_typing(room_id, agent_user_id, true, status);
    }
}

/// What a tool call shows as the agent's status.
fn status_of(tool: &str) -> String {
    match tool {
        "search" => "Searching…".to_string(),
        "screen" => "Looking at a screen…".to_string(),
        "data" => "Reading data…".to_string(),
        "skill" => "Reading a guide…".to_string(),
        "" => "Working…".to_string(),
        other => format!("Using {other}…"),
    }
}

/// The room side of a chat run's committed events: typing, the cards and the answer.
pub(crate) fn observe(factory: &Arc<ExecutionContextFactory>, batch: &RunEvents) {
    let Some(facade) = factory.entity() else {
        return;
    };
    let run = match run::load_run(&facade, &batch.run_id) {
        Ok(Some(run)) if run.surface == Surface::Chat => run,
        Ok(_) => return,
        Err(error) => {
            tracing::error!("chat: agent run {} could not be read: {error}", batch.run_id);
            return;
        }
    };
    let room = match room_of(&facade, &run) {
        Ok(Some(room)) => room,
        Ok(None) => return,
        Err(error) => {
            tracing::error!("chat: the room of agent run {} could not be read: {error}", run.run_id);
            return;
        }
    };
    for (_, event_type, payload) in &batch.events {
        let handled = match event_type.as_str() {
            "run.state" => {
                match payload
                    .get("state")
                    .and_then(Json::as_str)
                    .and_then(RunState::from_wire)
                {
                    Some(RunState::Working) => start_typing(factory, &run.run_id, &room, &status_of("")),
                    Some(state) if state.is_paused() || state.is_terminal() => stop_typing(factory, &run.run_id),
                    _ => {}
                }
                Ok(())
            }
            "tool.call" => {
                let tool = payload
                    .get("name")
                    .or_else(|| payload.get("tool"))
                    .and_then(Json::as_str)
                    .unwrap_or_default();
                start_typing(factory, &run.run_id, &room, &status_of(tool));
                Ok(())
            }
            "approval.requested" => post_approval(&facade, &run, &room, payload),
            "approval.decided" => update_approval(&facade, &room, payload),
            "question.asked" => post_question(&facade, &run, &room, payload),
            "run.done" => {
                stop_typing(factory, &run.run_id);
                settle_cards(&facade, &run, &room);
                post_answer(&facade, &run, &room, payload);
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(error) = handled {
            tracing::error!(
                "chat: the {event_type} of agent run {} did not reach room {}: {error}",
                run.run_id,
                room.room_id
            );
        }
    }
}

fn post_approval(facade: &EntityFacade, run: &RunRow, room: &Room, card: &Json) -> Result<(), Error> {
    let approval_id = card.get("approvalId").and_then(Json::as_str).unwrap_or_default();
    if approval_id.is_empty() || card_of(facade, room, approval_id)?.is_some() {
        return Ok(());
    }
    let summary = card
        .get("summary")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string();
    let requester = display_name(facade, &run.user_id);
    let assist = json!({"kind": "approval", "runId": run.run_id, "conversationId": run.conversation_id,
        "itemId": approval_id, "requesterUserId": run.user_id, "requesterName": requester,
        "title": "Approval needed", "summary": summary, "rows": card.get("changes").cloned().unwrap_or(json!([])),
        "classification": card.get("classification"), "risk": card.get("risk"),
        "expiresAt": card.get("expiresAt"), "state": "pending"});
    let text = format!("Approval needed for {requester}: {summary}");
    chat_call(
        "send#AssistCard",
        json!({"chatRoomId": room.room_id, "senderUserId": room.agent_user_id, "body": html_of(&text), "bodyText": text,
            "threadRootEventSeq": room.thread_root, "assist": assist}),
    )
    .map(|_| ())
}

fn update_approval(facade: &EntityFacade, room: &Room, payload: &Json) -> Result<(), Error> {
    let approval_id = payload.get("approvalId").and_then(Json::as_str).unwrap_or_default();
    let Some(seq) = card_of(facade, room, approval_id)? else {
        return Ok(());
    };
    let view: Option<ApprovalView> = approval::load(facade, approval_id)?;
    let decided_by = view
        .as_ref()
        .and_then(|view| view.decision_user_id.clone())
        .map(|user_id| json!({"userId": user_id, "name": display_name(facade, &user_id)}));
    let status = payload.get("status").and_then(Json::as_str).unwrap_or_default();
    chat_call(
        "update#AssistCard",
        json!({"chatRoomId": room.room_id, "eventSeq": seq, "fields": {"state": status,
            "decision": payload.get("decision"), "channel": payload.get("channel"), "decidedBy": decided_by,
            "feedback": payload.get("feedback")}}),
    )
    .map(|_| ())
}

fn post_question(facade: &EntityFacade, run: &RunRow, room: &Room, payload: &Json) -> Result<(), Error> {
    let question_id = payload.get("questionId").and_then(Json::as_str).unwrap_or_default();
    if question_id.is_empty() || card_of(facade, room, question_id)?.is_some() {
        return Ok(());
    }
    let text = payload
        .get("text")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string();
    let requester = display_name(facade, &run.user_id);
    let assist = json!({"kind": "question", "runId": run.run_id, "conversationId": run.conversation_id,
        "itemId": question_id, "requesterUserId": run.user_id, "requesterName": requester, "title": "Question",
        "summary": text, "options": payload.get("options").cloned().unwrap_or(json!([])),
        "multi": payload.get("multi"), "hasFields": payload.get("fields").and_then(Json::as_array).is_some_and(|fields| !fields.is_empty()),
        "state": "pending"});
    chat_call(
        "send#AssistCard",
        json!({"chatRoomId": room.room_id, "senderUserId": room.agent_user_id, "body": html_of(&format!("{requester}: {text}")),
            "bodyText": text, "threadRootEventSeq": room.thread_root, "assist": assist}),
    )
    .map(|_| ())
}

/// The run ended: each approval card of it shows how its approval ended (executed, failed, expired, …).
fn settle_cards(facade: &EntityFacade, run: &RunRow, room: &Room) {
    let approvals = facade
        .find("moqui.agent.AgentApproval")
        .and_then(|find| find.eq("runId", Value::string(&run.run_id)).disable_authz().list());
    let approvals = match approvals {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!(
                "chat: the approvals of agent run {} could not be read: {error}",
                run.run_id
            );
            return;
        }
    };
    for row in approvals.values() {
        let view = ApprovalView::of(row);
        let updated = card_of(facade, room, &view.approval_id).and_then(|seq| match seq {
            Some(seq) => chat_call(
                "update#AssistCard",
                json!({"chatRoomId": room.room_id, "eventSeq": seq, "fields": {"state": view.status.as_str()}}),
            )
            .map(|_| ()),
            None => Ok(()),
        });
        if let Err(error) = updated {
            tracing::warn!(
                "chat: the card of approval {} was not updated: {error}",
                view.approval_id
            );
        }
    }
}

/// The run's answer in the thread, or why it ended without one.
fn post_answer(facade: &EntityFacade, run: &RunRow, room: &Room, payload: &Json) {
    let state = payload
        .get("state")
        .and_then(Json::as_str)
        .and_then(RunState::from_wire);
    let text = match state {
        Some(RunState::Completed) => payload
            .get("result")
            .and_then(|result| result.get("text"))
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string(),
        Some(RunState::Canceled) => "I stopped.".to_string(),
        _ => {
            let why = quietly(|| {
                facade
                    .find(run::RUN)
                    .and_then(|find| find.eq("runId", Value::string(&run.run_id)).disable_authz().one())
            })
            .ok()
            .flatten()
            .and_then(|row| text_of(&row, "errorText").or_else(|| text_of(&row, "statusReason")))
            .unwrap_or_else(|| "the run failed".to_string());
            format!("I could not finish that: {}", crate::llm::tools::format::cut(&why, 300))
        }
    };
    if text.trim().is_empty() {
        return;
    }
    let assist = json!({"kind": "reply", "runId": run.run_id, "conversationId": run.conversation_id,
        "requesterUserId": run.user_id, "state": state.map_or("done", RunState::wire)});
    post_reply(
        &room.room_id,
        &room.agent_user_id,
        room.thread_root,
        &text,
        Some(assist),
    );
}

// ---- decisions from the room ----

/// The approval `approval_id` when `user_id` may decide it from its chat room (answered question 12): the approval's
/// run is a chat run, the user may post in its room, and the user is no agent (no agent member of the room, nor the
/// chat or run-as user of any agent) and not external (`EXTERNAL_USER`). `None` otherwise, so the caller answers as for
/// none.
///
/// # Errors
///
/// Returns an error when the rows cannot be read.
pub(crate) fn room_approval(
    facade: &EntityFacade,
    approval_id: &str,
    user_id: &str,
) -> Result<Option<ApprovalView>, Error> {
    let Some(view) = approval::load(facade, approval_id)? else {
        return Ok(None);
    };
    let Some(run) = run::load_run(facade, &view.run_id)? else {
        return Ok(None);
    };
    let Some(room) = room_of(facade, &run)? else {
        return Ok(None);
    };
    if is_agent_user(facade, user_id)? || member_role(facade, &room.room_id, user_id)?.as_deref() == Some("CmrAgent") {
        return Ok(None);
    }
    // an external user (EXTERNAL_USER: a client contact) decides no agent's approval, as they use no agent
    if crate::user::is_external_user(user_id) {
        return Ok(None);
    }
    let access = chat_call("get#RoomAccess", json!({"chatRoomId": room.room_id, "userId": user_id}))?;
    let can_post = access.get("canPost").map(to_plain_string).as_deref() == Some("true");
    Ok(can_post.then_some(view))
}

/// `decide#ChatCard`: the current user's press of a button on an agent's card in a room (`content.assist`): approve or
/// reject an approval (any human who may post in the room, answered question 12), or answer a question (the requester
/// only). Only from an interactive browser session, as every decision. Out: `status`.
fn decide_chat_card(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let (Some(room_id), Some(factory)) = (param(parameters, "chatRoomId"), factory()) else {
        add_error("decide#ChatCard needs chatRoomId");
        return Ok(BTreeMap::new());
    };
    let Some(seq) = param(parameters, "eventSeq").and_then(|text| text.parse::<i64>().ok()) else {
        add_error("decide#ChatCard needs the eventSeq of the card");
        return Ok(BTreeMap::new());
    };
    let decider = match Decider::from_request(Some(Channel::Chat)) {
        Ok(decider) => decider,
        Err(why) => {
            add_error(why);
            return Ok(BTreeMap::new());
        }
    };
    let card = quietly(|| room_message(facade, &room_id, seq))?;
    let Some(assist) = card.as_ref().and_then(|card| card.content.get("assist").cloned()) else {
        add_error(format!("there is no agent card {seq} in the room"));
        return Ok(BTreeMap::new());
    };
    let item_id = assist
        .get("itemId")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string();
    let decision = param(parameters, "decision").unwrap_or_default();
    match assist.get("kind").and_then(Json::as_str) {
        Some("approval") => {
            let decision = match decision.as_str() {
                "approve" => Decision::Approve { arguments: None },
                "reject" => Decision::Reject {
                    feedback: param(parameters, "feedback").unwrap_or_default(),
                },
                other => {
                    add_error(format!(
                        "{other} is not a decision of a room card; it is approve or reject"
                    ));
                    return Ok(BTreeMap::new());
                }
            };
            // the card must be the approval's own: of this room
            let view = quietly(|| approval::load(facade, &item_id))?;
            let of_room = match &view {
                Some(view) => quietly(|| {
                    run::load_run(facade, &view.run_id)
                        .and_then(|run| run.map_or(Ok(None), |run| room_of(facade, &run)))
                })?
                .is_some_and(|room| room.room_id == room_id),
                None => false,
            };
            if !of_room {
                add_error("there is no such approval");
                return Ok(BTreeMap::new());
            }
            match approval::decide(&factory, &item_id, &decider, &decision) {
                Ok(decided) => Ok(out([("status", Value::string(decided.status.as_str()))])),
                Err(error) => {
                    add_error(error.to_string());
                    Ok(BTreeMap::new())
                }
            }
        }
        Some("question") => {
            let run_id = assist
                .get("runId")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string();
            let requester = assist.get("requesterUserId").and_then(Json::as_str).unwrap_or_default();
            if requester != decider.user_id() {
                add_error("only the person who asked the agent answers its question");
                return Ok(BTreeMap::new());
            }
            let mut data = serde_json::Map::new();
            if let Some(choice) = param(parameters, "choice") {
                data.insert("choice".to_string(), json!(choice));
            }
            let text = param(parameters, "text").unwrap_or_default();
            if data.is_empty() && text.is_empty() {
                add_error("an answer needs a choice or text");
                return Ok(BTreeMap::new());
            }
            let data = (!data.is_empty()).then_some(Json::Object(data));
            match run::submit_input(&factory, &run_id, decider.user_id(), "answer", &text, data.as_ref()) {
                Ok(state) => {
                    let answered = quietly(|| {
                        chat_call(
                            "update#AssistCard",
                            json!({"chatRoomId": room_id, "eventSeq": seq, "fields": {"state": "answered",
                                "answer": param(parameters, "choice").unwrap_or(text.clone())}}),
                        )
                    });
                    if let Err(error) = answered {
                        tracing::warn!("the question card {room_id}::{seq} was not marked answered: {error}");
                    }
                    Ok(out([("status", Value::string(state.wire()))]))
                }
                Err(error) => {
                    add_error(error.to_string());
                    Ok(BTreeMap::new())
                }
            }
        }
        _ => {
            add_error("this card takes no decision");
            Ok(BTreeMap::new())
        }
    }
}
