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

//! `agent-apps` over SSH (plans/archive/Screen-TUI-Impl.md Phase 3c, docs/TUI.md "For agents"): the apps for a program, one
//! command line at a time (`moqui_tui::Agent`), on an `exec` channel with no pty.
//!
//! - `agent-apps open [path] [--size CxR] [--json]` starts a session the server keeps, and answers its id and the
//!   screen; `agent-apps <id> <command>` runs one command in it. So each step of an agent is one ssh command, and the
//!   agent holds no connection between steps. A session is found only among its own user's, ends on `close`, and ends
//!   after `IDLE` without a command.
//! - `agent-apps` alone reads command lines from the channel and answers each, until `close` or the end of input.
//!
//! Both need a user the SSH connection logged in (a key or a password), run every request as the user, and count
//! against `terminal` `max-sessions` and `terminal` `max-sessions-per-user` with `apps`. Exit status: 0 for an answer that ends
//! `--- ok`, 1 for `--- error`, 2 for a session that is not there or not allowed.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Instant;

use moqui_core::context::ExecutionContextFactory;
use moqui_tui::{Agent, AgentAnswer, AGENT_HELP};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::server::IDLE;
use crate::term::{SessionSlot, Sessions};
use crate::tui::HostLink;

const DEFAULT_PATH: &str = "/apps";
const DEFAULT_SIZE: (u16, u16) = (120, 40);
/// The longest command line the line mode keeps.
const MAX_LINE: usize = 64 * 1024;

/// One agent session the server holds between commands.
struct Held {
    username: String,
    agent: Agent<HostLink>,
    last_used: Instant,
    _slot: SessionSlot,
}

/// The sessions of `agent-apps open`, by id.
#[derive(Default)]
pub(super) struct AgentSessions {
    live: Mutex<HashMap<String, Arc<Mutex<Held>>>>,
}

impl AgentSessions {
    /// Drop the sessions no command has used for `IDLE`, which gives their slots back. One in use is left.
    fn reap(&self) {
        let mut live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        live.retain(|id, held| match held.try_lock() {
            Ok(held) => {
                let keep = held.last_used.elapsed() < IDLE;
                if !keep {
                    tracing::info!(
                        "SSH agent-apps session {id} of {} ended after {IDLE:?} unused",
                        held.username
                    );
                }
                keep
            }
            Err(_) => true,
        });
    }

    fn get(&self, id: &str, username: &str) -> Option<Arc<Mutex<Held>>> {
        let live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        let held = live.get(id)?;
        let mine = held.lock().unwrap_or_else(PoisonError::into_inner).username == username;
        mine.then(|| Arc::clone(held))
    }
}

/// What a channel's `agent-apps` needs from the connection.
pub(super) struct AgentAccess {
    pub factory: Weak<ExecutionContextFactory>,
    pub sessions: Arc<Sessions>,
    pub agents: Arc<AgentSessions>,
    pub username: String,
}

/// The options of `open`: a path, `--size CxR`, `--json`.
struct Open {
    path: String,
    size: (u16, u16),
    json: bool,
}

fn parse_open(words: &[&str]) -> Result<Open, String> {
    let mut open = Open {
        path: DEFAULT_PATH.to_string(),
        size: DEFAULT_SIZE,
        json: false,
    };
    let mut words = words.iter();
    while let Some(word) = words.next() {
        match *word {
            "--json" => open.json = true,
            "--size" => {
                let size = words.next().ok_or("--size takes <cols>x<rows>")?;
                open.size = size
                    .split_once('x')
                    .and_then(|(columns, rows)| Some((columns.parse().ok()?, rows.parse().ok()?)))
                    .filter(|(columns, rows): &(u16, u16)| (40..=500).contains(columns) && (10..=500).contains(rows))
                    .ok_or("--size is <cols>x<rows>, 40 to 500 by 10 to 500")?;
            }
            path if path.starts_with('/') => open.path = path.to_string(),
            other => return Err(format!("open takes a path, --size and --json, not {other}")),
        }
    }
    Ok(open)
}

/// A new agent, on a request worker of its own since the link blocks.
async fn new_agent(access: &AgentAccess, open: Open) -> Result<(Agent<HostLink>, SessionSlot), String> {
    let slot = access.sessions.take(&access.username)?;
    // agent-apps renders say where they come from; it writes with the authority of its own credentials, ungated.
    let link = HostLink::weak_as_user(Weak::clone(&access.factory), access.username.clone()).with_render_options(
        moqui_core::screen::RenderOptions {
            origin: moqui_core::screen::RenderOrigin::AgentApps,
            ..moqui_core::screen::RenderOptions::default()
        },
    );
    tokio::task::spawn_blocking(move || {
        let mut agent = Agent::open(link, &open.path, open.size.0, open.size.1);
        agent.set_json(open.json);
        (agent, slot)
    })
    .await
    .map_err(|_| "the session could not start".to_string())
}

fn status(answer: &AgentAnswer) -> u32 {
    u32::from(!answer.ok)
}

/// The answer to `open`: the id first, as a line of text or a `session` member of the JSON.
fn opened(id: &str, answer: &AgentAnswer) -> String {
    match serde_json::from_str::<serde_json::Value>(&answer.text) {
        Ok(mut value) if value.is_object() => {
            value["session"] = serde_json::json!(id);
            value.to_string()
        }
        _ => format!("session {id}\n{}", answer.text),
    }
}

