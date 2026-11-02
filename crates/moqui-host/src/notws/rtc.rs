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
//! `hand` and `reaction`, `ended`, the notices `recording` (W5, `rtc/recorder.rs`) and `captions` (W6), which a leg
//! that joins later gets right after its `joined`, and the captions' lines, `caption` (`rtc/captions.rs`).
//!
//! A `join` runs `CallServices.bind#Connection` as the socket's user on a request worker: the leg must be theirs and in
//! the call. That claims the leg's next connection generation, which every later frame of this socket carries, so the
//! media node refuses an older socket's frames. Frames of a leg this socket has not bound are refused here. The frames
//! go to the call's media node, on this server or another (`rtc-signal`, to that node only). While the socket is open
//! its bindings are kept alive (`heartbeat#Connections`, every 20 seconds); when it closes they are released
//! (`release#Connection`) and the leg leaves after the reconnect grace unless a new socket binds it.
//!
//! With cascading media nodes (W7) `bind#Connection` answers the leg's media node (this one, when it holds media) and
//! the call's; the frames go to the leg's, which links the room to the call's node. The `rtc-signal` messages between
//! two media nodes of a room, `relay` and `roomFrame`, go to the driver here.

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
const GUEST_BIND_SERVICE: &str = "org.moqui.impl.CallServices.bind#GuestConnection";
const GUEST_LEAVE_SERVICE: &str = "org.moqui.impl.CallServices.leave#AsGuest";
const GUEST_STATE_SERVICE: &str = "org.moqui.impl.CallServices.get#GuestState";
/// A guest's socket must send its credential this soon.
const GUEST_AUTH_WAIT: Duration = Duration::from_secs(10);
/// A guest's credential is checked again this often; one that no longer works closes the socket.
const GUEST_CHECK: Duration = Duration::from_secs(30);
const HEARTBEAT_SERVICE: &str = "org.moqui.impl.CallServices.heartbeat#Connections";
const RELEASE_SERVICE: &str = "org.moqui.impl.CallServices.release#Connection";
const HEARTBEAT_EVERY: Duration = Duration::from_secs(20);
/// A frame larger than this is refused.
const MAX_FRAME: usize = 128 * 1024;
/// The bindings one socket may hold (one per call it is in).
const MAX_BINDINGS: usize = 4;

static SIGNAL: OnceLock<moqui_core::notify::Subscription> = OnceLock::new();

