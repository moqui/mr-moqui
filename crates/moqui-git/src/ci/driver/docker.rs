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

//! The container driver (plans/Dogfood.md D19, F8): the run services of the `docker` host type, over the Docker Engine
//! API, which Podman serves on its socket too. An `InstanceHost` is reached over a Unix socket (`hostProtocol` `unix`,
//! `hostAddress` the socket's path), plain HTTP, or HTTPS with a client certificate (`tlsCaCert`, `tlsClientCert`,
//! `tlsClientKey`). A job's container has no privilege, no capabilities, no host socket and no host path, a non-root
//! user, a read-only root with a volume at its working directory and a tmpfs at `/tmp`, and the pool's limits
//! (docs/interfaces/Pipelines.md "Runners and drivers").

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::json;

use moqui_lib::error::Error as LibError;
use moqui_lib::http::{Method, RestClient, RestResponse};
use moqui_lib::object::{to_plain_string, Value};

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::rust_service;

use super::super::field;

type Parameters = BTreeMap<String, Value>;
type Out = Result<BTreeMap<String, Value>, LibError>;

/// The API version asked for: Docker 19.03 and later, and Podman's compatibility API.
const API: &str = "/v1.40";
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// An image pull may take this long.
const PULL_TIMEOUT: Duration = Duration::from_secs(15 * 60);

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    rust_service!(map, start_run_docker)?;
    rust_service!(map, inspect_run_docker)?;
    rust_service!(map, remove_run_docker)?;
    rust_service!(map, cleanup_runs_docker)?;
    Ok(())
}

/// A container of a listing: its id and its labels.
type Listed = (String, BTreeMap<String, String>);

/// A connection to a host's API.
pub struct DockerHost {
    client: RestClient,
    base: String,
}

impl DockerHost {
    /// The client for an `InstanceHost` row.
    ///
    /// # Errors
    ///
    /// When the row names no address.
    pub fn of(host: &EntityValue) -> Result<Self, Error> {
        let address = field(host, "hostAddress")
            .ok_or_else(|| Error::new("the instance host has no hostAddress (a socket path or a host name)"))?;
        let protocol = field(host, "hostProtocol").unwrap_or_else(|| "http".to_string());
        let mut client = RestClient::new().timeout(CALL_TIMEOUT);
        let base = match protocol.as_str() {
            "unix" => {
                client = client.unix_socket(address.trim_start_matches("unix://"));
                "http://localhost".to_string()
            }
            "http" | "https" => {
                let port = field(host, "adminPort").map_or(String::new(), |port| format!(":{port}"));
                format!("{protocol}://{address}{port}")
            }
            other => return Err(Error::new(format!("hostProtocol {other} is not unix, http or https"))),
        };
        if let Some(ca) = field(host, "tlsCaCert") {
            client = client.root_certificates_pem(ca.into_bytes());
        }
        if let (Some(cert), Some(key)) = (field(host, "tlsClientCert"), field(host, "tlsClientKey")) {
            client = client.client_identity_pem(format!("{}\n{}\n", cert.trim(), key.trim()).into_bytes());
        }
        Ok(Self { client, base })
    }

    /// A host reached over a Unix socket, with no `InstanceHost` row.
    pub fn unix(socket: impl Into<std::path::PathBuf>) -> Self {
        Self {
            client: RestClient::new().timeout(CALL_TIMEOUT).unix_socket(socket),
            base: "http://localhost".to_string(),
        }
    }