/// One `agent-apps <args>` exec: its output and exit status. `None` when there are no arguments, for the line mode.
pub(super) async fn one_shot(access: &AgentAccess, args: &str) -> Option<(String, u32)> {
    let words: Vec<&str> = args.split_whitespace().collect();
    let first = *words.first()?;
    access.agents.reap();
    Some(match first {
        "help" | "--help" => (format!("{AGENT_HELP}\n\nagent-apps open [path] [--size CxR] [--json] starts a session; agent-apps <id> <command> runs one command in it.\n--- ok"), 0),
        "open" => match parse_open(&words[1..]) {
            Err(reason) => (format!("--- error {reason}"), 2),
            Ok(open) => match new_agent(access, open).await {
                Err(reason) => (format!("--- error {reason}"), 2),
                Ok((agent, slot)) => {
                    let id = match moqui_core::user::random_string(8) {
                        Ok(id) => id,
                        Err(err) => return Some((format!("--- error {}", err.message()), 2)),
                    };
                    let answer = agent.look();
                    let held = Held {
                        username: access.username.clone(),
                        agent,
                        last_used: Instant::now(),
                        _slot: slot,
                    };
                    access
                        .agents
                        .live
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(id.clone(), Arc::new(Mutex::new(held)));
                    tracing::info!("SSH agent-apps session {id} of {} started", access.username);
                    (opened(&id, &answer), status(&answer))
                }
            },
        },
        id => {
            let Some(held) = access.agents.get(id, &access.username) else {
                return Some((format!("--- error no session {id}; agent-apps open starts one"), 2));
            };
            let command = args.trim_start().strip_prefix(id).unwrap_or("").trim().to_string();
            let ran = tokio::task::spawn_blocking(move || {
                let mut held = held.lock().unwrap_or_else(PoisonError::into_inner);
                held.last_used = Instant::now();
                let answer = held.agent.run(&command);
                (answer, held.agent.closed())
            })
            .await;
            match ran {
                Ok((answer, closed)) => {
                    if closed {
                        access
                            .agents
                            .live
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .remove(id);
                        tracing::info!("SSH agent-apps session {id} of {} closed", access.username);
                    }
                    let code = status(&answer);
                    (answer.text, code)
                }
                Err(_) => (format!("--- error the command stopped in session {id}"), 1),
            }
        }
    })
}

/// The line mode: a session for this channel, one answer per command line, until `close`, the end of input, or `IDLE`
/// without a line. Returns the exit status.
pub(super) async fn lines<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S, access: &AgentAccess) -> u32 {
    let open = Open {
        path: DEFAULT_PATH.to_string(),
        size: DEFAULT_SIZE,
        json: false,
    };
    let (agent, slot) = match new_agent(access, open).await {
        Ok(started) => started,
        Err(reason) => {
            let _ = stream.write_all(format!("--- error {reason}\n").as_bytes()).await;
            return 2;
        }
    };
    tracing::info!("SSH agent-apps line session of {} started", access.username);
    let agent = Arc::new(Mutex::new(agent));
    let first = agent.lock().unwrap_or_else(PoisonError::into_inner).look();
    if stream.write_all(format!("{}\n", first.text).as_bytes()).await.is_err() {
        return 1;
    }
    let mut pending: Vec<u8> = Vec::new();
    let mut buffer = [0u8; 4096];
    let mut last = 0;
    'read: loop {
        let read = match tokio::time::timeout(IDLE, stream.read(&mut buffer)).await {
            Ok(Ok(0) | Err(_)) => break,
            Err(_) => {
                let _ = stream
                    .write_all(b"--- error closed after 10 minutes without a command\n")
                    .await;
                break;
            }
            Ok(Ok(read)) => read,
        };
        pending.extend_from_slice(&buffer[..read]);
        while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line).trim().to_string();
            if line.is_empty() {
                continue;
            }
            let agent = Arc::clone(&agent);
            let ran = tokio::task::spawn_blocking(move || {
                let mut agent = agent.lock().unwrap_or_else(PoisonError::into_inner);
                let answer = agent.run(&line);
                (answer, agent.closed())
            })
            .await;
            let Ok((answer, closed)) = ran else {
                last = 1;
                break 'read;
            };
            last = status(&answer);
            if stream.write_all(format!("{}\n", answer.text).as_bytes()).await.is_err() || closed {
                break 'read;
            }
        }
        if pending.len() > MAX_LINE {
            pending.clear();
            let _ = stream
                .write_all(format!("--- error a line is longer than {MAX_LINE} bytes\n").as_bytes())
                .await;
        }
    }
    drop(slot);
    tracing::info!("SSH agent-apps line session of {} ended", access.username);
    last
}

#[cfg(test)]
mod tests {
    use super::parse_open;

    #[test]
    fn open_takes_a_path_a_size_and_json() {
        let open = parse_open(&["/apps/system", "--size", "160x50", "--json"]).unwrap();
        assert_eq!(
            (open.path.as_str(), open.size, open.json),
            ("/apps/system", (160, 50), true)
        );
        assert!(parse_open(&["--size", "9x9"]).is_err());
        assert!(parse_open(&["apps"]).is_err());
    }
}
