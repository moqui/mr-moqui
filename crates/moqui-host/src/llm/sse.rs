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

//! `GET /assist/runs/{runId}/events`: a run's events as Server-Sent Events. The stored events after the client's
//! sequence (`Last-Event-ID`, else `?after=`) are replayed from `AgentRunEvent`, then the stream follows the node's
//! hub (`moqui_core::llm::run::hub`): the run's live events when this node works it, else the stored ones the remote
//! tail reads every 400 ms. A stored event has its sequence as its `id:`; a live one has none. `: ping` goes out every
//! 15 seconds; the stream ends when the run ends or pauses. `GET /assist/runs/{runId}/events.json?after=` is the
//! catch-up of a client that polls: the stored events after the sequence, as JSON.
//!
//! Both authenticate as `/notws` does (the session cookie, else a `login_key` header, with the origin check), and only
//! the run's owner sees a run: for anyone else it is not found. The stream is written by a thread of its own into the
//! response body (the `ChannelBody` of `git_http.rs`), so it holds no request worker; at most 256 are open on a node,
//! and one more is answered 503 with `Retry-After`. The stream follows the hub with a `Follower`, which fills a gap
//! in the stored sequence from the database.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use bytes::Bytes;
use hyper::body::Frame;
use moqui_core::context::{ErrorKind, ExecutionContextFactory};
use moqui_core::llm::run::{self, events};
use moqui_core::user::verify_session_cookie;
use serde_json::{json, Value as Json};
use tokio::sync::mpsc;

use crate::http::{plain, HttpState};
use crate::notws::{key_user, origin_allowed, session_cookie, session_user};

type Chunk = std::io::Result<Bytes>;

/// A comment line this often keeps proxies and the browser from closing an idle stream.
const PING_EVERY: Duration = Duration::from_secs(15);
/// How often the stream reads the run's state, so it ends even when it missed the last event.
const CHECK_EVERY: Duration = Duration::from_secs(5);
/// Stored events replayed per read.
const PAGE: i64 = 500;
/// Event streams open at once on this node; one more is answered 503, and the client tries again.
const MAX_STREAMS: usize = 256;

static STREAMS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// One open stream, counted until its thread ends.
struct Open;

impl Open {
    fn take() -> Option<Self> {
        let open = STREAMS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if open >= MAX_STREAMS {
            STREAMS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            return None;
        }
        Some(Open)
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        STREAMS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// The response body: chunks from the stream's thread until it drops its sender.
struct ChannelBody(mpsc::Receiver<Chunk>);

impl hyper::body::Body for ChannelBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        self.0
            .poll_recv(cx)
            .map(|chunk| chunk.map(|chunk| chunk.map(Frame::data)))
    }
}

/// The user of the request, as `/notws` finds it; else the response to send.
async fn user_of(state: &HttpState, headers: &HeaderMap) -> Result<String, Box<Response>> {
    if !origin_allowed(&state.factory, headers, "agent run events") {
        return Err(Box::new(plain(403, "origin not allowed")));
    }
    let factory = Arc::clone(&state.factory);
    let loaded = match session_cookie(headers) {
        Some(cookie) => {
            let session_id = match verify_session_cookie(&cookie) {
                Ok(Some(id)) if !id.is_empty() => id,
                Ok(_) | Err(_) => return Err(Box::new(plain(401, "unauthorized"))),
            };
            tokio::task::spawn_blocking(move || session_user(factory, session_id)).await
        }
        None => {
            let Some(key) = headers
                .get("login_key")
                .and_then(|value| value.to_str().ok())
                .filter(|key| !key.is_empty())
                .map(str::to_string)
            else {
                return Err(Box::new(plain(401, "unauthorized")));
            };
            tokio::task::spawn_blocking(move || key_user(factory, key)).await
        }
    };
    match loaded {
        Ok(Ok(Some(user_id))) => Ok(user_id),
        Ok(Ok(None)) => Err(Box::new(plain(401, "unauthorized"))),
        Ok(Err(err)) if err.kind() == ErrorKind::Unavailable => {
            Err(Box::new(state.guard.unavailable("service unavailable")))
        }
        Ok(Err(err)) => Err(Box::new(crate::http::internal_error(err.message()))),
        Err(_) => Err(Box::new(plain(500, "request worker stopped"))),
    }
}

