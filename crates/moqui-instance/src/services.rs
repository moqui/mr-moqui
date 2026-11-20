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

//! The Rust services of `org.moqui.dev.InstanceServices` (framework/service/org/moqui/dev/InstanceServices.xml): the
//! configuration rows (environments, clusters, hosts, images, database hosts, instances made from their kind), the
//! preview and the actions, the `docker` host type's container services, the database kinds' `create#Database` and
//! `check#Database`, and the reconcile job. Each needs the sealed INSTANCE_ADMIN unless authorization is disabled
//! (plans/Security-Review.md SEC-TLS-02).

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_core::context::ExecutionContext;
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::extension::RustServiceFn;
use moqui_lib::docker::valid_name;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use crate::common::{
    authz_off, fail, field, one, out, require_instance_admin, required, rows, text, Out, Params, APP_INSTANCE,
    DATABASE_HOST, ENVIRONMENT, HOST, HOST_TYPE, IMAGE, INSTANCE_DEPENDENCY, INSTANCE_ENV, INSTANCE_HOST_CONFIG,
    INSTANCE_SECRET, INSTANCE_VOLUME, KIND, KIND_DEPENDENCY, KIND_ENV, KIND_HOST_CONFIG, KIND_SECRET, KIND_VOLUME,
};
use crate::kinds;
use crate::plan::{self, environment_of, instance};

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    moqui_core::rust_service!(map, store_instance_environment)?;
    moqui_core::rust_service!(map, store_instance_cluster)?;
    moqui_core::rust_service!(map, store_instance_host)?;
    moqui_core::rust_service!(map, store_instance_image)?;
    moqui_core::rust_service!(map, store_database_host)?;
    moqui_core::rust_service!(map, create_app_instance)?;
    moqui_core::rust_service!(map, preview_app_instance_provision)?;
    moqui_core::rust_service!(map, request_instance_action)?;
    moqui_core::rust_service!(map, provision_app_instance)?;
    moqui_core::rust_service!(map, start_app_instance)?;
    moqui_core::rust_service!(map, stop_app_instance)?;
    moqui_core::rust_service!(map, restart_app_instance)?;
    moqui_core::rust_service!(map, remove_app_instance)?;
    moqui_core::rust_service!(map, check_app_instance)?;
    moqui_core::rust_service!(map, init_instance_container)?;
    moqui_core::rust_service!(map, start_instance_container)?;
    moqui_core::rust_service!(map, stop_instance_container)?;
    moqui_core::rust_service!(map, remove_instance_container)?;
    moqui_core::rust_service!(map, check_instance_container)?;
    moqui_core::rust_service!(map, create_database_postgres)?;
    moqui_core::rust_service!(map, create_database_mysql)?;
    moqui_core::rust_service!(map, check_database_postgres)?;
    moqui_core::rust_service!(map, check_database_mysql)?;
    moqui_core::rust_service!(map, reconcile_app_instances)?;
    moqui_core::rust_service!(map, get_instance_fleet)?;
    moqui_core::rust_service!(map, run_instance_recovery)?;
    Ok(())
}

fn factory() -> Result<Arc<moqui_core::context::ExecutionContextFactory>, LibError> {
    ExecutionContext::with_current(|ec| Arc::clone(ec.factory())).ok_or_else(|| fail("No execution context is entered"))
}

fn registry() -> Result<Arc<moqui_vault_server::lease::LeaseRegistry>, LibError> {
    let factory = factory()?;
    Ok(moqui_vault_server::lease::registry(&factory))
}

fn flag(params: &Params, name: &str) -> bool {
    matches!(
        text(params, name).to_ascii_lowercase().as_str(),
        "y" | "yes" | "true" | "1"
    )
}

// ----- configuration rows -----

/// Stores a row of `entity` from the parameters: its id (`key`, made from `fallback` when not given), and each of
/// `fields` given; an empty parameter clears its field. Its id.
fn store(
    facade: &EntityFacade,
    params: &Params,
    entity: &str,
    key: &str,
    fields: &[&str],
    check: &dyn Fn(&EntityValue) -> Result<(), LibError>,
) -> Result<String, LibError> {
    require_instance_admin()?;
    let _authz = authz_off();
    let id = required(params, key)?;
    if !valid_name(&id) {
        return Err(fail(format!(
            "{key} {id:?} is not a name: letters, digits, '_', '.' and '-'"
        )));
    }
    let existing = one(facade, entity, key, &id)?;
    let mut row = match &existing {
        Some(row) => row.clone(),
        None => {
            let mut row = facade.make_value(entity)?;
            row.set(key, Value::string(&id));
            row
        }
    };
    for name in fields {
        if params.contains_key(*name) {
            let given = text(params, name);
            row.set(
                *name,
                if given.is_empty() {
                    Value::Null
                } else {
                    Value::string(given)
                },
            );
        }
    }
    check(&row)?;
    if existing.is_some() {
        row.update(facade)?;
    } else {
        row.create(facade)?;
    }
    Ok(id)
}

