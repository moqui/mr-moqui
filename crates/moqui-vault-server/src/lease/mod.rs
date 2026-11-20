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

//! Leases (plans/Vault-Instances.md §3.7): operational items used on the server for one operation.
//!
//! 1. An operation asks ([`LeaseRegistry::ask`]): the server makes an ephemeral X25519 key pair, keeps its private half
//!    in this cluster member's memory only, and signs the lease with its lease key ([`key`]): the lease id, the vault
//!    and key version, each item's id, type, binding and name, the operation, the cluster member, the time limit (at
//!    most `vault@lease-max-hours`, 4) and the lease's public key. The row is written with its `VeLeaseRequested` event
//!    and the vault's approvers are notified (topic `VaultLeaseWaiting`).
//! 2. A member with the approve role approves in their client (`approve#VaultLease`, any cluster member, a recent
//!    sign-in): the sealed item keys, the approver's signature and the `VeLeaseApproved` event in one transaction; at
//!    its commit the `vault-lease` cluster topic wakes the member that holds the lease, which also polls the row every
//!    two seconds in case a message is lost.
//! 3. The operation reads a value by binding ([`LeaseRegistry::leased`], `ec.vault.leased` in a script run under the
//!    lease, [`run_under`]): the member opens the sealed item keys with the lease's private key, checks the item row
//!    is the one the lease names, writes a `VeLeaseUsed` event in its own transaction, checks the lease is still held,
//!    and only then decrypts and returns the value; when the event cannot be written nothing is returned (§3.13, fail
//!    closed).
//! 4. The lease ends at the operation's end ([`LeaseRegistry::end`]), at its time limit (a timer sweeps them), on
//!    revoke and when the server stops or seals: its keys are dropped, which zeroes them, and nothing stored can open
//!    the sealed keys again. A restart empties the registry, so a use after it is refused with "lease ended; ask
//!    again", and once the cluster has started and named this member ([`after_cluster_start`]) its leases left open are
//!    marked ended ([`end_left_open`]), never another member's.
//!
//! A leased value never goes into a stored call: the registry keeps the secret fields it handed out, with the thread
//! they went to, until the lease ends, and the framework's stored-call paths refuse parameters that hold one on that
//! thread (`moqui_core::vault_hooks::refuse_leased`). The thread's lease ([`run_under`]) is read by every script on the
//! thread under it, not only the service the operation called: a limit docs/security/Vaults.md states.

pub(crate) mod key;
pub(crate) mod services;

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::transaction::TransactionAttribute;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{Value, ValueMap};
use moqui_lib::sync::{lock, wait_timeout};
use moqui_vault::hpke;
use moqui_vault::lease::{self as vault_lease, Approval, Lease, LeaseItem, SealedItemKey};
use moqui_vault::rng::OsRng;
use moqui_vault::text::{hex, unhex};
use moqui_vault::vault::ItemRef;
use moqui_vault::Key32;
use serde_json::{json, Value as Json};
use zeroize::Zeroizing;

use crate::common::{
    authz_off, fail, field, find, int_field, now, now_millis, refused, vault_row, ITEM, LEASE, MEMBER, OPERATIONAL,
    PERSONAL_KEY,
};
use crate::events::{self, Event};

pub(crate) const REQUESTED: &str = "VlsRequested";
pub(crate) const OPEN: &str = "VlsOpen";
pub(crate) const ENDED: &str = "VlsEnded";
pub(crate) const REVOKED: &str = "VlsRevoked";

/// How long a waiting operation sleeps before it reads the lease's row again, when no cluster message woke it.
const POLL: Duration = Duration::from_secs(2);
/// How often the registry drops the leases past their time limit and marks their rows ended.
const SWEEP: Duration = Duration::from_secs(30);
/// The shortest secret the stored-call guard looks for: a shorter one (a PIN) would match ordinary text, so it is
/// not looked for.
const MIN_SECRET: usize = 8;
/// The field names that hold a secret in an item whose type registers no fields (and so marks none masked).
const SECRET_NAMES: &[&str] = &["password", "passphrase", "secret", "key", "token", "pin"];

/// The refusal of a lease this member does not hold: ended, revoked, timed out, or held before a restart.
fn ended() -> LibError {
    fail("The lease ended; ask again (reasonCode leaseEnded)")
}

/// A lease this member holds: the signed statement, its private key, the item keys once opened, each secret value
/// handed out with the thread it went to (for the stored-call guard), and the environment a machine-access grant is
/// for, as asked (never read back from the row). Every key is a zeroizing type, dropped with the lease.
struct Held {
    lease: Lease,
    secret: hpke::SecretKey,
    item_keys: Vec<(String, Key32)>,
    handed: Vec<(ThreadId, Zeroizing<String>)>,
    requested_by: String,
    environment_id: Option<String>,
}

/// The leases this cluster member holds, by id: in memory only, so a restart empties it. One per factory
/// ([`registry`]), named by the member's cluster id once the cluster has started ([`after_cluster_start`]); the tests
/// make more, as two cluster members.
pub struct LeaseRegistry {
    member_id: Mutex<String>,
    held: Mutex<HashMap<String, Held>>,
    changed: Condvar,
}

impl std::fmt::Debug for LeaseRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LeaseRegistry")
            .field("member_id", &self.member_id())
            .field("held", &lock(&self.held).len())
            .finish()
    }
}

