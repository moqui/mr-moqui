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
//! with (plans/Runtime-Config.md §8.1, §8.3): [`WebContext`] (the app, the connection permits, the TLS handshake and
//! its certificates, the process-wide stop), and [`ServeTask`], one listener's serve loop on the axum runtime. A loop
//! bound in the boot's web pass takes no connection until `http::listen` releases [`WebContext::go`], where its accept
//! loops started before the registry (after the startup report, READY and the shutdown task); one started later serves
//! at once. It stops on the process-wide stop or its own, whichever comes first, then lets its connections finish for
//! the grace of that stop: `limits.shutdown` for the process stop, a live stop's own (`LIVE_STOP_GRACE`, or none), and
//! drops what is left.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use moqui_lib::sync::lock;
use tokio::net::TcpListener;
use tokio::sync::watch;

use super::{serve, Grace, Listening, StopSignal, Tls, WebApp};
use crate::listeners::StopHow;

/// What the web listeners serve with, built once by `http::listen` and installed on the registry.
pub(crate) struct WebContext {
    /// The axum runtime, which the loops run on and the binds are made on (`listeners::worker::on_runtime`).
    pub(super) handle: tokio::runtime::Handle,
    pub(super) web: WebApp,
    /// The connections at once, shared by the HTTP and HTTPS listeners (`connections`).
    pub(super) permits: Arc<tokio::sync::Semaphore>,
    /// The HTTPS handshake (the edge and manager links' hand-offs, ACME's challenges), built at boot whenever there is
    /// a web bind so HTTPS can be turned on later; none when its TLS material did not build.
    pub(super) tls: Option<Tls>,
    /// The certificates the handshake serves: with none, HTTPS binds only while ACME wants it.
    pub(super) certs: Option<Arc<crate::tls::CertStore>>,
    /// The gRPC services the HTTPS listener carries over HTTP/2, as its startup report row names them.
    pub(super) https_grpc: Option<String>,
    /// The process-wide stop, which every loop ends on.
    pub(super) shutdown: watch::Receiver<bool>,
    /// Turns true when the loops may take connections; a sender dropped first (a start that failed) serves nothing.
    pub(super) go: watch::Receiver<bool>,
    /// What a server started on an address keeps of it while its conf does not change ([`AddressHold`]).
    pub(super) held: AddressHold,
    #[cfg(feature = "grpc")]
    pub(super) guard: Arc<crate::guard::Guard>,
    /// The HTTPS listener's TLS settings, which the gRPC listener serves with while there is a certificate.
    #[cfg(feature = "grpc")]
    pub(super) grpc_tls: Option<Arc<rustls::ServerConfig>>,
}

impl WebContext {
    pub(crate) fn handle(&self) -> &tokio::runtime::Handle {
        &self.handle
    }

    pub(crate) fn https_grpc(&self) -> Option<&str> {
        self.https_grpc.as_deref()
    }

    /// Whether HTTPS has TLS settings, and then whether a certificate loads (read again at most every few seconds,
    /// so call it on a plain thread): none when the material did not build.
    pub(crate) fn has_certificate(&self) -> Option<bool> {
        self.certs.as_ref().map(|certs| !certs.is_empty())
    }

    /// Whether this node is to take edge links on its HTTPS listener but has no acceptor for them: one is made at boot
    /// only, with a certificate and HTTPS on, so HTTPS turned on later takes links after a restart
    /// (plans/Runtime-Config.md §3.3).
    pub(crate) fn edge_links_wait(&self) -> bool {
        moqui_core::edge::takes_links() && self.tls.as_ref().is_none_or(|tls| tls.edge.is_none())
    }

    pub(crate) fn held(&self) -> &AddressHold {
        &self.held
    }

    #[cfg(feature = "grpc")]
    pub(crate) fn grpc_tls(&self) -> Option<Arc<rustls::ServerConfig>> {
        self.grpc_tls.clone()
    }

    /// Serve `listener` as the listener `name`, with the HTTPS handshake when `tls`, until the process stops or the
    /// task's own stop, whose grace its connections then get.
    pub(crate) fn serve(&self, listener: TcpListener, tls: bool, name: &'static str) -> ServeTask {
        let (stop, own) = watch::channel(None);
        let released = released(self.go.clone(), self.shutdown.clone(), own.clone());
        let until = StopSignal::new(
            grace_on_stop(self.shutdown.clone(), own.clone(), self.web.limits.shutdown),
            ends_now(own),
        );
        let tls = if tls { self.tls.clone() } else { None };
        let (web, permits) = (self.web.clone(), Arc::clone(&self.permits));
        let listening = Listening::default();
        let reported = listening.clone();
        ServeTask::spawn(&self.handle, stop, listening, async move {
            if released.await {
                serve(listener, tls, web, permits, name, reported, until).await;
            }
        })
    }

