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

//! Restart from a running server (plans/design-dev-aide.md 6.5): the universal fallback for what does not reload (an
//! entity change until F5, a conf or component change). A request sets a flag and asks the host to shut down as it
//! does on SIGTERM, which stops embedded Postgres and frees the ports; after the factory is destroyed the host sees the
//! flag and runs itself again with the same arguments (on Unix the same process, so `./moq run` keeps it).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use moqui_lib::sync::lock;

static REQUESTED: AtomicBool = AtomicBool::new(false);

type Shutdown = Box<dyn Fn() + Send + Sync>;

static SHUTDOWN: Mutex<Option<Shutdown>> = Mutex::new(None);

/// The host's graceful shutdown, which [`request_restart`] calls. The HTTP server installs it when it starts.
pub fn set_shutdown(shutdown: impl Fn() + Send + Sync + 'static) {
    *lock(&SHUTDOWN) = Some(Box::new(shutdown));
}

/// Ask for a restart: the host shuts down gracefully, then runs again. False when no host can shut down (a command,
/// not a server), and nothing happens.
pub fn request_restart() -> bool {
    let shutdown = lock(&SHUTDOWN);
    let Some(shutdown) = shutdown.as_ref() else {
        return false;
    };
    REQUESTED.store(true, Ordering::SeqCst);
    tracing::warn!("Restart requested: shutting down, then running again with the same arguments");
    shutdown();
    true
}

/// Whether a restart was asked for; the host checks this after its shutdown.
#[must_use]
pub fn restart_requested() -> bool {
    REQUESTED.load(Ordering::SeqCst)
}
