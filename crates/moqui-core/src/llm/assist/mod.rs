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

//! The drawer's JSON calls (plans/LLM-A2A-Assist.md 4.5 and 4.11; the contract is docs/Assist.md): the transitions of
//! the webroot subscreen `assist` (`runtime/base-component/webroot/screen/webroot/assist.xml`) call the one service
//! `org.moqui.agent.AssistServices.call#Drawer` with the call's name, which reads the request's parameters and answers
//! the response itself: 200 with the call's JSON, or the error status with `{errorCode, errors, code}`.
//!
//! Every call acts for the current user only, who needs `AgentUse`: a conversation, run, approval, question, hand-off
//! or message of anyone else is not found (404), never named. A call from inside an agent's run is refused, so nobody,
//! the agent included, drives a conversation that is not theirs. A turn that starts a run counts one hit of the AT_LLM
//! tarpit (`agent:<agentId>`), a message handed to the active run none ([`crate::llm::run::start_run`]). A decision goes
//! through [`crate::llm::approval::Decider::from_request`]: a browser session by its cookie with the session token (the
//! transitions are POSTs, whose token the render checks), never a header login.
//!
//! The calls: [`read`] answers bootstrap, conversations, conversation, tasks, prompts and mentions; [`write`] turn,
//! cancel, retry, follow, context, submitted, decide, preview, answer and feedback; `canvas` the canvas calls
//! (canvasState, canvasAction, canvasAdjust, canvasProblems) and the read proxy (`read`); `extras` the calls of phase
//! 13.14 (queued, transcribe, textAction, memories, memory, skills, skill, events) and [`files`] the composer's files
//! (attachments, attachment). [`state`] reads the page
//! state a turn carries; [`prompts`] is the suggested prompts' registration point; [`audit`] has the admin services of
//! System › LLM › Conversations, Runs and Approvals.

pub mod audit;
mod canvas;
mod extras;
pub mod files;
pub mod prompts;
mod read;
pub mod state;
pub(crate) mod write;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_lib::error::Error;
use moqui_lib::object::{is_empty, to_plain_string, Value};
use serde_json::{json, Value as Json};

use crate::context::{ErrorKind, ExecutionContext, ExecutionContextFactory};
use crate::entity::{EntityFacade, EntityValue};
use crate::service::rust_services::{rust_service, RustServiceFn};

pub use state::{follow_stamp, DrawerState, FollowStamp};

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, call_drawer)?;
    audit::register(map)
}

/// The longest message a turn takes, in characters.
pub const MAX_MESSAGE_CHARS: usize = 16_000;
/// The longest client id and client message id.
const MAX_ID_CHARS: usize = 100;

/// Every call the drawer makes, in the order docs/Assist.md lists them.
pub const CALLS: &[&str] = &[
    "bootstrap",
    "conversations",
    "conversation",
    "turn",
    "cancel",
    "retry",
    "follow",
    "context",
    "submitted",
    "decide",
    "preview",
    "answer",
    "feedback",
    "tasks",
    "prompts",
    "mentions",
    "queued",
    "attachments",
    "attachment",
    "transcribe",
    "textAction",
    "memories",
    "memory",
    "skills",
    "skill",
    "events",
    "canvasState",
    "canvasAction",
    "canvasAdjust",
    "canvasProblems",
    "read",
];

/// Why a call did not answer: the HTTP status, a code a client can test and the text to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrawerError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
    /// Seconds to wait before trying again (the tarpit), sent as `Retry-After`.
    pub retry_after: Option<u64>,
}

impl DrawerError {
    pub(crate) fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            retry_after: None,
        }
    }

    /// 400: a parameter is missing or does not read.
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(400, "invalid", message)
    }

    /// 404: there is no such thing of this user's (another user's is not named).
    pub fn not_found(what: &str, id: &str) -> Self {
        Self::new(404, "not_found", format!("there is no {what} {id}"))
    }

    /// 403: the user may not do this here.
    pub fn refused(message: impl Into<String>) -> Self {
        Self::new(403, "refused", message)
    }

    /// 409: the thing is not in a state that takes this (decided already, the run ended, a run is active).
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(409, "conflict", message)
    }

    /// 501: the call belongs to a later phase.
    pub fn not_available(call: &str, phase: &str) -> Self {
        Self::new(501, "not_available", format!("{call} is not available yet (phase {phase})"))
    }

    /// The error of a failed operation: a tarpit is 429 with its seconds, an unavailable database or pool 503, an
    /// authorization failure 403, anything else 500.
    pub fn of(error: &Error) -> Self {
        match ErrorKind::of(error) {
            ErrorKind::Tarpit => Self {
                retry_after: retry_after_of(error),
                ..Self::new(429, "tarpit", error.message())
            },
            ErrorKind::Unavailable => Self::new(503, "unavailable", error.message()),
            ErrorKind::Unauthorized => Self::new(403, "refused", error.message()),
            _ => Self::new(500, "failed", error.message()),
        }
    }

    pub fn to_json(&self) -> Json {
        json!({"errorCode": self.status, "errors": self.message, "code": self.code})
    }
}

