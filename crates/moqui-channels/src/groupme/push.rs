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

//! GroupMe's push channel (dev.groupme.com, the push tutorial): Bayeux long-polling over HTTPS at `push.groupme.com/faye`,
//! one thread a bridge.
//!
//! - **A session** ([`Session::step`], one step at a time, so a test drives it with no timing): the token's user
//!   (`/users/me`), a handshake, a subscription to `/user/<id>` with `ext: {access_token, timestamp}`, then the catch-up
//!   ([`super::events::Inbound::sync`]) and `/meta/connect` long polls, each answer's messages worked in order. A new
//!   handshake every hour ("Signature must refresh every hour"), and when the server asks for one. The catch-up runs again
//!   every `syncMinutes`, the Topics with it once an hour.
//! - **Failures.** A refused token (401, from the API or the push server) marks the bridge not connected with what to
//!   do, and waits 5 minutes or a new token. Another failure waits 2 s, twice as long each time up to 5 minutes (a rate
//!   limit at least a minute); from the third in a row the bridge is not connected with the error.
//! - **The thread** ([`spawn`]) holds a `Weak` factory and works each step in a fresh context with authorization off. It
//!   reads its bridge again before each step: disabled or removed, it ends; a changed token or setting starts a new
//!   session at once, also during a wait. One node of a cluster holds the connection: the lease
//!   `moqui.edge.EdgeSenderLock` `groupme:<bridge>` ([`moqui_core::edge::replica::sender::claim`], the replication
//!   sender's and the SIP trunks' lock), claimed for [`LEASE`] and renewed between steps; another node asks every
//!   [`ASK_EVERY`] and takes it over when it lapses. A holder whose claims fail stops once two thirds of the lease
//!   passed since the last one that went through. The lock is let go when the thread ends.

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::EntityFacade;
use moqui_core::service::chat_bridge::{bridge_row, note_status, BridgeInfo};
use serde_json::{json, Value as Json};

use super::api::{id_of, ApiError};
use super::events::{Inbound, Rooms};
use super::{Config, Inner};

/// How long a claim of a bridge's connection lasts.
pub const LEASE: Duration = Duration::from_secs(120);
/// How often its holder renews it (between steps).
pub const RENEW: Duration = Duration::from_secs(20);
/// How often a node that does not hold it asks.
pub const ASK_EVERY: Duration = Duration::from_secs(15);
const RESUBSCRIBE: Duration = Duration::from_secs(3600);
const TOPICS_EVERY: Duration = Duration::from_secs(3600);
const REFUSED_WAIT: Duration = Duration::from_secs(300);
const MAX_BACKOFF: Duration = Duration::from_secs(300);
/// How far past the server's own timeout a long poll waits for its answer.
const POLL_SLACK: Duration = Duration::from_secs(15);
/// What the bridge says when GroupMe refuses its token.
pub const REFUSED: &str = "GroupMe refused the access token: get a new one (docs/interfaces/GroupMe.md)";