/// What an operation asks a lease for.
#[derive(Debug, Clone, Default)]
pub struct Ask<'a> {
    pub vault_id: &'a str,
    /// The items, by binding (`AppInstance:<id>:dbPassword`).
    pub bindings: &'a [String],
    /// What the lease is for, shown to the approver (`deploy run 1234`).
    pub operation: &'a str,
    /// The time limit asked for; none, or more than `vault@lease-max-hours`, is that setting.
    pub limit: Option<Duration>,
    /// The environment of a machine-access grant (§3.8), whose approval the server seals for it.
    pub environment_id: Option<&'a str>,
}

/// This factory's registry, made on first use, under the cluster member's id (`local` until the cluster starts, when
/// [`after_cluster_start`] names it by the member's id).
pub fn registry(factory: &ExecutionContextFactory) -> Arc<LeaseRegistry> {
    factory.extension(|factory| LeaseRegistry::new(&member_id(factory)))
}

/// The id a lease names its cluster member by.
pub(crate) fn member_id(factory: &ExecutionContextFactory) -> String {
    moqui_core::cluster::instance_id(factory).unwrap_or_else(|| "local".to_string())
}

/// The longest lease, from `vault@lease-max-hours` (4 when unset), never past the vault crate's 4 hours, which a
/// client refuses.
fn max_lease() -> Result<Duration, LibError> {
    let hours: f64 = moqui_core::context::conf_env::setting("vault@lease-max-hours")
        .parse()
        .unwrap_or(4.0);
    let max = Duration::from_millis(u64::try_from(vault_lease::MAX_LEASE_MS).unwrap_or(0));
    let asked = Duration::try_from_secs_f64(hours * 3600.0)
        .ok()
        .filter(|asked| !asked.is_zero())
        .ok_or_else(|| {
            fail(format!(
                "vault@lease-max-hours is {hours}: a number of hours above 0, at most 4"
            ))
        })?;
    Ok(asked.min(max))
}

/// Whether text has a control character (a newline, an escape), which could forge lines of an approver's prompt.
pub(crate) fn has_control(text: &str) -> bool {
    text.chars().any(char::is_control)
}

fn millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

/// A new lease id: random, since it is in the signed statement before the row exists.
fn new_lease_id() -> String {
    let mut bytes = [0_u8; 12];
    moqui_vault::rng::Rng::fill(&mut OsRng, &mut bytes);
    format!("lease-{}", hex(&bytes))
}

impl LeaseRegistry {
    pub fn new(member_id: &str) -> LeaseRegistry {
        LeaseRegistry {
            member_id: Mutex::new(member_id.to_string()),
            held: Mutex::new(HashMap::new()),
            changed: Condvar::new(),
        }
    }

    pub fn member_id(&self) -> String {
        lock(&self.member_id).clone()
    }

    /// Names the registry by the member's cluster id, which the cluster's start set after the registry was made.
    pub fn rekey(&self, member_id: &str) {
        member_id.clone_into(&mut lock(&self.member_id));
    }

    /// Whether this member holds the lease's key now.
    pub fn holds(&self, lease_id: &str) -> bool {
        self.sweep();
        lock(&self.held).contains_key(lease_id)
    }

    /// Wakes the operations waiting for a lease to open: a cluster message said one was approved or revoked.
    pub fn wake(&self) {
        self.changed.notify_all();
    }

    /// Forgets a lease without touching its row: it was revoked on another member, or the server is stopping.
    pub fn forget(&self, lease_id: &str) {
        lock(&self.held).remove(lease_id);
        self.changed.notify_all();
    }

    /// Forgets every lease: the server is stopping or sealing.
    pub fn clear(&self) {
        lock(&self.held).clear();
        self.changed.notify_all();
    }

