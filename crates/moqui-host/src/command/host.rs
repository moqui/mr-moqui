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

//! `sh [profile]`, `profiles` and `sessions` at the `moqui>` prompt (plans/Terminal-Fancy.md §6): a program on the
//! server's operating system run on the prompt's own stream until it ends, then the prompt again. It takes
//! `TERMINAL_HOST` beside the prompt's `SSH_SHELL`, a dev instance and the rule of §8.2: in the browser by where the
//! command line was opened from, over SSH by the peer's address. Each run is an audit row and counts toward the host
//! limits.

use std::sync::Weak;

use moqui_core::context::{Error as CoreError, ExecutionContextFactory};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;

use crate::term::access::{self, Facts};
use crate::term::host::{self, Launch, Profile};
use crate::term::session::Kind;

/// What `sh` needs from the prompt's transport.
pub(crate) struct HostAccess {
    pub factory: Weak<ExecutionContextFactory>,
    pub username: String,
    /// The window's size, as it changes.
    pub size: watch::Receiver<(u16, u16)>,
    /// Where the prompt was opened from.
    pub facts: Facts,
    /// The id the program's environment names (`MOQUI_TERMINAL_SESSION`): the command line's.
    pub session_id: String,
}

/// The profiles this user may run, or why `sh` is not available: run on the pool as the user.
async fn allowed_profiles(access: &HostAccess) -> Result<Vec<Profile>, String> {
    let username = access.username.clone();
    let facts = access.facts.clone();
    let checked = crate::worker::on_pool(&access.factory, move |factory| {
        moqui_core::user::internal_login_user(&username)?;
        let permitted = |permission: &str| moqui_core::user::has_permission(permission).unwrap_or(false);
        // the prompt's own sign-in or attach was the recent one
        if let Err(refusal) = access::check(access::Switches::of(factory), Kind::Host, &facts, &permitted, &|_| true) {
            return Ok(Err(refusal));
        }
        let own = moqui_core::user::get_preference(host::PROFILES_PREFERENCE)
            .ok()
            .flatten();
        Ok(Ok(host::available(&factory.conf_text(), own.as_deref())))
    })
    .await;
    match checked {
        Ok(Ok(profiles)) => Ok(profiles),
        Ok(Err(refusal)) => Err(refusal.message().to_string()),
        Err(err) => Err(err.message().to_string()),
    }
}

