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
//! `hostAddress` the socket's path), HTTPS with a client certificate (`tlsCaCert`, `tlsClientCert`, `tlsClientKey`), or
//! plain HTTP to this machine only (SEC-OUT-08). The hosted-instance services reach a host through this client too
//! (`call#DockerApi`). A job's container, and each of its service containers, has no privilege, no capabilities, no host
//! socket and no host path, a non-root user, an init that reaps orphans, a read-only root with a sized tmpfs at `/tmp`,
//! its working directory on a sized tmpfs (or on the root filesystem the pool's `diskMb` sizes), and the pool's
//! limits. Its names carry this server's id, and what it removes has this server's label
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
use super::network_name;

type Parameters = BTreeMap<String, Value>;
type Out = Result<BTreeMap<String, Value>, LibError>;

/// The API version asked for: Docker 19.03 and later, and Podman's compatibility API.
const API: &str = "/v1.40";
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// An image pull may take this long; pulls of one host take turns, and those of other hosts go on.
const PULL_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// The pulls of each host (by its address), one at a time.
static PULLS: std::sync::Mutex<BTreeMap<String, std::sync::Arc<std::sync::Mutex<()>>>> =
    std::sync::Mutex::new(BTreeMap::new());

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    rust_service!(map, start_run_docker)?;
    rust_service!(map, inspect_run_docker)?;
    rust_service!(map, remove_run_docker)?;
    rust_service!(map, cleanup_runs_docker)?;
    rust_service!(map, call_docker_api)?;
    Ok(())
}

/// A container of a listing.
struct Listed {
    id: String,
    labels: BTreeMap<String, String>,
    /// When it was made, epoch seconds.
    created: i64,
}

/// A connection to a host's API.
pub struct DockerHost {
    client: RestClient,
    base: String,
    /// What tells this host from another: its socket path or address.
    key: String,
}

impl DockerHost {
    /// The client for an `InstanceHost` row.
    ///
    /// # Errors
    ///
    /// When the row names no address, or plain http to a host that is not this machine (SEC-OUT-08).
    pub fn of(host: &EntityValue) -> Result<Self, Error> {
        Self::connect(host, None)
    }

    /// The client for an `InstanceHost` row, on `default_port` when the row has no `adminPort` (the hosted-instance
    /// services' 2375). Plain http goes only to this machine's loopback ([`moqui_lib::net::loopback_host`]): the
    /// Docker Engine API has no authentication of its own, and a registry login rides on an image pull.
    fn connect(host: &EntityValue, default_port: Option<u16>) -> Result<Self, Error> {
        let address = field(host, "hostAddress")
            .ok_or_else(|| Error::new("the instance host has no hostAddress (a socket path or a host name)"))?;
        let protocol = field(host, "hostProtocol").map_or_else(|| "http".to_string(), |text| text.to_ascii_lowercase());
        let key = format!("{protocol}:{address}");
        let mut client = RestClient::new().timeout(CALL_TIMEOUT);
        let base = match protocol.as_str() {
            "unix" => {
                client = client.unix_socket(address.trim_start_matches("unix://"));
                "http://localhost".to_string()
            }
            "http" if !moqui_lib::net::loopback_host(&address) => {
                return Err(Error::new(format!(
                    "the Docker host {address} is not reached over plain http, which goes only to this machine \
                     (localhost, 127.0.0.0/8 or ::1): use https, with the host's tlsCaCert, tlsClientCert and \
                     tlsClientKey, or a unix socket"
                )))
            }
            "http" | "https" => {
                let port = field(host, "adminPort")
                    .or_else(|| default_port.map(|port| port.to_string()))
                    .map_or(String::new(), |port| format!(":{port}"));
                // an IPv6 address is bracketed in a URL
                let name = if address.parse::<std::net::Ipv6Addr>().is_ok() {
                    format!("[{address}]")
                } else {
                    address.clone()
                };
                format!("{protocol}://{name}{port}")
            }
            other => return Err(Error::new(format!("hostProtocol {other} is not unix, http or https"))),
        };
        if let Some(ca) = field(host, "tlsCaCert") {
            client = client.root_certificates_pem(ca.into_bytes());
        }
        if let (Some(cert), Some(key)) = (field(host, "tlsClientCert"), field(host, "tlsClientKey")) {
            client = client.client_identity_pem(format!("{}\n{}\n", cert.trim(), key.trim()).into_bytes());
        }
        Ok(Self { client, base, key })
    }