/// What a step did, and how long the thread waits before the next.
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
    synced: Option<Instant>,
    topics_synced: Option<Instant>,
    timeout: Duration,
    failures: u32,
    noted: Option<(bool, Option<String>)>,
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
            synced: None,
            topics_synced: None,
            timeout: Duration::from_secs(30),
            failures: 0,
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

    /// Start again at the handshake (another node holds the connection; a new token).
    pub fn reset(&mut self) {
        let noted = self.noted.take();
        *self = Self::default();
        self.noted = noted;
    }

    /// Ask for the catch-up at the next step.
    pub fn sync_soon(&mut self) {
        self.synced = None;
    }

    fn id(&mut self) -> String {
        self.next_id += 1;
        self.next_id.to_string()
    }

    /// Keep the bridge's status: a failure when it changed; connected always (a subscription, so at most hourly), since a
    /// send or a new token may have marked the bridge not connected meanwhile, and the row writes nothing when it is so.
    fn note(&mut self, facade: &EntityFacade, bridge: &BridgeInfo, connected: bool, error: Option<String>) {
        let now = (connected, error);
        if !connected && self.noted.as_ref() == Some(&now) {
            return;
        }
        note_status(facade, &bridge.chat_bridge_id, now.0, now.1.as_deref());
        self.noted = Some(now);
    }

    /// One step. See the module notes.
    pub fn step(&mut self, inner: &Inner, facade: &EntityFacade, bridge: &BridgeInfo) -> Step {
        let config = Config::of(&bridge.config);
        let fingerprint = config.fingerprint();
        if fingerprint != self.fingerprint {
            self.reset();
            self.fingerprint = fingerprint;
        }
        if let Err(error) = config.check() {
            self.note(facade, bridge, false, Some(error));
            return Step::Wait(REFUSED_WAIT);
        }
        let own = match &self.own {
            Some(own) => own.clone(),
            None => match inner.own_id(&bridge.chat_bridge_id, &config) {
                Ok(own) => {
                    self.own = Some(own.clone());
                    own
                }
                Err(error) => return self.failed(facade, bridge, &config, &error),
            },
        };
        let inbound = Inbound {
            inner,
            bridge,
            config: &config,
            own: &own,
        };
        if !self.subscribed() || self.subscribed.is_some_and(|at| at.elapsed() >= RESUBSCRIBE) {
            if let Err(error) = self.subscribe(inner, &config, &own) {
                return self.failed(facade, bridge, &config, &error);
            }
            self.failures = 0;
            self.note(facade, bridge, true, None);
            let taken = self.sync(&inbound, facade);
            return Step::Subscribed(taken);
        }
        if self
            .synced
            .is_none_or(|at| at.elapsed() >= Duration::from_secs(config.sync_minutes.saturating_mul(60)))
        {
            return Step::Polled(self.sync(&inbound, facade));
        }
        let connect = json!([{"channel": "/meta/connect", "clientId": self.client_id, "connectionType": "long-polling",
            "id": self.id()}]);
        let answer = match inner.api(&config).faye(&connect, self.timeout + POLL_SLACK) {
            Ok(answer) => answer,
            Err(error) => return self.failed(facade, bridge, &config, &error),
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
                        return self.failed(
                            facade,
                            bridge,
                            &config,
                            &ApiError::Status {
                                status: 401,
                                errors: String::new(),
                            },
                        );
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
                    bridge.chat_bridge_id,
                    data.get("type").and_then(Json::as_str).unwrap_or_default()
                ),
            }
        }
        self.failures = 0;
        Step::Polled(taken)
    }

    /// The server's advice: its long poll's timeout, and whether to handshake again.
    fn advise(&mut self, message: &Json) {
        let Some(advice) = message.get("advice") else {
            return;
        };
        if let Some(timeout) = advice.get("timeout").and_then(Json::as_u64) {
            self.timeout = Duration::from_millis(timeout.clamp(1_000, 120_000));
        }
        if advice.get("reconnect").and_then(Json::as_str) == Some("handshake") {
            self.client_id = None;
        }
    }

    /// A handshake and the subscription to the user's channel.
    fn subscribe(&mut self, inner: &Inner, config: &Config, own: &str) -> Result<(), ApiError> {
        let api = inner.api(config);
        let handshake = json!([{"channel": "/meta/handshake", "version": "1.0", "supportedConnectionTypes": ["long-polling"],
            "id": self.id()}]);
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

    /// The catch-up, with the Topics once an hour.
    fn sync(&mut self, inbound: &Inbound<'_>, facade: &EntityFacade) -> usize {
        let topics = self.topics_synced.is_none_or(|at| at.elapsed() >= TOPICS_EVERY);
        if topics {
            self.rooms.topics_read.clear();
            self.topics_synced = Some(Instant::now());
        }
        // each room is asked again, so a new name from GroupMe reaches it
        self.rooms.made.clear();
        let taken = inbound.sync(&mut self.rooms, facade, topics);
        self.synced = Some(Instant::now());
        taken
    }

    /// A failure's wait, and the bridge's status: a refused token at once, another failure from the third in a row.
    fn failed(&mut self, facade: &EntityFacade, bridge: &BridgeInfo, config: &Config, error: &ApiError) -> Step {
        self.client_id = None;
        if error.unauthorized() {
            self.own = None;
            self.note(facade, bridge, false, Some(REFUSED.to_string()));
            return Step::Wait(REFUSED_WAIT);
        }
        self.failures = self.failures.saturating_add(1);
        let mut wait = Duration::from_secs(2)
            .saturating_mul(1 << self.failures.saturating_sub(1).min(8))
            .min(MAX_BACKOFF);
        if error.rate_limited() {
            wait = wait.max(Duration::from_secs(60));
        }
        let text = config.scrub(&error.text());
        tracing::warn!(
            "GroupMe bridge {}: the push channel failed ({} in a row), again in {} s: {text}",
            bridge.chat_bridge_id,
            self.failures,
            wait.as_secs()
        );
        if self.failures >= 3 {
            self.note(facade, bridge, false, Some(text));
        }
        Step::Wait(wait)
    }
}

/// This node's name in the bridge's lease.
fn member_of(factory: &ExecutionContextFactory) -> String {
    moqui_core::cluster::instance_id(factory).unwrap_or_else(|| format!("node-{}", std::process::id()))
}

/// The lock row of bridge `bridge_id`'s connection.
pub fn lock_name(bridge_id: &str) -> String {
    format!("groupme:{bridge_id}")
}

/// Claim or renew the connection of bridge `bridge_id` for `member`: whether it holds it now, none when the claim did
/// not go through.
pub fn claim(facade: &EntityFacade, bridge_id: &str, member: &str) -> Option<bool> {
    match moqui_core::edge::replica::sender::claim(facade, &lock_name(bridge_id), member, LEASE) {
        Ok(claim) => Some(claim.holds()),
        Err(error) => {
            tracing::warn!(
                "GroupMe bridge {bridge_id}: its connection's lease was not claimed: {}",
                error.message()
            );
            None
        }
    }
}