/// `profiles`: the programs `sh` runs.
pub(crate) async fn profiles(access: &HostAccess) -> String {
    match allowed_profiles(access).await {
        Ok(profiles) => profiles
            .iter()
            .map(|profile| {
                let program = profile
                    .command
                    .clone()
                    .unwrap_or_else(|| "your login shell".to_string());
                format!("{:<16} {} ({program})", profile.name, profile.title)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Err(reason) => format!("profiles: {reason}"),
    }
}

/// The profile names `sh` completes.
pub(crate) async fn profile_names(access: &HostAccess) -> Vec<String> {
    allowed_profiles(access)
        .await
        .map(|profiles| profiles.into_iter().map(|profile| profile.name).collect())
        .unwrap_or_default()
}

/// `sessions`: the user's kept terminal sessions.
pub(crate) fn sessions(factory: &Weak<ExecutionContextFactory>, username: &str) -> String {
    let Some(factory) = factory.upgrade() else {
        return String::new();
    };
    let listed = crate::term::registry::Registry::of(&factory).list(username);
    if listed.is_empty() {
        return "no sessions".to_string();
    }
    listed
        .iter()
        .map(|session| {
            let info = session.info();
            let state = match &info.ended {
                Some((status, _)) => format!("ended {status}"),
                None if info.attached => "shown".to_string(),
                None => "running".to_string(),
            };
            format!(
                "{}  {:<8} {:<10} {}",
                session.id,
                session.kind.as_str(),
                state,
                info.title
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `sh [profile]`: run the program on `stream` until it ends. Returns its exit status, after a line of why when it
/// could not start.
pub(crate) async fn run<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    stream: &mut S,
    access: &mut HostAccess,
    name: &str,
    newline: &str,
) -> u32 {
    let name = if name.is_empty() { host::SHELL_PROFILE } else { name };
    let refused = |message: String| format!("sh: {message}{newline}");
    let profiles = match allowed_profiles(access).await {
        Ok(profiles) => profiles,
        Err(reason) => {
            let _ = stream.write_all(refused(reason).as_bytes()).await;
            return 1;
        }
    };
    let Some(profile) = profiles.into_iter().find(|profile| profile.name == name) else {
        let _ = stream
            .write_all(refused(format!("no profile {name}; profiles lists them")).as_bytes())
            .await;
        return 1;
    };
    let Some(factory) = access.factory.upgrade() else {
        return 1;
    };
    let slot = match host::HostSessions::of(&factory).take(&access.username) {
        Ok(slot) => slot,
        Err(reason) => {
            let _ = stream.write_all(refused(reason).as_bytes()).await;
            return 1;
        }
    };
    let size = *access.size.borrow_and_update();
    let launch = Launch::of(
        &factory,
        &profile,
        &access.session_id,
        &access.username,
        None,
        false,
        size,
    );
    let program = match launch.start() {
        Ok(program) => program,
        Err(err) => {
            let message = format!("{} did not start: {err}", launch.command_line());
            let _ = stream.write_all(refused(message).as_bytes()).await;
            return 1;
        }
    };
    let audit_id = crate::term::registry::new_id().unwrap_or_default();
    record(
        &access.factory,
        &access.username,
        &audit_id,
        &profile,
        &launch,
        &access.facts,
    )
    .await;
    tracing::info!(
        "sh {} for {} at the prompt: {}",
        profile.name,
        access.username,
        launch.command_line()
    );
    let crate::term::session::Program {
        mut output,
        mut input,
        mut resize,
        hang_up,
        kill,
        ended,
        ..
    } = program;
    let mut program_ended = false;
    let mut from_stream = [0u8; 4096];
    let mut from_program = [0u8; 16 * 1024];
    let mut sizing = true;
    loop {
        tokio::select! {
            read = output.read(&mut from_program) => match read {
                Ok(0) | Err(_) => {
                    program_ended = true;
                    break;
                }
                Ok(read) => {
                    if stream.write_all(&from_program[..read]).await.is_err() {
                        break;
                    }
                }
            },
            read = stream.read(&mut from_stream) => match read {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if input.write_all(&from_stream[..read]).await.is_err() {
                        break;
                    }
                }
            },
            changed = access.size.changed(), if sizing => {
                if changed.is_ok() {
                    let (cols, rows) = *access.size.borrow_and_update();
                    resize(cols, rows);
                } else {
                    sizing = false;
                }
            }
        }
    }
    drop(input);
    // the prompt's stream closed under it (its session ended): the program is asked to end, then made to
    if !program_ended {
        if let Some(hang_up) = hang_up {
            hang_up();
        }
    }
    let mut ended = ended;
    let ended = match tokio::time::timeout(std::time::Duration::from_secs(5), &mut ended).await {
        Ok(ended) => ended,
        Err(_) => {
            if let Some(kill) = kill {
                kill();
            }
            ended.await
        }
    };
    drop(slot);
    crate::term::audit::on_end(access.factory.clone(), audit_id)(ended.status, "the program ended".to_string());
    ended.status
}

/// The audit row of a run, as the user.
async fn record(
    factory: &Weak<ExecutionContextFactory>,
    username: &str,
    id: &str,
    profile: &Profile,
    launch: &Launch,
    facts: &Facts,
) {
    let (username, id, profile_name, command_line) = (
        username.to_string(),
        id.to_string(),
        profile.name.clone(),
        launch.command_line(),
    );
    let directory = launch.directory.to_string_lossy().into_owned();
    let facts = facts.clone();
    let recorded = crate::worker::on_pool(factory, move |_| {
        moqui_core::user::internal_login_user(&username)?;
        crate::term::audit::record_open(&crate::term::audit::Opened {
            id: &id,
            username: &username,
            kind: Kind::Host,
            profile: &profile_name,
            command_line: &command_line,
            directory: &directory,
            facts: &facts,
        });
        Ok::<(), CoreError>(())
    })
    .await;
    if let Err(err) = recorded {
        tracing::error!("the audit row of sh was not written: {}", err.message());
    }
}
