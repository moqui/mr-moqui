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

//! The factory's background threads: work that reaches the database outside the service pools (the artifact index's
//! build, the observed calls' flush). The factory's shutdown waits for them, up to a limit, before the host stops the
//! database. Each one stops by checking [`super::ExecutionContextFactory::is_shutting_down`].
//!
//! It also marks the threads the framework starts for its own work ([`framework_thread`]): leaving the outermost
//! execution context on one is no drain point of an inline factory.

use std::cell::Cell;
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use moqui_lib::sync::lock;

use super::Error;

thread_local! {
    /// Set by [`framework_thread`] on a thread the framework started for its own work.
    static FRAMEWORK_THREAD: Cell<bool> = const { Cell::new(false) };
}

/// `body` as the body of a thread the framework starts for its own work: a sender, a watcher, a lease, a sweep, a
/// worker a call fans out to. Every framework thread that may enter an execution context is started with it
/// (`.spawn(framework_thread(move || ...))`, and the same for a scoped thread).
///
/// Leaving the outermost context on such a thread is no drain point of an inline factory
/// ([`super::Executor::Inline`]): the factory's queued work belongs to the thread that drains it (a test, the browser's
/// worker), and run here it would be taken from under that thread's `run_pending`, or run while that thread is still
/// in its transaction. The threaded executor never drains there, so a server is not changed by it.
///
/// Not for a thread that owns the queue: a test's thread, the spec runner's (`spawn_script_thread`), the browser's
/// worker. moqui-host's server threads run only under the threaded executor and are left unmarked.
///
/// The mark lasts while `body` runs and is put back as it was when `body` ends or panics, so a job wrapped with it on a
/// reused thread never leaves that thread unable to drain, and a nested use keeps the outer mark.
pub fn framework_thread<T, F: FnOnce() -> T>(body: F) -> impl FnOnce() -> T {
    move || {
        let _restore = MarkRestore(FRAMEWORK_THREAD.with(|mark| mark.replace(true)));
        body()
    }
}

/// Puts this thread's [`FRAMEWORK_THREAD`] mark back to what it was before [`framework_thread`] set it.
struct MarkRestore(bool);

impl Drop for MarkRestore {
    fn drop(&mut self) {
        let _ = FRAMEWORK_THREAD.try_with(|mark| mark.set(self.0));
    }
}

/// True on a thread started with [`framework_thread`].
pub(crate) fn is_framework_thread() -> bool {
    FRAMEWORK_THREAD.try_with(Cell::get).unwrap_or(false)
}

/// How long the shutdown waits for the background threads, all together.
pub(crate) const BACKGROUND_WAIT: Duration = Duration::from_secs(10);

#[derive(Default)]
pub(crate) struct BackgroundThreads {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    handles: Vec<JoinHandle<()>>,
    /// Set by [`BackgroundThreads::wait`]: a thread started after it would not be waited for, so none starts.
    closed: bool,
}

impl BackgroundThreads {
    /// Run `work` on a thread named `name` that [`Self::wait`] waits for.
    pub(crate) fn spawn(&self, name: &str, work: impl FnOnce() + Send + 'static) -> Result<(), Error> {
        let mut state = lock(&self.state);
        if state.closed {
            return Err(Error::new(format!("{name} not started: the factory is shutting down")));
        }
        state.handles.retain(|handle| !handle.is_finished());
        let handle = std::thread::Builder::new()
            .name(name.to_string())
            .spawn(framework_thread(work))
            .map_err(|err| Error::new(format!("could not start {name}: {err}")))?;
        state.handles.push(handle);
        Ok(())
    }

    /// Wait up to `limit` for every thread to end. One still running then is logged by name and left.
    pub(crate) fn wait(&self, limit: Duration) {
        let handles = {
            let mut state = lock(&self.state);
            state.closed = true;
            std::mem::take(&mut state.handles)
        };
        let current = std::thread::current().id();
        let deadline = Instant::now() + limit;
        for handle in handles {
            // a shutdown begun on one of these threads cannot wait for itself
            if handle.thread().id() == current {
                continue;
            }
            while !handle.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            let name = name_of(&handle);
            if handle.is_finished() {
                if handle.join().is_err() {
                    tracing::error!("Background thread {name} panicked");
                }
            } else {
                tracing::warn!(
                    "Background thread {name} still running after {} seconds of shutdown; not waiting for it",
                    limit.as_secs()
                );
            }
        }
    }
}

fn name_of(handle: &JoinHandle<()>) -> String {
    handle.thread().name().unwrap_or("(unnamed)").to_string()
}

#[cfg(test)]
#[path = "background_tests.rs"]
mod tests;