    /// Drops the leases past their time limit; their ids.
    fn sweep(&self) -> Vec<String> {
        let now = now_millis();
        let mut held = lock(&self.held);
        let expired: Vec<String> = held
            .iter()
            .filter(|(_, held)| held.lease.expires_at <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &expired {
            held.remove(id);
        }
        expired
    }

    /// Drops the leases past their time limit, which zeroes their keys, and marks their rows ended: the timer's work
    /// ([`after_cluster_start`]), so a lease no operation reads again still goes at its limit. A row that cannot be
    /// marked is logged.
    pub fn expire(&self, facade: &EntityFacade) {
        for lease_id in self.sweep() {
            if let Err(err) = mark_ended(facade, &lease_id, ENDED, None, "its time limit passed") {
                tracing::error!("Vault lease {lease_id} past its time limit was not marked ended: {err}");
            }
        }
    }

    /// Whether `text` holds a secret value a live lease of this member handed out to the operation on this thread: the
    /// guard of stored calls (`moqui_core::vault_hooks::refuse_leased`). A value another thread's operation read is
    /// that operation's to keep out.
    pub fn holds_value(&self, text: &str) -> bool {
        self.sweep();
        let thread = std::thread::current().id();
        let held = lock(&self.held);
        held.values()
            .flat_map(|held| held.handed.iter())
            .any(|(to, value)| *to == thread && text.contains(value.as_str()))
    }

    /// The signed statement of a lease held here.
    pub(crate) fn held_lease(&self, lease_id: &str) -> Option<Lease> {
        lock(&self.held).get(lease_id).map(|held| held.lease.clone())
    }

    /// The environment a machine-access grant's lease was asked for, as asked.
    /// The operation and vault of a lease this member holds, as its signed statement names them (never read back from
    /// the row's columns): what an operation checks before it runs under a lease it was handed (M1's instance actions).
    pub fn statement_of(&self, lease_id: &str) -> Option<(String, String)> {
        lock(&self.held)
            .get(lease_id)
            .map(|held| (held.lease.operation.clone(), held.lease.vault_id.clone()))
    }

    pub(crate) fn environment_of(&self, lease_id: &str) -> Option<String> {
        lock(&self.held)
            .get(lease_id)
            .and_then(|held| held.environment_id.clone())
    }

    /// Asks for a lease (§3.7, step 1): its items found by binding in the operational vault, its statement signed by
    /// the lease key, its row and `VeLeaseRequested` event written in the caller's transaction, and its private key
    /// kept here. The vault's approvers are told.
    pub fn ask(&self, facade: &EntityFacade, ask: &Ask) -> Result<Lease, LibError> {
        let _authz = authz_off();
        let tx = facade.transactions_for(LEASE)?;
        tx.run(TransactionAttribute::UseOrBegin, None, || self.ask_in_tx(facade, ask))
    }

    fn ask_in_tx(&self, facade: &EntityFacade, ask: &Ask) -> Result<Lease, LibError> {
        let (lease, secret) = self.statement(facade, ask)?;
        let requested_by = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
        let mut row = facade.make_value(LEASE)?;
        fill_statement(&mut row, &lease);
        row.set("statusId", Value::string(REQUESTED));
        if let Some(environment_id) = ask.environment_id {
            row.set("environmentId", Value::string(environment_id));
        }
        if !requested_by.is_empty() {
            row.set("requestedByUserId", Value::string(&requested_by));
        }
        row.set("requestedDate", now());
        row.create(facade)?;
        events::write(
            facade,
            Event {
                kind: "VeLeaseRequested",
                vault_id: Some(&lease.vault_id),
                lease_id: Some(&lease.lease_id),
                detail: Some(format!(
                    "{}: {} items, until {}",
                    lease.operation,
                    lease.items.len(),
                    lease.expires_at
                )),
                ..Event::default()
            },
        )?;
        services::notify_approvers(facade, &lease);
        self.keep(&lease, secret, Vec::new(), &requested_by, ask.environment_id);
        Ok(lease)
    }

    /// The signed statement of a new lease and its private key.
    fn statement(&self, facade: &EntityFacade, ask: &Ask) -> Result<(Lease, hpke::SecretKey), LibError> {
        let vault = vault_row(facade, ask.vault_id, false)?;
        if field(&vault, "vaultKindEnumId") != OPERATIONAL {
            return Err(fail("A lease is of an operational vault's items"));
        }
        if ask.bindings.is_empty() {
            return Err(fail("A lease names at least one item, by its binding"));
        }
        if ask.operation.trim().is_empty() || ask.operation.len() > 255 || has_control(ask.operation) {
            return Err(fail(
                "A lease names its operation, in at most 255 characters and with no control character",
            ));
        }
        let key_version = int_field(&vault, "keyVersion");
        let mut items = Vec::with_capacity(ask.bindings.len());
        for binding in ask.bindings {
            let row = find(facade, ITEM)?
                .eq("vaultId", Value::string(ask.vault_id))
                .eq("bindingKey", Value::string(binding))
                .one()?
                .ok_or_else(|| fail(format!("No item with the binding {binding} in vault {}", ask.vault_id)))?;
            if field(&row, "itemKeyWrapEnumId") != "VwVault" {
                return Err(fail(format!(
                    "The item {binding} was deposited by the server: a member opens the vault to take it in before a \
                     lease can name it"
                )));
            }
            if int_field(&row, "keyVersion") != key_version {
                return Err(fail(format!("The item {binding} is not at the vault's key version")));
            }
            if items.iter().any(|item: &LeaseItem| item.binding == *binding) {
                return Err(fail(format!("The binding {binding} is named twice")));
            }
            items.push(LeaseItem {
                item_id: field(&row, "vaultItemId"),
                item_type: field(&row, "vaultItemTypeId"),
                binding: binding.clone(),
                name: field(&row, "itemName"),
            });
        }
        let limit = ask.limit.unwrap_or(Duration::MAX).min(max_lease()?);
        let (secret, public) = vault_lease::new_lease_key(&mut OsRng).map_err(refused)?;
        let mut lease = Lease {
            lease_id: new_lease_id(),
            vault_id: ask.vault_id.to_string(),
            key_version: u64::try_from(key_version).map_err(|_| fail("a key version out of range"))?,
            items,
            operation: ask.operation.trim().to_string(),
            cluster_member: self.member_id(),
            expires_at: now_millis().saturating_add(millis(limit)),
            public,
            signature: Vec::new(),
        };
        lease.sign(&key::signing_key()?);
        Ok((lease, secret))
    }

    fn keep(
        &self,
        lease: &Lease,
        secret: hpke::SecretKey,
        item_keys: Vec<(String, Key32)>,
        requested_by: &str,
        environment_id: Option<&str>,
    ) {
        lock(&self.held).insert(
            lease.lease_id.clone(),
            Held {
                lease: lease.clone(),
                secret,
                item_keys,
                handed: Vec::new(),
                requested_by: requested_by.to_string(),
                environment_id: environment_id.map(str::to_string),
            },
        );
    }

    /// A lease opened at once with item keys the server already holds: machine access (§3.8), approved by "machine".
    /// Its row is written open, with its requested and approved events, in the caller's transaction.
    pub(crate) fn open_with_keys(
        &self,
        facade: &EntityFacade,
        ask: &Ask,
        open_key: &dyn Fn(&LeaseItem) -> Result<Key32, LibError>,
    ) -> Result<Lease, LibError> {
        let _authz = authz_off();
        let tx = facade.transactions_for(LEASE)?;
        tx.run(TransactionAttribute::UseOrBegin, None, || {
            self.open_in_tx(facade, ask, open_key)
        })
    }

    fn open_in_tx(
        &self,
        facade: &EntityFacade,
        ask: &Ask,
        open_key: &dyn Fn(&LeaseItem) -> Result<Key32, LibError>,
    ) -> Result<Lease, LibError> {
        let (lease, secret) = self.statement(facade, ask)?;
        let mut item_keys = Vec::with_capacity(lease.items.len());
        for item in &lease.items {
            item_keys.push((item.item_id.clone(), open_key(item)?));
        }
        let requested_by = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
        let mut row = facade.make_value(LEASE)?;
        fill_statement(&mut row, &lease);
        row.set("statusId", Value::string(OPEN));
        row.set("machineAccess", Value::string("Y"));
        if let Some(environment_id) = ask.environment_id {
            row.set("environmentId", Value::string(environment_id));
        }
        if !requested_by.is_empty() {
            row.set("requestedByUserId", Value::string(&requested_by));
        }
        row.set("requestedDate", now());
        row.set("approvedDate", now());
        row.create(facade)?;
        let environment = ask.environment_id.unwrap_or_default();
        for (kind, detail) in [
            (
                "VeLeaseRequested",
                format!("{}: {} items", lease.operation, lease.items.len()),
            ),
            (
                "VeLeaseApproved",
                format!("approved by machine access of environment {environment}"),
            ),
        ] {
            events::write(
                facade,
                Event {
                    kind,
                    vault_id: Some(&lease.vault_id),
                    lease_id: Some(&lease.lease_id),
                    actor: Some("machine"),
                    detail: Some(detail),
                    ..Event::default()
                },
            )?;
        }
        self.keep(&lease, secret, item_keys, &requested_by, ask.environment_id);
        Ok(lease)
    }

    /// Waits until the lease is approved and opened here, at most `timeout` (§3.7, step 2): woken by the cluster
    /// topic, and reading the row every two seconds in case a message was lost.
    pub fn wait_open(&self, facade: &EntityFacade, lease_id: &str, timeout: Duration) -> Result<(), LibError> {
        let until = Instant::now() + timeout;
        loop {
            if self.try_open(facade, lease_id)? {
                return Ok(());
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(fail(format!("The lease {lease_id} was not approved in time")));
            }
            let held = lock(&self.held);
            let _ = wait_timeout(&self.changed, held, left.min(POLL));
        }
    }

    /// Opens the lease here when its row says it was approved: true when its item keys are open. A lease not held
    /// here, past its time limit, revoked or ended is refused, and forgotten.
    fn try_open(&self, facade: &EntityFacade, lease_id: &str) -> Result<bool, LibError> {
        let _authz = authz_off();
        if !self.holds(lease_id) {
            return Err(ended());
        }
        if lock(&self.held)
            .get(lease_id)
            .is_some_and(|held| !held.item_keys.is_empty())
        {
            return Ok(true);
        }
        let row = find(facade, LEASE)?
            .eq("leaseId", Value::string(lease_id))
            .one()?
            .ok_or_else(ended)?;
        match field(&row, "statusId").as_str() {
            REQUESTED => Ok(false),
            OPEN => {
                self.open_approval(facade, lease_id, &row)?;
                Ok(true)
            }
            _ => {
                self.forget(lease_id);
                Err(ended())
            }
        }
    }

    /// Opens the approval on the row with the lease's private key: the approver's key is their active one and they
    /// hold the approve role at the lease's key version (checked again here, since the row could have been written by
    /// anyone with the database), then their signature over the lease this member signed (not the row's copy of it),
    /// then each item key.
    fn open_approval(&self, facade: &EntityFacade, lease_id: &str, row: &EntityValue) -> Result<(), LibError> {
        let approval = approval_of_row(row)?;
        // a recovery key is listed as a manager, never an approver, and no one signs in as it (§3.10)
        if moqui_vault::keyversion::recovery_key_of(&approval.approver_user_id).is_some() {
            return Err(fail("A recovery key does not approve a lease"));
        }
        let approver_key = find(facade, PERSONAL_KEY)?
            .eq("personalKeyId", Value::string(field(row, "approverKeyId")))
            .one()?
            .ok_or_else(|| fail("The lease's approver key is not found"))?;
        if field(&approver_key, "userId") != approval.approver_user_id
            || field(&approver_key, "statusId") != "VpkActive"
        {
            return Err(fail("The lease's approver key is not the approver's active key"));
        }
        let (vault_id, key_version) = {
            let held = lock(&self.held);
            let entry = held.get(lease_id).ok_or_else(ended)?;
            (entry.lease.vault_id.clone(), entry.lease.key_version)
        };
        let role = find(facade, MEMBER)?
            .eq("vaultId", Value::string(&vault_id))
            .eq("userId", Value::string(&approval.approver_user_id))
            .eq("keyVersion", Value::I64(i64::try_from(key_version).unwrap_or(-1)))
            .one()?
            .map(|member| field(&member, "roleEnumId"))
            .unwrap_or_default();
        if role != "VrApprove" {
            return Err(fail("The lease's approver does not hold the approve role in its vault"));
        }
        let verify = crate::personal::public_of(&approver_key)?.verify;
        let mut held = lock(&self.held);
        let Some(entry) = held.get_mut(lease_id) else {
            return Err(ended());
        };
        let keys = vault_lease::open_approval(&entry.lease, &entry.secret, &approval, &verify).map_err(refused)?;
        entry.item_keys = keys;
        Ok(())
    }

    /// The value of the item bound to `binding` in the lease (§3.7, step 3): the item's fields, or the one named. The
    /// item row must be the one the lease names; the `VeLeaseUsed` event is written and committed before the value is
    /// decrypted and returned, and when it cannot be written nothing is returned.
    pub fn leased(
        &self,
        facade: &EntityFacade,
        lease_id: &str,
        binding: &str,
        field_name: Option<&str>,
    ) -> Result<Value, LibError> {
        let _authz = authz_off();
        // the row's state each time: a revoke on another member ends the lease here too, message or none
        let row = find(facade, LEASE)?
            .eq("leaseId", Value::string(lease_id))
            .one()?
            .ok_or_else(ended)?;
        if int_field(&row, "expiresAt") <= now_millis() {
            self.forget(lease_id);
            mark_ended(facade, lease_id, ENDED, None, "its time limit passed")?;
            return Err(ended());
        }
        match field(&row, "statusId").as_str() {
            OPEN => {}
            REQUESTED if self.holds(lease_id) => {
                return Err(fail(format!("The lease {lease_id} is waiting for approval")));
            }
            _ => {
                self.forget(lease_id);
                return Err(ended());
            }
        }
        if !self.try_open(facade, lease_id)? {
            return Err(fail(format!("The lease {lease_id} is waiting for approval")));
        }
        let (item, item_key, requested_by) = {
            let held = lock(&self.held);
            let entry = held.get(lease_id).ok_or_else(ended)?;
            let item = entry
                .lease
                .items
                .iter()
                .find(|item| item.binding == binding)
                .cloned()
                .ok_or_else(|| fail(format!("The lease names no item bound to {binding}")))?;
            let key = entry
                .item_keys
                .iter()
                .find(|(id, _)| *id == item.item_id)
                .map(|(_, key)| key.clone())
                .ok_or_else(|| fail("The lease's approval lacks an item"))?;
            (item, key, entry.requested_by.clone())
        };
        let (vault_id, key_version) = {
            let held = lock(&self.held);
            let entry = held.get(lease_id).ok_or_else(ended)?;
            (entry.lease.vault_id.clone(), entry.lease.key_version)
        };
        let item_row = find(facade, ITEM)?
            .eq("vaultItemId", Value::string(&item.item_id))
            .one()?
            .ok_or_else(|| fail(format!("The item bound to {binding} is gone")))?;
        let same = field(&item_row, "vaultId") == vault_id
            && field(&item_row, "vaultItemTypeId") == item.item_type
            && field(&item_row, "bindingKey") == item.binding
            && field(&item_row, "itemName") == item.name
            && int_field(&item_row, "keyVersion") == i64::try_from(key_version).unwrap_or(-1);
        if !same {
            return Err(fail(format!(
                "The item bound to {binding} is not the item the lease names: ask again"
            )));
        }
        let actor = (!requested_by.is_empty()).then_some(requested_by.as_str());
        events::write_alone(
            facade,
            Event {
                kind: "VeLeaseUsed",
                vault_id: Some(&vault_id),
                item_id: Some(&item.item_id),
                lease_id: Some(lease_id),
                actor,
                detail: Some(format!("{binding} read")),
                ..Event::default()
            },
        )?;
        // revoked, timed out or stopped while the event was written: nothing comes back
        if !self.holds(lease_id) {
            return Err(ended());
        }
        let item_ref = ItemRef {
            vault_id: &vault_id,
            item_id: &item.item_id,
            key_version,
            label: Some(moqui_vault::names::ItemLabel {
                item_type: &item.item_type,
                binding: &item.binding,
                name: &item.name,
            }),
        };
        let sealed = unhex(&field(&item_row, "bodyCipher")).map_err(refused)?;
        let body = moqui_vault::vault::open_item_body(&item_key, item_ref, &sealed).map_err(refused)?;
        let parsed: Json =
            serde_json::from_slice(&body).map_err(|_| fail(format!("The item bound to {binding} is not JSON")))?;
        let Some(fields) = parsed.get("fields").and_then(Json::as_object) else {
            return Err(fail(format!("The item bound to {binding} has no fields")));
        };
        // the secret fields the guard looks for: the masked ones of the item's type, or by name when its type registers
        // none; a host, a user name or a port is not a secret, and matching it would refuse ordinary calls
        let masked = masked_fields(facade, &item.item_type);
        let thread = std::thread::current().id();
        let mut handed = Vec::new();
        for (name, value) in fields {
            let Some(text) = value.as_str() else { continue };
            let secret = match &masked {
                Some(masked) => masked.contains(name),
                None => secret_name(name),
            };
            if secret && text.chars().count() >= MIN_SECRET {
                handed.push((thread, Zeroizing::new(text.to_string())));
            }
        }
        let answer = match field_name {
            Some(name) => {
                let text = fields
                    .get(name)
                    .ok_or_else(|| fail(format!("The item bound to {binding} has no field {name}")))?;
                Value::string(text.as_str().map_or_else(|| text.to_string(), str::to_string))
            }
            None => {
                let map: ValueMap = fields
                    .iter()
                    .map(|(name, value)| (name.clone(), moqui_lib::json::from_json(value)))
                    .collect();
                Value::Map(map)
            }
        };
        if let Some(entry) = lock(&self.held).get_mut(lease_id) {
            entry.handed.extend(handed);
        }
        Ok(answer)
    }

    /// The opened item keys of a lease held here, for a machine-access grant to seal ([`crate::machine`]).
    pub(crate) fn item_keys(&self, lease_id: &str) -> Result<Vec<(String, Key32)>, LibError> {
        let held = lock(&self.held);
        let entry = held.get(lease_id).ok_or_else(ended)?;
        if entry.item_keys.is_empty() {
            return Err(fail(format!("The lease {lease_id} is not open")));
        }
        Ok(entry.item_keys.clone())
    }

    /// Ends a lease at its operation's end (§3.7, step 4): its keys dropped here, its row marked ended with its event.
    /// A lease already ended or revoked is left as it is.
    pub fn end(&self, facade: &EntityFacade, lease_id: &str) -> Result<(), LibError> {
        let requested_by = lock(&self.held)
            .remove(lease_id)
            .map(|held| held.requested_by.clone())
            .unwrap_or_default();
        self.changed.notify_all();
        let actor = (!requested_by.is_empty()).then_some(requested_by.as_str());
        mark_ended(facade, lease_id, ENDED, actor, "the operation ended")
    }
}

/// Whether a field of an item whose type registers no fields holds a secret, by its name: `password`, `dbPassword`,
/// `apiKey`, `token`, `secret`, `passphrase`, `pin`, in any case.
fn secret_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SECRET_NAMES.iter().any(|secret| lower.contains(secret))
}

