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

//! What `http::listen` builds once for its web listeners, which the listener registry's web entries start and stop
//! with (plans/Runtime-Config.md §8.1, RC0-C): [`WebContext`] (the app, the connection permits, the TLS handshake, the
//! boot's HTTPS plan, the process-wide stop), and [`ServeTask`], one listener's serve loop on the axum runtime. A loop
//! binds in the web pass but takes no connection until `http::listen` releases [`WebContext::go`], where its accept
//! loops started before the registry (after the startup report, READY and the shutdown task). It stops on the
//! process-wide stop or its own, whichever comes first, then lets its connections finish for up to `limits.shutdown`.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::{mpsc, Arc};
use std::time::Duration;

use moqui_core::context::ExecutionContextFactory;
use tokio::net::TcpListener;
use tokio::sync::watch;

use super::{serve, standing_by, Tls, WebApp};

/// The boot's HTTPS plan (`tls::plan`): where it binds, and whether it stands by with no certificate (`crate::acme`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HttpsPlan {
    pub(crate) bind: SocketAddr,
    pub(crate) standby: bool,
}

/// What the web listeners serve with, built once by `http::listen` and installed on the registry.
pub(crate) struct WebContext {
    pub(super) factory: Arc<ExecutionContextFactory>,
    /// The axum runtime, which the loops run on and the binds are made on (`listeners::worker::on_runtime`).
    pub(super) handle: tokio::runtime::Handle,
    pub(super) web: WebApp,
    /// The connections at once, shared by the HTTP and HTTPS listeners (`connections`).
    pub(super) permits: Arc<tokio::sync::Semaphore>,
    /// The HTTPS handshake (the edge and manager links' hand-offs, ACME's challenges); none with no HTTPS plan.
    pub(super) tls: Option<Tls>,
    pub(super) https: Option<HttpsPlan>,
    /// The gRPC services the HTTPS listener carries over HTTP/2, as its startup report row names them.
    pub(super) https_grpc: Option<String>,
    /// The process-wide stop, which every loop ends on.
    pub(super) shutdown: watch::Receiver<bool>,
    /// Turns true when the loops may take connections; a sender dropped first (a start that failed) serves nothing.
    pub(super) go: watch::Receiver<bool>,
    #[cfg(feature = "grpc")]
    pub(super) guard: Arc<crate::guard::Guard>,
    /// The HTTPS listener's TLS settings for the gRPC listener, when HTTPS serves (not standing by).
    #[cfg(feature = "grpc")]
    pub(super) grpc_tls: Option<Arc<rustls::ServerConfig>>,
}

impl WebContext {
    pub(crate) fn handle(&self) -> &tokio::runtime::Handle {
        &self.handle
    }

    pub(crate) fn https_plan(&self) -> Option<HttpsPlan> {
        self.https
    }

    pub(crate) fn https_grpc(&self) -> Option<&str> {
        self.https_grpc.as_deref()
    }

    #[cfg(feature = "grpc")]
    pub(crate) fn grpc_tls(&self) -> Option<Arc<rustls::ServerConfig>> {
        self.grpc_tls.clone()
    }

    /// Serve `listener`, with the HTTPS handshake when `tls`, until the process stops or the task's own stop.
    pub(crate) fn serve(&self, listener: TcpListener, tls: bool) -> ServeTask {
        let (stop, own) = watch::channel(false);
        let released = released(self.go.clone(), self.shutdown.clone(), own.clone());
        let until = either(self.shutdown.clone(), own);
        let tls = if tls { self.tls.clone() } else { None };
        let (web, permits) = (self.web.clone(), Arc::clone(&self.permits));
        ServeTask::spawn(&self.handle, stop, async move {
            if released.await {
                serve(listener, tls, web, permits, until).await;
            }
        })
    }