/// The sequence the client has: `Last-Event-ID`, else `?after=`, else 0.
fn after_of(headers: &HeaderMap, query: &HashMap<String, String>) -> i64 {
    headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .or_else(|| query.get("after").map(String::as_str))
        .and_then(|text| text.trim().parse::<i64>().ok())
        .filter(|seq| *seq >= 0)
        .unwrap_or(0)
}

/// Whether `user_id` owns the run, read in a context of its own.
fn owns(factory: &Arc<ExecutionContextFactory>, run_id: &str, user_id: &str) -> Result<bool, moqui_lib::error::Error> {
    let Some(facade) = factory.entity() else {
        return Err(moqui_lib::error::Error::new("the entity facade is not installed"));
    };
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    Ok(run::run_owner(&facade, run_id)?.as_deref() == Some(user_id))
}

async fn owner_checked(state: &HttpState, headers: &HeaderMap, run_id: &str) -> Result<(), Box<Response>> {
    let user_id = user_of(state, headers).await?;
    let factory = Arc::clone(&state.factory);
    let run_id = run_id.to_string();
    match tokio::task::spawn_blocking(move || owns(&factory, &run_id, &user_id)).await {
        Ok(Ok(true)) => Ok(()),
        Ok(Ok(false)) => Err(Box::new(plain(404, "not found"))),
        Ok(Err(error)) => Err(Box::new(crate::http::internal_error(error.message()))),
        Err(_) => Err(Box::new(plain(500, "the lookup stopped"))),
    }
}

