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

//! The outbox sent by the host (plans/Mobile-Desktop.md §6, MD4; `outbox.rs`): a plain thread, as `renewal.rs` and
//! `mirror.rs`, that takes the due operations of every account signed in here (`Store::outbox_claim`, which keeps the
//! order per target) and sends them through the proxy with the account's key, so they go while no window shows them,
//! and for an account that is not the current one.
//!
//! WebrootChat.js's rules: no answer, 408, 429 or 5xx is tried again after 2, 5, 15, then 60 seconds; 401 or 403 is held
//! until the user signs in again or the key is renewed (`Store::outbox_release_held`); another 4xx fails with the
//! server's message, for the user to retry or discard. A chat send answered is acknowledged and kept for its pushed
//! echo (`push::echo_ref`), which removes it; a mark-read, a mail send and a mail state change answered are removed. A
//! mail send is three steps, each taken once by its `clientRef` (stage 2), so a retry runs them all again: the draft
//! saved, each of its files uploaded, and sent (an answer with `duplicate` is a send that went before). An operation
//! held three times in a row fails; a chat message with files fails after 20 hours, as the page's would. Each claimed
//! operation is read again before it is sent, so one discarded or taken off meanwhile is not.
//!
//! The thread wakes when an operation is added or retried, when a page's `/notws` opens (the network is back), and
//! after a sign-in or a key renewal; else at the next operation's time. The store's lock is held only to claim and to
//! change an operation, never across a request, and one operation that fails (or panics) never stops the others.

use std::collections::HashSet;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use super::outbox::{self, backoff_ms, classify, payload_of, Next, CHAT_READ, CHAT_SEND, MAIL_SEND, MAIL_STATE};
use crate::account::Account;
use crate::app::{Client, ClientHost};
use crate::offline::{chat_base, compose_base, mail_base};
use crate::proxy::Request;
use crate::store::{OutboxOp, OutboxState, OutboxUpdate, Store, FILE_GRACE};
use crate::SHELL_ORIGIN;

/// Operations taken in one claim.
const CLAIM: u32 = 10;
/// Claims in one pass of an account, so a pass ends.
const ROUNDS: usize = 20;
/// The longest sleep with nothing due (a held operation waits for a wake).
const IDLE: Duration = Duration::from_secs(10 * 60);

/// Holds in a row (401, 403, each after a release) before an operation fails.
const HOLDS_MAX: u32 = 3;
/// The error of a held operation, with its holds in a row: `Waiting for a sign-in (2)`.
const HELD: &str = "Waiting for a sign-in";
/// How long a chat message with files waits before it fails: the server removes unsent files after 24 hours
/// (WebrootChat.js `OUTBOX_FILES_FOR`).
const FILES_FOR_MS: i64 = 20 * 3600 * 1000;

/// The holds in a row of an operation, from its error.
fn holds_of(op: &OutboxOp) -> u32 {
    op.error
        .as_deref()
        .and_then(|error| error.strip_prefix(HELD))
        .and_then(|rest| rest.trim().strip_prefix('(')?.strip_suffix(')')?.parse().ok())
        .unwrap_or(0)
}

/// What a request of the flusher got: the status and body of an answer, or none (status 0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Sent {
    Answer(u16, Vec<u8>),
    Offline,
}

impl Sent {
    fn status(&self) -> u16 {
        match self {
            Self::Answer(status, _) => *status,
            Self::Offline => 0,
        }
    }

    fn json(&self) -> Value {
        match self {
            Self::Answer(_, body) => serde_json::from_slice(body).unwrap_or(Value::Null),
            Self::Offline => Value::Null,
        }
    }
}

/// The account's server, as the flusher asks it: a `POST` of a body to a path, with the account's key.
pub(crate) trait Transport {
    fn post(&self, path: &str, content_type: &str, body: Vec<u8>) -> Sent;
}

/// The account's store, run for a moment and let go; false when it is not open (the account was removed).
pub(crate) trait StoreAccess {
    fn with_store(&self, work: &mut dyn FnMut(&Store)) -> bool;
}

/// `work` on the store of `access`; none when it is not open.
fn with<T>(access: &dyn StoreAccess, work: impl FnOnce(&Store) -> T) -> Option<T> {
    let mut work = Some(work);
    let mut out = None;
    access.with_store(&mut |store| {
        if let Some(work) = work.take() {
            out = Some(work(store));
        }
    });
    out
}

