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

//! The system scope (Commerce F72): `ec.artifactExecution.runAsSystem(body)` and its Rust form [`as_system`], which run
//! work with no user and authorization off in a fresh execution context sharing the caller's transaction, and the rule
//! that keeps it to a service's own code (docs/security/Attack-Surface.md, "The system scope").

use crate::context::{Error, ErrorKind, ExecutionContext};

/// Run `work` as the system: in a fresh context of its own, entered on this thread, with no user and authorization off.
/// It shares the caller's transaction (the transaction is the thread's), effective time and agent origin, but not the
/// caller's user, visit, authorization or artifact stack. The caller's context comes back when `work` returns, throws or
/// panics (the guards drop in reverse order), and what `work` added to the messages and errors is moved to the caller's.
/// Rust code calls this directly; a script reaches it only through `ec.artifactExecution.runAsSystem(body)`, which first
/// requires [`system_scope_allowed`].
///
/// # Errors
///
/// Returns an error when no context is entered.
pub fn as_system<T>(work: impl FnOnce() -> T) -> Result<T, Error> {
    let (factory, effective_time, agent_origin) = ExecutionContext::with_current(|ec| {
        (
            std::sync::Arc::clone(ec.factory()),
            ec.effective_time(),
            ec.agent_origin(),
        )
    })
    .ok_or_else(|| Error::new("runAsSystem needs an execution context"))?;
    let system = factory.new_context();
    system.set_effective_time(effective_time);
    system.set_agent_origin(agent_origin);
    let result = {
        let _entered = system.enter();
        let _authz = system.disable_authz();
        work()
    };
    let held = std::mem::take(&mut *system.message());
    ExecutionContext::with_current(|ec| ec.message().absorb(held));
    Ok(result)
}

/// Whether a script may call `runAsSystem` here: only from a service's own code (the top artifact frame is a
/// service's, with no screen render begun since), never from a screen's widget expressions, actions or transitions, a
/// REST resource or a web script outside a service. A remote or REST caller reaches it only through a service whose
/// own code calls it.
///
/// # Errors
///
/// Returns an error naming the rule when the call is refused.
pub(crate) fn system_scope_allowed() -> Result<(), Error> {
    if ExecutionContext::with_current(ExecutionContext::in_service_body).unwrap_or(false) {
        return Ok(());
    }
    Err(Error::of_kind(
        ErrorKind::Unauthorized,
        "runAsSystem is only available to a service's own code, not to a screen, a widget expression or a REST resource",
    ))
}

#[cfg(test)]
#[path = "system_tests.rs"]
mod tests;