/// The masked fields of an item type, none when the type registers no fields.
fn masked_fields(facade: &EntityFacade, item_type: &str) -> Option<Vec<String>> {
    let rows = find(facade, "moqui.security.vault.VaultItemTypeField")
        .ok()?
        .eq("vaultItemTypeId", Value::string(item_type))
        .list()
        .ok()?;
    if rows.is_empty() {
        return None;
    }
    Some(
        rows.iter()
            .filter(|row| field(row, "masked") == "Y")
            .map(|row| field(row, "fieldName"))
            .collect(),
    )
}

/// Marks a lease ended or revoked, with its event, unless it already is; in a transaction of its own when the caller
/// has none.
pub(crate) fn mark_ended(
    facade: &EntityFacade,
    lease_id: &str,
    status: &str,
    actor: Option<&str>,
    why: &str,
) -> Result<(), LibError> {
    let _authz = authz_off();
    let tx = facade.transactions_for(LEASE)?;
    tx.run(TransactionAttribute::UseOrBegin, None, || {
        let Some(mut row) = find(facade, LEASE)?
            .eq("leaseId", Value::string(lease_id))
            .for_update(true)
            .one()?
        else {
            return Ok(());
        };
        if ![REQUESTED, OPEN].contains(&field(&row, "statusId").as_str()) {
            return Ok(());
        }
        row.set("statusId", Value::string(status));
        row.set("endedDate", now());
        row.update(facade)?;
        let vault_id = field(&row, "vaultId");
        events::write(
            facade,
            Event {
                kind: if status == REVOKED {
                    "VeLeaseRevoked"
                } else {
                    "VeLeaseEnded"
                },
                vault_id: Some(&vault_id),
                lease_id: Some(lease_id),
                actor,
                detail: Some(why.to_string()),
                ..Event::default()
            },
        )
    })
}

