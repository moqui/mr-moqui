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

//! Write interception for agent renders (plans/LLM-A2A-Assist.md 4.7 and 4.8): where a render came from
//! ([`RenderOrigin`]), the gate a transition that writes meets ([`TransitionGate`]), and the observer that sees each
//! transition's outcome ([`TransitionObserver`]). Always compiled: the render cannot depend on the `llm` feature.
//!
//! These are in-process options of [`ScreenRender`](super::ScreenRender) ([`RenderOptions`]), which the in-process
//! transport of the terminal client passes on; an HTTP request cannot set them. A render with no gate runs as before.
//! Under [`TransitionGate::Hold`] a transition that is not read-only and runs actions answers 409 with
//! `{"heldWrite": {...}}` ([`HeldWrite`]) and runs nothing, not even the always-actions of its path. Under
//! [`TransitionGate::Approved`] it runs only when the hash of the request ([`held_write_hash`]) is the approved one, and
//! then outside the render's read fence. The hash covers everything that reaches the transition: the resolved path, the
//! extra path its `path-parameter`s bind, the inline `/~name=value/` and path-query parameters, and the request
//! parameters. A render with a gate runs everything else under the read fence, whether or not it asked for one.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::InstallSlot;
use serde_json::{json, Map, Value as Json};

/// Where a render came from. `decide#AgentApproval` takes a decision only from an interactive origin (a browser, the
/// terminal), never from an agent's own screen session, `agent-apps` or internal code.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RenderOrigin {
    /// Code in this process: a job, a test, a service rendering a screen.
    #[default]
    Internal,
    /// An HTTP request from a browser or another client.
    Browser,
    /// The terminal client of a person (SSH, the browser terminal, `moqui tui`).
    Terminal,
    /// An agent's own screen session.
    Agent { agent_id: String, run_id: Option<String> },
    /// `agent-apps` over SSH: a program driving the apps with its own credentials.
    AgentApps,
}

impl RenderOrigin {
    pub fn as_str(&self) -> &'static str {
        match self {
            RenderOrigin::Internal => "internal",
            RenderOrigin::Browser => "browser",
            RenderOrigin::Terminal => "terminal",
            RenderOrigin::Agent { .. } => "agent",
            RenderOrigin::AgentApps => "agent-apps",
        }
    }

    /// A person at a browser or a terminal.
    pub fn is_interactive(&self) -> bool {
        matches!(self, RenderOrigin::Browser | RenderOrigin::Terminal)
    }
}

/// What a transition that writes meets in this render.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum TransitionGate {
    /// No gate: the transition runs as for any render.
    #[default]
    Open,
    /// Hold it: answer 409 with the held write and run nothing.
    Hold,
    /// Run it only when the request hashes to `args_hash`; else answer 409 as for a hold, with the mismatch.
    Approved { approval_id: String, args_hash: String },
}

/// The in-process options of a render.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenderOptions {
    pub origin: RenderOrigin,
    pub gate: TransitionGate,
    /// Run the render under the read fence ([`crate::entity::fence`]); an approved transition runs outside it. With
    /// the gate open a transition runs fenced rather than held: an agent's lookups (a drop-down's options, a value, a
    /// dialog's body) render so, and one whose actions write fails at the fence.
    pub write_fence: bool,
}

/// A transition the gate held: what the approval card shows and what its hash covers.
#[derive(Clone, Debug, PartialEq)]
pub struct HeldWrite {
    pub screen_location: String,
    pub transition: String,
    /// The transition's single service, when it has one.
    pub service: Option<String>,
    pub authz_action: String,
    pub method: String,
    /// The resolved screen path of the request, as names joined by `/`.
    pub path: String,
    /// The path names after the transition, which its `path-parameter`s bind.
    pub extra_path: Vec<String>,
    /// The `path-parameter` names with the extra path values they get: shown, and covered through `extra_path`.
    pub path_parameters: Map<String, Json>,
    /// The inline `/~name=value/` segments and a path's own query; a request parameter of the same name wins over one.
    pub inline_parameters: Map<String, Json>,
    /// The request parameters as [`gate_parameters`] gives them.
    pub parameters: Map<String, Json>,
}