    /// Serve the bound gRPC listener until the process stops or the task's own stop, whose grace its connections then
    /// get.
    #[cfg(feature = "grpc")]
    pub(crate) fn serve_grpc(&self, bound: crate::grpc_listen::Bound) -> ServeTask {
        let (stop, own) = watch::channel(None);
        let released = released(self.go.clone(), self.shutdown.clone(), own.clone());
        let until = StopSignal::new(
            grace_on_stop(self.shutdown.clone(), own.clone(), self.web.limits.shutdown),
            ends_now(own),
        );
        let guard = Arc::clone(&self.guard);
        let listening = Listening::default();
        let reported = listening.clone();
        ServeTask::spawn(&self.handle, stop, listening, async move {
            if released.await {
                crate::grpc_listen::serve(bound, guard, reported, until).await;
            }
        })
    }
}

/// What a server started on an address (`ListenBind::Address`: a test's, `moqui test --serve`) keeps of it: HTTP on, at
/// that address's port and host, each until that setting (HTTP's switch, its port, `http-bind`) is set while the
/// server runs, or a conf reload changes it from what the conf said at the first web pass; from then on that setting
/// is the conf's (`crate::listeners`). So a server started on an address listens there as before, and its settings
/// still apply while it runs. A server whose listeners come from the conf (`moqui run`, the desktop node) holds none.
#[derive(Default)]
pub(crate) struct AddressHold {
    /// The conf's switch, port and address at the first web pass.
    boot: Mutex<Option<[String; 3]>>,
    /// Each one of them set or changed since, and the conf's from then on.
    released: [AtomicBool; 3],
}

impl AddressHold {
    /// Note the conf's switch, port and address at the first web pass; a later call changes nothing.
    pub(crate) fn note_boot(&self, now: &[String; 3]) {
        lock(&self.boot).get_or_insert_with(|| now.clone());
    }

    /// Which of the switch, the port and the address the address still holds.
    pub(crate) fn held(&self) -> [bool; 3] {
        let mut held = [false; 3];
        for (index, held) in held.iter_mut().enumerate() {
            *held = !self.released[index].load(Ordering::SeqCst);
        }
        held
    }

    /// The setting at `index` (0 the switch, 1 the port, 2 the address) is the conf's from now on.
    pub(crate) fn release(&self, index: usize) {
        if let Some(released) = self.released.get(index) {
            released.store(true, Ordering::SeqCst);
        }
    }

    /// After a conf reload: each setting `now` has changed from the first web pass is the conf's from now on.
    pub(crate) fn release_changed(&self, now: &[String; 3]) {
        let boot = lock(&self.boot).clone();
        let Some(boot) = boot else {
            return;
        };
        for index in 0..3 {
            if boot[index] != now[index] {
                self.release(index);
            }
        }
    }
}

/// A web listener's own stop, as its watch carries it: none while it serves, then how the registry stops it.
type OwnStop = watch::Receiver<Option<StopHow>>;

/// Whether a loop may take connections: true once `go` turns true, false when a stop comes first or `go`'s sender is
/// dropped (a start that failed after the web pass).
async fn released(mut go: watch::Receiver<bool>, process: watch::Receiver<bool>, own: OwnStop) -> bool {
    tokio::select! {
        released = go.wait_for(|go| *go) => released.is_ok(),
        () = either(process, own) => false,
    }
}

/// Resolves when the process-wide stop turns true or the listener's own comes, or either sender is gone.
async fn either(process: watch::Receiver<bool>, own: OwnStop) {
    grace_on_stop(process, own, None).await;
}

/// The grace of the first stop to come: the process-wide one's `limit` (`limits.shutdown`), or the listener's own as
/// [`grace_of`] maps it. A process-wide sender gone is the process stopping, as before the registry; the listener's
/// own gone is its loop dropped, which ends its connections at once.
async fn grace_on_stop(mut process: watch::Receiver<bool>, mut own: OwnStop, limit: Option<Duration>) -> Grace {
    tokio::select! {
        _ = process.wait_for(|stopped| *stopped) => Grace::of(limit),
        how = own.wait_for(Option::is_some) => match how {
            Ok(how) => grace_of((*how).unwrap_or(StopHow::Now), limit),
            Err(_) => Grace::Upto(Duration::ZERO),
        },
    }
}