/// The seconds to wait of the first context error in `error`'s source chain that has them.
fn retry_after_of(error: &Error) -> Option<u64> {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(found) = current {
        if let Some(seconds) = found
            .downcast_ref::<crate::context::Error>()
            .and_then(crate::context::Error::retry_after)
        {
            return Some(seconds);
        }
        current = found.source();
    }
    None
}

impl From<Error> for DrawerError {
    fn from(error: Error) -> Self {
        Self::of(&error)
    }
}

/// What a call answers.
pub type Answer = Result<Json, DrawerError>;

/// A call's parameters: the request's, a JSON body's fields or a form's (where an object or a list is JSON text).
#[derive(Clone, Debug, Default)]
pub struct Params(BTreeMap<String, Value>);

impl Params {
    pub fn new(parameters: BTreeMap<String, Value>) -> Self {
        Self(parameters)
    }

    /// The parameters as a map, to make another call's from them.
    pub fn into_map(self) -> BTreeMap<String, Value> {
        self.0
    }

    /// From JSON: each field of an object.
    pub fn of_json(value: &Json) -> Self {
        match moqui_lib::json::from_json(value) {
            Value::Map(map) => Self(map.into_iter().collect()),
            _ => Self::default(),
        }
    }

    /// A scalar parameter, trimmed; empty is none.
    pub fn text(&self, name: &str) -> Option<String> {
        self.0
            .get(name)
            .filter(|value| !is_empty(value) && !matches!(value, Value::Map(_) | Value::List(_)))
            .map(to_plain_string)
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
    }

    /// [`Self::text`], else 400.
    ///
    /// # Errors
    ///
    /// When it is missing.
    pub fn required(&self, name: &str) -> Result<String, DrawerError> {
        self.text(name)
            .ok_or_else(|| DrawerError::invalid(format!("{name} is required")))
    }

