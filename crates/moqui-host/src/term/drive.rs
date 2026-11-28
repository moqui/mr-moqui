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

//! Sessions driven by a program (plans/Terminal-Fancy2.md §4.3): `get#TerminalScreen`, `send#TerminalInput`,
//! `wait#TerminalSession` and `find#TerminalEvents`, for a command line or a host session, never an apps session. The
//! caller is the signed-in user, with an attach's checks for the kind and the place (a program signed in by a login key
//! from another machine brings a ticket of `confirm#TerminalUser`), or a program inside one of the user's sessions with
//! that session's token ([`super::token`]), from this machine only. A call with no request is the server's own code (the
//! dev tools' `terminal` agent tool, behind its approval) and counts as this machine. What a program types never renews
//! the HTTP session.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_core::entity::EntityFacade;
use moqui_lib::json::from_json;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;

use super::access::{self, Facts};
use super::registry::Registry;
use super::screen::{Read, Scope};
use super::services::{answer, factory, permitted, refuse, text, username};
use super::session::{now_millis, Kind, Session};

type Out = Result<BTreeMap<String, Value>, moqui_lib::error::Error>;

/// The longest a wait may be asked for.
const WAIT_MAX: Duration = Duration::from_secs(300);
/// The waits of one user at once: each holds a worker thread.
const WAITS_PER_USER: usize = 4;
/// How often a wait looks again.
const WAIT_STEP: Duration = Duration::from_millis(100);
/// The longest pattern a wait takes.
const PATTERN_MAX: usize = 1024;
/// The lines a wait's answer and a refused match carry.
const TAIL: usize = 10;

pub(super) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    moqui_core::rust_service!(map, get_terminal_screen)?;
    moqui_core::rust_service!(map, send_terminal_input)?;
    moqui_core::rust_service!(map, wait_terminal_session)?;
    moqui_core::rust_service!(map, find_terminal_events)?;
    moqui_core::rust_service!(map, find_drivable_terminal_sessions)?;
    Ok(())
}

/// Who drives: the signed-in user from where the request says, or a program in one of the user's sessions.
enum Caller {
    User { username: String, facts: Facts },
    Token { from: Arc<Session> },
}

/// The request's caller: a session's token when the request carries one, else the signed-in user.
fn caller(registry: &Registry) -> Result<Caller, Out> {
    let request = moqui_core::web::current_request();
    let header = |name: &str| {
        request
            .as_ref()
            .and_then(|request| request.header(name))
            .unwrap_or_default()
            .to_string()
    };
    let (session_id, token) = (header(super::token::SESSION_HEADER), header(super::token::TOKEN_HEADER));
    if token.is_empty() {
        // with no request, the server's own code calls (an agent's tool, behind its approval): this machine; a GET, which
        // only reads and which a page of another site cannot read the answer of, carries no Origin of its own and is
        // taken as the request's own, as get#TerminalOptions takes one
        let facts = match &request {
            Some(request) if request.method.eq_ignore_ascii_case("GET") => Facts::of_current_request().listing(),
            Some(_) => Facts::of_current_request(),
            None => Facts {
                via: moqui_core::web::RequestVia::InProcess,
                ..Facts::default()
            },
        };
        return match username() {
            Ok(username) => Ok(Caller::User { username, facts }),
            Err(refused) => Err(refused),
        };
    }
    // a token is for a program on this machine: it never leaves it, and is refused from anywhere else
    let local = Facts::of_current_request().loopback_direct();
    let from = registry
        .find_any(&session_id)
        .filter(|from| from.info().ended.is_none());
    match from {
        Some(from) if local && super::token::holds(&session_id, &token) => Ok(Caller::Token { from }),
        _ => Err(refuse("notSignedIn", "That terminal token is not good here")),
    }
}

