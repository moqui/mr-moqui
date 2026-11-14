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

//! Runner drivers (plans/Dogfood.md F8, docs/interfaces/Pipelines.md "Runners and drivers"). A pool's `InstanceHost` has
//! a host type, and the host type names the services that start a run that ends, inspect it, remove it and clean up
//! what ended or lost runs left (`InstanceHostType.runStartService`, `runInspectService`, `runRemoveService`,
//! `runCleanupService`; the interfaces are `org.moqui.dev.InstanceServices.start#InstanceRun` and the rest). So a
//! driver is a set of services and the engine does not change: [`docker`] speaks the Docker Engine API, which Podman
//! serves too, and [`fake`] records what it is asked, for tests.

use std::collections::BTreeMap;

use moqui_lib::object::{to_plain_string, Value, ValueMap};

use moqui_core::context::Error;
use moqui_core::entity::{EntityFacade, EntityValue};

use super::{field, parameters};

pub mod docker;
pub mod fake;

pub(crate) const INSTANCE_HOST: &str = "moqui.server.instance.InstanceHost";
pub(crate) const INSTANCE_HOST_TYPE: &str = "moqui.server.instance.InstanceHostType";

/// The label that ties a container to its job run, and the one that says which server made it.
pub const LABEL_JOB: &str = "moqui.ci.job";
pub const LABEL_RUN: &str = "moqui.ci.run";
pub const LABEL_SERVER: &str = "moqui.ci.server";

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    docker::register(map)?;
    fake::register(map)
}

/// A container beside the job.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ServiceSpec {
    pub name: String,
    pub image: String,
    pub env: Vec<(String, String)>,
}

/// A run that ends: one container for a job.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunSpec {
    pub name: String,
    pub image: String,
    pub command: Vec<String>,
    pub env: Vec<(String, String)>,
    pub labels: Vec<(String, String)>,
    pub services: Vec<ServiceSpec>,
    pub cpu_limit: Option<f64>,
    pub memory_mb: Option<i64>,
    pub disk_mb: Option<i64>,
    pub pids_limit: Option<i64>,
    pub network_mode: Option<String>,
}

/// What a host says of a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstanceState {
    Running,
    Exited(Option<i64>),
    Missing,
}

fn string_map(pairs: &[(String, String)]) -> Value {
    let map: ValueMap = pairs
        .iter()
        .map(|(name, value)| (name.clone(), Value::string(value)))
        .collect();
    Value::Map(map)
}

/// The service of a host's type for `field_name` (`runStartService` and the rest).
fn host_service(entity: &EntityFacade, host_id: &str, field_name: &str) -> Result<String, Error> {
    let _rows = crate::rows_guard();
    let host = super::one(entity, INSTANCE_HOST, "instanceHostId", host_id)?
        .ok_or_else(|| Error::new(format!("no instance host {host_id}")))?;
    let type_id = field(&host, "hostTypeId").unwrap_or_default();
    let host_type = super::one(entity, INSTANCE_HOST_TYPE, "hostTypeId", &type_id)?
        .ok_or_else(|| Error::new(format!("instance host {host_id} has no host type")))?;
    field(&host_type, field_name).ok_or_else(|| {
        Error::new(format!(
            "host type {type_id} of instance host {host_id} has no {field_name}: it does not start runs that end"
        ))
    })
}

