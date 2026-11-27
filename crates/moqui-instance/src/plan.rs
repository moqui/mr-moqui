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

//! What a provision makes (plans/archive/Vault-Instances.md §4.3, steps 1 and 2): the instance and each dependency not made
//! yet, dependencies first; for each its container, volumes and network, the credentials it makes and deposits, the
//! database and user it makes on its database dependency, and the vault items its lease needs: a host's TLS client
//! key, a registry login, a database host's admin login, or the admin login of a database instance made before. The
//! preview shows this, and the provision runs it.
//!
//! **Where each login goes is in the lease.** The operation a lease names, which the server signs and the approver
//! reads, lists each leased item with its destination (`DatabaseHost:db1:admin to db.example.com:5432`), and each use
//! checks the item's own destination against the row: a row changed after the approval, or an item whose own host or
//! registry is not the row's, is refused ([`Destination`]).

use std::time::Duration;

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{Value, ValueMap};

use crate::common::{
    fail, field, host_key_binding, host_needs_key, one, rows, APP_INSTANCE, DATABASE_HOST, ENVIRONMENT, HOST, IMAGE,
    INSTANCE_DEPENDENCY, INSTANCE_SECRET, INSTANCE_VOLUME, KIND,
};

/// How deep dependencies go: an instance, its database, nothing past that today; a cycle is refused before this.
const MAX_DEPTH: usize = 4;
/// The longest operation a lease names (the vault's limit).
const MAX_OPERATION: usize = 255;

/// Where a database's admin login comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminSource {
    /// Made by this provision for the dependency instance of this id, held in memory.
    Made(String),
    /// A vault item of the lease, by binding (fields `username`, `password`, `host`, `port`).
    Leased(String),
}

/// How the database and user are made on the server.
#[derive(Debug, Clone)]
pub enum Reach {
    /// A database instance: a one-shot client container on its network, on its host, reads the logins from secret
    /// files and makes them there. Nothing crosses another network, and the server publishes no port for it.
    Network {
        /// The `InstanceHost` row of the server's host.
        host: EntityValue,
        network: String,
        /// The server's image, whose client the one-shot runs.
        image: String,
        /// The server's container name on the network.
        server: String,
        port: String,
    },
    /// A database host: the manager connects over TLS that checks the server's certificate and name.
    Direct { host: String, port: String },
}

/// The database a step makes on its database dependency.
#[derive(Debug, Clone)]
pub struct DatabaseUse {
    pub conf_name: String,
    pub reach: Reach,
    /// The admin user of a server made here (postgres, root); a leased login names its own.
    pub admin_user: String,
    pub admin: AdminSource,
    /// The server the item must name (`host`, `port` of a leased admin login), checked at its use.
    pub expect_host: String,
    pub expect_port: String,
    pub database: String,
    pub user: String,
    /// The `AppInstanceSecret` that holds the user's password (made and deposited by this provision).
    pub password_secret: String,
    /// The server is made by this provision: wait for it to take connections, and on a failure it goes with the rest.
    pub made_here: bool,
    /// What the instance's container reaches the server as: its name on the network, or the host's address.
    pub container_host: String,
    pub container_port: String,
    /// What the server is, for the preview: an instance's or a database host's id.
    pub on: String,
}

/// One instance the provision makes.
#[derive(Debug, Clone)]
pub struct Step {
    pub instance: EntityValue,
    pub kind: EntityValue,
    pub host: EntityValue,
    pub image: EntityValue,
    /// Its `AppInstanceSecret` rows.
    pub secrets: Vec<EntityValue>,
    pub database: Option<DatabaseUse>,
}

impl Step {
    pub fn id(&self) -> String {
        field(&self.instance, "appInstanceId")
    }
}

/// A leased item and where its value goes, in the lease's operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    pub binding: String,
    pub to: String,
}

/// A provision's plan.
#[derive(Debug, Clone)]
pub struct Plan {
    pub target: String,
    pub environment_id: String,
    pub vault_id: String,
    pub machine_access: bool,
    pub lease_limit: Option<Duration>,
    /// The instances to make, dependencies first; empty when the target is made already.
    pub steps: Vec<Step>,
    /// The vault items the provision opens, by binding: what its lease must name.
    pub lease_bindings: Vec<String>,
    /// Where each leased item goes.
    pub destinations: Vec<Destination>,
    /// The operation its lease names: the action, the instance and each destination.
    pub operation: String,
}

