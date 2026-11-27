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

//! GroupMe's push channel (dev.groupme.com, the push tutorial): Bayeux long-polling over HTTPS at
//! `push.groupme.com/faye`.
//!
//! - **The coordinator** ([`Coordinator`], one a node, a thread of its own, started by the first GroupMe bridge's
//!   `start`): every [`Timing::pass`] it lists the GroupMe bridges in one query (no configuration read but for a
//!   bridge whose row changed: its `lastUpdatedStamp`), claims or renews the leases of all of them in one pass
//!   ([`claim_all`]: `moqui.edge.EdgeSenderLock` rows `groupme:<bridge>`, the replication sender's and the SIP trunks'
//!   lock table) and runs a session for each bridge it holds, at most [`Timing::node_max`] (the property
//!   `chat@groupme-node-max`, 50); a bridge another node holds runs nothing here. Its member name is the node's instance
//!   id and a token of its own, so an old coordinator's release never frees a newer one's lease. The lease lasts
//!   [`Timing::lease`] (120 s) and is renewed by the coordinator, apart from the sessions, so a long poll or a long
//!   catch-up never outlasts it; a coordinator whose claims fail stops its sessions once two thirds of the lease passed
//!   since the last one that went through. A panic in a pass is logged and the next pass runs.
//! - **A session** ([`Session::step`], one step at a time, so a test drives it with no thread): the token's user
//!   (`/users/me`), a handshake, a subscription to `/user/<id>` with `ext: {access_token, timestamp}`, then the catch-up
//!   ([`super::events::Inbound::sync`]) and `/meta/connect` long polls (at most 60 s and [`POLL_SLACK`]), each answer's
//!   messages worked in order. A new handshake every hour ("Signature must refresh every hour"), and when the server
//!   asks for one. The catch-up runs again every `syncMinutes`, or in a minute when a room did not catch up.
//! - **The status** is the session's alone (the adapter [`keeps_status`](super::GroupMeAdapter)): connected once
//!   subscribed; a refused token (`tokenRefused: ...`) at once; another failure (`connectFailed: ...`) from the third in
//!   a row; `roomLimit: ...` while a room was refused for the bridge's limit. Each is written only when the row says
//!   otherwise. A wait of 30 s or more is kept as the bridge's `retryDate`, so a node that takes the connection over
//!   waits it out.
//! - **Waits.** A refused token waits 5 minutes or a new token; another failure 2 s, twice as long each time up to 5
//!   minutes (a rate limit at least a minute).

use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use moqui_core::context::framework_thread;
use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::{Condition, EntityFacade};
use moqui_core::service::chat_bridge::{note_status, BridgeInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;
use serde_json::{json, Value as Json};

use super::api::{id_of, ApiError};
use super::events::{Inbound, Rooms};
use super::{Config, Inner};

const BRIDGE: &str = "moqui.chat.ChatBridge";
const LOCK: &str = "moqui.edge.EdgeSenderLock";
const RESUBSCRIBE: Duration = Duration::from_secs(3600);
const TOPICS_EVERY: Duration = Duration::from_secs(3600);
/// The catch-up again soon, when a room did not catch up.
const SYNC_AGAIN: Duration = Duration::from_secs(60);
const REFUSED_WAIT: Duration = Duration::from_secs(300);
const MAX_BACKOFF: Duration = Duration::from_secs(300);
/// A wait at least this long is kept as the bridge's `retryDate`.
const KEPT_WAIT: Duration = Duration::from_secs(30);
/// How far past the server's own timeout (at most 60 s) a long poll waits for its answer.
pub const POLL_SLACK: Duration = Duration::from_secs(15);
/// The bridge's last error when GroupMe refuses its token.
pub const REFUSED: &str = "tokenRefused: GroupMe refused the access token: get a new one (docs/interfaces/GroupMe.md)";
/// The bridge's last error while a room was refused for its limit.
pub const ROOM_LIMIT: &str =
    "roomLimit: The bridge's room limit (maxRooms, maxDirectRooms) is reached: new rooms are not made";

/// How a node's coordinator is timed: the defaults, or a test's.
#[derive(Clone, Debug)]
pub struct Timing {
    /// How often it lists the bridges and claims their leases.
    pub pass: Duration,
    /// How long a claim lasts.
    pub lease: Duration,
    /// The most bridges a node holds.
    pub node_max: usize,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            pass: Duration::from_secs(15),
            lease: Duration::from_secs(120),
            node_max: moqui_core::context::conf_env::setting("chat@groupme-node-max")
                .parse::<usize>()
                .ok()
                .filter(|n| *n > 0)
                .unwrap_or(50),
        }
    }
}

