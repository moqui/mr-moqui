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

//! Rotation (plans/Vault-Instances.md §4.7, M5): a managed credential changed at its source, then on each instance that
//! uses it, then in the vault, under a lease or the environment's machine access.
//!
//! The kind's `rotate#Credential` ([`InstanceKindSecret.rotateService`]) runs, for each credential, in this order:
//!
//! 0. **Before anything changes**: the binding is claimed (one rotation of an item at a time across a cluster,
//!    [`claim`]); the vault's deposit key is checked; each instance that uses the credential must be up and unsealed on
//!    its link (as the manager takes it, M6: unsealed by its transit on that link, or proved by the key derived from the
//!    root key; a bare claim reads `unverified` and the put is refused; a rotated value goes only to an instance the
//!    manager would deploy to); and the value it holds now must be one that could go back.
//! 1. **Staged**: a new value made here and deposited before anything else
//!    ([`moqui_vault_server::lease::LeaseRegistry::stage_rotation`]): the item's body, opened under the lease with only
//!    the rotated field changed, sealed under a new item key wrapped to the deposit key, a pending revision the manager
//!    cannot read back; the item's revision then is the one the rotation started from.
//! 2. **The source** ([`source`]): the value set where the credential is checked: a database user's password on its
//!    database (a one-shot on the database's network, or verified TLS to a database host, with the admin login
//!    leased), a database's admin password (signed in as the admin with the value it replaces), an OpenSearch admin
//!    password (its security API).
//! 3. **The instances**: the value put on each mr-moqui instance that uses it (`moqui operator secret put` over its
//!    link, or SSH, never a service's parameter), then the instance restarted (over its link; over SSH its container,
//!    through its host) so it reads the value.
//! 4. **The check**: the source takes the new value, and each instance comes back up and unsealed within
//!    `manage@deploy-wait-seconds`.
//! 5. **The item**: the staged value becomes the item's next revision
//!    ([`moqui_vault_server::deposit::promote_revision`]), only from the revision the rotation started from. The
//!    revision before stays in `VaultItemHistory`, the way back. A member takes the new revision in (`take#VaultDeposit`)
//!    before a lease names the item again, and an environment's machine access to it must be given again.
//!
//! **When a step after the source fails** (an instance refused the value, did not come back, the source did not take
//! the value, or the item was written meanwhile), the old value is put back at the source and on each instance that
//! took the new one, which is restarted; the item is not changed and the staged value is dropped. When the old value
//! cannot be put back everywhere, the staged value is kept for a member to recover (the source may hold it). Each
//! credential of a rotation is its own `InstanceAction` (`IatRotate`, on the instance whose secret it is): one that
//! fails is reported and the rest go on. A database user's credential is rotated before its database's admin login in
//! the same rotation, since its change signs in with the admin login the lease holds.
//!
//! A credential the manager cannot change (an item with no rotate service: a registry token, a host's client key, a
//! database host's admin login, an ingest key) is never rotated here; [`due`] shows it as due with what to do by hand.

pub mod claim;
pub mod due;
pub(crate) mod services;
pub mod source;

use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::seal::secrets::{DATASOURCE_PASSWORD, SEARCH_CLUSTER_PASSWORD};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{to_plain_string, Value};
use moqui_vault_server::lease::{Ask, LeaseRegistry};
use zeroize::Zeroizing;

use crate::actions::{self, Requested};
use crate::common::{
    fail, field, find, host_needs_key, one, rows, APP_INSTANCE, ENVIRONMENT, HOST, IMAGE, INSTANCE_DEPENDENCY,
    INSTANCE_SECRET, KIND, KIND_SECRET,
};
use crate::deploy::LinkStatus;
use crate::plan::{self, AdminSource, Destination, Reach};
use source::{Network, Source};

/// The kinds' rotate service: what `InstanceKindSecret.rotateService` names for a credential the manager rotates.
pub const ROTATE_SERVICE: &str = "org.moqui.dev.InstanceServices.rotate#Credential";
const VAULT_ITEM: &str = "moqui.security.vault.VaultItem";
const LINK_COMMAND: &str = "org.moqui.dev.InstanceLinkServices.call#InstanceCommand";
/// How often a rotation asks whether a restarted instance is up.
const POLL: Duration = Duration::from_secs(2);
/// The longest operation a lease names.
const MAX_OPERATION: usize = 255;

/// What a rotation does to a credential; the order in which a rotation of several runs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Method {
    /// A database user's password (an mr-moqui instance's `dbPassword`): set by the database's admin login.
    DatabaseUser,
    /// An OpenSearch instance's admin password.
    SearchAdmin,
    /// A database instance's admin password: after the users of that database, whose change signs in with it.
    DatabaseAdmin,
}

/// An instance that uses a credential, and the secret its value is put as there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Use {
    pub instance_id: String,
    pub name: &'static str,
}

/// A credential the manager rotates: its item and where its value goes.
#[derive(Debug, Clone)]
pub struct Credential {
    /// The instance whose secret it is, which its action is recorded on.
    pub instance_id: String,
    pub secret_name: String,
    pub binding: String,
    pub item_id: String,
    pub item_name: String,
    pub item_type: String,
    /// The item's field that holds the value.
    pub field_name: String,
    pub method: Method,
    pub source: Source,
    pub uses: Vec<Use>,
    /// The kind of the source, whose rule the new value follows (`kinds::credential_for`).
    pub source_kind: String,
}

