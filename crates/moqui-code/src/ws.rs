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

//! The `/agentws` frame protocol (plans/agent-code/09-Wire-API.md section 2), with no transport: the host's route
//! (`crates/moqui-host/src/agentws.rs`) reads frames from the socket, hands each text frame to [`Session::on_text`]
//! and each wake of the change feed to [`Session::on_wake`], and writes the frames they return. Both run in an
//! execution context the host entered as the socket's user, one per call, as a REST call does.
//!
//! Frames (JSON text):
//!
//! | From | Frame |
//! |---|---|
//! | server | `{"hello":{environmentId, protocol, serverVersion, capabilities, user}}`, first |
//! | client | `{"id":7,"m":"<method>","p":{..}}` request; with `"s":true` it opens a stream |
//! | server | `{"id":7,"r":{..}}` answer, `{"id":7,"e":{code, message, requiredScope, requiredPermission}}` error |
//! | server | `{"id":8,"i":{..}}` stream item, `{"id":8,"end":true}` stream end |
//! | client | `{"id":8,"cancel":true}` ends a stream |
//! | both | `{"ping":n}` and `{"pong":n}` |
//!
//! Limits kept from T3: [`MAX_STREAMS`] open streams per socket and a request frame of at most [`MAX_FRAME`] bytes.

use std::collections::BTreeMap;
use std::path::Path;

use moqui_core::entity::EntityFacade;
use serde_json::{json, Value};

use crate::change::Cursor;
use crate::command::{self, CommandError};
use crate::scopes::{method_spec, Grant, Refusal};
use crate::support::has_permission;
use crate::{environment, settings, shell};

/// The most streams one socket holds open.
pub const MAX_STREAMS: usize = 64;
/// The largest request frame; uploads go over HTTP.
pub const MAX_FRAME: usize = 1024 * 1024;
/// The interval of the server's ping, in seconds.
pub const PING_SECONDS: u64 = 25;
/// A socket silent for this long is closed, in seconds.
pub const SILENT_SECONDS: u64 = 60;
/// The most sockets one user holds on one device.
pub const MAX_SOCKETS_PER_DEVICE: usize = 4;
/// The server's queue for one socket; above it the socket closes with 1008.
pub const MAX_QUEUE_BYTES: usize = 8 * 1024 * 1024;

/// What the socket needs from the host.
pub struct Env<'a> {
    pub facade: &'a EntityFacade,
    pub runtime: &'a Path,
}

/// The protocol check of the upgrade (T3 rules): a server newer than the client does not support it; a server older
/// needs an update.
///
/// # Errors
///
/// The close reason, when the versions differ.
pub fn check_protocol(client: Option<u32>) -> Result<(), String> {
    match client {
        None => Ok(()),
        Some(version) if version == crate::PROTOCOL => Ok(()),
        Some(version) if version < crate::PROTOCOL => Err(format!(
            "client not supported: protocol {version}, this server speaks {}",
            crate::PROTOCOL
        )),
        Some(version) => Err(format!(
            "needs a newer server: the client speaks protocol {version}, this server {}",
            crate::PROTOCOL
        )),
    }
}

/// An error frame waiting for its request id.
type ErrorFrame = Box<dyn Fn(&Value) -> String>;

enum Stream {
    Shell(Cursor),
}

/// One socket's state.
pub struct Session {
    pub user_id: String,
    pub grant: Grant,
    streams: BTreeMap<u64, Stream>,
}

fn error_frame(id: &Value, code: &str, message: &str) -> String {
    json!({"id": id, "e": {"code": code, "message": message}}).to_string()
}

fn refusal_frame(id: &Value, refusal: &Refusal) -> String {
    json!({"id": id, "e": {
        "code": "forbidden",
        "message": format!("this needs scope {} and permission {}", refusal.required_scope.name(), refusal.required_permission),
        "requiredScope": refusal.required_scope.name(),
        "requiredPermission": refusal.required_permission,
    }})
    .to_string()
}

fn command_error_frame(id: &Value, err: &CommandError) -> String {
    let mut e = json!({"code": err.code, "message": err.message});
    if let Some(permission) = err.required_permission {
        e["requiredPermission"] = json!(permission);
    }
    json!({"id": id, "e": e}).to_string()
}

