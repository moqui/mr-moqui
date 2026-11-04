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
//! With a server at `clientApi` 4 (MD5; `Job::server`): every operation goes with its `clientRef` (a mark-read and a
//! state change too), and an answer with `duplicate` is one done before; a message's edit, redaction, reaction and
//! poll vote go to Chat's `edit`, `redact`, `react` and `votePoll`, an internal message to Compose's `send`. A mail
//! state change sends each thread's `seen`; a thread the server changed since comes back in `failedList` as a
//! conflict: the operation keeps those threads, in the `conflict` state with the server's `current`. A mail send
//! saves its draft with `baseStamp` and `saveRef`; a draft changed on the server is a conflict: nothing is sent, the
//! user's text is kept as a copy (a new draft, `ironstone.compose.email.<ref>`, with its files) and the server's
//! draft stays. A file of more than [`CHUNK_MIN`] bytes goes in chunks (`uploadChunk`), how far it got kept in the
//! store, and after a restart or a retry it goes on from what `uploadStatus` says the server holds; then `upload`
//! takes it by its `uploadId`. A refusal (another 4xx) keeps its status: a rejection, for the user to retry or
//! discard, which never holds up another operation. Below `clientApi` 4 the MD4 behavior stays: no `seen`,
//! `baseStamp` or chunks.
//!
//! The thread wakes when an operation is added or retried, when a page's `/notws` opens (the network is back), and
//! after a sign-in or a key renewal; else at the next operation's time. The store's lock is held only to claim and to
//! change an operation, never across a request, and one operation that fails (or panics) never stops the others.