/// A vault a row names is an operational vault.
fn check_vault(facade: &EntityFacade, row: &EntityValue) -> Result<(), LibError> {
    let vault_id = field(row, "vaultId");
    if vault_id.is_empty() {
        return Ok(());
    }
    let vault = one(facade, "moqui.security.vault.Vault", "vaultId", &vault_id)?
        .ok_or_else(|| fail(format!("No vault {vault_id}")))?;
    if field(&vault, "vaultKindEnumId") != "VkOperational" {
        return Err(fail(format!("Vault {vault_id} is not an operational vault")));
    }
    Ok(())
}

fn check_indicator(row: &EntityValue, name: &str) -> Result<(), LibError> {
    match field(row, name).as_str() {
        "" | "Y" | "N" => Ok(()),
        other => Err(fail(format!("{name} is Y or N, not {other}"))),
    }
}

/// `store#InstanceEnvironment`.
fn store_instance_environment(params: &Params, facade: &EntityFacade) -> Out {
    let id = store(
        facade,
        params,
        ENVIRONMENT,
        "environmentId",
        &[
            "environmentName",
            "environmentTypeEnumId",
            "vaultId",
            "approverUserGroupId",
            "requiredPipeline",
            "machineAccess",
            "unsealPolicyEnumId",
            "unsealMethod",
            "transportEnumId",
            "leaseLimitMinutes",
        ],
        &|row| {
            check_vault(facade, row)?;
            check_indicator(row, "machineAccess")
        },
    )?;
    Ok(out([("environmentId", Value::string(id))]))
}

/// `store#InstanceCluster`.
fn store_instance_cluster(params: &Params, facade: &EntityFacade) -> Out {
    let id = store(
        facade,
        params,
        "moqui.server.instance.InstanceCluster",
        "clusterId",
        &["environmentId", "clusterName"],
        &|row| {
            let environment = field(row, "environmentId");
            if one(facade, ENVIRONMENT, "environmentId", &environment)?.is_none() {
                return Err(fail(format!("No InstanceEnvironment {environment}")));
            }
            Ok(())
        },
    )?;
    Ok(out([("clusterId", Value::string(id))]))
}

/// `store#InstanceHost`.
fn store_instance_host(params: &Params, facade: &EntityFacade) -> Out {
    let id = store(
        facade,
        params,
        HOST,
        "instanceHostId",
        &[
            "hostTypeId",
            "hostProtocol",
            "hostAddress",
            "adminPort",
            "tlsCaCert",
            "tlsClientCert",
            "vaultId",
            "environmentId",
        ],
        &|row| {
            let host_type = field(row, "hostTypeId");
            if one(facade, HOST_TYPE, "hostTypeId", &host_type)?.is_none() {
                return Err(fail(format!("No InstanceHostType {host_type}")));
            }
            match field(row, "hostProtocol").to_ascii_lowercase().as_str() {
                "unix" | "http" | "https" => {}
                other => return Err(fail(format!("hostProtocol {other} is not unix, http or https"))),
            }
            let environment = field(row, "environmentId");
            if !environment.is_empty() && one(facade, ENVIRONMENT, "environmentId", &environment)?.is_none() {
                return Err(fail(format!("No InstanceEnvironment {environment}")));
            }
            check_vault(facade, row)
        },
    )?;
    Ok(out([("instanceHostId", Value::string(id))]))
}