fn call(name: &str, input: BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>, Error> {
    let options = moqui_core::service::call::CallOptions {
        disable_authz: true,
        ..moqui_core::service::call::CallOptions::default()
    };
    moqui_core::service::call::sync_checked(name, &input, &options)
}

/// Start `spec` on a host. The container's id there.
pub fn start(entity: &EntityFacade, host_id: &str, spec: &RunSpec) -> Result<String, Error> {
    let service = host_service(entity, host_id, "runStartService")?;
    let services: Vec<Value> = spec
        .services
        .iter()
        .map(|service| {
            let mut map = ValueMap::default();
            map.insert("name".to_string(), Value::string(&service.name));
            map.insert("image".to_string(), Value::string(&service.image));
            map.insert("envMap".to_string(), string_map(&service.env));
            Value::Map(map)
        })
        .collect();
    let number = |value: Option<i64>| value.map_or(Value::Null, Value::I64);
    let out = call(
        &service,
        parameters([
            ("instanceHostId", Value::string(host_id)),
            ("name", Value::string(&spec.name)),
            ("image", Value::string(&spec.image)),
            ("command", Value::List(spec.command.iter().map(Value::string).collect())),
            ("envMap", string_map(&spec.env)),
            ("labels", string_map(&spec.labels)),
            ("services", Value::List(services)),
            (
                "cpuLimit",
                spec.cpu_limit
                    .map_or(Value::Null, |cpus| Value::string(cpus.to_string())),
            ),
            ("memoryMb", number(spec.memory_mb)),
            ("diskMb", number(spec.disk_mb)),
            ("pidsLimit", number(spec.pids_limit)),
            (
                "networkMode",
                spec.network_mode.clone().map_or(Value::Null, Value::string),
            ),
        ]),
    )?;
    out.get("instanceRef")
        .map(to_plain_string)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Error::new(format!("{service} started nothing (no instanceRef)")))
}

/// What the host says of the container `instance_ref`.
pub fn inspect(entity: &EntityFacade, host_id: &str, instance_ref: &str) -> Result<InstanceState, Error> {
    let service = host_service(entity, host_id, "runInspectService")?;
    let out = call(
        &service,
        parameters([
            ("instanceHostId", Value::string(host_id)),
            ("instanceRef", Value::string(instance_ref)),
        ]),
    )?;
    let exit = out
        .get("exitCode")
        .map(to_plain_string)
        .and_then(|code| code.parse().ok());
    Ok(match out.get("state").map(to_plain_string).as_deref() {
        Some("running") => InstanceState::Running,
        Some("missing") => InstanceState::Missing,
        _ => InstanceState::Exited(exit),
    })
}

/// Remove the container `instance_ref`, running or not, with the service containers and the network of its job.
pub fn remove(entity: &EntityFacade, host_id: &str, instance_ref: &str, job_run_id: &str) -> Result<(), Error> {
    let service = host_service(entity, host_id, "runRemoveService")?;
    call(
        &service,
        parameters([
            ("instanceHostId", Value::string(host_id)),
            ("instanceRef", Value::string(instance_ref)),
            ("labelName", Value::string(LABEL_JOB)),
            ("labelValue", Value::string(job_run_id)),
        ]),
    )?;
    Ok(())
}

/// Remove the containers this server made on a host whose job run is not in `keep`. The count removed.
pub fn cleanup(entity: &EntityFacade, host_id: &str, keep: &[String]) -> Result<i64, Error> {
    let service = host_service(entity, host_id, "runCleanupService")?;
    let out = call(
        &service,
        parameters([
            ("instanceHostId", Value::string(host_id)),
            ("labelName", Value::string(LABEL_JOB)),
            ("keepValues", Value::List(keep.iter().map(Value::string).collect())),
            ("filterLabels", string_map(&[(LABEL_SERVER.to_string(), server_id())])),
        ]),
    )?;
    Ok(out
        .get("removedCount")
        .map(to_plain_string)
        .and_then(|count| count.parse().ok())
        .unwrap_or(0))
}

/// What tells this server's containers from another's on a shared host: the conf's `git-ci` `server-id`, else the
/// public URL's host.
pub fn server_id() -> String {
    let configured = super::setting("server-id");
    if !configured.is_empty() {
        return configured;
    }
    let url = crate::server::public_base_url();
    let host = url
        .split_once("://")
        .map_or(url.as_str(), |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or_default()
        .to_string();
    if host.is_empty() {
        "moqui".to_string()
    } else {
        host
    }
}

/// The instance host row `host_id`.
pub(crate) fn host_row(entity: &EntityFacade, host_id: &str) -> Result<EntityValue, Error> {
    let _rows = crate::rows_guard();
    super::one(entity, INSTANCE_HOST, "instanceHostId", host_id)?
        .ok_or_else(|| Error::new(format!("no instance host {host_id}")))
}