/// A binding asked for that is not rotated, and why (what to do by hand, or what to do first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotRotated {
    pub binding: String,
    pub reason: String,
}

/// A rotation's plan: the credentials, in the order they run, the lease that opens them and what it names.
#[derive(Debug, Clone)]
pub struct Plan {
    pub environment_id: String,
    pub vault_id: String,
    pub machine_access: bool,
    pub lease_limit: Option<Duration>,
    pub credentials: Vec<Credential>,
    pub not_rotated: Vec<NotRotated>,
    /// The vault items the rotation opens, by binding: what its lease must name.
    pub bindings: Vec<String>,
    /// The operation its lease names.
    pub operation: String,
}

/// The plan of a rotation of `bindings` (operational items of the environment's vault).
pub fn plan(facade: &EntityFacade, environment_id: &str, bindings: &[String]) -> Result<Plan, LibError> {
    let environment = one(facade, ENVIRONMENT, "environmentId", environment_id)?
        .ok_or_else(|| fail(format!("No InstanceEnvironment {environment_id}")))?;
    let vault_id = field(&environment, "vaultId");
    if vault_id.is_empty() {
        return Err(fail(format!(
            "InstanceEnvironment {environment_id} names no operational vault"
        )));
    }
    let mut credentials: Vec<Credential> = Vec::new();
    let mut not_rotated = Vec::new();
    let mut opened: Vec<String> = Vec::new();
    let mut destinations: Vec<Destination> = Vec::new();
    for binding in bindings {
        if credentials.iter().any(|known| known.binding == *binding)
            || not_rotated.iter().any(|known: &NotRotated| known.binding == *binding)
        {
            continue;
        }
        match credential(facade, &environment, &vault_id, binding) {
            Ok((credential, more, to)) => {
                for binding in std::iter::once(credential.binding.clone()).chain(more) {
                    if !opened.contains(&binding) {
                        opened.push(binding);
                    }
                }
                for destination in to {
                    if !destinations.iter().any(|known| known.binding == destination.binding) {
                        destinations.push(destination);
                    }
                }
                credentials.push(credential);
            }
            Err(reason) => not_rotated.push(NotRotated {
                binding: binding.clone(),
                reason,
            }),
        }
    }
    // a database's users first: their change signs in with the admin login the lease holds before it is rotated
    credentials.sort_by_key(|credential| credential.method);
    let operation = operation(environment_id, &credentials, &destinations);
    Ok(Plan {
        environment_id: environment_id.to_string(),
        vault_id,
        machine_access: field(&environment, "machineAccess") == "Y",
        lease_limit: plan::lease_limit(&environment),
        credentials,
        not_rotated,
        bindings: opened,
        operation,
    })
}

/// The operation a rotation's lease names: the credential and where each leased item goes when it fits a lease, else
/// how many credentials of which environment (the lease lists each item).
fn operation(environment_id: &str, credentials: &[Credential], destinations: &[Destination]) -> String {
    let head = match credentials {
        [one] => format!("rotate {}", one.binding),
        many => format!("rotate {} credentials of environment {environment_id}", many.len()),
    };
    let to: Vec<String> = destinations
        .iter()
        .map(|destination| format!("{} to {}", destination.binding, destination.to))
        .collect();
    let full = if to.is_empty() {
        head.clone()
    } else {
        format!("{head}; {}", to.join(", "))
    };
    if full.len() <= MAX_OPERATION && !full.chars().any(char::is_control) {
        full
    } else {
        head.chars().take(MAX_OPERATION).collect()
    }
}

/// The instance id and secret name of a binding `AppInstance:<id>:<secret>`.
pub fn instance_binding(binding: &str) -> Option<(&str, &str)> {
    let rest = binding.strip_prefix("AppInstance:")?;
    let (id, secret) = rest.rsplit_once(':')?;
    (!id.is_empty() && !secret.is_empty()).then_some((id, secret))
}