/// At boot: the leases this member asked for before it stopped are ended, since their private keys went with it
/// (§3.7: "a restart of the cluster member ends its leases"). A row that fails is logged and the rest go on.
pub fn end_left_open(facade: &EntityFacade, member_id: &str) -> Result<usize, LibError> {
    let _authz = authz_off();
    let rows = find(facade, LEASE)?
        .eq("clusterMemberId", Value::string(member_id))
        .list()?;
    let mut count = 0;
    for row in rows.iter() {
        if ![REQUESTED, OPEN].contains(&field(row, "statusId").as_str()) {
            continue;
        }
        let lease_id = field(row, "leaseId");
        match mark_ended(facade, &lease_id, ENDED, None, "the server restarted") {
            Ok(()) => count += 1,
            Err(err) => tracing::error!("Vault lease {lease_id} was not marked ended at the restart: {err}"),
        }
    }
    Ok(count)
}

// ----- The statement and approval as rows and JSON -----

/// A lease's items as the row keeps them and the statement's JSON names them.
fn items_json(items: &[LeaseItem]) -> Json {
    Json::Array(
        items
            .iter()
            .map(|item| {
                json!({ "item_id": item.item_id, "item_type": item.item_type, "binding": item.binding, "name": item.name })
            })
            .collect(),
    )
}