/// `store#InstanceImage`.
fn store_instance_image(params: &Params, facade: &EntityFacade) -> Out {
    let id = store(
        facade,
        params,
        IMAGE,
        "instanceImageId",
        &[
            "instanceKindId",
            "imageName",
            "registryLocation",
            "registryLogin",
            "vaultId",
        ],
        &|row| {
            let kind = field(row, "instanceKindId");
            if one(facade, KIND, "instanceKindId", &kind)?.is_none() {
                return Err(fail(format!("No InstanceKind {kind}")));
            }
            let image = field(row, "imageName");
            if image.is_empty() || image.contains(char::is_whitespace) {
                return Err(fail(format!("{image:?} is not an image")));
            }
            check_indicator(row, "registryLogin")?;
            check_vault(facade, row)
        },
    )?;
    Ok(out([("instanceImageId", Value::string(id))]))
}

/// `store#DatabaseHost`.
fn store_database_host(params: &Params, facade: &EntityFacade) -> Out {
    let id = store(
        facade,
        params,
        DATABASE_HOST,
        "databaseHostId",
        &[
            "instanceKindId",
            "hostAddress",
            "hostPort",
            "instanceAddress",
            "vaultId",
        ],
        &|row| {
            let kind = field(row, "instanceKindId");
            let kind_row =
                one(facade, KIND, "instanceKindId", &kind)?.ok_or_else(|| fail(format!("No InstanceKind {kind}")))?;
            if field(&kind_row, "confName").is_empty() {
                return Err(fail(format!("InstanceKind {kind} is not a database kind")));
            }
            check_vault(facade, row)
        },
    )?;
    Ok(out([("databaseHostId", Value::string(id))]))
}

// ----- an instance from its kind -----

/// What [`create_instance`] is given.
#[derive(Debug, Clone, Default)]
pub struct NewInstance {
    pub app_instance_id: String,
    pub instance_image_id: String,
    pub instance_host_id: String,
    pub environment_id: String,
    pub cluster_id: String,
    pub instance_name: String,
    pub host_name: String,
    pub host_port: String,
    pub database_instance_id: String,
    pub database_host_id: String,
    pub search_instance_id: String,
    pub database_name: String,
    pub database_user: String,
}