/// A credential of `binding`, with the other bindings its lease must name and where each goes; else why it is not
/// rotated here.
fn credential(
    facade: &EntityFacade,
    environment: &EntityValue,
    vault_id: &str,
    binding: &str,
) -> Result<(Credential, Vec<String>, Vec<Destination>), String> {
    let text = |err: LibError| err.message().to_string();
    let item = find(facade, VAULT_ITEM)
        .and_then(|find| {
            find.eq("vaultId", Value::string(vault_id))
                .eq("bindingKey", Value::string(binding))
                .one()
        })
        .map_err(text)?
        .ok_or_else(|| format!("No item is bound to {binding} in the environment's vault {vault_id}"))?;
    let Some((instance_id, secret_name)) = instance_binding(binding) else {
        return Err(due::by_hand_text(binding, &field(&item, "vaultItemTypeId"), ""));
    };
    let instance = one(facade, APP_INSTANCE, "appInstanceId", instance_id)
        .map_err(text)?
        .ok_or_else(|| format!("No AppInstance {instance_id}"))?;
    if field(&instance, "environmentId") != field(environment, "environmentId") {
        return Err(format!(
            "AppInstance {instance_id} is not in environment {}",
            field(environment, "environmentId")
        ));
    }
    let secret = rows(facade, INSTANCE_SECRET, "appInstanceId", instance_id)
        .map_err(text)?
        .into_iter()
        .find(|secret| field(secret, "secretName") == secret_name && field(secret, "bindingKey") == binding)
        .ok_or_else(|| format!("AppInstance {instance_id} binds no secret {secret_name} to {binding}"))?;
    let kind_id = field(&instance, "instanceKindId");
    let kind_secret = find(facade, KIND_SECRET)
        .and_then(|find| {
            find.eq("instanceKindId", Value::string(&kind_id))
                .eq("secretName", Value::string(secret_name))
                .one()
        })
        .map_err(text)?;
    let service = kind_secret
        .as_ref()
        .map(|row| field(row, "rotateService"))
        .unwrap_or_default();
    if service.is_empty() {
        let hint = kind_secret
            .as_ref()
            .map(|row| field(row, "rotateByHand"))
            .unwrap_or_default();
        return Err(due::by_hand_text(binding, &field(&item, "vaultItemTypeId"), &hint).replace("{id}", instance_id));
    }
    if service != ROTATE_SERVICE {
        return Err(format!(
            "{kind_id}'s {secret_name} names the rotate service {service}, not {ROTATE_SERVICE}"
        ));
    }
    if field(&item, "itemKeyWrapEnumId") != "VwVault" {
        return Err(format!(
            "{binding} was deposited by the manager and not taken in yet: a member opens the vault to take it in, then \
             it can be rotated (reasonCode notTakenIn)"
        ));
    }
    if field(&instance, "instanceUuid").is_empty() {
        return Err(format!("AppInstance {instance_id} is not provisioned"));
    }
    let kind = one(facade, KIND, "instanceKindId", &kind_id)
        .map_err(text)?
        .ok_or_else(|| format!("AppInstance {instance_id} has no InstanceKind {kind_id}"))?;
    let conf_name = field(&kind, "confName");
    let mut more = Vec::new();
    let mut to = Vec::new();
    let (method, source, source_kind) = match (conf_name.as_str(), kind_id.as_str(), secret_name) {
        ("postgres" | "mysql" | "mysql8", _, "admin") => {
            let network = network_of(facade, &instance, &kind, &mut more, &mut to).map_err(text)?;
            to.push(Destination {
                binding: binding.to_string(),
                to: format!("{}:{}", network.server, network.port),
            });
            (Method::DatabaseAdmin, Source::DatabaseAdmin(network), kind_id.clone())
        }
        (_, "opensearch", "admin") => {
            let network = network_of(facade, &instance, &kind, &mut more, &mut to).map_err(text)?;
            to.push(Destination {
                binding: binding.to_string(),
                to: format!("{}:{}", network.server, network.port),
            });
            (Method::SearchAdmin, Source::SearchAdmin(network), kind_id.clone())
        }
        (_, _, "dbPassword") => {
            let database = plan::database_of(facade, instance_id)
                .map_err(text)?
                .ok_or_else(|| format!("AppInstance {instance_id} has no database dependency"))?;
            let AdminSource::Leased(admin_binding) = database.admin.clone() else {
                return Err(format!(
                    "AppInstance {instance_id}'s database has no admin login to lease"
                ));
            };
            more.push(admin_binding.clone());
            to.push(Destination {
                binding: admin_binding.clone(),
                to: format!("{}:{}", database.expect_host, database.expect_port),
            });
            if let Reach::Network { host, .. } = &database.reach {
                if host_needs_key(host) {
                    let destination = plan::host_destination(host);
                    more.push(destination.binding.clone());
                    to.push(destination);
                }
            }
            to.push(Destination {
                binding: binding.to_string(),
                to: format!("user {} on {}", database.user, database.on),
            });
            let source_kind = match &database.on {
                on if on.starts_with("AppInstance ") => on
                    .strip_prefix("AppInstance ")
                    .and_then(|id| one(facade, APP_INSTANCE, "appInstanceId", id).ok().flatten())
                    .map(|row| field(&row, "instanceKindId"))
                    .unwrap_or_default(),
                _ => database.conf_name.clone(),
            };
            (
                Method::DatabaseUser,
                Source::DatabaseUser {
                    database: Box::new(database),
                    admin_binding,
                },
                source_kind,
            )
        }
        _ => {
            return Err(format!(
            "No rotation is built for {kind_id}'s {secret_name}: change it by hand and write it to the item, marked \
                 rotated"
        ))
        }
    };
    let uses = uses_of(facade, &instance, &kind_id, method).map_err(text)?;
    for used in &uses {
        let row = one(facade, APP_INSTANCE, "appInstanceId", &used.instance_id)
            .map_err(text)?
            .ok_or_else(|| format!("No AppInstance {}", used.instance_id))?;
        let transport = crate::link::transport(facade, &row).map_err(text)?;
        let ssh_key = crate::ssh::key_binding(&used.instance_id);
        let has_ssh_key = find(facade, VAULT_ITEM)
            .and_then(|find| {
                find.eq("vaultId", Value::string(vault_id))
                    .eq("bindingKey", Value::string(&ssh_key))
                    .one()
            })
            .map_err(text)?
            .is_some();
        if transport == "ssh" || (transport == "both" && has_ssh_key) {
            more.push(ssh_key.clone());
            to.push(Destination {
                binding: ssh_key,
                to: format!("ssh {}", field(&row, "sshAddress")),
            });
            // over SSH the instance is restarted through its container's host
            let host = crate::lifecycle::host_of(facade, &row).map_err(text)?;
            if host_needs_key(&host) {
                let destination = plan::host_destination(&host);
                more.push(destination.binding.clone());
                to.push(destination);
            }
        }
        to.push(Destination {
            binding: binding.to_string(),
            to: format!("AppInstance {}", used.instance_id),
        });
    }
    Ok((
        Credential {
            instance_id: instance_id.to_string(),
            secret_name: secret_name.to_string(),
            binding: binding.to_string(),
            item_id: field(&item, "vaultItemId"),
            item_name: field(&item, "itemName"),
            item_type: field(&item, "vaultItemTypeId"),
            field_name: Some(field(&secret, "fieldName"))
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "password".to_string()),
            method,
            source,
            uses,
            source_kind,
        },
        more,
        to,
    ))
}

