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

//! Recording a session (plans/Terminal-Fancy2.md §4.5, D2): the one exception to "what is shown is never recorded",
//! so it is opt-in, always shown and bounded. `terminal@recording` is `offer` (the owner starts and stops it, or a
//! profile's `record="true"` starts its sessions recorded), `always` (every host session from its start, which the owner
//! cannot stop) or `off`. A recording is an asciicast file under `runtime/terminal/recordings/<username>/`, `0600` in a
//! `0700` directory, owned by the server's OS user, and a `moqui.terminal.TerminalRecording` row. Only its owner plays
//! it; an administrator lists and deletes recordings and never plays them. They expire after `recording-days`, and one
//! stops at `recording-max-mb`. The server's pump writes a session in process, the holder a held one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_lib::object::{to_plain_string, Value};

use super::record::Keep;
use super::services::{answer, factory, refuse, text, username, Out};
use super::session::{Kind, Recording, Session};

const ENTITY: &str = "moqui.terminal.TerminalRecording";
const DAYS: usize = 30;
const MAX_MB: usize = 256;

/// What the conf says of recording: `offer` (the default), `always` or `off`; any other value is `off`, logged.
pub(crate) fn mode(factory: &ExecutionContextFactory) -> &'static str {
    match factory.conf_setting("terminal", "recording").trim() {
        "" | "offer" => "offer",
        "always" => "always",
        "off" => "off",
        other => {
            tracing::error!("terminal@recording {other:?} is not offer, always or off: recording is off");
            "off"
        }
    }
}

pub(super) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    moqui_core::rust_service!(map, find_terminal_recordings)?;
    moqui_core::rust_service!(map, get_terminal_recording)?;
    moqui_core::rust_service!(map, find_all_terminal_recordings)?;
    moqui_core::rust_service!(map, delete_terminal_recording)?;
    moqui_core::rust_service!(map, clean_terminal_recordings)?;
    Ok(())
}

/// The directory of `username`'s recordings.
fn dir_of(factory: &ExecutionContextFactory, username: &str) -> PathBuf {
    let safe: String = username
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '@') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    factory.runtime_dir().join("terminal").join("recordings").join(safe)
}

/// A file for a new recording of session `id`: `<id>-<n>.cast`, the next `n` there.
fn next_file(dir: &Path, id: &str) -> PathBuf {
    (1..)
        .map(|n| dir.join(format!("{id}-{n}.cast")))
        .find(|path| !path.exists())
        .unwrap_or_else(|| dir.join(format!("{id}.cast")))
}

/// The byte limit of a recording.
fn limit(factory: &ExecutionContextFactory) -> u64 {
    super::terminal_number(factory, "recording-max-mb", MAX_MB) as u64 * 1024 * 1024
}

/// Start or stop the recording of `session`, in the holder for a held session, else in its pump.
fn route(factory: &Arc<ExecutionContextFactory>, session: &Arc<Session>, keep: Option<Keep>) -> Result<(), String> {
    let (factory, session) = (Arc::clone(factory), Arc::clone(session));
    let done = crate::edge::block_on(async move {
        if session.held {
            super::held::Holder::of(&factory)
                .record(&session.id, keep.as_ref())
                .await
                .map_err(|err| err.to_string())
        } else {
            session.record(keep).await
        }
    });
    done.map_err(|err| err.message().to_string())?
}

/// Start recording `session`, `forced` by the conf; a row is written as the signed-in user's thread allows (authz off).
/// Refused when the conf has recording off, or for an apps session.
pub(crate) fn start(
    factory: &Arc<ExecutionContextFactory>,
    session: &Arc<Session>,
    forced: bool,
) -> Result<(), String> {
    if mode(factory) == "off" {
        return Err("Recording is off on this server".to_string());
    }
    if session.kind == Kind::Apps {
        return Err("An apps session is not recorded".to_string());
    }
    if session.recording().is_some() {
        return Ok(());
    }
    let path = next_file(&dir_of(factory, &session.username), &session.id);
    route(
        factory,
        session,
        Some(Keep::File {
            path: path.clone(),
            limit: limit(factory),
        }),
    )?;
    let id = moqui_core::user::random_string(22).map_err(|err| err.message().to_string())?;
    let location = path.to_string_lossy().into_owned();
    let written = ExecutionContext::with_current(|ec| -> Result<(), moqui_core::context::Error> {
        let _authz = ec.disable_authz();
        let facade = factory
            .entity()
            .ok_or_else(|| moqui_core::context::Error::new("no entity facade"))?;
        let mut row = facade.make_value(ENTITY)?;
        row.set("recordingId", Value::string(&id));
        row.set("terminalSessionId", Value::string(&session.id));
        row.set("username", Value::string(&session.username));
        row.set("title", Value::string(&session.info().title));
        row.set("location", Value::string(&location));
        row.set("fromDate", moqui_core::user::now_value());
        row.set("forced", Value::string(if forced { "Y" } else { "N" }));
        row.create(&facade)?;
        Ok(())
    });
    if let Some(Err(err)) = written {
        tracing::error!(
            "terminal session {}: its recording's row was not written: {}",
            session.id,
            err.message()
        );
    }
    tracing::info!(
        "terminal session {} of {} recorded{} to {location}",
        session.id,
        session.username,
        if forced {
            " (the conf's recording=\"always\")"
        } else {
            ""
        }
    );
    session.set_recording(Some(Recording { id, location, forced }));
    Ok(())
}

