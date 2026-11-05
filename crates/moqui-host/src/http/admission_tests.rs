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

//! The shared admission of the screen, DAV and MCP routes (HL-006): a request whose client left before a worker took
//! it is not run, a full queue is 503 at once, a request past the queue timeout is 503, and no blocking thread waits
//! while a job is queued.

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, Request, State};
use axum::response::Response;
use moqui_core::context::{ExecutionContext, ExecutionContextFactory};

use super::{run, Refusal};
use crate::http::{HttpState, ListenLimits};

/// A factory whose request pool has one worker and room for `queue` jobs.
fn factory(queue: usize) -> (ExecutionContext, Arc<ExecutionContextFactory>) {
    let context = ExecutionContextFactory::test_context();
    let factory = Arc::clone(context.factory());
    factory.request_pool().start(1, queue).unwrap();
    (context, factory)
}

/// Take the pool's one worker until the returned sender is dropped or sent to.
fn hold_the_worker(factory: &Arc<ExecutionContextFactory>) -> mpsc::Sender<()> {
    let (release, held) = mpsc::channel::<()>();
    let (started, running) = mpsc::channel::<()>();
    factory
        .request_pool()
        .try_submit_then(
            Arc::clone(factory),
            move || {
                let _ = started.send(());
                let _ = held.recv();
                Ok(())
            },
            |_| {},
        )
        .unwrap();
    running.recv_timeout(Duration::from_secs(5)).unwrap();
    release
}

fn state(factory: &Arc<ExecutionContextFactory>) -> Arc<HttpState> {
    let (_, shutdown) = tokio::sync::watch::channel(false);
    Arc::new(HttpState {
        factory: Arc::clone(factory),
        shutdown,
        limits: ListenLimits::defaults(),
        guard: crate::guard::tests::guard("<web-protection/>", ""),
    })
}

fn peer() -> ConnectInfo<SocketAddr> {
    ConnectInfo("127.0.0.1:40000".parse().unwrap())
}

fn request(method: &str, path: &str) -> Request {
    axum::http::Request::builder()
        .method(method)
        .uri(path)
        .body(axum::body::Body::from("{}"))
        .unwrap()
}

type Handler = fn(Arc<HttpState>) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>;

/// The three routes, each with a request it serves.
fn adapters() -> Vec<(&'static str, Handler)> {
    vec![
        ("screen", |state| {
            Box::pin(crate::http::dispatch_screen(State(state), peer(), request("GET", "/")))
        }),
        ("dav", |state| {
            Box::pin(crate::dav_http::handle(
                State(state),
                peer(),
                request("OPTIONS", "/dav/"),
            ))
        }),
        #[cfg(feature = "llm")]
        ("mcp", |state| {
            Box::pin(crate::llm::mcp::handle(State(state), peer(), request("POST", "/mcp")))
        }),
    ]
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap()
}

/// Wait (at most 5 s) for `check`.
fn eventually(check: impl Fn() -> bool) {
    let until = std::time::Instant::now() + Duration::from_secs(5);
    while !check() {
        assert!(std::time::Instant::now() < until, "it did not happen");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A client that leaves while its request is queued: the job is not run, and is counted abandoned.
#[test]
fn a_request_whose_client_left_before_a_worker_took_it_is_not_run() {
    let (_context, factory) = factory(8);
    let runtime = runtime();
    for (name, handler) in adapters() {
        let state = state(&factory);
        let release = hold_the_worker(&factory);
        let left = runtime
            .block_on(async { tokio::time::timeout(Duration::from_millis(50), handler(Arc::clone(&state))).await });
        assert!(left.is_err(), "{name}: the request waits for the worker");
        drop(release);
        eventually(|| state.guard.counts.abandoned.load(Ordering::Relaxed) == 1);
        assert_eq!(state.guard.counts.queue_full.load(Ordering::Relaxed), 0, "{name}");
    }
}

/// A full queue is answered 503 with Retry-After at once, and counted.
#[test]
fn a_full_queue_is_answered_503_at_once() {
    let (_context, factory) = factory(1);
    let runtime = runtime();
    for (name, handler) in adapters() {
        let state = state(&factory);
        let release = hold_the_worker(&factory);
        // the one place in the queue
        let (filler, filled) = mpsc::channel::<()>();
        factory
            .request_pool()
            .try_submit_then(
                Arc::clone(&factory),
                move || {
                    drop(filled);
                    Ok(())
                },
                |_| {},
            )
            .unwrap();
        let answered = runtime
            .block_on(async { tokio::time::timeout(Duration::from_secs(2), handler(Arc::clone(&state))).await })
            .expect("answered at once");
        assert_eq!(answered.status(), 503, "{name}");
        assert!(answered.headers().contains_key("retry-after"), "{name}");
        assert_eq!(state.guard.counts.queue_full.load(Ordering::Relaxed), 1, "{name}");
        drop(release);
        drop(filler);
        eventually(|| factory.request_pool().queue_depth() == 0);
    }
}

/// With the runtime's one blocking thread taken, a queued request still completes when a worker takes it: nothing
/// blocks a thread while it waits; one that waited past the queue timeout is refused without running.
#[test]
fn a_queued_request_holds_no_blocking_thread_and_expires_past_the_queue_timeout() {
    let (_context, factory) = factory(8);
    let runtime = runtime();
    let guard = crate::guard::tests::guard("<web-protection/>", "");
    let (unblock, blocked) = mpsc::channel::<()>();
    runtime.block_on(async {
        let blocking = tokio::task::spawn_blocking(move || {
            let _ = blocked.recv();
        });
        let release = hold_the_worker(&factory);
        let waiting = tokio::spawn(run(Arc::clone(&factory), Arc::clone(&guard), None, || 42));
        let late = tokio::spawn(run(
            Arc::clone(&factory),
            Arc::clone(&guard),
            Some(Duration::from_millis(20)),
            || 43,
        ));
        tokio::time::sleep(Duration::from_millis(60)).await;
        drop(release);
        let answer = tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("completed with no blocking thread free")
            .unwrap();
        assert_eq!(answer, Ok(42));
        assert_eq!(late.await.unwrap(), Err(Refusal::Expired));
        assert_eq!(guard.counts.queue_expired.load(Ordering::Relaxed), 1);
        // each route, queued behind a busy worker, completes once the worker is free
        for (name, handler) in adapters() {
            let release = hold_the_worker(&factory);
            let queued = tokio::spawn(handler(state(&factory)));
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert!(!queued.is_finished(), "{name}: waits for the worker");
            drop(release);
            let answered = tokio::time::timeout(Duration::from_secs(5), queued)
                .await
                .unwrap_or_else(|_| panic!("{name}: completed with no blocking thread free"))
                .unwrap();
            assert_ne!(answered.status(), 503, "{name}");
        }
        unblock.send(()).unwrap();
        blocking.await.unwrap();
    });
}
