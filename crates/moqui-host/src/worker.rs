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

//! Run a blocking framework job on a request worker from async code (the SSH server and the SMTP listener), without
//! blocking the async runtime. The tasks hold the factory weakly, so a server that stopped is not kept alive by them, and
//! its database clients are never dropped on a runtime thread.

use std::sync::{Arc, Weak};

use moqui_core::context::{Error as CoreError, ExecutionContext, ExecutionContextFactory};

/// Run `job` on a request worker, in a fresh context with no user and authorization on, and wait without blocking the
/// async runtime. A job that logs a user in runs as that user; the login calls turn authorization off only while they
/// read the user tables.
pub(crate) async fn on_pool<T, F>(factory: &Weak<ExecutionContextFactory>, job: F) -> Result<T, CoreError>
where
    T: Send + 'static,
    F: FnOnce(&ExecutionContextFactory) -> Result<T, CoreError> + Send + 'static,
{
    let factory = Weak::clone(factory);
    tokio::task::spawn_blocking(move || {
        let rx = {
            let factory = factory
                .upgrade()
                .ok_or_else(|| CoreError::new("the server is stopping"))?;
            let owner = Arc::clone(&factory);
            let inner = Arc::clone(&factory);
            factory.request_pool().try_submit(owner, move || job(&inner))?
        };
        rx.recv().map_err(|_| CoreError::new("request worker stopped"))?
    })
    .await
    .map_err(|_| CoreError::new("request worker panicked"))?
}

/// [`on_pool`] with authorization off for the whole job, for SFTP: its calls run as no user, and the `SSH_SFTP`
/// permission checked at login is the gate.
pub(crate) async fn on_pool_without_authz<T, F>(factory: &Weak<ExecutionContextFactory>, job: F) -> Result<T, CoreError>
where
    T: Send + 'static,
    F: FnOnce(&ExecutionContextFactory) -> Result<T, CoreError> + Send + 'static,
{
    on_pool(factory, move |factory| {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        job(factory)
    })
    .await
}