use std::collections::HashSet;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use super::outbox::{
    self, backoff_ms, classify, payload_of, Next, CHAT_EDIT, CHAT_REACT, CHAT_READ, CHAT_REDACT, CHAT_SEND,
    INTERNAL_SEND, MAIL_SEND, MAIL_STATE, POLL_VOTE,
};
use crate::account::Account;
use crate::app::{Client, ClientHost};
use crate::login::ServerSync;
use crate::offline::{chat_base, compose_base, mail_base, number};
use crate::proxy::Request;
use crate::store::{order_group, OutboxOp, OutboxState, OutboxUpdate, Store, Upload, FILE_GRACE};
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
/// A file larger than this goes in chunks, with a server at `clientApi` 4: the server's least chunk but the last
/// (`client_upload_min_chunk_bytes`).
pub(crate) const CHUNK_MIN: usize = 262_144;
/// The bytes of a chunk.
const CHUNK_BYTES: usize = 1024 * 1024;
/// The key prefix of Compose's drafts kept here (Compose.qvue `emailLocalKey`): a conflict's copy is kept as one.
const COMPOSE_DRAFT: &str = "ironstone.compose.email.";

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

    /// A `GET` of a path with its query (an upload's status); none by default.
    fn get(&self, _path: &str) -> Sent {
        Sent::Offline
    }
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
    /// What the server supports of the client's sync (MD5 at `clientApi` 4).
    pub(crate) server: ServerSync,
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
    /// The server's copy changed since (MD5): nothing done, its state kept for the user; a mail state change's
    /// payload narrowed to the threads that conflict.
    Conflict { current: Value, payload: Option<Value> },
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
            let order = (order_group(&op.kind).to_string(), op.target.clone());
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
        Outcome::Conflict { mut current, payload } => {
            tracing::info!("Outbox {} ({}): a conflict, kept for the user", op.id, op.kind);
            with(job.store, |store| {
                store.atomic(|store| {
                    if let Some(payload) = &payload {
                        // the threads that conflict: their rows as the server has them, and read again
                        let keys: Vec<String> = payload["targets"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(outbox::target_key)
                            .collect();
                        if let Err(err) = outbox::undo_mail_state(store, payload, Some(&keys)) {
                            tracing::warn!("Outbox {}: the threads' rows were not put back: {err}", op.id);
                        }
                        let mut kept = payload.clone();
                        if let Some(map) = kept.as_object_mut() {
                            map.remove("undo");
                        }
                        store.outbox_set_payload(&op.id, &kept.to_string())?;
                    }
                    if op.kind == MAIL_SEND {
                        match keep_copy(store, op) {
                            Ok((key, reference)) => {
                                current["copyKey"] = json!(key);
                                current["copyRef"] = json!(reference);
                            }
                            Err(err) => tracing::warn!("Outbox {}: the copy of the draft was not kept: {err}", op.id),
                        }
                    }
                    store
                        .outbox_settle(
                            &op.id,
                            &update(OutboxState::Conflict, now, Some("Changed on the server".to_string())),
                            409,
                            Some(&current.to_string()),
                        )
                        .map(|_| ())
                })
            })
        }
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
            // a refusal keeps its status: a rejection the user sees; a refused state change's threads are put back
            // as they were and read again
            let refused = if classify(status) == Next::Fail { status } else { 0 };
            with(job.store, |store| {
                if op.kind == MAIL_STATE && state.state == OutboxState::Failed {
                    let payload = payload_of(op);
                    if let Err(err) = outbox::undo_mail_state(store, &payload, None) {
                        tracing::warn!("Outbox {}: the threads' rows were not put back: {err}", op.id);
                    }
                }
                store.outbox_settle(&op.id, &state, refused, None).map(|_| ())
            })
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
        // a message's change (edit, redaction, reaction, vote): the message it changed
        "eventSeq": payload["eventSeq"],
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
            let mut params = vec![
                (
                    "chatRoomId".to_string(),
                    payload["chatRoomId"].as_str().unwrap_or("").to_string(),
                ),
                ("eventSeq".to_string(), payload["lastReadEventSeq"].to_string()),
            ];
            if job.server.syncs() {
                params.push(("clientRef".to_string(), op.id.clone()));
            }
            let sent = post_form(job, &path, &params);
            if is_ok(&sent) {
                Outcome::Done(None)
            } else {
                not_sent(&sent)
            }
        }
        MAIL_STATE => send_state(job, op, &payload),
        CHAT_EDIT | CHAT_REDACT | CHAT_REACT | POLL_VOTE => send_message_change(job, op, &payload),
        MAIL_SEND => send_mail(job, op, &payload),
        INTERNAL_SEND => {
            if !job.server.internal_send() {
                return Outcome::NotSent(400, Some("An internal message needs a connection to send".to_string()));
            }
            let mut fields = fields_of(&payload);
            fields.push(("clientRef".to_string(), op.id.clone()));
            let sent = post_form(job, &format!("{}/send", compose_base(&job.comms_root)), &fields);
            sent_outcome(&sent)
        }
        other => Outcome::NotSent(400, Some(format!("No operation {other}"))),
    }
}