/// The signed statement as the vault crate reads it (`ops::shapes::lease_of`): what the approver's client checks and
/// the dialog shows.
pub fn lease_json(lease: &Lease) -> Json {
    json!({
        "lease_id": lease.lease_id, "vault_id": lease.vault_id, "key_version": lease.key_version,
        "items": items_json(&lease.items), "operation": lease.operation, "cluster_member": lease.cluster_member,
        "expires_at": lease.expires_at, "public": hex(lease.public.as_bytes()), "signature": hex(&lease.signature),
    })
}

fn fill_statement(row: &mut EntityValue, lease: &Lease) {
    row.set("leaseId", Value::string(&lease.lease_id));
    row.set("vaultId", Value::string(&lease.vault_id));
    row.set("keyVersion", Value::I64(i64::try_from(lease.key_version).unwrap_or(0)));
    row.set("leaseItems", Value::string(items_json(&lease.items).to_string()));
    row.set("operation", Value::string(&lease.operation));
    row.set("clusterMemberId", Value::string(&lease.cluster_member));
    row.set("leasePublic", Value::string(hex(lease.public.as_bytes())));
    row.set("expiresAt", Value::I64(lease.expires_at));
    row.set("serverSignature", Value::string(hex(&lease.signature)));
}

/// The statement a lease row holds, as signed (the signature is checked by whoever relies on it).
pub(crate) fn lease_of_row(row: &EntityValue) -> Result<Lease, LibError> {
    let items: Json = serde_json::from_str(&field(row, "leaseItems")).map_err(|_| fail("a stored lease's items"))?;
    let shaped = json!({ "lease": {
        "lease_id": field(row, "leaseId"), "vault_id": field(row, "vaultId"), "key_version": int_field(row, "keyVersion"),
        "items": items, "operation": field(row, "operation"), "cluster_member": field(row, "clusterMemberId"),
        "expires_at": int_field(row, "expiresAt"), "public": field(row, "leasePublic"),
        "signature": field(row, "serverSignature"),
    }});
    moqui_vault::ops::shapes::lease_of(&shaped).map_err(refused)
}

