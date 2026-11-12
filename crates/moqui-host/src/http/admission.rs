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

//! How an HTTP request reaches a request worker (plans/archive/High-Level-Review.md HL-006): the screens, DAV and MCP share it.
//!
//! - **Completion.** The job is queued with the pool's completion hook
//!   ([`RequestPool::try_submit_then`](moqui_core::service::RequestPool::try_submit_then)), which finishes the
//!   handler's future once the job's context and cleanup have ended on the worker; no thread, blocking task or second
//!   result waits on the reply.
//! - **Overload.** A full queue (or a stopped pool) is answered 503 with `Retry-After` at once, counted as
//!   `request_queue_full`.
//! - **Queue age.** A job a worker takes after the guard's `overload` `queue-timeout` is not run; its client gets
//!   503, counted as `request_queue_expired`.
//! - **Abandonment.** A job whose client has gone (its handler future was dropped: the client closed, or a deadline
//!   passed) before a worker took it is not run, counted as `request_abandoned`. Once it has started it runs to its end,
//!   so a write a client started is not cut part way.
//! - **The factory.** The handler lets go of its hold on the factory as soon as the job is queued, off the runtime when
//!   it is the last ([`crate::worker::release`]); the job holds its own on the worker.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::response::Response;
use moqui_core::context::{ErrorKind, ExecutionContextFactory};

use crate::guard::Guard;

/// What a worker did with a queued request.
enum Outcome<T> {
    Ran(T),
    /// The client had gone before a worker took it.
    Abandoned,
    /// It waited longer than the queue timeout.
    Expired,
}

/// Why a request was not served.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The queue is full or the pool stopped: nothing was queued.
    Busy,
    /// It waited in the queue past the queue timeout.
    Expired,
    /// The worker or the pool stopped without an answer.
    Lost(String),
}

impl Refusal {
    /// The answer to the client: 503 with `Retry-After` for a busy server, 500 when the worker was lost.
    pub(crate) fn response(&self, guard: &Guard) -> Response {
        match self {
            Self::Busy => guard.unavailable("the server is busy; try again"),
            Self::Expired => guard.unavailable("the server is busy; try again"),
            Self::Lost(why) => crate::http::plain(500, &format!("the request worker stopped: {why}")),
        }
    }
}

/// Run `job` on a request worker of `factory` under the shared policies above, and await its answer without holding
/// a thread. `queue_timeout` is the guard's `overload` `queue-timeout`.
pub(crate) async fn run<T, F>(
    factory: Arc<ExecutionContextFactory>,
    guard: Arc<Guard>,
    queue_timeout: Option<Duration>,
    job: F,
) -> Result<T, Refusal>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (sender, answer) = tokio::sync::oneshot::channel::<Result<Outcome<T>, String>>();
    // shared by the job, which looks whether the client is still there, and the completion hook, which answers
    let sender = Arc::new(Mutex::new(Some(sender)));
    let waiting = Arc::clone(&sender);
    let queued = Instant::now();
    let counts = Arc::clone(&guard);
    let submitted = factory.request_pool().try_submit_then(
        Arc::clone(&factory),
        move || {
            let gone = moqui_lib::sync::lock(&waiting)
                .as_ref()
                .is_none_or(tokio::sync::oneshot::Sender::is_closed);
            if gone {
                counts.counts.abandoned.fetch_add(1, Ordering::Relaxed);
                return Ok(Outcome::Abandoned);
            }
            if queue_timeout.is_some_and(|wait| queued.elapsed() > wait) {
                counts.counts.queue_expired.fetch_add(1, Ordering::Relaxed);
                return Ok(Outcome::Expired);
            }
            Ok(Outcome::Ran(job()))
        },
        move |result| {
            if let Some(sender) = moqui_lib::sync::lock(&sender).take() {
                let _ = sender.send(result.map_err(|err| err.message().to_string()));
            }
        },
    );
    crate::worker::release(factory);
    if let Err(err) = submitted {
        if err.kind() == ErrorKind::Unavailable {
            guard.counts.queue_full.fetch_add(1, Ordering::Relaxed);
            return Err(Refusal::Busy);
        }
        return Err(Refusal::Lost(err.message().to_string()));
    }
    match answer.await {
        Ok(Ok(Outcome::Ran(value))) => Ok(value),
        Ok(Ok(Outcome::Expired)) => Err(Refusal::Expired),
        // the handler is waiting, so it is not abandoned; a worker that saw it gone answers nobody
        Ok(Ok(Outcome::Abandoned)) => Err(Refusal::Lost("the client had gone".to_string())),
        Ok(Err(why)) => Err(Refusal::Lost(why)),
        Err(_) => Err(Refusal::Lost("no answer".to_string())),
    }
}

#[cfg(test)]
#[path = "admission_tests.rs"]
mod tests;
