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

//! `attach <id>` at the SSH prompt (plans/Terminal-Fancy.md §15): the prompt's stream becomes the client of one of the
//! user's kept sessions, as a page's socket on `/termws?session=` is, until Ctrl+\ detaches it, the session ends or
//! another window takes it. The session goes on after a detach. A command line or a host session asks what attaching
//! to it from the browser asks (§8.2), by the SSH peer's address. Over SSH only: in the browser the Terminal's session
//! list does this, and a command line attached inside its own stream would feed itself.

use std::sync::{Arc, Weak};

use moqui_core::context::ExecutionContextFactory;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::host::HostAccess;
use crate::term::access;
use crate::term::registry::Registry;
use crate::term::session::{Kind, Out, Session};

/// The key that detaches (Ctrl+\, as in dtach and abduco). Ctrl+Q is the apps' own quit, and ssh takes `~.` itself.
const DETACH: u8 = 0x1c;

/// What a session may have left on in the user's terminal, put back as the prompt's: the main screen, the cursor
/// shown, no mouse reports, normal cursor keys and keypad.
const RESET_MODES: &[u8] = b"\x1b[?1049l\x1b[?25h\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1l\x1b>";

/// The user's session that `id` names: the whole id, or the start of one id only. `Err` says why there is none.
fn find(factory: &Arc<ExecutionContextFactory>, id: &str, username: &str) -> Result<Arc<Session>, String> {
    let registry = Registry::of(factory);
    if let Some(session) = registry.find(id, username) {
        return Ok(session);
    }
    let mut starting: Vec<Arc<Session>> = registry
        .list(username)
        .into_iter()
        .filter(|session| !id.is_empty() && session.id.starts_with(id))
        .collect();
    match starting.len() {
        1 => Ok(starting.remove(0)),
        0 => Err(format!("no such session {id}; sessions lists them")),
        _ => Err(format!("{id} starts more than one session; type more of it")),
    }
}

/// The ids `attach` completes.
pub(crate) fn session_ids(factory: &Weak<ExecutionContextFactory>, username: &str) -> Vec<String> {
    factory
        .upgrade()
        .map(|factory| {
            Registry::of(&factory)
                .list(username)
                .iter()
                .map(|session| session.id.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether the user may attach to a session of `kind` from where the prompt was opened: run on the pool as the user.
async fn may_attach(access: &HostAccess, kind: Kind) -> Result<(), String> {
    if kind == Kind::Apps {
        return Ok(());
    }
    let username = access.username.clone();
    let facts = access.facts.clone();
    let checked = crate::worker::on_pool(&access.factory, move |factory| {
        moqui_core::user::internal_login_user(&username)?;
        let permitted = |permission: &str| moqui_core::user::has_permission(permission).unwrap_or(false);
        // the SSH sign-in is the recent one, as for `sh`
        Ok(access::check(access::Switches::of(factory), kind, &facts, &permitted, &|_| true))
    })
    .await;
    match checked {
        Ok(Ok(())) => Ok(()),
        Ok(Err(refusal)) => Err(refusal.message().to_string()),
        Err(err) => Err(err.message().to_string()),
    }
}

/// `attach <id>`: the stream is the session's client until a detach, its end, or another window taking it. Returns
/// what the prompt prints after.
pub(crate) async fn run<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    stream: &mut S,
    access: &mut HostAccess,
    id: &str,
) -> String {
    if !access.facts.ssh {
        return "attach: over SSH only; the Terminal's session list shows a session here".to_string();
    }
    if id.is_empty() {
        return "attach: which session? sessions lists them".to_string();
    }
    let Some(factory) = access.factory.upgrade() else {
        return String::new();
    };
    // a session the holder kept from the server before is found once it is taken back
    if Registry::of(&factory).find(id, &access.username).is_none() {
        crate::term::held::Holder::of(&factory)
            .adopt(Arc::downgrade(&factory))
            .await;
    }
    let kept = match find(&factory, id, &access.username) {
        Ok(kept) => kept,
        Err(reason) => return format!("attach: {reason}"),
    };
    drop(factory);
    if let Err(reason) = may_attach(access, kept.kind).await {
        tracing::info!("attach {} by {} over SSH refused: {reason}", kept.id, access.username);
        return format!("attach: {reason}");
    }
    let (cols, rows) = *access.size.borrow_and_update();
    let Some(attached) = kept.attach(None, cols, rows).await else {
        return format!("attach: {} has ended", kept.id);
    };
    tracing::info!("{} attached {} over SSH", access.username, kept.id);
    let client = attached.client;
    let mut out = attached.out;
    // how far this client has drawn, from the session's count at the attach: acknowledged so it is never paused
    let mut drawn: u64 = 0;
    let mut ended: Option<String> = None;
    let mut reason = String::new();
    let mut detached = false;
    let mut from_stream = [0u8; 4096];
    let mut sizing = true;
    loop {
        tokio::select! {
            next = out.recv() => match next {
                Some(Out::Bytes(bytes)) => {
                    if stream.write_all(&bytes).await.is_err() {
                        break;
                    }
                    drawn += bytes.len() as u64;
                    kept.ack(client, drawn).await;
                }
                Some(Out::Text(text)) => {
                    let Ok(message) = serde_json::from_str::<serde_json::Value>(&text) else {
                        continue;
                    };
                    match message["type"].as_str() {
                        Some("attached") => drawn = message["from"].as_u64().unwrap_or(0),
                        Some("ended") => ended = Some(message["status"].to_string()),
                        _ => {}
                    }
                }
                Some(Out::Close(why)) => {
                    reason = why;
                    break;
                }
                None => {
                    reason = "the window fell behind".to_string();
                    break;
                }
            },
            read = stream.read(&mut from_stream) => match read {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let typed = &from_stream[..read];
                    let (before, detach) = match typed.iter().position(|byte| *byte == DETACH) {
                        Some(at) => (&typed[..at], true),
                        None => (typed, false),
                    };
                    if !before.is_empty() {
                        // the program no longer reads: its end comes through `out`
                        let _ = kept.write(before.to_vec()).await;
                    }
                    if detach {
                        detached = true;
                        break;
                    }
                }
            },
            changed = access.size.changed(), if sizing => {
                if changed.is_ok() {
                    let (cols, rows) = *access.size.borrow_and_update();
                    kept.resize(client, cols, rows).await;
                } else {
                    sizing = false;
                }
            }
        }
    }
    kept.detach(client).await;
    let _ = stream.write_all(RESET_MODES).await;
    if detached {
        format!("\ndetached; attach {} to come back", kept.id)
    } else if let Some(status) = ended {
        format!("\n{} ended ({status})", kept.id)
    } else if reason.is_empty() {
        String::new()
    } else {
        format!("\n{}: {reason}", kept.id)
    }
}