impl HeldWrite {
    /// [`held_write_hash`] of this write.
    pub fn hash(&self) -> String {
        held_write_hash(self)
    }

    /// The `heldWrite` object of the 409 answer.
    pub fn to_json(&self) -> Json {
        json!({
            "screenLocation": self.screen_location,
            "transition": self.transition,
            "service": self.service,
            "authzAction": self.authz_action,
            "method": self.method,
            "path": self.path,
            "extraPath": self.extra_path,
            "pathParameters": self.path_parameters,
            "inlineParameters": self.inline_parameters,
            "parameters": self.parameters,
            "argsHash": self.hash(),
        })
    }
}

/// The inline parameters as a held write shows them and its hash covers, the transport's own left out.
pub fn gate_inline_parameters(inline: &BTreeMap<String, String>) -> Map<String, Json> {
    inline
        .iter()
        .filter(|(name, _)| !TRANSPORT_PARAMETERS.contains(&name.as_str()))
        .map(|(name, value)| (name.clone(), Json::String(value.clone())))
        .collect()
}

/// Request parameters the transport adds, which are not part of what is approved.
const TRANSPORT_PARAMETERS: &[&str] = &["moquiSessionToken", "lastStandalone", "renderMode", "moquiFormName"];

/// The request parameters a held write shows and its hash covers: the transport's own left out, every value as its
/// text (a list as a list of texts), since a form posts text and the same values may come back typed; an uploaded
/// file as `[upload]`.
pub fn gate_parameters(parameters: &BTreeMap<String, Value>) -> Map<String, Json> {
    parameters
        .iter()
        .filter(|(name, _)| !TRANSPORT_PARAMETERS.contains(&name.as_str()))
        .map(|(name, value)| (name.clone(), gate_value(value)))
        .collect()
}

fn gate_value(value: &Value) -> Json {
    match value {
        Value::Null => Json::Null,
        Value::List(items) => Json::Array(items.iter().map(gate_value).collect()),
        Value::Map(entries) => Json::Object(entries.iter().map(|(key, item)| (key.clone(), gate_value(item))).collect()),
        Value::Object(_) | Value::Bytes(_) => Json::String("[upload]".to_string()),
        other => Json::String(to_plain_string(other)),
    }
}

/// SHA-256 over the RFC 8785 canonical JSON of `{v: 1, screenLocation, transition, method, path, extraPath,
/// inlineParameters, parameters}`: what an approval of a held screen write stores as its arguments hash and the gate
/// recomputes before it runs the transition. Another record in the extra path, another inline value or another
/// parameter is another write.
pub fn held_write_hash(write: &HeldWrite) -> String {
    moqui_lib::json::canonical_sha256(&json!({
        "v": 1,
        "screenLocation": write.screen_location,
        "transition": write.transition,
        "method": write.method,
        "path": write.path,
        "extraPath": write.extra_path,
        "inlineParameters": write.inline_parameters,
        "parameters": write.parameters,
    }))
}

/// What a transition did, for the observer.
#[derive(Clone, Debug, PartialEq)]
pub struct TransitionOutcome {
    pub origin: RenderOrigin,
    pub user_id: Option<String>,
    pub screen_location: String,
    pub transition: String,
    pub method: String,
    /// The request parameters as [`gate_parameters`] gives them: the values posted.
    pub parameters: Map<String, Json>,
    /// The extra path after the transition and the inline parameters, as [`HeldWrite`] has them.
    pub extra_path: Vec<String>,
    pub inline_parameters: Map<String, Json>,
    /// The gate held it (or the approval did not match), so nothing ran.
    pub held: bool,
    /// The user came by a request header (Basic, a key), or the session is one a header login made: a program, not a
    /// person at a browser. A hand-off takes no save of such a request.
    pub header_authenticated: bool,
    /// The response status; 200 when nothing was sent.
    pub status: u16,
    /// A redirect's location, when the response was one.
    pub location: Option<String>,
    /// The error, when the transition failed with one.
    pub error: Option<String>,
    pub errors: Vec<String>,
    /// `field: message` of each validation error.
    pub validation_errors: Vec<String>,
    pub messages: Vec<String>,
}