/// A mail state change: its threads with their `seen` (MD5), the request once by its ref; the threads the server
/// changed since come back as a conflict, the rest are done.
fn send_state(job: &Job<'_>, op: &OutboxOp, payload: &Value) -> Outcome {
    let path = format!("{}/state", mail_base(&job.comms_root));
    let syncs = job.server.syncs();
    let mut targets = payload["targets"].clone();
    if !syncs {
        for target in targets.as_array_mut().into_iter().flatten() {
            if let Some(target) = target.as_object_mut() {
                target.remove("seen");
            }
        }
    }
    let mut params = vec![
        ("targetsJson".to_string(), targets.to_string()),
        (
            "action".to_string(),
            payload["action"].as_str().unwrap_or("").to_string(),
        ),
    ];
    if let Some(folder) = payload["emailFolderId"].as_str() {
        params.push(("emailFolderId".to_string(), folder.to_string()));
    }
    if syncs {
        let reference = payload["requestRef"].as_str().unwrap_or(&op.id).to_string();
        params.push(("clientRef".to_string(), reference));
    }
    let sent = post_form(job, &path, &params);
    if !is_ok(&sent) {
        return not_sent(&sent);
    }
    let json = sent.json();
    let failed = json["failedList"].as_array().cloned().unwrap_or_default();
    let conflicts: Vec<Value> = failed
        .iter()
        .filter(|entry| entry["reason"].as_str() == Some("conflict"))
        .cloned()
        .collect();
    if failed.len() > conflicts.len() {
        tracing::warn!(
            "Outbox {}: some threads were not changed: {}",
            op.id,
            json["failedList"]
        );
    }
    if conflicts.is_empty() {
        return Outcome::Done(None);
    }
    // the operation keeps the threads that conflict, for the user to see, retry or discard
    let key_of = |target: &Value| {
        let id = target["emailMessageId"]
            .as_str()
            .or_else(|| target["communicationEventId"].as_str())
            .or_else(|| target["threadId"].as_str())
            .unwrap_or("");
        format!("{}:{id}", target["store"].as_str().unwrap_or("email"))
    };
    let mut narrowed = payload.clone();
    let kept: Vec<Value> = payload["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|target| {
            let key = key_of(target);
            conflicts
                .iter()
                .any(|entry| entry["key"].as_str() == Some(key.as_str()))
        })
        .cloned()
        .collect();
    narrowed["targets"] = Value::Array(kept);
    Outcome::Conflict {
        current: json!({ "conflicts": conflicts }),
        payload: Some(narrowed),
    }
}

/// A message's edit, redaction, reaction or poll vote, once by its `clientRef`.
fn send_message_change(job: &Job<'_>, op: &OutboxOp, payload: &Value) -> Outcome {
    let (tail, names): (&str, &[&str]) = match op.kind.as_str() {
        CHAT_EDIT => ("edit", &["body", "encryptedJson", "mentionUserIds"]),
        CHAT_REDACT => ("redact", &["reason"]),
        CHAT_REACT => ("react", &["key", "encryptedJson", "reactionEventSeq"]),
        _ => ("votePoll", &["optionIds"]),
    };
    let mut params = vec![
        (
            "chatRoomId".to_string(),
            payload["chatRoomId"].as_str().unwrap_or("").to_string(),
        ),
        ("eventSeq".to_string(), payload["eventSeq"].to_string()),
        ("clientRef".to_string(), op.id.clone()),
    ];
    for name in names {
        if let Some(text) = text_of(&payload[*name]) {
            params.push(((*name).to_string(), text));
        }
    }
    let sent = post_form(job, &format!("{}/{tail}", chat_base(&job.comms_root)), &params);
    if is_ok(&sent) {
        let json = sent.json();
        // a refusal answers no fields of the change, with its errors
        match errors_of(&json) {
            Some(message) if json["duplicate"].as_bool() != Some(true) => Outcome::NotSent(400, Some(message)),
            _ => Outcome::Done(Some(json)),
        }
    } else {
        not_sent(&sent)
    }
}

/// A mail send's fields as form parameters, but its `clientRef`.
fn fields_of(payload: &Value) -> Vec<(String, String)> {
    payload["fields"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(name, _)| name.as_str() != "clientRef")
        .filter_map(|(name, value)| text_of(value).map(|text| (name.clone(), text)))
        .collect()
}