    /// The HTTPS listener of a node with no certificate yet, bound at `bind` while ACME wants it (`standing_by`, which
    /// records its own row), until the process stops or the task's own stop.
    pub(crate) fn stand_by(&self, bind: SocketAddr) -> ServeTask {
        let (stop, own) = watch::channel(false);
        let released = released(self.go.clone(), self.shutdown.clone(), own.clone());
        let until = merged(&self.handle, self.shutdown.clone(), own);
        let (factory, web, permits) = (Arc::clone(&self.factory), self.web.clone(), Arc::clone(&self.permits));
        let tls = self.tls.clone();
        ServeTask::spawn(&self.handle, stop, async move {
            // a plan that stands by has its TLS settings: with none nothing could be served
            if let (true, Some(tls)) = (released.await, tls) {
                standing_by(&factory, bind, tls, web, permits, until).await;
            }
        })
    }

    /// Serve the bound gRPC listener until the process stops or the task's own stop.
    #[cfg(feature = "grpc")]
    pub(crate) fn serve_grpc(&self, bound: crate::grpc_listen::Bound) -> ServeTask {
        let (stop, own) = watch::channel(false);
        let released = released(self.go.clone(), self.shutdown.clone(), own.clone());
        let until = either(self.shutdown.clone(), own);
        let guard = Arc::clone(&self.guard);
        ServeTask::spawn(&self.handle, stop, async move {
            if released.await {
                crate::grpc_listen::serve(bound, guard, until).await;
            }
        })
    }
}

/// Whether a loop may take connections: true once `go` turns true, false when a stop comes first or `go`'s sender is
/// dropped (a start that failed after the web pass).
async fn released(mut go: watch::Receiver<bool>, process: watch::Receiver<bool>, own: watch::Receiver<bool>) -> bool {
    tokio::select! {
        released = go.wait_for(|go| *go) => released.is_ok(),
        () = either(process, own) => false,
    }
}

/// Resolves when the process-wide stop or the listener's own turns true, or its sender is gone.
async fn either(mut process: watch::Receiver<bool>, mut own: watch::Receiver<bool>) {
    tokio::select! {
        _ = process.wait_for(|stopped| *stopped) => {}
        _ = own.wait_for(|stopped| *stopped) => {}
    }
}

/// A stop that turns true with the process-wide stop or the listener's own, for a loop that reads its stop as a watch.
fn merged(
    handle: &tokio::runtime::Handle,
    process: watch::Receiver<bool>,
    own: watch::Receiver<bool>,
) -> watch::Receiver<bool> {
    let (tx, rx) = watch::channel(*process.borrow());
    handle.spawn(async move {
        either(process, own).await;
        let _ = tx.send(true);
    });
    rx
}

/// One web listener's loop on the axum runtime: told to stop by its own watch, reported finished on a plain channel,
/// so a plain thread (the registry's pass or stop) waits for it without a runtime.
pub(crate) struct ServeTask {
    stop: watch::Sender<bool>,
    done: mpsc::Receiver<()>,
    task: tokio::task::AbortHandle,
}

impl ServeTask {
    fn spawn(
        handle: &tokio::runtime::Handle,
        stop: watch::Sender<bool>,
        run: impl Future<Output = ()> + Send + 'static,
    ) -> Self {
        let (finished, done) = mpsc::sync_channel(1);
        let task = handle
            .spawn(async move {
                run.await;
                let _ = finished.send(());
            })
            .abort_handle();
        Self { stop, done, task }
    }

    /// Stop taking connections; those open finish for up to `limits.shutdown`.
    pub(crate) fn begin_stop(&self) {
        let _ = self.stop.send(true);
    }

    /// Wait up to `wait` for the loop to end; a loop that panicked or was dropped has ended too.
    pub(crate) fn finished(&self, wait: Duration) -> bool {
        !matches!(self.done.recv_timeout(wait), Err(mpsc::RecvTimeoutError::Timeout))
    }
}

/// A loop dropped without a stop that finished (a boot that failed part way, or a stop past its wait) stops at once.
impl Drop for ServeTask {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        self.task.abort();
    }
}

#[cfg(test)]
#[path = "listening_tests.rs"]
mod tests;