/// Stop recording `session`; a forced recording is not stopped.
pub(crate) fn stop(factory: &Arc<ExecutionContextFactory>, session: &Arc<Session>) -> Result<(), String> {
    let Some(recording) = session.recording() else {
        return Ok(());
    };
    if recording.forced {
        return Err("This server records every session: its recording cannot be stopped".to_string());
    }
    route(factory, session, None)?;
    session.set_recording(None);
    if let Some(facade) = factory.entity() {
        close_rows(&facade, &session.id);
    }
    tracing::info!("terminal session {}: its recording stopped", session.id);
    Ok(())
}

/// Start the recording an opening asks for: forced by `recording="always"` for a host session, or the profile's.
pub(crate) fn at_open(factory: &Arc<ExecutionContextFactory>, session: &Arc<Session>, profile_records: bool) {
    let forced = mode(factory) == "always" && session.kind == Kind::Host;
    if forced || (profile_records && mode(factory) == "offer") {
        if let Err(err) = start(factory, session, forced) {
            tracing::error!("terminal session {}: its recording did not start: {err}", session.id);
        }
    }
}

/// Close the open rows of session `id`: their end and the size of their file.
fn close_rows(facade: &EntityFacade, id: &str) {
    let closed = (|| -> Result<(), moqui_core::context::Error> {
        let rows = facade
            .find(ENTITY)?
            .eq("terminalSessionId", Value::string(id))
            .eq("thruDate", Value::Null)
            .disable_authz()
            .list()?;
        for mut row in rows.values().iter().cloned() {
            let location = row.get("location").map(to_plain_string).unwrap_or_default();
            let bytes = std::fs::metadata(&location).map_or(0, |meta| meta.len());
            row.set("thruDate", moqui_core::user::now_value());
            row.set("bytes", Value::I64(i64::try_from(bytes).unwrap_or(i64::MAX)));
            row.update(facade)?;
        }
        Ok(())
    })();
    if let Err(err) = closed {
        tracing::error!(
            "terminal session {id}: its recordings' rows were not closed: {}",
            err.message()
        );
    }
}

/// Close session `id`'s recording rows once it ended, on the request pool.
pub(crate) fn close_on_end(factory: Weak<ExecutionContextFactory>, id: String) {
    crate::edge::handle().spawn(async move {
        let row_id = id.clone();
        let _ = crate::worker::on_pool_without_authz(&factory, move |factory| {
            if let Some(facade) = factory.entity() {
                close_rows(&facade, &row_id);
            }
            Ok(())
        })
        .await;
    });
}

/// A held session taken back after a restart: its recording, which the holder kept writing, is shown again from its
/// open row (on the request pool).
pub(crate) fn reload(factory: Weak<ExecutionContextFactory>, session: Arc<Session>) {
    crate::edge::handle().spawn(async move {
        let id = session.id.clone();
        let found = crate::worker::on_pool_without_authz(&factory, move |factory| {
            let facade = factory
                .entity()
                .ok_or_else(|| moqui_core::context::Error::new("the entity facade is not installed"))?;
            let row = facade
                .find(ENTITY)?
                .eq("terminalSessionId", Value::string(&id))
                .eq("thruDate", Value::Null)
                .order_by("fromDate", true)
                .limit(1)
                .disable_authz()
                .list()?
                .values()
                .first()
                .cloned();
            Ok(row.map(|row| {
                let text = |key: &str| row.get(key).map(to_plain_string).unwrap_or_default();
                Recording {
                    id: text("recordingId"),
                    location: text("location"),
                    forced: text("forced") == "Y",
                }
            }))
        })
        .await;
        match found {
            Ok(Some(recording)) => session.set_recording(Some(recording)),
            Ok(None) => {}
            Err(err) => tracing::error!(
                "terminal session {}: its recording was not read: {}",
                session.id,
                err.message()
            ),
        }
    });
}