/// Sees each transition's outcome: the hand-off watches of phase 13.4 match a user's save of the screen they were
/// handed. Called on the render's thread after the transition ran, its transaction ended.
pub trait TransitionObserver: Send + Sync {
    /// Whether to build the outcome of a transition of this user at all (a user with an active watch).
    fn wants(&self, user_id: Option<&str>) -> bool;
    fn observed(&self, outcome: &TransitionOutcome);
}

static OBSERVER: InstallSlot<dyn TransitionObserver> = InstallSlot::new();

/// Use `observer` for every transition in this process.
pub fn install_transition_observer(observer: Arc<dyn TransitionObserver>) {
    OBSERVER.install(observer);
}

/// Remove the installed observer.
pub fn clear_transition_observer() {
    OBSERVER.clear();
}

pub(super) fn observer() -> Option<Arc<dyn TransitionObserver>> {
    OBSERVER.get()
}

/// What a render running on this thread came from: its origin, whether its user came by a header (Basic, `api_key`,
/// `login_key`, `authUsername`) rather than the session cookie, and whether a transition of it presented the session
/// token and it matched.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Frame {
    origin: RenderOrigin,
    header_authenticated: bool,
    session_token_checked: bool,
}

thread_local! {
    /// The renders running on this thread, innermost last.
    static ORIGIN: RefCell<Vec<Frame>> = const { RefCell::new(Vec::new()) };
}

pub(super) struct OriginGuard;

impl OriginGuard {
    pub(super) fn install(origin: RenderOrigin, header_authenticated: bool) -> Self {
        ORIGIN.with(|stack| {
            stack.borrow_mut().push(Frame {
                origin,
                header_authenticated,
                session_token_checked: false,
            });
        });
        Self
    }
}

impl Drop for OriginGuard {
    fn drop(&mut self) {
        ORIGIN.with(|stack| {
            stack.borrow_mut().pop();
        });
    }
}

/// The transition of the innermost render presented the session token, and it matched.
pub(super) fn note_session_token_checked() {
    ORIGIN.with(|stack| {
        if let Some(frame) = stack.borrow_mut().last_mut() {
            frame.session_token_checked = true;
        }
    });
}

/// The origin of the render running on this thread, if one is.
pub fn current_origin() -> Option<RenderOrigin> {
    ORIGIN.with(|stack| stack.borrow().last().map(|frame| frame.origin.clone()))
}

/// How a person is deciding through the render running on this thread, when one is (plans/LLM-A2A-Assist.md 4.8):
/// the terminal client ([`RenderOrigin::Terminal`]), or a browser request authenticated by its session cookie whose
/// transition presented a valid session token, the session's user having come by the login form (a session a header
/// login made is a program's, [`crate::user::header_login_session`]). Never a header-authenticated request, an agent's
/// own screen session,
/// `agent-apps` or internal code, so an A2A, MCP or REST caller cannot decide an approval.
pub fn interactive_request() -> Option<InteractiveRequest> {
    ORIGIN.with(|stack| {
        let stack = stack.borrow();
        let frame = stack.last()?;
        // a render inside another one (a service rendering a screen) is only as interactive as all of them
        if stack.iter().any(|frame| !frame.origin.is_interactive()) {
            return None;
        }
        match frame.origin {
            RenderOrigin::Terminal => Some(InteractiveRequest::Terminal),
            // a session whose user came by a header (a key, Basic) is a program's, though it now sends the cookie
            RenderOrigin::Browser
                if !frame.header_authenticated
                    && frame.session_token_checked
                    && !crate::user::header_login_session() =>
            {
                Some(InteractiveRequest::Browser)
            }
            _ => None,
        }
    })
}

/// Where an interactive decision comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InteractiveRequest {
    /// A browser session, by its cookie and session token.
    Browser,
    /// The terminal client of a person.
    Terminal,
}

/// Run `body` as if a render of `origin` were running on this thread: what the approval tests decide through.
#[cfg(test)]
pub(crate) fn with_request_frame<T>(
    origin: RenderOrigin,
    header_authenticated: bool,
    session_token_checked: bool,
    body: impl FnOnce() -> T,
) -> T {
    let _guard = OriginGuard::install(origin, header_authenticated);
    if session_token_checked {
        note_session_token_checked();
    }
    body()
}