/// The operation a lease of an instance action names, which the action checks against the lease it is handed: the
/// action, the instance and where each leased item goes. Refused past the vault's 255 characters.
pub fn operation(action: &str, instance_id: &str, destinations: &[Destination]) -> Result<String, LibError> {
    let mut text = format!("{action} AppInstance {instance_id}");
    if !destinations.is_empty() {
        let to: Vec<String> = destinations
            .iter()
            .map(|destination| format!("{} to {}", destination.binding, destination.to))
            .collect();
        text = format!("{text}; {}", to.join(", "));
    }
    if text.len() > MAX_OPERATION || text.chars().any(char::is_control) {
        return Err(fail(format!(
            "The lease of {action} AppInstance {instance_id} would name {} characters of destinations, more than a lease \
             holds ({MAX_OPERATION}), or a control character: provision its dependencies first",
            text.len()
        )));
    }
    Ok(text)
}

/// Where a host's client key goes: the Docker host it signs in to.
pub fn host_destination(host: &EntityValue) -> Destination {
    let port = Some(field(host, "adminPort"))
        .filter(|port| !port.is_empty())
        .unwrap_or_else(|| "2375".to_string());
    Destination {
        binding: host_key_binding(&field(host, "instanceHostId")),
        to: format!("docker host {}:{port}", field(host, "hostAddress")),
    }
}

/// An instance's environment row, with its vault: an instance with none is not managed under a vault.
pub fn environment_of(facade: &EntityFacade, instance: &EntityValue) -> Result<EntityValue, LibError> {
    let id = field(instance, "environmentId");
    let environment = one(facade, ENVIRONMENT, "environmentId", &id)?.ok_or_else(|| {
        fail(format!(
            "AppInstance {} names no InstanceEnvironment: an instance is provisioned in an environment, whose vault holds its items",
            field(instance, "appInstanceId")
        ))
    })?;
    if field(&environment, "vaultId").is_empty() {
        return Err(fail(format!("InstanceEnvironment {id} names no operational vault")));
    }
    Ok(environment)
}

/// A lease's time limit for an environment's operations, when it sets one.
pub fn lease_limit(environment: &EntityValue) -> Option<Duration> {
    field(environment, "leaseLimitMinutes")
        .parse::<u64>()
        .ok()
        .filter(|minutes| *minutes > 0)
        .map(|minutes| Duration::from_secs(minutes * 60))
}

/// Refuses a host, image or database host whose items are in another vault than the environment's: a lease is of one
/// vault.
fn same_vault(what: &str, row: &EntityValue, vault_id: &str) -> Result<(), LibError> {
    let named = field(row, "vaultId");
    if !named.is_empty() && named != vault_id {
        return Err(fail(format!(
            "{what} keeps its items in vault {named}, not the environment's vault {vault_id}: a provision's lease is of one vault"
        )));
    }
    Ok(())
}

pub(crate) fn instance(facade: &EntityFacade, id: &str) -> Result<EntityValue, LibError> {
    one(facade, APP_INSTANCE, "appInstanceId", id)?.ok_or_else(|| fail(format!("No AppInstance {id}")))
}

/// Whether an instance has its container: it was provisioned.
pub fn is_made(instance: &EntityValue) -> bool {
    !field(instance, "instanceUuid").is_empty()
}

/// A registry's name as compared: no scheme, no trailing slash, lower case.
pub fn registry_name(location: &str) -> String {
    let location = location.trim();
    let location = location
        .strip_prefix("https://")
        .or_else(|| location.strip_prefix("http://"))
        .unwrap_or(location);
    location.trim_end_matches('/').to_ascii_lowercase()
}

/// The registry an image reference pulls from: its first part when that names a host (a dot, a port or `localhost`),
/// else Docker Hub. `X-Registry-Auth` goes there, whatever a row says.
pub fn image_registry(image: &str) -> String {
    match image.split_once('/') {
        Some((first, _)) if first.contains(['.', ':']) || first == "localhost" => registry_name(first),
        _ => "docker.io".to_string(),
    }
}

/// An image with a registry login pulls from the registry its row names: the login goes to the registry the image's
/// name says, so the two must be one.
pub fn check_registry(image: &EntityValue) -> Result<(), LibError> {
    let location = registry_name(&field(image, "registryLocation"));
    let pulls_from = image_registry(&field(image, "imageName"));
    if location.is_empty() || location != pulls_from {
        return Err(fail(format!(
            "InstanceImage {} pulls from {pulls_from}, not its registryLocation {location:?}: its registry login would go \
             to {pulls_from} (reasonCode registryMismatch)",
            field(image, "instanceImageId")
        )));
    }
    Ok(())
}

