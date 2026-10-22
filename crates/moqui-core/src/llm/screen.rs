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

//! The screen sessions of the agents (plans/LLM-A2A-Assist.md 4.7): the [`ScreenSessions`] provider the `screen` tool
//! drives, and its install slot. The host installs one (`crates/moqui-host/src/llm/screens.rs`, an in-process
//! terminal client per session); without it the [`caps::SCREEN`](super::tools::caps::SCREEN) capability is absent and
//! the screen tools are not offered.
//!
//! A session belongs to one user and one conversation. It renders as the agent ([`RenderOrigin::Agent`]
//! (crate::screen::RenderOrigin)) under the read fence, every write held by the transition gate: a press that writes
//! answers a [`PendingWrite`], which an approval (phase 13.4) runs later with [`ScreenSessions::execute_approved`]. A
//! session is a cache: [`ScreenState`] (the path, its parameters and the unsaved values), kept in
//! `AgentConversation.screenStateJson`, rebuilds it on another node or for a later run.

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_lib::error::Error;
use moqui_lib::object::Value;
use moqui_lib::sync::InstallSlot;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::entity::EntityFacade;

/// Where a headless session starts when the agent names no start path.
pub const DEFAULT_START_PATH: &str = "/apps";
/// The drawing size of a session unless asked otherwise (plan 7.7: 100x30).
pub const DEFAULT_COLUMNS: u16 = 100;
pub const DEFAULT_ROWS: u16 = 30;
/// The most rows a `screen` call may ask for.
pub const MAX_ROWS: u16 = 100;

const CONVERSATION: &str = "moqui.agent.AgentConversation";

/// Why a session call did not answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScreenError {
    /// No session of that id for this owner: it was reaped, closed, held by another node, or another user's.
    NotFound,
    /// Refused before anything ran (a limit, a path that is not a screen); the text says why, for the model.
    Refused(String),
    /// It failed while running.
    Failed(String),
}

impl std::fmt::Display for ScreenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScreenError::NotFound => f.write_str("there is no such screen session"),
            ScreenError::Refused(text) | ScreenError::Failed(text) => f.write_str(text),
        }
    }
}

/// What a new session is: whose, for which conversation and run, where it starts and how big it draws, whether the
/// user's page follows it, and the state to rebuild.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OpenSpec {
    pub user_id: String,
    pub username: String,
    pub conversation_id: String,
    pub agent_id: String,
    pub run_id: Option<String>,
    /// The agent's start path, else [`DEFAULT_START_PATH`].
    pub start_path: String,
    pub columns: u16,
    pub rows: u16,
    /// The user's page follows (the drawer): the commands answer follow events. Headless surfaces have it off.
    pub follow: bool,
    /// The state a session of this conversation left, to stand where it stood.
    pub state: Option<ScreenState>,
}

/// The state of a session that rebuilds it: the path, its parameters and the values typed but not saved, by form
/// `domId` and request name (the names the browser posts, so a drawer's state reads the same way).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenState {
    pub path: String,
    #[serde(default)]
    pub parameters: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, BTreeMap<String, String>>,
}

impl ScreenState {
    /// The path with its parameters as a query.
    pub fn url(&self) -> String {
        if self.parameters.is_empty() {
            return self.path.clone();
        }
        let query: Vec<String> = self
            .parameters
            .iter()
            .map(|(name, value)| format!("{}={}", encode(name), encode(value)))
            .collect();
        format!("{}?{}", self.path, query.join("&"))
    }
}

fn encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(char::from(byte)),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// One call of the `screen` tool, run in order: `go`, `set` (each in order), `find`, `press` (`do`), `key`. With none
/// it looks again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScreenCommand {
    /// The run the command is for: the render origin of each request.
    pub agent_id: String,
    pub run_id: Option<String>,
    pub go: Option<String>,
    /// `(element, value)` in order.
    pub set: Vec<(String, String)>,
    pub find: Option<String>,
    pub press: Option<String>,
    /// Key chords, space separated: only when no element fits.
    pub key: Option<String>,
    /// Answer the element list after the screen.
    pub elements: bool,
    /// The drawing height.
    pub rows: Option<u16>,
}