/// The session `id` the caller may drive: its own command line or shell, checked as an attach is (the kind's
/// permission and switch, the place, a recent sign-in from another machine), or one its token reaches.
fn target(registry: &Registry, caller: &Caller, id: &str, ticket: &str) -> Result<Arc<Session>, Out> {
    let not_found = || refuse("notFound", "No such terminal session");
    let session = match caller {
        Caller::Token { from } => registry
            .find_any(id)
            .filter(|to| super::token::reaches(from, to))
            .ok_or_else(not_found)?,
        Caller::User { username, facts } => {
            let session = registry.find(id, username).ok_or_else(not_found)?;
            if session.kind == Kind::Apps {
                return Err(not_found());
            }
            let factory = factory().map_err(Err)?;
            let recent =
                |millis: i64| moqui_core::user::recent_auth(millis) || registry.ticket_holds(ticket, username, millis);
            if let Err(refusal) =
                access::check(access::Switches::of(&factory), session.kind, facts, &permitted, &recent)
            {
                tracing::info!("terminal session {id}: driving refused for {username}: {refusal:?}");
                return Err(refuse(refusal.code(), refusal.message()));
            }
            session
        }
    };
    if session.kind == Kind::Apps {
        return Err(not_found());
    }
    Ok(session)
}

/// The caller and its session from the parameters, or the refusal.
fn caller_and_target(parameters: &BTreeMap<String, Value>) -> Result<(Arc<Registry>, Arc<Session>), Out> {
    let factory = factory().map_err(Err)?;
    super::services::adopt_held(&factory);
    let registry = Registry::of(&factory);
    let caller = caller(&registry)?;
    let session = target(
        &registry,
        &caller,
        &text(parameters, "sessionId"),
        &text(parameters, "ticket"),
    )?;
    Ok((registry, session))
}

/// The session's screen read on its pump.
fn read(session: &Arc<Session>, scope: Scope, ansi: bool) -> Option<Read> {
    let session = Arc::clone(session);
    crate::edge::block_on(async move { session.screen(scope, ansi).await })
        .ok()
        .flatten()
}

fn lines_json(lines: &[String]) -> serde_json::Value {
    serde_json::Value::Array(
        lines
            .iter()
            .map(|line| serde_json::Value::String(line.clone()))
            .collect(),
    )
}

/// What every answer says of the session beside its lines.
fn about(session: &Session, read: Option<&Read>) -> serde_json::Value {
    let info = session.info();
    serde_json::json!({
        "sessionId": session.id,
        "kind": session.kind.as_str(),
        "title": info.title,
        "cwd": info.cwd,
        "status": info.status,
        "state": if info.ended.is_some() { "ended" } else { "running" },
        "exitStatus": info.ended.as_ref().map(|(status, _)| *status),
        "cols": read.map_or(info.cols, |read| read.cols),
        "rows": read.map_or(info.rows, |read| read.rows),
        "count": read.map_or(0, |read| read.count),
        "lastEvent": session.last_event(),
    })
}

fn out(value: serde_json::Value) -> Out {
    let serde_json::Value::Object(fields) = value else {
        return Ok(BTreeMap::new());
    };
    Ok(fields
        .iter()
        .map(|(key, value)| (key.clone(), from_json(value)))
        .collect())
}

/// `get#TerminalScreen`: the session's screen as text (§4.2): `scope` visible (the default), scrollback, or last with
/// its `lines`; `format` ansi adds the screen as escape sequences.
pub fn get_terminal_screen(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let (_registry, session) = match caller_and_target(parameters) {
        Ok(found) => found,
        Err(refused) => return refused,
    };
    let lines = text(parameters, "lines").parse::<usize>().unwrap_or(24);
    let Some(scope) = Scope::parse(&text(parameters, "scope"), lines.min(10_000)) else {
        return refuse("badScope", "A screen's scope is visible, scrollback or last");
    };
    let Some(screen) = read(&session, scope, text(parameters, "format") == "ansi") else {
        return refuse("notFound", "That terminal session has no screen");
    };
    let mut value = about(&session, Some(&screen));
    value["lines"] = lines_json(&screen.lines);
    value["cursor"] = serde_json::json!({"row": screen.cursor.0, "col": screen.cursor.1});
    value["alternate"] = serde_json::Value::Bool(screen.alternate);
    value["rebuilt"] = serde_json::Value::Bool(screen.rebuilt);
    if !screen.ansi.is_empty() {
        value["ansi"] = serde_json::Value::String(screen.ansi);
    }
    out(value)
}

/// `send#TerminalInput`: `text` as typed, then the named `keys` (comma separated, or a list), in the program's modes.
pub fn send_terminal_input(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    // input runs code: never under an agent's fence or in a sim, as opening a session is not
    moqui_core::entity::fence::check_effect(moqui_core::entity::fence::Effect::Process, "send#TerminalInput")?;
    let (_registry, session) = match caller_and_target(parameters) {
        Ok(found) => found,
        Err(refused) => return refused,
    };
    let keys = key_names(parameters.get("keys"));
    let typed = text(parameters, "text");
    let sending = Arc::clone(&session);
    let sent = crate::edge::block_on(async move { sending.send_keys(typed, keys).await });
    match sent {
        Ok(Ok(count)) => answer("count", &serde_json::json!(count)),
        Ok(Err(why)) => refuse("badInput", &why),
        Err(_) => refuse("notFound", "That terminal session no longer reads its input"),
    }
}