/// A name that may be a database identifier: the instance's name with each other character `_`.
fn plain(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Whether `kind` takes a dependency of `role` served by `served_by`.
fn takes(facade: &EntityFacade, kind: &str, role: &str, served_by: &str) -> Result<bool, LibError> {
    Ok(rows(facade, KIND_DEPENDENCY, "instanceKindId", kind)?
        .iter()
        .any(|row| field(row, "dependencyRole") == role && field(row, "allowedKindId") == served_by))
}

/// Makes an instance from its image's kind: the row, its dependencies (checked against the kind's), its settings,
/// volumes and host config from the kind's templates with the placeholders filled, and its secret bindings. Its id.
pub fn create_instance(facade: &EntityFacade, given: &NewInstance) -> Result<String, LibError> {
    let image = one(facade, IMAGE, "instanceImageId", &given.instance_image_id)?
        .ok_or_else(|| fail(format!("No InstanceImage {}", given.instance_image_id)))?;
    let kind_id = field(&image, "instanceKindId");
    if one(facade, KIND, "instanceKindId", &kind_id)?.is_none() {
        return Err(fail(format!("No InstanceKind {kind_id}")));
    }
    let environment = one(facade, ENVIRONMENT, "environmentId", &given.environment_id)?
        .ok_or_else(|| fail(format!("No InstanceEnvironment {}", given.environment_id)))?;
    if field(&environment, "vaultId").is_empty() {
        return Err(fail(format!(
            "InstanceEnvironment {} names no operational vault",
            given.environment_id
        )));
    }
    if one(facade, HOST, "instanceHostId", &given.instance_host_id)?.is_none() {
        return Err(fail(format!("No InstanceHost {}", given.instance_host_id)));
    }
    let name = given.instance_name.trim();
    if !valid_name(name) {
        return Err(fail(format!(
            "instanceName {name:?} is not a container name: letters, digits, '_', '.' and '-'"
        )));
    }
    let id = Some(given.app_instance_id.trim())
        .filter(|id| !id.is_empty())
        .unwrap_or(name)
        .to_string();
    if !valid_name(&id) {
        return Err(fail(format!("appInstanceId {id:?} is not a name")));
    }
    if one(facade, APP_INSTANCE, "appInstanceId", &id)?.is_some() {
        return Err(fail(format!("AppInstance {id} is there already")));
    }
    if !given.host_port.is_empty() && given.host_port.parse::<u16>().ok().filter(|port| *port > 0).is_none() {
        return Err(fail(format!("hostPort {} is not a port", given.host_port)));
    }
    let mut values: BTreeMap<&str, String> = BTreeMap::new();
    values.insert("appInstanceId", id.clone());
    values.insert("instanceName", name.to_string());
    values.insert("hostName", given.host_name.clone());
    values.insert("secretsDir", moqui_lib::docker::SECRETS_DIR.to_string());
    let mut dependencies: Vec<(&str, String, String)> = Vec::new();
    // the database: another instance of the environment, made first, or a database host
    if !given.database_instance_id.is_empty() || !given.database_host_id.is_empty() {
        let (served_by, host, port, conf) = if !given.database_instance_id.is_empty() {
            let other = instance(facade, &given.database_instance_id)?;
            if field(&other, "environmentId") != given.environment_id {
                return Err(fail(format!(
                    "AppInstance {} is in another environment",
                    given.database_instance_id
                )));
            }
            if field(&other, "instanceHostId") != given.instance_host_id {
                return Err(fail(format!(
                    "AppInstance {} is on another host: an instance reaches its database instance on its host's network",
                    given.database_instance_id
                )));
            }
            let other_kind = one(facade, KIND, "instanceKindId", &field(&other, "instanceKindId"))?
                .ok_or_else(|| fail(format!("AppInstance {} has no kind", given.database_instance_id)))?;
            (
                field(&other_kind, "instanceKindId"),
                field(&other, "instanceName"),
                field(&other_kind, "containerPort"),
                field(&other_kind, "confName"),
            )
        } else {
            let host = one(facade, DATABASE_HOST, "databaseHostId", &given.database_host_id)?
                .ok_or_else(|| fail(format!("No DatabaseHost {}", given.database_host_id)))?;
            let host_kind = field(&host, "instanceKindId");
            let conf = one(facade, KIND, "instanceKindId", &host_kind)?
                .map(|row| field(&row, "confName"))
                .unwrap_or_default();
            let address = Some(field(&host, "instanceAddress"))
                .filter(|address| !address.is_empty())
                .unwrap_or_else(|| field(&host, "hostAddress"));
            (host_kind, address, field(&host, "hostPort"), conf)
        };
        if !takes(facade, &kind_id, "database", &served_by)? {
            return Err(fail(format!(
                "An instance of kind {kind_id} takes no {served_by} database"
            )));
        }
        let database = Some(given.database_name.clone())
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| plain(name));
        let user = Some(given.database_user.clone())
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| plain(name));
        crate::database::identifier(&database, "database")?;
        crate::database::identifier(&user, "user")?;
        values.insert("databaseConf", conf);
        values.insert("databaseHost", host);
        values.insert("databasePort", port);
        values.insert("databaseName", database);
        values.insert("databaseUser", user);
        dependencies.push((
            "database",
            given.database_instance_id.clone(),
            given.database_host_id.clone(),
        ));
    }
    if !given.search_instance_id.is_empty() {
        let other = instance(facade, &given.search_instance_id)?;
        if field(&other, "environmentId") != given.environment_id {
            return Err(fail(format!(
                "AppInstance {} is in another environment",
                given.search_instance_id
            )));
        }
        let served_by = field(&other, "instanceKindId");
        if !takes(facade, &kind_id, "search", &served_by)? {
            return Err(fail(format!(
                "An instance of kind {kind_id} takes no {served_by} search"
            )));
        }
        let port = one(facade, KIND, "instanceKindId", &served_by)?
            .map(|row| field(&row, "containerPort"))
            .unwrap_or_default();
        values.insert("searchBackend", served_by);
        values.insert("searchUrl", format!("https://{}:{port}", field(&other, "instanceName")));
        dependencies.push(("search", given.search_instance_id.clone(), String::new()));
    }
    let network = format!("moqui-{}", plain(&given.environment_id).to_ascii_lowercase());
    let mut row = facade.make_value(APP_INSTANCE)?;
    let set = |row: &mut EntityValue, name: &str, value: &str| {
        if !value.is_empty() {
            row.set(name, Value::string(value));
        }
    };
    set(&mut row, "appInstanceId", &id);
    set(&mut row, "instanceKindId", &kind_id);
    set(&mut row, "instanceImageId", &given.instance_image_id);
    set(&mut row, "instanceHostId", &given.instance_host_id);
    set(&mut row, "environmentId", &given.environment_id);
    set(&mut row, "clusterId", &given.cluster_id);
    set(&mut row, "instanceName", name);
    set(&mut row, "hostName", &given.host_name);
    set(&mut row, "hostPort", &given.host_port);
    set(&mut row, "networkName", &network);
    set(
        &mut row,
        "databaseName",
        values.get("databaseName").map_or("", String::as_str),
    );
    set(
        &mut row,
        "databaseUser",
        values.get("databaseUser").map_or("", String::as_str),
    );
    row.create(facade)?;
    for (role, other, host) in &dependencies {
        let mut dependency = facade.make_value(INSTANCE_DEPENDENCY)?;
        dependency.set("appInstanceId", Value::string(&id));
        dependency.set("dependencyRole", Value::string(*role));
        set(&mut dependency, "dependsOnInstanceId", other);
        set(&mut dependency, "databaseHostId", host);
        dependency.create(facade)?;
    }
    for template in rows(facade, KIND_ENV, "instanceKindId", &kind_id)? {
        let value = kinds::expand(&field(&template, "envValue"), &values)?;
        if value.is_empty() {
            continue;
        }
        let mut env = facade.make_value(INSTANCE_ENV)?;
        env.set("appInstanceId", Value::string(&id));
        env.set("envName", Value::string(field(&template, "envName")));
        env.set("envValue", Value::string(value));
        env.create(facade)?;
    }
    for template in rows(facade, KIND_VOLUME, "instanceKindId", &kind_id)? {
        let volume_name = kinds::expand(&field(&template, "volumeName"), &values)?;
        if !valid_name(&volume_name) {
            return Err(fail(format!("The volume name {volume_name:?} is not a name")));
        }
        let mut volume = facade.make_value(INSTANCE_VOLUME)?;
        volume.set("appInstanceId", Value::string(&id));
        volume.set("mountPoint", Value::string(field(&template, "mountPoint")));
        volume.set("volumeName", Value::string(volume_name));
        volume.create(facade)?;
    }
    for template in rows(facade, KIND_HOST_CONFIG, "instanceKindId", &kind_id)? {
        let mut config = facade.make_value(INSTANCE_HOST_CONFIG)?;
        config.set("appInstanceId", Value::string(&id));
        config.set("hostConfigName", Value::string(field(&template, "hostConfigName")));
        config.set("hostConfigValue", Value::string(field(&template, "hostConfigValue")));
        config.create(facade)?;
    }
    let roles: Vec<&str> = dependencies.iter().map(|(role, _, _)| *role).collect();
    for template in rows(facade, KIND_SECRET, "instanceKindId", &kind_id)? {
        let when = field(&template, "whenDependency");
        if !when.is_empty() && !roles.contains(&when.as_str()) {
            continue;
        }
        let mut secret = facade.make_value(INSTANCE_SECRET)?;
        secret.set("appInstanceId", Value::string(&id));
        for name in [
            "secretName",
            "vaultItemTypeId",
            "fieldName",
            "fileName",
            "envName",
            "generated",
        ] {
            set(&mut secret, name, &field(&template, name));
        }
        secret.set(
            "bindingKey",
            Value::string(field(&template, "bindingPattern").replace("{id}", &id)),
        );
        secret.create(facade)?;
    }
    Ok(id)
}

