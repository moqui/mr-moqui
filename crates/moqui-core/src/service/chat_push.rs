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

//! The chat push worker (plans/Comms-Messaging.md §7.2, §7.3; phase 2 steps 2b to 2d, the server side of 2h), on the
//! pattern of the bridge delivery worker ([`super::chat_bridge`]). The services are
//! `framework/service/org/moqui/chat/ChatPushServices.xml`, whose notes give the rules.
//!
//! - `append#Event` writes a `ChatPushJob` (room, event) in the event's transaction and `wake#ChatPush` wakes this
//!   node's worker once it commits ([`start`]: a thread per node, woken, once a minute while pushes wait, and once at
//!   start, so the rows a crash left are taken). A [`round`] at a given time (`run#ChatPush`, the tests' clock):
//!   1. each job, claimed with a lease ([`LEASE_MILLIS`]): the members and the rules 1, 3, 4 and 6 then, a `PushOutbox`
//!      row for each device of each user who gets it, merged into a pending or held row of the same device and room
//!      with a count (an edit is not another message), each device in a transaction of its own and written once
//!      however often the job is worked (a lease that ran out, a crash); then the job goes;
//!   2. the held rows of users no longer on do not disturb become pending: one summary per room and device;
//!   3. each pending row due, claimed with a lease and sent as the claim locked it: not sent when its user has read up
//!      to it since, when its device now belongs to another user, or (a clear) when a newer push of the room waits or
//!      shows unread; else encrypted to its device ([`push_crypto::encrypt`], RFC 8291) and sent by its provider
//!      through the [`PushTransport`], with a collapse key or Topic per device, room and kind ([`collapse_key`]);
//!   4. once an hour, the rows done more than a week ago go.
//! - Failures: each job, row and device is on its own; one that fails is logged and the rest go on. A send not taken
//!   waits with backoff and jitter ([`FIRST_WAIT_MILLIS`] doubled, at most [`MAX_WAIT_MILLIS`], or the push service's
//!   Retry-After), up to [`MAX_ATTEMPTS`] or its time to live; FCM's gone deletes the registration, Web Push's and
//!   UnifiedPush's (404, 410) disable it.
//! - `queue#ReadClear` (`mark#Read`, in a transaction of its own): the user's rows of the room not yet sent are dropped
//!   (one newer than the read counts only what is left), and each other device that was sent one gets a `clear` push,
//!   unless a newer push of the room is unread there. A device that moves to another user drops what waits for it
//!   ([`forget_device_pushes`]). `ack#Push` (the app or the service worker showed a push) keeps the device's acked
//!   count.
//! - `queue#ReminderPush` (plans/Comms-Messaging.md phase 15, §7.2 rule 9): a reminder the user set, one `PokReminder`
//!   row a device ([`queue_reminder`]), sent as a `notification` push: not dropped by a read marker nor closed by a
//!   room's clear, with a collapse key of its own; held during do not disturb as messages are.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use moqui_db::sql::Condition;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;
use serde_json::json;
use sha2::Digest;

use super::chat_bridge::{current_factory, force_new, millis_of, node_id, now_millis, param, stamp, text_of};
use crate::context::framework_thread;
use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::{EntityFacade, EntityValue};
use crate::notify::push_crypto;

#[path = "chat_push_transport.rs"]
mod transport;
use transport::transport;
pub use transport::{set_transport, web_outcome, Provider, PushTransport, Sent, WebPost, SLOW_SEND, USER_ROUND_BUDGET};

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, wake_chat_push)?;
    rust_service!(map, run_chat_push)?;
    rust_service!(map, queue_read_clear)?;
    rust_service!(map, ack_push)?;
    rust_service!(map, send_test_push)?;
    rust_service!(map, queue_reminder_push)?;
    Ok(())
}

/// The notification topics this worker pushes: their notifications are stored and on the socket, and
/// [`crate::notify::NotificationMessage::send`] pushes none of them, so a mention is not pushed twice.
pub const PUSHED_TOPICS: [&str; 2] = ["ChatMention", "ChatReminder"];

const JOB: &str = "moqui.chat.ChatPushJob";
const OUTBOX: &str = "moqui.chat.PushOutbox";
const DEVICE: &str = "moqui.security.user.WebPushSubscription";
const EVENT: &str = "moqui.chat.ChatEvent";
const ROOM: &str = "moqui.chat.ChatRoom";
const MEMBER: &str = "moqui.chat.ChatRoomMember";
const ACTIVITY: &str = "moqui.chat.ChatUserActivity";
const STATUS: &str = "moqui.security.UserStatus";
const PREFERENCE: &str = "moqui.security.UserPreference";

const PENDING: &str = "PosPending";
const HELD: &str = "PosHeld";
const SENT: &str = "PosSent";
const FAILED: &str = "PosFailed";
const DROPPED: &str = "PosDropped";
const KIND_MESSAGE: &str = "PokMessage";
const KIND_CLEAR: &str = "PokClear";
const KIND_REMINDER: &str = "PokReminder";

/// How long a worker holds a job or a push it works.
pub const LEASE_MILLIS: i64 = 60_000;
/// A message's time to live: 4 weeks, FCM's most. A phone offline that long still gets one push per room.
pub const MESSAGE_TTL_SECONDS: i64 = 28 * 86_400;
/// A clear's time to live: a day.
pub const CLEAR_TTL_SECONDS: i64 = 86_400;
/// A reminder's time to live: a day.
pub const REMINDER_TTL_SECONDS: i64 = 86_400;
/// The wait after the first send not taken; it doubles after each, to [`MAX_WAIT_MILLIS`].
pub const FIRST_WAIT_MILLIS: i64 = 10_000;
pub const MAX_WAIT_MILLIS: i64 = 3_600_000;
/// Sends of a push in all, before it fails.
pub const MAX_ATTEMPTS: i64 = 12;
/// Tries of a job before it is given up (logged).
const JOB_ATTEMPTS: i64 = 5;
const JOBS_PER_ROUND: i64 = 200;
const PUSHES_PER_ROUND: i64 = 500;
/// The times a round reads the pushes due again after it put off the rest of the users whose budget was spent.
const DELIVERY_PASSES: usize = 4;
/// How often the worker looks without being woken, while pushes wait.
const SWEEP: Duration = Duration::from_secs(60);
/// The rows done (sent, failed, dropped) are kept this long.
const KEEP_MILLIS: i64 = 7 * 86_400_000;
/// A push this long after it was written is checked again before it goes ([`gone_since_queued`], SEC-COM-92).
const WAITED_CHECK: i64 = 30_000;
const MAX_SNIPPET: usize = 140;
const MAX_NAME: usize = 100;

// ---- helpers ----

/// A whole count of a number field (an integer may come back as a decimal).
fn count_of(row: &EntityValue, field: &str) -> i64 {
    row.get(field)
        .filter(|value| !value.is_null())
        .map(to_plain_string)
        .and_then(|text| text.split('.').next().and_then(|whole| whole.trim().parse().ok()))
        .unwrap_or(0)
}

fn leased(row: &EntityValue, now: i64) -> bool {
    text_of(row, "leaseOwner").is_some() && millis_of(row, "leaseExpiresDate").is_some_and(|until| until > now)
}

fn cut_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The key of the collapse keys: the server's VAPID key (`push/web@vapid-key`, generated and shared by the cluster),
/// else one made for this process (no VAPID key: a test, or a server that pushes to no browser).
fn collapse_secret() -> Vec<u8> {
    static PROCESS: std::sync::OnceLock<[u8; 32]> = std::sync::OnceLock::new();
    let seed = crate::context::conf_env::setting("push/web@vapid-key");
    if !seed.trim().is_empty() && !seed.ends_with("CHANGEME") {
        return seed.into_bytes();
    }
    PROCESS
        .get_or_init(|| {
            let mut key = [0_u8; 32];
            if getrandom::fill(&mut key).is_err() {
                key = sha2::Sha256::digest(format!("chat-push:{}", std::process::id()).as_bytes()).into();
            }
            key
        })
        .to_vec()
}

/// The collapse key (FCM) and Topic (Web Push) of a device's pushes of a room, a message's or a clear's (so a clear
/// never replaces a message waiting at the push service): an HMAC with a server secret over the registration and the
/// room, so the push service can neither find a room id from it nor tell that two users' pushes are of one room; at
/// most 32 characters of the base64url alphabet (RFC 8030 §5.4).
pub fn collapse_key(device: &str, room_id: &str, clear: bool) -> String {
    use hmac::{KeyInit, Mac};
    let secret = collapse_secret();
    let Ok(mut mac) = hmac::Hmac::<sha2::Sha256>::new_from_slice(&secret) else {
        return String::new();
    };
    let kind = if clear { "clear" } else { "message" };
    mac.update(format!("moqui-chat-push-collapse\0{device}\0{kind}\0{room_id}").as_bytes());
    let digest = mac.finalize().into_bytes();
    let hex: String = digest.iter().take(15).map(|byte| format!("{byte:02x}")).collect();
    format!("c{hex}")
}

/// The tag a device replaces and closes a room's notifications by (in the encrypted payload only).
fn room_tag(room_id: &str) -> String {
    format!("chat-{room_id}")
}

/// The wait before a push's next send after `attempt` sends: the push service's Retry-After when it gave one, else
/// [`FIRST_WAIT_MILLIS`] doubled for each send before, with a jitter of up to a fifth either way taken from the push's
/// id, at most [`MAX_WAIT_MILLIS`].
pub fn backoff(push_id: &str, attempt: i64, after_millis: Option<i64>) -> i64 {
    if let Some(after) = after_millis.filter(|after| *after > 0) {
        return after.min(MAX_WAIT_MILLIS);
    }
    let doublings = u32::try_from((attempt - 1).clamp(0, 20)).unwrap_or(20);
    let base = FIRST_WAIT_MILLIS
        .saturating_mul(1_i64 << doublings)
        .min(MAX_WAIT_MILLIS);
    let digest = sha2::Sha256::digest(format!("{push_id}:{attempt}").as_bytes());
    let spread = i64::from(u16::from_be_bytes([digest[0], digest[1]])) % 401 - 200; // -200..=200 thousandths
    (base + base * spread / 1000).clamp(1_000, MAX_WAIT_MILLIS)
}

fn is_dnd(row: &EntityValue, now: i64) -> bool {
    text_of(row, "statusEnumId").as_deref() == Some("UpsDnd")
        && millis_of(row, "clearAtDate").is_none_or(|clear| clear > now)
}

/// Of `users`, those on do not disturb now (UserStatus UpsDnd, by hand or by the hours job, whose clear-at time has not
/// come).
fn on_dnd(facade: &EntityFacade, users: &[String], now: i64) -> Result<HashSet<String>, LibError> {
    if users.is_empty() {
        return Ok(HashSet::new());
    }
    Ok(facade
        .find(STATUS)?
        .condition(Condition::is_in("userId", users.iter().map(Value::string).collect()))
        .disable_authz()
        .list()?
        .values()
        .iter()
        .filter(|row| is_dnd(row, now))
        .filter_map(|row| text_of(row, "userId"))
        .collect())
}