/// Resolves when the listener's own stop asks to end its connections now, also after another stop began its grace, or
/// when its sender is gone (its loop dropped).
async fn ends_now(mut own: OwnStop) {
    let _ = own.wait_for(|stop| *stop == Some(StopHow::Now)).await;
}

/// The grace a web listener's connections get when the registry stops it `how` (plans/Runtime-Config.md §3.1):
/// `limit` (`limits.shutdown`) for the process stop, as before the registry; a live stop's own grace; none for "end
/// connections now".
fn grace_of(how: StopHow, limit: Option<Duration>) -> Grace {
    match how {
        StopHow::Process => Grace::of(limit),
        StopHow::Grace(grace) => Grace::Upto(grace),
        StopHow::Now => Grace::Upto(Duration::ZERO),
    }
}

/// One web listener's loop on the axum runtime: told to stop by its own watch, reported finished on a plain channel,
/// so a plain thread (the registry's pass or stop) waits for it without a runtime.
pub(crate) struct ServeTask {
    stop: watch::Sender<Option<StopHow>>,
    done: mpsc::Receiver<()>,
    task: tokio::task::AbortHandle,
    /// Its open connections, upgraded sockets included, and whether its port is let go.
    listening: Listening,
}

impl ServeTask {
    fn spawn(
        handle: &tokio::runtime::Handle,
        stop: watch::Sender<Option<StopHow>>,
        listening: Listening,
        run: impl Future<Output = ()> + Send + 'static,
    ) -> Self {
        let (finished, done) = mpsc::sync_channel(1);
        let freed = listening.clone();
        let task = handle
            .spawn(async move {
                run.await;
                // a loop that never served, or ended, holds no port
                freed.port_freed();
                let _ = finished.send(());
            })
            .abort_handle();
        Self {
            stop,
            done,
            task,
            listening,
        }
    }

    /// The connections it holds open now.
    pub(crate) fn connections(&self) -> usize {
        self.listening.connections()
    }

    /// Stop taking connections; those open finish for the grace of `how` ([`grace_of`]), then are dropped. A loop
    /// already stopping keeps its first stop's grace, unless `how` ends the connections now.
    pub(crate) fn begin_stop(&self, how: StopHow) {
        upgrade(&self.stop, how);
    }

    /// What ends this loop's connections now, also once its stop has begun with a grace: the registry keeps it while
    /// the loop drains, outside its lock, for "end connections now" (plans/Runtime-Config.md §3.1).
    pub(crate) fn end_now_handle(&self) -> crate::listeners::EndNow {
        let stop = self.stop.clone();
        Arc::new(move || upgrade(&stop, StopHow::Now))
    }

    /// Wait up to `wait` for the loop's port to be let go, which comes before its connections' grace: a new bind of
    /// the same port need not wait for the drain. A loop that ended or panicked holds none.
    pub(crate) fn released(&self, wait: Duration) -> bool {
        self.listening.wait_freed(wait) || self.task.is_finished()
    }

    /// Wait up to `wait` for the loop to end; a loop that panicked or was dropped has ended too.
    pub(crate) fn finished(&self, wait: Duration) -> bool {
        !matches!(self.done.recv_timeout(wait), Err(mpsc::RecvTimeoutError::Timeout))
    }
}

/// Begin a stop `how`: the first stop gives the grace, and a later one only ends the connections now.
fn upgrade(stop: &watch::Sender<Option<StopHow>>, how: StopHow) {
    stop.send_if_modified(|current| match (*current, how) {
        (None, _) | (Some(StopHow::Process | StopHow::Grace(_)), StopHow::Now) => {
            *current = Some(how);
            true
        }
        _ => false,
    });
}

/// A loop dropped without a stop that finished (a boot that failed part way, or a stop past its wait) stops at once,
/// and its connections go with it.
impl Drop for ServeTask {
    fn drop(&mut self) {
        self.begin_stop(StopHow::Now);
        self.task.abort();
    }
}

#[cfg(test)]
#[path = "listening_tests.rs"]
mod tests;