// ---- a bridge as its session sees it ----

/// A bridge as the coordinator last read it: its configuration, status, last error and `retryDate`.
#[derive(Clone, Debug)]
pub struct Slot {
    pub info: BridgeInfo,
    pub status: String,
    pub error: Option<String>,
    /// When its connection may try again (epoch milliseconds).
    pub retry: Option<i64>,
    pub stamp: Option<String>,
}

fn text(row: &moqui_core::entity::EntityValue, field: &str) -> Option<String> {
    row.get(field).map(to_plain_string).filter(|text| !text.is_empty())
}

/// Bridge `bridge_id` read whole (its `configJson` decrypted). None when there is no such bridge.
///
/// # Errors
///
/// When the row cannot be read.
pub fn slot_of(facade: &EntityFacade, bridge_id: &str) -> Result<Option<Slot>, LibError> {
    let row = facade
        .find(BRIDGE)?
        .eq("chatBridgeId", Value::string(bridge_id))
        .disable_authz()
        .one()?;
    Ok(row.map(|row| Slot {
        info: BridgeInfo {
            chat_bridge_id: bridge_id.to_string(),
            type_enum_id: text(&row, "typeEnumId").unwrap_or_default(),
            adapter_name: text(&row, "adapterName").unwrap_or_default(),
            name: text(&row, "name").unwrap_or_default(),
            config: text(&row, "configJson")
                .and_then(|config| serde_json::from_str(&config).ok())
                .unwrap_or(Json::Null),
        },
        status: text(&row, "statusId").unwrap_or_default(),
        error: text(&row, "lastErrorText"),
        retry: row.get("retryDate").and_then(moqui_core::user::value_millis),
        stamp: text(&row, "lastUpdatedStamp"),
    }))
}

/// Keep (or clear) bridge `bridge_id`'s `retryDate`.
fn set_retry(facade: &EntityFacade, bridge_id: &str, at: Option<i64>) {
    let value = at.map_or(Value::Null, moqui_core::user::stamp_value);
    let result = facade.find(BRIDGE).and_then(|find| {
        find.eq("chatBridgeId", Value::string(bridge_id))
            .disable_authz()
            .update_all(vec![("retryDate".to_string(), value)])
    });
    if let Err(error) = result {
        tracing::warn!("GroupMe bridge {bridge_id}: its retry time was not kept: {error}");
    }
}

fn now_millis() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

// ---- a session ----

/// What a step did, and how long the session waits before the next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Subscribed (and caught up): the events taken.
    Subscribed(usize),
    /// A long poll (or a catch-up) answered: the events taken. The next step follows at once.
    Polled(usize),
    /// A failure, or nothing to do: wait this long.
    Wait(Duration),
}

