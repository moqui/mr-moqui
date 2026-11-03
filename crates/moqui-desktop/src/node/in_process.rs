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
//! [`moqui_host::start_embedded_with`], with no port (the window reaches it in process, [`super::scheme`]) or, when
//! the user opens them, listening on a loopback HTTP port too; stopped when the app quits, which stops the embedded
//! Postgres. A restart the node asks for (`moqui_core::devtools::restart`) restarts the app.
//! This is the default mode; [`super::sidecar`] is the other.

use std::path::PathBuf;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use super::Event;

/// The node thread. [`Node::stop`] stops the node and waits for it.
pub(crate) struct Node {
    stop: Sender<()>,
    thread: JoinHandle<()>,
}

impl Node {
    /// Boot the node on `runtime` on a thread of its own, with `ports` listening on 127.0.0.1 at `port`, with
    /// `properties` set beside the listen properties. `events` hears when it starts, fails or stops by itself; it runs
    /// on the node thread.
    pub(super) fn start(
        runtime: PathBuf,
        port: u16,
        ports: bool,
        properties: Vec<(&'static str, String)>,
        events: impl Fn(Event) + Send + 'static,
    ) -> std::io::Result<Self> {
        let (stop, stopped) = channel::<()>();
        let thread = std::thread::Builder::new()
            .name("moqui-desktop-node".to_string())
            .spawn(move || {
                for (name, value) in super::listen_properties(port).into_iter().chain(properties) {
                    moqui_lib::env::set_property(name, value);
                }
                let server = match moqui_host::start_embedded_with(&runtime, ports) {
                    Ok(server) => server,
                    Err(err) => {
                        tracing::error!("The node did not start: {err}");
                        events(Event::Failed(err.to_string()));
                        return;
                    }
                };
                match server.in_process() {
                    Some(local) => events(Event::StartedInProcess(local.clone())),
                    None => events(Event::Failed("the node has no app in process".to_string())),
                }
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
    pub(super) fn stop(self) {
        let _ = self.stop.send(());
        if self.thread.join().is_err() {
            tracing::error!("The node thread panicked while stopping");
        }
    }
}