/// A server instance on its environment's network, reached by a one-shot of its own image; its host's client key, when
/// it has one, is added to the lease.
fn network_of(
    facade: &EntityFacade,
    instance: &EntityValue,
    kind: &EntityValue,
    more: &mut Vec<String>,
    to: &mut Vec<Destination>,
) -> Result<Network, LibError> {
    let id = field(instance, "appInstanceId");
    let host = one(facade, HOST, "instanceHostId", &field(instance, "instanceHostId"))?
        .ok_or_else(|| fail(format!("AppInstance {id} has no InstanceHost")))?;
    if host_needs_key(&host) {
        let destination = plan::host_destination(&host);
        more.push(destination.binding.clone());
        to.push(destination);
    }
    let image = one(facade, IMAGE, "instanceImageId", &field(instance, "instanceImageId"))?
        .ok_or_else(|| fail(format!("AppInstance {id} has no InstanceImage")))?;
    Ok(Network {
        host,
        network: field(instance, "networkName"),
        image: field(&image, "imageName"),
        server: field(instance, "instanceName"),
        port: field(kind, "containerPort"),
        conf_name: field(kind, "confName"),
        instance_id: id,
    })
}

/// The mr-moqui instances that use a credential, and the secret its value is put as on each: an instance's own
/// database password; an OpenSearch admin password on each instance whose search it is. A database's admin login is
/// used by the manager alone.
fn uses_of(facade: &EntityFacade, instance: &EntityValue, kind_id: &str, method: Method) -> Result<Vec<Use>, LibError> {
    let id = field(instance, "appInstanceId");
    Ok(match method {
        Method::DatabaseAdmin => Vec::new(),
        Method::DatabaseUser if kind_id == "mr-moqui" => vec![Use {
            instance_id: id,
            name: DATASOURCE_PASSWORD,
        }],
        Method::DatabaseUser => Vec::new(),
        Method::SearchAdmin => {
            let mut uses = Vec::new();
            let dependents = find(facade, INSTANCE_DEPENDENCY)?
                .eq("dependsOnInstanceId", Value::string(&id))
                .eq("dependencyRole", Value::string("search"))
                .list()?
                .into_values();
            for dependency in dependents {
                let user_id = field(&dependency, "appInstanceId");
                let Some(user) = one(facade, APP_INSTANCE, "appInstanceId", &user_id)? else {
                    continue;
                };
                if field(&user, "instanceKindId") == "mr-moqui"
                    && !field(&user, "instanceUuid").is_empty()
                    && field(&user, "desiredStateEnumId") != "IdsRemoved"
                {
                    uses.push(Use {
                        instance_id: user_id,
                        name: SEARCH_CLUSTER_PASSWORD,
                    });
                }
            }
            uses
        }
    })
}

// ----- the instances -----

/// What a rotation asks of the instances that use a credential: whether each is up and unsealed, the put of the new
/// value, and its restart. [`LinkInstances`] is the manager's; the tests give a fake.
pub trait Instances {
    fn status(&self, instance_id: &str) -> Result<LinkStatus, LibError>;
    /// Put the secret `name` as `value` (never a service's parameter: the link's Rust path, or SSH's standard input).
    fn put_secret(&self, instance_id: &str, name: &str, value: &str) -> Result<(), LibError>;
    fn restart(&self, instance_id: &str) -> Result<(), LibError>;
}

/// The instances as this manager reaches them: by the environment's transport, the link (`link`, or `both` while the
/// row says it is linked) or SSH (`ssh`, or `both` while it is not), under the thread's lease.
pub struct LinkInstances<'a> {
    pub facade: &'a EntityFacade,
}

impl LinkInstances<'_> {
    /// The instance's row, and whether the manager reaches it over SSH now.
    fn via(&self, instance_id: &str) -> Result<(EntityValue, bool), LibError> {
        let row = crate::link::instance(self.facade, instance_id)?;
        let ssh = match crate::link::transport(self.facade, &row)? {
            "ssh" => true,
            "both" => field(&row, "linkState") != "linked",
            _ => false,
        };
        Ok((row, ssh))
    }

    fn ssh(&self, row: &EntityValue, command: &crate::ssh::SshCommand) -> Result<String, LibError> {
        let id = field(row, "appInstanceId");
        let target = crate::ssh::SshTarget::of(row)?;
        let key = crate::common::leased(&crate::ssh::key_binding(&id), "privateKey")?;
        let program = Some(moqui_core::context::conf_env::setting("manage@ssh-command"))
            .map(|program| program.trim().to_string())
            .filter(|program| !program.is_empty())
            .unwrap_or_else(|| "ssh".to_string());
        crate::ssh::run(&program, &target, key.as_bytes(), command)
    }

    fn command(instance_id: &str, command: &str) -> Result<std::collections::BTreeMap<String, Value>, LibError> {
        let parameters = [
            ("appInstanceId".to_string(), Value::string(instance_id)),
            ("command".to_string(), Value::string(command)),
        ]
        .into();
        crate::lifecycle::call(LINK_COMMAND, &parameters)
    }
}

