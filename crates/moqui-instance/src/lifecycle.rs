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

//! Start, stop, restart, remove and check (plans/archive/Vault-Instances.md §4.1, §4.2): an action runs the services its
//! instance's host type names (`InstanceHostType`'s start, stop, remove and check services, only of
//! `org.moqui.dev.InstanceServices`: the driver contract), under a lease when its host's client key is a vault item.
//! The `docker` host type's services are here too ([`start_container`] and the others), over the shared driver: they
//! act on the instance's container by its id and write what the host says of it.

use std::collections::BTreeMap;
use std::time::Duration;

use moqui_core::context::ExecutionContext;
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_lib::docker::ContainerInfo;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use moqui_vault_server::lease::LeaseRegistry;

use crate::actions;
use crate::common::{
    connect_host, fail, field, host_key_binding, host_needs_key, now, one, rows, HOST, HOST_TYPE, INSTANCE_VOLUME,
    SERVICE_PACKAGE,
};
use crate::plan::{environment_of, instance};
use crate::provision::Outcome;

/// How long a stop gives a container before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(30);

/// The instance's host row.
pub(crate) fn host_of(facade: &EntityFacade, instance: &EntityValue) -> Result<EntityValue, LibError> {
    let host_id = field(instance, "instanceHostId");
    one(facade, HOST, "instanceHostId", &host_id)?.ok_or_else(|| {
        fail(format!(
            "AppInstance {} names no InstanceHost {host_id}",
            field(instance, "appInstanceId")
        ))
    })
}

/// The vault items an action on the instance's container opens: its host's client key, when it has one.
pub fn host_bindings(facade: &EntityFacade, instance_id: &str) -> Result<Vec<String>, LibError> {
    let row = instance(facade, instance_id)?;
    let host = host_of(facade, &row)?;
    Ok(if host_needs_key(&host) {
        vec![host_key_binding(&field(&host, "instanceHostId"))]
    } else {
        Vec::new()
    })
}

/// The vault items an action on the instance's container opens and the operation its lease names, with where the host's
/// client key goes.
pub fn needs(facade: &EntityFacade, instance_id: &str, verb: &str) -> Result<(Vec<String>, String), LibError> {
    let row = instance(facade, instance_id)?;
    let host = host_of(facade, &row)?;
    let destinations = if host_needs_key(&host) {
        vec![crate::plan::host_destination(&host)]
    } else {
        Vec::new()
    };
    let operation = crate::plan::operation(verb, instance_id, &destinations)?;
    Ok((
        destinations
            .into_iter()
            .map(|destination| destination.binding)
            .collect(),
        operation,
    ))
}

/// The host type's services an action runs, in order: restart is its stop, then its start.
fn services_of(facade: &EntityFacade, host: &EntityValue, verb: &str) -> Result<Vec<String>, LibError> {
    let type_id = field(host, "hostTypeId");
    let host_type = one(facade, HOST_TYPE, "hostTypeId", &type_id)?
        .ok_or_else(|| fail(format!("No InstanceHostType {type_id}")))?;
    let fields: &[&str] = match verb {
        "start" => &["startService"],
        "stop" => &["stopService"],
        "restart" => &["stopService", "startService"],
        "remove" => &["removeService"],
        "check" => &["checkService"],
        other => return Err(fail(format!("{other} is not start, stop, restart, remove or check"))),
    };
    let mut services = Vec::new();
    for name in fields {
        let service = field(&host_type, name);
        if service.is_empty() {
            return Err(fail(format!("InstanceHostType {type_id} has no {name}")));
        }
        // a row names the service: only one of these, so a host type row cannot run any other
        if !service.starts_with(SERVICE_PACKAGE) {
            return Err(fail(format!(
                "InstanceHostType {type_id} names a {name} outside {SERVICE_PACKAGE}"
            )));
        }
        services.push(service);
    }
    Ok(services)
}