    /// A host reached over a Unix socket, with no `InstanceHost` row.
    pub fn unix(socket: impl Into<std::path::PathBuf>) -> Self {
        let socket = socket.into();
        Self {
            key: format!("unix:{}", socket.display()),
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
        let id = self.create(name, &create_body(&parameters, &Hardening::default(), None), true)?;
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
        let turn = std::sync::Arc::clone(moqui_lib::sync::lock(&PULLS).entry(self.key.clone()).or_default());
        let _turn = moqui_lib::sync::lock(&turn);
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

    /// Create a container, with `pull` pulling its image when the host does not have it. Its id.
    fn create(&self, name: &str, body: &serde_json::Value, pull: bool) -> Result<String, Error> {
        let path = format!("/containers/create?name={}", encode(name));
        let mut response = self.send(Method::Post, &path, Some(body), CALL_TIMEOUT)?;
        if response.status() == 404 {
            let image = body["Image"].as_str().unwrap_or_default();
            if !pull {
                return Err(Error::new(format!(
                    "the image {image} is not on the host, and the pool takes untrusted runs, whose images are not \
                     pulled on demand: pull it on the host first"
                )));
            }
            self.pull(image)?;
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

    /// With `container-user="image"`, a container made from an image whose user is not a numeric uid other than 0 (its
    /// `Config.User` empty, root, or a name) is refused before it starts: a job never runs as root, in any form, and a
    /// name is whatever the image's own `/etc/passwd` says, which may be uid 0 under another name
    /// ([`is_numeric_user`]).
    ///
    /// Whatever the user, a container whose image declares a `VOLUME` is refused too: the engine makes an anonymous
    /// volume at each such path, writable, on the host's disk and sized by neither `StorageOpt` nor a tmpfs, there even
    /// with a read-only root (SEC-GIT-93). The checks read the made container once, before it starts.
    fn check_made(&self, id: &str, name: &str, image_user: bool) -> Result<(), Error> {
        let response = self.send(Method::Get, &format!("/containers/{id}/json"), None, CALL_TIMEOUT)?;
        if response.status() >= 300 {
            return Err(Self::problem(&format!("inspecting container {name}"), &response));
        }
        let made = serde_json::from_str::<serde_json::Value>(response.body()).unwrap_or(serde_json::Value::Null);
        let mut volumes: Vec<String> = made["Config"]["Volumes"]
            .as_object()
            .map(|volumes| volumes.keys().cloned().collect())
            .unwrap_or_default();
        if let Some(mounts) = made["Mounts"].as_array() {
            volumes.extend(
                mounts
                    .iter()
                    .filter(|mount| mount["Type"] == "volume")
                    .filter_map(|mount| mount["Destination"].as_str().map(str::to_string)),
            );
        }
        volumes.sort();
        volumes.dedup();
        if !volumes.is_empty() {
            return Err(Error::new(format!(
                "the image of container {name} declares a VOLUME ({}), which the engine would make on the host's disk \
                 outside the job's limits: a CI image declares none",
                volumes.join(", ")
            )));
        }
        if !image_user {
            return Ok(());
        }
        let user = made["Config"]["User"].as_str().map(str::to_string).unwrap_or_default();
        if !is_numeric_user(&user) {
            return Err(Error::new(format!(
                "the image of container {name} may run as root (its user is {}), and git-ci container-user=\"image\" \
                 takes only an image whose USER is a numeric uid other than 0 (a name may be uid 0 in the image): set \
                 its USER to a uid, or set container-user to a uid",
                if user.trim().is_empty() { "unset" } else { user.trim() }
            )));
        }
        Ok(())
    }

    fn start(&self, id: &str) -> Result<(), Error> {
        let response = self.send(Method::Post, &format!("/containers/{id}/start"), None, CALL_TIMEOUT)?;
        // 304: already started
        if response.status() >= 300 && response.status() != 304 {
            return Err(Self::problem(&format!("starting container {id}"), &response));
        }
        Ok(())
    }

    /// Whether container `id` carries this server's label (`moqui.ci.server` of [`super::server_id`]): what this
    /// server made, and so may remove. One that is not there does not.
    fn ours(&self, id: &str) -> Result<bool, Error> {
        let response = self.send(Method::Get, &format!("/containers/{id}/json"), None, CALL_TIMEOUT)?;
        if response.status() == 404 {
            return Ok(false);
        }
        if response.status() >= 300 {
            return Err(Self::problem(&format!("inspecting container {id}"), &response));
        }
        let body: serde_json::Value = serde_json::from_str(response.body()).unwrap_or(serde_json::Value::Null);
        Ok(body["Config"]["Labels"][super::LABEL_SERVER].as_str() == Some(super::server_id().as_str()))
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
                        Some(Listed {
                            id: container["Id"].as_str()?.to_string(),
                            labels,
                            created: container["Created"].as_i64().unwrap_or(0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Make a job's network; `internal` reaches nothing outside it. One there already (409) is an error: it is
    /// another run's.
    fn create_network(&self, name: &str, labels: &serde_json::Value, internal: bool) -> Result<(), Error> {
        let response = self.send(
            Method::Post,
            "/networks/create",
            Some(&json!({"Name": name, "Driver": "bridge", "Internal": internal, "Labels": labels})),
            CALL_TIMEOUT,
        )?;
        if response.status() >= 300 {
            return Err(Self::problem(&format!("creating network {name}"), &response));
        }
        Ok(())
    }

    /// Put container `id` on the network `name` too.
    fn connect_network(&self, name: &str, id: &str) -> Result<(), Error> {
        let response = self.send(
            Method::Post,
            &format!("/networks/{}/connect", encode(name)),
            Some(&json!({"Container": id})),
            CALL_TIMEOUT,
        )?;
        if response.status() >= 300 {
            return Err(Self::problem(
                &format!("connecting container {id} to network {name}"),
                &response,
            ));
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

/// `instanceRef` when it is a container id as Docker gives one (hex digits, a full id or a prefix): nothing else is
/// put in an API path. `None` when it is not given; an error recorded when it is not an id.
fn container_ref(parameters: &Parameters) -> Result<Option<String>, ()> {
    let Some(id) = text(parameters, "instanceRef") else {
        return Ok(None);
    };
    if id.is_empty() || id.len() > 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        add_error(format!("instanceRef {id} is not a container id"));
        return Err(());
    }
    Ok(Some(id))
}

/// The label filters of `filterLabels`, and always this server's own label: a listing names only what this server
/// made, whatever the caller asked.
fn own_filters(parameters: &Parameters) -> Vec<String> {
    let mut filters: Vec<String> = string_pairs(parameters.get("filterLabels"))
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    let own = format!("{}={}", super::LABEL_SERVER, super::server_id());
    if !filters.contains(&own) {
        filters.push(own);
    }
    filters
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
/// image's with `image`), `container-read-only-root` (true), `container-workdir` (/workspace), `container-tmp-mb`
/// (1024) and `container-workspace-mb` (4096, the working directory's tmpfs when the pool sets no `diskMb`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hardening {
    pub user: Option<String>,
    pub read_only_root: bool,
    pub workdir: String,
    pub tmp_mb: u64,
    pub workspace_mb: u64,
}

impl Default for Hardening {
    fn default() -> Self {
        Self {
            user: Some("1000:1000".to_string()),
            read_only_root: true,
            workdir: "/workspace".to_string(),
            tmp_mb: 1024,
            workspace_mb: 4096,
        }
    }
}

/// Whether a container user is root in any form: uid 0 or the name root, with or without a group.
pub fn is_root_user(user: &str) -> bool {
    let name = user.split(':').next().unwrap_or_default().trim();
    name.eq_ignore_ascii_case("root") || (!name.is_empty() && name.bytes().all(|byte| byte == b'0'))
}

/// Whether an image's user is a numeric uid other than 0, with or without a group (`1000`, `1000:1000`): the only user
/// `container-user="image"` takes, since a name is resolved by the image's own `/etc/passwd`, which an image may make
/// uid 0 under any name.
pub fn is_numeric_user(user: &str) -> bool {
    let uid = user.split(':').next().unwrap_or_default().trim();
    !uid.is_empty() && uid.bytes().all(|byte| byte.is_ascii_digit()) && !is_root_user(uid)
}

impl Hardening {
    pub fn from_conf() -> Self {
        let default = Self::default();
        let setting = super::super::setting;
        Self {
            user: match setting("container-user").as_str() {
                "" => default.user,
                "image" => None,
                // never root, whatever the conf says
                user if is_root_user(user) => default.user,
                user => Some(user.to_string()),
            },
            read_only_root: setting("container-read-only-root") != "false",
            workdir: Some(setting("container-workdir"))
                .filter(|dir| dir.starts_with('/'))
                .unwrap_or(default.workdir),
            tmp_mb: super::super::number_setting("container-tmp-mb", default.tmp_mb),
            workspace_mb: super::super::number_setting("container-workspace-mb", default.workspace_mb),
        }
    }

    /// The tmpfs options of a mount the container's user writes: owned by it when the user is numeric.
    fn tmpfs(&self, mb: u64) -> String {
        let owner = self
            .user
            .as_deref()
            .and_then(|user| {
                let (uid, gid) = user.split_once(':').unwrap_or((user, user));
                (uid.bytes().all(|byte| byte.is_ascii_digit()) && gid.bytes().all(|byte| byte.is_ascii_digit()))
                    .then(|| format!(",uid={uid},gid={gid},mode=0700"))
            })
            .unwrap_or_else(|| ",mode=1777".to_string());
        format!("rw,exec,nosuid,nodev,size={mb}m{owner}")
    }
}

/// The network mode a pool's `egressRule` asks for: `bridge` when empty, else the mode when it is one a pool may have
/// ([`valid_egress`]); any other (the host's network, another container's, a namespace path) is `none`.
pub fn egress_mode(rule: Option<&str>) -> String {
    match rule.map(str::trim).filter(|rule| !rule.is_empty()) {
        None => "bridge".to_string(),
        Some(rule) if !valid_egress(rule) => "none".to_string(),
        Some(rule) => rule.to_string(),
    }
}

/// Whether `rule` may be a pool's `egressRule`: `bridge`, `none`, or the name of a network of the host (letters,
/// digits, dot, dash and underscore). A list of what is refused would miss a mode: on Podman `ns:<path>` joins the
/// network namespace at a path, the host's included (`ns:/proc/1/ns/net`), and `slirp4netns:...` and `pasta:...`
/// take options; `host` and `container:...` are the host's network and another container's (SEC-GIT-93).
pub fn valid_egress(rule: &str) -> bool {
    let rule = rule.trim();
    if rule.eq_ignore_ascii_case("host") {
        return false;
    }
    rule == "bridge" || rule == "none" || crate::ci::file::valid_name(rule)
}

/// The limits of the pool on a container's host config: CPUs, memory with no swap past it, processes, the root
/// filesystem's size.
fn limits(host: &mut serde_json::Value, parameters: &Parameters) {
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
    // the working directory (the checkout, the caches and HOME) is bounded: on the root filesystem the pool's diskMb
    // sizes, which is then writable, else on a tmpfs of container-workspace-mb (it counts toward the memory limit)
    let sized_root = number(parameters, "diskMb").is_some_and(|disk| disk > 0);
    let mut tmpfs = serde_json::Map::new();
    tmpfs.insert("/tmp".to_string(), json!(hardening.tmpfs(hardening.tmp_mb)));
    if !sized_root {
        tmpfs.insert(
            hardening.workdir.clone(),
            json!(hardening.tmpfs(hardening.workspace_mb)),
        );
    }
    let mut host = json!({
        "Privileged": false,
        "CapDrop": ["ALL"],
        "SecurityOpt": ["no-new-privileges"],
        "ReadonlyRootfs": hardening.read_only_root && !sized_root,
        "AutoRemove": false,
        "PublishAllPorts": false,
        "Binds": [],
        // an init as process 1 reaps what a step leaves, so orphans do not use up the pids limit
        "Init": true,
        "Tmpfs": tmpfs,
        "NetworkMode": network
            .map(str::to_string)
            .unwrap_or_else(|| egress_mode(text(parameters, "networkMode").as_deref())),
    });
    limits(&mut host, parameters);
    let mut body = json!({
        "Image": text(parameters, "image").unwrap_or_default(),
        "Env": env,
        "Labels": labels,
        "WorkingDir": hardening.workdir,
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
/// network: kept in as the job's own container is (its user, no capabilities, a read-only root with a sized tmpfs at
/// `/tmp`, an init, the pool's limits). A service image runs as that user and writes under `/tmp`.
pub fn service_body(
    service: &BTreeMap<String, Value>,
    labels: &serde_json::Value,
    network: &str,
    parameters: &Parameters,
    hardening: &Hardening,
) -> serde_json::Value {
    let env: Vec<String> = string_pairs(service.get("envMap"))
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    let name = service.get("name").map(to_plain_string).unwrap_or_default();
    let sized_root = number(parameters, "diskMb").is_some_and(|disk| disk > 0);
    let mut host = json!({
        "Privileged": false,
        "CapDrop": ["ALL"],
        "SecurityOpt": ["no-new-privileges"],
        "ReadonlyRootfs": hardening.read_only_root && !sized_root,
        "AutoRemove": false,
        "PublishAllPorts": false,
        "Binds": [],
        "Init": true,
        "Tmpfs": { "/tmp": hardening.tmpfs(hardening.tmp_mb) },
        "NetworkMode": network,
    });
    limits(&mut host, parameters);
    let mut body = json!({
        "Image": service.get("image").map(to_plain_string).unwrap_or_default(),
        "Env": env,
        "Labels": labels,
        "HostConfig": host,
        "NetworkingConfig": { "EndpointsConfig": { network: { "Aliases": [name] } } },
    });
    if let Some(user) = &hardening.user {
        body["User"] = json!(user);
    }
    body
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
    // the job's own network reaches out only when the pool's does (bridge); for none or a network of the host it is
    // internal, and the job's container is put on the pool's network too, so egress follows the pool
    let egress = egress_mode(text(parameters, "networkMode").as_deref());
    let pull = parameters
        .get("pullImages")
        .is_some_and(|pull| to_plain_string(pull) == "true");
    let hardening = Hardening::from_conf();
    let mut made: Vec<String> = Vec::new();
    let started = (|| -> Result<String, Error> {
        if let Some(network) = &network {
            host.create_network(network, &labels, egress != "bridge")?;
            for service in &services {
                let service_name = service.get("name").map(to_plain_string).unwrap_or_default();
                let id = host.create(
                    &format!("{name}-{service_name}"),
                    &service_body(service, &labels, network, parameters, &hardening),
                    pull,
                )?;
                made.push(id.clone());
                host.check_made(&id, &format!("{name}-{service_name}"), hardening.user.is_none())?;
                host.start(&id)?;
            }
        }
        let id = host.create(&name, &create_body(parameters, &hardening, network.as_deref()), pull)?;
        made.push(id.clone());
        host.check_made(&id, &name, hardening.user.is_none())?;
        if network.is_some() && egress != "bridge" && egress != "none" {
            host.connect_network(&egress, &id)?;
        }
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
    let id = match container_ref(parameters) {
        Ok(Some(id)) => id,
        Ok(None) => {
            add_error("instanceRef is required");
            return Ok(BTreeMap::new());
        }
        Err(()) => return Ok(BTreeMap::new()),
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
    let Ok(named) = container_ref(parameters) else {
        return Ok(BTreeMap::new());
    };
    // the container named goes only when this server made it (its label): an id names any container of the host
    let mut ids: Vec<String> = Vec::new();
    if let Some(id) = named {
        match host.ours(&id) {
            Ok(true) => ids.push(id),
            Ok(false) => tracing::info!("container {id} is not this server's (or not there); not removed"),
            Err(err) => add_error(err.message()),
        }
    }
    if let (Some(label), Some(value)) = (text(parameters, "labelName"), text(parameters, "labelValue")) {
        // only this server's: another server's job run of the same id on a shared host is not this one's
        let mut filters = own_filters(parameters);
        filters.push(format!("{label}={value}"));
        match host.list(&filters) {
            Ok(listed) => {
                for container in listed {
                    if !ids.contains(&container.id) {
                        ids.push(container.id);
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
    if let Some(network) = text(parameters, "networkName") {
        host.remove_network(&network);
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
    let mut filters = own_filters(parameters);
    filters.push(label.clone());
    let listed = match host.list(&filters) {
        Ok(listed) => listed,
        Err(err) => {
            add_error(err.message());
            return Ok(BTreeMap::new());
        }
    };
    // one made after the caller read what to keep may be a job that just started
    let before = number(parameters, "createdBefore");
    let mut removed = 0;
    for container in listed {
        let Some(job) = container.labels.get(&label) else {
            continue;
        };
        if keep.contains(job) || before.is_some_and(|before| container.created >= before) {
            continue;
        }
        // one container that does not go does not keep the others
        match host.remove(&container.id) {
            Ok(()) => {
                removed += 1;
                host.remove_network(&network_name(&super::container_name(job)));
            }
            Err(err) => tracing::warn!("a container of an ended job run was not removed: {err}"),
        }
    }
    Ok([("removedCount".to_string(), Value::I64(removed))]
        .into_iter()
        .collect())
}

/// The permission `call#DockerApi` needs, as the hosted-instance services check it.
const INSTANCE_ADMIN: &str = "INSTANCE_ADMIN";
/// The hosted-instance services' port when the host row has none, as they had it.
const INSTANCE_PORT: u16 = 2375;
/// The longest `timeoutSeconds` a call may ask for.
const MAX_CALL_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// `call#DockerApi`: one Docker Engine API call to an `InstanceHost`, for the hosted-instance services of
/// `InstanceServices.xml` (`init#InstanceDocker` and the others), on the client the CI runs use ([`DockerHost`]):
/// a unix socket, https with the row's TLS fields, or plain http to this machine only (SEC-OUT-08). Needs
/// `INSTANCE_ADMIN` unless authorization is disabled. In: `instanceHostId`, `method`, `path` (under the API version),
/// `query`, `body` (JSON text sent as it is, or a map or list), `registryAuth` (sent as `X-Registry-Auth`),
/// `timeoutSeconds`. Out: `statusCode`, `reasonPhrase` and `responseBody` (an object or list when the answer is one in
/// JSON, else its text). A refusal or a failure to connect is a message error and no `statusCode`.
fn call_docker_api(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let authz_disabled = ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(false);
    if !authz_disabled && !moqui_core::user::has_permission(INSTANCE_ADMIN).unwrap_or(false) {
        add_error(format!("Hosted instances need the {INSTANCE_ADMIN} permission"));
        return Ok(BTreeMap::new());
    }
    let Some(host_id) = text(parameters, "instanceHostId") else {
        add_error("instanceHostId is required");
        return Ok(BTreeMap::new());
    };
    let method_name = text(parameters, "method").unwrap_or_else(|| "GET".to_string());
    let Some(method) = Method::parse(&method_name) else {
        add_error(format!("HTTP method {method_name} is not supported"));
        return Ok(BTreeMap::new());
    };
    let path = text(parameters, "path").unwrap_or_default();
    if !path.starts_with('/') || path.contains("..") || path.contains(['?', '#']) || path.contains(char::is_whitespace)
    {
        add_error(format!("Docker API path {path} is not a path under the API version"));
        return Ok(BTreeMap::new());
    }
    let host = match super::host_row(facade, &host_id).and_then(|row| DockerHost::connect(&row, Some(INSTANCE_PORT))) {
        Ok(host) => host,
        Err(err) => {
            add_error(format!("Docker host {host_id}: {}", err.message()));
            return Ok(BTreeMap::new());
        }
    };
    let query: Vec<String> = string_pairs(parameters.get("query"))
        .into_iter()
        .map(|(name, value)| format!("{}={}", encode(&name), encode(&value)))
        .collect();
    let target = if query.is_empty() {
        path
    } else {
        format!("{path}?{}", query.join("&"))
    };
    let timeout = number(parameters, "timeoutSeconds")
        .and_then(|seconds| u64::try_from(seconds).ok())
        .filter(|seconds| *seconds > 0)
        .map_or(CALL_TIMEOUT, |seconds| {
            Duration::from_secs(seconds).min(MAX_CALL_TIMEOUT)
        });
    let url = format!("{}{API}{target}", host.base);
    if let Err(refused) = moqui_core::entity::fence::check_effect(
        moqui_core::entity::fence::Effect::RemoteCall,
        &format!("HTTP {method} {}", moqui_lib::http::redacted(&url)),
    ) {
        add_error(refused.message());
        return Ok(BTreeMap::new());
    }
    // sim mode sends nothing, as the script REST client
    let sim = facade.sim_active();
    let mut call = host.client.request(method, url).timeout(timeout);
    if let Some(auth) = text(parameters, "registryAuth") {
        call = call.header("X-Registry-Auth", auth);
    }
    match parameters.get("body") {
        Some(Value::String(body)) if !body.trim().is_empty() => call = call.text("application/json", body.clone()),
        Some(body @ (Value::Map(_) | Value::List(_))) => call = call.json(&moqui_lib::json::to_json(body)),
        _ => {}
    }
    let response = match call.send(sim) {
        Ok(response) => response,
        Err(err) => {
            tracing::error!("Error connecting to Docker host {host_id}: {err}");
            add_error(format!("Error connecting to Docker host {host_id}: {}", err.message()));
            return Ok(BTreeMap::new());
        }
    };
    let body = match serde_json::from_str::<serde_json::Value>(response.body()) {
        Ok(json @ (serde_json::Value::Object(_) | serde_json::Value::Array(_))) => moqui_lib::json::from_json(&json),
        _ => Value::string(response.body()),
    };
    Ok([
        ("statusCode".to_string(), Value::I64(i64::from(response.status()))),
        (
            "reasonPhrase".to_string(),
            Value::string(moqui_core::outbound::reason_phrase(response.status())),
        ),
        ("responseBody".to_string(), body),
    ]
    .into_iter()
    .collect())
}

#[cfg(test)]
#[path = "docker_tests.rs"]
mod tests;