fn answer(id: &Value, result: &Value) -> String {
    json!({"id": id, "r": result}).to_string()
}

impl Session {
    #[must_use]
    pub fn new(user_id: impl Into<String>, grant: Grant) -> Self {
        Self {
            user_id: user_id.into(),
            grant,
            streams: BTreeMap::new(),
        }
    }

    /// The first frame.
    ///
    /// # Errors
    ///
    /// When the environment id cannot be had.
    pub fn hello(&self, env: &Env<'_>) -> Result<String, moqui_lib::error::Error> {
        Ok(json!({"hello": {
            "environmentId": environment::environment_id(env.facade, env.runtime)?,
            "protocol": crate::PROTOCOL,
            "serverVersion": env!("CARGO_PKG_VERSION"),
            "capabilities": environment::capabilities(),
            "user": {"userId": self.user_id},
        }})
        .to_string())
    }

    /// The open streams.
    #[must_use]
    pub fn stream_count(&self) -> usize {
        self.streams.len()
    }

    /// The frames that answer one text frame.
    pub fn on_text(&mut self, env: &Env<'_>, text: &str) -> Vec<String> {
        if text.len() > MAX_FRAME {
            return vec![error_frame(&Value::Null, "frame_too_large", "a request frame is at most 1 MiB")];
        }
        let Ok(frame) = serde_json::from_str::<Value>(text) else {
            return vec![error_frame(&Value::Null, "bad_frame", "the frame is not JSON")];
        };
        if let Some(n) = frame.get("ping") {
            return vec![json!({"pong": n}).to_string()];
        }
        if frame.get("pong").is_some() {
            return Vec::new();
        }
        let id = frame.get("id").cloned().unwrap_or(Value::Null);
        let Some(stream_id) = id.as_u64() else {
            return vec![error_frame(&id, "bad_frame", "the frame needs a numeric id")];
        };
        if frame.get("cancel").and_then(Value::as_bool) == Some(true) {
            return match self.streams.remove(&stream_id) {
                Some(_) => vec![json!({"id": id, "end": true}).to_string()],
                None => Vec::new(),
            };
        }
        let Some(method) = frame.get("m").and_then(Value::as_str) else {
            return vec![error_frame(&id, "bad_frame", "the frame needs a method m")];
        };
        let Some(spec) = method_spec(method) else {
            return vec![error_frame(&id, "unknown_method", &format!("{method} is not a method"))];
        };
        if let Err(refusal) = self.grant.check(spec, has_permission) {
            return vec![refusal_frame(&id, &refusal)];
        }
        let params = frame.get("p").cloned().unwrap_or_else(|| json!({}));
        let wants_stream = frame.get("s").and_then(Value::as_bool) == Some(true);
        if spec.stream != wants_stream {
            let text = if spec.stream { "opens a stream: send s:true" } else { "is not a stream" };
            return vec![error_frame(&id, "bad_frame", &format!("{method} {text}"))];
        }
        if spec.stream {
            if self.streams.contains_key(&stream_id) {
                return vec![error_frame(&id, "bad_frame", "the stream id is open")];
            }
            if self.streams.len() >= MAX_STREAMS {
                return vec![error_frame(&id, "too_many_streams", "a socket holds at most 64 streams")];
            }
            return self.open_stream(env, stream_id, &id, method, &params);
        }
        match self.call(env, method, &params) {
            Ok(result) => vec![answer(&id, &result)],
            Err(frame) => vec![frame(&id)],
        }
    }