/// `create#AppInstance`.
fn create_app_instance(params: &Params, facade: &EntityFacade) -> Out {
    require_instance_admin()?;
    let _authz = authz_off();
    let given = NewInstance {
        app_instance_id: text(params, "appInstanceId"),
        instance_image_id: required(params, "instanceImageId")?,
        instance_host_id: required(params, "instanceHostId")?,
        environment_id: required(params, "environmentId")?,
        cluster_id: text(params, "clusterId"),
        instance_name: required(params, "instanceName")?,
        host_name: text(params, "hostName"),
        host_port: text(params, "hostPort"),
        database_instance_id: text(params, "databaseInstanceId"),
        database_host_id: text(params, "databaseHostId"),
        search_instance_id: text(params, "searchInstanceId"),
        database_name: text(params, "databaseName"),
        database_user: text(params, "databaseUser"),
    };
    let id = create_instance(facade, &given)?;
    Ok(out([("appInstanceId", Value::string(id))]))
}

// ----- the preview and the actions -----

/// `preview#AppInstanceProvision`: what a provision will make and the lease it needs (§4.3, step 2).
fn preview_app_instance_provision(params: &Params, facade: &EntityFacade) -> Out {
    require_instance_admin()?;
    let _authz = authz_off();
    let plan = plan::build(facade, &required(params, "appInstanceId")?)?;
    Ok(out([("preview", Value::Map(plan::preview(facade, &plan)?))]))
}

