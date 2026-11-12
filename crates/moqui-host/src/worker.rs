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

//! Run a blocking framework job on a request worker from async code (the SSH server, the SMTP listener, the XMPP
//! server, the edge link and the notification socket), without blocking the async runtime. The pool's completion hook
//! ([`RequestPool::try_submit_then`](moqui_core::service::RequestPool::try_submit_then)) finishes a oneshot the task
//! awaits, so no thread waits on the reply (`plans/archive/High-Level-Review.md` HL-006); the job's context and cleanup stay on
//! the worker. The tasks hold the factory weakly, so a server that stopped is not kept alive by them, and its database
//! clients are never dropped on a runtime thread ([`release`]).

use std::sync::{Arc, Weak};

use moqui_core::context::{Error as CoreError, ExecutionContextFactory};

/// Run `job` on a request worker, in a fresh context with no user and authorization on, and wait without blocking the
/// async runtime. A job that logs a user in runs as that user; the login calls turn authorization off only while they
/// read the user tables.
pub(crate) async fn on_pool<T, F>(factory: &Weak<ExecutionContextFactory>, job: F) -> Result<T, CoreError>
where
    T: Send + 'static,
    F: FnOnce(&ExecutionContextFactory) -> Result<T, CoreError> + Send + 'static,
{
    let Some(strong) = factory.upgrade() else {
        return Err(CoreError::new("the server is stopping"));
    };
    let inner = Arc::clone(&strong);
    submit(strong, move || job(&inner)).await
}

/// Run `job` on `factory`'s request worker in a fresh context and await its result. `factory` is let go at once, off the
/// runtime when it is the last hold ([`release`]); the job holds its own.
pub(crate) async fn submit<T, F>(factory: Arc<ExecutionContextFactory>, job: F) -> Result<T, CoreError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, CoreError> + Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    let submitted = factory
        .request_pool()
        .try_submit_then(Arc::clone(&factory), job, move |result| {
            let _ = tx.send(result);
        });
    release(factory);
    submitted?;
    rx.await.map_err(|_| CoreError::new("request worker stopped"))?
}

/// Let go of `factory` from async code. When this is the last hold (the server stopped meanwhile), the factory, and
/// the database clients it closes, are dropped on a blocking thread rather than a runtime thread.
pub(crate) fn release(factory: Arc<ExecutionContextFactory>) {
    if let Some(last) = Arc::into_inner(factory) {
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn_blocking(move || drop(last));
            }
            Err(_) => drop(last),
        }
    }
}

/// [`on_pool`] with authorization off for the whole job, for SFTP: its calls run as no user, and the `SSH_SFTP`
/// permission checked at login is the gate; and for the edge link's own bookkeeping.
#[cfg(any(feature = "ssh", feature = "inbound-smtp", feature = "edge"))]
pub(crate) async fn on_pool_without_authz<T, F>(factory: &Weak<ExecutionContextFactory>, job: F) -> Result<T, CoreError>
where
    T: Send + 'static,
    F: FnOnce(&ExecutionContextFactory) -> Result<T, CoreError> + Send + 'static,
{
    on_pool(factory, move |factory| {
        let _authz =
            moqui_core::context::ExecutionContext::with_current(moqui_core::context::ExecutionContext::disable_authz);
        job(factory)
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use moqui_core::context::ExecutionContextFactory;

    /// The job's answer arrives by the pool's completion hook: with the runtime's one blocking thread taken, it still
    /// completes, where a blocking task waiting on the reply would never start.
    #[test]
    fn a_job_completes_with_every_blocking_thread_taken() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        let context = ExecutionContextFactory::test_context();
        let factory = Arc::clone(context.factory());
        factory.request_pool().start(1, 4).unwrap();
        let (release, held) = std::sync::mpsc::channel::<()>();
        runtime.block_on(async {
            let blocking = tokio::task::spawn_blocking(move || {
                let _ = held.recv();
            });
            let answer = tokio::time::timeout(Duration::from_secs(5), super::submit(Arc::clone(&factory), || Ok(42)))
                .await
                .expect("the job did not complete with no blocking thread free");
            assert_eq!(answer.unwrap(), 42);
            let weak = Arc::downgrade(&factory);
            let answer = super::on_pool(&weak, |_| Ok("weak")).await;
            assert_eq!(answer.unwrap(), "weak");
            release.send(()).unwrap();
            blocking.await.unwrap();
        });
    }
}