/// An approval as the client's `lease_approve` answers it.
pub(crate) fn approval_of(given: &Json) -> Result<Approval, LibError> {
    let text = |name: &str| given[name].as_str().unwrap_or_default().to_string();
    let Some(rows) = given["sealed"].as_array() else {
        return Err(fail("An approval has its sealed item keys"));
    };
    let mut sealed = Vec::with_capacity(rows.len());
    for row in rows {
        let item_id = row["item_id"].as_str().unwrap_or_default().to_string();
        let bytes = unhex(row["sealed"].as_str().unwrap_or_default()).map_err(refused)?;
        if item_id.is_empty() || bytes.is_empty() || bytes.len() > 512 {
            return Err(fail("Each sealed item key has its item_id and sealed key"));
        }
        sealed.push(SealedItemKey { item_id, sealed: bytes });
    }
    Ok(Approval {
        lease_id: text("lease_id"),
        sealed,
        approver_user_id: text("approver_user_id"),
        signature: unhex(&text("signature")).map_err(refused)?,
    })
}

pub(crate) fn approval_json(approval: &Approval) -> Json {
    let sealed: Vec<Json> = approval
        .sealed
        .iter()
        .map(|s| json!({ "item_id": s.item_id, "sealed": hex(&s.sealed) }))
        .collect();
    json!({ "lease_id": approval.lease_id, "sealed": sealed, "approver_user_id": approval.approver_user_id,
            "signature": hex(&approval.signature) })
}

fn approval_of_row(row: &EntityValue) -> Result<Approval, LibError> {
    let sealed: Json =
        serde_json::from_str(&field(row, "sealedItemKeys")).map_err(|_| fail("The lease has no approval"))?;
    approval_of(&json!({
        "lease_id": field(row, "leaseId"), "sealed": sealed, "approver_user_id": field(row, "approvedByUserId"),
        "signature": field(row, "approvalSignature"),
    }))
}

// ----- The thread's lease, and the framework's hooks -----