/// What a pass of one account sends, and where its answers go.
pub(crate) struct Job<'a> {
    pub(crate) transport: &'a dyn Transport,
    pub(crate) store: &'a dyn StoreAccess,
    pub(crate) comms_root: String,
    pub(crate) now: &'a dyn Fn() -> i64,
    /// Told when operations are taken to send, before their requests, so the pages show them sending.
    pub(crate) claimed: &'a dyn Fn(),
}

/// What a pass did: whether an operation changed, the pages' `sent` events, whether one is held, and the time the next
/// one waiting is due.
#[derive(Debug, Default)]
pub(crate) struct Flushed {
    pub(crate) changed: bool,
    pub(crate) sent: Vec<Value>,
    pub(crate) held: bool,
    pub(crate) next_ms: Option<i64>,
}

/// How an operation's send ended.
#[derive(Debug, Clone, PartialEq)]
enum Outcome {
    /// A chat send answered: kept for its echo, its answer for the pages.
    Acked(Value),
    /// Answered: removed, with an answer for the pages when there is one.
    Done(Option<Value>),
    /// Not sent: what next, by the status, with the server's message.
    NotSent(u16, Option<String>),
    /// No longer in the outbox (discarded, its echo came, its account's data deleted): nothing to change.
    Gone,
}

/// Send what is due of one account, claim by claim, until nothing is due.
pub(crate) fn flush(job: &Job<'_>) -> Flushed {
    let mut flushed = Flushed::default();
    for _ in 0..ROUNDS {
        let claimed = with(job.store, |store| store.outbox_claim((job.now)(), CLAIM)).unwrap_or_default();
        if claimed.is_empty() {
            break;
        }
        flushed.changed = true;
        (job.claimed)();
        // a claim may take several of one target; once one of them does not go, the later ones wait behind it
        let mut stopped: HashSet<(String, String)> = HashSet::new();
        for op in claimed {
            let order = (op.kind.clone(), op.target.clone());
            if !op.target.is_empty() && stopped.contains(&order) {
                let back = OutboxUpdate {
                    state: OutboxState::Queued,
                    attempts: op.attempts.saturating_sub(1),
                    next_ms: op.next_ms,
                    error: op.error.clone(),
                };
                if let Some(Err(err)) = with(job.store, |store| store.outbox_update(&op.id, &back)) {
                    tracing::error!("The outbox operation {} was not put back: {err}", op.id);
                }
                continue;
            }
            // discarded or taken off since the claim, or the account's data deleted: not sent
            if !still_sending(job, &op) {
                continue;
            }
            let outcome =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| send_op(job, &op))).unwrap_or_else(|_| {
                    tracing::error!("Sending the operation {} panicked", op.id);
                    Outcome::NotSent(0, Some("The send failed".to_string()))
                });
            // a refused one does not hold up its target (the store's order rule); one to retry or held does
            if let Outcome::NotSent(status, _) = &outcome {
                if classify(*status) != Next::Fail {
                    stopped.insert(order);
                }
            }
            settle(job, &op, outcome, &mut flushed);
        }
    }
    // files of drafts and sends nothing refers to any more (a page saved one and never used it)
    with(job.store, |store| store.gc_files(FILE_GRACE));
    // one due now but not sent waits behind another of its target, whose time is later
    let now = (job.now)();
    flushed.next_ms = with(job.store, |store| {
        store
            .outbox_list()
            .iter()
            .filter(|op| matches!(op.state, OutboxState::Queued | OutboxState::Retry) && op.next_ms > now)
            .map(|op| op.next_ms)
            .min()
    })
    .flatten();
    flushed
}