/// Calls a service by name; its message errors are taken and returned as an error.
pub(crate) fn call(name: &str, parameters: &BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>, LibError> {
    let answer = moqui_core::service::sync(name, parameters);
    let errors = ExecutionContext::with_current(|ec| {
        let text = ec.message().errors().join("; ");
        ec.message().clear_errors();
        text
    })
    .unwrap_or_default();
    match answer {
        Err(err) => Err(fail(format!("{name}: {} {errors}", err.message()))),
        Ok(_) if !errors.is_empty() => Err(fail(format!("{name}: {errors}"))),
        Ok(out) => Ok(out),
    }
}

/// Start, stop, restart or remove an instance's container (see the module doc); an action row records it.
pub fn act(
    facade: &EntityFacade,
    registry: &LeaseRegistry,
    instance_id: &str,
    verb: &str,
    lease_id: Option<&str>,
    remove_volumes: bool,
) -> Result<Outcome, LibError> {
    let row = instance(facade, instance_id)?;
    let environment = environment_of(facade, &row)?;
    let host = host_of(facade, &row)?;
    actions::check_caller(&environment)?;
    let services = services_of(facade, &host, verb)?;
    let (bindings, operation) = needs(facade, instance_id, verb)?;
    let opened = match actions::open_authority(
        facade,
        registry,
        &field(&environment, "environmentId"),
        &field(&environment, "vaultId"),
        field(&environment, "machineAccess") == "Y",
        &bindings,
        &operation,
        lease_id,
    ) {
        Ok(opened) => opened,
        Err(err) => {
            actions::record(
                facade,
                instance_id,
                verb,
                actions::REFUSED,
                lease_id,
                false,
                err.message(),
            )?;
            return Err(err);
        }
    };
    let action_id = actions::record(
        facade,
        instance_id,
        verb,
        actions::RUNNING,
        opened.lease_id.as_deref(),
        opened.machine,
        "",
    )?;
    let mut report = Vec::new();
    let parameters: BTreeMap<String, Value> = [
        ("appInstanceId".to_string(), Value::string(instance_id)),
        ("removeVolumes".to_string(), Value::Bool(remove_volumes)),
    ]
    .into_iter()
    .collect();
    let body = |report: &mut Vec<String>| -> Result<(), LibError> {
        for service in &services {
            call(service, &parameters)?;
            report.push(format!("{service} ran"));
        }
        Ok(())
    };
    let ran = match &opened.lease_id {
        Some(lease_id) => moqui_vault_server::lease::run_under(lease_id, || body(&mut report)),
        None => body(&mut report),
    };
    actions::end_lease(facade, registry, &opened);
    if ran.is_ok() {
        let desired = match verb {
            "stop" => "IdsStopped",
            "remove" => "IdsRemoved",
            _ => "IdsRunning",
        };
        let mut row = instance(facade, instance_id)?;
        row.set("desiredStateEnumId", Value::string(desired));
        row.update(facade)?;
        if verb == "remove" {
            // Monitor's registration goes with it: its ingest key revoked, its scrape target removed (§4.8)
            report.extend(crate::monitor::unregister(facade, instance_id));
        }
    }
    crate::provision::finish(facade, &action_id, &opened, ran, report)
}

/// Writes what the host says of an instance's container: its state (or `missing`), health, image digest and when.
pub fn observe(facade: &EntityFacade, instance_id: &str, info: Option<&ContainerInfo>) -> Result<(), LibError> {
    let mut row = instance(facade, instance_id)?;
    match info {
        Some(info) => {
            row.set("observedState", Value::string(&info.status));
            row.set(
                "observedHealth",
                info.health.as_deref().map_or(Value::Null, Value::string),
            );
            let digest = info
                .repo_digests
                .first()
                .cloned()
                .unwrap_or_else(|| info.image_id.clone());
            if !digest.is_empty() {
                row.set("imageDigest", Value::string(digest));
            }
        }
        None => {
            row.set("observedState", Value::string("missing"));
            row.set("observedHealth", Value::Null);
        }
    }
    row.set("observedDate", now());
    row.update(facade)?;
    Ok(())
}

/// Marks an instance's host as not answering.
pub(crate) fn unreachable(facade: &EntityFacade, instance_id: &str) -> Result<(), LibError> {
    let mut row = instance(facade, instance_id)?;
    row.set("observedState", Value::string("unreachable"));
    row.set("observedDate", now());
    row.update(facade)?;
    Ok(())
}

/// The instance and its container's id, refused when it has none.
fn container_of(facade: &EntityFacade, instance_id: &str) -> Result<(EntityValue, String), LibError> {
    let row = instance(facade, instance_id)?;
    let container = field(&row, "instanceUuid");
    if container.is_empty() {
        return Err(fail(format!(
            "AppInstance {instance_id} has no container: provision it first"
        )));
    }
    Ok((row, container))
}

/// The `docker` host type's start: starts the container and writes what its host says.
pub fn start_container(facade: &EntityFacade, instance_id: &str) -> Result<Option<ContainerInfo>, LibError> {
    let (row, container) = container_of(facade, instance_id)?;
    let host = connect_host(&host_of(facade, &row)?)?;
    host.start(&container)?;
    let info = host.inspect(&container)?;
    observe(facade, instance_id, info.as_ref())?;
    Ok(info)
}

/// The `docker` host type's stop.
pub fn stop_container(facade: &EntityFacade, instance_id: &str) -> Result<Option<ContainerInfo>, LibError> {
    let (row, container) = container_of(facade, instance_id)?;
    let host = connect_host(&host_of(facade, &row)?)?;
    host.stop(&container, STOP_GRACE)?;
    let info = host.inspect(&container)?;
    observe(facade, instance_id, info.as_ref())?;
    Ok(info)
}

/// The `docker` host type's remove: the container (on Podman with its secrets), and its volumes, which hold its data,
/// only when asked. The instance's row stays, with no container.
pub fn remove_container(facade: &EntityFacade, instance_id: &str, remove_volumes: bool) -> Result<(), LibError> {
    let (mut row, container) = container_of(facade, instance_id)?;
    let host = connect_host(&host_of(facade, &row)?)?;
    host.remove_container(&container, false)?;
    if remove_volumes {
        for volume in rows(facade, INSTANCE_VOLUME, "appInstanceId", instance_id)? {
            let name = field(&volume, "volumeName");
            // each volume on its own: one the host refuses is logged and the rest go
            if let Err(err) = host.remove_volume(&name) {
                tracing::error!("Instances: the volume {name} of AppInstance {instance_id} was not removed: {err}");
            }
        }
    }
    row.set("instanceUuid", Value::Null);
    row.set("observedState", Value::string("missing"));
    row.set("observedHealth", Value::Null);
    row.set("observedDate", now());
    row.update(facade)?;
    Ok(())
}

/// The `docker` host type's check: what the host says of the container, written to the row.
pub fn check_container(facade: &EntityFacade, instance_id: &str) -> Result<Option<ContainerInfo>, LibError> {
    let (row, container) = container_of(facade, instance_id)?;
    let host = connect_host(&host_of(facade, &row)?)?;
    let info = host.inspect(&container)?;
    observe(facade, instance_id, info.as_ref())?;
    Ok(info)
}
