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

//! The reconcile job (plans/Vault-Instances.md §4.1): for each instance that has a container, what its host says of
//! it (its state, health and image) is written to the row beside the desired state. It changes nothing on a host and
//! no desired state: a difference is for an operator, and for Monitor's alerts (M4). A host reached with a client key,
//! whose key is a vault item, is observed only while an action runs under a lease, until the manager link (M2) reports
//! each instance's state. Each instance is observed on its own: a host that does not answer is marked unreachable and
//! the others go on.

use moqui_core::entity::EntityFacade;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use crate::common::{connect_host, field, find, host_needs_key, APP_INSTANCE};
use crate::lifecycle::{host_of, observe, unreachable};

/// What a reconcile pass did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Reconciled {
    pub observed: usize,
    pub unreachable: usize,
    /// Instances on a host whose client key is a vault item, left as they were.
    pub skipped: usize,
}

/// One pass over the instances that have a container.
pub fn reconcile(facade: &EntityFacade) -> Result<Reconciled, LibError> {
    let instances = find(facade, APP_INSTANCE)?.list()?.into_values();
    let mut done = Reconciled::default();
    for row in instances {
        let id = field(&row, "appInstanceId");
        let container = field(&row, "instanceUuid");
        if container.is_empty() {
            continue;
        }
        let host = match host_of(facade, &row) {
            Ok(host) => host,
            Err(err) => {
                tracing::error!("Instances: AppInstance {id} was not observed: {err}");
                continue;
            }
        };
        if host_needs_key(&host) {
            done.skipped += 1;
            continue;
        }
        let seen = connect_host(&host).and_then(|client| client.inspect(&container));
        let written = match seen {
            Ok(info) => {
                done.observed += 1;
                observe(facade, &id, info.as_ref())
            }
            Err(err) => {
                tracing::warn!("Instances: the host of AppInstance {id} did not answer: {err}");
                done.unreachable += 1;
                unreachable(facade, &id)
            }
        };
        if let Err(err) = written {
            tracing::error!("Instances: what was seen of AppInstance {id} was not written: {err}");
        }
    }
    Ok(done)
}

/// The pass's counts as a service result.
pub(crate) fn counts(done: Reconciled) -> [(&'static str, Value); 3] {
    let count = |n: usize| Value::I64(i64::try_from(n).unwrap_or(i64::MAX));
    [
        ("observedCount", count(done.observed)),
        ("unreachableCount", count(done.unreachable)),
        ("skippedCount", count(done.skipped)),
    ]
}