/// Move an operation on after its send.
fn settle(job: &Job<'_>, op: &OutboxOp, outcome: Outcome, flushed: &mut Flushed) {
    let now = (job.now)();
    let update = |state: OutboxState, next_ms: i64, error: Option<String>| OutboxUpdate {
        state,
        attempts: op.attempts,
        next_ms,
        error,
    };
    let written = match outcome {
        Outcome::Acked(answer) => {
            flushed.sent.push(sent_event(op, answer));
            with(job.store, |store| {
                store
                    .outbox_update(&op.id, &update(OutboxState::Acked, now, None))
                    .map(|_| ())
            })
        }
        Outcome::Done(answer) => {
            if let Some(answer) = answer {
                flushed.sent.push(sent_event(op, answer));
            }
            with(job.store, |store| store.outbox_remove(&op.id).map(|_| ()))
        }
        Outcome::Gone => return,
        Outcome::NotSent(status, message) => {
            let state = match classify(status) {
                Next::Retry => update(OutboxState::Retry, now + backoff_ms(op.attempts), message),
                // held again after a sign-in or a renewed key: the server refuses it, not the key
                Next::Hold if holds_of(op) + 1 >= HOLDS_MAX => update(
                    OutboxState::Failed,
                    now,
                    Some(message.unwrap_or_else(|| format!("The server refused it ({status})"))),
                ),
                Next::Hold => {
                    flushed.held = true;
                    update(OutboxState::Held, now, Some(format!("{HELD} ({})", holds_of(op) + 1)))
                }
                Next::Fail => update(
                    OutboxState::Failed,
                    now,
                    Some(message.unwrap_or_else(|| format!("The server answered {status}"))),
                ),
            };
            tracing::info!(
                "Outbox {} ({}): not sent ({status}), {}",
                op.id,
                op.kind,
                state.state.as_str()
            );
            with(job.store, |store| store.outbox_update(&op.id, &state).map(|_| ()))
        }
    };
    match written {
        Some(Ok(())) => {}
        Some(Err(err)) => tracing::error!("The outbox operation {} was not moved on: {err}", op.id),
        None => tracing::warn!("The outbox operation {} was sent, and its store is gone", op.id),
    }
}

/// The pages' `sent` event of an operation answered: the chat send's room and thread (as WebrootChat.js `sent`), or the
/// mail send's answer.
fn sent_event(op: &OutboxOp, answer: Value) -> Value {
    let payload = payload_of(op);
    json!({
        "kind": "sent",
        "op": op.kind,
        "clientRef": op.id,
        "chatRoomId": payload["chatRoomId"],
        "threadRootEventSeq": payload["threadRootEventSeq"],
        "json": answer,
    })
}

/// Send one operation.
fn send_op(job: &Job<'_>, op: &OutboxOp) -> Outcome {
    let payload = payload_of(op);
    match op.kind.as_str() {
        CHAT_SEND => {
            let url = payload["url"].as_str().unwrap_or("");
            if payload["hasFiles"].as_bool() == Some(true) && (job.now)() - op.created_ms > FILES_FOR_MS {
                return Outcome::NotSent(410, Some("Not sent: its files are no longer kept".to_string()));
            }
            // checked when added; checked again, as the comms app may have moved
            if !outbox::comms_path(url, "/Chat/send") {
                return Outcome::NotSent(400, Some(format!("Not the comms app's send: {url}")));
            }
            let mut params: Vec<(String, String)> = payload["params"]
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(name, _)| !matches!(name.as_str(), "chatRoomId" | "clientRef"))
                .filter_map(|(name, value)| text_of(value).map(|text| (name.clone(), text)))
                .collect();
            params.push((
                "chatRoomId".to_string(),
                payload["chatRoomId"].as_str().unwrap_or("").to_string(),
            ));
            params.push(("clientRef".to_string(), op.id.clone()));
            let sent = post_form(job, url, &params);
            let json = sent.json();
            if is_ok(&sent) {
                // as WebrootChat.js: an answer with no event is not a send
                if !json["event"].is_null() || !json["eventSeq"].is_null() {
                    return Outcome::Acked(json);
                }
                return Outcome::NotSent(200, errors_of(&json));
            }
            not_sent(&sent)
        }
        CHAT_READ => {
            let path = format!("{}/markRead", chat_base(&job.comms_root));
            let params = [
                (
                    "chatRoomId".to_string(),
                    payload["chatRoomId"].as_str().unwrap_or("").to_string(),
                ),
                ("eventSeq".to_string(), payload["lastReadEventSeq"].to_string()),
            ];
            let sent = post_form(job, &path, &params);
            if is_ok(&sent) {
                Outcome::Done(None)
            } else {
                not_sent(&sent)
            }
        }
        MAIL_STATE => {
            let path = format!("{}/state", mail_base(&job.comms_root));
            let mut params = vec![
                ("targetsJson".to_string(), payload["targets"].to_string()),
                (
                    "action".to_string(),
                    payload["action"].as_str().unwrap_or("").to_string(),
                ),
            ];
            if let Some(folder) = payload["emailFolderId"].as_str() {
                params.push(("emailFolderId".to_string(), folder.to_string()));
            }
            let sent = post_form(job, &path, &params);
            if !is_ok(&sent) {
                return not_sent(&sent);
            }
            let json = sent.json();
            if json["failedList"].as_array().is_some_and(|failed| !failed.is_empty()) {
                tracing::warn!(
                    "Outbox {}: some threads were not changed: {}",
                    op.id,
                    json["failedList"]
                );
            }
            Outcome::Done(None)
        }
        MAIL_SEND => send_mail(job, op, &payload),
        other => Outcome::NotSent(400, Some(format!("No operation {other}"))),
    }
}