/// `request#InstanceAction`: from a screen; runs now, or asks for its lease and runs once it opens.
fn request_instance_action(params: &Params, facade: &EntityFacade) -> Out {
    require_instance_admin()?;
    let _authz = authz_off();
    let id = required(params, "appInstanceId")?;
    let verb = crate::actions::verb(&required(params, "actionEnumId")?)?;
    let requested = crate::actions::request(
        &factory()?,
        facade,
        &registry()?,
        &id,
        verb,
        flag(params, "removeVolumes"),
    )?;
    Ok(out([
        ("instanceActionId", Value::string(requested.action_id)),
        ("statusEnumId", Value::string(requested.status)),
        ("leaseId", requested.lease_id.map_or(Value::Null, Value::string)),
    ]))
}

fn action_out(outcome: &crate::provision::Outcome) -> BTreeMap<String, Value> {
    out([
        ("instanceActionId", Value::string(&outcome.action_id)),
        ("report", Value::string(outcome.report.join("\n"))),
    ])
}

fn act(params: &Params, facade: &EntityFacade, verb: &str) -> Out {
    require_instance_admin()?;
    let _authz = authz_off();
    let id = required(params, "appInstanceId")?;
    let lease = text(params, "leaseId");
    let outcome = crate::actions::run(
        facade,
        &*registry()?,
        &id,
        verb,
        Some(lease.as_str()).filter(|lease| !lease.is_empty()),
        flag(params, "removeVolumes"),
    )?;
    Ok(action_out(&outcome))
}

/// `provision#AppInstance`.
fn provision_app_instance(params: &Params, facade: &EntityFacade) -> Out {
    act(params, facade, "provision")
}

/// `start#AppInstance`.
fn start_app_instance(params: &Params, facade: &EntityFacade) -> Out {
    act(params, facade, "start")
}

/// `stop#AppInstance`.
fn stop_app_instance(params: &Params, facade: &EntityFacade) -> Out {
    act(params, facade, "stop")
}

/// `restart#AppInstance`.
fn restart_app_instance(params: &Params, facade: &EntityFacade) -> Out {
    act(params, facade, "restart")
}

/// `remove#AppInstance`.
fn remove_app_instance(params: &Params, facade: &EntityFacade) -> Out {
    act(params, facade, "remove")
}

/// `check#AppInstance`: the host type's check of the container, when its host needs no key from a lease.
fn check_app_instance(params: &Params, facade: &EntityFacade) -> Out {
    require_instance_admin()?;
    let _authz = authz_off();
    let id = required(params, "appInstanceId")?;
    if !crate::lifecycle::host_bindings(facade, &id)?.is_empty() && moqui_vault_server::lease::current_lease().is_none()
    {
        return Err(fail(format!(
            "AppInstance {id}'s host is reached with a client key from the vault: it is checked during an action, under its lease"
        )));
    }
    container_out(crate::lifecycle::check_container(facade, &id)?)
}

// ----- the docker host type -----

fn container_out(info: Option<moqui_lib::docker::ContainerInfo>) -> Out {
    Ok(match info {
        Some(info) => out([
            ("connectSuccess", Value::Bool(true)),
            ("instanceExists", Value::Bool(true)),
            ("instanceRunning", Value::Bool(info.running)),
            ("statusString", Value::string(&info.status)),
            ("healthString", info.health.map_or(Value::Null, Value::string)),
        ]),
        None => out([
            ("connectSuccess", Value::Bool(true)),
            ("instanceExists", Value::Bool(false)),
            ("instanceRunning", Value::Bool(false)),
            ("statusString", Value::string("missing")),
            ("healthString", Value::Null),
        ]),
    })
}

/// `init#InstanceContainer`: the provision of the instance, under the lease the thread runs under.
fn init_instance_container(params: &Params, facade: &EntityFacade) -> Out {
    require_instance_admin()?;
    let _authz = authz_off();
    let id = required(params, "appInstanceId")?;
    let lease = moqui_vault_server::lease::current_lease();
    let outcome = crate::provision::provision(facade, &*registry()?, &id, lease.as_deref())?;
    let row = instance(facade, &id)?;
    Ok(out([
        ("instanceCreated", Value::Bool(true)),
        ("instanceUuid", Value::string(field(&row, "instanceUuid"))),
        ("warnings", Value::string(outcome.report.join("\n"))),
    ]))
}