    fn call(&self, env: &Env<'_>, method: &str, params: &Value) -> Result<Value, ErrorFrame> {
        let failed = |err: moqui_lib::error::Error| -> ErrorFrame {
            let text = err.to_string();
            Box::new(move |id| error_frame(id, "failed", &text))
        };
        let invalid = |text: &'static str| -> ErrorFrame {
            Box::new(move |id| error_frame(id, "invalid_params", text))
        };
        match method {
            "server.getDescriptor" => environment::descriptor(env.facade, env.runtime).map_err(failed),
            "server.getConfig" => Ok(json!({
                "environmentId": environment::environment_id(env.facade, env.runtime).map_err(failed)?,
                "protocol": crate::PROTOCOL,
                "user": {"userId": self.user_id},
                "permissions": {
                    "use": has_permission(crate::USE_PERMISSION),
                    "host": has_permission(crate::HOST_PERMISSION),
                    "admin": has_permission(crate::ADMIN_PERMISSION),
                },
                "clientSettings": settings::client_settings(),
            })),
            "orchestration.dispatchCommand" => {
                let command = params.get("command").unwrap_or(params);
                command::dispatch(env.facade, &self.user_id, command, false)
                    .map(|receipt| receipt.to_json())
                    .map_err(|err| -> ErrorFrame {
                        Box::new(move |id| command_error_frame(id, &err))
                    })
            }
            "orchestration.getShellSnapshot" => shell::snapshot(env.facade, &self.user_id).map_err(failed),
            "settings.getClient" => Ok(Value::Object(settings::client_settings())),
            "settings.updateClient" => {
                let patch = params.get("patch").and_then(Value::as_object).ok_or_else(|| invalid("p.patch is an object"))?;
                settings::set_client(patch).map_err(failed)?;
                Ok(Value::Object(settings::client_settings()))
            }
            "settings.getServer" => Ok(Value::Object(settings::server_settings(env.facade))),
            "settings.updateServer" => {
                let patch = params.get("patch").and_then(Value::as_object).ok_or_else(|| invalid("p.patch is an object"))?;
                for (name, value) in patch {
                    settings::check(settings::Kind::Server, name, value)
                        .or_else(|err| if value.is_null() { Ok(()) } else { Err(err) })
                        .map_err(|err| failed(moqui_lib::error::Error::new(err.0)))?;
                }
                crate::support::in_tx(env.facade, settings::SETTING, || {
                    for (name, value) in patch {
                        settings::set_server(env.facade, name, value, &self.user_id)?;
                    }
                    Ok(())
                })
                .map_err(failed)?;
                Ok(Value::Object(settings::server_settings(env.facade)))
            }
            "settings.getSchema" => Ok(settings::schema_json()),
            _ => Err(Box::new(|id| error_frame(id, "unknown_method", "no handler for the method"))),
        }
    }

    fn open_stream(&mut self, env: &Env<'_>, stream_id: u64, id: &Value, method: &str, params: &Value) -> Vec<String> {
        match method {
            "orchestration.subscribeShell" => {
                let mut frames = Vec::new();
                let cursor = match params.get("afterSequence").and_then(Value::as_i64) {
                    Some(after) => Cursor::after(after),
                    None => match shell::snapshot(env.facade, &self.user_id) {
                        Ok(snapshot) => {
                            let sequence = snapshot.get("sequence").and_then(Value::as_i64).unwrap_or(0);
                            frames.push(json!({"id": id, "i": {"kind": "snapshot", "snapshot": snapshot}}).to_string());
                            Cursor::after(sequence)
                        }
                        Err(err) => return vec![error_frame(id, "failed", &err.to_string())],
                    },
                };
                self.streams.insert(stream_id, Stream::Shell(cursor));
                frames.extend(self.read_stream(env, stream_id));
                frames
            }
            _ => vec![error_frame(id, "unknown_method", "no stream for the method")],
        }
    }

    fn read_stream(&mut self, env: &Env<'_>, stream_id: u64) -> Vec<String> {
        let user_id = self.user_id.clone();
        let Some(Stream::Shell(cursor)) = self.streams.get_mut(&stream_id) else {
            return Vec::new();
        };
        let mut frames = Vec::new();
        loop {
            match cursor.read(env.facade, &user_id) {
                Ok((changes, more)) => {
                    for change in changes {
                        frames.push(json!({"id": stream_id, "i": {"kind": "change", "change": change.to_json()}}).to_string());
                    }
                    if !more {
                        break;
                    }
                }
                Err(err) => {
                    tracing::warn!("agent code: the shell stream of {user_id} was not read: {err}");
                    break;
                }
            }
            if frames.len() > 5000 {
                break;
            }
        }
        frames
    }

    /// The stream items after a wake (a change committed) or a ping.
    pub fn on_wake(&mut self, env: &Env<'_>) -> Vec<String> {
        let ids: Vec<u64> = self.streams.keys().copied().collect();
        ids.into_iter().flat_map(|id| self.read_stream(env, id)).collect()
    }
}

#[cfg(test)]
#[path = "ws_tests.rs"]
mod tests;