/// A command's answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScreenAnswer {
    /// Every step did what it asked; a step that failed stopped the ones after it.
    pub ok: bool,
    /// The screen as text, ending `--- ok` or `--- error <why>`.
    pub text: String,
    pub path: String,
    pub parameters: BTreeMap<String, String>,
    pub title: String,
    /// The write a press would have run, held for the user's approval.
    pub held: Option<PendingWrite>,
    /// For the user's page, when the session follows: `ui.navigate`, `ui.fill`, `ui.dialog`, `ui.focus`,
    /// `ui.highlight`.
    pub follow: Vec<FollowEvent>,
    /// The state to keep for the conversation.
    pub state: ScreenState,
}

/// A stored event for the user's page (plan 4.5).
#[derive(Clone, Debug, PartialEq)]
pub struct FollowEvent {
    pub event_type: String,
    pub payload: Json,
}

/// What the browser has on its page, sent with each drawer turn: the path, its parameters, the changed values of its
/// forms, the open dialogs and the selection.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserState {
    pub path: String,
    #[serde(default)]
    pub parameters: BTreeMap<String, String>,
    #[serde(default)]
    pub forms: Vec<BrowserForm>,
    #[serde(default)]
    pub dialogs: Vec<String>,
}

/// One form of the page: its id (the qvt `domId`), its values by request name, and which of them the user changed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserForm {
    pub dom_id: String,
    #[serde(default)]
    pub values: BTreeMap<String, String>,
    /// The names the user changed; none named is every value.
    #[serde(default)]
    pub dirty: Vec<String>,
}

impl BrowserForm {
    /// The values to put in the session: the dirty ones, or all when none is marked.
    pub fn dirty_values(&self) -> BTreeMap<String, String> {
        if self.dirty.is_empty() {
            return self.values.clone();
        }
        self.values
            .iter()
            .filter(|(name, _)| self.dirty.contains(name))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    }
}

/// A write the gate held: the request exactly as it was made (sent again with the approval's grant, so the hash the
/// gate recomputes is the one approved), the 409's `heldWrite` with its `argsHash`, and the changed values the approval
/// card shows. Serializable, so phase 13.4 keeps it on the approval.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingWrite {
    /// The user (id) whose session held it: only they may recheck it or run it approved.
    #[serde(default)]
    pub owner: String,
    /// The agent whose session held it: the render origin of the recheck and of the approved run.
    #[serde(default)]
    pub agent_id: String,
    /// The absolute url the session asked for, its query included.
    pub url: String,
    pub method: String,
    pub fields: BTreeMap<String, String>,
    /// `heldWrite` of the 409 answer (`screenLocation`, `transition`, `service`, `authzAction`, `path`, ...).
    pub held: Json,
    pub args_hash: String,
    /// The screen the press was on.
    pub path: String,
    #[serde(default)]
    pub parameters: BTreeMap<String, String>,
    /// The element pressed.
    #[serde(default)]
    pub element: String,
    /// The qvt id of the form that posted, when one did.
    #[serde(default)]
    pub form: String,
    /// The fields whose values the agent changed: the original against the proposed.
    #[serde(default)]
    pub changes: Vec<FieldChange>,
}

impl PendingWrite {
    /// The transition's name, as `heldWrite` gives it.
    pub fn transition(&self) -> &str {
        self.held.get("transition").and_then(Json::as_str).unwrap_or("")
    }
}

/// One changed field of a held write.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldChange {
    /// The request name.
    pub field: String,
    pub label: String,
    pub original: String,
    pub proposed: String,
}

/// A field that changed on the server since its write was proposed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldConflict {
    pub field: String,
    pub label: String,
    pub original: String,
    pub current: String,
    pub proposed: String,
}

/// What an approval gives the write it approves: the gate runs it only when the request hashes to `args_hash`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApprovalGrant {
    pub approval_id: String,
    pub args_hash: String,
    /// The run the write is for: the render origin of the request.
    pub agent_id: String,
    pub run_id: Option<String>,
}

/// The screen sessions of this process.
pub trait ScreenSessions: Send + Sync {
    /// The session of `spec.conversation_id` for its user when one is live here, else a new one at the state's path or
    /// the start path; its id.
    ///
    /// # Errors
    ///
    /// [`ScreenError::Refused`] at a limit (sessions in all, per user).
    fn open(&self, spec: &OpenSpec) -> Result<String, ScreenError>;

    /// The live session of the conversation for `owner` (a user id), if this node has one.
    fn find(&self, owner: &str, conversation_id: &str) -> Option<String>;

