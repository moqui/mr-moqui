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

//! Call signaling on the notification socket (plans/Comms-Framework.md §6.3). A client sends JSON frames with `rtc`:
//! `join` (with its offer), `offer`, `answer`, `candidate`, `publish`, `unpublish`, `subscribe`, `mute`, `hand`,
//! `reaction` and `leave`, each with `callSessionId` and `callLegId` and an optional `id` that the reply carries. Replies
//! and the media node's offers and events come back on the topic `rtc:<callSessionId>`: `joined`, `answer`, `offer`,
//! `ack`, `error` (`code`), `connected`, `mediaLost`, `participant`, `track`, `layer`, `quality`, the relayed `mute`,
//! `hand` and `reaction`, and `ended`.
//!
//! A `join` runs `CallServices.bind#Connection` as the socket's user on a request worker: the leg must be theirs and in
//! the call. That claims the leg's next connection generation, which every later frame of this socket carries, so the
//! media node refuses an older socket's frames. Frames of a leg this socket has not bound are refused here. The frames
//! go to the call's media node, on this server or another (`rtc-signal`, to that node only). While the socket is open
//! its bindings are kept alive (`heartbeat#Connections`, every 20 seconds); when it closes they are released
//! (`release#Connection`) and the leg leaves after the reconnect grace unless a new socket binds it.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::notify::{TopicMessage, RTC_SIGNAL};
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;
use serde_json::{json, Value as Json};

use super::{Push, Slot};
use crate::rtc::{Command, Route};

const BIND_SERVICE: &str = "org.moqui.impl.CallServices.bind#Connection";
const LEAVE_SERVICE: &str = "org.moqui.impl.CallServices.leave#Call";
const HEARTBEAT_SERVICE: &str = "org.moqui.impl.CallServices.heartbeat#Connections";
const RELEASE_SERVICE: &str = "org.moqui.impl.CallServices.release#Connection";
const HEARTBEAT_EVERY: Duration = Duration::from_secs(20);
/// A frame larger than this is refused.
const MAX_FRAME: usize = 128 * 1024;
/// The bindings one socket may hold (one per call it is in).
const MAX_BINDINGS: usize = 4;

static SIGNAL: OnceLock<moqui_core::notify::Subscription> = OnceLock::new();

/// A leg this socket bound, with the generation it claimed and the node holding its media.
#[derive(Clone, Debug)]
struct Binding {
    call: String,
    leg: String,
    generation: u64,
    media_node: String,
}

fn bindings() -> &'static Mutex<HashMap<u64, Vec<Binding>>> {
    static BINDINGS: OnceLock<Mutex<HashMap<u64, Vec<Binding>>>> = OnceLock::new();
    BINDINGS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn factory_ref() -> &'static Mutex<Weak<ExecutionContextFactory>> {
    static FACTORY: OnceLock<Mutex<Weak<ExecutionContextFactory>>> = OnceLock::new();
    FACTORY.get_or_init(|| Mutex::new(Weak::new()))
}

fn send(slot: &Slot, frame: Json) {
    let _ = slot.tx.try_send(Push {
        text: frame.to_string(),
        message_id: String::new(),
    });
}

fn error(slot: &Slot, call: &str, leg: &str, id: &Json, code: &str, message: &str) {
    send(
        slot,
        json!({"topic": format!("rtc:{call}"), "rtc": "error", "callSessionId": call, "callLegId": leg, "id": id,
            "code": code, "message": message}),
    );
}

