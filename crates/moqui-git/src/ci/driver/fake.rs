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

//! The `fake` host type: run services that start nothing, record what they were asked and answer as the host's
//! `hostAddress` says, for the engine's tests and for trying pipelines with no container host
//! (docs/interfaces/Pipelines.md "The fake host type"). `success` and `hang` start a container that runs until it is
//! removed (nothing runs in it, so a test plays the runner, or the job runs to its time limit); `fail` refuses to
//! start; `lost` starts one the host then does not know; `exit` starts one that has exited with code 137.

use std::collections::BTreeMap;
use std::sync::Mutex;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{to_plain_string, Value};

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_core::rust_service;

use super::super::field;

type Parameters = BTreeMap<String, Value>;
type Out = Result<BTreeMap<String, Value>, LibError>;

/// One call of a fake run service.
#[derive(Clone, Debug, PartialEq)]
pub struct FakeCall {
    /// start, inspect, remove or cleanup.
    pub action: String,
    pub host_id: String,
    pub parameters: BTreeMap<String, Value>,
}

/// A container the fake host holds.
#[derive(Clone, Debug, PartialEq)]
pub struct FakeContainer {
    pub host_id: String,
    pub image: String,
    pub env: BTreeMap<String, String>,
    pub labels: BTreeMap<String, String>,
}

/// What the fake hosts of a factory were asked and hold.
#[derive(Default)]
pub struct FakeHosts {
    calls: Mutex<Vec<FakeCall>>,
    containers: Mutex<BTreeMap<String, FakeContainer>>,
    next: Mutex<u64>,
}

impl FakeHosts {
    pub fn of(factory: &ExecutionContextFactory) -> std::sync::Arc<Self> {
        factory.extension(|_| Self::default())
    }

    pub fn calls(&self) -> Vec<FakeCall> {
        moqui_lib::sync::lock(&self.calls).clone()
    }

    /// The containers started and not removed, by their id.
    pub fn containers(&self) -> BTreeMap<String, FakeContainer> {
        moqui_lib::sync::lock(&self.containers).clone()
    }
}

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    rust_service!(map, start_run_fake)?;
    rust_service!(map, inspect_run_fake)?;
    rust_service!(map, remove_run_fake)?;
    rust_service!(map, cleanup_runs_fake)?;
    Ok(())
}

fn hosts() -> Option<std::sync::Arc<FakeHosts>> {
    ExecutionContext::with_current(|ec| FakeHosts::of(ec.factory()))
}

fn text(parameters: &Parameters, key: &str) -> String {
    parameters.get(key).map(to_plain_string).unwrap_or_default()
}

fn string_map(value: Option<&Value>) -> BTreeMap<String, String> {
    match value {
        Some(Value::Map(map)) => map
            .iter()
            .map(|(name, value)| (name.clone(), to_plain_string(value)))
            .collect(),
        _ => BTreeMap::new(),
    }
}

/// How the fake host `host_id` behaves: its `hostAddress`, `success` when empty.
fn mode(facade: &EntityFacade, host_id: &str) -> String {
    super::host_row(facade, host_id)
        .ok()
        .and_then(|row| field(&row, "hostAddress"))
        .unwrap_or_else(|| "success".to_string())
}

fn record(action: &str, parameters: &Parameters) -> Option<std::sync::Arc<FakeHosts>> {
    let hosts = hosts()?;
    moqui_lib::sync::lock(&hosts.calls).push(FakeCall {
        action: action.to_string(),
        host_id: text(parameters, "instanceHostId"),
        parameters: parameters.clone(),
    });
    Some(hosts)
}

fn add_error(message: impl Into<String>) {
    let message = message.into();
    ExecutionContext::with_current(|ec| ec.message().add_error(message));
}

/// `start#RunFake`. Out: `instanceRef`.
fn start_run_fake(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(hosts) = record("start", parameters) else {
        return Ok(BTreeMap::new());
    };
    let host_id = text(parameters, "instanceHostId");
    if mode(facade, &host_id) == "fail" {
        add_error(format!("the fake host {host_id} starts nothing (hostAddress fail)"));
        return Ok(BTreeMap::new());
    }
    let id = {
        let mut next = moqui_lib::sync::lock(&hosts.next);
        *next += 1;
        format!("fake-{}", *next)
    };
    moqui_lib::sync::lock(&hosts.containers).insert(
        id.clone(),
        FakeContainer {
            host_id,
            image: text(parameters, "image"),
            env: string_map(parameters.get("envMap")),
            labels: string_map(parameters.get("labels")),
        },
    );
    Ok([("instanceRef".to_string(), Value::string(id))].into_iter().collect())
}

/// `inspect#RunFake`. Out: `state` (running, exited or missing), `exitCode`.
fn inspect_run_fake(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(hosts) = record("inspect", parameters) else {
        return Ok(BTreeMap::new());
    };
    let known = moqui_lib::sync::lock(&hosts.containers).contains_key(&text(parameters, "instanceRef"));
    let (state, exit) = match (known, mode(facade, &text(parameters, "instanceHostId")).as_str()) {
        (false, _) | (_, "lost") => ("missing", Value::Null),
        (_, "exit") => ("exited", Value::I64(137)),
        _ => ("running", Value::Null),
    };
    Ok([
        ("state".to_string(), Value::string(state)),
        ("exitCode".to_string(), exit),
    ]
    .into_iter()
    .collect())
}

/// `remove#RunFake`.
fn remove_run_fake(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    if let Some(hosts) = record("remove", parameters) {
        moqui_lib::sync::lock(&hosts.containers).remove(&text(parameters, "instanceRef"));
    }
    Ok(BTreeMap::new())
}

/// `cleanup#RunsFake`. Out: `removedCount`.
fn cleanup_runs_fake(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let Some(hosts) = record("cleanup", parameters) else {
        return Ok(BTreeMap::new());
    };
    let host_id = text(parameters, "instanceHostId");
    let label = text(parameters, "labelName");
    let keep: Vec<String> = match parameters.get("keepValues") {
        Some(Value::List(values)) => values.iter().map(to_plain_string).collect(),
        _ => Vec::new(),
    };
    let filter = string_map(parameters.get("filterLabels"));
    let mut containers = moqui_lib::sync::lock(&hosts.containers);
    let before = containers.len();
    containers.retain(|_, container| {
        let ours = container.host_id == host_id
            && filter
                .iter()
                .all(|(name, value)| container.labels.get(name) == Some(value));
        let kept = container.labels.get(&label).is_some_and(|job| keep.contains(job));
        !ours || kept
    });
    let removed = i64::try_from(before - containers.len()).unwrap_or(0);
    Ok([("removedCount".to_string(), Value::I64(removed))]
        .into_iter()
        .collect())
}
