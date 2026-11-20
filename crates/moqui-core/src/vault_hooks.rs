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

//! Where the framework reaches the vault server (crates/moqui-vault-server, plans/Vault-Instances.md §3.7): that crate
//! sits on top of this one and submits one [`VaultHooks`]. A host built without it has none: `ec.vault.leased` throws
//! that the vault is not built in, and nothing is leased, so [`refuse_leased`] passes every call.
//!
//! - [`leased`] is the only way a script reads a vault value: an operational item's value from the lease the thread
//!   runs under, by its binding (`ec.vault.leased(binding)`). There is no member that hands out a value at will.
//! - [`refuse_leased`] keeps a leased value out of a stored call: the service paths that store a call's parameters (an
//!   async call, its queued job when the queue is full, a job run, the outbox of a call's tx rules) refuse parameters
//!   that hold a value a live lease handed out (§3.7, "A leased value stays in the operation").

use std::collections::BTreeMap;
use std::sync::OnceLock;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

/// Submit the vault server's [`VaultHooks`]; moqui-vault-server does, once.
pub use inventory::submit;

/// [`VaultHooks::machine_run`]: an environment, the bindings, the operation the lease names, and the body run under it.
pub type MachineRun = fn(
    environment_id: &str,
    bindings: &[String],
    operation: &str,
    body: &mut dyn FnMut() -> Result<(), LibError>,
) -> Result<(), LibError>;

/// What the framework calls in the vault server.
pub struct VaultHooks {
    /// The value of the item bound to `binding` in the lease this thread runs under: the item's fields, or the one
    /// field named. Each use writes its event before the value is returned, and no value comes back when that event
    /// cannot be written.
    pub leased: fn(binding: &str, field: Option<&str>) -> Result<Value, LibError>,
    /// Whether `text` holds a secret value a live lease handed out to the operation on this thread.
    pub holds_leased: fn(text: &str) -> bool,
    /// Runs `body` under a lease of `bindings` opened from the machine access of environment `environment_id` (§3.8),
    /// approved by "machine", which ends when `body` returns: an unattended operation of that environment, such as a CI
    /// run's call to its runner host. Refused when the environment holds no machine access to each binding.
    pub machine_run: MachineRun,
}
inventory::collect!(VaultHooks);

/// The vault server's hooks, when this binary links it.
#[must_use]
pub fn get() -> Option<&'static VaultHooks> {
    static HOOKS: OnceLock<Option<&'static VaultHooks>> = OnceLock::new();
    *HOOKS.get_or_init(|| {
        let mut entries = inventory::iter::<VaultHooks>.into_iter();
        let first = entries.next();
        if entries.next().is_some() {
            tracing::error!("more than one vault server submitted VaultHooks; using the first");
        }
        first
    })
}

/// `ec.vault.leased(binding, field)`: the value of the item bound to `binding` in the thread's lease.
///
/// # Errors
///
/// When the binary has no vault server, the thread runs under no lease, the lease names no such item, it has ended, or
/// the use's event could not be written.
pub fn leased(binding: &str, field: Option<&str>) -> Result<Value, LibError> {
    match get() {
        Some(hooks) => (hooks.leased)(binding, field),
        None => Err(LibError::new(
            "vault is not built in: this binary has no vault server (the vault feature)",
        )),
    }
}

/// Runs `body` under a machine-access lease of `bindings` for environment `environment_id`; inside it, [`leased`]
/// reads their values. The lease ends when `body` returns.
///
/// # Errors
///
/// When the binary has no vault server, the environment has no machine access to a binding, or `body` fails.
pub fn machine_run<T>(
    environment_id: &str,
    bindings: &[String],
    operation: &str,
    body: impl FnOnce() -> Result<T, LibError>,
) -> Result<T, LibError> {
    let Some(hooks) = get() else {
        return Err(LibError::new(
            "vault is not built in: this binary has no vault server (the vault feature)",
        ));
    };
    let mut body = Some(body);
    let mut answer = None;
    (hooks.machine_run)(environment_id, bindings, operation, &mut || {
        let body = body.take().ok_or_else(|| LibError::new("a machine-access body runs once"))?;
        answer = Some(body());
        Ok(())
    })?;
    answer.unwrap_or_else(|| Err(LibError::new("the machine-access lease ran nothing")))
}

/// Refuses `parameters` of a call about to be stored (queued, scheduled, kept in the outbox) when any value in them, at
/// any depth, holds a value a live lease handed out. `what` names the call in the refusal, never the value.
///
/// # Errors
///
/// When a leased value is found.
pub fn refuse_leased(what: &str, parameters: &BTreeMap<String, Value>) -> Result<(), LibError> {
    let Some(hooks) = get() else {
        return Ok(());
    };
    for (name, value) in parameters {
        if (hooks.holds_leased)(name) || holds(hooks, value) {
            return Err(LibError::new(format!(
                "{what}: the parameter {name} holds a value leased from a vault, which never goes into a queued or \
                 scheduled call (plans/Vault-Instances.md §3.7)"
            )));
        }
    }
    Ok(())
}

fn holds(hooks: &VaultHooks, value: &Value) -> bool {
    match value {
        Value::String(text) => (hooks.holds_leased)(text),
        Value::List(items) => items.iter().any(|item| holds(hooks, item)),
        Value::Map(map) => map
            .iter()
            .any(|(key, item)| (hooks.holds_leased)(key) || holds(hooks, item)),
        Value::Bytes(bytes) => (hooks.holds_leased)(&String::from_utf8_lossy(bytes)),
        // numbers, dates and host objects (an entity value's own fields are not a call's parameters to store)
        _ => false,
    }
}