/// One bridge's connection to the push server. See the module notes.
#[derive(Debug)]
pub struct Session {
    fingerprint: String,
    own: Option<String>,
    client_id: Option<String>,
    next_id: u64,
    subscribed: Option<Instant>,
    next_sync: Option<Instant>,
    topics_synced: Option<Instant>,
    timeout: Duration,
    failures: u32,
    /// The bridge's retryDate was read (a hand-over waits it out).
    restored: bool,
    /// What the session last wrote, and when: not written again until the coordinator's read of the row agrees.
    noted: Option<((bool, Option<String>), Instant)>,
    rooms: Rooms,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            fingerprint: String::new(),
            own: None,
            client_id: None,
            next_id: 0,
            subscribed: None,
            next_sync: None,
            topics_synced: None,
            timeout: Duration::from_secs(30),
            failures: 0,
            restored: false,
            noted: None,
            rooms: Rooms::default(),
        }
    }
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the session is subscribed now.
    pub fn subscribed(&self) -> bool {
        self.client_id.is_some() && self.subscribed.is_some()
    }

    /// The configuration this session connected with.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Start again at the handshake (a new token, a lost connection), keeping what it wrote of the status.
    pub fn reset(&mut self) {
        let noted = self.noted.take();
        let restored = self.restored;
        *self = Self::default();
        self.noted = noted;
        self.restored = restored;
    }

    fn id(&mut self) -> String {
        self.next_id += 1;
        self.next_id.to_string()
    }

    /// Keep the bridge's status, unless its row (as the coordinator read it) says so already or this session wrote
    /// the same a moment ago.
    fn note(&mut self, facade: &EntityFacade, slot: &Slot, connected: bool, error: Option<String>) {
        let status = if connected { "CbsConnected" } else { "CbsNotConnected" };
        let row_says = slot.status == status && (error.is_none() || slot.error == error);
        let wanted = (connected, error);
        // a failure repeated before the coordinator reads the row again is not written again; connected is written
        // whenever the row says otherwise (a subscription, at most hourly), as a send may have changed it meanwhile
        if row_says
            || !connected
                && self
                    .noted
                    .as_ref()
                    .is_some_and(|(noted, at)| *noted == wanted && at.elapsed() < Duration::from_secs(60))
        {
            return;
        }
        note_status(facade, &slot.info.chat_bridge_id, wanted.0, wanted.1.as_deref());
        self.noted = Some((wanted, Instant::now()));
    }

    /// One step. See the module notes.
    pub fn step(&mut self, inner: &Inner, facade: &EntityFacade, slot: &Slot) -> Step {
        let config = Config::of(&slot.info.config);
        let fingerprint = config.fingerprint();
        if fingerprint != self.fingerprint {
            self.reset();
            self.fingerprint = fingerprint;
        }
        // a node that took the connection over waits out the last holder's wait
        if !self.restored {
            self.restored = true;
            if let Some(wait) = slot
                .retry
                .and_then(|at| u64::try_from(at - now_millis()).ok())
                .filter(|wait| *wait > 0)
            {
                self.failures = 3;
                return Step::Wait(Duration::from_millis(wait).min(REFUSED_WAIT));
            }
        }
        if let Err(error) = config.check() {
            self.note(facade, slot, false, Some(error));
            return Step::Wait(REFUSED_WAIT);
        }
        let own = match &self.own {
            Some(own) => own.clone(),
            None => match inner.own_id(&slot.info.chat_bridge_id, &config) {
                Ok(own) => {
                    self.own = Some(own.clone());
                    own
                }
                Err(error) => return self.failed(facade, slot, &config, &error),
            },
        };
        let inbound = Inbound {
            inner,
            facade,
            bridge: &slot.info,
            config: &config,
            own: &own,
        };
        if !self.subscribed() || self.subscribed.is_some_and(|at| at.elapsed() >= RESUBSCRIBE) {
            if let Err(error) = self.subscribe(inner, &config, &own) {
                return self.failed(facade, slot, &config, &error);
            }
            self.failures = 0;
            if slot.retry.is_some() {
                set_retry(facade, &slot.info.chat_bridge_id, None);
            }
            self.note(facade, slot, true, None);
            // a new connection may have missed messages: every room catches up again
            self.rooms.caught_up.clear();
            let taken = self.sync(&inbound, facade, slot, &config);
            return Step::Subscribed(taken);
        }
        if self.next_sync.is_none_or(|at| Instant::now() >= at) {
            return Step::Polled(self.sync(&inbound, facade, slot, &config));
        }
        let connect = json!([{"channel": "/meta/connect", "clientId": self.client_id, "connectionType": "long-polling",
            "id": self.id()}]);
        let answer = match inner.api(&config).faye(&connect, self.timeout + POLL_SLACK) {
            Ok(answer) => answer,
            Err(error) => return self.failed(facade, slot, &config, &error),
        };
        let mut taken = 0;
        for message in &answer {
            let channel = message.get("channel").and_then(Json::as_str).unwrap_or_default();
            if channel == "/meta/connect" {
                self.advise(message);
                if message.get("successful").and_then(Json::as_bool) != Some(true) {
                    // the server forgot the client: a new handshake
                    let refused = message
                        .get("error")
                        .and_then(Json::as_str)
                        .is_some_and(|error| error.starts_with("401"));
                    self.client_id = None;
                    if refused {
                        let error = ApiError::Status {
                            status: 401,
                            errors: String::new(),
                        };
                        return self.failed(facade, slot, &config, &error);
                    }
                }
                continue;
            }
            if !channel.starts_with("/user/") {
                continue;
            }
            let Some(data) = message.get("data") else { continue };
            let worked = std::panic::catch_unwind(AssertUnwindSafe(|| inbound.handle(&mut self.rooms, data)));
            match worked {
                Ok(count) => taken += count,
                Err(_) => tracing::error!(
                    "GroupMe bridge {}: a push event failed and was left: {}",
                    slot.info.chat_bridge_id,
                    data.get("type").and_then(Json::as_str).unwrap_or_default()
                ),
            }
        }
        self.failures = 0;
        self.note_limit(facade, slot);
        Step::Polled(taken)
    }

    /// The room limit in the bridge's status, while a room was refused.
    fn note_limit(&mut self, facade: &EntityFacade, slot: &Slot) {
        if std::mem::take(&mut self.rooms.limit_hit) {
            self.note(facade, slot, true, Some(ROOM_LIMIT.to_string()));
        }
    }

    /// The server's advice: its long poll's timeout (at most a minute), and whether to handshake again.
    fn advise(&mut self, message: &Json) {
        let Some(advice) = message.get("advice") else {
            return;
        };
        if let Some(timeout) = advice.get("timeout").and_then(Json::as_u64) {
            self.timeout = Duration::from_millis(timeout.clamp(1_000, 60_000));
        }
        if advice.get("reconnect").and_then(Json::as_str) == Some("handshake") {
            self.client_id = None;
        }
    }

    /// A handshake and the subscription to the user's channel.
    fn subscribe(&mut self, inner: &Inner, config: &Config, own: &str) -> Result<(), ApiError> {
        let api = inner.api(config);
        let handshake = json!([{"channel": "/meta/handshake", "version": "1.0",
            "supportedConnectionTypes": ["long-polling"], "id": self.id()}]);
        let answer = api.faye(&handshake, Duration::from_secs(30))?;
        let shaken = answer
            .iter()
            .find(|message| message.get("channel").and_then(Json::as_str) == Some("/meta/handshake"))
            .filter(|message| message.get("successful").and_then(Json::as_bool) == Some(true))
            .cloned()
            .ok_or_else(|| ApiError::Status {
                status: 502,
                errors: "the push server refused the handshake".to_string(),
            })?;
        let client = id_of(&shaken, "clientId").ok_or_else(|| ApiError::Status {
            status: 502,
            errors: "the push server named no client".to_string(),
        })?;
        self.advise(&shaken);
        let stamp = jiff::Timestamp::now().as_second();
        let subscribe = json!([{"channel": "/meta/subscribe", "clientId": client, "subscription": format!("/user/{own}"),
            "id": self.id(), "ext": {"access_token": config.token, "timestamp": stamp}}]);
        let answer = api.faye(&subscribe, Duration::from_secs(30))?;
        let subscribed = answer
            .iter()
            .find(|message| message.get("channel").and_then(Json::as_str) == Some("/meta/subscribe"))
            .cloned()
            .unwrap_or(Json::Null);
        if subscribed.get("successful").and_then(Json::as_bool) != Some(true) {
            let error = subscribed.get("error").and_then(Json::as_str).unwrap_or_default();
            return Err(ApiError::Status {
                status: if error.starts_with("401") || error.starts_with("403") {
                    401
                } else {
                    502
                },
                errors: config.scrub(error),
            });
        }
        self.client_id = Some(client);
        self.subscribed = Some(Instant::now());
        Ok(())
    }

    /// The catch-up, with the Topics once an hour; the next one in `syncMinutes`, or in a minute when a room did not
    /// catch up. A sync with no room refused clears a room-limit status.
    fn sync(&mut self, inbound: &Inbound<'_>, facade: &EntityFacade, slot: &Slot, config: &Config) -> usize {
        let topics = self.topics_synced.is_none_or(|at| at.elapsed() >= TOPICS_EVERY);
        if topics {
            self.rooms.topics_read.clear();
            self.topics_synced = Some(Instant::now());
        }
        // each room is asked again, so a new name from GroupMe reaches it and its mark is read
        self.rooms.made.clear();
        self.rooms.limit_hit = false;
        let (taken, complete) = inbound.sync(&mut self.rooms, topics);
        let every = Duration::from_secs(config.sync_minutes.saturating_mul(60));
        self.next_sync = Some(Instant::now() + if complete { every } else { SYNC_AGAIN.min(every) });
        if self.rooms.limit_hit {
            self.note_limit(facade, slot);
        } else if slot
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("roomLimit"))
        {
            self.note(facade, slot, true, Some(String::new()));
        }
        taken
    }

    /// A failure's wait, and the bridge's status: a refused token at once, another failure from the third in a row. A
    /// long wait is kept as the bridge's retryDate.
    fn failed(&mut self, facade: &EntityFacade, slot: &Slot, config: &Config, error: &ApiError) -> Step {
        self.client_id = None;
        let id = &slot.info.chat_bridge_id;
        let wait = if error.unauthorized() {
            self.own = None;
            self.note(facade, slot, false, Some(REFUSED.to_string()));
            REFUSED_WAIT
        } else {
            self.failures = self.failures.saturating_add(1);
            let mut wait = Duration::from_secs(2)
                .saturating_mul(1 << self.failures.saturating_sub(1).min(8))
                .min(MAX_BACKOFF);
            if error.rate_limited() {
                wait = wait.max(Duration::from_secs(60));
            }
            let text = config.scrub(&error.text());
            tracing::warn!(
                "GroupMe bridge {id}: the push channel failed ({} in a row), again in {} s: {text}",
                self.failures,
                wait.as_secs()
            );
            if self.failures >= 3 {
                self.note(facade, slot, false, Some(format!("connectFailed: {text}")));
            }
            wait
        };
        if wait >= KEPT_WAIT {
            let until = now_millis().saturating_add(i64::try_from(wait.as_millis()).unwrap_or(i64::MAX));
            set_retry(facade, id, Some(until));
        }
        Step::Wait(wait)
    }
}