    /// Run `command` in a container of `image` to its end, kept in as a job's container is, and remove it: the
    /// container's exit code. For the opt-in test on a real Docker or Podman socket.
    ///
    /// # Errors
    ///
    /// When the host refuses a call, or the container has not ended after `wait`.
    pub fn run_once(&self, name: &str, image: &str, command: &[&str], wait: Duration) -> Result<Option<i64>, Error> {
        let parameters: Parameters = [
            ("image".to_string(), Value::string(image)),
            (
                "command".to_string(),
                Value::List(command.iter().map(|word| Value::string(*word)).collect()),
            ),
            ("memoryMb".to_string(), Value::I64(256)),
            ("pidsLimit".to_string(), Value::I64(64)),
            ("networkMode".to_string(), Value::string("none")),
        ]
        .into_iter()
        .collect();
        let id = self.create(name, &create_body(&parameters, &Hardening::default(), None))?;
        let outcome = (|| -> Result<Option<i64>, Error> {
            self.start(&id)?;
            let started = std::time::Instant::now();
            loop {
                let response = self.send(Method::Get, &format!("/containers/{id}/json"), None, CALL_TIMEOUT)?;
                if response.status() >= 300 {
                    return Err(Self::problem(&format!("inspecting container {id}"), &response));
                }
                let body: serde_json::Value = serde_json::from_str(response.body()).unwrap_or(serde_json::Value::Null);
                if body["State"]["Running"].as_bool() != Some(true)
                    && body["State"]["Status"].as_str() != Some("created")
                {
                    return Ok(body["State"]["ExitCode"].as_i64());
                }
                if started.elapsed() > wait {
                    return Err(Error::new(format!("container {id} has not ended after {wait:?}")));
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        })();
        let removed = self.remove(&id);
        let exit = outcome?;
        removed?;
        Ok(exit)
    }

    fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&serde_json::Value>,
        timeout: Duration,
    ) -> Result<RestResponse, Error> {
        let mut call = self
            .client
            .request(method, format!("{}{API}{path}", self.base))
            .timeout(timeout)
            .allow_in_sim(true);
        if let Some(body) = body {
            call = call.json(body);
        }
        call.send(false).map_err(Error::from)
    }

    /// The message of an API error response.
    fn problem(what: &str, response: &RestResponse) -> Error {
        let message = serde_json::from_str::<serde_json::Value>(response.body())
            .ok()
            .and_then(|body| body["message"].as_str().map(str::to_string))
            .unwrap_or_else(|| response.body().chars().take(300).collect());
        Error::new(format!("{what}: {} {message}", response.status()))
    }

    fn pull(&self, image: &str) -> Result<(), Error> {
        let (name, tag) = match image.rsplit_once(':') {
            Some((name, tag)) if !tag.contains('/') => (name, tag),
            _ => (image, "latest"),
        };
        let response = self.send(
            Method::Post,
            &format!("/images/create?fromImage={}&tag={}", encode(name), encode(tag)),
            None,
            PULL_TIMEOUT,
        )?;
        if response.status() >= 300 {
            return Err(Self::problem(&format!("pulling image {image}"), &response));
        }
        // the stream of the pull ends when it is done; its last lines say when it failed
        if let Some(line) = response.body().lines().rev().find(|line| line.contains("\"error\"")) {
            return Err(Error::new(format!("pulling image {image}: {line}")));
        }
        Ok(())
    }

    /// Create a container, pulling its image when the host does not have it. Its id.
    fn create(&self, name: &str, body: &serde_json::Value) -> Result<String, Error> {
        let path = format!("/containers/create?name={}", encode(name));
        let mut response = self.send(Method::Post, &path, Some(body), CALL_TIMEOUT)?;
        if response.status() == 404 {
            self.pull(body["Image"].as_str().unwrap_or_default())?;
            response = self.send(Method::Post, &path, Some(body), CALL_TIMEOUT)?;
        }
        if response.status() >= 300 {
            return Err(Self::problem(&format!("creating container {name}"), &response));
        }
        serde_json::from_str::<serde_json::Value>(response.body())
            .ok()
            .and_then(|body| body["Id"].as_str().map(str::to_string))
            .ok_or_else(|| Error::new(format!("creating container {name}: the answer has no Id")))
    }

    fn start(&self, id: &str) -> Result<(), Error> {
        let response = self.send(Method::Post, &format!("/containers/{id}/start"), None, CALL_TIMEOUT)?;
        // 304: already started
        if response.status() >= 300 && response.status() != 304 {
            return Err(Self::problem(&format!("starting container {id}"), &response));
        }
        Ok(())
    }

    /// Remove a container with its volumes, running or not. One that is not there is removed.
    fn remove(&self, id: &str) -> Result<(), Error> {
        let response = self.send(
            Method::Delete,
            &format!("/containers/{id}?force=true&v=true"),
            None,
            CALL_TIMEOUT,
        )?;
        if response.status() >= 300 && response.status() != 404 {
            return Err(Self::problem(&format!("removing container {id}"), &response));
        }
        Ok(())
    }

    /// The containers, running or not, with all of `labels` (`name=value`): each one's id and labels.
    fn list(&self, labels: &[String]) -> Result<Vec<Listed>, Error> {
        let filters = json!({ "label": labels }).to_string();
        let response = self.send(
            Method::Get,
            &format!("/containers/json?all=true&filters={}", encode(&filters)),
            None,
            CALL_TIMEOUT,
        )?;
        if response.status() >= 300 {
            return Err(Self::problem("listing containers", &response));
        }
        let listed: serde_json::Value = serde_json::from_str(response.body())
            .map_err(|err| Error::new(format!("listing containers: the answer is not JSON: {err}")))?;
        Ok(listed
            .as_array()
            .map(|containers| {
                containers
                    .iter()
                    .filter_map(|container| {
                        let labels = container["Labels"]
                            .as_object()
                            .map(|labels| {
                                labels
                                    .iter()
                                    .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_string())))
                                    .collect()
                            })
                            .unwrap_or_default();
                        Some((container["Id"].as_str()?.to_string(), labels))
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    fn create_network(&self, name: &str, labels: &serde_json::Value) -> Result<(), Error> {
        let response = self.send(
            Method::Post,
            "/networks/create",
            Some(&json!({"Name": name, "Driver": "bridge", "Labels": labels})),
            CALL_TIMEOUT,
        )?;
        if response.status() >= 300 && response.status() != 409 {
            return Err(Self::problem(&format!("creating network {name}"), &response));
        }
        Ok(())
    }

    fn remove_network(&self, name: &str) {
        match self.send(
            Method::Delete,
            &format!("/networks/{}", encode(name)),
            None,
            CALL_TIMEOUT,
        ) {
            Ok(response) if response.status() < 300 || response.status() == 404 => {}
            Ok(response) => tracing::warn!(
                "network {name} was not removed: {}",
                Self::problem("removing", &response)
            ),
            Err(err) => tracing::warn!("network {name} was not removed: {err}"),
        }
    }
}

/// Percent-encode a query value.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn text(parameters: &Parameters, key: &str) -> Option<String> {
    parameters
        .get(key)
        .filter(|value| !moqui_lib::object::is_empty(value))
        .map(to_plain_string)
        .filter(|text| !text.trim().is_empty())
}

fn string_pairs(value: Option<&Value>) -> Vec<(String, String)> {
    match value {
        Some(Value::Map(map)) => map
            .iter()
            .map(|(name, value)| (name.clone(), to_plain_string(value)))
            .collect(),
        _ => Vec::new(),
    }
}

fn number(parameters: &Parameters, key: &str) -> Option<i64> {
    text(parameters, key).and_then(|text| text.split('.').next().and_then(|whole| whole.parse().ok()))
}

/// How a job's container is kept in, beside its pool's limits: the conf's `git-ci` `container-user` (1000:1000; the
/// image's with `image`), `container-read-only-root` (true), `container-workdir` (/workspace) and
/// `container-tmp-mb` (1024).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hardening {
    pub user: Option<String>,
    pub read_only_root: bool,
    pub workdir: String,
    pub tmp_mb: u64,
}

impl Default for Hardening {
    fn default() -> Self {
        Self {
            user: Some("1000:1000".to_string()),
            read_only_root: true,
            workdir: "/workspace".to_string(),
            tmp_mb: 1024,
        }
    }
}

impl Hardening {
    fn from_conf() -> Self {
        let default = Self::default();
        let setting = super::super::setting;
        Self {
            user: match setting("container-user").as_str() {
                "" => default.user,
                "image" => None,
                // never root, whatever the conf says
                "0" | "root" | "0:0" | "root:root" => default.user,
                user => Some(user.to_string()),
            },
            read_only_root: setting("container-read-only-root") != "false",
            workdir: Some(setting("container-workdir"))
                .filter(|dir| dir.starts_with('/'))
                .unwrap_or(default.workdir),
            tmp_mb: super::super::number_setting("container-tmp-mb", default.tmp_mb),
        }
    }
}

/// The body of `POST /containers/create` for a job's container, from the parameters of `start#InstanceRun`.
/// `network` is the job's own network when it has service containers.
pub fn create_body(parameters: &Parameters, hardening: &Hardening, network: Option<&str>) -> serde_json::Value {
    let env: Vec<String> = string_pairs(parameters.get("envMap"))
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    let labels: serde_json::Map<String, serde_json::Value> = string_pairs(parameters.get("labels"))
        .into_iter()
        .map(|(name, value)| (name, json!(value)))
        .collect();
    let command: Vec<String> = match parameters.get("command") {
        Some(Value::List(words)) => words.iter().map(to_plain_string).collect(),
        _ => Vec::new(),
    };
    let mut host = json!({
        "Privileged": false,
        "CapDrop": ["ALL"],
        "SecurityOpt": ["no-new-privileges"],
        "ReadonlyRootfs": hardening.read_only_root,
        "AutoRemove": false,
        "PublishAllPorts": false,
        "Binds": [],
        "Tmpfs": { "/tmp": format!("rw,exec,nosuid,nodev,size={}m", hardening.tmp_mb) },
        "NetworkMode": network
            .map(str::to_string)
            .or_else(|| text(parameters, "networkMode"))
            .unwrap_or_else(|| "bridge".to_string()),
    });
    if let Some(cpus) = text(parameters, "cpuLimit").and_then(|cpus| cpus.parse::<f64>().ok()) {
        if cpus > 0.0 {
            #[allow(clippy::cast_possible_truncation)]
            let nano = (cpus * 1_000_000_000.0).round() as i64;
            host["NanoCpus"] = json!(nano);
        }
    }
    if let Some(memory) = number(parameters, "memoryMb").filter(|memory| *memory > 0) {
        host["Memory"] = json!(memory * 1024 * 1024);
        // no swap beyond the memory limit
        host["MemorySwap"] = json!(memory * 1024 * 1024);
    }
    if let Some(pids) = number(parameters, "pidsLimit").filter(|pids| *pids > 0) {
        host["PidsLimit"] = json!(pids);
    }
    if let Some(disk) = number(parameters, "diskMb").filter(|disk| *disk > 0) {
        host["StorageOpt"] = json!({ "size": format!("{disk}m") });
    }
    let mut body = json!({
        "Image": text(parameters, "image").unwrap_or_default(),
        "Env": env,
        "Labels": labels,
        "WorkingDir": hardening.workdir,
        // an anonymous volume, removed with the container: the checkout, the caches and HOME
        "Volumes": { hardening.workdir.clone(): {} },
        "AttachStdin": false,
        "Tty": false,
        "HostConfig": host,
    });
    if !command.is_empty() {
        body["Cmd"] = json!(command);
    }
    if let Some(user) = &hardening.user {
        body["User"] = json!(user);
    }
    body
}

/// The body of `POST /containers/create` for a service container beside a job, reached by its name on the job's
/// network.
pub fn service_body(
    service: &BTreeMap<String, Value>,
    labels: &serde_json::Value,
    network: &str,
    parameters: &Parameters,
) -> serde_json::Value {
    let env: Vec<String> = string_pairs(service.get("envMap"))
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    let name = service.get("name").map(to_plain_string).unwrap_or_default();
    let mut host = json!({
        "Privileged": false,
        "CapDrop": ["ALL"],
        "CapAdd": ["CHOWN", "SETUID", "SETGID", "DAC_OVERRIDE", "FOWNER"],
        "SecurityOpt": ["no-new-privileges"],
        "AutoRemove": false,
        "Binds": [],
        "NetworkMode": network,
    });
    if let Some(memory) = number(parameters, "memoryMb").filter(|memory| *memory > 0) {
        host["Memory"] = json!(memory * 1024 * 1024);
    }
    if let Some(pids) = number(parameters, "pidsLimit").filter(|pids| *pids > 0) {
        host["PidsLimit"] = json!(pids);
    }
    json!({
        "Image": service.get("image").map(to_plain_string).unwrap_or_default(),
        "Env": env,
        "Labels": labels,
        "HostConfig": host,
        "NetworkingConfig": { "EndpointsConfig": { network: { "Aliases": [name] } } },
    })
}

fn add_error(message: impl Into<String>) {
    let message = message.into();
    ExecutionContext::with_current(|ec| ec.message().add_error(message));
}

fn connect(parameters: &Parameters, facade: &EntityFacade) -> Option<DockerHost> {
    let Some(host_id) = text(parameters, "instanceHostId") else {
        add_error("instanceHostId is required");
        return None;
    };
    match super::host_row(facade, &host_id).and_then(|row| DockerHost::of(&row)) {
        Ok(host) => Some(host),
        Err(err) => {
            add_error(format!("instance host {host_id}: {}", err.message()));
            None
        }
    }
}

/// The network of a job with service containers, by its container's name.
fn network_name(container: &str) -> String {
    format!("{container}-net")
}

/// `start#RunDocker`: create and start a job's container, after its service containers on a network of the job's
/// own. Out: `instanceRef`, the container's id. What was made is removed when a later step fails.
fn start_run_docker(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(host) = connect(parameters, facade) else {
        return Ok(BTreeMap::new());
    };
    let (Some(name), Some(_image)) = (text(parameters, "name"), text(parameters, "image")) else {
        add_error("name and image are required");
        return Ok(BTreeMap::new());
    };
    let services: Vec<BTreeMap<String, Value>> = match parameters.get("services") {
        Some(Value::List(services)) => services
            .iter()
            .filter_map(|service| match service {
                Value::Map(map) => Some(map.iter().map(|(key, value)| (key.clone(), value.clone())).collect()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let labels: serde_json::Value = string_pairs(parameters.get("labels"))
        .into_iter()
        .map(|(name, value)| (name, json!(value)))
        .collect::<serde_json::Map<String, serde_json::Value>>()
        .into();
    let network = (!services.is_empty()).then(|| network_name(&name));
    let mut made: Vec<String> = Vec::new();
    let started = (|| -> Result<String, Error> {
        if let Some(network) = &network {
            host.create_network(network, &labels)?;
            for service in &services {
                let service_name = service.get("name").map(to_plain_string).unwrap_or_default();
                let id = host.create(
                    &format!("{name}-{service_name}"),
                    &service_body(service, &labels, network, parameters),
                )?;
                made.push(id.clone());
                host.start(&id)?;
            }
        }
        let id = host.create(
            &name,
            &create_body(parameters, &Hardening::from_conf(), network.as_deref()),
        )?;
        made.push(id.clone());
        host.start(&id)?;
        Ok(id)
    })();
    match started {
        Ok(id) => Ok([("instanceRef".to_string(), Value::string(id))].into_iter().collect()),
        Err(err) => {
            for id in &made {
                if let Err(remove) = host.remove(id) {
                    tracing::warn!("a container of a run that did not start was not removed: {remove}");
                }
            }
            if let Some(network) = &network {
                host.remove_network(network);
            }
            add_error(err.message());
            Ok(BTreeMap::new())
        }
    }
}

/// `inspect#RunDocker`. Out: `state` (running, exited or missing), `exitCode`.
fn inspect_run_docker(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(host) = connect(parameters, facade) else {
        return Ok(BTreeMap::new());
    };
    let Some(id) = text(parameters, "instanceRef") else {
        add_error("instanceRef is required");
        return Ok(BTreeMap::new());
    };
    let response = match host.send(Method::Get, &format!("/containers/{id}/json"), None, CALL_TIMEOUT) {
        Ok(response) => response,
        Err(err) => {
            add_error(err.message());
            return Ok(BTreeMap::new());
        }
    };
    let out = |state: &str, exit: Value| -> Out {
        Ok([
            ("state".to_string(), Value::string(state)),
            ("exitCode".to_string(), exit),
        ]
        .into_iter()
        .collect())
    };
    if response.status() == 404 {
        return out("missing", Value::Null);
    }
    if response.status() >= 300 {
        add_error(DockerHost::problem(&format!("inspecting container {id}"), &response).message());
        return Ok(BTreeMap::new());
    }
    let body: serde_json::Value = serde_json::from_str(response.body()).unwrap_or(serde_json::Value::Null);
    let state = &body["State"];
    match state["Status"].as_str() {
        Some("created" | "running" | "restarting" | "paused") => out("running", Value::Null),
        _ if state["Running"].as_bool() == Some(true) => out("running", Value::Null),
        _ => out("exited", state["ExitCode"].as_i64().map_or(Value::Null, Value::I64)),
    }
}

/// `remove#RunDocker`: remove a job's container, and with `labelName` and `labelValue` every container of that label
/// (its service containers) and the job's network.
fn remove_run_docker(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(host) = connect(parameters, facade) else {
        return Ok(BTreeMap::new());
    };
    let mut ids: Vec<String> = text(parameters, "instanceRef").into_iter().collect();
    if let (Some(label), Some(value)) = (text(parameters, "labelName"), text(parameters, "labelValue")) {
        match host.list(&[format!("{label}={value}")]) {
            Ok(listed) => {
                for (id, _) in listed {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
            }
            Err(err) => tracing::warn!("the containers of {label}={value} were not listed: {err}"),
        }
    }
    for id in ids {
        if let Err(err) = host.remove(&id) {
            add_error(err.message());
        }
    }
    // the job's network is named after its container, and goes once nothing is on it
    if let Some(value) = text(parameters, "labelValue") {
        host.remove_network(&network_name(&super::super::engine::container_name(&value)));
    }
    Ok(BTreeMap::new())
}

/// `cleanup#RunsDocker`: remove the containers with every label of `filterLabels` whose `labelName` label is not one
/// of `keepValues`. Out: `removedCount`.
fn cleanup_runs_docker(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(host) = connect(parameters, facade) else {
        return Ok(BTreeMap::new());
    };
    let Some(label) = text(parameters, "labelName") else {
        add_error("labelName is required");
        return Ok(BTreeMap::new());
    };
    let keep: Vec<String> = match parameters.get("keepValues") {
        Some(Value::List(values)) => values.iter().map(to_plain_string).collect(),
        _ => Vec::new(),
    };
    let mut filters: Vec<String> = string_pairs(parameters.get("filterLabels"))
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    filters.push(label.clone());
    let listed = match host.list(&filters) {
        Ok(listed) => listed,
        Err(err) => {
            add_error(err.message());
            return Ok(BTreeMap::new());
        }
    };
    let mut removed = 0;
    for (id, labels) in listed {
        let Some(job) = labels.get(&label) else { continue };
        if keep.contains(job) {
            continue;
        }
        // one container that does not go does not keep the others
        match host.remove(&id) {
            Ok(()) => {
                removed += 1;
                host.remove_network(&network_name(&super::super::engine::container_name(job)));
            }
            Err(err) => tracing::warn!("a container of an ended job run was not removed: {err}"),
        }
    }
    Ok([("removedCount".to_string(), Value::I64(removed))]
        .into_iter()
        .collect())
}

#[cfg(test)]
#[path = "docker_tests.rs"]
mod tests;