impl Instances for LinkInstances<'_> {
    fn status(&self, instance_id: &str) -> Result<LinkStatus, LibError> {
        let (row, ssh) = self.via(instance_id)?;
        if !ssh {
            let out = Self::command(instance_id, "status")?;
            let text = |name: &str| out.get(name).map(to_plain_string).unwrap_or_default();
            return Ok(LinkStatus {
                seal_state: text("sealState"),
                health: text("health"),
                schema_hash: text("schemaHash"),
            });
        }
        // over SSH: the seal as `moqui operator status` prints it, and the health of its container when it has one
        let printed = self.ssh(&row, &crate::ssh::SshCommand::Status)?;
        let seal_state = printed
            .lines()
            .find_map(|line| line.trim().strip_prefix("State: "))
            .map(|state| state.split(',').next().unwrap_or_default().trim().to_string())
            .unwrap_or_default();
        let health = if field(&row, "instanceUuid").is_empty() {
            "ok".to_string()
        } else {
            match crate::lifecycle::check_container(self.facade, instance_id)? {
                Some(info) if info.running && info.health.as_deref().is_none_or(|health| health == "healthy") => {
                    "ok".to_string()
                }
                Some(info) => info.health.unwrap_or(info.status),
                None => "missing".to_string(),
            }
        };
        Ok(LinkStatus {
            seal_state,
            health,
            schema_hash: String::new(),
        })
    }

    fn put_secret(&self, instance_id: &str, name: &str, value: &str) -> Result<(), LibError> {
        let (row, ssh) = self.via(instance_id)?;
        if !ssh {
            return crate::link::link_put(instance_id, name, value).map_err(|err| {
                err.unwrap_or_else(|| {
                    fail(format!(
                        "AppInstance {instance_id} is not linked now: the value was not put"
                    ))
                })
            });
        }
        self.ssh(
            &row,
            &crate::ssh::SshCommand::PutSecret {
                name: name.to_string(),
                value: Zeroizing::new(value.to_string()),
                force: false,
            },
        )
        .map(|_| ())
    }

    fn restart(&self, instance_id: &str) -> Result<(), LibError> {
        let (row, ssh) = self.via(instance_id)?;
        if !ssh {
            return Self::command(instance_id, "restart").map(|_| ());
        }
        if field(&row, "instanceUuid").is_empty() {
            return Err(fail(format!(
                "AppInstance {instance_id} has no container this manager restarts: restart it by hand"
            )));
        }
        crate::lifecycle::stop_container(self.facade, instance_id)?;
        crate::lifecycle::start_container(self.facade, instance_id).map(|_| ())
    }
}

/// Waits until each instance of `ids` is up and unsealed, at most `wait`: the ones that are not.
fn wait_ready(instances: &dyn Instances, ids: &[String], wait: Duration) -> Result<(), LibError> {
    let until = Instant::now() + wait;
    let mut waiting: Vec<(String, String)> = ids.iter().map(|id| (id.clone(), String::new())).collect();
    loop {
        for (id, last) in &mut waiting {
            match instances.status(id) {
                Ok(status) if status.ready() => last.clear(),
                Ok(status) => *last = format!("{}, {}", status.seal_state, status.health),
                Err(err) => *last = err.message().to_string(),
            }
        }
        waiting.retain(|(_, last)| !last.is_empty());
        if waiting.is_empty() {
            return Ok(());
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            let named: Vec<String> = waiting
                .iter()
                .map(|(id, last)| format!("AppInstance {id} ({last})"))
                .collect();
            return Err(fail(format!(
                "{} did not come back up and unsealed in {}s",
                named.join(", "),
                wait.as_secs()
            )));
        }
        std::thread::sleep(POLL.min(left));
    }
}

// ----- the run -----

/// How a rotation runs.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// How long an instance is given to come back up and unsealed after its restart.
    pub wait: Duration,
}

impl Options {
    /// From the conf: `manage@deploy-wait-seconds` (600), as a deploy waits.
    #[must_use]
    pub fn from_conf() -> Options {
        let seconds: u64 = moqui_core::context::conf_env::setting("manage@deploy-wait-seconds")
            .parse()
            .unwrap_or(600);
        Options {
            wait: Duration::from_secs(seconds),
        }
    }
}

/// What a rotation did.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    /// Each credential's action, in order.
    pub action_ids: Vec<String>,
    /// The bindings rotated, with each one's new revision.
    pub rotated: Vec<(String, i64)>,
    /// The bindings that failed, with why (the old value put back).
    pub failed: Vec<(String, String)>,
    pub report: Vec<String>,
}

impl Outcome {
    /// One line of what it did, for a waiting action's row.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut line = format!(
            "rotated {} of {}",
            self.rotated.len(),
            self.rotated.len() + self.failed.len()
        );
        if !self.failed.is_empty() {
            let failed: Vec<&str> = self.failed.iter().map(|(binding, _)| binding.as_str()).collect();
            line = format!("{line}; failed, the old value kept: {}", failed.join(", "));
        }
        format!("{line} (actions {})", self.action_ids.join(", "))
    }
}

/// How long a run's claim of a binding holds when its manager stops before it lets it go: the longest a lease runs.
const CLAIM_MILLIS: i64 = 4 * 3_600_000;

/// A token of a run under machine access, which has no lease asked for before it.
fn run_token() -> String {
    let mut bytes = [0_u8; 8];
    moqui_vault::rng::Rng::fill(&mut moqui_vault::rng::OsRng, &mut bytes);
    format!("run-{}", moqui_vault::text::hex(&bytes))
}