/// `GET /assist/runs/{runId}/events`.
pub(crate) async fn events(
    State(state): State<Arc<HttpState>>,
    Path(run_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = owner_checked(&state, &headers, &run_id).await {
        return *response;
    }
    let after = after_of(&headers, &query);
    let Some(open) = Open::take() else {
        return Response::builder()
            .status(503)
            .header("retry-after", "5")
            .header("content-type", "text/plain; charset=utf-8")
            .body(axum::body::Body::from("too many event streams"))
            .unwrap_or_else(|_| plain(503, "too many event streams"));
    };
    let (tx, rx) = mpsc::channel::<Chunk>(64);
    let factory = Arc::clone(&state.factory);
    let shutdown = state.shutdown.clone();
    let spawned = std::thread::Builder::new()
        .name("moqui-agent-sse".to_string())
        .spawn(move || {
            let _open = open;
            stream(&factory, &run_id, after, &tx, &shutdown);
        });
    if let Err(error) = spawned {
        return crate::http::internal_error(&format!("the event stream did not start: {error}"));
    }
    Response::builder()
        .status(200)
        .header("content-type", "text/event-stream; charset=utf-8")
        .header("cache-control", "no-cache, no-store")
        .header("x-accel-buffering", "no")
        .body(axum::body::Body::new(ChannelBody(rx)))
        .unwrap_or_else(|_| plain(500, "the event stream header was not valid"))
}

/// One SSE frame: a stored event with its `id:`, a live one without.
fn frame(seq: Option<i64>, event_type: &str, payload: &Json) -> String {
    let data = json!({"seq": seq, "type": event_type, "payload": payload});
    match seq {
        Some(seq) => format!("id: {seq}\nevent: {event_type}\ndata: {data}\n\n"),
        None => format!("event: {event_type}\ndata: {data}\n\n"),
    }
}

/// The stream's thread: replay, then follow the hub until the run ends or pauses, the client goes away or the server
/// stops. A read that fails ends the stream; the client reconnects with its last id.
fn stream(
    factory: &Arc<ExecutionContextFactory>,
    run_id: &str,
    after: i64,
    tx: &mpsc::Sender<Chunk>,
    shutdown: &tokio::sync::watch::Receiver<bool>,
) {
    let send = |text: String| tx.blocking_send(Ok(Bytes::from(text))).is_ok();
    let Some(facade) = factory.entity() else {
        return;
    };
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    if !send("retry: 2000\n\n".to_string()) {
        return;
    }
    let mut last = after;
    // the stored events after the client's, page by page; false when the stream is over
    let replay = |last: &mut i64| -> bool {
        loop {
            let page = match events::stored_after(&facade, run_id, *last, PAGE) {
                Ok(page) => page,
                Err(error) => {
                    tracing::error!("the events of agent run {run_id} could not be read: {error}");
                    return false;
                }
            };
            for event in &page {
                *last = event.seq;
                if !send(frame(Some(event.seq), &event.event_type, &event.payload)) {
                    return false;
                }
                if events::ends_stream(&event.event_type, &event.payload) {
                    return false;
                }
            }
            if i64::try_from(page.len()).unwrap_or(0) < PAGE {
                return true;
            }
        }
    };
    if !replay(&mut last) {
        return;
    }
    let hub = run::engine(factory).hub();
    let _watch = hub.watch(factory, run_id, last);
    let mut follower = run::hub::Follower::new(&hub, run_id, last);
    let mut wrote = Instant::now();
    let mut checked = Instant::now();
    loop {
        if *shutdown.borrow() || tx.is_closed() {
            return;
        }
        let next = match follower.next(&hub, &facade, Duration::from_secs(1)) {
            Ok(next) => next,
            Err(error) => {
                tracing::error!("the events of agent run {run_id} could not be read: {error}");
                return;
            }
        };
        last = follower.last();
        for event in next {
            if !send(frame(event.seq, &event.event_type, &event.payload)) {
                return;
            }
            wrote = Instant::now();
            if event.seq.is_some() && events::ends_stream(&event.event_type, &event.payload) {
                return;
            }
        }
        if wrote.elapsed() >= PING_EVERY {
            if !send(": ping\n\n".to_string()) {
                return;
            }
            wrote = Instant::now();
        }
        if checked.elapsed() >= CHECK_EVERY {
            checked = Instant::now();
            let settled = run::load_run(&facade, run_id)
                .ok()
                .flatten()
                .is_none_or(|run| run.state.is_terminal() || run.state.is_paused());
            if settled && !run::engine(factory).is_local(run_id) {
                // the last events may have come after the hub was read: send what is stored, then end
                replay(&mut last);
                return;
            }
        }
    }
}

/// `GET /assist/runs/{runId}/events.json?after=`: `{events: [{seq, type, payload}], lastEventSeq, statusId, state}`.
pub(crate) async fn events_json(
    State(state): State<Arc<HttpState>>,
    Path(run_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = owner_checked(&state, &headers, &run_id).await {
        return *response;
    }
    let after = after_of(&headers, &query);
    let factory = Arc::clone(&state.factory);
    let read = tokio::task::spawn_blocking(move || -> Result<Json, moqui_lib::error::Error> {
        let facade = factory
            .entity()
            .ok_or_else(|| moqui_lib::error::Error::new("the entity facade is not installed"))?;
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let stored = events::stored_after(&facade, &run_id, after, 1000)?;
        let run = run::load_run(&facade, &run_id)?;
        Ok(json!({
            "events": stored.iter().map(events::StoredEvent::to_json).collect::<Vec<_>>(),
            "lastEventSeq": stored.last().map_or(after, |event| event.seq),
            "statusId": run.as_ref().map(|run| run.state.status_id()),
            "state": run.as_ref().map(|run| run.state.wire()),
        }))
    })
    .await;
    match read {
        Ok(Ok(body)) => Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .header("cache-control", "no-store")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap_or_else(|_| plain(500, "the catch-up could not be written")),
        Ok(Err(error)) => crate::http::internal_error(error.message()),
        Err(_) => plain(500, "the catch-up stopped"),
    }
}
