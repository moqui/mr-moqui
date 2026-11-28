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

//! The registry behind core's `moqui_core::listeners` slot (plans/Runtime-Config.md §8.1), so the services in core and
//! devkit read and reconcile the listeners without depending on the host. A service runs with an ExecutionContext
//! entered, so a reconcile asked here only enqueues to the worker and waits for its answer; it never runs a pass on the
//! service's thread. The conf's `listeners` group hook (§3.2, §8.2) only enqueues too: a conf reload or a setting set
//! while running asks the worker for a pass and goes on.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc::RecvTimeoutError, Arc, Mutex, Weak};
use std::time::Duration;

use moqui_core::context::conf_reload::{self, HookCause};
use moqui_core::context::conf_schema::Group;
use moqui_core::context::ExecutionContextFactory;
use moqui_core::listeners::{Cause, ListenerControl, ListenerInfo, Outcome, ReconcileRequest};
use moqui_lib::sync::lock;

use super::{Inner, Registry};

/// What a request to a registry that has ended answers.
const ENDED: &str = "the listener registry has stopped";

/// [`ListenerControl`] over a registry. It holds the registry weakly: the factory's slot must not keep the listeners,
/// which the process stop ends, nor make a cycle with the factory the registry holds.
pub(crate) struct HostControl {
    registry: Weak<Inner>,
}

impl HostControl {
    fn registry(&self) -> Option<Registry> {
        self.registry.upgrade().map(Registry)
    }
}

impl ListenerControl for HostControl {
    fn listeners(&self) -> Vec<ListenerInfo> {
        self.registry().map(|registry| registry.snapshot()).unwrap_or_default()
    }

    fn reconcile(&self, request: ReconcileRequest, wait: Duration) -> Result<Vec<Outcome>, String> {
        let answer = self
            .registry()
            .ok_or_else(|| ENDED.to_string())?
            .request(request, Duration::ZERO);
        match answer.recv_timeout(wait) {
            Ok(outcomes) => outcomes,
            Err(RecvTimeoutError::Timeout) => Err(format!(
                "the listeners did not finish within {} s; the change goes on",
                wait.as_secs()
            )),
            Err(RecvTimeoutError::Disconnected) => Err(ENDED.to_string()),
        }
    }

    fn reconcile_later(&self, request: ReconcileRequest, delay: Duration) {
        if let Some(registry) = self.registry() {
            // nobody waits for the answer: the pass records each row, and its receiver is dropped here
            drop(registry.request(request, delay));
        }
    }
}

/// Make `registry` `factory`'s listener control (`moqui_core::listeners::control`), and the registry its conf hook
/// asks for a pass.
pub(crate) fn install_control(factory: &ExecutionContextFactory, registry: &Registry) {
    moqui_core::listeners::install(
        factory,
        Arc::new(HostControl {
            registry: Arc::downgrade(&registry.0),
        }),
    );
    listen_to_conf(factory, registry);
}

/// The registry a factory's conf hook asks for a pass, held weakly as the control holds it, and whether the hook is
/// registered: once per factory, however many registries are installed on it in turn.
#[derive(Default)]
struct ConfHook {
    registered: AtomicBool,
    registry: Mutex<Weak<Inner>>,
}

/// After a conf reload or a set that reached the `listeners` group, queue a reconcile of `registry`, its cause
/// [`Cause::ConfReload`] or [`Cause::SetValue`] with the selectors set. The hook runs on the thread of the reload or the
/// set, which may hold a transaction or an entered ExecutionContext, so it never reconciles there: the worker runs the
/// pass, merging the requests that wait.
fn listen_to_conf(factory: &ExecutionContextFactory, registry: &Registry) {
    let hook = factory.extension(|_| ConfHook::default());
    *lock(&hook.registry) = Arc::downgrade(&registry.0);
    if hook.registered.swap(true, Ordering::SeqCst) {
        return;
    }
    conf_reload::on_apply_with_cause(factory, Group::Listeners, request_reconcile);
}

/// The `listeners` group hook: a pass for each of `causes` queued on the worker of `factory`'s registry, when one is
/// installed and running. A caller that ends connections now applies its settings holding this hook
/// (`conf_reload::set_many_holding`) and asks for the one pass itself, with `end_now`; a stop already draining is ended
/// at once whatever the order ([`super::worker::submit`]).
fn request_reconcile(factory: &ExecutionContextFactory, causes: &[HookCause]) {
    let Some(hook) = factory.extension_if_present::<ConfHook>() else {
        return;
    };
    let Some(registry) = lock(&hook.registry).upgrade().map(Registry) else {
        return;
    };
    for cause in causes {
        let cause = match cause {
            HookCause::Reload => Cause::ConfReload,
            HookCause::Set(selectors) => Cause::SetValue(selectors.join(", ")),
        };
        // nobody waits for the answer: the pass records each listener's row
        drop(registry.request(ReconcileRequest::new(cause), Duration::ZERO));
    }
}