/// Wait `wait` unless the thread is stopped, or (asked every 10 s) its bridge is disabled, gone or changed.
fn pause(weak: &Weak<ExecutionContextFactory>, bridge_id: &str, stop: &AtomicBool, fingerprint: &str, wait: Duration) {
    let until = Instant::now() + wait;
    let mut asked = Instant::now();
    while Instant::now() < until {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        std::thread::sleep(Duration::from_millis(250).min(until.saturating_duration_since(Instant::now())));
        if asked.elapsed() >= Duration::from_secs(10) {
            asked = Instant::now();
            let Some(factory) = weak.upgrade() else { return };
            let Some(facade) = factory.entity() else { return };
            let context = factory.new_context();
            let _entered = context.enter();
            let _authz = context.disable_authz();
            match bridge_row(&facade, bridge_id) {
                Ok(Some((info, status))) => {
                    if status == "CbsDisabled" || Config::of(&info.config).fingerprint() != fingerprint {
                        return;
                    }
                }
                Ok(None) => return,
                Err(_) => {}
            }
        }
    }
}

/// Spawn bridge `bridge_id`'s push thread. See the module notes.
///
/// # Errors
///
/// When the thread cannot be made.
pub fn spawn(
    inner: Arc<Inner>,
    weak: Weak<ExecutionContextFactory>,
    bridge_id: String,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    std::thread::Builder::new()
        .name(format!("moqui-groupme-{bridge_id}"))
        .spawn(move || run(&inner, &weak, &bridge_id, &stop))
        .map(|_| ())
        .map_err(|error| format!("The GroupMe push thread did not start: {error}"))
}

fn run(inner: &Inner, weak: &Weak<ExecutionContextFactory>, bridge_id: &str, stop: &AtomicBool) {
    let mut session = Session::new();
    let mut member = String::new();
    // when this node last held the lease by a claim that went through, and when it last asked
    let mut held: Option<Instant> = None;
    let mut asked: Option<Instant> = None;
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let wait = {
            let Some(factory) = weak.upgrade() else { return };
            if factory.is_shutting_down() {
                return;
            }
            let Some(facade) = factory.entity() else { return };
            if member.is_empty() {
                member = member_of(&factory);
            }
            let context = factory.new_context();
            let _entered = context.enter();
            let _authz = context.disable_authz();
            let info = match bridge_row(&facade, bridge_id) {
                Ok(Some((info, status))) if status != "CbsDisabled" => info,
                Ok(_) => break,
                Err(error) => {
                    tracing::warn!("GroupMe bridge {bridge_id}: its row was not read: {error}");
                    pause(weak, bridge_id, stop, session.fingerprint(), ASK_EVERY);
                    continue;
                }
            };
            if asked.is_none_or(|at| at.elapsed() >= RENEW) || held.is_none() {
                let before = Instant::now();
                asked = Some(before);
                match claim(&facade, bridge_id, &member) {
                    Some(true) => held = Some(before),
                    Some(false) => held = None,
                    // a claim that failed: still held until two thirds of the lease passed since the last good one
                    None => {
                        if held.is_some_and(|at| at.elapsed() >= LEASE * 2 / 3) {
                            held = None;
                        }
                    }
                }
            }
            if held.is_none() {
                if session.subscribed() {
                    tracing::info!("GroupMe bridge {bridge_id}: another node holds its connection");
                }
                session.reset();
                ASK_EVERY
            } else {
                match std::panic::catch_unwind(AssertUnwindSafe(|| session.step(inner, &facade, &info))) {
                    Ok(Step::Wait(wait)) => wait,
                    Ok(Step::Subscribed(_) | Step::Polled(_)) => Duration::ZERO,
                    Err(_) => {
                        tracing::error!("GroupMe bridge {bridge_id}: its push step failed");
                        session.reset();
                        Duration::from_secs(30)
                    }
                }
            }
        };
        if !wait.is_zero() {
            let fingerprint = session.fingerprint().to_string();
            pause(weak, bridge_id, stop, &fingerprint, wait);
        }
    }
    // ended: a later start spawns a new thread
    stop.store(true, Ordering::SeqCst);
    // let the lease go, so another node takes it at once
    if let Some(factory) = weak.upgrade() {
        if let Some(facade) = factory.entity() {
            let context = factory.new_context();
            let _entered = context.enter();
            let _authz = context.disable_authz();
            if let Err(error) = moqui_core::edge::replica::sender::release(&facade, &lock_name(bridge_id), &member) {
                tracing::warn!(
                    "GroupMe bridge {bridge_id}: its lease was not let go: {}",
                    error.message()
                );
            }
        }
    }
}