/// A mail send: the draft saved by its `clientRef`, each queued file uploaded to it, and sent, each step answering the
/// same when repeated with the `clientRef`, so a retry starts over. An internal message is refused (its send is not
/// taken once yet, MD5). A draft of the server's whose own `clientRef` is another (one saved before the page knew it)
/// is sent by its `emailMessageId` without the ref: a retry after a lost answer then fails as no longer a draft
/// instead of sending twice. Between the steps the operation must still be there (not discarded, its data not gone).
fn send_mail(job: &Job<'_>, op: &OutboxOp, payload: &Value) -> Outcome {
    let fields: Vec<(String, String)> = payload["fields"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(name, _)| name.as_str() != "clientRef")
        .filter_map(|(name, value)| text_of(value).map(|text| (name.clone(), text)))
        .collect();
    let field = |name: &str| {
        fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    if field("fromValue").as_deref() == Some("internal") {
        return Outcome::NotSent(400, Some("An internal message needs a connection to send".to_string()));
    }
    match send_email(job, op, payload, &fields, true) {
        Outcome::NotSent(status, Some(message))
            if (400..500).contains(&status)
                && field("emailMessageId").is_some()
                && (message.contains("another clientRef") || message.contains("of another message")) =>
        {
            tracing::warn!("Outbox {}: the draft has another clientRef; sent by its id", op.id);
            send_email(job, op, payload, &fields, false)
        }
        outcome => outcome,
    }
}

/// The steps of an email's send, with the operation's `clientRef` or (`by_ref` false) by the draft's id only.
fn send_email(job: &Job<'_>, op: &OutboxOp, payload: &Value, fields: &[(String, String)], by_ref: bool) -> Outcome {
    const DRAFT: [&str; 11] = [
        "emailMessageId",
        "fromValue",
        "toJson",
        "ccJson",
        "bccJson",
        "subject",
        "body",
        "bodyText",
        "quoteHtml",
        "parentEmailMessageId",
        "isForward",
    ];
    let base = compose_base(&job.comms_root);
    let reference = |list: &mut Vec<(String, String)>| {
        if by_ref {
            list.push(("clientRef".to_string(), op.id.clone()));
        }
    };
    let mut draft: Vec<(String, String)> = fields
        .iter()
        .filter(|(name, _)| DRAFT.contains(&name.as_str()))
        .cloned()
        .collect();
    reference(&mut draft);
    let saved = post_form(job, &format!("{base}/saveDraft"), &draft);
    if !is_ok(&saved) {
        return not_sent(&saved);
    }
    let json = saved.json();
    let Some(message) = json["emailMessageId"]
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
    else {
        return Outcome::NotSent(
            200,
            errors_of(&json).or_else(|| Some("The draft was not saved".to_string())),
        );
    };
    for entry in payload["files"].as_array().into_iter().flatten() {
        if !still_sending(job, op) {
            return Outcome::Gone;
        }
        let file = entry["file"].as_str().unwrap_or("");
        let Some(bytes) = with(job.store, |store| store.read_file(file)).flatten() else {
            return Outcome::NotSent(400, Some(format!("The file {} is no longer kept", entry["name"])));
        };
        let name = entry["name"].as_str().unwrap_or(file);
        let content_type = entry["contentType"].as_str().unwrap_or("application/octet-stream");
        // on the draft found by the clientRef (a file it has is not added again), else by its id
        let target = if by_ref {
            [("clientRef", op.id.as_str())]
        } else {
            [("emailMessageId", message.as_str())]
        };
        let (form_type, body) = multipart(&target, name, content_type, &bytes);
        let uploaded = job.transport.post(&format!("{base}/upload"), &form_type, body);
        if !is_ok(&uploaded) {
            return not_sent(&uploaded);
        }
    }
    if !still_sending(job, op) {
        return Outcome::Gone;
    }
    let mut send: Vec<(String, String)> = fields
        .iter()
        .filter(|(name, _)| name != "emailMessageId")
        .cloned()
        .collect();
    send.push(("emailMessageId".to_string(), message));
    reference(&mut send);
    let sent = post_form(job, &format!("{base}/send"), &send);
    if !is_ok(&sent) {
        return not_sent(&sent);
    }
    let json = sent.json();
    let went =
        json["duplicate"].as_bool() == Some(true) || !json["emailMessageId"].is_null() || !json["mode"].is_null();
    if went {
        Outcome::Done(Some(json))
    } else {
        Outcome::NotSent(
            200,
            errors_of(&json).or_else(|| Some("The message was not sent".to_string())),
        )
    }
}

/// Whether an operation claimed is still to send: there, and still `sending` (not discarded, its account's data not
/// deleted, not taken off by its echo).
fn still_sending(job: &Job<'_>, op: &OutboxOp) -> bool {
    with(job.store, |store| store.outbox_get(&op.id))
        .flatten()
        .is_some_and(|kept| kept.state == OutboxState::Sending)
}

/// A plain value as a form parameter's text.
fn text_of(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn is_ok(sent: &Sent) -> bool {
    (200..300).contains(&sent.status())
}

/// An answer that is not a success, with the server's message.
fn not_sent(sent: &Sent) -> Outcome {
    Outcome::NotSent(sent.status(), errors_of(&sent.json()))
}

/// The server's message in an answer's JSON (`errors`, text or a list), if any.
fn errors_of(json: &Value) -> Option<String> {
    match &json["errors"] {
        Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
        Value::Array(list) if !list.is_empty() => Some(
            list.iter()
                .map(|item| item.as_str().map_or_else(|| item.to_string(), str::to_string))
                .collect::<Vec<_>>()
                .join(" "),
        ),
        _ => None,
    }
}

/// `POST` a form to `path`.
fn post_form(job: &Job<'_>, path: &str, params: &[(String, String)]) -> Sent {
    job.transport.post(
        path,
        "application/x-www-form-urlencoded",
        form_body(params).into_bytes(),
    )
}

/// Parameters as an `application/x-www-form-urlencoded` body.
pub(crate) fn form_body(params: &[(String, String)]) -> String {
    let mut url = url::Url::parse("http://form.invalid/").unwrap_or_else(|_| unreachable!("a fixed url parses"));
    url.query_pairs_mut()
        .extend_pairs(params.iter().map(|(name, value)| (name.as_str(), value.as_str())));
    url.query().unwrap_or("").to_string()
}

/// A `multipart/form-data` body of `fields` and one file as `contentFile`: its content type (with the boundary) and
/// bytes.
pub(crate) fn multipart(fields: &[(&str, &str)], name: &str, content_type: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = format!(
        "----moqui{}",
        crate::store::new_client_ref().unwrap_or_else(|_| "outbox0boundary".to_string())
    );
    // a quote or line break in the name would end its header
    let file_name: String = name
        .chars()
        .map(|c| {
            if c == '"' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let content_type: String = content_type.chars().filter(|c| !c.is_control()).collect();
    let mut body = Vec::with_capacity(bytes.len() + 512);
    for (field, value) in fields {
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"\r\n\r\n{value}\r\n").as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"contentFile\"; filename=\"{file_name}\"\r\nContent-Type: {content_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// The flusher's wake: set by the outbox's changes, the network coming back and a sign-in, taken by the thread.
#[derive(Default)]
pub struct Flusher {
    woken: Mutex<bool>,
    signal: Condvar,
}

impl Flusher {
    /// Have the thread look at the outboxes now.
    pub fn wake(&self) {
        *moqui_lib::sync::lock(&self.woken) = true;
        self.signal.notify_all();
    }

    /// Wait for a wake, or `timeout`.
    fn wait(&self, timeout: Duration) {
        let guard = moqui_lib::sync::lock(&self.woken);
        let (mut woken, _) = self
            .signal
            .wait_timeout_while(guard, timeout, |woken| !*woken)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *woken = false;
    }
}

/// Start the flusher's thread.
pub fn start(client: &Arc<Client>, host: &Arc<dyn ClientHost>) {
    let client = Arc::clone(client);
    let host = Arc::clone(host);
    let started = std::thread::Builder::new()
        .name("client-outbox".to_string())
        .spawn(move || loop {
            if client.is_stopped() {
                return;
            }
            let next = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| flush_all(&client, host.as_ref())))
                .unwrap_or_else(|_| {
                    tracing::error!("The outbox pass panicked");
                    Some(now_ms() + 60_000)
                });
            let wait = next.map_or(IDLE, |next| {
                Duration::from_millis(u64::try_from(next - now_ms()).unwrap_or(0).max(200)).min(IDLE)
            });
            client.flusher.wait(wait);
        });
    if let Err(err) = started {
        tracing::error!("The outbox sender did not start: {err}");
    }
}

/// Make the held operations of an account due again (a sign-in, a renewed key) and wake the thread.
pub fn release_held(client: &Client, id: &str) {
    match client.with_store(id, Store::outbox_release_held) {
        Some(Ok(count)) if count > 0 => tracing::info!("{count} held operations of {id} are sent again"),
        Some(Err(err)) => tracing::warn!("The held operations of {id} were not released: {err}"),
        _ => {}
    }
    client.flusher.wake();
}

/// One pass over every account signed in here; gives when the next operation is due.
fn flush_all(client: &Client, host: &dyn ClientHost) -> Option<i64> {
    let accounts = moqui_lib::sync::lock(&client.accounts).list().to_vec();
    let mut next: Option<i64> = None;
    for account in accounts {
        let Some(key) = client.keys.get(&account.id) else {
            continue;
        };
        let transport = ProxyTransport {
            host,
            client,
            account: &account,
            key: &key,
        };
        let store = AccountStore {
            client,
            id: &account.id,
        };
        let job = Job {
            transport: &transport,
            store: &store,
            comms_root: client.comms_root_of(&account.id),
            now: &now_ms,
            claimed: &|| host.notify_pages(&account.id, &json!({ "kind": "changed" })),
        };
        let flushed = flush(&job);
        if flushed.changed {
            host.notify_pages(&account.id, &json!({ "kind": "changed" }));
        }
        for event in &flushed.sent {
            host.notify_pages(&account.id, event);
        }
        if let Some(due) = flushed.next_ms {
            next = Some(next.map_or(due, |kept| kept.min(due)));
        }
    }
    next
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
}

/// An account's store on the client.
struct AccountStore<'a> {
    client: &'a Client,
    id: &'a str,
}

impl StoreAccess for AccountStore<'_> {
    fn with_store(&self, work: &mut dyn FnMut(&Store)) -> bool {
        self.client.with_store(self.id, |store| work(store)).is_some()
    }
}

/// The account's server through the proxy, with its key, as the pages' requests go.
struct ProxyTransport<'a> {
    host: &'a dyn ClientHost,
    client: &'a Client,
    account: &'a Account,
    key: &'a str,
}

impl Transport for ProxyTransport<'_> {
    fn post(&self, path: &str, content_type: &str, body: Vec<u8>) -> Sent {
        let request = Request {
            method: "POST".to_string(),
            path: path.to_string(),
            headers: vec![
                ("Accept".to_string(), "application/json".to_string()),
                ("Content-Type".to_string(), content_type.to_string()),
                ("X-Requested-With".to_string(), "XMLHttpRequest".to_string()),
            ],
            body,
        };
        let upstream = match self
            .client
            .proxy
            .send(&self.account.server, Some(self.key), SHELL_ORIGIN, request)
        {
            Ok(upstream) => upstream,
            Err(err) => {
                tracing::info!("Outbox: {} {path}: {err}", self.account.server);
                return Sent::Offline;
            }
        };
        let status = upstream.status;
        if status == 401 {
            self.host.check_key(self.account);
        }
        match upstream.body() {
            Ok(body) => Sent::Answer(status, body),
            Err(err) => {
                tracing::info!("Outbox: {path} was cut off: {err}");
                Sent::Offline
            }
        }
    }
}

#[cfg(test)]
#[path = "flush_tests.rs"]
mod tests;