    /// An id the client made (a client id, a client message id): letters, digits, `-`, `_`, `.` and `:`, at most
    /// 100 characters.
    ///
    /// # Errors
    ///
    /// When it is missing or is not such an id.
    pub fn client_id(&self, name: &str) -> Result<String, DrawerError> {
        let id = self.required(name)?;
        let fits = id.chars().count() <= MAX_ID_CHARS
            && id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':'));
        if fits {
            Ok(id)
        } else {
            Err(DrawerError::invalid(format!(
                "{name} is letters, digits, '-', '_', '.' and ':', at most {MAX_ID_CHARS} characters"
            )))
        }
    }

    /// A parameter as it came (an uploaded file a test passes, say).
    pub fn raw(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }

    /// A JSON parameter: an object or list as it came in a JSON body, or JSON text in a form field.
    pub fn json(&self, name: &str) -> Option<Json> {
        match self.0.get(name)? {
            Value::Null => None,
            value @ (Value::Map(_) | Value::List(_)) => Some(moqui_lib::json::to_json(value)),
            Value::String(text) => {
                let text = text.trim();
                if text.starts_with('{') || text.starts_with('[') {
                    serde_json::from_str(text).ok()
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// A whole number.
    pub fn int(&self, name: &str) -> Option<i64> {
        match self.0.get(name)? {
            Value::I64(number) => Some(*number),
            Value::I32(number) => Some(i64::from(*number)),
            other => to_plain_string(other).trim().parse().ok(),
        }
    }

    /// `true` or `false` (a boolean, or its text).
    pub fn flag(&self, name: &str) -> Option<bool> {
        match self.0.get(name)? {
            Value::Bool(flag) => Some(*flag),
            other => match to_plain_string(other).trim() {
                "true" | "Y" | "1" => Some(true),
                "false" | "N" | "0" => Some(false),
                _ => None,
            },
        }
    }
}

/// Who calls: the user (with `AgentUse`), the factory and its entity facade.
pub(crate) struct Caller {
    pub user_id: String,
    pub factory: Arc<ExecutionContextFactory>,
    pub facade: EntityFacade,
}

impl Caller {
    /// The current user of the request, checked: logged in (401), not inside an agent's run and with `AgentUse` (403).
    ///
    /// # Errors
    ///
    /// The refusal.
    pub(crate) fn current() -> Result<Self, DrawerError> {
        let factory = ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
            .ok_or_else(|| DrawerError::new(503, "unavailable", "there is no execution context"))?;
        if ExecutionContext::with_current(|ec| ec.agent_origin().is_some()).unwrap_or(false) {
            return Err(DrawerError::refused("an agent's run cannot use the drawer"));
        }
        let user_id = crate::user::current_user_id()
            .ok()
            .flatten()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| DrawerError::new(401, "unauthorized", "Authentication required"))?;
        if !crate::user::has_permission("AgentUse").unwrap_or(false) {
            return Err(DrawerError::refused("this needs the AgentUse permission"));
        }
        let facade = factory
            .entity()
            .ok_or_else(|| DrawerError::new(503, "unavailable", "the entity facade is not installed"))?;
        Ok(Self {
            user_id,
            factory,
            facade,
        })
    }

    /// The conversation `conversation_id` when it is this user's, else 404.
    pub(crate) fn conversation(&self, conversation_id: &str) -> Result<EntityValue, DrawerError> {
        crate::llm::run::quietly(|| {
            self.facade
                .find(crate::llm::run::CONVERSATION)?
                .eq("conversationId", Value::string(conversation_id))
                .disable_authz()
                .one()
        })?
        .filter(|row| crate::llm::text_of(row, "userId").as_deref() == Some(self.user_id.as_str()))
        .ok_or_else(|| DrawerError::not_found("conversation", conversation_id))
    }

    /// The run `run_id` when it is this user's, else 404.
    pub(crate) fn run(&self, run_id: &str) -> Result<crate::llm::run::RunRow, DrawerError> {
        crate::llm::run::quietly(|| crate::llm::run::load_run(&self.facade, run_id))?
            .filter(|run| run.user_id == self.user_id)
            .ok_or_else(|| DrawerError::not_found("run", run_id))
    }

    pub(crate) fn has_permission(&self, permission: &str) -> bool {
        crate::user::has_permission(permission).unwrap_or(false)
    }
}

/// Answer the drawer call `name` for the current user, with `params`.
pub fn call(name: &str, params: &Params) -> Answer {
    let caller = Caller::current()?;
    let llm = crate::llm::facade(&caller.factory);
    if !llm.enabled() && name != "bootstrap" {
        return Err(DrawerError::new(503, "disabled", "the agents are turned off on this server"));
    }
    match name {
        "bootstrap" => read::bootstrap(&caller),
        "conversations" => read::conversations(&caller, params),
        "conversation" => read::conversation(&caller, params),
        "tasks" => read::tasks(&caller),
        "prompts" => read::prompts(&caller, params),
        "mentions" => read::mentions(&caller, params),
        "turn" => write::turn(&caller, params),
        "cancel" => write::cancel(&caller, params),
        "retry" => write::retry(&caller, params),
        "follow" => write::follow(&caller, params),
        "context" => write::context(&caller, params),
        "submitted" => write::submitted(&caller, params),
        "decide" => write::decide(&caller, params),
        "preview" => write::preview(&caller, params),
        "answer" => write::answer(&caller, params),
        "feedback" => write::feedback(&caller, params),
        "queued" => extras::queued(&caller, params),
        "attachments" => files::attachments(&caller, params),
        "attachment" => files::download(&caller, params).map(|()| Json::Null),
        "transcribe" => extras::transcribe(&caller, params),
        "textAction" => extras::text_action(&caller, params),
        "memories" => extras::memories(&caller, params),
        "memory" => extras::memory_call(&caller, params),
        "skills" => extras::skills(&caller, params),
        "skill" => extras::skill(&caller, params),
        "events" => extras::events(&caller, params),
        "canvasState" => canvas::canvas_state(&caller, params),
        "canvasAction" => canvas::canvas_action(&caller, params),
        "canvasAdjust" => canvas::canvas_adjust(&caller, params),
        "canvasProblems" => canvas::canvas_problems(&caller, params),
        "read" => canvas::read(&caller, params),
        other => Err(DrawerError::new(404, "not_found", format!("there is no drawer call {other}"))),
    }
}

/// Send an answer as the response: 200 and its JSON, or the error's status with `{errorCode, errors, code}` (and
/// `Retry-After` for a tarpit). Never cached.
pub fn send(answer: &Answer) {
    let mut headers = vec![
        ("Content-Type".to_string(), "application/json".to_string()),
        ("Cache-Control".to_string(), "no-store".to_string()),
    ];
    let (status, body) = match answer {
        Ok(body) => (200, body.to_string()),
        Err(error) => {
            if let Some(seconds) = error.retry_after {
                headers.push(("Retry-After".to_string(), seconds.to_string()));
            }
            (error.status, error.to_json().to_string())
        }
    };
    crate::web::send_bytes_with_headers(status, headers, body.into_bytes());
}

/// `call#Drawer`: answer the drawer call `call` with the request's parameters, as the response. It adds no message
/// and no error, so the transition sends nothing else.
fn call_drawer(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Result<BTreeMap<String, Value>, Error> {
    let name = parameters.get("call").map(to_plain_string).unwrap_or_default();
    let answer = call(name.trim(), &Params::new(crate::web::parameters()));
    // a download sent its file itself
    if name.trim() == "attachment" && answer.is_ok() {
        return Ok(BTreeMap::new());
    }
    if let Err(error) = &answer {
        if error.status >= 500 && error.status != 501 && error.status != 503 {
            tracing::error!("the drawer call {name} failed: {}", error.message);
        }
    }
    send(&answer);
    Ok(BTreeMap::new())
}