/// How a compose send's answer ends the operation: done when it went (or went before), a conflict, or not sent.
fn sent_outcome(sent: &Sent) -> Outcome {
    if !is_ok(sent) {
        return not_sent(sent);
    }
    let json = sent.json();
    if json["conflict"].as_bool() == Some(true) {
        return Outcome::Conflict {
            current: json!({ "compose": json["compose"] }),
            payload: None,
        };
    }
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

/// A conflict's copy of a mail send: the user's draft kept as a new draft of Compose's (Compose.qvue `saveLocal`'s
/// shape, under a new `clientRef`), with the files it sends; gives its key and its `clientRef`.
fn keep_copy(store: &Store, op: &OutboxOp) -> Result<(String, String), moqui_lib::error::Error> {
    let payload = payload_of(op);
    let fields = &payload["fields"];
    let list = |name: &str| -> Value {
        fields[name]
            .as_str()
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
            .filter(Value::is_array)
            .unwrap_or_else(|| json!([]))
    };
    let reference = crate::store::new_client_ref()?;
    let key = format!("{COMPOSE_DRAFT}{reference}");
    let draft = json!({
        "to": list("toJson"), "cc": list("ccJson"), "bcc": list("bccJson"),
        "subject": fields["subject"], "body": fields["body"], "quoteHtml": fields["quoteHtml"],
        "fromValue": fields["fromValue"], "parentEmailMessageId": fields["parentEmailMessageId"],
        "isForward": fields["isForward"], "attachments": [], "uploads": [],
    });
    let value = json!({
        "at": now_ms(), "email": true, "clientRef": reference, "draft": draft, "queued": payload["files"],
        "conflictOf": { "clientRef": op.id, "emailMessageId": fields["emailMessageId"] },
    });
    store.put_draft(&key, &value.to_string(), &op.files)?;
    Ok((key, reference))
}

/// A mail send: the draft saved by its `clientRef`, each queued file uploaded to it, and sent, each step answering the
/// same when repeated with the `clientRef`, so a retry starts over. An internal message is refused (its send is not
/// taken once yet, MD5). A draft of the server's whose own `clientRef` is another (one saved before the page knew it)
/// is sent by its `emailMessageId` without the ref: a retry after a lost answer then fails as no longer a draft
/// instead of sending twice. Between the steps the operation must still be there (not discarded, its data not gone).
fn send_mail(job: &Job<'_>, op: &OutboxOp, payload: &Value) -> Outcome {
    let mut fields = fields_of(payload);
    if !job.server.syncs() {
        fields.retain(|(name, _)| !matches!(name.as_str(), "baseStamp" | "saveRef"));
    }
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
    const DRAFT: [&str; 13] = [
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
        "baseStamp",
        "saveRef",
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
    if json["conflict"].as_bool() == Some(true) {
        return Outcome::Conflict {
            current: json!({ "compose": json["compose"] }),
            payload: None,
        };
    }
    // the stamp of the draft as this save left it, for the send to compare with
    let stamp = json["draftStamp"].clone();
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
        let uploaded = if job.server.syncs() && bytes.len() > CHUNK_MIN {
            let upload_id = match upload_chunks(job, op, file, name, content_type, &bytes) {
                Ok(upload_id) => upload_id,
                Err(outcome) => return outcome,
            };
            let mut params: Vec<(String, String)> = target
                .iter()
                .map(|(field, value)| ((*field).to_string(), (*value).to_string()))
                .collect();
            params.push(("uploadId".to_string(), upload_id));
            post_form(job, &format!("{base}/upload"), &params)
        } else {
            let (form_type, body) = multipart(&target, name, content_type, &bytes);
            job.transport.post(&format!("{base}/upload"), &form_type, body)
        };
        if !is_ok(&uploaded) {
            return not_sent(&uploaded);
        }
    }
    if !still_sending(job, op) {
        return Outcome::Gone;
    }
    let mut send: Vec<(String, String)> = fields
        .iter()
        .filter(|(name, _)| name != "emailMessageId" && !(name == "baseStamp" && text_of(&stamp).is_some()))
        .cloned()
        .collect();
    send.push(("emailMessageId".to_string(), message));
    if let Some(stamp) = text_of(&stamp).filter(|_| job.server.syncs()) {
        send.push(("baseStamp".to_string(), stamp));
    }
    reference(&mut send);
    let sent = post_form(job, &format!("{base}/send"), &send);
    sent_outcome(&sent)
}

/// A file of a mail send in chunks (`uploadChunk`), from where the server is: a new upload, or one kept here that
/// `uploadStatus` says how far it got (complete, part of it, or not found: from the start). Gives its `uploadId` once
/// the server holds it whole.
fn upload_chunks(
    job: &Job<'_>,
    op: &OutboxOp,
    file: &str,
    name: &str,
    content_type: &str,
    bytes: &[u8],
) -> Result<String, Outcome> {
    let base = compose_base(&job.comms_root);
    let total = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
    let kept = with(job.store, |store| store.upload(&op.id, file)).flatten();
    let (upload_id, mut offset) = match kept.filter(|kept| kept.total == total) {
        Some(kept) => {
            let status = job.transport.get(&format!(
                "{base}/uploadStatus?uploadId={}",
                super::mirror::encode(&kept.upload_id)
            ));
            if !is_ok(&status) {
                return Err(not_sent(&status));
            }
            let json = status.json();
            if json["found"].as_bool() != Some(true) {
                (kept.upload_id, 0)
            } else if json["complete"].as_bool() == Some(true) {
                return Ok(kept.upload_id);
            } else {
                (kept.upload_id, number(&json["received"]).unwrap_or(0).clamp(0, total))
            }
        }
        None => {
            let upload_id = crate::store::new_client_ref().map_err(|err| Outcome::NotSent(0, Some(err.to_string())))?;
            (upload_id, 0)
        }
    };
    let note = |offset: i64| {
        let upload = Upload {
            op_id: op.id.clone(),
            file: file.to_string(),
            upload_id: upload_id.clone(),
            offset,
            total,
        };
        if let Some(Err(err)) = with(job.store, |store| store.put_upload(&upload)) {
            tracing::warn!("Outbox {}: the upload of {file} was not noted: {err}", op.id);
        }
    };
    note(offset);
    let rounds = bytes.len() / CHUNK_BYTES + 8;
    for _ in 0..rounds {
        if !still_sending(job, op) {
            return Err(Outcome::Gone);
        }
        let from = usize::try_from(offset).unwrap_or(0);
        let to = from.saturating_add(CHUNK_BYTES).min(bytes.len());
        let Some(chunk) = bytes.get(from..to) else {
            return Err(Outcome::NotSent(
                0,
                Some(format!("The upload of {name} lost its place")),
            ));
        };
        let (offset_text, total_text) = (offset.to_string(), total.to_string());
        let fields = [
            ("uploadId", upload_id.as_str()),
            ("offset", offset_text.as_str()),
            ("total", total_text.as_str()),
            ("fileName", name),
            ("contentType", content_type),
        ];
        let (form_type, body) = multipart_part(&fields, "chunk", name, content_type, chunk);
        let sent = job.transport.post(&format!("{base}/uploadChunk"), &form_type, body);
        if !is_ok(&sent) {
            return Err(not_sent(&sent));
        }
        let json = sent.json();
        let Some(received) = number(&json["received"]) else {
            return Err(Outcome::NotSent(
                200,
                errors_of(&json).or_else(|| Some(format!("The upload of {name} was not taken"))),
            ));
        };
        offset = received.clamp(0, total);
        note(offset);
        if json["complete"].as_bool() == Some(true) {
            return Ok(upload_id);
        }
    }
    Err(Outcome::NotSent(
        0,
        Some(format!("The upload of {name} did not finish")),
    ))
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
    multipart_part(fields, "contentFile", name, content_type, bytes)
}

/// [`multipart`] with the file as the part `part`.
pub(crate) fn multipart_part(
    fields: &[(&str, &str)],
    part: &str,
    name: &str,
    content_type: &str,
    bytes: &[u8],
) -> (String, Vec<u8>) {
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
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{part}\"; filename=\"{file_name}\"\r\nContent-Type: {content_type}\r\n\r\n"
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
            server: client.server_sync_of(&account.id),
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
pub(crate) struct AccountStore<'a> {
    pub(crate) client: &'a Client,
    pub(crate) id: &'a str,
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

    fn get(&self, path: &str) -> Sent {
        let request = Request {
            method: "GET".to_string(),
            path: path.to_string(),
            headers: vec![("Accept".to_string(), "application/json".to_string())],
            body: Vec::new(),
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