    /// Run `command` in the session `id` of `owner`.
    ///
    /// # Errors
    ///
    /// [`ScreenError::NotFound`] when there is no such session for this owner.
    fn command(&self, id: &str, owner: &str, command: &ScreenCommand) -> Result<ScreenAnswer, ScreenError>;

    /// Bring the session to where the user's browser is: its path, then the values the user changed. Answers what the
    /// page should do to follow anything the session added (normally nothing).
    ///
    /// # Errors
    ///
    /// [`ScreenError::NotFound`] when there is no such session for this owner.
    fn sync(&self, id: &str, owner: &str, state: &BrowserState) -> Result<Vec<FollowEvent>, ScreenError>;

    /// The fields of a held write whose values changed on the server since it was proposed: the screen rendered again
    /// (fenced, held) and its form's values compared with the originals.
    ///
    /// # Errors
    ///
    /// [`ScreenError::NotFound`] when the write is not `owner`'s, [`ScreenError::Failed`] when the screen does not
    /// render.
    fn recheck(&self, id: &str, owner: &str, write: &PendingWrite) -> Result<Vec<FieldConflict>, ScreenError>;

    /// Run an approved write: the request of `write` sent again with the gate [`Approved`]
    /// (crate::screen::TransitionGate::Approved) by `grant`, so it runs only when its hash is the approved one; then
    /// the session goes where the transition sent it. The session may be gone: the write runs on a link of its own.
    ///
    /// # Errors
    ///
    /// [`ScreenError::NotFound`] when the write is not `owner`'s, [`ScreenError::Refused`] when the gate holds it
    /// again (a mismatch), [`ScreenError::Failed`] when it fails.
    fn execute_approved(
        &self,
        id: &str,
        owner: &str,
        write: &PendingWrite,
        grant: &ApprovalGrant,
    ) -> Result<ScreenAnswer, ScreenError>;

    /// End the session.
    fn close(&self, id: &str);
}

static SESSIONS: InstallSlot<dyn ScreenSessions> = InstallSlot::new();

/// Use `sessions` for every agent screen session in this process.
pub fn install_screen_sessions(sessions: Arc<dyn ScreenSessions>) {
    SESSIONS.install(sessions);
}

/// Remove the installed provider.
pub fn clear_screen_sessions() {
    SESSIONS.clear();
}

/// The installed provider.
pub fn sessions() -> Option<Arc<dyn ScreenSessions>> {
    SESSIONS.get()
}

/// Whether screen sessions can be opened in this process.
pub fn installed() -> bool {
    SESSIONS.get().is_some()
}

/// Why `path` is not one a `go` may open, judged by its text: not from the root, a scheme or host, `..`, `//`, a
/// backslash or a control character. A path that passes is then resolved to a screen ([`check_screen_path`]).
pub fn path_problem(path: &str) -> Option<String> {
    let bare = path.split(['?', '#']).next().unwrap_or(path);
    if !bare.starts_with('/') {
        return Some(format!(
            "{path} is not a path from the root; go takes a screen path such as /apps/system"
        ));
    }
    if bare.contains("://") || bare.starts_with("//") || bare.contains("//") {
        return Some(format!("{path} is not a path on this server"));
    }
    if bare.split('/').any(|name| name == "..") || path.contains('\\') || path.chars().any(char::is_control) {
        return Some(format!("{path} is not a screen path"));
    }
    None
}

/// Refuse a `go` to anything that is not a screen: a path [`path_problem`] refuses, one that names no screen, a
/// transition or a file. Whether the user may open it is the render's to say (a permission error is the answer).
///
/// # Errors
///
/// The refusal, worded for the model.
pub fn check_screen_path(factory: &crate::context::ExecutionContextFactory, path: &str) -> Result<(), String> {
    if let Some(problem) = path_problem(path) {
        return Err(problem);
    }
    let bare = path.split(['?', '#']).next().unwrap_or(path);
    match crate::screen::locate(factory, &root_screen(factory), bare) {
        Ok(hit) if hit.transition.is_some() => Err(format!(
            "{bare} is an action ({}), not a screen; open its screen and press its button with do",
            hit.transition.unwrap_or_default()
        )),
        Ok(_) => Ok(()),
        Err(_) => Err(format!("there is no screen at {bare}; use a path a tool returned")),
    }
}

