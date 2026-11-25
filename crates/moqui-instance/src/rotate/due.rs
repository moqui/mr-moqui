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

//! Rotation ages (plans/archive/Vault-Instances.md §4.7, §4.8): which operational items are past their age, and which of them
//! the manager can rotate.
//!
//! - **The age.** An operational item's own `rotationDays` (its binding's policy, `set#VaultItemRotation`), else its
//!   type's (`VaultItemType.rotationDays`: 90 days for a database credential and a login, 180 for a registry login, 365
//!   for an API key, an SSH key and a TLS key pair); none, never due. It counts from `rotatedDate`, when the value last
//!   changed at its source (made, deposited, rotated, or written marked rotated), else from when the item was made.
//! - **The list** of an environment ([`list`]): each item of its vault with its age, policy and whether it is due, the
//!   instance and secret it is bound to, whether the manager can rotate it (its kind's secret names the rotate
//!   service, [`super::ROTATE_SERVICE`]) and, when it cannot, what to do by hand. This is the incident list
//!   (Sealed-Secrets §3.12): the manager rotates every item of an environment that has a rotate service, and shows the
//!   rest.
//! - **Counts** for Monitor: the items past their age by environment (the gauge `moqui.instance.items_past_rotation`)
//!   and by instance (the fleet screen's column).
//! - **The removal offer** ([`offer`]): what removing a member from an operational vault offers to rotate (§3.9).

use std::collections::BTreeMap;

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{Value, ValueMap};

use crate::common::{field, find, one, APP_INSTANCE, ENVIRONMENT, INSTANCE_SECRET, KIND_SECRET};

const VAULT: &str = "moqui.security.vault.Vault";
const VAULT_ITEM: &str = "moqui.security.vault.VaultItem";
const ITEM_TYPE: &str = "moqui.security.vault.VaultItemType";
const DAY_MILLIS: i64 = 86_400_000;

/// One operational item as the rotation list shows it. No value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemAge {
    pub vault_id: String,
    pub item_id: String,
    pub binding: String,
    pub name: String,
    pub item_type: String,
    /// The policy: the item's own age, else its type's; none, never due.
    pub rotation_days: Option<i64>,
    /// Days since its value last changed at its source.
    pub age_days: i64,
    pub due: bool,
    /// The instance and secret it is bound to, when it is an instance's.
    pub instance_id: String,
    pub secret_name: String,
    /// The manager rotates it (its kind's secret names the rotate service).
    pub rotatable: bool,
    /// Deposited by the manager and not taken in yet: a member takes it in before it is leased or rotated.
    pub deposited: bool,
    /// What to do by hand when the manager cannot rotate it.
    pub by_hand: String,
}

impl ItemAge {
    /// The row a service answers.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut map = ValueMap::default();
        let text = |given: &str| Value::string(given);
        map.insert("vaultId".to_string(), text(&self.vault_id));
        map.insert("vaultItemId".to_string(), text(&self.item_id));
        map.insert("bindingKey".to_string(), text(&self.binding));
        map.insert("itemName".to_string(), text(&self.name));
        map.insert("vaultItemTypeId".to_string(), text(&self.item_type));
        map.insert(
            "rotationDays".to_string(),
            self.rotation_days.map_or(Value::Null, Value::I64),
        );
        map.insert("ageDays".to_string(), Value::I64(self.age_days));
        map.insert("due".to_string(), Value::Bool(self.due));
        map.insert("appInstanceId".to_string(), text(&self.instance_id));
        map.insert("secretName".to_string(), text(&self.secret_name));
        map.insert("rotatable".to_string(), Value::Bool(self.rotatable));
        map.insert("deposited".to_string(), Value::Bool(self.deposited));
        map.insert("byHand".to_string(), text(&self.by_hand));
        Value::Map(map)
    }
}

/// What to do by hand to rotate `binding` (of type `item_type`), when the manager cannot: `hint` when its kind gives
/// one, else by what the binding is.
#[must_use]
pub fn by_hand_text(binding: &str, item_type: &str, hint: &str) -> String {
    if !hint.trim().is_empty() {
        return hint.trim().to_string();
    }
    let mark = "write it to this item in your vault client, marked rotated";
    let mut parts = binding.splitn(3, ':');
    let (kind, id) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
    let field_name = parts.next().unwrap_or_default();
    match (kind, field_name, item_type) {
        ("InstanceHost", _, _) => format!(
            "Make a new client certificate and key for Docker host {id}: put the certificate on the host's row and the \
             key in this item, marked rotated; then revoke the old certificate"
        ),
        ("InstanceImage", _, _) => {
            format!("Make a new token at the registry of image {id}, {mark}; then revoke the old token there")
        }
        ("DatabaseHost", _, _) => format!("Change the admin password on database host {id}, then {mark}"),
        ("AppInstance", "sshKey", _) => format!(
            "Make a new SSH key: put its public half in the authorized keys of AppInstance {id}'s SSH user and its \
             private half in this item, marked rotated; then remove the old public key"
        ),
        _ => format!("Change it at its source, then {mark}"),
    }
}