/// A leg this socket bound, with the generation it claimed, the node holding its media and the call's media node (W7:
/// another when the leg's is a leg node).
#[derive(Clone, Debug)]
struct Binding {
    call: String,
    leg: String,
    generation: u64,
    media_node: String,
    origin_node: String,
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
        "join" => join(factory, slot, Who::User(slot.user_id.clone()), call, leg, id, frame),
        "leave" => leave(factory, slot, Who::User(slot.user_id.clone()), call, leg, id, frame),
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

/// Who a socket binds a leg as: its signed-in user, or a guest by their credential.
#[derive(Clone)]
enum Who {
    User(String),
    Guest(String),
}

/// Runs `work` as `who`: logged in as the user, or as internal work for a guest (the guest services check the
/// credential), with service authorization off either way.
async fn as_who<T: Send + 'static>(
    factory: &Arc<ExecutionContextFactory>,
    who: &Who,
    work: impl FnOnce() -> Result<T, moqui_core::context::Error> + Send + 'static,
) -> Result<T, moqui_core::context::Error> {
    match who {
        Who::User(user_id) => as_user(factory, user_id.clone(), work).await,
        Who::Guest(_) => {
            super::on_pool(factory, move || {
                let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
                work()
            })
            .await
        }
    }
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

fn join(
    factory: &Arc<ExecutionContextFactory>,
    slot: &Arc<Slot>,
    who: Who,
    call: String,
    leg: String,
    id: Json,
    frame: Json,
) {
    let factory = Arc::clone(factory);
    let slot = Arc::clone(slot);
    tokio::spawn(async move {
        let own = crate::rtc::own_node(&factory);
        let mut parameters = BTreeMap::from([
            ("callSessionId".to_string(), Value::string(&call)),
            ("callLegId".to_string(), Value::string(&leg)),
            ("signalingNodeId".to_string(), Value::string(&own)),
        ]);
        let service = match &who {
            Who::User(_) => BIND_SERVICE,
            Who::Guest(credential) => {
                parameters.insert("credential".to_string(), Value::string(credential));
                GUEST_BIND_SERVICE
            }
        };
        let bound = as_who(&factory, &who, move || Ok(call_service(service, parameters))).await;
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
        let origin_node = out
            .get("originNodeId")
            .map(to_plain_string)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| media_node.clone());
        let binding = Binding {
            call: call.clone(),
            leg: leg.clone(),
            generation,
            media_node,
            origin_node,
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

fn leave(
    factory: &Arc<ExecutionContextFactory>,
    slot: &Arc<Slot>,
    who: Who,
    call: String,
    leg: String,
    id: Json,
    frame: Json,
) {
    let factory = Arc::clone(factory);
    let slot = Arc::clone(slot);
    tokio::spawn(async move {
        if let Some(binding) = bound(slot.id, &call, &leg) {
            forward(&factory, &slot, &binding, &Json::Null, frame);
        }
        let (service, parameters) = match &who {
            Who::User(_) => (
                LEAVE_SERVICE,
                BTreeMap::from([
                    ("callSessionId".to_string(), Value::string(&call)),
                    ("callLegId".to_string(), Value::string(&leg)),
                ]),
            ),
            Who::Guest(credential) => (
                GUEST_LEAVE_SERVICE,
                BTreeMap::from([("credential".to_string(), Value::string(credential))]),
            ),
        };
        let left = as_who(&factory, &who, move || Ok(call_service(service, parameters))).await;
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
            origin: binding.origin_node.clone(),
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
            "callSessionId": binding.call, "callLegId": binding.leg, "generation": binding.generation,
            "originNode": binding.origin_node, "frame": frame}),
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

/// A reply of the edge node `edge`'s media driver for a socket here (plans/Edge-Node.md §5.4, E7), from the link. A
/// compromised edge node must not reach any socket of this server, so it is delivered only to a socket that bound a
/// leg of that call whose media is on `edge`: the frame names the call (`callSessionId`, topic `rtc:<call>`) and the
/// leg (`to`, else `callLegId`), and the message targets this node.
#[cfg_attr(not(feature = "edge"), allow(dead_code))]
pub(crate) fn deliver_edge_reply(edge: &str, message: &TopicMessage) -> Result<(), String> {
    let Some(factory) = lock(factory_ref()).upgrade() else {
        return Err("the server is stopping".to_string());
    };
    if edge.is_empty() || !moqui_core::cluster::for_this_member(&factory, message) {
        return Err("a reply for another node".to_string());
    }
    let body = &message.body;
    let (Some(socket), Some(frame)) = (body.get("socketId").and_then(Json::as_u64), body.get("frame")) else {
        return Err("a reply without its socket or frame".to_string());
    };
    let text = |k: &str| frame.get(k).and_then(Json::as_str).unwrap_or_default().to_string();
    let call = text("callSessionId");
    let leg = frame
        .get("to")
        .and_then(Json::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| text("callLegId"));
    if call.is_empty() || leg.is_empty() || text("topic") != format!("rtc:{call}") {
        return Err("a reply that names no call and leg".to_string());
    }
    let bound = lock(bindings()).get(&socket).is_some_and(|list| {
        list.iter()
            .any(|b| b.call == call && b.leg == leg && b.media_node == edge)
    });
    if !bound {
        return Err(format!(
            "no socket here bound leg {leg} of call {call} with its media on the edge node"
        ));
    }
    super::push_to_socket(socket, frame.to_string());
    Ok(())
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
                origin: text("originNode"),
                frame: body.get("frame").cloned().unwrap_or(Json::Null),
            };
            if let Err(code) = crate::rtc::command(command) {
                tracing::debug!("call frame for {} not taken: {code}", text("callSessionId"));
            }
        }
        // W7: the SFU of another media node of the room, about their relay link
        Some("relay") => {
            let message = body
                .get("message")
                .cloned()
                .map(serde_json::from_value::<moqui_rtc::RelayMessage>);
            let Some(Ok(message)) = message else {
                tracing::debug!("call relay message for {} not read", text("callSessionId"));
                return;
            };
            let command = Command::Relay {
                room: text("callSessionId"),
                from: text("from"),
                message: Box::new(message),
            };
            if let Err(code) = crate::rtc::command(command) {
                tracing::debug!("call relay message for {} not taken: {code}", text("callSessionId"));
            }
        }
        Some("roomFrame") => {
            let notice = body
                .get("notice")
                .and_then(Json::as_str)
                .and_then(crate::rtc::NoticeKind::parse)
                .map(|kind| (kind, body.get("keep").and_then(Json::as_bool).unwrap_or(false)));
            let command = Command::RoomFrame {
                room: text("callSessionId"),
                from: text("from"),
                except: body.get("except").and_then(Json::as_str).map(str::to_string),
                frame: body.get("frame").cloned().unwrap_or(Json::Null),
                notice,
            };
            if let Err(code) = crate::rtc::command(command) {
                tracing::debug!("call room frame for {} not taken: {code}", text("callSessionId"));
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
                Some("link") => Command::Link {
                    room: text("callSessionId"),
                    origin: text("originNodeId"),
                },
                // captions run beside the driver, not in it (rtc/captions.rs)
                Some("startCaptions") => {
                    let options = crate::rtc::captions::StartOptions {
                        user_id: Some(text("userId")).filter(|id| !id.is_empty()),
                        provider_label: text("provider"),
                        local: body.get("local").and_then(Json::as_bool).unwrap_or(false),
                    };
                    crate::rtc::captions::start(&factory, &text("callSessionId"), options);
                    return;
                }
                Some("stopCaptions") => {
                    crate::rtc::captions::stop(&text("callSessionId"));
                    return;
                }
                Some("recording") => {
                    crate::rtc::recorder::control(&factory, body);
                    return;
                }
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

// ---------- the guest socket (W3) ----------

/// The call and leg a guest's socket may signal for, and its credential.
struct GuestScope {
    call: String,
    leg: String,
    credential: String,
}

/// `GET /callws`: the socket of a guest's page (the /call/<token> page), with no session. Its first frame is
/// `{rtc: 'guestAuth', credential}`, within 10 seconds; the credential (from `join#AsGuest`) is checked with
/// `get#GuestState` and names one call and leg. After that the socket takes only `rtc` frames of that call and leg:
/// no topics, no co-editing, nothing else. The credential is checked again every 30 seconds; when it no longer works
/// (revoked, expired, the guest denied or removed) the page is told the call ended and the socket closes.
/// Adds `/callws` to the routes.
pub(crate) fn guest_route(app: axum::Router<Arc<crate::http::HttpState>>) -> axum::Router<Arc<crate::http::HttpState>> {
    app.route("/callws", axum::routing::get(guest_upgrade).connect(guest_upgrade))
}

pub(crate) async fn guest_upgrade(
    ws: axum::extract::ws::WebSocketUpgrade,
    headers: axum::http::HeaderMap,
    site: super::RequestSite,
    axum::extract::State(state): axum::extract::State<Arc<crate::http::HttpState>>,
) -> axum::response::Response {
    if !super::origin_allowed(&state.factory, &headers, &site, "call") {
        return crate::http::plain(403, "origin not allowed");
    }
    let factory = Arc::clone(&state.factory);
    let shutdown = state.shutdown.clone();
    ws.max_message_size(MAX_FRAME)
        .max_frame_size(MAX_FRAME)
        .on_upgrade(move |socket| run_guest(socket, factory, shutdown))
}

/// The guest's state by credential: (admission, callSessionId, callLegId), or None when the check could not run.
async fn guest_state(factory: &Arc<ExecutionContextFactory>, credential: &str) -> Option<(String, String, String)> {
    let parameters = BTreeMap::from([("credential".to_string(), Value::string(credential))]);
    let out = as_who(factory, &Who::Guest(credential.to_string()), move || {
        Ok(call_service(GUEST_STATE_SERVICE, parameters))
    })
    .await;
    let out = match out {
        Ok(Ok(out)) => out,
        _ => return None,
    };
    let text = |k: &str| out.get(k).map(to_plain_string).unwrap_or_default();
    Some((text("admission"), text("callSessionId"), text("callLegId")))
}

async fn run_guest(
    mut socket: axum::extract::ws::WebSocket,
    factory: Arc<ExecutionContextFactory>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    use axum::extract::ws::Message;
    let first = tokio::time::timeout(GUEST_AUTH_WAIT, socket.recv()).await;
    let credential = match first {
        Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str::<Json>(&text)
            .ok()
            .filter(|f| f.get("rtc").and_then(Json::as_str) == Some("guestAuth"))
            .and_then(|f| f.get("credential").and_then(Json::as_str).map(str::to_string))
            .filter(|c| !c.is_empty() && c.len() <= 100),
        _ => None,
    };
    let Some(credential) = credential else {
        return refuse_guest(socket, "badRequest").await;
    };
    let Some((admission, call, leg)) = guest_state(&factory, &credential).await else {
        return refuse_guest(socket, "unavailable").await;
    };
    if !matches!(admission.as_str(), "admitted" | "waiting") || call.is_empty() || leg.is_empty() {
        return refuse_guest(socket, "notFound").await;
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel(super::SOCKET_QUEUE);
    let id = super::NEXT_SOCKET.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // a guest is no user: its id matches no notification's recipients, and it has no topics
    let slot = Arc::new(Slot {
        id,
        user_id: format!("guest:{call}:{leg}"),
        session_id: "guest".to_string(),
        topics: Mutex::new(std::collections::HashSet::new()),
        tx,
        overflow: tokio::sync::Notify::new(),
    });
    super::register(&slot);
    let scope = GuestScope {
        call: call.clone(),
        leg: leg.clone(),
        credential: credential.clone(),
    };
    let authed = json!({"topic": format!("rtc:{call}"), "rtc": "guestAuthed", "callSessionId": call, "callLegId": leg,
        "admission": admission});
    let mut open = socket.send(Message::Text(authed.to_string().into())).await.is_ok();
    let mut check = tokio::time::interval(GUEST_CHECK);
    check.tick().await;
    let mut ping = tokio::time::interval(super::PING_EVERY);
    ping.tick().await;
    while open {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            () = slot.overflow.notified() => break,
            _ = check.tick() => {
                let still = guest_state(&factory, &scope.credential).await;
                if !matches!(still, Some((ref a, _, _)) if a == "admitted" || a == "waiting") {
                    let ended = json!({"topic": format!("rtc:{}", scope.call), "rtc": "ended", "callSessionId": scope.call,
                        "callLegId": scope.leg, "reason": "removed"});
                    let _ = socket.send(Message::Text(ended.to_string().into())).await;
                    break;
                }
            }
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(text))) => guest_frame(&factory, &slot, &scope, &text),
                Some(Ok(Message::Ping(payload))) => open = socket.send(Message::Pong(payload)).await.is_ok(),
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            push = rx.recv() => {
                let Some(push) = push else { break };
                open = socket.send(Message::Text(push.text.into())).await.is_ok();
            }
            _ = ping.tick() => open = socket.send(Message::Ping(Vec::new().into())).await.is_ok(),
        }
    }
    super::unregister(id);
    socket_closed(&factory, id);
}

/// Tells a guest's page its link does not work, and closes the socket.
async fn refuse_guest(mut socket: axum::extract::ws::WebSocket, code: &str) {
    use axum::extract::ws::{CloseFrame, Message};
    let frame = json!({"rtc": "error", "code": code, "message": "this link no longer works"});
    let _ = socket.send(Message::Text(frame.to_string().into())).await;
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code: 1008,
            reason: "refused".into(),
        })))
        .await;
}