/// A frame from a client: true when it was a call signaling frame (handled), false for anything else.
pub(super) fn handle(factory: &Arc<ExecutionContextFactory>, slot: &Arc<Slot>, text: &str) -> bool {
    if !text.trim_start().starts_with('{') || !text.contains("\"rtc\"") {
        return false;
    }
    let Ok(frame) = serde_json::from_str::<Json>(text) else {
        return false;
    };
    let Some(op) = frame.get("rtc").and_then(Json::as_str).map(str::to_string) else {
        return false;
    };
    let call = frame
        .get("callSessionId")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string();
    let leg = frame
        .get("callLegId")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string();
    let id = frame.get("id").cloned().unwrap_or(Json::Null);
    if call.is_empty() || leg.is_empty() || call.len() > 64 || leg.len() > 64 {
        error(
            slot,
            &call,
            &leg,
            &id,
            "badRequest",
            "a call frame names its callSessionId and callLegId",
        );
        return true;
    }
    if text.len() > MAX_FRAME {
        error(slot, &call, &leg, &id, "tooLarge", "the frame is too large");
        return true;
    }
    match op.as_str() {
        "join" => join(factory, slot, call, leg, id, frame),
        "leave" => leave(factory, slot, call, leg, id, frame),
        _ => {
            let Some(binding) = bound(slot.id, &call, &leg) else {
                error(
                    slot,
                    &call,
                    &leg,
                    &id,
                    "notJoined",
                    "join the call on this connection first",
                );
                return true;
            };
            forward(factory, slot, &binding, &id, frame);
        }
    }
    true
}

fn bound(slot_id: u64, call: &str, leg: &str) -> Option<Binding> {
    lock(bindings())
        .get(&slot_id)?
        .iter()
        .find(|b| b.call == call && b.leg == leg)
        .cloned()
}

/// Runs `work` on a request worker logged in as the socket's user, with service authorization off: the call services
/// it runs check the user's part in the call.
async fn as_user<T: Send + 'static>(
    factory: &Arc<ExecutionContextFactory>,
    user_id: String,
    work: impl FnOnce() -> Result<T, moqui_core::context::Error> + Send + 'static,
) -> Result<T, moqui_core::context::Error> {
    super::on_pool(factory, move || {
        let username = {
            let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
            ExecutionContext::with_current(|ec| {
                ec.factory()
                    .entity()?
                    .find("moqui.security.UserAccount")
                    .ok()?
                    .eq("userId", Value::string(&user_id))
                    .one()
                    .ok()
                    .flatten()
                    .and_then(|row| row.get("username").map(to_plain_string))
            })
            .flatten()
        };
        let username = username.ok_or_else(|| moqui_core::context::Error::new("the socket's user is gone"))?;
        moqui_core::user::internal_login_user(&username)?;
        // the call services check the user's part in the call themselves, as for the Calls screens' transitions
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        work()
    })
    .await
}