/// `start#InstanceContainer`.
fn start_instance_container(params: &Params, facade: &EntityFacade) -> Out {
    require_instance_admin()?;
    let _authz = authz_off();
    let info = crate::lifecycle::start_container(facade, &required(params, "appInstanceId")?)?;
    let running = info.is_some_and(|info| info.running);
    Ok(out([
        ("instanceStarted", Value::Bool(running)),
        ("instanceRunning", Value::Bool(running)),
    ]))
}

/// `stop#InstanceContainer`.
fn stop_instance_container(params: &Params, facade: &EntityFacade) -> Out {
    require_instance_admin()?;
    let _authz = authz_off();
    let info = crate::lifecycle::stop_container(facade, &required(params, "appInstanceId")?)?;
    let running = info.is_some_and(|info| info.running);
    Ok(out([
        ("instanceStopped", Value::Bool(!running)),
        ("instanceRunning", Value::Bool(running)),
    ]))
}

/// `remove#InstanceContainer`.
fn remove_instance_container(params: &Params, facade: &EntityFacade) -> Out {
    require_instance_admin()?;
    let _authz = authz_off();
    crate::lifecycle::remove_container(
        facade,
        &required(params, "appInstanceId")?,
        flag(params, "removeVolumes"),
    )?;
    Ok(out([
        ("instanceRemoved", Value::Bool(true)),
        ("instanceExists", Value::Bool(false)),
    ]))
}

/// `check#InstanceContainer`.
fn check_instance_container(params: &Params, facade: &EntityFacade) -> Out {
    require_instance_admin()?;
    let _authz = authz_off();
    container_out(crate::lifecycle::check_container(
        facade,
        &required(params, "appInstanceId")?,
    )?)
}

// ----- create#Database and check#Database -----

/// The database an instance has on its database dependency, with its server's kind checked, and the admin login from the
/// lease the call runs under, checked against the server its item names.
fn leased_database(
    facade: &EntityFacade,
    instance_id: &str,
    conf: &str,
) -> Result<(plan::DatabaseUse, crate::database::AdminLogin), LibError> {
    let database = plan::database_of(facade, instance_id)?
        .ok_or_else(|| fail(format!("AppInstance {instance_id} has no database dependency")))?;
    if database.conf_name != conf {
        return Err(fail(format!(
            "AppInstance {instance_id}'s database is {}, not {conf}",
            database.conf_name
        )));
    }
    let plan::AdminSource::Leased(binding) = &database.admin else {
        return Err(fail("The admin login of a database made before is leased"));
    };
    let admin = crate::provision::leased_admin(&database, binding)?;
    Ok((database, admin))
}

/// `create#Database` for a database kind: the instance's database and user, the user's password made here and
/// deposited as its `dbPassword` binding (refused when that is deposited already), with the leased admin login; refused
/// when the user or the database is there. When the deposit fails, what was made is dropped.
fn create_database(params: &Params, facade: &EntityFacade, conf: &str) -> Out {
    require_instance_admin()?;
    let _authz = authz_off();
    let id = required(params, "appInstanceId")?;
    let row = instance(facade, &id)?;
    let environment = environment_of(facade, &row)?;
    crate::actions::check_caller(&environment)?;
    let secret = rows(facade, INSTANCE_SECRET, "appInstanceId", &id)?
        .into_iter()
        .find(|secret| field(secret, "secretName") == "dbPassword")
        .ok_or_else(|| fail(format!("AppInstance {id} binds no dbPassword")))?;
    if !field(&secret, "depositedDate").is_empty() {
        return Err(fail(format!(
            "AppInstance {id}'s dbPassword is deposited already: rotate it (M5)"
        )));
    }
    let (database, admin) = leased_database(facade, &id, conf)?;
    let password = crate::common::new_credential();
    let report = crate::provision::create_on(&database, &admin, &password, &id)?;
    let body = zeroize::Zeroizing::new(
        serde_json::json!({ "v": 1, "type": "database-credential", "name": format!("{id} dbPassword"),
            "fields": { "host": database.container_host, "port": database.container_port, "database": database.database,
                        "username": database.user, "password": password.as_str() } })
        .to_string(),
    );
    let deposited = moqui_vault_server::deposit::deposit_item(
        facade,
        &moqui_vault_server::deposit::Deposit {
            vault_id: &field(&environment, "vaultId"),
            item_id: &crate::common::new_item_id(),
            item_type: "database-credential",
            binding: &field(&secret, "bindingKey"),
            name: &format!("{id} dbPassword"),
            body: body.as_bytes(),
        },
    );
    if let Err(err) = deposited {
        if let Err(dropped) = crate::provision::drop_on(&database, &admin) {
            tracing::error!(
                "Instances: database {} of AppInstance {id} was not dropped: {dropped}",
                database.database
            );
        }
        return Err(err);
    }
    let mut secret = secret;
    secret.set("depositedDate", crate::common::now());
    secret.update(facade)?;
    Ok(out([
        ("databaseCreated", Value::Bool(true)),
        ("userCreated", Value::Bool(true)),
        ("report", Value::string(report.join("\n"))),
    ]))
}