/// The plan of a provision of `target_id`.
pub fn build(facade: &EntityFacade, target_id: &str) -> Result<Plan, LibError> {
    let target = instance(facade, target_id)?;
    let environment = environment_of(facade, &target)?;
    let environment_id = field(&environment, "environmentId");
    let vault_id = field(&environment, "vaultId");
    // dependencies first, depth first, each once
    let mut order: Vec<EntityValue> = Vec::new();
    visit(facade, &target, &environment_id, &mut Vec::new(), &mut order)?;
    let made_here: Vec<String> = order
        .iter()
        .filter(|row| !is_made(row))
        .map(|row| field(row, "appInstanceId"))
        .collect();
    let mut steps = Vec::new();
    let mut destinations: Vec<Destination> = Vec::new();
    let mut add = |destination: Destination| {
        if !destinations.iter().any(|known| known.binding == destination.binding) {
            destinations.push(destination);
        }
    };
    for row in order.into_iter().filter(|row| !is_made(row)) {
        let id = field(&row, "appInstanceId");
        let kind_id = field(&row, "instanceKindId");
        let kind = one(facade, KIND, "instanceKindId", &kind_id)?
            .ok_or_else(|| fail(format!("AppInstance {id}: no InstanceKind {kind_id}")))?;
        let host_id = field(&row, "instanceHostId");
        let host = one(facade, HOST, "instanceHostId", &host_id)?
            .ok_or_else(|| fail(format!("AppInstance {id}: no InstanceHost {host_id}")))?;
        same_vault(&format!("InstanceHost {host_id}"), &host, &vault_id)?;
        if host_needs_key(&host) {
            add(host_destination(&host));
        }
        let image_id = field(&row, "instanceImageId");
        let image = one(facade, IMAGE, "instanceImageId", &image_id)?
            .ok_or_else(|| fail(format!("AppInstance {id}: no InstanceImage {image_id}")))?;
        if field(&image, "instanceKindId") != kind_id {
            return Err(fail(format!(
                "AppInstance {id}: InstanceImage {image_id} is not of kind {kind_id}"
            )));
        }
        same_vault(&format!("InstanceImage {image_id}"), &image, &vault_id)?;
        if field(&image, "registryLogin") == "Y" {
            check_registry(&image)?;
            add(Destination {
                binding: format!("InstanceImage:{image_id}:registry"),
                to: registry_name(&field(&image, "registryLocation")),
            });
        }
        let secrets = rows(facade, INSTANCE_SECRET, "appInstanceId", &id)?;
        for leased in secrets.iter().filter(|secret| field(secret, "generated") != "Y") {
            add(Destination {
                binding: field(leased, "bindingKey"),
                to: format!("container {}", field(&row, "instanceName")),
            });
        }
        let database = database_use(facade, &row, &made_here, &vault_id)?;
        if let Some(database) = &database {
            if let AdminSource::Leased(binding) = &database.admin {
                add(Destination {
                    binding: binding.clone(),
                    to: format!("{}:{}", database.expect_host, database.expect_port),
                });
            }
            if let Reach::Network { host, .. } = &database.reach {
                if host_needs_key(host) {
                    add(host_destination(host));
                }
            }
        }
        steps.push(Step {
            instance: row,
            kind,
            host,
            image,
            secrets,
            database,
        });
    }
    let operation = operation("provision", target_id, &destinations)?;
    Ok(Plan {
        target: target_id.to_string(),
        environment_id,
        vault_id,
        machine_access: field(&environment, "machineAccess") == "Y",
        lease_limit: lease_limit(&environment),
        steps,
        lease_bindings: destinations
            .iter()
            .map(|destination| destination.binding.clone())
            .collect(),
        destinations,
        operation,
    })
}