/// Of `users`, those whose preference CHAT_NOTIFY_PREVIEW is N: their pushes have no snippet.
fn preview_off(facade: &EntityFacade, users: &[String]) -> Result<HashSet<String>, LibError> {
    if users.is_empty() {
        return Ok(HashSet::new());
    }
    Ok(facade
        .find(PREFERENCE)?
        .condition(Condition::is_in("userId", users.iter().map(Value::string).collect()))
        .eq("preferenceKey", Value::string("CHAT_NOTIFY_PREVIEW"))
        .disable_authz()
        .list()?
        .values()
        .iter()
        .filter(|row| matches!(text_of(row, "preferenceValue").as_deref(), Some("N" | "false")))
        .filter_map(|row| text_of(row, "userId"))
        .collect())
}

/// Whether `user` has read room `room_id` up to `seq` (a member's read marker at or past it).
fn read_past(facade: &EntityFacade, user: &str, room_id: &str, seq: i64) -> Result<bool, LibError> {
    Ok(facade
        .find(MEMBER)?
        .eq("chatRoomId", Value::string(room_id))
        .eq("userId", Value::string(user))
        .disable_authz()
        .one()?
        .is_some_and(|member| count_of(&member, "lastReadEventSeq") >= seq))
}

// ---- the worker ----

/// This node's worker: whether it runs, what wakes it, and when it last cleaned.
#[derive(Default)]
struct Worker {
    started: AtomicBool,
    /// Pushes wait (seen in the last round, or a wake): the worker then also looks once a minute.
    active: AtomicBool,
    wake: Mutex<bool>,
    signal: Condvar,
    cleaned: Mutex<Option<i64>>,
    /// The worker thread, joined by [`shutdown`].
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

fn worker(factory: &ExecutionContextFactory) -> Arc<Worker> {
    factory.extension(|_| Worker::default())
}

/// Wake this node's worker (a job or a clear committed).
pub fn wake(factory: &ExecutionContextFactory) {
    let worker = worker(factory);
    worker.active.store(true, Ordering::SeqCst);
    *lock(&worker.wake) = true;
    worker.signal.notify_all();
}

/// Wake the worker once the current transaction commits (at once outside one; nothing on a rollback).
fn wake_after_commit(facade: &EntityFacade) {
    let Some(factory) = current_factory() else {
        return;
    };
    let weak = Arc::downgrade(&factory);
    let send = move || {
        if let Some(factory) = weak.upgrade() {
            wake(&factory);
        }
    };
    match facade.default_transactions() {
        Ok(tx) if tx.in_transaction() => {
            if let Err(err) = tx.on_commit(send) {
                tracing::warn!("Could not wait for the commit to wake the chat push worker: {err}");
                wake(&factory);
            }
        }
        _ => send(),
    }
}

/// `wake#ChatPush`: wake the worker once the caller's transaction commits.
fn wake_chat_push(
    _parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    wake_after_commit(facade);
    Ok(BTreeMap::new())
}

/// Start this node's worker thread, once (the host calls it after the cluster starts, so leases name the instance).
/// Its first round runs at once: the rows a crash left.
pub fn start(factory: &Arc<ExecutionContextFactory>) {
    if factory.entity().is_none() {
        return;
    }
    let state = worker(factory);
    if state.started.swap(true, Ordering::SeqCst) {
        return;
    }
    let weak: Weak<ExecutionContextFactory> = Arc::downgrade(factory);
    let spawned = std::thread::Builder::new()
        .name("moqui-chat-push".to_string())
        .spawn(framework_thread(move || {
            let mut first = true;
            loop {
                // the factory is held only for a round, not while waiting, so a stopped server releases it
                let worker = {
                    let Some(factory) = weak.upgrade() else {
                        return;
                    };
                    if factory.is_shutting_down() {
                        return;
                    }
                    worker(&factory)
                };
                if !first {
                    let woken = {
                        let guard = lock(&worker.wake);
                        let (mut guard, _) = worker
                            .signal
                            .wait_timeout_while(guard, SWEEP, |woken| !*woken)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        std::mem::take(&mut *guard)
                    };
                    if !woken && !worker.active.load(Ordering::SeqCst) {
                        continue;
                    }
                }
                first = false;
                let Some(factory) = weak.upgrade() else {
                    return;
                };
                if factory.is_shutting_down() {
                    return;
                }
                let Some(facade) = factory.entity() else {
                    return;
                };
                let context = factory.new_context();
                let _entered = context.enter();
                let _authz = context.disable_authz();
                // a panic is one round's: logged, and the next round goes on
                let outcome =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| round(&factory, &facade, now_millis())));
                match outcome {
                    Ok(Ok(report)) => {
                        worker.active.store(report.waiting > 0, Ordering::SeqCst);
                        if report.sent > 0 || report.failed > 0 {
                            tracing::debug!(
                                "chat push round: {} jobs, {} sent, {} failed, {} waiting",
                                report.jobs,
                                report.sent,
                                report.failed,
                                report.waiting
                            );
                        }
                    }
                    Ok(Err(error)) => tracing::error!("the chat push round failed: {error}"),
                    Err(_) => tracing::error!("the chat push round panicked"),
                }
            }
        }));
    match spawned {
        Ok(handle) => *lock(&state.thread) = Some(handle),
        Err(error) => tracing::error!("the chat push worker did not start: {error}"),
    }
}

/// Wake this node's worker to end, and wait for it. The factory calls it as it shuts down.
pub(crate) fn shutdown(factory: &ExecutionContextFactory) {
    let worker = worker(factory);
    let Some(handle) = lock(&worker.thread).take() else {
        return;
    };
    *lock(&worker.wake) = true;
    worker.signal.notify_all();
    if handle.thread().id() != std::thread::current().id() && handle.join().is_err() {
        tracing::error!("the chat push worker panicked");
    }
}

/// What a round did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoundReport {
    /// Jobs worked into the outbox.
    pub jobs: usize,
    /// Outbox rows written or merged.
    pub queued: usize,
    pub sent: usize,
    /// Pushes not taken that will be sent again.
    pub retried: usize,
    /// Pushes that ended without being sent.
    pub failed: usize,
    /// Pushes not sent because the user read the room first.
    pub dropped: usize,
    /// Jobs and pushes left waiting (pending or held).
    pub waiting: usize,
}

/// `run#ChatPush`: one round now (or at `nowMillis`, a test's clock).
fn run_chat_push(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let factory = current_factory().ok_or_else(|| LibError::new("no execution context"))?;
    let now = param(parameters, "nowMillis")
        .and_then(|text| text.parse::<i64>().ok())
        .unwrap_or_else(now_millis);
    let report = round(&factory, facade, now)?;
    let as_value = |count: usize| Value::I64(i64::try_from(count).unwrap_or(i64::MAX));
    Ok(BTreeMap::from([
        ("jobCount".to_string(), as_value(report.jobs)),
        ("queuedCount".to_string(), as_value(report.queued)),
        ("sentCount".to_string(), as_value(report.sent)),
        ("retriedCount".to_string(), as_value(report.retried)),
        ("failedCount".to_string(), as_value(report.failed)),
        ("droppedCount".to_string(), as_value(report.dropped)),
    ]))
}

/// One round at `now`: the jobs into the outbox, the held pushes of users no longer on do not disturb, the pushes due,
/// and the old rows once an hour. Each job, user and push is on its own: one that fails is logged and the rest go on.
///
/// # Errors
///
/// Returns an error when the jobs or the pushes due cannot be read.
pub fn round(factory: &ExecutionContextFactory, facade: &EntityFacade, now: i64) -> Result<RoundReport, LibError> {
    let transport = transport(factory);
    let node = node_id(factory);
    let mut report = RoundReport::default();
    fan_out(facade, transport.as_ref(), &node, now, &mut report)?;
    release_held(facade, now);
    deliver_due(facade, transport.as_ref(), &node, now, PUSHES_PER_ROUND, &mut report)?;
    clean(&worker(factory), facade, now);
    let waiting = facade.find(JOB)?.disable_authz().count()?
        + facade
            .find(OUTBOX)?
            .condition(Condition::is_in(
                "statusEnumId",
                vec![Value::string(PENDING), Value::string(HELD)],
            ))
            .disable_authz()
            .count()?;
    report.waiting = usize::try_from(waiting).unwrap_or(usize::MAX);
    Ok(report)
}

/// Take a row of `entity` (a job or a push) for this worker: in one transaction, read FOR UPDATE and checked to be
/// still at `attempts` (and pending, for a push) with no live lease, then leased with one more attempt. The row as it
/// was locked: what a send goes by, as another node may have merged a message into it since it was listed.
fn claim(
    facade: &EntityFacade,
    entity: &str,
    (key, id): (&str, &str),
    attempts: i64,
    node: &str,
    now: i64,
) -> Result<Option<EntityValue>, LibError> {
    force_new(facade, entity, || {
        let row = facade
            .find(entity)?
            .eq(key, Value::string(id))
            .use_cache(false)
            .for_update(true)
            .disable_authz()
            .one()?;
        let Some(mut row) = row else { return Ok(None) };
        let pending = entity != OUTBOX || text_of(&row, "statusEnumId").as_deref() == Some(PENDING);
        if !pending || count_of(&row, "attemptCount") != attempts || leased(&row, now) {
            return Ok(None);
        }
        row.set("attemptCount", Value::I64(attempts + 1));
        row.set("leaseOwner", Value::string(node));
        row.set("leaseExpiresDate", stamp(now + LEASE_MILLIS));
        row.update(facade)?;
        Ok(Some(row))
    })
}

// ---- 1. the jobs ----

fn fan_out(
    facade: &EntityFacade,
    transport: &dyn PushTransport,
    node: &str,
    now: i64,
    report: &mut RoundReport,
) -> Result<(), LibError> {
    let jobs = facade
        .find(JOB)?
        .order_by("createdDate", false)
        .limit(JOBS_PER_ROUND)
        .disable_authz()
        .list()?;
    for job in jobs.values() {
        let job_id = text_of(job, "chatPushJobId").unwrap_or_default();
        let attempts = count_of(job, "attemptCount");
        if leased(job, now) {
            continue;
        }
        match claim(facade, JOB, ("chatPushJobId", &job_id), attempts, node, now) {
            Ok(Some(_)) => {}
            Ok(None) => continue,
            Err(error) => {
                tracing::error!("chat push job {job_id} could not be taken: {error}");
                continue;
            }
        }
        match fan_out_job(facade, transport, job, now) {
            Ok(queued) => {
                report.jobs += 1;
                report.queued += queued;
            }
            Err(error) => {
                // taken again once its lease runs out, until it has had its tries
                tracing::error!("chat push job {job_id} failed: {error}");
                let kept = force_new(facade, JOB, || {
                    let find = facade.find(JOB)?.eq("chatPushJobId", Value::string(&job_id));
                    if attempts + 1 >= JOB_ATTEMPTS {
                        tracing::error!("chat push job {job_id} given up after {JOB_ATTEMPTS} tries");
                        find.disable_authz().delete_all()
                    } else {
                        find.disable_authz()
                            .update_all([("lastErrorText".to_string(), Value::string(error.to_string()))])
                    }
                });
                if let Err(error) = kept {
                    tracing::error!("chat push job {job_id} not updated: {error}");
                }
            }
        }
    }
    Ok(())
}

