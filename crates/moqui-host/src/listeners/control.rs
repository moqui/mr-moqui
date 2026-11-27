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
//! service's thread.

use std::sync::{mpsc::RecvTimeoutError, Arc, Weak};
use std::time::Duration;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::listeners::{ListenerControl, ListenerInfo, Outcome, ReconcileRequest};

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

/// Make `registry` `factory`'s listener control (`moqui_core::listeners::control`).
pub(crate) fn install_control(factory: &ExecutionContextFactory, registry: &Registry) {
    moqui_core::listeners::install(
        factory,
        Arc::new(HostControl {
            registry: Arc::downgrade(&registry.0),
        }),
    );
}