fn visit(
    facade: &EntityFacade,
    row: &EntityValue,
    environment_id: &str,
    path: &mut Vec<String>,
    order: &mut Vec<EntityValue>,
) -> Result<(), LibError> {
    let id = field(row, "appInstanceId");
    if path.contains(&id) {
        return Err(fail(format!(
            "AppInstance {id} depends on itself through {}",
            path.join(" -> ")
        )));
    }
    if path.len() >= MAX_DEPTH {
        return Err(fail(format!(
            "AppInstance {id}: dependencies go deeper than {MAX_DEPTH}"
        )));
    }
    if order.iter().any(|done| field(done, "appInstanceId") == id) {
        return Ok(());
    }
    if field(row, "environmentId") != environment_id {
        return Err(fail(format!(
            "AppInstance {id} is in environment {}, not {environment_id}: an instance's dependencies are in its environment",
            field(row, "environmentId")
        )));
    }
    path.push(id.clone());
    for dependency in rows(facade, INSTANCE_DEPENDENCY, "appInstanceId", &id)? {
        let other = field(&dependency, "dependsOnInstanceId");
        if other.is_empty() {
            continue;
        }
        let other = instance(facade, &other)?;
        visit(facade, &other, environment_id, path, order)?;
    }
    path.pop();
    order.push(row.clone());
    Ok(())
}

/// The database an instance has on its database dependency, as an action after its provision sees it: the admin login
/// leased, and where it goes.
pub fn database_of(facade: &EntityFacade, instance_id: &str) -> Result<Option<DatabaseUse>, LibError> {
    let row = instance(facade, instance_id)?;
    let environment = environment_of(facade, &row)?;
    database_use(facade, &row, &[], &field(&environment, "vaultId"))
}

/// The database `row` makes on its database dependency, if it has one.
fn database_use(
    facade: &EntityFacade,
    row: &EntityValue,
    made_here: &[String],
    vault_id: &str,
) -> Result<Option<DatabaseUse>, LibError> {
    let id = field(row, "appInstanceId");
    let Some(dependency) = rows(facade, INSTANCE_DEPENDENCY, "appInstanceId", &id)?
        .into_iter()
        .find(|dependency| field(dependency, "dependencyRole") == "database")
    else {
        return Ok(None);
    };
    let database = field(row, "databaseName");
    let user = field(row, "databaseUser");
    if database.is_empty() || user.is_empty() {
        return Err(fail(format!(
            "AppInstance {id} has a database dependency and no databaseName or databaseUser"
        )));
    }
    let password_secret = "dbPassword".to_string();
    let other_id = field(&dependency, "dependsOnInstanceId");
    if !other_id.is_empty() {
        let other = instance(facade, &other_id)?;
        let kind = one(facade, KIND, "instanceKindId", &field(&other, "instanceKindId"))?
            .ok_or_else(|| fail(format!("AppInstance {other_id} has no InstanceKind")))?;
        let host = one(facade, HOST, "instanceHostId", &field(&other, "instanceHostId"))?
            .ok_or_else(|| fail(format!("AppInstance {other_id} has no InstanceHost")))?;
        if field(&other, "instanceHostId") != field(row, "instanceHostId") {
            return Err(fail(format!(
                "AppInstance {id}'s database, AppInstance {other_id}, is on another host: an instance reaches its \
                 database instance on its host's network"
            )));
        }
        let image = one(facade, IMAGE, "instanceImageId", &field(&other, "instanceImageId"))?
            .ok_or_else(|| fail(format!("AppInstance {other_id} has no InstanceImage")))?;
        let conf_name = field(&kind, "confName");
        let server = field(&other, "instanceName");
        let port = field(&kind, "containerPort");
        let admin = if made_here.contains(&other_id) {
            AdminSource::Made(other_id.clone())
        } else {
            AdminSource::Leased(format!("AppInstance:{other_id}:admin"))
        };
        return Ok(Some(DatabaseUse {
            admin_user: if conf_name == "postgres" { "postgres" } else { "root" }.to_string(),
            conf_name,
            reach: Reach::Network {
                host,
                network: field(&other, "networkName"),
                image: field(&image, "imageName"),
                server: server.clone(),
                port: port.clone(),
            },
            expect_host: server.clone(),
            expect_port: port.clone(),
            made_here: made_here.contains(&other_id),
            admin,
            database,
            user,
            password_secret,
            container_host: server,
            container_port: port,
            on: format!("AppInstance {other_id}"),
        }));
    }
    let host_id = field(&dependency, "databaseHostId");
    let database_host = one(facade, DATABASE_HOST, "databaseHostId", &host_id)?.ok_or_else(|| {
        fail(format!(
            "AppInstance {id}'s database dependency names no instance or DatabaseHost"
        ))
    })?;
    same_vault(&format!("DatabaseHost {host_id}"), &database_host, vault_id)?;
    let kind = one(facade, KIND, "instanceKindId", &field(&database_host, "instanceKindId"))?
        .ok_or_else(|| fail(format!("DatabaseHost {host_id} has no InstanceKind")))?;
    let address = field(&database_host, "hostAddress");
    let port = field(&database_host, "hostPort");
    Ok(Some(DatabaseUse {
        conf_name: field(&kind, "confName"),
        reach: Reach::Direct {
            host: address.clone(),
            port: port.clone(),
        },
        admin_user: String::new(),
        admin: AdminSource::Leased(format!("DatabaseHost:{host_id}:admin")),
        expect_host: address.clone(),
        expect_port: port.clone(),
        database,
        user,
        password_secret,
        made_here: false,
        container_host: Some(field(&database_host, "instanceAddress"))
            .filter(|text| !text.is_empty())
            .unwrap_or(address),
        container_port: port,
        on: format!("DatabaseHost {host_id}"),
    }))
}