/// The root screen the agents' paths are under: the `webroot` webapp's root screen for any host (its `.*` entry), as
/// a request with no host of its own is served, else `webroot.xml`. The one source for the paths the tools check and
/// render.
pub fn root_screen(factory: &crate::context::ExecutionContextFactory) -> String {
    match crate::screen::info::root_screen_for_host(factory, "", "webroot") {
        Ok(root) => root,
        Err(error) => {
            tracing::error!(
                "the webroot root screen could not be read, so {} is used: {error}",
                crate::screen::ROOT_SCREEN
            );
            crate::screen::ROOT_SCREEN.to_string()
        }
    }
}

/// The state kept for a conversation, if any. One that does not read is logged and left out.
///
/// # Errors
///
/// Returns an error when the row cannot be read.
pub fn load_state(facade: &EntityFacade, conversation_id: &str) -> Result<Option<ScreenState>, Error> {
    let Some(row) = facade
        .find(CONVERSATION)?
        .eq("conversationId", Value::string(conversation_id))
        .disable_authz()
        .one()?
    else {
        return Ok(None);
    };
    let Some(text) = super::text_of(&row, "screenStateJson") else {
        return Ok(None);
    };
    match serde_json::from_str::<ScreenState>(&text) {
        Ok(state) => Ok(Some(state)),
        Err(error) => {
            tracing::error!(
                "the screen state of conversation {conversation_id} does not read, so it is left out: {error}"
            );
            Ok(None)
        }
    }
}

/// Who keeps a conversation's screen state, which [`store_state`] checks before it writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateWriter<'a> {
    /// A step of the conversation's run on `node`: written only while `node` holds the run's lease (a node that lost
    /// it writes nothing, as every write of the engine).
    Run { run_id: &'a str, node: &'a str },
    /// The conversation's user (a drawer's sync-in): written only when the conversation is theirs.
    Owner(&'a str),
}

/// Keep `state` for the conversation, in a transaction of its own (or the one in place), after `writer` is checked.
/// It is the runtime's own bookkeeping, so it is written outside a read tool's fence.
///
/// # Errors
///
/// Returns an error when the writer may not write it (another node holds the run's lease, the run is not of this
/// conversation, the conversation is not the user's or there is none), or the row cannot be written.
pub fn store_state(
    facade: &EntityFacade,
    conversation_id: &str,
    writer: StateWriter<'_>,
    state: &ScreenState,
) -> Result<(), Error> {
    let text = serde_json::to_string(state).map_err(|error| Error::new(format!("screen state: {error}")))?;
    let _lifted = crate::entity::fence::lift();
    super::run::quietly(|| {
        super::run::in_tx(facade, || {
            if let StateWriter::Run { run_id, node } = writer {
                let run = super::run::held(facade, run_id, node, super::run::Hold::Lease)?;
                if super::text_of(&run, "conversationId").as_deref() != Some(conversation_id) {
                    return Err(Error::new(format!(
                        "agent run {run_id} is not of conversation {conversation_id}"
                    )));
                }
            }
            let mut row = facade
                .find(CONVERSATION)?
                .eq("conversationId", Value::string(conversation_id))
                .for_update(true)
                .disable_authz()
                .one()?
                .ok_or_else(|| Error::new(format!("there is no conversation {conversation_id}")))?;
            if let StateWriter::Owner(owner) = writer {
                if super::text_of(&row, "userId").as_deref() != Some(owner) {
                    // as for no conversation: another user's is not named
                    return Err(Error::new(format!("there is no conversation {conversation_id}")));
                }
            }
            if super::text_of(&row, "screenStateJson").as_deref() == Some(text.as_str()) {
                return Ok(());
            }
            row.set("screenStateJson", Value::string(&text));
            row.update(facade)
        })
    })
}