fn millis(row: &EntityValue, name: &str) -> Option<i64> {
    row.get(name).and_then(moqui_core::user::value_millis)
}

/// The kinds' secrets: by kind and secret, the rotate service and what to do by hand.
type KindSecrets = BTreeMap<(String, String), (String, String)>;

/// What the rows say of the kinds' secrets.
fn kind_secrets(facade: &EntityFacade) -> Result<KindSecrets, LibError> {
    Ok(find(facade, KIND_SECRET)?
        .list()?
        .into_values()
        .into_iter()
        .map(|row| {
            (
                (field(&row, "instanceKindId"), field(&row, "secretName")),
                (field(&row, "rotateService"), field(&row, "rotateByHand")),
            )
        })
        .collect())
}

/// What the ages are read against, read once for any number of vaults: the item types' ages, the kinds' secrets, each
/// instance's kind and each instance secret's binding (so a vault's ages are one query of its items).
struct Lookup {
    type_days: BTreeMap<String, i64>,
    kinds: KindSecrets,
    /// Each instance's kind, by id.
    instances: BTreeMap<String, String>,
    /// Each instance secret's binding, by instance and secret.
    bound: BTreeMap<(String, String), String>,
}

impl Lookup {
    fn read(facade: &EntityFacade) -> Result<Lookup, LibError> {
        let instances = find(facade, APP_INSTANCE)?
            .list()?
            .into_values()
            .into_iter()
            .map(|row| (field(&row, "appInstanceId"), field(&row, "instanceKindId")))
            .collect();
        let bound = find(facade, INSTANCE_SECRET)?
            .list()?
            .into_values()
            .into_iter()
            .map(|row| {
                (
                    (field(&row, "appInstanceId"), field(&row, "secretName")),
                    field(&row, "bindingKey"),
                )
            })
            .collect();
        Ok(Lookup {
            type_days: type_days(facade)?,
            kinds: kind_secrets(facade)?,
            instances,
            bound,
        })
    }
}

/// The ages of every item of the operational vault `vault_id` at `now`.
fn vault_ages(facade: &EntityFacade, vault_id: &str, now: i64, lookup: &Lookup) -> Result<Vec<ItemAge>, LibError> {
    let mut out = Vec::new();
    let items = find(facade, VAULT_ITEM)?
        .eq("vaultId", Value::string(vault_id))
        .order_by("bindingKey", false)
        .list()?
        .into_values();
    for item in items {
        let binding = field(&item, "bindingKey");
        let item_type = field(&item, "vaultItemTypeId");
        let own: Option<i64> = field(&item, "rotationDays").parse().ok().filter(|days| *days > 0);
        let rotation_days = own.or_else(|| lookup.type_days.get(&item_type).copied());
        let since = millis(&item, "rotatedDate")
            .or_else(|| millis(&item, "createdDate"))
            .unwrap_or(now);
        let age_days = (now - since).max(0) / DAY_MILLIS;
        let due = rotation_days.is_some_and(|days| now - since >= days * DAY_MILLIS);
        let (mut instance_id, mut secret_name) = (String::new(), String::new());
        let mut service_hint: Option<(String, String)> = None;
        if let Some((id, secret)) = super::instance_binding(&binding) {
            let bound = lookup.bound.get(&(id.to_string(), secret.to_string())) == Some(&binding);
            if let (true, Some(kind)) = (bound, lookup.instances.get(id)) {
                instance_id = id.to_string();
                secret_name = secret.to_string();
                service_hint = lookup.kinds.get(&(kind.clone(), secret.to_string())).cloned();
            }
        }
        let rotatable = service_hint
            .as_ref()
            .is_some_and(|(service, _)| service == super::ROTATE_SERVICE);
        let by_hand = if rotatable {
            String::new()
        } else {
            let hint = service_hint.map(|(_, hint)| hint).unwrap_or_default();
            by_hand_text(&binding, &item_type, &hint).replace("{id}", &instance_id)
        };
        out.push(ItemAge {
            vault_id: vault_id.to_string(),
            item_id: field(&item, "vaultItemId"),
            binding,
            name: field(&item, "itemName"),
            item_type,
            rotation_days,
            age_days,
            due,
            instance_id,
            secret_name,
            rotatable,
            deposited: field(&item, "itemKeyWrapEnumId") == "VwDeposit",
            by_hand,
        });
    }
    Ok(out)
}