/// `check#Database`: whether the admin login connects and the database and user are there, from a one-shot on the
/// server's network or over verified TLS.
fn check_database(params: &Params, facade: &EntityFacade, conf: &str) -> Out {
    require_instance_admin()?;
    let _authz = authz_off();
    let id = required(params, "appInstanceId")?;
    let (database, admin) = leased_database(facade, &id, conf)?;
    let found = crate::provision::check_on(&database, &admin, &id)?;
    Ok(out([
        ("dbConnectSuccess", Value::Bool(found.connected)),
        ("databaseExists", Value::Bool(found.database_exists)),
        ("dbUserExists", Value::Bool(found.user_exists)),
    ]))
}

/// `create#DatabasePostgres`.
fn create_database_postgres(params: &Params, facade: &EntityFacade) -> Out {
    create_database(params, facade, "postgres")
}

/// `create#DatabaseMySQL`.
fn create_database_mysql(params: &Params, facade: &EntityFacade) -> Out {
    create_database(params, facade, "mysql")
}

/// `check#DatabasePostgres`.
fn check_database_postgres(params: &Params, facade: &EntityFacade) -> Out {
    check_database(params, facade, "postgres")
}

/// `check#DatabaseMySQL`.
fn check_database_mysql(params: &Params, facade: &EntityFacade) -> Out {
    check_database(params, facade, "mysql")
}
// ----- the reconcile job -----

/// `reconcile#AppInstances`: the job; observes, changes nothing else.
fn reconcile_app_instances(_params: &Params, facade: &EntityFacade) -> Out {
    require_instance_admin()?;
    let _authz = authz_off();
    let done = crate::reconcile::reconcile(facade)?;
    crate::monitor::refresh(facade);
    Ok(crate::reconcile::counts(done)
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect())
}

// ----- Monitor (§4.8) -----

fn get_instance_fleet(_params: &Params, facade: &EntityFacade) -> Out {
    require_instance_admin()?;
    let _authz = authz_off();
    let rows = crate::fleet::fleet(facade, moqui_core::user::now_millis())?
        .into_iter()
        .map(|row| Value::Map(row.into_iter().collect()))
        .collect();
    Ok(out([("fleetList", Value::List(rows))]))
}

/// Monitor's recovery pass (authorization disabled), a holder of MONITOR_RECOVER, or INSTANCE_ADMIN.
fn require_recovery() -> Result<(), LibError> {
    if require_instance_admin().is_ok() || moqui_core::user::has_permission("MONITOR_RECOVER").unwrap_or(false) {
        return Ok(());
    }
    Err(fail(
        "Recovery of a managed instance needs MONITOR_RECOVER or INSTANCE_ADMIN",
    ))
}

fn run_instance_recovery(params: &Params, facade: &EntityFacade) -> Out {
    require_recovery()?;
    let _authz = authz_off();
    let asked = crate::recovery::Asked {
        action: required(params, "action")?,
        target: text(params, "target"),
        incident_instance: text(params, "incidentInstanceId"),
        automatic: flag(params, "automatic"),
        recovery_action_id: text(params, "recoveryActionId"),
    };
    let answer = crate::recovery::run(&factory()?, facade, &registry()?, &asked)?;
    Ok(out([
        ("status", Value::string(answer.status)),
        (
            "instanceActionId",
            answer.instance_action_id.map_or(Value::Null, Value::string),
        ),
        ("leaseId", answer.lease_id.map_or(Value::Null, Value::string)),
        ("state", Value::string(answer.state)),
    ]))
}