/// Calls a service and takes its errors: the answer, or the reason code and message of its refusal.
fn call_service(name: &str, parameters: BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>, (String, String)> {
    let called = moqui_core::service::sync(name, &parameters);
    let errors = ExecutionContext::with_current(|ec| {
        let errors = ec.message().errors().join("; ");
        ec.message().clear_errors();
        errors
    })
    .unwrap_or_default();
    match called {
        Ok(out) if errors.is_empty() => Ok(out),
        Ok(out) => {
            let code = out.get("reasonCode").map(to_plain_string).filter(|c| !c.is_empty());
            Err((code.unwrap_or_else(|| "refused".to_string()), errors))
        }
        Err(err) => Err(("failed".to_string(), err.message().to_string())),
    }
}

fn join(factory: &Arc<ExecutionContextFactory>, slot: &Arc<Slot>, call: String, leg: String, id: Json, frame: Json) {
    let factory = Arc::clone(factory);
    let slot = Arc::clone(slot);
    tokio::spawn(async move {
        let own = crate::rtc::own_node(&factory);
        let (c, l, node) = (call.clone(), leg.clone(), own.clone());
        let bound = as_user(&factory, slot.user_id.clone(), move || {
            Ok(call_service(
                BIND_SERVICE,
                BTreeMap::from([
                    ("callSessionId".to_string(), Value::string(&c)),
                    ("callLegId".to_string(), Value::string(&l)),
                    ("signalingNodeId".to_string(), Value::string(&node)),
                ]),
            ))
        })
        .await;
        let out = match bound {
            Ok(Ok(out)) => out,
            Ok(Err((code, message))) => return error(&slot, &call, &leg, &id, &code, &message),
            Err(err) => return error(&slot, &call, &leg, &id, "unavailable", err.message()),
        };
        let generation = out
            .get("generation")
            .and_then(|v| to_plain_string(v).parse::<u64>().ok())
            .unwrap_or(0);
        let media_node = out.get("mediaNodeId").map(to_plain_string).unwrap_or_default();
        if generation == 0 || media_node.is_empty() {
            return error(&slot, &call, &leg, &id, "failed", "the call has no media server");
        }
        let binding = Binding {
            call: call.clone(),
            leg: leg.clone(),
            generation,
            media_node,
        };
        {
            let mut all = lock(bindings());
            let list = all.entry(slot.id).or_default();
            list.retain(|b| !(b.call == call && b.leg == leg));
            if list.len() >= MAX_BINDINGS {
                drop(all);
                return error(
                    &slot,
                    &call,
                    &leg,
                    &id,
                    "tooManyCalls",
                    "this connection is in too many calls",
                );
            }
            list.push(binding.clone());
        }
        forward(&factory, &slot, &binding, &id, frame);
    });
}

fn leave(factory: &Arc<ExecutionContextFactory>, slot: &Arc<Slot>, call: String, leg: String, id: Json, frame: Json) {
    let factory = Arc::clone(factory);
    let slot = Arc::clone(slot);
    tokio::spawn(async move {
        if let Some(binding) = bound(slot.id, &call, &leg) {
            forward(&factory, &slot, &binding, &Json::Null, frame);
        }
        let (c, l) = (call.clone(), leg.clone());
        let left = as_user(&factory, slot.user_id.clone(), move || {
            Ok(call_service(
                LEAVE_SERVICE,
                BTreeMap::from([
                    ("callSessionId".to_string(), Value::string(&c)),
                    ("callLegId".to_string(), Value::string(&l)),
                ]),
            ))
        })
        .await;
        if let Some(list) = lock(bindings()).get_mut(&slot.id) {
            list.retain(|b| !(b.call == call && b.leg == leg));
        }
        match left {
            Ok(Ok(_)) => send(
                &slot,
                json!({"topic": format!("rtc:{call}"), "rtc": "ack", "callSessionId": call, "callLegId": leg, "id": id}),
            ),
            Ok(Err((code, message))) => error(&slot, &call, &leg, &id, &code, &message),
            Err(err) => error(&slot, &call, &leg, &id, "unavailable", err.message()),
        }
    });
}

/// Sends a bound leg's frame to its media node, here or by `rtc-signal`.
fn forward(factory: &Arc<ExecutionContextFactory>, slot: &Slot, binding: &Binding, id: &Json, frame: Json) {
    let own = crate::rtc::own_node(factory);
    if binding.media_node == own {
        let command = Command::Frame {
            route: Route {
                node: own,
                socket_id: slot.id,
            },
            room: binding.call.clone(),
            leg: binding.leg.clone(),
            generation: binding.generation,
            frame,
        };
        if let Err(code) = crate::rtc::command(command) {
            error(
                slot,
                &binding.call,
                &binding.leg,
                id,
                code,
                "the call's media server is not available",
            );
        }
        return;
    }
    let Some(facade) = factory.entity() else { return };
    let message = TopicMessage {
        origin_id: moqui_core::notify::origin_id().to_string(),
        message_id: moqui_core::entity::uuid_v4().unwrap_or_default(),
        body: json!({"target": binding.media_node, "kind": "frame", "origin": own, "socketId": slot.id,
            "callSessionId": binding.call, "callLegId": binding.leg, "generation": binding.generation, "frame": frame}),
    };
    let (node, call, leg, id) = (
        binding.media_node.clone(),
        binding.call.clone(),
        binding.leg.clone(),
        id.clone(),
    );
    let slot_id = slot.id;
    // the member's address may be read from the database: not on the socket's task
    tokio::task::spawn_blocking(move || {
        if let Err(err) = moqui_core::cluster::send_to(&facade, &node, RTC_SIGNAL, &message) {
            let code = if err.message() == "overloaded" {
                "overloaded"
            } else {
                "unavailable"
            };
            let frame = json!({"topic": format!("rtc:{call}"), "rtc": "error", "callSessionId": call, "callLegId": leg,
                "id": id, "code": code, "message": "the call's media server is not available"});
            super::push_to_socket(slot_id, frame.to_string());
        }
    });
}

/// The socket closed: its bindings are released, and the legs leave after the reconnect grace.
pub(super) fn socket_closed(factory: &Arc<ExecutionContextFactory>, slot_id: u64) {
    let Some(list) = lock(bindings()).remove(&slot_id) else {
        return;
    };
    if list.is_empty() {
        return;
    }
    let factory = Arc::clone(factory);
    tokio::spawn(async move {
        let released = super::on_pool(&factory, move || {
            let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
            for b in list {
                let parameters = BTreeMap::from([
                    ("callLegId".to_string(), Value::string(&b.leg)),
                    (
                        "generation".to_string(),
                        Value::I64(i64::try_from(b.generation).unwrap_or(0)),
                    ),
                ]);
                if let Err((_, message)) = call_service(RELEASE_SERVICE, parameters) {
                    tracing::warn!("call leg {} connection not released: {message}", b.leg);
                }
            }
            Ok(())
        })
        .await;
        if let Err(err) = released {
            tracing::warn!("call connections not released: {}", err.message());
        }
    });
}

/// A message on `rtc-signal` for this node: a frame for the media here, a reply for a socket here, or a control
/// message of the call services.
fn deliver_signal(message: &TopicMessage) {
    let Some(factory) = lock(factory_ref()).upgrade() else {
        return;
    };
    if !moqui_core::cluster::for_this_member(&factory, message) {
        return;
    }
    let body = &message.body;
    let text = |k: &str| body.get(k).and_then(Json::as_str).unwrap_or_default().to_string();
    match body.get("kind").and_then(Json::as_str) {
        Some("reply") => {
            if let (Some(socket), Some(frame)) = (body.get("socketId").and_then(Json::as_u64), body.get("frame")) {
                super::push_to_socket(socket, frame.to_string());
            }
        }
        Some("frame") => {
            let route = Route {
                node: text("origin"),
                socket_id: body.get("socketId").and_then(Json::as_u64).unwrap_or(0),
            };
            let command = Command::Frame {
                route,
                room: text("callSessionId"),
                leg: text("callLegId"),
                generation: body.get("generation").and_then(Json::as_u64).unwrap_or(0),
                frame: body.get("frame").cloned().unwrap_or(Json::Null),
            };
            if let Err(code) = crate::rtc::command(command) {
                tracing::debug!("call frame for {} not taken: {code}", text("callSessionId"));
            }
        }
        Some("control") => {
            let command = match body.get("action").and_then(Json::as_str) {
                Some("closeLeg") => Command::CloseLeg {
                    room: text("callSessionId"),
                    leg: text("callLegId"),
                },
                Some("closeRoom") => Command::CloseRoom {
                    room: text("callSessionId"),
                },
                _ => return,
            };
            if let Err(code) = crate::rtc::command(command) {
                tracing::debug!("call control for {} not taken: {code}", text("callSessionId"));
            }
        }
        _ => {}
    }
}

/// Subscribes to `rtc-signal` and starts the heartbeat of this server's bound legs.
pub(crate) fn install(factory: &Arc<ExecutionContextFactory>, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    *lock(factory_ref()) = Arc::downgrade(factory);
    if SIGNAL.get().is_none() {
        match moqui_core::notify::subscribe(RTC_SIGNAL, Arc::new(deliver_signal)) {
            Ok(subscription) => {
                let _ = SIGNAL.set(subscription);
            }
            Err(err) => tracing::error!("calls: the rtc-signal topic was not subscribed: {}", err.message()),
        }
    }
    let factory = Arc::clone(factory);
    tokio::spawn(async move {
        let mut every = tokio::time::interval(HEARTBEAT_EVERY);
        loop {
            tokio::select! {
                _ = every.tick() => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
            }
            let list: Vec<Value> = lock(bindings())
                .values()
                .flatten()
                .map(|b| moqui_lib::json::from_json(&json!({"callLegId": b.leg, "generation": b.generation})))
                .collect();
            if list.is_empty() {
                continue;
            }
            let beat = super::on_pool(&factory, move || {
                let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
                call_service(
                    HEARTBEAT_SERVICE,
                    BTreeMap::from([("connectionList".to_string(), Value::List(list))]),
                )
                .map(|_| ())
                .map_err(|(_, message)| moqui_core::context::Error::new(message))
            })
            .await;
            if let Err(err) = beat {
                tracing::warn!("call connections not kept alive: {}", err.message());
            }
        }
    });
}