/// Runs a rotation's plan under the lease `lease_id`, or the environment's machine access (see the module doc). Its
/// error is a refusal before anything changed; a credential that fails, or that another rotation holds, is in the
/// outcome's `failed`.
pub fn run(
    facade: &EntityFacade,
    registry: &LeaseRegistry,
    instances: &dyn Instances,
    plan: &Plan,
    lease_id: Option<&str>,
    options: &Options,
) -> Result<Outcome, LibError> {
    let Some(first) = plan.credentials.first() else {
        let reasons: Vec<String> = plan
            .not_rotated
            .iter()
            .map(|not| format!("{}: {}", not.binding, not.reason))
            .collect();
        return Err(fail(format!("Nothing the manager can rotate: {}", reasons.join("; "))));
    };
    let environment = one(facade, ENVIRONMENT, "environmentId", &plan.environment_id)?
        .ok_or_else(|| fail(format!("No InstanceEnvironment {}", plan.environment_id)))?;
    actions::check_caller(&environment)?;
    let refuse = |err: LibError| -> Result<Outcome, LibError> {
        actions::record(
            facade,
            &first.instance_id,
            "rotate",
            actions::REFUSED,
            lease_id,
            false,
            err.message(),
        )?;
        Err(err)
    };
    // it deposits before it changes a source: a vault whose deposit key is gone or unsigned is refused first
    if let Err(err) = moqui_vault_server::deposit::check_deposit_key(facade, &plan.vault_id) {
        return refuse(fail(format!(
            "A rotation deposits each new value into vault {}, which cannot take a deposit now: {}",
            plan.vault_id,
            err.message()
        )));
    }
    let opened = match actions::open_authority(
        facade,
        registry,
        &plan.environment_id,
        &plan.vault_id,
        plan.machine_access,
        &plan.bindings,
        &plan.operation,
        lease_id,
    ) {
        Ok(opened) => opened,
        Err(err) => return refuse(err),
    };
    let Some(open_lease) = opened.lease_id.clone() else {
        return refuse(fail("A rotation opens its items under a lease"));
    };
    // a request's claims are its lease's; a run under machine access takes its own
    let token = lease_id.map_or_else(run_token, str::to_string);
    let mut outcome = Outcome::default();
    for credential in &plan.credentials {
        let until = moqui_core::user::now_millis() + CLAIM_MILLIS;
        if let Err(err) = claim::claim(facade, &plan.vault_id, &credential.binding, &token, until) {
            if let Ok(id) = actions::record(
                facade,
                &credential.instance_id,
                "rotate",
                actions::REFUSED,
                opened.lease_id.as_deref(),
                opened.machine,
                err.message(),
            ) {
                outcome.action_ids.push(id);
            }
            outcome
                .failed
                .push((credential.binding.clone(), err.message().to_string()));
            continue;
        }
        let action_id = match actions::record(
            facade,
            &credential.instance_id,
            "rotate",
            actions::RUNNING,
            opened.lease_id.as_deref(),
            opened.machine,
            "",
        ) {
            Ok(id) => id,
            Err(err) => {
                tracing::error!(
                    "Instances: the rotation of {} was not recorded: {err}",
                    credential.binding
                );
                claim::release(facade, &plan.vault_id, &credential.binding, &token, None);
                outcome
                    .failed
                    .push((credential.binding.clone(), err.message().to_string()));
                continue;
            }
        };
        claim::note_action(facade, &plan.vault_id, &credential.binding, &token, &action_id);
        outcome.action_ids.push(action_id.clone());
        let mut report = vec![format!("rotate {}", credential.binding)];
        let one = One {
            facade,
            registry,
            lease_id: &open_lease,
            vault_id: &plan.vault_id,
            instances,
            options,
        };
        let ran = moqui_vault_server::lease::run_under(&open_lease, || one.rotate(credential, &mut report));
        let status = match ran {
            Ok(revision) => {
                outcome.rotated.push((credential.binding.clone(), revision));
                actions::DONE
            }
            Err(err) => {
                tracing::error!("Instances: the rotation of {} failed: {err}", credential.binding);
                report.push(format!("failed: {}", err.message()));
                outcome
                    .failed
                    .push((credential.binding.clone(), err.message().to_string()));
                actions::FAILED
            }
        };
        if let Err(err) = actions::finish(facade, &action_id, status, &report.join("\n")) {
            tracing::error!("Instances: the rotation action {action_id} was not updated: {err}");
        }
        claim::release(
            facade,
            &plan.vault_id,
            &credential.binding,
            &token,
            Some((status, &action_id)),
        );
        outcome.report.extend(report);
    }
    actions::end_lease(facade, registry, &opened);
    Ok(outcome)
}

/// What one credential's rotation runs with.
struct One<'a> {
    facade: &'a EntityFacade,
    registry: &'a LeaseRegistry,
    lease_id: &'a str,
    vault_id: &'a str,
    instances: &'a dyn Instances,
    options: &'a Options,
}