// ---- the leases ----

/// The lock row of bridge `bridge_id`'s connection.
pub fn lock_name(bridge_id: &str) -> String {
    format!("groupme:{bridge_id}")
}

/// Claim or renew, for `member`, the connections of `bridge_ids` in one pass: renew those it holds, take those free
/// or lapsed (by the database's clock) up to `node_max` in all. The bridges it holds now.
///
/// # Errors
///
/// When the lock rows cannot be read or written.
pub fn claim_all(
    facade: &EntityFacade,
    bridge_ids: &[String],
    member: &str,
    node_max: usize,
    lease: Duration,
) -> Result<HashSet<String>, LibError> {
    if bridge_ids.is_empty() {
        return Ok(HashSet::new());
    }
    let names: Vec<String> = bridge_ids.iter().map(|id| lock_name(id)).collect();
    let values = |list: &[String]| list.iter().map(Value::string).collect::<Vec<_>>();
    let now = moqui_core::cluster::db_now(facade);
    let until = now.saturating_add(i64::try_from(lease.as_millis()).unwrap_or(i64::MAX));
    let rows = facade
        .find(LOCK)?
        .condition(Condition::is_in("lockName", values(&names)))
        .use_cache(false)
        .disable_authz()
        .list()?;
    let mut seen: HashMap<String, (String, Option<i64>)> = HashMap::new();
    for row in rows.values() {
        seen.insert(
            text(row, "lockName").unwrap_or_default(),
            (
                text(row, "instanceId").unwrap_or_default(),
                row.get("leaseExpireDate").and_then(moqui_core::user::value_millis),
            ),
        );
    }
    // a bridge's first lock row
    let missing: Vec<String> = names.iter().filter(|name| !seen.contains_key(*name)).cloned().collect();
    for name in &missing {
        let mut row = facade.make_value(LOCK)?;
        row.set("lockName", Value::string(name));
        row.create_if_missing(facade)?;
        seen.insert(name.clone(), (String::new(), None));
    }
    let mine: Vec<String> = names
        .iter()
        .filter(|name| seen.get(*name).is_some_and(|(holder, _)| holder == member))
        .cloned()
        .collect();
    if !mine.is_empty() {
        facade
            .find(LOCK)?
            .condition(Condition::is_in("lockName", values(&mine)))
            .eq("instanceId", Value::string(member))
            .disable_authz()
            .update_all(vec![(
                "leaseExpireDate".to_string(),
                moqui_core::user::stamp_value(until),
            )])?;
    }
    let room = node_max.saturating_sub(mine.len());
    let free: Vec<String> = names
        .iter()
        .filter(|name| {
            seen.get(*name).is_some_and(|(holder, lease)| {
                holder != member && (holder.is_empty() || lease.is_none_or(|at| at <= now))
            })
        })
        .take(room)
        .cloned()
        .collect();
    if !free.is_empty() {
        // taken only while still free or lapsed: another node's claim between the read and here wins
        facade
            .find(LOCK)?
            .condition(Condition::is_in("lockName", values(&free)))
            .condition(Condition::or(vec![
                Condition::is_null("instanceId"),
                Condition::is_null("leaseExpireDate"),
                Condition::less_equal("leaseExpireDate", moqui_core::user::stamp_value(now)),
            ]))
            .disable_authz()
            .update_all(vec![
                ("instanceId".to_string(), Value::string(member)),
                ("claimedDate".to_string(), moqui_core::user::stamp_value(now)),
                ("leaseExpireDate".to_string(), moqui_core::user::stamp_value(until)),
            ])?;
    }
    let held = facade
        .find(LOCK)?
        .condition(Condition::is_in("lockName", values(&names)))
        .eq("instanceId", Value::string(member))
        .condition(Condition::greater(
            "leaseExpireDate",
            moqui_core::user::stamp_value(now),
        ))
        .use_cache(false)
        .disable_authz()
        .list()?;
    Ok(held
        .values()
        .iter()
        .filter_map(|row| text(row, "lockName"))
        .filter_map(|name| name.strip_prefix("groupme:").map(str::to_string))
        .collect())
}