/// What a room's push says of an event, the same for each user but for the snippet.
struct Content {
    room_id: String,
    event_seq: i64,
    room_name: Option<String>,
    direct: bool,
    sender_name: String,
    snippet: Option<String>,
    encrypted: bool,
    sent_date: i64,
    link: String,
    /// An edit (that mentions someone): merged into a pending push of the room it is not another message.
    edit: bool,
    /// A call's event that pushes as a message (§7.2 rule 6): `missed` or `voicemail`.
    call: Option<&'static str>,
    /// From an outside actor of a bridge (a text from an outside number): the app's Texts channel.
    external: bool,
    /// A disappearing message's time (`expireDate`, phase 16): no snippet, and no push after it.
    expires: Option<i64>,
    /// An incoming text's one-time code (`content.otp.code`, plans/Comms-Messaging.md §18.3 phase 21), for the
    /// notification's Copy: in the payload only where the push may carry the message's words (rule 5).
    otp: Option<String>,
}

/// What a call's event (CetCall) pushes as (§7.2 rule 6): a missed call to the users it names, a voicemail to the
/// room; its other events (a call starting, an answered call ending) push nothing.
fn call_kind(event: &EntityValue) -> Option<&'static str> {
    if text_of(event, "eventTypeEnumId").as_deref() != Some("CetCall") {
        return None;
    }
    let content: serde_json::Value = text_of(event, "contentJson")
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    match content["action"].as_str() {
        Some("voicemail") => Some("voicemail"),
        Some("ended")
            if content["missedUserIdList"]
                .as_array()
                .is_some_and(|users| !users.is_empty()) =>
        {
            Some("missed")
        }
        _ => None,
    }
}

/// The title a browser's service worker falls back on, from the payload's own fields (the app and a new service worker
/// build theirs from the fields).
fn title_of(payload: &serde_json::Value) -> String {
    let sender = payload["senderName"].as_str().unwrap_or_default();
    let room = payload["roomName"].as_str().filter(|room| !room.is_empty());
    let place = room.unwrap_or(sender);
    let count = payload["count"].as_i64().unwrap_or(1);
    match (
        payload["call"].as_str(),
        room,
        payload["direct"].as_bool().unwrap_or(false),
    ) {
        (Some("missed"), _, _) => format!("Missed call in {place}"),
        (Some(_), _, _) => format!("Voicemail in {place}"),
        _ if count > 1 => format!("{count} new messages in {place}"),
        (None, Some(room), false) => format!("{sender} in {room}"),
        _ => sender.to_string(),
    }
}

impl Content {
    fn of(facade: &EntityFacade, room: &EntityValue, event: &EntityValue) -> Result<Self, LibError> {
        let room_id = text_of(room, "chatRoomId").unwrap_or_default();
        let encrypted =
            text_of(room, "encrypted").as_deref() == Some("Y") || text_of(event, "encrypted").as_deref() == Some("Y");
        let call = call_kind(event);
        let expires = millis_of(event, "expireDate");
        // a code (phase 21): its digits only, never from an encrypted room or a call's event
        let otp = if encrypted || call.is_some() {
            None
        } else {
            text_of(event, "contentJson")
                .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
                .and_then(|content| content["otp"]["code"].as_str().map(str::to_string))
                .filter(|code| (4..=8).contains(&code.len()) && code.chars().all(|c| c.is_ascii_digit()))
        };
        let snippet = if encrypted || call.is_some() || expires.is_some() {
            None
        } else {
            let text = text_of(event, "bodyText").unwrap_or_default();
            let shown = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if shown.is_empty() {
                // a message of files: their names
                let content: serde_json::Value = text_of(event, "contentJson")
                    .and_then(|json| serde_json::from_str(&json).ok())
                    .unwrap_or_default();
                let names: Vec<&str> = content["attachments"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|file| file["fileName"].as_str())
                    .collect();
                (!names.is_empty()).then(|| cut_chars(&format!("📎 {}", names.join(", ")), MAX_SNIPPET))
            } else {
                Some(cut_chars(&shown, MAX_SNIPPET))
            }
        };
        let root = crate::context::conf_env::setting("comms@app-root");
        let root = if root.trim().is_empty() {
            "/qapps/BasicComms".to_string()
        } else {
            root.trim().trim_end_matches('/').to_string()
        };
        let encoded: String = room_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || "-_.~".contains(c) {
                    c.to_string()
                } else {
                    let mut bytes = [0_u8; 4];
                    c.encode_utf8(&mut bytes)
                        .bytes()
                        .map(|byte| format!("%{byte:02X}"))
                        .collect()
                }
            })
            .collect();
        Ok(Self {
            link: format!("{root}/Chat?chatRoomId={encoded}"),
            event_seq: count_of(event, "eventSeq"),
            room_name: text_of(room, "roomName").map(|name| cut_chars(&name, MAX_NAME)),
            direct: text_of(room, "roomTypeEnumId").as_deref() == Some("CrtDirect"),
            sender_name: cut_chars(
                &super::chat_bridge::sender_name(facade, event)?.unwrap_or_default(),
                MAX_NAME,
            ),
            snippet,
            encrypted,
            sent_date: millis_of(event, "sentDate").unwrap_or(0),
            edit: text_of(event, "eventTypeEnumId").as_deref() == Some("CetEdit"),
            call,
            external: text_of(event, "senderUserId").is_none() && text_of(event, "senderActorId").is_some(),
            expires,
            otp,
            room_id,
        })
    }

    /// The payload of a message push (plans/Comms-Messaging.md §7.2 rule 5): the room, the sender's name and, when
    /// `preview` and the room is plain, a snippet; `count` messages merged, the newest's; for a call's event, `call`.
    /// `title` is a fallback built from these fields (title_of), `tag` the room's, which a device's notification of the
    /// room is replaced and closed by.
    fn payload(&self, preview: bool, count: i64, mention: bool) -> serde_json::Value {
        let mut payload = json!({
            "v": 1,
            "kind": "message",
            "chatRoomId": self.room_id,
            "eventSeq": self.event_seq,
            "roomName": self.room_name,
            "direct": self.direct,
            "senderName": self.sender_name,
            "snippet": if preview { self.snippet.clone() } else { None },
            "count": count,
            "mention": mention,
            "encrypted": self.encrypted,
            "call": self.call,
            "external": self.external,
            "sentDate": self.sent_date,
            "link": self.link,
            "tag": room_tag(&self.room_id),
            "topic": "ChatMessage",
        });
        // a one-time code for the notification's Copy (phase 21), only where the push carries the message's words: none
        // with previews off, in an encrypted room or for a disappearing message (its snippet is none there too)
        if let Some(code) = self.otp.as_ref().filter(|_| preview && self.snippet.is_some()) {
            payload["otp"] = json!({"code": code});
        }
        payload["title"] = title_of(&payload).into();
        payload
    }
}

/// The level a member's pushes follow (§7.2 rule 1).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    All,
    /// The user's mention, reply and thread reply rows; a muted room is here too.
    Mentions,
    None,
}

fn level_of(member: &EntityValue) -> Level {
    match text_of(member, "notificationLevelEnumId").as_deref() {
        Some("CnlNone") => Level::None,
        _ if text_of(member, "isMuted").as_deref() == Some("Y") => Level::Mentions,
        Some("CnlMentions") => Level::Mentions,
        _ => Level::All,
    }
}

/// Who gets the push of `event`, each with whether it mentions or replies to them (rules 1, 3 and 6): each member but
/// the sender and the agents (in a direct room also one who left), by level, not one who read it; a user it mentions
/// who is not a member (a public or record room); for a call's event, a missed call's users or a voicemail's room
/// (each member whose level is not None, as a mention would). Never a member whose room is out of their sight (Spam &
/// blocked: `filterEnumId`), nor a user who blocked a local sender (`ChatUserBlock`; plans/Comms-Messaging.md phase 11).
fn recipients(
    facade: &EntityFacade,
    room: &EntityValue,
    event: &EntityValue,
) -> Result<BTreeMap<String, bool>, LibError> {
    let room_id = text_of(room, "chatRoomId").unwrap_or_default();
    let seq = count_of(event, "eventSeq");
    let sender = text_of(event, "senderUserId");
    let edit = text_of(event, "eventTypeEnumId").as_deref() == Some("CetEdit");
    let direct = text_of(room, "roomTypeEnumId").as_deref() == Some("CrtDirect");
    let call = call_kind(event);
    // a missed call is pushed to those who missed it
    let missed: Option<HashSet<String>> = (call == Some("missed")).then(|| {
        let content: serde_json::Value = text_of(event, "contentJson")
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        content["missedUserIdList"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|user| user.as_str().map(str::to_string))
            .collect()
    });
    let mentioned: BTreeSet<String> = facade
        .find(ACTIVITY)?
        .eq("chatRoomId", Value::string(&room_id))
        .eq("eventSeq", Value::I64(seq))
        .condition(Condition::is_in(
            "activityKindEnumId",
            vec![
                Value::string("CuaMention"),
                Value::string("CuaReply"),
                Value::string("CuaThreadReply"),
            ],
        ))
        .disable_authz()
        .list()?
        .values()
        .iter()
        .filter_map(|row| text_of(row, "userId"))
        .collect();
    let mut out = BTreeMap::new();
    let mut members = HashSet::new();
    let rows = facade
        .find(MEMBER)?
        .eq("chatRoomId", Value::string(&room_id))
        .disable_authz()
        .list()?;
    let blockers = blockers_of(facade, sender.as_deref())?;
    for member in rows.values() {
        let Some(user) = text_of(member, "userId") else {
            continue;
        };
        members.insert(user.clone());
        let gone = !direct && text_of(member, "leftDate").is_some();
        let agent = text_of(member, "roleEnumId").as_deref() == Some("CmrAgent");
        let filtered = text_of(member, "filterEnumId").is_some() || blockers.contains(&user);
        if sender.as_deref() == Some(user.as_str())
            || gone
            || agent
            || filtered
            || count_of(member, "lastReadEventSeq") >= seq
        {
            continue;
        }
        let mention = mentioned.contains(&user);
        let level = level_of(member);
        let pushed = match (call, &missed) {
            (Some(_), Some(missed)) => level != Level::None && missed.contains(&user),
            (Some(_), None) => level != Level::None,
            _ => match level {
                Level::None => false,
                Level::Mentions => mention,
                Level::All => mention || !edit,
            },
        };
        if pushed {
            out.insert(user, mention);
        }
    }
    if call.is_none() {
        for user in mentioned {
            if !members.contains(&user) && sender.as_deref() != Some(user.as_str()) && !blockers.contains(&user) {
                out.insert(user, true);
            }
        }
    }
    Ok(out)
}