impl One<'_> {
    /// Rotates one credential under the lease (see the module doc): its new revision.
    fn rotate(&self, credential: &Credential, report: &mut Vec<String>) -> Result<i64, LibError> {
        let instances = self.instances;
        // 0. each instance that uses it is up and unsealed now, before anything changes
        for used in &credential.uses {
            let status = instances.status(&used.instance_id)?;
            if !status.ready() {
                return Err(fail(format!(
                    "AppInstance {} is {} ({}): a rotated value goes only to an instance that is up and unsealed; \
                     nothing was changed",
                    used.instance_id, status.seal_state, status.health
                )));
            }
        }
        // the revision it starts from: its new value becomes the next only from this one
        let expected = one(self.facade, VAULT_ITEM, "vaultItemId", &credential.item_id)?
            .map(|row| field(&row, "revision"))
            .and_then(|revision| revision.parse::<i64>().ok())
            .ok_or_else(|| fail(format!("No item {}", credential.item_id)))?;
        // the item's value it replaces and its user, from the lease
        let fields = match moqui_core::vault_hooks::leased(&credential.binding, None)? {
            Value::Map(map) => map,
            _ => return Err(fail(format!("The item bound to {} has no fields", credential.binding))),
        };
        let text_of = |name: &str| Zeroizing::new(fields.get(name).map(to_plain_string).unwrap_or_default());
        let old = text_of(&credential.field_name);
        let user = text_of("username");
        drop(fields);
        if old.is_empty() {
            return Err(fail(format!(
                "The item bound to {} has no {}",
                credential.binding, credential.field_name
            )));
        }
        if let Source::DatabaseAdmin(network) = &credential.source {
            // the login goes only to the server its own item names
            source::own_admin(network, &credential.binding)?;
        }
        // the old value can go back where the new one goes, or nothing starts
        source::check_put_back(&credential.source, &user, &old)?;
        let new = crate::kinds::credential_for(&credential.source_kind);
        // the new value is deposited before the source changes, so it is never only in this process
        let pending = self.registry.stage_rotation(
            self.facade,
            self.lease_id,
            &credential.binding,
            &credential.field_name,
            &new,
            expected,
        )?;
        report.push(format!(
            "staged the new value of {} as pending revision {pending}",
            credential.binding
        ));
        // 1. the source
        if let Err(err) = source::set(&credential.source, &user, &old, &new) {
            // a set can fail after part of it applied (MySQL's admin is two users): the old value is set again where the
            // source takes it; where that fails too, the source may hold the new one, which stays pending
            let back = source::set(&credential.source, &user, &new, &old).is_ok();
            if back {
                report.push(format!(
                    "set the old value of {} again at its source",
                    credential.binding
                ));
            }
            self.settle(credential, &pending, back, report);
            return Err(fail(format!(
                "the new value was not set at its source: {}",
                err.message()
            )));
        }
        report.push(format!("set the new value of {} at its source", credential.binding));
        // 2. each instance that uses it, then 3. the check, then 4. the item; a failure puts the old value back
        let mut took: Vec<&Use> = Vec::new();
        let changed = (|| -> Result<i64, LibError> {
            for used in &credential.uses {
                instances.put_secret(&used.instance_id, used.name, &new)?;
                took.push(used);
                report.push(format!("put {} on AppInstance {}", used.name, used.instance_id));
                instances.restart(&used.instance_id)?;
                report.push(format!("restarted AppInstance {}", used.instance_id));
            }
            report.extend(source::check(&credential.source, &user, &new)?);
            let ids: Vec<String> = credential.uses.iter().map(|used| used.instance_id.clone()).collect();
            wait_ready(instances, &ids, self.options.wait)?;
            if !ids.is_empty() {
                report.push(format!("up and unsealed: {}", ids.join(", ")));
            }
            let revision = moqui_vault_server::deposit::promote_revision(
                self.facade,
                self.vault_id,
                &credential.item_id,
                &credential.binding,
                &pending,
            )?;
            report.push(format!(
                "deposited revision {revision} of {}; the one before is in its history, and a member takes the new one \
                 in",
                credential.binding
            ));
            Ok(revision)
        })();
        match changed {
            Ok(revision) => Ok(revision),
            Err(err) => {
                let back = put_back(instances, credential, &user, &old, &new, &took, self.options, report);
                self.settle(credential, &pending, back, report);
                Err(fail(format!(
                    "{}; {}",
                    err.message(),
                    if back {
                        "the old value was put back"
                    } else {
                        "the old value was NOT put back everywhere, and the new one is kept as a pending revision"
                    }
                )))
            }
        }
    }

    /// A staged value after a failure: dropped when the old value is back everywhere, else kept for a member to
    /// recover (a revealer opens it with reveal#VaultItem's pendingId).
    fn settle(&self, credential: &Credential, pending: &str, back: bool, report: &mut Vec<String>) {
        let done = if back {
            moqui_vault_server::deposit::drop_pending(self.facade, &credential.item_id, pending)
        } else {
            report.push(format!(
                "the new value stays pending revision {pending} of {}: a member with the reveal role opens it to set \
                 it in the vault",
                credential.binding
            ));
            moqui_vault_server::deposit::keep_pending(
                self.facade,
                &credential.item_id,
                pending,
                "the rotation could not put the old value back everywhere: the source may hold this value",
            )
        };
        if let Err(err) = done {
            tracing::error!(
                "Instances: the pending revision {pending} of {} was not settled: {err}",
                credential.binding
            );
        }
    }
}

