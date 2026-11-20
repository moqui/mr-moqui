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

//! Managed instances (plans/Vault-Instances.md §4): the services of `org.moqui.dev.InstanceServices` that are Rust,
//! instance kinds, provisioning, the reconcile job, deploys, rotation and the manager's transit. Core reaches this crate
//! only through the [`moqui_core::extension`] entry below, with the feature `instances`.
//!
//! M1 (§6): instances on one driver, the container driver CI's runs use too ([`moqui_lib::docker`]).
//!
//! - [`database`]: a database kind's `create#Database`, its undo and `check#Database`: a one-shot client on the server's
//!   network, or verified TLS to a database host.
//! - [`kinds`]: what a kind's container is made of (its templates, its secrets as files, never in its environment,
//!   labels or command).
//! - [`plan`]: what a provision makes, dependencies first, the vault items its lease needs and where each goes (§4.3,
//!   the preview).
//! - [`provision`]: the provision under a lease or the environment's machine access: the credentials it makes are
//!   deposited (no lease needed), the database and user made with the admin login, then the volumes, network and
//!   container with secrets as files; a provision that opens items and has neither is refused, and one that fails is
//!   undone.
//! - [`lifecycle`]: start, stop, restart, remove and check, through the host type's services by name (the driver
//!   contract of `InstanceHostType`), each under a lease when its host's key is a vault item.
//! - [`actions`]: `InstanceAction` rows, and an action asked for from a screen: run now, or a lease asked for and the
//!   action run once it is approved.
//! - [`reconcile`]: the job that writes what each host says of its instances and changes nothing else (§4.1).
//! - [`deploy`]: deploys to an environment's instances (M3, §4.6): the gate, the lease its approval opens, the plan
//!   with versions before and after, a backup first, the roll one instance at a time (drained over the link when a
//!   cluster member), the annotations, and rollback to the recorded versions, refused across a schema change.
//! - [`link`]: the manager's rules of the manager link (M2): pairing once by a token or code, the approvers pinned at
//!   pairing, who may call an instance, and the bootstrap bundle held in memory until the instance takes it.
//! - [`services`]: the Rust services, and the INSTANCE_ADMIN check each makes.
//! - [`ssh`]: SSH, the other transport: the link's operator commands as `moqui operator` over SSH, the key from a lease.
//!
//! M4 (§6): Monitor for managed instances.
//!
//! - [`monitor`]: the identity a provision gives an instance (its id, environment, ingest key and OTLP address), its
//!   scrape target, and what the shipped instance alert rules read (gauges and counters).
//! - [`fleet`]: the fleet screen's rows.
//! - [`recovery`]: Monitor's `instance` recovery adapter, through the manager's own actions.
//!
//! M5 (§6): rotation.
//!
//! - [`rotate`]: a managed credential changed at its source, put on each instance that uses it and checked, then
//!   written as the item's new revision with the one before kept in its history; the old value put back when a step
//!   fails. Rotation ages ([`rotate::due`]), the job at the policy age, the incident list and a member removal's offer.

use std::collections::BTreeMap;

use moqui_core::extension::RustServiceFn;

pub mod actions;
mod common;
pub mod database;
pub mod deploy;
pub mod fleet;
pub mod kinds;
pub mod lifecycle;
pub mod link;
pub mod monitor;
pub mod plan;
pub mod provision;
pub mod reconcile;
pub mod recovery;
pub mod rotate;
pub mod services;
pub mod ssh;

moqui_core::extension::submit! {
    moqui_core::extension::Extension {
        features: &["instances"],
        register_services,
        // the gauges the shipped instance alert rules read (§4.8)
        after_boot: monitor::start,
        // links held before a restart are unknown until they link again (§4.8)
        after_cluster_start: monitor::after_cluster_start,
        ..moqui_core::extension::Extension::EMPTY
    }
}

fn register_services(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    // a member's removal from an operational vault offers to rotate its items at their source (§3.9, M5)
    moqui_vault_server::members::install_rotation_offer(rotate::due::offer);
    services::register(map)?;
    rotate::services::register(map)
}

#[cfg(test)]
mod tests;