fn type_days(facade: &EntityFacade) -> Result<BTreeMap<String, i64>, LibError> {
    Ok(find(facade, ITEM_TYPE)?
        .list()?
        .into_values()
        .into_iter()
        .filter_map(|row| {
            let days: i64 = field(&row, "rotationDays").parse().ok()?;
            (days > 0).then(|| (field(&row, "vaultItemTypeId"), days))
        })
        .collect())
}

/// The rotation list of an environment at `now` (see the module doc): every item of its vault.
pub fn list(facade: &EntityFacade, environment_id: &str, now: i64) -> Result<Vec<ItemAge>, LibError> {
    let environment = one(facade, ENVIRONMENT, "environmentId", environment_id)?
        .ok_or_else(|| crate::common::fail(format!("No InstanceEnvironment {environment_id}")))?;
    let vault_id = field(&environment, "vaultId");
    if vault_id.is_empty() {
        return Ok(Vec::new());
    }
    vault_ages(facade, &vault_id, now, &Lookup::read(facade)?)
}

/// The items past their age in each environment's vault at `now`: (environment, count), for every environment with a
/// vault. One vault that fails to read is logged and counted as none.
pub fn past_by_environment(facade: &EntityFacade, now: i64) -> Result<Vec<(String, usize)>, LibError> {
    let lookup = Lookup::read(facade)?;
    // each vault once, however many environments share it
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut out = Vec::new();
    for environment in find(facade, ENVIRONMENT)?.list()?.into_values() {
        let (id, vault_id) = (field(&environment, "environmentId"), field(&environment, "vaultId"));
        if vault_id.is_empty() {
            continue;
        }
        if !counts.contains_key(&vault_id) {
            let count = match vault_ages(facade, &vault_id, now, &lookup) {
                Ok(ages) => ages.iter().filter(|age| age.due).count(),
                Err(err) => {
                    tracing::error!("Instances: the rotation ages of vault {vault_id} were not read: {err}");
                    0
                }
            };
            counts.insert(vault_id.clone(), count);
        }
        out.push((id, counts[&vault_id]));
    }
    Ok(out)
}

/// The items past their age bound to each instance at `now` (`AppInstance:<id>:...` in its environment's vault).
pub fn past_by_instance(facade: &EntityFacade, now: i64) -> Result<BTreeMap<String, usize>, LibError> {
    let lookup = Lookup::read(facade)?;
    let mut out: BTreeMap<String, usize> = BTreeMap::new();
    let mut vaults: Vec<String> = find(facade, ENVIRONMENT)?
        .list()?
        .into_values()
        .iter()
        .map(|environment| field(environment, "vaultId"))
        .filter(|vault_id| !vault_id.is_empty())
        .collect();
    vaults.sort_unstable();
    vaults.dedup();
    for vault_id in vaults {
        match vault_ages(facade, &vault_id, now, &lookup) {
            Ok(ages) => {
                for age in ages.into_iter().filter(|age| age.due) {
                    if let Some((id, _)) = super::instance_binding(&age.binding) {
                        *out.entry(id.to_string()).or_default() += 1;
                    }
                }
            }
            Err(err) => tracing::error!("Instances: the rotation ages of vault {vault_id} were not read: {err}"),
        }
    }
    Ok(out)
}

/// What removing a member from the operational vault `vault_id` offers to rotate (§3.9): each of its items, whether the
/// manager can rotate it and else what to do by hand. Installed into the vault server's removal
/// (`moqui_vault_server::members::install_rotation_offer`); a failure is logged and offers nothing.
pub fn offer(facade: &EntityFacade, vault_id: &str) -> Vec<Value> {
    let _authz = crate::common::authz_off();
    let ages = (|| -> Result<Vec<ItemAge>, LibError> {
        let operational = one(facade, VAULT, "vaultId", vault_id)?
            .is_some_and(|vault| field(&vault, "vaultKindEnumId") == "VkOperational");
        if !operational {
            return Ok(Vec::new());
        }
        vault_ages(facade, vault_id, moqui_core::user::now_millis(), &Lookup::read(facade)?)
    })();
    match ages {
        Ok(ages) => ages.iter().map(ItemAge::to_value).collect(),
        Err(err) => {
            tracing::error!("Instances: the rotation offer of vault {vault_id} was not made: {err}");
            Vec::new()
        }
    }
}