/// Let every GroupMe connection `member` holds go, so another node takes them at its next pass.
///
/// # Errors
///
/// When the lock rows cannot be written.
pub fn release_all(facade: &EntityFacade, member: &str) -> Result<(), LibError> {
    facade
        .find(LOCK)?
        .condition(Condition::like("lockName", Value::string("groupme:%")))
        .eq("instanceId", Value::string(member))
        .disable_authz()
        .update_all(vec![
            ("instanceId".to_string(), Value::Null),
            ("leaseExpireDate".to_string(), Value::Null),
        ])?;
    Ok(())
}

// ---- the coordinator ----

/// A bridge the coordinator knows: its last read, and its session's thread while it holds it.
struct Known {
    slot: Arc<Mutex<Slot>>,
    worker: Option<(Arc<AtomicBool>, JoinHandle<()>)>,
}

#[derive(Default)]
struct State {
    known: HashMap<String, Known>,
    held: HashSet<String>,
    /// When a claim last went through.
    claimed: Option<Instant>,
}

/// A node's GroupMe coordinator. See the module notes.
pub struct Coordinator {
    member: String,
    stop: AtomicBool,
    woken: Mutex<bool>,
    signal: Condvar,
    state: Mutex<State>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

static NEXT: AtomicU64 = AtomicU64::new(1);

impl Coordinator {
    /// This node's coordinator for `inner`, started when none runs (or the last one ended).
    ///
    /// # Errors
    ///
    /// When its thread cannot be made.
    pub fn ensure(inner: &Arc<Inner>, weak: &Weak<ExecutionContextFactory>) -> Result<Arc<Self>, String> {
        let mut slot = lock(&inner.coordinator);
        if let Some(running) = slot.as_ref() {
            let alive = lock(&running.thread)
                .as_ref()
                .is_some_and(|handle| !handle.is_finished());
            if alive && !running.stop.load(Ordering::SeqCst) {
                return Ok(Arc::clone(running));
            }
        }
        let factory = weak
            .upgrade()
            .ok_or_else(|| "noContext: the server is stopping".to_string())?;
        let instance =
            moqui_core::cluster::instance_id(&factory).unwrap_or_else(|| format!("node-{}", std::process::id()));
        // a token of this coordinator's own: an older one's release never frees its leases
        let token = (u64::from(std::process::id()) << 20)
            ^ NEXT.fetch_add(1, Ordering::SeqCst)
            ^ jiff::Timestamp::now().as_millisecond().unsigned_abs();
        let instance: String = instance.chars().take(48).collect();
        let coordinator = Arc::new(Self {
            member: format!("{instance}#{:x}", token & 0xff_ffff_ffff),
            stop: AtomicBool::new(false),
            woken: Mutex::new(false),
            signal: Condvar::new(),
            state: Mutex::new(State::default()),
            thread: Mutex::new(None),
        });
        let running = Arc::clone(&coordinator);
        let inner_weak = Arc::downgrade(inner);
        let factory_weak = weak.clone();
        let handle = std::thread::Builder::new()
            .name("moqui-groupme".to_string())
            .spawn(framework_thread(move || running.run(&inner_weak, &factory_weak)))
            .map_err(|error| format!("noThread: the GroupMe coordinator did not start: {error}"))?;
        *lock(&coordinator.thread) = Some(handle);
        *slot = Some(Arc::clone(&coordinator));
        Ok(coordinator)
    }

