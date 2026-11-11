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

use super::access::{self, Facts};
use super::registry::Registry;
use super::session::Kind;
use super::{terminal_number, DEFAULT_MAX, DEFAULT_MAX_PER_USER};

moqui_core::extension::submit! {
    moqui_core::extension::Extension {
        features: &["tui"],
        register_services,
        after_boot: warn_remote,
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

/// A server whose command line or host sessions are reachable from other machines says so at start (§8.2).
fn warn_remote(factory: &ExecutionContextFactory) {
    if !factory.conf_flag("terminal", "remote", false) || !moqui_core::devtools::is_dev_instance() {
        return;
    }
    tracing::warn!(
        "terminal remote is on: a user with SSH_SHELL may open the moqui> command line, and one with TERMINAL_HOST a \
         shell as this server's operating system user, from other machines over HTTPS with a recent sign-in \
         (plans/Terminal-Fancy.md §8.2)"
    );
}

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

/// A permission of the signed-in user; a check that fails is a no.
fn permitted(permission: &str) -> bool {
    moqui_core::user::has_permission(permission).unwrap_or(false)
}

/// The host profiles the signed-in user may open: name, title, own (theirs, not the conf's) and agent.
#[cfg(all(feature = "devtools", unix))]
fn host_profiles(factory: &ExecutionContextFactory) -> Vec<serde_json::Value> {
    let own = moqui_core::user::get_preference(super::host::PROFILES_PREFERENCE)
        .ok()
        .flatten();
    super::host::available(&factory.conf_text(), own.as_deref())
        .iter()
        .map(
            |profile| json!({"name": profile.name, "title": profile.title, "own": profile.own, "agent": profile.agent}),
        )
        .collect()
}

#[cfg(not(all(feature = "devtools", unix)))]
fn host_profiles(_factory: &ExecutionContextFactory) -> Vec<serde_json::Value> {
    Vec::new()
}

#[cfg(all(feature = "devtools", unix))]
fn host_outlives(factory: &ExecutionContextFactory) -> bool {
    super::held::Holder::of(factory).in_use()
}

#[cfg(not(all(feature = "devtools", unix)))]
fn host_outlives(_factory: &ExecutionContextFactory) -> bool {
    false
}

/// `get#TerminalOptions`: the kinds this user may open from here, and the limits.
fn get_terminal_options(_parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let _username = match username() {
        Ok(username) => username,
        Err(refused) => return refused,
    };
    let factory = factory()?;
    let registry = Registry::of(&factory);
    let facts = Facts::of_current_request().listing();
    let offered = access::offered(access::Switches::of(&factory), &facts, &permitted);
    let profiles = if offered.contains(&Kind::Host) {
        host_profiles(&factory)
    } else {
        Vec::new()
    };
    let kinds: Vec<&str> = offered.into_iter().map(Kind::as_str).collect();
    let options = json!({
        "kinds": kinds,
        "profiles": profiles,
        "maxSessions": terminal_number(&factory, "max-sessions", DEFAULT_MAX),
        "maxSessionsPerUser": terminal_number(&factory, "max-sessions-per-user", DEFAULT_MAX_PER_USER),
        "detachSeconds": registry.detach.as_secs(),
        // a host session outlives the server while the holder keeps it (§7.5)
        "hostSessionsOutliveServer": host_outlives(&factory),
    });
    answer("options", &options)
}

/// The host sessions the holder kept from the server before, listed again on the first look after a start (§7.5).
#[cfg(all(feature = "devtools", unix))]
pub(crate) fn adopt_held(factory: &Arc<ExecutionContextFactory>) {
    let holder = super::held::Holder::of(factory);
    if holder.adopted() {
        return;
    }
    let weak = Arc::downgrade(factory);
    if let Err(err) = crate::edge::block_on(async move { holder.adopt(weak).await }) {
        tracing::error!("the terminal holder's sessions were not taken back: {}", err.message());
    }
}

#[cfg(not(all(feature = "devtools", unix)))]
pub(crate) fn adopt_held(_factory: &Arc<ExecutionContextFactory>) {}

/// `find#TerminalSessions`: the user's sessions, oldest first.
fn find_terminal_sessions(_parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let username = match username() {
        Ok(username) => username,
        Err(refused) => return refused,
    };
    let factory = factory()?;
    adopt_held(&factory);
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
    let facts = Facts::of_current_request();
    if let Err(refusal) = access::check(
        access::Switches::of(&factory),
        kind,
        &facts,
        &permitted,
        &moqui_core::user::recent_auth,
    ) {
        tracing::info!("terminal {} session refused for {username}: {refusal:?}", kind.as_str());
        return refuse(refusal.code(), refusal.message());
    }
    let opened = match kind {
        Kind::Apps => registry.open_apps(&factory, &username, &title, cols, rows),
        #[cfg(feature = "devtools")]
        Kind::Command => open_command(&factory, &registry, &username, &title, cols, rows, &facts),
        #[cfg(all(feature = "devtools", unix))]
        Kind::Host => {
            let opening = HostOpening {
                profile: &text(parameters, "profileName"),
                directory: &text(parameters, "directory"),
                resume: text(parameters, "resume") == "true",
                title: &title,
                size: (cols, rows),
            };
            match open_host(&factory, &registry, &username, &opening, &facts) {
                Ok(session) => Ok(session),
                Err(HostRefusal::NoProfile) => return refuse(NOT_AVAILABLE, "No such terminal profile"),
                Err(HostRefusal::Refused(reason)) => Err(reason),
            }
        }
        #[allow(unreachable_patterns)]
        _ => return refuse(NOT_AVAILABLE, "That terminal is not available here"),
    };
    match opened {
        Ok(session) => {
            // a command or host session's opening is logged with its audit row
            if kind == Kind::Apps {
                tracing::info!("terminal session {} (apps) opened for {username}", session.id);
            }
            let mut out = BTreeMap::from([("sessionId".to_string(), Value::string(&session.id))]);
            out.insert("session".to_string(), from_json(&session.to_json()));
            Ok(out)
        }
        Err(reason) => refuse("limit", &reason),
    }
}

/// A command line, with its audit row.
#[cfg(feature = "devtools")]
fn open_command(
    factory: &Arc<ExecutionContextFactory>,
    registry: &Arc<Registry>,
    username: &str,
    title: &str,
    cols: u16,
    rows: u16,
    facts: &Facts,
) -> Result<Arc<super::session::Session>, String> {
    let id = super::registry::new_id()?;
    let spec = super::registry::CommandSpec {
        id: id.clone(),
        username: username.to_string(),
        title: title.to_string(),
        cols,
        rows,
        on_end: Some(super::audit::on_end(Arc::downgrade(factory), id.clone())),
        facts: facts.clone(),
    };
    let session = registry.open_command(factory, spec)?;
    super::audit::record_open(&super::audit::Opened {
        id: &id,
        username,
        kind: Kind::Command,
        profile: "",
        command_line: "",
        directory: "",
        facts,
    });
    Ok(session)
}

#[cfg(all(feature = "devtools", unix))]
struct HostOpening<'a> {
    profile: &'a str,
    directory: &'a str,
    resume: bool,
    title: &'a str,
    size: (u16, u16),
}

#[cfg(all(feature = "devtools", unix))]
enum HostRefusal {
    NoProfile,
    Refused(String),
}

/// A program on the server's operating system, with its audit row.
#[cfg(all(feature = "devtools", unix))]
fn open_host(
    factory: &Arc<ExecutionContextFactory>,
    registry: &Arc<Registry>,
    username: &str,
    opening: &HostOpening<'_>,
    facts: &Facts,
) -> Result<Arc<super::session::Session>, HostRefusal> {
    let name = if opening.profile.is_empty() {
        super::host::SHELL_PROFILE
    } else {
        opening.profile
    };
    let own = moqui_core::user::get_preference(super::host::PROFILES_PREFERENCE)
        .ok()
        .flatten();
    let Some(profile) = super::host::available(&factory.conf_text(), own.as_deref())
        .into_iter()
        .find(|profile| profile.name == name)
    else {
        return Err(HostRefusal::NoProfile);
    };
    let id = super::registry::new_id().map_err(HostRefusal::Refused)?;
    let directory = Some(opening.directory).filter(|directory| !directory.is_empty());
    let launch = super::host::Launch::of(
        factory,
        &profile,
        &id,
        username,
        directory,
        opening.resume,
        opening.size,
    );
    let command_line = launch.command_line();
    let directory = launch.directory.to_string_lossy().into_owned();
    let spec = super::registry::HostSpec {
        id: id.clone(),
        username: username.to_string(),
        title: opening.title.to_string(),
        profile: profile.clone(),
        launch,
        on_end: Some(super::audit::on_end(Arc::downgrade(factory), id.clone())),
    };
    let session = registry.open_host(factory, spec).map_err(HostRefusal::Refused)?;
    super::audit::record_open(&super::audit::Opened {
        id: &id,
        username,
        kind: Kind::Host,
        profile: &profile.name,
        command_line: &command_line,
        directory: &directory,
        facts,
    });
    Ok(session)
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