/// The users who blocked the local user `sender` in chat (`ChatUserBlock`); none for an external sender.
fn blockers_of(facade: &EntityFacade, sender: Option<&str>) -> Result<HashSet<String>, LibError> {
    let Some(sender) = sender.filter(|sender| !sender.is_empty()) else {
        return Ok(HashSet::new());
    };
    Ok(facade
        .find("moqui.chat.ChatUserBlock")?
        .eq("blockedUserId", Value::string(sender))
        .disable_authz()
        .list()?
        .values()
        .iter()
        .filter_map(|row| text_of(row, "userId"))
        .collect())
}

/// Work one job: its recipients' devices get a push each (written or merged), each in a transaction of its own, so one
/// device that fails is logged and the rest go on; then the job goes. Out: the rows written or merged. A job worked
/// again (its lease ran out during a long fan-out, a crash) writes nothing twice: a device that has a push of the room
/// at this event or later is left as it is ([`queue_message`]).
fn fan_out_job(
    facade: &EntityFacade,
    transport: &dyn PushTransport,
    job: &EntityValue,
    now: i64,
) -> Result<usize, LibError> {
    let job_id = text_of(job, "chatPushJobId").unwrap_or_default();
    let room_id = text_of(job, "chatRoomId").unwrap_or_default();
    let seq = count_of(job, "eventSeq");
    let delete_job = || {
        force_new(facade, JOB, || {
            facade
                .find(JOB)?
                .eq("chatPushJobId", Value::string(&job_id))
                .disable_authz()
                .delete_all()
        })
    };
    let room = facade
        .find(ROOM)?
        .eq("chatRoomId", Value::string(&room_id))
        .disable_authz()
        .one()?;
    let event = facade
        .find(EVENT)?
        .eq("chatRoomId", Value::string(&room_id))
        .eq("eventSeq", Value::I64(seq))
        .disable_authz()
        .one()?;
    // a message deleted (or its room) before its push: nothing to say
    let (Some(room), Some(event)) = (room, event.filter(|event| text_of(event, "redactedDate").is_none())) else {
        delete_job()?;
        return Ok(0);
    };
    let recipients = recipients(facade, &room, &event)?;
    // only users who may still sign in: an account disabled for good, or whose terminateDate passed (which no entity
    // ECA sees), gets nothing, sent at once or later (SEC-COM-92)
    let users = signing_in(facade, recipients.keys().cloned().collect(), now)?;
    let devices = if users.is_empty() {
        Vec::new()
    } else {
        facade
            .find(DEVICE)?
            .condition(Condition::is_in("userId", users.iter().map(Value::string).collect()))
            .condition(Condition::is_null("disabledDate"))
            .disable_authz()
            .list()?
            .values()
            .to_vec()
    };
    if devices.is_empty() {
        delete_job()?;
        return Ok(0);
    }
    let hidden = preview_off(facade, &users)?;
    let dnd = on_dnd(facade, &users, now)?;
    let content = Content::of(facade, &room, &event)?;
    let mut queued = 0;
    for device in &devices {
        let Some(user) = text_of(device, "userId") else {
            continue;
        };
        let Some(mention) = recipients.get(&user) else {
            continue;
        };
        if !transport.available(Provider::of(device)) {
            continue;
        }
        let push = Push {
            user: &user,
            mention: *mention,
            preview: !hidden.contains(&user),
            held: dnd.contains(&user),
        };
        match force_new(facade, OUTBOX, || queue_message(facade, device, &content, &push, now)) {
            Ok(true) => queued += 1,
            Ok(false) => {}
            Err(error) => tracing::error!(
                subscription_id = %text_of(device, "subscriptionId").unwrap_or_default(),
                "the push of chat event {room_id}::{seq} was not queued for a device: {error}"
            ),
        }
    }
    delete_job()?;
    Ok(queued)
}

/// One user's push of an event.
struct Push<'a> {
    user: &'a str,
    mention: bool,
    preview: bool,
    /// The user is on do not disturb: held until it ends.
    held: bool,
}

/// Write the push of `content` to `device`, or merge it into the device's pending or held push of the room that no
/// worker holds: one push per room, with the count (an edit is not another message). Nothing when the device already
/// has a push of the room at this event or later (the job worked again). Out: whether a row was written or merged.
fn queue_message(
    facade: &EntityFacade,
    device: &EntityValue,
    content: &Content,
    push: &Push<'_>,
    now: i64,
) -> Result<bool, LibError> {
    let subscription = text_of(device, "subscriptionId").unwrap_or_default();
    let status = if push.held { HELD } else { PENDING };
    let rows = facade
        .find(OUTBOX)?
        .eq("subscriptionId", Value::string(&subscription))
        .eq("chatRoomId", Value::string(&content.room_id))
        .eq("kindEnumId", Value::string(KIND_MESSAGE))
        .condition(Condition::greater_equal("eventSeq", Value::I64(content.event_seq)))
        .disable_authz()
        .count()?;
    if rows > 0 {
        return Ok(false);
    }
    let open = facade
        .find(OUTBOX)?
        .eq("subscriptionId", Value::string(&subscription))
        .eq("chatRoomId", Value::string(&content.room_id))
        .eq("kindEnumId", Value::string(KIND_MESSAGE))
        .condition(Condition::is_in(
            "statusEnumId",
            vec![Value::string(PENDING), Value::string(HELD)],
        ))
        .use_cache(false)
        .for_update(true)
        .disable_authz()
        .list()?
        .values()
        .iter()
        .find(|row| !leased(row, now))
        .cloned();
    // a disappearing message's push is not sent past the message's own time (phase 16)
    let ttl_end = now + MESSAGE_TTL_SECONDS * 1000;
    let ends = content.expires.map_or(ttl_end, |end| end.min(ttl_end));
    let expire = stamp(ends);
    if let Some(mut row) = open {
        // merged, the push stands for the earlier messages too: it lasts as long as the longest of them
        let expire = stamp(millis_of(&row, "expireDate").map_or(ends, |kept| kept.max(ends)));
        let before = count_of(&row, "messageCount").max(1);
        let count = if content.edit { before } else { before + 1 };
        let mention = push.mention || text_of(&row, "mention").as_deref() == Some("Y");
        row.set("eventSeq", Value::I64(content.event_seq));
        row.set("messageCount", Value::I64(count));
        row.set("mention", Value::string(if mention { "Y" } else { "N" }));
        row.set(
            "payloadJson",
            Value::string(content.payload(push.preview, count, mention).to_string()),
        );
        row.set("statusEnumId", Value::string(status));
        row.set("expireDate", expire);
        row.update(facade)?;
        return Ok(true);
    }
    let mut row = facade.make_value(OUTBOX)?;
    row.set_sequenced_id_primary(facade)?;
    row.set("subscriptionId", Value::string(&subscription));
    row.set("userId", Value::string(push.user));
    row.set("chatRoomId", Value::string(&content.room_id));
    row.set("eventSeq", Value::I64(content.event_seq));
    row.set("kindEnumId", Value::string(KIND_MESSAGE));
    row.set("statusEnumId", Value::string(status));
    row.set("messageCount", Value::I64(1));
    row.set("mention", Value::string(if push.mention { "Y" } else { "N" }));
    row.set(
        "payloadJson",
        Value::string(content.payload(push.preview, 1, push.mention).to_string()),
    );
    row.set("attemptCount", Value::I64(0));
    row.set("nextAttemptDate", stamp(now));
    row.set("createdDate", stamp(now));
    row.set("expireDate", expire);
    row.create(facade)?;
    Ok(true)
}

// ---- 2. do not disturb ends ----

/// The held pushes of each user no longer on do not disturb become pending, at once: one summary per room and device,
/// as they were merged. Each user on their own.
fn release_held(facade: &EntityFacade, now: i64) {
    let held = facade.find(OUTBOX).and_then(|find| {
        find.eq("statusEnumId", Value::string(HELD))
            .select_field("userId")
            .disable_authz()
            .list()
    });
    let users: Vec<String> = match held {
        Ok(rows) => rows
            .values()
            .iter()
            .filter_map(|row| text_of(row, "userId"))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        Err(error) => {
            tracing::error!("the held chat pushes could not be read: {error}");
            return;
        }
    };
    let dnd = match on_dnd(facade, &users, now) {
        Ok(dnd) => dnd,
        Err(error) => {
            tracing::error!("the do not disturb statuses could not be read: {error}");
            return;
        }
    };
    for user in users.iter().filter(|user| !dnd.contains(*user)) {
        let released = force_new(facade, OUTBOX, || {
            facade
                .find(OUTBOX)?
                .eq("userId", Value::string(user))
                .eq("statusEnumId", Value::string(HELD))
                .disable_authz()
                .update_all([
                    ("statusEnumId".to_string(), Value::string(PENDING)),
                    ("nextAttemptDate".to_string(), stamp(now)),
                ])
        });
        if let Err(error) = released {
            tracing::error!("the held chat pushes of user {user} were not released: {error}");
        }
    }
}

// ---- 3. the sends ----

enum Step {
    Sent,
    Retried,
    Failed,
    Dropped,
    /// Held by another worker, or taken meanwhile.
    Skipped,
}

fn deliver_due(
    facade: &EntityFacade,
    transport: &dyn PushTransport,
    node: &str,
    now: i64,
    limit: i64,
    report: &mut RoundReport,
) -> Result<(), LibError> {
    // each recipient's pushes take at most the transport's budget of the round. The rest of a user whose budget is
    // spent wait for the next round, and the pushes due are read again, so a user with many old pushes does not hold
    // every other user's for the round's whole read (SEC-COM-81)
    let mut spent = UserBudget::new(transport.user_round_budget());
    for _ in 0..DELIVERY_PASSES {
        let rows = facade
            .find(OUTBOX)?
            .eq("statusEnumId", Value::string(PENDING))
            .condition(Condition::or(vec![
                Condition::is_null("nextAttemptDate"),
                Condition::less_equal("nextAttemptDate", stamp(now)),
            ]))
            .order_by("createdDate", false)
            .limit(limit)
            .disable_authz()
            .list()?;
        let mut waiting = BTreeSet::new();
        for row in rows.values() {
            let user = text_of(row, "userId").unwrap_or_default();
            if spent.exhausted(&user) {
                waiting.insert(user);
                continue;
            }
            let started = std::time::Instant::now();
            let step = deliver_one(facade, transport, row, node, now);
            spent.add(&user, started.elapsed());
            match step {
                Ok(Step::Sent) => report.sent += 1,
                Ok(Step::Retried) => report.retried += 1,
                Ok(Step::Failed) => report.failed += 1,
                Ok(Step::Dropped) => report.dropped += 1,
                Ok(Step::Skipped) => {}
                Err(error) => tracing::error!(
                    "chat push {} could not be sent: {error}",
                    text_of(row, "pushOutboxId").unwrap_or_default()
                ),
            }
        }
        if waiting.is_empty() {
            break;
        }
        for user in &waiting {
            put_off(facade, user, now);
        }
    }
    Ok(())
}