    /// Its name in the leases.
    pub fn member(&self) -> &str {
        &self.member
    }

    /// The bridges whose connections it holds now.
    pub fn held(&self) -> HashSet<String> {
        lock(&self.state).held.clone()
    }

    /// Run a pass now.
    pub fn wake(&self) {
        *lock(&self.woken) = true;
        self.signal.notify_all();
    }

    /// Stop bridge `bridge_id`'s session now (disabled); the next pass lets its lease lapse.
    pub fn stop_bridge(&self, bridge_id: &str) {
        let mut state = lock(&self.state);
        if let Some(known) = state.known.get_mut(bridge_id) {
            if let Some((stop, _)) = known.worker.take() {
                stop.store(true, Ordering::SeqCst);
            }
        }
        state.held.remove(bridge_id);
        drop(state);
        self.wake();
    }

    /// Stop it and every session, letting its leases go.
    pub fn stop_all(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.wake();
        let handle = lock(&self.thread).take();
        if let Some(handle) = handle {
            if handle.thread().id() != std::thread::current().id() {
                let _ = handle.join();
            }
        }
    }

    fn run(&self, inner: &Weak<Inner>, weak: &Weak<ExecutionContextFactory>) {
        loop {
            if self.stop.load(Ordering::SeqCst) {
                break;
            }
            let pass = {
                let Some(factory) = weak.upgrade() else { break };
                if factory.is_shutting_down() {
                    break;
                }
                let Some(inner) = inner.upgrade() else { break };
                let Some(facade) = factory.entity() else { break };
                let context = factory.new_context();
                let _entered = context.enter();
                let _authz = context.disable_authz();
                let timing = inner.timing();
                // a panic in a pass (a claim, a read) is logged, and the next pass runs
                let worked = std::panic::catch_unwind(AssertUnwindSafe(|| self.pass(&inner, weak, &facade, &timing)));
                if worked.is_err() {
                    tracing::error!("GroupMe: the coordinator's pass failed; the next one runs as usual");
                }
                timing.pass
            };
            let guard = lock(&self.woken);
            let (mut guard, _) = self
                .signal
                .wait_timeout_while(guard, pass, |woken| !*woken && !self.stop.load(Ordering::SeqCst))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard = false;
        }
        // stopped: every session, and the leases go
        let workers: Vec<(Arc<AtomicBool>, JoinHandle<()>)> = lock(&self.state)
            .known
            .drain()
            .filter_map(|(_, known)| known.worker)
            .collect();
        for (stop, _) in &workers {
            stop.store(true, Ordering::SeqCst);
        }
        lock(&self.state).held.clear();
        if let Some(factory) = weak.upgrade() {
            if let Some(facade) = factory.entity() {
                let context = factory.new_context();
                let _entered = context.enter();
                let _authz = context.disable_authz();
                if let Err(error) = release_all(&facade, &self.member) {
                    tracing::warn!("GroupMe: the coordinator's leases were not let go: {error}");
                }
            }
        }
    }