/// A recording's row as a list shows it.
fn listed(row: &moqui_core::entity::EntityValue) -> serde_json::Value {
    let text = |key: &str| row.get(key).map(to_plain_string).unwrap_or_default();
    serde_json::json!({
        "recordingId": text("recordingId"), "sessionId": text("terminalSessionId"), "username": text("username"),
        "title": text("title"), "fromDate": text("fromDate"), "thruDate": text("thruDate"),
        "bytes": row.get("bytes").map(to_plain_string).and_then(|bytes| bytes.parse::<i64>().ok()),
        "forced": text("forced") == "Y",
    })
}

fn rows(facade: &EntityFacade, username: Option<&str>) -> Result<Vec<serde_json::Value>, moqui_core::context::Error> {
    let mut find = facade
        .find(ENTITY)?
        .order_by("fromDate", true)
        .limit(500)
        .disable_authz();
    if let Some(username) = username {
        find = find.eq("username", Value::string(username));
    }
    Ok(find.list()?.values().iter().map(listed).collect())
}

/// `find#TerminalRecordings`: the caller's own recordings, newest first.
pub fn find_terminal_recordings(_parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let username = match username() {
        Ok(username) => username,
        Err(refused) => return refused,
    };
    answer("recordings", &serde_json::Value::Array(rows(facade, Some(&username))?))
}

/// `get#TerminalRecording`: where one of the caller's own recordings is, for the Terminal to play or download; nobody
/// else's, an administrator's included (D2).
pub fn get_terminal_recording(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let username = match username() {
        Ok(username) => username,
        Err(refused) => return refused,
    };
    let row = facade
        .find(ENTITY)?
        .eq("recordingId", Value::string(text(parameters, "recordingId")))
        .disable_authz()
        .one()?
        .filter(|row| row.get("username").map(to_plain_string).as_deref() == Some(username.as_str()));
    let Some(row) = row else {
        return refuse("notFound", "No such recording");
    };
    let location = row.get("location").map(to_plain_string).unwrap_or_default();
    if !Path::new(&location).is_file() {
        return refuse("notFound", "That recording's file is gone");
    }
    let mut out = BTreeMap::from([("location".to_string(), Value::string(format!("file:{location}")))]);
    out.insert("recording".to_string(), moqui_lib::json::from_json(&listed(&row)));
    Ok(out)
}

/// The System app's authorization for an administrator's action on terminals.
fn may_administer(action: &str) -> bool {
    moqui_core::user::authorize_own("AT_XML_SCREEN", "component://tools/screen/System.xml", action).is_ok()
}

/// `find#AllTerminalRecordings`: every user's recordings (System › Terminal Sessions), never their contents.
pub fn find_all_terminal_recordings(_parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !may_administer("view") {
        return refuse("notAllowed", "Not allowed");
    }
    answer("recordings", &serde_json::Value::Array(rows(facade, None)?))
}

/// Remove a recording's file and row.
fn remove(facade: &EntityFacade, row: &moqui_core::entity::EntityValue) -> Result<(), moqui_core::context::Error> {
    let location = row.get("location").map(to_plain_string).unwrap_or_default();
    if !location.is_empty() {
        if let Err(err) = std::fs::remove_file(&location) {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::error!("the recording {location} was not removed: {err}");
            }
        }
    }
    row.delete(facade)?;
    Ok(())
}

/// `delete#TerminalRecording`: an administrator removes a recording, file and row.
pub fn delete_terminal_recording(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !may_administer("update") {
        return refuse("notAllowed", "Not allowed");
    }
    let id = text(parameters, "recordingId");
    let Some(row) = facade
        .find(ENTITY)?
        .eq("recordingId", Value::string(&id))
        .disable_authz()
        .one()?
    else {
        return refuse("notFound", "No such recording");
    };
    remove(facade, &row)?;
    tracing::info!(
        "terminal recording {id} of {} deleted by an administrator",
        row.get("username").map(to_plain_string).unwrap_or_default()
    );
    Ok(BTreeMap::new())
}

/// `clean#TerminalRecordings`: the recordings older than `recording-days`, file and row; each one that fails is
/// logged and the rest go on.
pub fn clean_terminal_recordings(_parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let factory = factory()?;
    let days = super::terminal_number(&factory, "recording-days", DAYS) as i64;
    let cutoff = moqui_core::user::now_millis() - days * 86_400_000;
    let old = facade
        .find(ENTITY)?
        .condition(moqui_core::entity::Condition::less(
            "fromDate",
            moqui_core::user::stamp_value(cutoff),
        ))
        .disable_authz()
        .list()?
        .values()
        .to_vec();
    let mut removed = 0_i64;
    for row in &old {
        match remove(facade, row) {
            Ok(()) => removed += 1,
            Err(err) => tracing::error!("an old terminal recording was not removed: {}", err.message()),
        }
    }
    Ok(BTreeMap::from([("removed".to_string(), Value::I64(removed))]))
}