thread_local! {
    /// The leases the operations on this thread run under, innermost last.
    static CURRENT: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Runs `body` under the lease: a kind's service it calls reads values from it by binding (`ec.vault.leased`).
pub fn run_under<T>(lease_id: &str, body: impl FnOnce() -> T) -> T {
    struct Pop;
    impl Drop for Pop {
        fn drop(&mut self) {
            CURRENT.with(|current| current.borrow_mut().pop());
        }
    }
    CURRENT.with(|current| current.borrow_mut().push(lease_id.to_string()));
    let _pop = Pop;
    body()
}

/// The lease this thread runs under, if any.
pub fn current_lease() -> Option<String> {
    CURRENT.with(|current| current.borrow().last().cloned())
}

/// `moqui_core::vault_hooks::leased`: the value from the thread's lease, through the entered factory's registry.
pub(crate) fn hook_leased(binding: &str, field_name: Option<&str>) -> Result<Value, LibError> {
    let lease_id = current_lease().ok_or_else(|| fail("This code runs under no vault lease"))?;
    let (factory, facade) =
        moqui_core::context::ExecutionContext::with_current(|ec| (Arc::clone(ec.factory()), ec.factory().entity()))
            .ok_or_else(|| fail("No execution context is entered"))?;
    let facade = facade.ok_or_else(|| fail("No entity facade is installed"))?;
    registry(&factory).leased(&facade, &lease_id, binding, field_name)
}

/// `moqui_core::vault_hooks::holds_leased`: whether the entered factory's registry handed out `text`.
pub(crate) fn hook_holds_leased(text: &str) -> bool {
    moqui_core::context::ExecutionContext::with_current(|ec| {
        ec.factory()
            .extension_if_present::<LeaseRegistry>()
            .is_some_and(|registry| registry.holds_value(text))
    })
    .unwrap_or(false)
}

/// The registry's cluster listener, kept with the factory: an approval or revoke on another member wakes or ends the
/// leases held here.
struct Listener {
    _subscription: moqui_core::notify::Subscription,
}

/// Once the cluster has started and named this member (`moqui_core::extension::cluster_started`): the registry takes
/// the member's id, the leases this member left open before a restart are ended (only its own: another member's are
/// its to end), the `vault-lease` topic is listened to, and a timer drops leases past their time limit.
pub(crate) fn after_cluster_start(factory: &ExecutionContextFactory) {
    let registry = registry(factory);
    registry.rekey(&member_id(factory));
    if let Some(facade) = factory.entity() {
        let ended = {
            let _authz = authz_off();
            end_left_open(&facade, &registry.member_id())
        };
        match ended {
            Ok(0) => {}
            Ok(count) => tracing::info!("Vault: {count} leases asked for before the restart were ended"),
            Err(err) => tracing::error!("Vault: the leases asked for before the restart were not ended: {err}"),
        }
    }
    let weak = Arc::downgrade(&registry);
    let listener: moqui_core::notify::Listener = Arc::new(move |message| {
        let Some(registry) = weak.upgrade() else {
            return;
        };
        let body = &message.body;
        if body["member"].as_str() == Some(registry.member_id().as_str()) && body["kind"].as_str() == Some("revoked") {
            if let Some(lease_id) = body["leaseId"].as_str() {
                registry.forget(lease_id);
            }
        }
        registry.wake();
    });
    match moqui_core::notify::subscribe(moqui_core::notify::VAULT_LEASE, listener) {
        Ok(subscription) => {
            factory.extension(move |_| Listener {
                _subscription: subscription,
            });
        }
        Err(err) => tracing::error!("Vault: the vault-lease topic was not subscribed: {}", err.message()),
    }
    start_sweeper(factory, &registry);
}

/// The timer that drops leases past their time limit and marks their rows ended, every [`SWEEP`], while the factory
/// and its registry live.
fn start_sweeper(factory: &ExecutionContextFactory, registry: &Arc<LeaseRegistry>) {
    let Some(facade) = factory.entity() else {
        return;
    };
    let weak = Arc::downgrade(registry);
    // the factory by its address: the sweep enters a context of its own on it while it is the running server's
    let address = std::ptr::from_ref(factory) as usize;
    let spawned = std::thread::Builder::new()
        .name("vault-lease-sweep".to_string())
        .spawn(move || loop {
            std::thread::sleep(SWEEP);
            let Some(registry) = weak.upgrade() else {
                return;
            };
            let running = ExecutionContextFactory::running().filter(|running| Arc::as_ptr(running) as usize == address);
            let context = running.as_ref().map(ExecutionContextFactory::new_context);
            let _entered = context.as_ref().map(moqui_core::context::ExecutionContext::enter);
            let swept = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| registry.expire(&facade)));
            if swept.is_err() {
                tracing::error!("Vault: the lease sweep failed; it runs again");
            }
        });
    if let Err(err) = spawned {
        tracing::error!("Vault: the lease sweep did not start ({err}); leases end at their next use");
    }
}

/// At shutdown (and a seal): every lease held here is forgotten, which zeroes its keys.
pub(crate) fn before_shutdown(factory: &ExecutionContextFactory) {
    if let Some(registry) = factory.extension_if_present::<LeaseRegistry>() {
        registry.clear();
    }
}

/// Tells the member that holds the lease it was approved or revoked, once the caller's transaction commits.
pub(crate) fn tell_member(facade: &EntityFacade, lease_id: &str, member: &str, kind: &'static str) {
    let body = json!({ "leaseId": lease_id, "member": member, "kind": kind });
    let send = {
        let facade = facade.clone();
        move || {
            let message = moqui_core::notify::TopicMessage {
                origin_id: moqui_core::notify::origin_id().to_string(),
                message_id: String::new(),
                body,
            };
            if let Err(err) = moqui_core::cluster::publish(&facade, moqui_core::notify::VAULT_LEASE, &message) {
                tracing::error!("Vault: the lease's member was not told (it reads the row within two seconds): {err}");
            }
        }
    };
    match facade.transactions_for(LEASE) {
        Ok(tx) if tx.in_transaction() => {
            if let Err(err) = tx.on_commit(send) {
                tracing::error!(
                    "Vault: the lease's member will not be told (it reads the row within two seconds): {err}"
                );
            }
        }
        _ => send(),
    }
}