    /// One pass: the bridges, their leases, their sessions.
    fn pass(&self, inner: &Arc<Inner>, weak: &Weak<ExecutionContextFactory>, facade: &EntityFacade, timing: &Timing) {
        let rows = match facade.find(BRIDGE).and_then(|find| {
            find.eq("typeEnumId", Value::string("CbtGroupMe"))
                .condition(Condition::or(vec![
                    Condition::is_null("statusId"),
                    Condition::not_equals("statusId", Value::string("CbsDisabled")),
                ]))
                .select_field("chatBridgeId")
                .select_field("statusId")
                .select_field("lastErrorText")
                .select_field("retryDate")
                .select_field("lastUpdatedStamp")
                .disable_authz()
                .list()
        }) {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!("GroupMe: the bridges were not listed: {error}");
                return;
            }
        };
        let mut candidates: Vec<String> = Vec::new();
        let mut state = lock(&self.state);
        let listed: HashSet<String> = rows
            .values()
            .iter()
            .filter_map(|row| text(row, "chatBridgeId"))
            .collect();
        // a bridge gone or disabled: its session stops
        let gone: Vec<String> = state.known.keys().filter(|id| !listed.contains(*id)).cloned().collect();
        for id in gone {
            if let Some(Known {
                worker: Some((stop, _)),
                ..
            }) = state.known.remove(&id)
            {
                stop.store(true, Ordering::SeqCst);
            }
        }
        for row in rows.values() {
            let Some(id) = text(row, "chatBridgeId") else { continue };
            let stamp = text(row, "lastUpdatedStamp");
            let fresh = state
                .known
                .get(&id)
                .is_some_and(|known| lock(&known.slot).stamp == stamp && stamp.is_some());
            if fresh {
                // the status, its error and retry from the list: no configuration read
                if let Some(known) = state.known.get(&id) {
                    let mut slot = lock(&known.slot);
                    slot.status = text(row, "statusId").unwrap_or_default();
                    slot.error = text(row, "lastErrorText");
                    slot.retry = row.get("retryDate").and_then(moqui_core::user::value_millis);
                }
            } else {
                match slot_of(facade, &id) {
                    Ok(Some(read)) => match state.known.get(&id) {
                        Some(known) => *lock(&known.slot) = read,
                        None => {
                            state.known.insert(
                                id.clone(),
                                Known {
                                    slot: Arc::new(Mutex::new(read)),
                                    worker: None,
                                },
                            );
                        }
                    },
                    Ok(None) => continue,
                    Err(error) => {
                        tracing::warn!("GroupMe bridge {id} was not read: {error}");
                        continue;
                    }
                }
            }
            let config = state
                .known
                .get(&id)
                .map(|known| Config::of(&lock(&known.slot).info.config));
            if config.is_some_and(|config| config.push && config.check().is_ok()) {
                candidates.push(id);
            }
        }
        candidates.sort();
        let held = match claim_all(facade, &candidates, &self.member, timing.node_max, timing.lease) {
            Ok(held) => {
                state.claimed = Some(Instant::now());
                held
            }
            Err(error) => {
                tracing::warn!("GroupMe: the connections' leases were not claimed: {error}");
                // still held until two thirds of the lease passed since the last claim that went through
                if state.claimed.is_some_and(|at| at.elapsed() < timing.lease * 2 / 3) {
                    state.held.clone()
                } else {
                    HashSet::new()
                }
            }
        };
        for (id, known) in &mut state.known {
            let holds = held.contains(id) && candidates.contains(id);
            let running = known
                .worker
                .as_ref()
                .is_some_and(|(stop, handle)| !handle.is_finished() && !stop.load(Ordering::SeqCst));
            if holds && !running {
                let stop = Arc::new(AtomicBool::new(false));
                let slot = Arc::clone(&known.slot);
                let (inner, weak, flag) = (Arc::downgrade(inner), weak.clone(), Arc::clone(&stop));
                match std::thread::Builder::new()
                    .name(format!("moqui-groupme-{id}"))
                    .spawn(framework_thread(move || work(&inner, &weak, &slot, &flag)))
                {
                    Ok(handle) => known.worker = Some((stop, handle)),
                    Err(error) => tracing::error!("GroupMe bridge {id}: its session did not start: {error}"),
                }
            } else if !holds {
                if let Some((stop, _)) = known.worker.take() {
                    stop.store(true, Ordering::SeqCst);
                    tracing::info!("GroupMe bridge {id}: this node no longer holds its connection");
                }
            }
        }
        state.held = held;
    }
}