/// Put `user`'s pushes due at `now` off to the next round (just after `now`), so the round's next read and the
/// next round's start with other users' pushes. A user whose pushes are not put off is logged; the others go on.
fn put_off(facade: &EntityFacade, user: &str, now: i64) {
    let moved = force_new(facade, OUTBOX, || {
        facade
            .find(OUTBOX)?
            .eq("userId", Value::string(user))
            .eq("statusEnumId", Value::string(PENDING))
            .condition(Condition::or(vec![
                Condition::is_null("nextAttemptDate"),
                Condition::less_equal("nextAttemptDate", stamp(now)),
            ]))
            .disable_authz()
            .update_all([("nextAttemptDate".to_string(), stamp(now + 1))])
    });
    if let Err(error) = moved {
        tracing::error!("the chat pushes of user {user} were not put off to the next round: {error}");
    }
}

/// The time each user's pushes took of a delivery round, against a budget per user.
pub(crate) struct UserBudget {
    budget: std::time::Duration,
    spent: std::collections::HashMap<String, std::time::Duration>,
}

impl UserBudget {
    pub(crate) fn new(budget: std::time::Duration) -> Self {
        Self {
            budget,
            spent: std::collections::HashMap::new(),
        }
    }

    pub(crate) fn add(&mut self, user: &str, took: std::time::Duration) {
        *self.spent.entry(user.to_string()).or_default() += took;
    }

    /// Whether `user`'s pushes have taken the whole budget of this round.
    pub(crate) fn exhausted(&self, user: &str) -> bool {
        self.spent.get(user).is_some_and(|spent| *spent >= self.budget)
    }
}

/// The FCM `messages:send` body of a chat push: a data message of its kind (`chat` or `clear`) and `pushId`, the
/// registration's id as on a ring (which account of the app it is for, so which key opens it), the payload sealed to
/// the device (`enc`, base64url) when it gave its keys, else nothing more; with the device's collapse key of the room
/// ([`collapse_key`]).
pub fn fcm_body(
    token: &str,
    device: &str,
    clear: bool,
    sealed: Option<&[u8]>,
    ttl_seconds: i64,
    room_id: &str,
) -> serde_json::Value {
    let collapse = collapse_key(device, room_id, clear);
    let mut data = serde_json::Map::new();
    data.insert("kind".to_string(), (if clear { "clear" } else { "chat" }).into());
    data.insert("pushId".to_string(), device.into());
    if let Some(sealed) = sealed {
        data.insert("enc".to_string(), URL_SAFE_NO_PAD.encode(sealed).into());
    }
    json!({"message": {
        "token": token,
        "data": data,
        "android": {
            // high only for a notification that shows: Android takes high priority from an app whose high pushes show nothing
            "priority": if clear { "NORMAL" } else { "HIGH" },
            "ttl": format!("{ttl_seconds}s"),
            "collapse_key": collapse,
        },
    }})
}

/// Why a message push written when its job was worked is no longer sent (SEC-COM-92), or None: its event was deleted
/// (`redacted`), the user left the room, not a direct one (`left`), or the account may no longer sign in, disabled for
/// good (not a lockout after failed logins, which ends by itself) or terminated (`accountDisabled`).
fn gone_since_queued(
    facade: &EntityFacade,
    user: &str,
    room_id: &str,
    seq: i64,
    now: i64,
) -> Result<Option<&'static str>, LibError> {
    let event = facade
        .find(EVENT)?
        .eq("chatRoomId", Value::string(room_id))
        .eq("eventSeq", Value::I64(seq))
        .disable_authz()
        .one()?;
    if event.is_none_or(|event| text_of(&event, "redactedDate").is_some()) {
        return Ok(Some("redacted"));
    }
    let direct = facade
        .find(ROOM)?
        .eq("chatRoomId", Value::string(room_id))
        .disable_authz()
        .one()?
        .is_some_and(|room| text_of(&room, "roomTypeEnumId").as_deref() == Some("CrtDirect"));
    let member = facade
        .find(MEMBER)?
        .eq("chatRoomId", Value::string(room_id))
        .eq("userId", Value::string(user))
        .disable_authz()
        .list()?;
    let in_room = member
        .values()
        .iter()
        .any(|member| direct || text_of(member, "leftDate").is_none());
    if !in_room {
        return Ok(Some("left"));
    }
    let signs_in = !signing_in(facade, vec![user.to_string()], now)?.is_empty();
    Ok((!signs_in).then_some("accountDisabled"))
}

/// Of `users`, those whose account may sign in at `now`, in one query: not disabled for good (a lockout after failed
/// logins, which has a disabledDateTime, ends by itself) and not past its terminateDate.
fn signing_in(facade: &EntityFacade, users: Vec<String>, now: i64) -> Result<Vec<String>, LibError> {
    if users.is_empty() {
        return Ok(users);
    }
    let accounts = facade
        .find("moqui.security.UserAccount")?
        .condition(Condition::is_in("userId", users.iter().map(Value::string).collect()))
        .disable_authz()
        .list()?;
    let may: std::collections::HashSet<String> = accounts
        .values()
        .iter()
        .filter(|account| {
            let disabled =
                text_of(account, "disabled").as_deref() == Some("Y") && text_of(account, "disabledDateTime").is_none();
            let terminated = millis_of(account, "terminateDate").is_some_and(|end| end <= now);
            !disabled && !terminated
        })
        .filter_map(|account| text_of(account, "userId"))
        .collect();
    Ok(users.into_iter().filter(|user| may.contains(user)).collect())
}

/// Whether the device has a newer push of the room than `seq` that the user has not read: one waiting, or one shown
/// and not cleared. A clear up to `seq` would remove its notification, so none is sent.
fn newer_unread(facade: &EntityFacade, subscription: &str, room_id: &str, seq: i64) -> Result<bool, LibError> {
    Ok(facade
        .find(OUTBOX)?
        .eq("subscriptionId", Value::string(subscription))
        .eq("chatRoomId", Value::string(room_id))
        .eq("kindEnumId", Value::string(KIND_MESSAGE))
        .condition(Condition::greater("eventSeq", Value::I64(seq)))
        .condition(Condition::or(vec![
            Condition::is_in("statusEnumId", vec![Value::string(PENDING), Value::string(HELD)]),
            Condition::and(vec![
                Condition::equals("statusEnumId", Value::string(SENT)),
                Condition::is_null("clearedDate"),
            ]),
        ]))
        .disable_authz()
        .count()?
        > 0)
}

