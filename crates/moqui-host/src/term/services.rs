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

//! `org.moqui.terminal.TerminalServices` (plans/Terminal-Fancy.md §5.7): what the Terminal screen's transitions call to
//! list, open, rename and end the signed-in user's kept sessions. Each works on the caller's own sessions only, whatever
//! calls it; a refusal adds an error and answers a `reasonCode`. The socket (`/termws?session=`) attaches to a session
//! these opened.

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_core::extension::RustServiceFn;
use moqui_lib::error::Error as LibError;
use moqui_lib::json::from_json;
use moqui_lib::object::{is_empty, to_plain_string, Value};
use serde_json::json;

use super::registry::Registry;
use super::session::Kind;
use super::{terminal_number, DEFAULT_MAX, DEFAULT_MAX_PER_USER};

moqui_core::extension::submit! {
    moqui_core::extension::Extension {
        features: &["tui"],
        register_services,
        before_shutdown: super::registry::before_shutdown,
        ..moqui_core::extension::Extension::EMPTY
    }
}

fn register_services(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    moqui_core::rust_service!(map, get_terminal_options)?;
    moqui_core::rust_service!(map, find_terminal_sessions)?;
    moqui_core::rust_service!(map, open_terminal_session)?;
    moqui_core::rust_service!(map, update_terminal_session)?;
    moqui_core::rust_service!(map, close_terminal_session)?;
    Ok(())
}

type Out = Result<BTreeMap<String, Value>, LibError>;

/// The refusal that tells nothing about why: a kind this server does not offer here, for this user, from this place.
pub(crate) const NOT_AVAILABLE: &str = "notAvailable";

fn factory() -> Result<Arc<ExecutionContextFactory>, LibError> {
    ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
        .ok_or_else(|| LibError::new("a terminal service needs an execution context"))
}

/// The signed-in user, or a refusal.
fn username() -> Result<String, Out> {
    match moqui_core::user::current_username() {
        Ok(Some(username)) if !username.is_empty() => Ok(username),
        _ => Err(refuse("notSignedIn", "Sign in to use the terminal")),
    }
}

/// An error for the caller and `reasonCode` in the answer.
pub(crate) fn refuse(reason_code: &str, message: &str) -> Out {
    let message = message.to_string();
    ExecutionContext::with_current(|ec| ec.message().add_error(message));
    Ok(BTreeMap::from([("reasonCode".to_string(), Value::string(reason_code))]))
}

fn text(parameters: &BTreeMap<String, Value>, key: &str) -> String {
    parameters
        .get(key)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .unwrap_or_default()
}

/// A size from the parameters, within 20 by 10 and 500 by 500.
fn dimension(parameters: &BTreeMap<String, Value>, key: &str, least: u16, default: u16) -> u16 {
    // a page's JSON numbers may come as decimals
    text(parameters, key)
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .map_or(default, |value| {
            let whole = value.round().clamp(f64::from(least), 500.0);
            u16::try_from(whole as i64).unwrap_or(default)
        })
}

fn answer(key: &str, value: &serde_json::Value) -> Out {
    Ok(BTreeMap::from([(key.to_string(), from_json(value))]))
}

/// `get#TerminalOptions`: the kinds this user may open from here, and the limits.
fn get_terminal_options(_parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let _username = match username() {
        Ok(username) => username,
        Err(refused) => return refused,
    };
    let factory = factory()?;
    let registry = Registry::of(&factory);
    let options = json!({
        "kinds": ["apps"],
        "profiles": [],
        "maxSessions": terminal_number(&factory, "max-sessions", DEFAULT_MAX),
        "maxSessionsPerUser": terminal_number(&factory, "max-sessions-per-user", DEFAULT_MAX_PER_USER),
        "detachSeconds": registry.detach.as_secs(),
        // a host session ends with the server until the holder keeps it (§7.5)
        "hostSessionsOutliveServer": false,
    });
    answer("options", &options)
}

/// `find#TerminalSessions`: the user's sessions, oldest first.
fn find_terminal_sessions(_parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let username = match username() {
        Ok(username) => username,
        Err(refused) => return refused,
    };
    let factory = factory()?;
    let sessions: Vec<serde_json::Value> = Registry::of(&factory)
        .list(&username)
        .iter()
        .map(|session| session.to_json())
        .collect();
    answer("sessions", &serde_json::Value::Array(sessions))
}

/// `open#TerminalSession`: a new session of `kind` at `cols` by `rows`; answers `sessionId` and the session.
fn open_terminal_session(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let username = match username() {
        Ok(username) => username,
        Err(refused) => return refused,
    };
    let Some(kind) = Kind::parse(&text(parameters, "kind")) else {
        return refuse(NOT_AVAILABLE, "That terminal is not available here");
    };
    let factory = factory()?;
    let registry = Registry::of(&factory);
    let cols = dimension(parameters, "cols", 20, 80);
    let rows = dimension(parameters, "rows", 10, 24);
    let title = text(parameters, "title");
    let opened = match kind {
        Kind::Apps => registry.open_apps(&factory, &username, &title, cols, rows),
        Kind::Command | Kind::Host => return refuse(NOT_AVAILABLE, "That terminal is not available here"),
    };
    match opened {
        Ok(session) => {
            tracing::info!(
                "terminal session {} ({}) opened for {username}",
                session.id,
                kind.as_str()
            );
            let mut out = BTreeMap::from([("sessionId".to_string(), Value::string(&session.id))]);
            out.insert("session".to_string(), from_json(&session.to_json()));
            Ok(out)
        }
        Err(reason) => refuse("limit", &reason),
    }
}

/// `update#TerminalSession`: the session's title, the user's own name for it (empty gives it back to the program).
fn update_terminal_session(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let username = match username() {
        Ok(username) => username,
        Err(refused) => return refused,
    };
    let factory = factory()?;
    let Some(session) = Registry::of(&factory).find(&text(parameters, "sessionId"), &username) else {
        return refuse("notFound", "No such terminal session");
    };
    session.rename(text(parameters, "title").trim());
    answer("session", &session.to_json())
}

/// `close#TerminalSession`: end one of the user's own sessions.
fn close_terminal_session(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let username = match username() {
        Ok(username) => username,
        Err(refused) => return refused,
    };
    let factory = factory()?;
    let Some(session) = Registry::of(&factory).find(&text(parameters, "sessionId"), &username) else {
        return refuse("notFound", "No such terminal session");
    };
    session.end("the session was ended");
    Ok(BTreeMap::new())
}