/// The preview (§4.3, step 2): what will be made, and the lease it needs. No value of any kind.
pub fn preview(facade: &EntityFacade, plan: &Plan) -> Result<ValueMap, LibError> {
    fn text(given: impl AsRef<str>) -> Value {
        Value::string(given.as_ref())
    }
    let mut containers = Vec::new();
    let mut volumes = Vec::new();
    let mut networks: Vec<Value> = Vec::new();
    let mut databases = Vec::new();
    let mut deposits = Vec::new();
    let mut host_names = Vec::new();
    for step in &plan.steps {
        let id = step.id();
        let mut container = ValueMap::default();
        container.insert("appInstanceId".to_string(), text(&id));
        container.insert("name".to_string(), text(field(&step.instance, "instanceName")));
        container.insert("image".to_string(), text(field(&step.image, "imageName")));
        container.insert("instanceHostId".to_string(), text(field(&step.host, "instanceHostId")));
        container.insert("kind".to_string(), text(field(&step.kind, "instanceKindId")));
        containers.push(Value::Map(container));
        for volume in rows(facade, INSTANCE_VOLUME, "appInstanceId", &id)? {
            volumes.push(text(field(&volume, "volumeName")));
        }
        let network = text(field(&step.instance, "networkName"));
        if !moqui_lib::object::is_empty(&network) && !networks.contains(&network) {
            networks.push(network);
        }
        for secret in step.secrets.iter().filter(|secret| field(secret, "generated") == "Y") {
            let mut deposit = ValueMap::default();
            deposit.insert("bindingKey".to_string(), text(field(secret, "bindingKey")));
            deposit.insert("vaultItemTypeId".to_string(), text(field(secret, "vaultItemTypeId")));
            deposit.insert("file".to_string(), text(field(secret, "fileName")));
            deposits.push(Value::Map(deposit));
        }
        if let Some(database) = &step.database {
            let mut made = ValueMap::default();
            made.insert("database".to_string(), text(&database.database));
            made.insert("user".to_string(), text(&database.user));
            made.insert("on".to_string(), text(&database.on));
            databases.push(Value::Map(made));
        }
        let host_name = field(&step.instance, "hostName");
        if !host_name.is_empty() {
            host_names.push(text(host_name));
        }
    }
    let mut preview = ValueMap::default();
    preview.insert("appInstanceId".to_string(), text(&plan.target));
    preview.insert("environmentId".to_string(), text(&plan.environment_id));
    preview.insert("vaultId".to_string(), text(&plan.vault_id));
    preview.insert("containers".to_string(), Value::List(containers));
    preview.insert("volumes".to_string(), Value::List(volumes));
    preview.insert("networks".to_string(), Value::List(networks));
    preview.insert("databases".to_string(), Value::List(databases));
    preview.insert("deposits".to_string(), Value::List(deposits));
    preview.insert("hostNames".to_string(), Value::List(host_names));
    preview.insert(
        "leaseBindings".to_string(),
        Value::List(plan.lease_bindings.iter().map(text).collect()),
    );
    preview.insert(
        "destinations".to_string(),
        Value::List(
            plan.destinations
                .iter()
                .map(|destination| text(format!("{} to {}", destination.binding, destination.to)))
                .collect(),
        ),
    );
    preview.insert("machineAccess".to_string(), Value::Bool(plan.machine_access));
    preview.insert("operation".to_string(), text(&plan.operation));
    Ok(preview)
}