/// Wait `wait` unless the session is stopped or its bridge's configuration changed.
fn pause(stop: &AtomicBool, slot: &Mutex<Slot>, fingerprint: &str, wait: Duration) {
    let until = Instant::now() + wait;
    while Instant::now() < until {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        if !fingerprint.is_empty() && Config::of(&lock(slot).info.config).fingerprint() != fingerprint {
            return;
        }
        std::thread::sleep(Duration::from_millis(250).min(until.saturating_duration_since(Instant::now())));
    }
}

/// A held bridge's session: steps until stopped (the lease lost, the bridge disabled or gone, the node stopping).
fn work(inner: &Weak<Inner>, weak: &Weak<ExecutionContextFactory>, slot: &Mutex<Slot>, stop: &AtomicBool) {
    let mut session = Session::new();
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let wait = {
            let Some(factory) = weak.upgrade() else { break };
            if factory.is_shutting_down() {
                break;
            }
            let Some(inner) = inner.upgrade() else { break };
            let Some(facade) = factory.entity() else { break };
            let current = lock(slot).clone();
            let context = factory.new_context();
            let _entered = context.enter();
            let _authz = context.disable_authz();
            match std::panic::catch_unwind(AssertUnwindSafe(|| session.step(&inner, &facade, &current))) {
                Ok(Step::Wait(wait)) => wait,
                Ok(Step::Subscribed(_) | Step::Polled(_)) => Duration::ZERO,
                Err(_) => {
                    tracing::error!("GroupMe bridge {}: its push step failed", current.info.chat_bridge_id);
                    session.reset();
                    Duration::from_secs(30)
                }
            }
        };
        if !wait.is_zero() {
            let fingerprint = session.fingerprint().to_string();
            pause(stop, slot, &fingerprint, wait);
        }
    }
}