/// Puts the old value back at the source (signed in with the new one where the source needs it) and on each instance
/// that took the new one, which is restarted; each failure is reported, and the rest go on. Whether it is back
/// everywhere.
#[allow(clippy::too_many_arguments)]
fn put_back(
    instances: &dyn Instances,
    credential: &Credential,
    user: &str,
    old: &str,
    new: &str,
    took: &[&Use],
    options: &Options,
    report: &mut Vec<String>,
) -> bool {
    let mut back = true;
    match source::set(&credential.source, user, new, old) {
        Ok(lines) => {
            report.extend(lines);
            report.push(format!(
                "put the old value of {} back at its source",
                credential.binding
            ));
        }
        Err(err) => {
            back = false;
            tracing::error!(
                "Instances: the old value of {} was not put back at its source: {err}",
                credential.binding
            );
            report.push(format!(
                "NOT put back at its source: {}; the source may hold the new value, kept as a pending revision",
                err.message()
            ));
        }
    }
    let mut restarted = Vec::new();
    for used in took {
        let put = instances
            .put_secret(&used.instance_id, used.name, old)
            .and_then(|()| instances.restart(&used.instance_id));
        match put {
            Ok(()) => {
                report.push(format!("put the old value back on AppInstance {}", used.instance_id));
                restarted.push(used.instance_id.clone());
            }
            Err(err) => {
                back = false;
                tracing::error!(
                    "Instances: the old value of {} was not put back on AppInstance {}: {err}",
                    credential.binding,
                    used.instance_id
                );
                report.push(format!(
                    "NOT put back on AppInstance {}: {}",
                    used.instance_id,
                    err.message()
                ));
            }
        }
    }
    if let Err(err) = wait_ready(instances, &restarted, options.wait) {
        report.push(err.message().to_string());
    }
    back
}

// ----- asked for -----

/// `request#InstanceRotation`: rotates `bindings` of the environment now under its machine access, or asks for the
/// lease, claims each binding for it (one rotation of an item at a time) and runs the rotation on this member once an
/// approver opens it (its waiting action on the first credential's instance). What is not rotated, and why, comes back
/// beside it.
pub fn request(
    factory: &Arc<ExecutionContextFactory>,
    facade: &EntityFacade,
    registry: &Arc<LeaseRegistry>,
    environment_id: &str,
    bindings: &[String],
) -> Result<(Requested, Plan), LibError> {
    let plan = plan(facade, environment_id, bindings)?;
    let Some(first) = plan.credentials.first() else {
        let reasons: Vec<String> = plan
            .not_rotated
            .iter()
            .map(|not| format!("{}: {}", not.binding, not.reason))
            .collect();
        return Err(fail(format!("Nothing the manager can rotate: {}", reasons.join("; "))));
    };
    let environment = one(facade, ENVIRONMENT, "environmentId", environment_id)?
        .ok_or_else(|| fail(format!("No InstanceEnvironment {environment_id}")))?;
    actions::check_caller(&environment)?;
    if plan.machine_access {
        let outcome = run(
            facade,
            registry,
            &LinkInstances { facade },
            &plan,
            None,
            &Options::from_conf(),
        )?;
        let status = if outcome.failed.is_empty() {
            actions::DONE
        } else {
            actions::FAILED
        };
        return Ok((
            Requested {
                action_id: outcome.action_ids.first().cloned().unwrap_or_default(),
                status: status.to_string(),
                lease_id: None,
            },
            plan,
        ));
    }
    // a binding another rotation holds asks the approvers for nothing
    for credential in &plan.credentials {
        claim::check_free(facade, &plan.vault_id, &credential.binding)?;
    }
    let lease = registry.ask(
        facade,
        &Ask {
            vault_id: &plan.vault_id,
            bindings: &plan.bindings,
            operation: &plan.operation,
            limit: plan.lease_limit,
            environment_id: None,
        },
    )?;
    let lease_id = lease.lease_id.clone();
    // each binding is this request's while it waits: a second rotation of one is refused, and this one gives up its lease
    let mut claimed: Vec<&str> = Vec::new();
    for credential in &plan.credentials {
        if let Err(err) = claim::claim(facade, &plan.vault_id, &credential.binding, &lease_id, lease.expires_at) {
            for binding in claimed {
                claim::release(facade, &plan.vault_id, binding, &lease_id, None);
            }
            if let Err(ended) = registry.end(facade, &lease_id) {
                registry.forget(&lease_id);
                tracing::error!("Instances: the lease {lease_id} of a refused rotation was not ended: {ended}");
            }
            return Err(err);
        }
        claimed.push(&credential.binding);
    }
    let action_id = actions::record(
        facade,
        &first.instance_id,
        "rotate",
        actions::WAITING,
        Some(&lease_id),
        false,
        &format!(
            "waiting for an approver to open lease {lease_id}: {}",
            plan.credentials
                .iter()
                .map(|credential| credential.binding.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    )?;
    let limit = Duration::from_millis(u64::try_from(lease.expires_at - moqui_core::user::now_millis()).unwrap_or(0));
    let (environment, asked) = (environment_id.to_string(), bindings.to_vec());
    actions::wait_then_run(
        factory,
        registry,
        &action_id,
        &lease_id,
        limit,
        &format!("instance-rotate-{environment_id}"),
        move |facade, registry, lease_id| {
            // the plan again on the thread: a row changed since the ask makes another operation, which the lease refuses
            let plan = self::plan(facade, &environment, &asked)?;
            let outcome = run(
                facade,
                registry,
                &LinkInstances { facade },
                &plan,
                Some(lease_id),
                &Options::from_conf(),
            )?;
            if outcome.rotated.is_empty() {
                return Err(fail(outcome.summary()));
            }
            Ok(outcome.summary())
        },
    )?;
    Ok((
        Requested {
            action_id,
            status: actions::WAITING.to_string(),
            lease_id: Some(lease_id),
        },
        plan,
    ))
}