fn deliver_one(
    facade: &EntityFacade,
    transport: &dyn PushTransport,
    row: &EntityValue,
    node: &str,
    now: i64,
) -> Result<Step, LibError> {
    let id = text_of(row, "pushOutboxId").unwrap_or_default();
    let attempts = count_of(row, "attemptCount");
    if leased(row, now) {
        return Ok(Step::Skipped);
    }
    let subscription = text_of(row, "subscriptionId").unwrap_or_default();
    let user = text_of(row, "userId").unwrap_or_default();
    let room_id = text_of(row, "chatRoomId").unwrap_or_default();
    let clear = text_of(row, "kindEnumId").as_deref() == Some(KIND_CLEAR);
    let reminder = text_of(row, "kindEnumId").as_deref() == Some(KIND_REMINDER);
    let fail = |reason: &str, error: Option<&str>| -> Result<Step, LibError> {
        finish(facade, &id, attempts, FAILED, Some(reason), error, now)?;
        device_result(facade, &subscription, DeviceResult::Failed, now);
        Ok(Step::Failed)
    };
    let Some(expires) = millis_of(row, "expireDate").filter(|end| *end > now) else {
        return fail(
            "expired",
            Some("Its time to live ended before its push service took it"),
        );
    };
    // read since it was queued (rule 3); a reminder is of a message usually read already (rule 9)
    if !clear && !reminder && read_past(facade, &user, &room_id, count_of(row, "eventSeq"))? {
        finish(facade, &id, attempts, DROPPED, Some("read"), None, now)?;
        return Ok(Step::Dropped);
    }
    let device = facade
        .find(DEVICE)?
        .eq("subscriptionId", Value::string(&subscription))
        .disable_authz()
        .one()?;
    let Some(device) = device.filter(|device| text_of(device, "disabledDate").is_none()) else {
        finish(facade, &id, attempts, FAILED, Some("deviceGone"), None, now)?;
        return Ok(Step::Failed);
    };
    // a browser or app now signed in as someone else gets nothing of the earlier user's
    if text_of(&device, "userId").as_deref() != Some(user.as_str()) {
        finish(facade, &id, attempts, DROPPED, Some("deviceMoved"), None, now)?;
        return Ok(Step::Dropped);
    }
    // a clear that would remove the notification of a newer message not read
    if clear && newer_unread(facade, &subscription, &room_id, count_of(row, "eventSeq"))? {
        finish(facade, &id, attempts, DROPPED, Some("newer"), None, now)?;
        return Ok(Step::Dropped);
    }
    // a push that waited (do not disturb, its push service; written WAITED_CHECK or more ago) is of what may have
    // changed since it was written; one sent at once is not read again, and a test push is of no room
    let waited = millis_of(row, "createdDate").is_some_and(|created| now - created >= WAITED_CHECK);
    if !clear && waited && room_id != TEST_ROOM {
        if let Some(reason) = gone_since_queued(facade, &user, &room_id, count_of(row, "eventSeq"), now)? {
            finish(facade, &id, attempts, DROPPED, Some(reason), None, now)?;
            return Ok(Step::Dropped);
        }
    }
    let provider = Provider::of(&device);
    // a native app's registration ends with its device key
    if provider != Provider::Web {
        let key = text_of(&device, "loginKey").unwrap_or_default();
        if !super::web_push::live_device_keys(facade, vec![Value::string(&key)])?.contains(&key) {
            tracing::info!(subscription_id = %subscription, "native push registration's device key has ended; deleted");
            delete_device(facade, &subscription);
            finish(facade, &id, attempts, FAILED, Some("deviceKeyEnded"), None, now)?;
            return Ok(Step::Failed);
        }
    }
    // the row as locked: another node may have merged a message into it since it was listed
    let Some(row) = claim(facade, OUTBOX, ("pushOutboxId", &id), attempts, node, now)? else {
        return Ok(Step::Skipped);
    };
    let row = &row;
    let expires = millis_of(row, "expireDate").unwrap_or(expires);
    let attempt = attempts + 1;
    let mut payload: serde_json::Value = text_of(row, "payloadJson")
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_else(|| json!({}));
    // the registration it is for (as on a ring), and what the device acknowledges it with (ack#Push)
    payload["pushId"] = subscription.clone().into();
    payload["ackId"] = id.clone().into();
    let plain = payload.to_string();
    let keys = text_of(&device, "p256dhKey").zip(text_of(&device, "authSecret"));
    let sealed = match &keys {
        Some((p256dh, auth)) => match push_crypto::encrypt(plain.as_bytes(), p256dh, auth) {
            Ok(sealed) => Some(sealed),
            Err(reason) => {
                finish(facade, &id, attempt, FAILED, Some("encrypt"), Some(&reason), now)?;
                device_result(facade, &subscription, DeviceResult::Failed, now);
                return Ok(Step::Failed);
            }
        },
        None => None,
    };
    let ttl_seconds = ((expires - now) / 1000).max(1);
    let endpoint = text_of(&device, "endpoint").unwrap_or_default();
    // a reminder's collapse key is its own: it replaces no push of the room, and no later one of the room replaces it
    let collapse_room = if reminder {
        reminder_collapse_room(&room_id, count_of(row, "eventSeq"))
    } else {
        room_id.clone()
    };
    let started = std::time::Instant::now();
    let sent = match (provider, sealed) {
        (Provider::Fcm, sealed) => transport.fcm(
            &subscription,
            &fcm_body(
                &endpoint,
                &subscription,
                clear,
                sealed.as_deref(),
                ttl_seconds,
                &collapse_room,
            ),
        ),
        (_, Some(body)) => transport.web(
            &subscription,
            provider,
            &WebPost {
                endpoint: &endpoint,
                body,
                ttl_seconds,
                urgency: if clear { "normal" } else { "high" },
                topic: &collapse_key(&subscription, &collapse_room, clear),
            },
        ),
        // a browser or distributor always has its keys; one without cannot be pushed to
        (_, None) => Sent::Refused("the device has no push keys".to_string()),
    };
    match sent {
        Sent::Delivered => {
            finish_sent(facade, row, attempt, now)?;
            // an endpoint that holds the one push worker most of its timeout counts as failing, toward its disabling,
            // as one that never answers does: a host a browser names cannot stall every user's pushes by answering late
            // (plans/Security-Review-Medium.md SEC-COM-81)
            let slow = provider != Provider::Fcm && started.elapsed() >= transport.slow_after();
            if slow {
                let took = started.elapsed();
                tracing::info!(subscription_id = %subscription, "chat push {id} was taken only after {took:?}");
            }
            let result = if slow { DeviceResult::Failed } else { DeviceResult::Delivered };
            device_result(facade, &subscription, result, now);
            Ok(Step::Sent)
        }
        Sent::Gone => {
            finish(facade, &id, attempt, FAILED, Some("gone"), None, now)?;
            if provider == Provider::Fcm {
                tracing::info!(subscription_id = %subscription, "FCM registration gone (UNREGISTERED); deleted");
                delete_device(facade, &subscription);
            } else {
                device_result(facade, &subscription, DeviceResult::Gone, now);
            }
            Ok(Step::Failed)
        }
        Sent::Refused(error) => {
            tracing::warn!(subscription_id = %subscription, "chat push {id} refused: {error}");
            finish(facade, &id, attempt, FAILED, Some("refused"), Some(&error), now)?;
            device_result(facade, &subscription, DeviceResult::Failed, now);
            Ok(Step::Failed)
        }
        Sent::Retry { after_millis, error } => {
            if attempt >= MAX_ATTEMPTS {
                finish(facade, &id, attempt, FAILED, Some("attempts"), Some(&error), now)?;
                device_result(facade, &subscription, DeviceResult::Failed, now);
                return Ok(Step::Failed);
            }
            let wait = backoff(&id, attempt, after_millis);
            force_new(facade, OUTBOX, || {
                facade
                    .find(OUTBOX)?
                    .eq("pushOutboxId", Value::string(&id))
                    .eq("statusEnumId", Value::string(PENDING))
                    .eq("attemptCount", Value::I64(attempt))
                    .disable_authz()
                    .update_all([
                        ("lastErrorText".to_string(), Value::string(&error)),
                        ("nextAttemptDate".to_string(), stamp(now + wait)),
                        ("leaseOwner".to_string(), Value::Null),
                        ("leaseExpiresDate".to_string(), Value::Null),
                    ])
            })?;
            Ok(Step::Retried)
        }
    }
}

/// End a push's send: its status, reason and error, and no lease; only while it is still pending at `attempt` (a read
/// meanwhile dropped it, or another worker took it over).
fn finish(
    facade: &EntityFacade,
    id: &str,
    attempt: i64,
    status: &str,
    reason: Option<&str>,
    error: Option<&str>,
    now: i64,
) -> Result<(), LibError> {
    let text = |value: Option<&str>| value.map_or(Value::Null, Value::string);
    force_new(facade, OUTBOX, || {
        let mut fields = vec![
            ("statusEnumId".to_string(), Value::string(status)),
            ("reasonCode".to_string(), text(reason)),
            ("leaseOwner".to_string(), Value::Null),
            ("leaseExpiresDate".to_string(), Value::Null),
            ("nextAttemptDate".to_string(), Value::Null),
        ];
        if error.is_some() {
            fields.push(("lastErrorText".to_string(), text(error)));
        }
        if status == DROPPED {
            fields.push(("clearedDate".to_string(), stamp(now)));
        }
        facade
            .find(OUTBOX)?
            .eq("pushOutboxId", Value::string(id))
            .eq("statusEnumId", Value::string(PENDING))
            .eq("attemptCount", Value::I64(attempt))
            .disable_authz()
            .update_all(fields)
    })?;
    Ok(())
}

/// A push its push service took. One a read dropped while it was on its way did reach the device: its room's clear
/// follows it at once, so the device does not keep a notification of what was read.
fn finish_sent(facade: &EntityFacade, row: &EntityValue, attempt: i64, now: i64) -> Result<(), LibError> {
    let id = text_of(row, "pushOutboxId").unwrap_or_default();
    let clear_after = force_new(facade, OUTBOX, || {
        let current = facade
            .find(OUTBOX)?
            .eq("pushOutboxId", Value::string(&id))
            .use_cache(false)
            .for_update(true)
            .disable_authz()
            .one()?;
        let Some(mut current) = current else { return Ok(None) };
        let status = text_of(&current, "statusEnumId").unwrap_or_default();
        let dropped = status == DROPPED;
        if !(status == PENDING && count_of(&current, "attemptCount") == attempt) && !dropped {
            return Ok(None);
        }
        current.set("statusEnumId", Value::string(SENT));
        current.set("sentDate", stamp(now));
        current.set("reasonCode", Value::Null);
        current.set("lastErrorText", Value::Null);
        current.set("leaseOwner", Value::Null);
        current.set("leaseExpiresDate", Value::Null);
        current.set("nextAttemptDate", Value::Null);
        if dropped {
            current.set("clearedDate", stamp(now));
        }
        current.update(facade)?;
        Ok(dropped.then(|| count_of(&current, "eventSeq")))
    })?;
    if let Some(seq) = clear_after {
        let user = text_of(row, "userId").unwrap_or_default();
        let room_id = text_of(row, "chatRoomId").unwrap_or_default();
        let subscription = text_of(row, "subscriptionId").unwrap_or_default();
        force_new(facade, OUTBOX, || {
            queue_clear(facade, &user, &subscription, &room_id, seq, now)
        })?;
    }
    Ok(())
}

enum DeviceResult {
    Delivered,
    Failed,
    Gone,
}

/// Keep a push's result on its device (the health counts): a send taken, one that ended without being sent (a browser
/// or distributor failing [`super::web_push::MAX_FAILURES`] in a row is disabled), or the device gone (disabled).
fn device_result(facade: &EntityFacade, subscription: &str, result: DeviceResult, now: i64) {
    let kept = force_new(facade, DEVICE, || {
        let row = facade
            .find(DEVICE)?
            .eq("subscriptionId", Value::string(subscription))
            .use_cache(false)
            .for_update(true)
            .disable_authz()
            .one()?;
        let Some(mut row) = row else { return Ok(()) };
        let plus = |row: &EntityValue, field: &str| Value::I64(count_of(row, field) + 1);
        match result {
            DeviceResult::Delivered => {
                row.set("sentCount", plus(&row, "sentCount"));
                row.set("lastPushDate", stamp(now));
                row.set("lastSuccessDate", stamp(now));
                row.set("failureCount", Value::I64(0));
            }
            DeviceResult::Failed => {
                let failures = count_of(&row, "failureCount") + 1;
                row.set("failedCount", plus(&row, "failedCount"));
                row.set("failureCount", Value::I64(failures));
                row.set("lastFailureDate", stamp(now));
                if failures >= super::web_push::MAX_FAILURES && Provider::of(&row) != Provider::Fcm {
                    tracing::info!(subscription_id = %subscription, "push subscription failed {failures} times; disabled");
                    row.set("disabledDate", stamp(now));
                }
            }
            DeviceResult::Gone => {
                tracing::info!(subscription_id = %subscription, "push subscription gone at its push service; disabled");
                row.set("failedCount", plus(&row, "failedCount"));
                row.set("lastFailureDate", stamp(now));
                row.set("disabledDate", stamp(now));
            }
        }
        row.update(facade)
    });
    if let Err(error) = kept {
        tracing::warn!(subscription_id = %subscription, "push result not kept: {error}");
    }
}

/// Delete an FCM or UnifiedPush registration (gone at its push service, or its device key ended); logged when it
/// cannot be.
fn delete_device(facade: &EntityFacade, subscription: &str) {
    let deleted = force_new(facade, DEVICE, || {
        facade
            .find(DEVICE)?
            .eq("subscriptionId", Value::string(subscription))
            .disable_authz()
            .delete_all()
    });
    if let Err(error) = deleted {
        tracing::warn!(subscription_id = %subscription, "push registration not deleted: {error}");
    }
}

// ---- 4. cleaning ----