/// A guest's frame: only `rtc` frames of its own call and leg.
fn guest_frame(factory: &Arc<ExecutionContextFactory>, slot: &Arc<Slot>, scope: &GuestScope, text: &str) {
    let Ok(frame) = serde_json::from_str::<Json>(text) else {
        return;
    };
    let Some(op) = frame.get("rtc").and_then(Json::as_str).map(str::to_string) else {
        return;
    };
    let id = frame.get("id").cloned().unwrap_or(Json::Null);
    let call = frame.get("callSessionId").and_then(Json::as_str).unwrap_or_default();
    let leg = frame.get("callLegId").and_then(Json::as_str).unwrap_or_default();
    if call != scope.call || leg != scope.leg {
        error(
            slot,
            &scope.call,
            &scope.leg,
            &id,
            "notAllowed",
            "a guest signals only for their own call",
        );
        return;
    }
    let who = Who::Guest(scope.credential.clone());
    match op.as_str() {
        "join" => join(factory, slot, who, scope.call.clone(), scope.leg.clone(), id, frame),
        "leave" => leave(factory, slot, who, scope.call.clone(), scope.leg.clone(), id, frame),
        _ => match bound(slot.id, &scope.call, &scope.leg) {
            Some(binding) => forward(factory, slot, &binding, &id, frame),
            None => error(
                slot,
                &scope.call,
                &scope.leg,
                &id,
                "notJoined",
                "join the call on this connection first",
            ),
        },
    }
}
