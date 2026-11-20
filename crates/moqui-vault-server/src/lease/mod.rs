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
//!    is the one the lease names, writes a `VeLeaseUsed` event in its own transaction and only then decrypts and
//!    returns the value; when the event cannot be written nothing is returned (§3.13, fail closed).
//! 4. The lease ends at the operation's end ([`LeaseRegistry::end`]), at its time limit, on revoke and when the server
//!    stops or seals: its keys are dropped, which zeroes them, and nothing stored can open the sealed keys again. A
//!    restart empties the registry, so a use after it is refused with "lease ended; ask again", and the boot marks the
//!    member's leases left open ended ([`end_left_open`]).
//!
//! A leased value never goes into a stored call: the registry keeps what it handed out until the lease ends, and the
//! framework's stored-call paths refuse parameters that hold one (`moqui_core::vault_hooks::refuse_leased`).

pub(crate) mod key;
pub(crate) mod services;

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
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
    authz_off, fail, field, find, int_field, now, now_millis, refused, vault_row, ITEM, LEASE, OPERATIONAL,
    PERSONAL_KEY,
};
use crate::events::{self, Event};

pub(crate) const REQUESTED: &str = "VlsRequested";
pub(crate) const OPEN: &str = "VlsOpen";
pub(crate) const ENDED: &str = "VlsEnded";
pub(crate) const REVOKED: &str = "VlsRevoked";

/// How long a waiting operation sleeps before it reads the lease's row again, when no cluster message woke it.
const POLL: Duration = Duration::from_secs(2);
/// The shortest field value of an item of no known type that the stored-call guard looks for; a masked field of a
/// known type is looked for from [`MIN_MASKED`] characters.
const MIN_UNTYPED: usize = 8;
const MIN_MASKED: usize = 4;

/// The refusal of a lease this member does not hold: ended, revoked, timed out, or held before a restart.
fn ended() -> LibError {
    fail("The lease ended; ask again (reasonCode leaseEnded)")
}

/// A lease this member holds: the signed statement, its private key, the item keys once opened, and each value
/// handed out (for the stored-call guard). Every key is a zeroizing type, dropped with the lease.
struct Held {
    lease: Lease,
    secret: hpke::SecretKey,
    item_keys: Vec<(String, Key32)>,
    handed: Vec<Zeroizing<String>>,
    requested_by: String,
}

/// The leases this cluster member holds, by id: in memory only, so a restart empties it. One per factory
/// ([`registry`]); the tests make more, as two cluster members.
pub struct LeaseRegistry {
    member_id: String,
    held: Mutex<HashMap<String, Held>>,
    changed: Condvar,
}

impl std::fmt::Debug for LeaseRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LeaseRegistry")
            .field("member_id", &self.member_id)
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

/// This factory's registry, made on first use, under the cluster member's id (`local` outside a cluster).
pub fn registry(factory: &ExecutionContextFactory) -> Arc<LeaseRegistry> {
    factory.extension(|factory| LeaseRegistry::new(&member_id(factory)))
}

/// The id a lease names its cluster member by.
pub(crate) fn member_id(factory: &ExecutionContextFactory) -> String {
    moqui_core::cluster::instance_id(factory).unwrap_or_else(|| "local".to_string())
}

/// The longest lease, from `vault@lease-max-hours` (4 when unset), never past the vault crate's 4 hours, which a
/// client refuses.
fn max_lease() -> Duration {
    let hours: f64 = moqui_core::context::conf_env::setting("vault@lease-max-hours")
        .parse()
        .unwrap_or(4.0);
    let hours = if hours.is_finite() && hours > 0.0 { hours } else { 4.0 };
    let max = Duration::from_millis(u64::try_from(vault_lease::MAX_LEASE_MS).unwrap_or(0));
    Duration::from_secs_f64(hours * 3600.0).min(max)
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
            member_id: member_id.to_string(),
            held: Mutex::new(HashMap::new()),
            changed: Condvar::new(),
        }
    }

    pub fn member_id(&self) -> &str {
        &self.member_id
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

    /// Drops the leases past their time limit.
    fn sweep(&self) {
        let now = now_millis();
        lock(&self.held).retain(|_, held| held.lease.expires_at > now);
    }

    /// Whether `text` holds a value one of this member's live leases handed out.
    pub fn holds_value(&self, text: &str) -> bool {
        self.sweep();
        let held = lock(&self.held);
        held.values()
            .flat_map(|held| held.handed.iter())
            .any(|value| text.contains(value.as_str()))
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
        self.keep(&lease, secret, Vec::new(), &requested_by);
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
        if ask.operation.trim().is_empty() || ask.operation.len() > 255 {
            return Err(fail("A lease names its operation, in at most 255 characters"));
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
        let limit = ask.limit.unwrap_or(Duration::MAX).min(max_lease());
        let (secret, public) = vault_lease::new_lease_key(&mut OsRng).map_err(refused)?;
        let mut lease = Lease {
            lease_id: new_lease_id(),
            vault_id: ask.vault_id.to_string(),
            key_version: u64::try_from(key_version).map_err(|_| fail("a key version out of range"))?,
            items,
            operation: ask.operation.trim().to_string(),
            cluster_member: self.member_id.clone(),
            expires_at: now_millis().saturating_add(millis(limit)),
            public,
            signature: Vec::new(),
        };
        lease.sign(&key::signing_key()?);
        Ok((lease, secret))
    }

    fn keep(&self, lease: &Lease, secret: hpke::SecretKey, item_keys: Vec<(String, Key32)>, requested_by: &str) {
        lock(&self.held).insert(
            lease.lease_id.clone(),
            Held {
                lease: lease.clone(),
                secret,
                item_keys,
                handed: Vec::new(),
                requested_by: requested_by.to_string(),
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
        self.keep(&lease, secret, item_keys, &requested_by);
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

    /// Opens the approval on the row with the lease's private key: the approver's signature over the lease this member
    /// signed (not the row's copy of it), then each item key.
    fn open_approval(&self, facade: &EntityFacade, lease_id: &str, row: &EntityValue) -> Result<(), LibError> {
        let approval = approval_of_row(row)?;
        let approver_key = find(facade, PERSONAL_KEY)?
            .eq("personalKeyId", Value::string(field(row, "approverKeyId")))
            .one()?
            .ok_or_else(|| fail("The lease's approver key is not found"))?;
        if field(&approver_key, "userId") != approval.approver_user_id {
            return Err(fail("The lease's approver key is not the approver's"));
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
        let masked = masked_fields(facade, &item.item_type);
        let mut handed = Vec::new();
        for (name, value) in fields {
            let Some(text) = value.as_str() else { continue };
            let wanted = match &masked {
                Some(masked) => masked.contains(name) && text.len() >= MIN_MASKED,
                None => text.len() >= MIN_UNTYPED,
            };
            if wanted {
                handed.push(Zeroizing::new(text.to_string()));
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

/// At boot: the leases this member left open before a restart are ended, and the `vault-lease` topic is listened to.
pub(crate) fn after_boot(factory: &ExecutionContextFactory) {
    let registry = registry(factory);
    if let Some(facade) = factory.entity() {
        let ended = {
            let _authz = authz_off();
            end_left_open(&facade, registry.member_id())
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
        if body["member"].as_str() == Some(registry.member_id()) && body["kind"].as_str() == Some("revoked") {
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