/// Remove the pushes done more than [`KEEP_MILLIS`] ago, once an hour.
fn clean(worker: &Worker, facade: &EntityFacade, now: i64) {
    {
        let mut cleaned = lock(&worker.cleaned);
        if cleaned.is_some_and(|at| now - at < 3_600_000) {
            return;
        }
        *cleaned = Some(now);
    }
    let result = force_new(facade, OUTBOX, || {
        facade
            .find(OUTBOX)?
            .condition(Condition::is_in(
                "statusEnumId",
                vec![Value::string(SENT), Value::string(FAILED), Value::string(DROPPED)],
            ))
            .condition(Condition::less("createdDate", stamp(now - KEEP_MILLIS)))
            .disable_authz()
            .delete_all()
    });
    if let Err(error) = result {
        tracing::error!("Could not remove the old chat pushes: {error}");
    }
}

// ---- read on another device, acknowledgements ----

/// Queue a clear of the room for the device, merged into its pending clear of the room that no worker holds.
fn queue_clear(
    facade: &EntityFacade,
    user: &str,
    subscription: &str,
    room_id: &str,
    seq: i64,
    now: i64,
) -> Result<(), LibError> {
    let payload = json!({"v": 1, "kind": "clear", "chatRoomId": room_id, "lastReadEventSeq": seq,
        "tag": room_tag(room_id)})
    .to_string();
    let open = facade
        .find(OUTBOX)?
        .eq("subscriptionId", Value::string(subscription))
        .eq("chatRoomId", Value::string(room_id))
        .eq("kindEnumId", Value::string(KIND_CLEAR))
        .eq("statusEnumId", Value::string(PENDING))
        .use_cache(false)
        .for_update(true)
        .disable_authz()
        .list()?
        .values()
        .iter()
        .find(|row| !leased(row, now))
        .cloned();
    let expire = stamp(now + CLEAR_TTL_SECONDS * 1000);
    if let Some(mut row) = open {
        row.set("eventSeq", Value::I64(seq.max(count_of(&row, "eventSeq"))));
        row.set("payloadJson", Value::string(payload));
        row.set("expireDate", expire);
        return row.update(facade);
    }
    let mut row = facade.make_value(OUTBOX)?;
    row.set_sequenced_id_primary(facade)?;
    row.set("subscriptionId", Value::string(subscription));
    row.set("userId", Value::string(user));
    row.set("chatRoomId", Value::string(room_id));
    row.set("eventSeq", Value::I64(seq));
    row.set("kindEnumId", Value::string(KIND_CLEAR));
    row.set("statusEnumId", Value::string(PENDING));
    row.set("messageCount", Value::I64(0));
    row.set("payloadJson", Value::string(payload));
    row.set("attemptCount", Value::I64(0));
    row.set("nextAttemptDate", stamp(now));
    row.set("createdDate", stamp(now));
    row.set("expireDate", expire);
    row.create(facade)
}

