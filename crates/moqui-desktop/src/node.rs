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

//! The node in the app's process (plans/Mobile-Desktop.md, configuration 2): booted on a thread of its own with
//! [`moqui_host::start_embedded`], listening on a loopback port only, and stopped when the app quits, which stops the
//! embedded Postgres. A restart the node asks for (`moqui_core::devtools::restart`) restarts the app.

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

/// What the node thread tells the app.
pub(crate) enum Event {
    /// Listening at this address.
    Started(SocketAddr),
    /// Did not start, with the error.
    Failed(String),
    /// Stopped by itself: a termination signal, or a restart the node asked for (true).
    Stopped { restart: bool },
}

/// The node thread. [`Node::stop`] stops the node and waits for it.
pub(crate) struct Node {
    stop: Sender<()>,
    thread: JoinHandle<()>,
}

impl Node {
    /// Boot the node on `runtime` on a thread of its own, listening on 127.0.0.1 at `port`. `events` hears when it
    /// starts, fails or stops by itself; it runs on the node thread.
    pub(crate) fn start(runtime: PathBuf, port: u16, events: impl Fn(Event) + Send + 'static) -> std::io::Result<Self> {
        let (stop, stopped) = channel::<()>();
        let thread = std::thread::Builder::new()
            .name("moqui-desktop-node".to_string())
            .spawn(move || {
                set_listen_properties(port);
                let server = match moqui_host::start_embedded(&runtime) {
                    Ok(server) => server,
                    Err(err) => {
                        tracing::error!("The node did not start: {err}");
                        events(Event::Failed(err.to_string()));
                        return;
                    }
                };
                events(Event::Started(server.address()));
                loop {
                    match stopped.recv_timeout(Duration::from_millis(500)) {
                        // The app is quitting: dropping the server stops it, then Postgres.
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                        Err(RecvTimeoutError::Timeout) if server.is_stopped() => {
                            drop(server);
                            events(Event::Stopped {
                                restart: moqui_core::devtools::restart::restart_requested(),
                            });
                            return;
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                }
            })?;
        Ok(Self { stop, thread })
    }

    /// Stop the node and wait until it and Postgres have stopped. A node still booting finishes its boot first.
    pub(crate) fn stop(self) {
        let _ = self.stop.send(());
        if self.thread.join().is_err() {
            tracing::error!("The node thread panicked while stopping");
        }
    }
}

/// The listeners of a node that only this computer reaches: HTTP on 127.0.0.1 at `port`, no public HTTPS, the cluster
/// listener on loopback at a port of the kernel's choosing, and no OpenSearch (search runs on Postgres). The edge link
/// dials out, so nothing listens for it.
fn set_listen_properties(port: u16) {
    for (name, value) in [
        ("webapp_http_bind", "127.0.0.1".to_string()),
        ("webapp_http_port", port.to_string()),
        ("webapp_https_listen_port", "off".to_string()),
        ("cluster_bind_address", "127.0.0.1".to_string()),
        ("cluster_https_port", "0".to_string()),
        ("opensearch_run", "false".to_string()),
    ] {
        moqui_lib::env::set_property(name, value);
    }
}

/// The node's HTTP port: the one it had last time (`saved`) when it is still free, so the window's origin, and with it
/// what the pages keep in local storage, stays the same; otherwise a free one.
///
/// # Errors
///
/// Returns an error when no loopback port can be bound.
pub(crate) fn choose_port(saved: Option<u16>) -> std::io::Result<u16> {
    if let Some(port) = saved.filter(|port| *port != 0) {
        if TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok() {
            return Ok(port);
        }
        tracing::warn!("Port {port} of the last run is taken; the node takes another, and the pages start with empty local storage");
    }
    Ok(TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?.local_addr()?.port())
}

/// The port in the file at `path`, written by [`save_port`].
pub(crate) fn saved_port(path: &Path) -> Option<u16> {
    let text = std::fs::read_to_string(path).ok()?;
    let port: u16 = text.trim().parse().ok()?;
    Some(port)
}

/// Keep `port` in the file at `path` for the next run.
pub(crate) fn save_port(path: &Path, port: u16) {
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(path, format!("{port}\n")));
    if let Err(err) = written {
        tracing::warn!("Could not keep the node's port in {}: {err}", path.display());
    }
}

/// Whether a server already answers on this runtime's own port (`webapp_http_port`, default 8080), as `moqui tui`
/// checks: two nodes on one runtime would share its Postgres. A development run on this repository's runtime only.
pub(crate) fn served_on_runtime_port() -> Option<u16> {
    let port = moqui_core::web::effective_http_port(&moqui_lib::env::get_prop_or_env("webapp_http_port")).ok()?;
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    std::net::TcpStream::connect_timeout(&address, Duration::from_millis(300))
        .ok()
        .map(|_| port)
}

#[cfg(test)]
#[path = "node_tests.rs"]
mod tests;
