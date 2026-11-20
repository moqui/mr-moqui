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
    authz_off, fail, field, find, one, out, require_instance_admin, required, rows, text, Out, Params, APP_INSTANCE,
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
    moqui_core::rust_service!(map, preview_instance_deploy)?;
    moqui_core::rust_service!(map, request_instance_deploy)?;
    moqui_core::rust_service!(map, prepare_git_deployment)?;
    moqui_core::rust_service!(map, deploy_git_deployment)?;
    moqui_core::rust_service!(map, backup_database_instance)?;
    moqui_core::rust_service!(map, approve_instance_deploy)?;
    moqui_core::rust_service!(map, get_instance_fleet)?;
    moqui_core::rust_service!(map, run_instance_recovery)?;
    moqui_core::rust_service!(map, cancel_instance_recovery)?;
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
            "singlePersonDeploy",
            "automaticRecovery",
        ],
        &|row| {
            // a CI environment's machine access is named git-ci-<id> (crates/moqui-git): no InstanceEnvironment takes it
            if field(row, "environmentId").starts_with("git-ci-") {
                return Err(fail("An InstanceEnvironment id does not start with git-ci-, which CI environments' machine access names"));
            }
            check_vault(facade, row)?;
            check_indicator(row, "singlePersonDeploy")?;
            check_indicator(row, "machineAccess")?;
            check_indicator(row, "automaticRecovery")
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
            "sourceRepositoryId",
            "sourceRevision",
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
pub(crate) fn request_instance_action(params: &Params, facade: &EntityFacade) -> Out {
    let id = required(params, "appInstanceId")?;
    // the caller's own authorization: INSTANCE_ADMIN and the environment's group, before it is turned off
    crate::actions::check_instance_caller(facade, &id)?;
    let _authz = authz_off();
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

pub(crate) fn act(params: &Params, facade: &EntityFacade, verb: &str) -> Out {
    let id = required(params, "appInstanceId")?;
    crate::actions::check_instance_caller(facade, &id)?;
    let _authz = authz_off();
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
// ----- deploys (M3, plans/Vault-Instances.md §4.6) -----

/// The deploy a screen's parameters ask for: a new image (`instanceImageId`), a component's commit (`componentName`,
/// `repositoryUrl`, `refName`, `commitId`), or the rollback of a deploy action (`rollbackOfActionId`); narrowed to an
/// instance or a cluster.
fn deploy_request(params: &Params, facade: &EntityFacade) -> Result<crate::deploy::Request, LibError> {
    use crate::deploy::{Change, Component, Request};
    let instance_id = text(params, "appInstanceId");
    let environment_id = match text(params, "environmentId") {
        given if !given.is_empty() => given,
        _ if !instance_id.is_empty() => field(&instance(facade, &instance_id)?, "environmentId"),
        _ => return Err(fail("environmentId or appInstanceId is required")),
    };
    let rollback = text(params, "rollbackOfActionId");
    let image = text(params, "instanceImageId");
    let component = text(params, "componentName");
    let change =
        match (rollback.is_empty(), image.is_empty(), component.is_empty()) {
            (false, true, true) => Change::Rollback { action_id: rollback },
            (true, false, true) => Change::Image { image_id: image },
            (true, true, false) => {
                let ref_name = required(params, "refName")?;
                // the URL is the one the CI environment of the repository deploying here names, never the caller's
                let repository = required(params, "repositoryId")?;
                let repository_url = find(facade, "moqui.resource.GitCiEnvironment")?
                    .eq("instanceEnvironmentId", Value::string(&environment_id))
                    .eq("repositoryId", Value::string(&repository))
                    .list()?
                    .into_values()
                    .first()
                    .map(|ci| field(ci, "cloneUrl"))
                    .unwrap_or_default();
                Change::Component(Component {
                    name: component,
                    repository_url,
                    ref_name: if ref_name.starts_with("refs/") {
                        ref_name
                    } else {
                        format!("refs/heads/{ref_name}")
                    },
                    commit_id: required(params, "commitId")?,
                    data_types: text(params, "dataTypes")
                        .split([',', ' '])
                        .map(str::trim)
                        .filter(|kind| !kind.is_empty())
                        .map(str::to_string)
                        .collect(),
                })
            }
            _ => return Err(fail(
                "A deploy is of one of: instanceImageId, componentName (with repositoryId, refName and commitId), or \
                 rollbackOfActionId",
            )),
        };
    let mut request = Request::new(&environment_id, change);
    // an image's commit is the one its row records (the gate's), never a caller's
    if matches!(request.change, Change::Component(_)) {
        request.commit_id = text(params, "commitId");
    }
    request.repository_id = text(params, "repositoryId");
    request.instance_id = instance_id;
    request.cluster_id = text(params, "clusterId");
    Ok(request)
}

/// The caller of a deploy service, checked under the caller's own authorization before any of it is turned off:
/// `INSTANCE_ADMIN`, and a member of the environment's approver group when it names one (a caller with authorization
/// disabled, crates/moqui-git's deploy step, passes; its decider was checked there). The rows are read with
/// authorization off for the read alone.
fn check_deploy_caller(environment_id: &str, facade: &EntityFacade) -> Result<(), LibError> {
    require_instance_admin()?;
    let environment = one(facade, ENVIRONMENT, "environmentId", environment_id)?
        .ok_or_else(|| fail(format!("No InstanceEnvironment {environment_id}")))?;
    crate::actions::check_caller(&environment)
}

/// `prepare#` and `deploy#GitDeployment` are crates/moqui-git's deploy step's, which calls them with authorization
/// disabled: a caller with it on is refused, whatever it holds; and the deployment is in `status` (waiting-pipeline,
/// deploying), with the lease its row names (`lease`, when given).
fn deployment_step(
    facade: &EntityFacade,
    deployment_id: &str,
    status: &str,
    lease: Option<&str>,
) -> Result<(), LibError> {
    let internal = ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(false);
    if !internal {
        return Err(fail(
            "A deployment's prepare and deploy are its deploy step's (crates/moqui-git), not a caller's",
        ));
    }
    let deployment = one(facade, "moqui.resource.GitDeployment", "deploymentId", deployment_id)?
        .ok_or_else(|| fail(format!("No GitDeployment {deployment_id}")))?;
    if field(&deployment, "statusId") != status {
        return Err(fail(format!(
            "GitDeployment {deployment_id} is {}, not {status}",
            field(&deployment, "statusId")
        )));
    }
    if lease.is_some_and(|lease| lease != field(&deployment, "leaseId")) {
        return Err(fail(format!("The lease is not GitDeployment {deployment_id}'s")));
    }
    Ok(())
}

/// The environment of a deployment's managed instances.
fn deployment_environment(facade: &EntityFacade, deployment_id: &str) -> Result<String, LibError> {
    let deployment = one(facade, "moqui.resource.GitDeployment", "deploymentId", deployment_id)?
        .ok_or_else(|| fail(format!("No GitDeployment {deployment_id}")))?;
    let ci = one(
        facade,
        "moqui.resource.GitCiEnvironment",
        "environmentId",
        &field(&deployment, "environmentId"),
    )?
    .ok_or_else(|| fail(format!("GitDeployment {deployment_id}'s environment is gone")))?;
    Ok(field(&ci, "instanceEnvironmentId"))
}

/// `preview#InstanceDeploy`: what a deploy would change, back up and lease.
fn preview_instance_deploy(params: &Params, facade: &EntityFacade) -> Out {
    let request = deploy_request(params, facade)?;
    check_deploy_caller(&request.environment_id, facade)?;
    let _authz = authz_off();
    let plan = crate::deploy::plan(facade, &crate::deploy::ServiceLink, &request)?;
    Ok(out([("preview", Value::Map(crate::deploy::preview(&plan)))]))
}

/// `request#InstanceDeploy`: a deploy by hand from the instance or environment page; runs now, or asks for its lease
/// and runs once an approver opens it.
fn request_instance_deploy(params: &Params, facade: &EntityFacade) -> Out {
    let request = deploy_request(params, facade)?;
    check_deploy_caller(&request.environment_id, facade)?;
    let _authz = authz_off();
    // what was asked, kept on a deploy that waits for another member's approval (approve#InstanceDeploy)
    let stored = moqui_lib::json::to_json(&Value::Map(params.clone().into_iter().collect())).to_string();
    let requested = crate::deploy::request(
        &factory()?,
        facade,
        &registry()?,
        Arc::new(crate::deploy::ServiceLink),
        request,
        &stored,
    )?;
    Ok(out([
        ("instanceActionId", Value::string(requested.action_id)),
        ("statusEnumId", Value::string(requested.status)),
        ("leaseId", requested.lease_id.map_or(Value::Null, Value::string)),
    ]))
}

/// `prepare#GitDeployment`: crates/moqui-git's call once a deployment to managed instances passed its pipeline: the
/// gate, and the lease of what the deploy opens, held on this member.
fn prepare_git_deployment(params: &Params, facade: &EntityFacade) -> Out {
    let deployment_id = required(params, "deploymentId")?;
    deployment_step(facade, &deployment_id, "waiting-pipeline", None)?;
    check_deploy_caller(&deployment_environment(facade, &deployment_id)?, facade)?;
    let _authz = authz_off();
    let prepared = crate::deploy::prepare_git(facade, &*registry()?, &required(params, "deploymentId")?)?;
    Ok(out([
        ("leaseId", prepared.lease_id.map_or(Value::Null, Value::string)),
        ("approvalRequired", Value::Bool(prepared.approval_required)),
        (
            "leaseMinutes",
            Value::I64(i64::try_from(prepared.lease_minutes).unwrap_or(240)),
        ),
    ]))
}

/// `deploy#GitDeployment`: crates/moqui-git's deploy step for managed instances, under the deployment's lease or the
/// environment's machine access.
fn deploy_git_deployment(params: &Params, facade: &EntityFacade) -> Out {
    let deployment_id = required(params, "deploymentId")?;
    deployment_step(facade, &deployment_id, "deploying", Some(&text(params, "leaseId")))?;
    check_deploy_caller(&deployment_environment(facade, &deployment_id)?, facade)?;
    let _authz = authz_off();
    let component = crate::deploy::Component {
        name: required(params, "componentName")?,
        repository_url: required(params, "repositoryUrl")?,
        data_types: text(params, "dataTypes")
            .split(',')
            .map(str::trim)
            .filter(|kind| !kind.is_empty())
            .map(str::to_string)
            .collect(),
        ..crate::deploy::Component::default()
    };
    let request = crate::deploy::git_request(facade, &deployment_id, component, &text(params, "source"))?;
    let lease = text(params, "leaseId");
    let outcome = crate::deploy::run(
        facade,
        &*registry()?,
        &crate::deploy::ServiceLink,
        &request,
        Some(lease.as_str()).filter(|lease| !lease.is_empty()),
    )?;
    Ok(out([
        (
            "deployResult",
            Value::string(crate::deploy::result_json(&request, &outcome)),
        ),
        ("report", Value::string(outcome.report.join("\n"))),
    ]))
}

/// `backup#DatabaseInstance`: a database kind's backup (§4.6, step 5): one database (`databaseName`) or all, dumped by a
/// one-shot client on the server's network into the server's backups volume (`<instanceName>-backups`), with its admin
/// login from the lease the call runs under, checked against the server the item names. Out: `backupFile`
/// (`volume:file`).
fn backup_database_instance(params: &Params, facade: &EntityFacade) -> Out {
    let id = required(params, "appInstanceId")?;
    let row = instance(facade, &id)?;
    check_deploy_caller(&field(&row, "environmentId"), facade)?;
    let _authz = authz_off();
    let kind = one(facade, KIND, "instanceKindId", &field(&row, "instanceKindId"))?
        .ok_or_else(|| fail(format!("AppInstance {id} has no InstanceKind")))?;
    let conf_name = field(&kind, "confName");
    let admin_user = match conf_name.as_str() {
        "postgres" => "postgres",
        "mysql" => "root",
        _ => {
            return Err(fail(format!(
                "AppInstance {id} is not a database server: postgres or mysql"
            )))
        }
    };
    let server = field(&row, "instanceName");
    let port = field(&kind, "containerPort");
    let mut admin = crate::provision::leased_login(&format!("AppInstance:{id}:admin"), &server, &port)?;
    if admin.user.is_empty() {
        admin.user = admin_user.to_string();
    }
    let database = Some(text(params, "databaseName")).filter(|name| !name.is_empty());
    let file = format!(
        "{}-{}.{}",
        database.as_deref().unwrap_or("all"),
        moqui_core::user::now_millis(),
        if conf_name == "postgres" && database.is_some() {
            "dump"
        } else {
            "sql"
        }
    );
    let volume = format!("{server}-backups");
    let host_row = crate::lifecycle::host_of(facade, &row)?;
    let host = crate::common::connect_host(&host_row)?;
    if host.inspect_volume(&volume)?.is_none() {
        host.create_volume(&volume, &[("moqui.instance".to_string(), id.clone())])?;
    }
    let image = one(facade, IMAGE, "instanceImageId", &field(&row, "instanceImageId"))?
        .ok_or_else(|| fail(format!("AppInstance {id} has no InstanceImage")))?;
    let shot = crate::database::one_shot_backup(
        crate::database::Server {
            conf_name: &conf_name,
            host: &server,
            port: &port,
        },
        &admin,
        database.as_deref(),
        &file,
        &volume,
    )?;
    let (code, lines) = crate::provision::run_one_shot(
        &host_row,
        &field(&row, "networkName"),
        &field(&image, "imageName"),
        &id,
        shot,
    )?;
    if code != 0 {
        return Err(fail(format!(
            "The backup of AppInstance {id} failed (exit {code}): {}",
            lines.join("; ")
        )));
    }
    Ok(out([("backupFile", Value::string(format!("{volume}:{file}")))]))
}

/// `approve#InstanceDeploy`: a deploy by hand that waits for another member of its environment's approver group (it
/// opens no vault item, or the environment has machine access, and is not single-person): by INSTANCE_ADMIN and a
/// member of the group, checked as the caller, never the one who asked; the deploy as it was asked then runs, and the
/// waiting action names how it ended.
fn approve_instance_deploy(params: &Params, facade: &EntityFacade) -> Out {
    let action_id = required(params, "instanceActionId")?;
    let action = one(facade, crate::common::ACTION, "instanceActionId", &action_id)?
        .ok_or_else(|| fail(format!("No InstanceAction {action_id}")))?;
    let row = instance(facade, &field(&action, "appInstanceId"))?;
    check_deploy_caller(&field(&row, "environmentId"), facade)?;
    let stored = field(&action, "deployRequest");
    if field(&action, "statusEnumId") != crate::actions::WAITING
        || !field(&action, "leaseId").is_empty()
        || stored.is_empty()
    {
        return Err(fail(format!(
            "InstanceAction {action_id} is not a deploy waiting for a member's approval"
        )));
    }
    let approver = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
    if approver.is_empty() || approver == field(&action, "userId") {
        return Err(fail(
            "A deploy is approved by another member of the group than who asked for it (reasonCode selfApproval)",
        ));
    }
    let _authz = authz_off();
    let asked: serde_json::Value = serde_json::from_str(&stored).map_err(|err| fail(err.to_string()))?;
    let Value::Map(asked) = moqui_lib::json::from_json(&asked) else {
        return Err(fail(format!("InstanceAction {action_id} keeps no deploy")));
    };
    let asked: Params = asked.into_iter().collect();
    let request = deploy_request(&asked, facade)?;
    let ran = crate::deploy::run(facade, &*registry()?, &crate::deploy::ServiceLink, &request, None);
    let (status, report) = match &ran {
        Ok(outcome) => (
            crate::actions::DONE,
            format!(
                "approved by {approver}; ran as actions {}",
                outcome.action_ids.join(", ")
            ),
        ),
        Err(err) => (
            crate::actions::FAILED,
            format!("approved by {approver}; {}", err.message()),
        ),
    };
    crate::actions::finish(facade, &action_id, status, &report)?;
    let outcome = ran?;
    Ok(out([
        (
            "instanceActionId",
            Value::string(outcome.action_ids.last().cloned().unwrap_or_default()),
        ),
        ("statusEnumId", Value::string(crate::actions::DONE)),
    ]))
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

/// Monitor's recovery pass calls these with authorization disabled; another caller needs INSTANCE_ADMIN. What runs and
/// for whom comes from Monitor's rows, not the parameters (crate::recovery).
fn run_instance_recovery(params: &Params, facade: &EntityFacade) -> Out {
    require_instance_admin()?;
    let _authz = authz_off();
    let recovery_action_id = required(params, "recoveryActionId")?;
    let answer = crate::recovery::run(&factory()?, facade, &registry()?, &recovery_action_id)?;
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

fn cancel_instance_recovery(params: &Params, facade: &EntityFacade) -> Out {
    require_instance_admin()?;
    let _authz = authz_off();
    let recovery_action_id = required(params, "recoveryActionId")?;
    let why = Some(text(params, "reason"))
        .filter(|why| !why.is_empty())
        .unwrap_or_else(|| "cancelled by Monitor".to_string());
    let registry = registry()?;
    let cancelled = crate::recovery::cancel(facade, &registry, &recovery_action_id, &why)?;
    Ok(out([("cancelled", Value::Bool(cancelled))]))
}