/// `queue#ReadClear`: the user read rooms up to `readMap`'s eventSeq. Their pushes not yet sent, up to it, are dropped;
/// each device that was sent one (but the one of this request's device key, which read it) gets a clear. In the
/// caller's transaction; the worker wakes after the commit. Out: clearCount.
fn queue_read_clear(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let out = |count: i64| Ok(BTreeMap::from([("clearCount".to_string(), Value::I64(count))]));
    let (Some(user), Some(Value::Map(reads))) = (param(parameters, "userId"), parameters.get("readMap")) else {
        return out(0);
    };
    if !crate::entity::info::is_entity_defined(facade, OUTBOX) {
        return out(0);
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let reader: HashSet<String> = match super::web_push::request_device_key(facade, &user)? {
        Some(key) => facade
            .find(DEVICE)?
            .eq("loginKey", Value::string(key))
            .disable_authz()
            .list()?
            .values()
            .iter()
            .filter_map(|row| text_of(row, "subscriptionId"))
            .collect(),
        None => HashSet::new(),
    };
    let now = now_millis();
    let mut clears = 0;
    for (room_id, seq) in reads {
        let Ok(seq) = to_plain_string(seq).split('.').next().unwrap_or("").parse::<i64>() else {
            continue;
        };
        let read = |find: crate::entity::EntityFind| {
            find.eq("userId", Value::string(&user))
                .eq("chatRoomId", Value::string(room_id))
                .eq("kindEnumId", Value::string(KIND_MESSAGE))
                .condition(Condition::less_equal("eventSeq", Value::I64(seq)))
                .disable_authz()
        };
        // not sent yet: not sent at all
        read(facade.find(OUTBOX)?)
            .condition(Condition::is_in(
                "statusEnumId",
                vec![Value::string(PENDING), Value::string(HELD)],
            ))
            .update_all([
                ("statusEnumId".to_string(), Value::string(DROPPED)),
                ("reasonCode".to_string(), Value::string("read")),
                ("clearedDate".to_string(), stamp(now)),
            ])?;
        // waiting, and newer than the read: what of it was read is no longer counted
        recount_partly_read(facade, &user, room_id, seq)?;
        // sent: those devices remove the room's notification
        let shown = read(facade.find(OUTBOX)?)
            .eq("statusEnumId", Value::string(SENT))
            .condition(Condition::is_null("clearedDate"))
            .list()?;
        let devices: BTreeSet<String> = shown
            .values()
            .iter()
            .filter_map(|row| text_of(row, "subscriptionId"))
            .collect();
        if devices.is_empty() {
            continue;
        }
        read(facade.find(OUTBOX)?)
            .eq("statusEnumId", Value::string(SENT))
            .condition(Condition::is_null("clearedDate"))
            .update_all([("clearedDate".to_string(), stamp(now))])?;
        for device in devices.iter().filter(|device| !reader.contains(*device)) {
            // its notification shows a newer message, not read: it stays
            if newer_unread(facade, device, room_id, seq)? {
                continue;
            }
            queue_clear(facade, &user, device, room_id, seq, now)?;
            clears += 1;
        }
    }
    if clears > 0 {
        wake_after_commit(facade);
    }
    out(clears)
}

/// A waiting push of the room newer than the read at `seq` counted messages the user has now read: its count becomes
/// at most the events after the read (an upper bound: not every event is a message), and its title follows.
fn recount_partly_read(facade: &EntityFacade, user: &str, room_id: &str, seq: i64) -> Result<(), LibError> {
    let rows = facade
        .find(OUTBOX)?
        .eq("userId", Value::string(user))
        .eq("chatRoomId", Value::string(room_id))
        .eq("kindEnumId", Value::string(KIND_MESSAGE))
        .condition(Condition::greater("eventSeq", Value::I64(seq)))
        .condition(Condition::is_in(
            "statusEnumId",
            vec![Value::string(PENDING), Value::string(HELD)],
        ))
        .use_cache(false)
        .for_update(true)
        .disable_authz()
        .list()?;
    for row in rows.values() {
        let count = count_of(row, "messageCount");
        let left = count.min((count_of(row, "eventSeq") - seq).max(1));
        if left >= count {
            continue;
        }
        let mut row = row.clone();
        let mut payload: serde_json::Value = text_of(&row, "payloadJson")
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_else(|| json!({}));
        payload["count"] = left.into();
        payload["title"] = title_of(&payload).into();
        row.set("messageCount", Value::I64(left));
        row.set("payloadJson", Value::string(payload.to_string()));
        row.update(facade)?;
    }
    Ok(())
}

/// Drop the pushes waiting for a device that now belongs to another user (a browser or app signed in as someone else,
/// `register#PushDevice` and `store#WebPushSubscription`): its new user is never sent the earlier user's messages. The
/// send checks the device's user too. Logged when it cannot be.
pub(crate) fn forget_device_pushes(facade: &EntityFacade, subscription: &str) {
    if subscription.is_empty() || !crate::entity::info::is_entity_defined(facade, OUTBOX) {
        return;
    }
    let dropped = force_new(facade, OUTBOX, || {
        facade
            .find(OUTBOX)?
            .eq("subscriptionId", Value::string(subscription))
            .condition(Condition::is_in(
                "statusEnumId",
                vec![Value::string(PENDING), Value::string(HELD)],
            ))
            .disable_authz()
            .update_all([
                ("statusEnumId".to_string(), Value::string(DROPPED)),
                ("reasonCode".to_string(), Value::string("deviceMoved")),
            ])
    });
    if let Err(error) = dropped {
        tracing::warn!(subscription_id = %subscription, "the pushes of a moved device were not dropped: {error}");
    }
}

/// `ack#Push`: the current user's device showed the push `ackId` (a chat push's `ackId`, or the `pushId` a push of
/// send#WebPush carries, its registration's id). The device keeps its acked count and last ack. Out: acked, false for a
/// push that is not the user's (no error, so the answer tells nothing of other users' pushes).
fn ack_push(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Result<BTreeMap<String, Value>, LibError> {
    let out = |acked: bool| Ok(BTreeMap::from([("acked".to_string(), Value::Bool(acked))]));
    let (Some(user), Some(push_id)) = (
        crate::user::current_user_id()
            .ok()
            .flatten()
            .filter(|id| !id.is_empty()),
        param(parameters, "ackId"),
    ) else {
        return out(false);
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let now = now_millis();
    let push = facade
        .find(OUTBOX)?
        .eq("pushOutboxId", Value::string(&push_id))
        .eq("userId", Value::string(&user))
        .disable_authz()
        .one()?;
    let subscription = match push {
        Some(mut push) => {
            if text_of(&push, "ackedDate").is_some() {
                return out(true);
            }
            push.set("ackedDate", stamp(now));
            push.update(facade)?;
            text_of(&push, "subscriptionId")
        }
        None => facade
            .find(DEVICE)?
            .eq("subscriptionId", Value::string(&push_id))
            .eq("userId", Value::string(&user))
            .disable_authz()
            .one()?
            .and_then(|device| text_of(&device, "subscriptionId")),
    };
    let Some(subscription) = subscription else {
        return out(false);
    };
    let device = facade
        .find(DEVICE)?
        .eq("subscriptionId", Value::string(&subscription))
        .use_cache(false)
        .for_update(true)
        .disable_authz()
        .one()?;
    if let Some(mut device) = device {
        device.set("ackedCount", Value::I64(count_of(&device, "ackedCount") + 1));
        device.set("lastAckDate", stamp(now));
        device.update(facade)?;
    }
    out(true)
}

// the tests read the pushes back with the device's key, which needs the web-push feature's crypto
/// The room a test push is of (none: no read marker, no clear).
const TEST_ROOM: &str = "_test";
/// The least time between two test pushes to a device (`send#TestPush`), in milliseconds.
const TEST_EVERY_MS: i64 = 10_000;

/// Queue a test notification to each of `subscriptions` of `user` (plans/Comms-Messaging.md §7.6, the check screen):
/// the same outbox, encryption and send as a message, with the payload `{kind: test, title, sentDate}`. Out: each
/// push's id, which the device acknowledges it with.
pub(crate) fn queue_test(
    facade: &EntityFacade,
    user: &str,
    subscriptions: &[String],
    now: i64,
) -> Result<Vec<String>, LibError> {
    let mut ids = Vec::new();
    for subscription in subscriptions {
        let mut row = facade.make_value(OUTBOX)?;
        row.set_sequenced_id_primary(facade)?;
        let id = text_of(&row, "pushOutboxId").unwrap_or_default();
        let payload = json!({"v": 1, "kind": "test", "title": "Test notification", "sentDate": now,
            "tag": "moqui-test", "topic": "PushTest"});
        row.set("subscriptionId", Value::string(subscription));
        row.set("userId", Value::string(user));
        row.set("chatRoomId", Value::string(TEST_ROOM));
        row.set("eventSeq", Value::I64(0));
        row.set("kindEnumId", Value::string(KIND_MESSAGE));
        row.set("statusEnumId", Value::string(PENDING));
        row.set("messageCount", Value::I64(1));
        row.set("payloadJson", Value::string(payload.to_string()));
        row.set("attemptCount", Value::I64(0));
        row.set("nextAttemptDate", stamp(now));
        row.set("createdDate", stamp(now));
        row.set("expireDate", stamp(now + CLEAR_TTL_SECONDS * 1000));
        row.create(facade)?;
        ids.push(id);
    }
    Ok(ids)
}

/// Whether one of `subscriptions` had a test push queued in the last [`TEST_EVERY_MS`]: one a device in that time (the
/// check screen's button, not a way to flood a phone).
pub(crate) fn test_too_soon(facade: &EntityFacade, subscriptions: &[String], now: i64) -> Result<bool, LibError> {
    if subscriptions.is_empty() {
        return Ok(false);
    }
    Ok(facade
        .find(OUTBOX)?
        .condition(Condition::is_in(
            "subscriptionId",
            subscriptions.iter().map(Value::string).collect(),
        ))
        .eq("chatRoomId", Value::string(TEST_ROOM))
        .condition(Condition::greater("createdDate", stamp(now - TEST_EVERY_MS)))
        .disable_authz()
        .count()?
        > 0)
}

/// `send#TestPush`: a test notification to the calling device, through the full path (the outbox, encrypted, the push
/// service): the registrations of this request's device key (the app), else the current user's browser with
/// `endpoint`. Out: `ackIdList` (empty when the request is no device of the user's: `reasonCode` noDevice, or a device
/// of the user's had a test in the last [`TEST_EVERY_MS`]: `reasonCode` tooSoon), `queuedDate` (Unix milliseconds);
/// the worker is woken after the commit.
fn send_test_push(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let now = now_millis();
    let out = |ids: Vec<String>| {
        let mut out = BTreeMap::from([
            (
                "ackIdList".to_string(),
                Value::List(ids.iter().map(Value::string).collect()),
            ),
            ("queuedDate".to_string(), Value::I64(now)),
        ]);
        if ids.is_empty() {
            out.insert("reasonCode".to_string(), Value::string("noDevice"));
        }
        Ok(out)
    };
    let Some(user) = crate::user::current_user_id()
        .ok()
        .flatten()
        .filter(|id| !id.is_empty())
    else {
        return out(Vec::new());
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut find = facade
        .find(DEVICE)?
        .eq("userId", Value::string(&user))
        .condition(Condition::is_null("disabledDate"))
        .disable_authz();
    if let Some(key) = super::web_push::request_device_key(facade, &user)? {
        find = find.eq("loginKey", Value::string(key));
    } else if let Some(endpoint) = param(parameters, "endpoint") {
        find = find.eq("endpointHash", Value::string(super::web_push::endpoint_hash(&endpoint)));
    } else {
        return out(Vec::new());
    }
    let subscriptions: Vec<String> = find
        .list()?
        .values()
        .iter()
        .filter_map(|row| text_of(row, "subscriptionId"))
        .collect();
    // one test a user in that time, whichever device asks: a test per endpoint let one user queue a post to each of
    // their endpoints every ten seconds (plans/Security-Review-Medium.md SEC-COM-81)
    let all_devices: Vec<String> = facade
        .find(DEVICE)?
        .eq("userId", Value::string(&user))
        .disable_authz()
        .list()?
        .values()
        .iter()
        .filter_map(|row| text_of(row, "subscriptionId"))
        .collect();
    if !subscriptions.is_empty() && test_too_soon(facade, &all_devices, now)? {
        let mut out = BTreeMap::from([
            ("ackIdList".to_string(), Value::List(Vec::new())),
            ("queuedDate".to_string(), Value::I64(now)),
            ("reasonCode".to_string(), Value::string("tooSoon")),
        ]);
        out.insert("retryAfterSeconds".to_string(), Value::I64(TEST_EVERY_MS / 1000));
        return Ok(out);
    }
    let ids = queue_test(facade, &user, &subscriptions, now)?;
    if !ids.is_empty() {
        wake_after_commit(facade);
    }
    out(ids)
}

// ---- reminders (plans/Comms-Messaging.md phase 15) ----

/// What the collapse key of a reminder of message `seq` of `room_id` is made over, in place of the room.
pub fn reminder_collapse_room(room_id: &str, seq: i64) -> String {
    format!("{room_id}\0reminder\0{seq}")
}

/// A reminder's push to one user: the message it is about and the texts the caller made in the user's language.
pub(crate) struct Reminder<'a> {
    pub user: &'a str,
    pub reminder_id: &'a str,
    pub room_id: &'a str,
    pub event_seq: i64,
    /// The notification's title ("Reminder: Ann in Team").
    pub title: &'a str,
    /// Its body when the message's text is not shown (an encrypted room, previews off, a message of no text).
    pub hidden_body: &'a str,
}

/// Queue `reminder` to each device of its user (rule 9): a `PokReminder` row, held while the user is on do not
/// disturb, living a day, its payload a `notification` push that every client shows (`{kind: notification, topic:
/// ChatReminder, title, body, link, tag}`), the body the message's snippet only in a plain room for a user whose
/// previews are on (rule 5). Out: the rows written. Nothing for a room or message that is gone.
pub(crate) fn queue_reminder(facade: &EntityFacade, reminder: &Reminder<'_>, now: i64) -> Result<usize, LibError> {
    let room = facade
        .find(ROOM)?
        .eq("chatRoomId", Value::string(reminder.room_id))
        .disable_authz()
        .one()?;
    let event = facade
        .find(EVENT)?
        .eq("chatRoomId", Value::string(reminder.room_id))
        .eq("eventSeq", Value::I64(reminder.event_seq))
        .disable_authz()
        .one()?;
    let (Some(room), Some(event)) = (room, event.filter(|event| text_of(event, "redactedDate").is_none())) else {
        return Ok(0);
    };
    let users = [reminder.user.to_string()];
    let devices = facade
        .find(DEVICE)?
        .eq("userId", Value::string(reminder.user))
        .condition(Condition::is_null("disabledDate"))
        .disable_authz()
        .list()?
        .values()
        .to_vec();
    if devices.is_empty() {
        return Ok(0);
    }
    let content = Content::of(facade, &room, &event)?;
    let preview = !preview_off(facade, &users)?.contains(reminder.user);
    let held = on_dnd(facade, &users, now)?.contains(reminder.user);
    let body = content
        .snippet
        .clone()
        .filter(|_| preview && !content.encrypted)
        .unwrap_or_else(|| reminder.hidden_body.to_string());
    let payload = json!({
        "v": 1,
        "kind": "notification",
        "topic": "ChatReminder",
        "title": cut_chars(reminder.title, MAX_NAME * 2),
        "body": body,
        "link": format!("{}&eventSeq={}", content.link, content.event_seq),
        "tag": format!("chat-reminder-{}", reminder.reminder_id),
        "chatRoomId": content.room_id,
        "eventSeq": content.event_seq,
        "encrypted": content.encrypted,
        "sentDate": now,
    });
    let factory = current_factory();
    let transport = factory.as_deref().map(transport);
    let mut queued = 0;
    for device in &devices {
        if transport
            .as_ref()
            .is_some_and(|transport| !transport.available(Provider::of(device)))
        {
            continue;
        }
        let subscription = text_of(device, "subscriptionId").unwrap_or_default();
        let mut row = facade.make_value(OUTBOX)?;
        row.set_sequenced_id_primary(facade)?;
        row.set("subscriptionId", Value::string(&subscription));
        row.set("userId", Value::string(reminder.user));
        row.set("chatRoomId", Value::string(reminder.room_id));
        row.set("eventSeq", Value::I64(reminder.event_seq));
        row.set("kindEnumId", Value::string(KIND_REMINDER));
        row.set("statusEnumId", Value::string(if held { HELD } else { PENDING }));
        row.set("messageCount", Value::I64(1));
        row.set("mention", Value::string("N"));
        row.set("payloadJson", Value::string(payload.to_string()));
        row.set("attemptCount", Value::I64(0));
        row.set("nextAttemptDate", stamp(now));
        row.set("createdDate", stamp(now));
        row.set("expireDate", stamp(now + REMINDER_TTL_SECONDS * 1000));
        row.create(facade)?;
        queued += 1;
    }
    Ok(queued)
}

/// `queue#ReminderPush`: a reminder's push to each device of `userId`, in the caller's transaction, the worker woken
/// after the commit. In: `userId`, `chatMessageReminderId`, `chatRoomId`, `eventSeq`, `title`, `hiddenBody`, and
/// `nowMillis` (the tests' clock). Out: `queuedCount`.
fn queue_reminder_push(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let (Some(user), Some(reminder_id), Some(room_id)) = (
        param(parameters, "userId"),
        param(parameters, "chatMessageReminderId"),
        param(parameters, "chatRoomId"),
    ) else {
        return Err(LibError::new(
            "queue#ReminderPush needs userId, chatMessageReminderId and chatRoomId",
        ));
    };
    let event_seq = param(parameters, "eventSeq")
        .and_then(|text| {
            text.split('.')
                .next()
                .and_then(|whole| whole.trim().parse::<i64>().ok())
        })
        .ok_or_else(|| LibError::new("queue#ReminderPush needs eventSeq"))?;
    let now = param(parameters, "nowMillis")
        .and_then(|text| text.parse::<i64>().ok())
        .unwrap_or_else(now_millis);
    let title = param(parameters, "title").unwrap_or_else(|| "Reminder".to_string());
    let hidden_body = param(parameters, "hiddenBody").unwrap_or_default();
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let reminder = Reminder {
        user: &user,
        reminder_id: &reminder_id,
        room_id: &room_id,
        event_seq,
        title: &title,
        hidden_body: &hidden_body,
    };
    let queued = queue_reminder(facade, &reminder, now)?;
    if queued > 0 {
        wake_after_commit(facade);
    }
    Ok(BTreeMap::from([(
        "queuedCount".to_string(),
        Value::I64(i64::try_from(queued).unwrap_or(i64::MAX)),
    )]))
}

#[cfg(all(test, feature = "web-push"))]
#[path = "chat_push_tests.rs"]
mod tests;
