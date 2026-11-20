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

//! What a provision makes (plans/Vault-Instances.md §4.3, steps 1 and 2): the instance and each dependency not made
//! yet, dependencies first; for each its container, volumes and network, the credentials it makes and deposits, the
//! database and user it makes on its database dependency, and the vault items its lease needs: a host's TLS client
//! key, a registry login, a database host's admin login, or the admin login of a database instance made before. The
//! preview shows this, and the provision runs it.

use std::time::Duration;

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{Value, ValueMap};

use crate::common::{
    fail, field, host_is_local, host_key_binding, host_needs_key, one, rows, APP_INSTANCE, DATABASE_HOST, ENVIRONMENT,
    HOST, IMAGE, INSTANCE_DEPENDENCY, INSTANCE_SECRET, INSTANCE_VOLUME, KIND,
};

/// How deep dependencies go: an instance, its database, nothing past that today; a cycle is refused before this.
const MAX_DEPTH: usize = 4;

/// Where a database's admin login comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminSource {
    /// Made by this provision for the dependency instance of this id, held in memory.
    Made(String),
    /// A vault item of the lease, by binding (field `username` and `password`).
    Leased(String),
}

/// The database a step makes on its database dependency, and how the manager reaches that server.
#[derive(Debug, Clone)]
pub struct DatabaseUse {
    pub conf_name: String,
    pub host: String,
    pub port: String,
    pub ssl_mode: String,
    /// The admin user of a server made here (postgres, root); a leased login names its own.
    pub admin_user: String,
    pub admin: AdminSource,
    pub database: String,
    pub user: String,
    /// The `AppInstanceSecret` that holds the user's password (made and deposited by this provision).
    pub password_secret: String,
    /// Wait for the server to take connections: it is made by this provision.
    pub wait: bool,
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
}

/// The operation a lease of an instance action names, which the action checks against the lease it is handed.
pub fn operation(action: &str, instance_id: &str) -> String {
    format!("{action} AppInstance {instance_id}")
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
    let mut bindings: Vec<String> = Vec::new();
    let mut add = |binding: String| {
        if !bindings.contains(&binding) {
            bindings.push(binding);
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
            add(host_key_binding(&host_id));
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
            add(format!("InstanceImage:{image_id}:registry"));
        }
        let secrets = rows(facade, INSTANCE_SECRET, "appInstanceId", &id)?;
        if let Some(leased) = secrets.iter().find(|secret| field(secret, "generated") != "Y") {
            add(field(leased, "bindingKey"));
        }
        let database = database_use(facade, &row, &made_here, &vault_id)?;
        if let Some(DatabaseUse {
            admin: AdminSource::Leased(binding),
            ..
        }) = &database
        {
            add(binding.clone());
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
    Ok(Plan {
        target: target_id.to_string(),
        environment_id,
        vault_id,
        machine_access: field(&environment, "machineAccess") == "Y",
        lease_limit: lease_limit(&environment),
        steps,
        lease_bindings: bindings,
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

/// The database `row` makes on its database dependency, if it has one and a password secret for it.
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
        let port = field(&other, "hostPort");
        if port.is_empty() {
            return Err(fail(format!(
                "AppInstance {id}'s database is made on AppInstance {other_id}, which publishes no hostPort for the manager to reach"
            )));
        }
        let address = if host_is_local(&host) {
            "127.0.0.1".to_string()
        } else {
            field(&host, "hostAddress")
        };
        let conf_name = field(&kind, "confName");
        let admin = if made_here.contains(&other_id) {
            AdminSource::Made(other_id.clone())
        } else {
            AdminSource::Leased(format!("AppInstance:{other_id}:admin"))
        };
        return Ok(Some(DatabaseUse {
            admin_user: if conf_name == "postgres" { "postgres" } else { "root" }.to_string(),
            conf_name,
            host: address,
            port,
            ssl_mode: "disable".to_string(),
            wait: made_here.contains(&other_id),
            admin,
            database,
            user,
            password_secret,
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
    Ok(Some(DatabaseUse {
        conf_name: field(&kind, "confName"),
        host: field(&database_host, "hostAddress"),
        port: field(&database_host, "hostPort"),
        ssl_mode: String::new(),
        admin_user: String::new(),
        admin: AdminSource::Leased(format!("DatabaseHost:{host_id}:admin")),
        database,
        user,
        password_secret,
        wait: false,
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
    preview.insert("machineAccess".to_string(), Value::Bool(plan.machine_access));
    preview.insert("operation".to_string(), text(operation("provision", &plan.target)));
    Ok(preview)
}