/// Bring a conversation's screen to the user's browser (a drawer turn's sync-in): its live session when this node has
/// one of `owner`'s, else the kept state, so the next session opens where the user is.
///
/// # Errors
///
/// Returns an error when the conversation is not `owner`'s, or the state cannot be written.
pub fn sync_in(
    facade: &EntityFacade,
    owner: &str,
    conversation_id: &str,
    state: &BrowserState,
) -> Result<Vec<FollowEvent>, Error> {
    if let Some(sessions) = sessions() {
        if let Some(id) = sessions.find(owner, conversation_id) {
            match sessions.sync(&id, owner, state) {
                Ok(events) => return Ok(events),
                Err(ScreenError::NotFound) => {}
                Err(error) => {
                    tracing::warn!("the screen session of conversation {conversation_id} did not sync: {error}")
                }
            }
        }
    }
    let values = state
        .forms
        .iter()
        .map(|form| (form.dom_id.clone(), form.dirty_values()))
        .filter(|(_, values)| !values.is_empty())
        .collect();
    store_state(
        facade,
        conversation_id,
        StateWriter::Owner(owner),
        &ScreenState {
            path: state.path.clone(),
            parameters: state.parameters.clone(),
            values,
        },
    )?;
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_go_path_must_be_a_plain_path_from_the_root() {
        assert_eq!(path_problem("/apps/system?x=1"), None);
        for bad in [
            "apps/system",
            "https://example.com/apps",
            "//example.com/apps",
            "/apps//system",
            "/apps/../x",
            "/apps\\x",
            "/apps/\u{7}",
        ] {
            assert!(path_problem(bad).is_some(), "{bad}");
        }
    }

    fn row(facade: &EntityFacade, entity: &str, fields: &[(&str, &str)]) {
        let mut value = facade.make_value(entity).unwrap();
        for (field, text) in fields {
            value.set(*field, Value::string(*text));
        }
        value.create(facade).unwrap();
    }

    /// The state is the conversation's user's to write (sync-in), and a step's only while its node holds the run's
    /// lease and the run is of that conversation.
    #[test]
    fn a_state_is_kept_only_by_the_owner_or_by_the_node_that_holds_the_run() {
        let facade = crate::testing::open();
        row(&facade, CONVERSATION, &[("conversationId", "SC1"), ("userId", "U1")]);
        row(&facade, CONVERSATION, &[("conversationId", "SC2"), ("userId", "U1")]);
        row(
            &facade,
            "moqui.agent.AgentRun",
            &[
                ("runId", "SR1"),
                ("conversationId", "SC1"),
                ("userId", "U1"),
                ("leaseOwner", "node-a"),
            ],
        );
        let state = |path: &str| ScreenState {
            path: path.to_string(),
            ..ScreenState::default()
        };
        let kept = |id: &str| load_state(&facade, id).unwrap().map(|state| state.path);

        assert!(store_state(&facade, "SC1", StateWriter::Owner("U2"), &state("/apps/u2")).is_err());
        let synced = sync_in(&facade, "U2", "SC1", &BrowserState::default());
        assert!(synced.is_err(), "another user's conversation");
        assert_eq!(kept("SC1"), None);
        store_state(&facade, "SC1", StateWriter::Owner("U1"), &state("/apps/u1")).unwrap();
        assert_eq!(kept("SC1").as_deref(), Some("/apps/u1"));

        let lost = StateWriter::Run {
            run_id: "SR1",
            node: "node-b",
        };
        let error = store_state(&facade, "SC1", lost, &state("/apps/lost")).unwrap_err();
        assert!(crate::llm::run::is_lease_lost(&error), "{error}");
        let other = StateWriter::Run {
            run_id: "SR1",
            node: "node-a",
        };
        assert!(
            store_state(&facade, "SC2", other, &state("/apps/other")).is_err(),
            "not the run's conversation"
        );
        store_state(&facade, "SC1", other, &state("/apps/held")).unwrap();
        assert_eq!(kept("SC1").as_deref(), Some("/apps/held"));
        assert_eq!(kept("SC2"), None);
    }

    #[test]
    fn a_state_reads_back_and_gives_its_url() {
        let state = ScreenState {
            path: "/apps/x".to_string(),
            parameters: BTreeMap::from([("q".to_string(), "a b".to_string())]),
            values: BTreeMap::from([(
                "EditForm".to_string(),
                BTreeMap::from([("name".to_string(), "Ann".to_string())]),
            )]),
        };
        let text = serde_json::to_string(&state).unwrap();
        assert_eq!(serde_json::from_str::<ScreenState>(&text).unwrap(), state);
        assert_eq!(state.url(), "/apps/x?q=a%20b");
        let form = BrowserForm {
            dom_id: "F".to_string(),
            values: BTreeMap::from([("a".to_string(), "1".to_string()), ("b".to_string(), "2".to_string())]),
            dirty: vec!["b".to_string()],
        };
        assert_eq!(
            form.dirty_values(),
            BTreeMap::from([("b".to_string(), "2".to_string())])
        );
    }
}