/// The names of `keys`: a list, or text separated by commas or spaces.
fn key_names(keys: Option<&Value>) -> Vec<String> {
    let Some(keys) = keys.filter(|keys| !moqui_lib::object::is_empty(keys)) else {
        return Vec::new();
    };
    match moqui_lib::json::to_json(keys) {
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        other => other
            .as_str()
            .map(|text| {
                text.split([',', ' '])
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// `find#TerminalEvents`: the session's events after number `after` (0 for all it keeps).
pub fn find_terminal_events(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let (_registry, session) = match caller_and_target(parameters) {
        Ok(found) => found,
        Err(refused) => return refused,
    };
    let after = text(parameters, "after").parse::<u64>().unwrap_or(0);
    let events: Vec<serde_json::Value> = session
        .events_after(after)
        .iter()
        .map(super::events::Event::to_json)
        .collect();
    let mut value = about(&session, None);
    value["events"] = serde_json::Value::Array(events);
    out(value)
}

/// `find#DrivableTerminalSessions`: the sessions the caller may drive, as `find#TerminalSessions` lists them: the
/// signed-in user's command lines and shells, or those a token reaches (`moqui term list`).
pub fn find_drivable_terminal_sessions(_parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let factory = factory()?;
    super::services::adopt_held(&factory);
    let registry = Registry::of(&factory);
    let listed: Vec<Arc<Session>> = match caller(&registry) {
        Ok(Caller::User { username, .. }) => registry.list(&username),
        Ok(Caller::Token { from }) => registry
            .list(&from.username)
            .into_iter()
            .filter(|to| super::token::reaches(&from, to))
            .collect(),
        Err(refused) => return refused,
    };
    let sessions: Vec<serde_json::Value> = listed
        .iter()
        .filter(|session| session.kind != Kind::Apps)
        .map(|session| session.to_json())
        .collect();
    answer("sessions", &serde_json::Value::Array(sessions))
}

/// What a wait waits for: any one of them ends it.
#[derive(Default)]
pub(crate) struct Until {
    statuses: Vec<String>,
    pub(crate) pattern: Option<regex::Regex>,
    prompt: bool,
    quiet_ms: Option<i64>,
    ended: bool,
}

impl Until {
    fn of(parameters: &BTreeMap<String, Value>) -> Result<Self, String> {
        let yes = |name: &str| matches!(text(parameters, name).as_str(), "true" | "Y");
        Self::parse(
            key_names(parameters.get("status")),
            &text(parameters, "pattern"),
            yes("prompt"),
            text(parameters, "quietMs").parse::<i64>().ok(),
            yes("ended"),
        )
    }

    /// A wait for any of: a status among `statuses`, a screen matching `pattern` (empty for none), a prompt mark, output
    /// quiet for `quiet_ms`, the end. Refused with none of them, or a pattern that is no regular expression.
    pub(crate) fn parse(
        statuses: Vec<String>,
        pattern: &str,
        prompt: bool,
        quiet_ms: Option<i64>,
        ended: bool,
    ) -> Result<Self, String> {
        let pattern = if pattern.is_empty() {
            None
        } else if pattern.len() > PATTERN_MAX {
            return Err(format!("A wait's pattern is at most {PATTERN_MAX} characters"));
        } else {
            Some(
                // ^ and $ at each line's ends: the screen is read line by line
                regex::RegexBuilder::new(pattern)
                    .multi_line(true)
                    .size_limit(1 << 20)
                    .build()
                    .map_err(|err| format!("The pattern is not a regular expression: {err}"))?,
            )
        };
        let until = Self {
            statuses,
            pattern,
            prompt,
            quiet_ms: quiet_ms.filter(|ms| *ms > 0),
            ended,
        };
        if until.statuses.is_empty()
            && until.pattern.is_none()
            && !until.prompt
            && until.quiet_ms.is_none()
            && !until.ended
        {
            return Err("A wait needs status, pattern, prompt, quietMs or ended".to_string());
        }
        Ok(until)
    }
}

/// The waits running now, by user.
fn waits() -> &'static Mutex<HashMap<String, usize>> {
    static WAITS: std::sync::LazyLock<Mutex<HashMap<String, usize>>> = std::sync::LazyLock::new(Default::default);
    &WAITS
}

/// A user's place among the waits, given back when dropped.
struct WaitSlot(String);

impl WaitSlot {
    fn take(username: &str) -> Option<Self> {
        let mut waits = lock(waits());
        let count = waits.entry(username.to_string()).or_insert(0);
        if *count >= WAITS_PER_USER {
            return None;
        }
        *count += 1;
        Some(Self(username.to_string()))
    }
}

impl Drop for WaitSlot {
    fn drop(&mut self) {
        let mut waits = lock(waits());
        if let Some(count) = waits.get_mut(&self.0) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                waits.remove(&self.0);
            }
        }
    }
}

/// `wait#TerminalSession`: until the session's status is one of `status`, its screen matches `pattern`, its shell
/// marks a `prompt`, its output is quiet for `quietMs`, or it `ended`; at most `timeoutSeconds` (300). Answers what
/// matched (`matched`, empty on a timeout) and the screen's last lines.
pub fn wait_terminal_session(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let (_registry, session) = match caller_and_target(parameters) {
        Ok(found) => found,
        Err(refused) => return refused,
    };
    let until = match Until::of(parameters) {
        Ok(until) => until,
        Err(why) => return refuse("badWait", &why),
    };
    let Some(_slot) = WaitSlot::take(&session.username) else {
        return refuse("limit", "At most 4 waits on terminal sessions at once");
    };
    let seconds = text(parameters, "timeoutSeconds")
        .parse::<f64>()
        .ok()
        .filter(|s| s.is_finite() && *s >= 0.0);
    let timeout = seconds.map_or(WAIT_MAX, Duration::from_secs_f64).min(WAIT_MAX);
    let deadline = Instant::now() + timeout;
    let from_event = session.last_event();
    let matched = loop {
        let screen = until
            .pattern
            .as_ref()
            .and_then(|_| read(&session, Scope::Last(usize::from(session.info().rows.max(1))), false));
        if let Some(matched) = matched(&session, &until, from_event, screen.as_ref()) {
            break matched;
        }
        if Instant::now() >= deadline {
            break "";
        }
        std::thread::sleep(WAIT_STEP.min(deadline.saturating_duration_since(Instant::now())));
    };
    let tail = read(&session, Scope::Last(TAIL), false);
    let mut value = about(&session, tail.as_ref());
    value["matched"] = serde_json::Value::String(matched.to_string());
    value["timedOut"] = serde_json::Value::Bool(matched.is_empty());
    value["lines"] = lines_json(tail.as_ref().map_or(&[][..], |read| &read.lines));
    out(value)
}

/// What of `until` holds now, if anything, `screen` being the session's last rows when it waits for a pattern; an
/// ended session ends any wait.
pub(crate) fn matched(
    session: &Session,
    until: &Until,
    from_event: u64,
    screen: Option<&Read>,
) -> Option<&'static str> {
    let info = session.info();
    let ended = info.ended.is_some();
    let status = if ended { "ended" } else { info.status.as_str() };
    if until.statuses.iter().any(|wanted| wanted == status) {
        return Some("status");
    }
    if until.prompt
        && session
            .events_after(from_event)
            .iter()
            .any(|event| event.kind == "prompt")
    {
        return Some("prompt");
    }
    if let Some(pattern) = &until.pattern {
        if screen.is_some_and(|read| pattern.is_match(&read.lines.join("\n"))) {
            return Some("pattern");
        }
    }
    if let Some(quiet) = until.quiet_ms {
        if info.last_output > 0 && now_millis() - info.last_output >= quiet {
            return Some("quiet");
        }
    }
    ended.then_some("ended")
}

#[cfg(test)]
mod tests {
    use super::key_names;
    use moqui_lib::object::Value;

    #[test]
    fn keys_come_as_a_list_or_as_text() {
        assert_eq!(key_names(Some(&Value::string("Enter, C-c"))), ["Enter", "C-c"]);
        assert_eq!(key_names(Some(&Value::string(""))), Vec::<String>::new());
        assert_eq!(key_names(None), Vec::<String>::new());
    }
}
