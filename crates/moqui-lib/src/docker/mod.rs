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

//! A client of the Docker Engine API, which Podman serves on its socket too (plans/Dogfood.md D19, F8;
//! plans/Vault-Instances.md §4.2): the one container driver that CI's runs (`moqui_git::ci::driver::docker`) and
//! managed instances share. A host is reached over a Unix socket, HTTPS with a client certificate, or plain HTTP to
//! this machine only (SEC-OUT-08). This module knows no entity: the caller maps its host row to a [`HostAddress`].
//!
//! Beside the calls CI's runs make, it makes containers that stay ([`ContainerSpec`], [`DockerHost::create_spec`]):
//! created without starting, started, stopped, restarted, inspected, their logs read and removed, with volumes and
//! networks. No secret goes into a container's environment, labels or command: a secret is a file under
//! `/run/secrets`, put into the created container before it starts on Docker ([`archive`]), a Podman secret on Podman
//! ([`podman`]). [`fake`] is a fake engine for the tests of what uses the driver (feature `docker-fake`).

mod archive;
mod container;
#[cfg(any(test, feature = "docker-fake"))]
pub mod fake;
mod podman;
mod spec;

pub use archive::secrets_tar;
pub use container::{demux_logs, ContainerInfo, VolumeInfo, MAX_LOG_LINES};
pub use podman::{is_podman, LIBPOD};
pub use spec::{
    valid_name, valid_network, ContainerSpec, HealthCheck, Mount, MountKind, PortBinding, RestartPolicy, SecretFile,
    NETWORK_KEYWORDS, SECRETS_DIR, SECRETS_LABEL,
};

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::json;

use crate::error::Error;
use crate::http::{Method, RestClient, RestResponse};

/// The API version asked for: Docker 19.03 and later, and Podman's compatibility API.
pub const API: &str = "/v1.40";
/// A call's limit, unless it says otherwise.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// An image pull may take this long; pulls of one host take turns, and those of other hosts go on.
pub const PULL_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// The pulls of each host (by its address), one at a time.
static PULLS: std::sync::Mutex<BTreeMap<String, std::sync::Arc<std::sync::Mutex<()>>>> =
    std::sync::Mutex::new(BTreeMap::new());

/// Where a host is and how it is reached: the plain values of an `InstanceHost` row (`hostProtocol`, `hostAddress`,
/// `adminPort`, `tlsCaCert`, `tlsClientCert`, `tlsClientKey`), each `None` when the row has none. Its `Debug` shows no
/// client key.
#[derive(Clone, Default)]
pub struct HostAddress {
    pub protocol: Option<String>,
    pub address: Option<String>,
    pub port: Option<String>,
    pub tls_ca_cert: Option<String>,
    pub tls_client_cert: Option<String>,
    pub tls_client_key: Option<String>,
}

impl std::fmt::Debug for HostAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostAddress")
            .field("protocol", &self.protocol)
            .field("address", &self.address)
            .field("port", &self.port)
            .field("tls_ca_cert", &self.tls_ca_cert.is_some())
            .field("tls_client_cert", &self.tls_client_cert.is_some())
            .field("tls_client_key", &self.tls_client_key.as_ref().map(|_| ".."))
            .finish()
    }
}

/// A container of a listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub id: String,
    pub labels: BTreeMap<String, String>,
    /// When it was made, epoch seconds.
    pub created: i64,
}

/// What `POST /containers/create` answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Created {
    /// The new container's id.
    Id(String),
    /// The host does not have the image (404).
    NoSuchImage,
}

/// Which engine serves a host's API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    Docker,
    /// Podman, whose own API makes the secret files ([`podman`]).
    Podman,
}

/// A connection to a host's API.
pub struct DockerHost {
    client: RestClient,
    base: String,
    /// What tells this host from another: its socket path or address.
    key: String,
    /// The engine, once asked.
    engine: std::sync::OnceLock<Engine>,
}

impl std::fmt::Debug for DockerHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockerHost")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl DockerHost {
    /// The client for a host, on `default_port` when it has no port (the hosted-instance services' 2375). Plain http
    /// goes only to this machine's loopback ([`crate::net::loopback_host`]): the Docker Engine API has no
    /// authentication of its own, and a registry login rides on an image pull.
    ///
    /// # Errors
    ///
    /// When it names no address, a protocol that is not unix, http or https, or plain http to a host that is not this
    /// machine (SEC-OUT-08).
    pub fn connect(host: &HostAddress, default_port: Option<u16>) -> Result<Self, Error> {
        let address = nonempty(host.address.as_deref())
            .ok_or_else(|| Error::new("the instance host has no hostAddress (a socket path or a host name)"))?;
        let protocol = nonempty(host.protocol.as_deref()).map_or_else(|| "http".to_string(), str::to_ascii_lowercase);
        let key = format!("{protocol}:{address}");
        let mut client = RestClient::new().timeout(CALL_TIMEOUT);
        let base = match protocol.as_str() {
            "unix" => {
                client = client.unix_socket(address.trim_start_matches("unix://"));
                "http://localhost".to_string()
            }
            "http" if !crate::net::loopback_host(address) => {
                return Err(Error::new(format!(
                    "the Docker host {address} is not reached over plain http, which goes only to this machine \
                     (localhost, 127.0.0.0/8 or ::1): use https, with the host's tlsCaCert, tlsClientCert and \
                     tlsClientKey, or a unix socket"
                )))
            }
            "http" | "https" => {
                let port = nonempty(host.port.as_deref())
                    .map(str::to_string)
                    .or_else(|| default_port.map(|port| port.to_string()))
                    .map_or(String::new(), |port| format!(":{port}"));
                // an IPv6 address is bracketed in a URL
                let name = if address.parse::<std::net::Ipv6Addr>().is_ok() {
                    format!("[{address}]")
                } else {
                    address.to_string()
                };
                format!("{protocol}://{name}{port}")
            }
            other => return Err(Error::new(format!("hostProtocol {other} is not unix, http or https"))),
        };
        if let Some(ca) = nonempty(host.tls_ca_cert.as_deref()) {
            client = client.root_certificates_pem(ca.as_bytes().to_vec());
        }
        if let (Some(cert), Some(key)) = (
            nonempty(host.tls_client_cert.as_deref()),
            nonempty(host.tls_client_key.as_deref()),
        ) {
            client = client.client_identity_pem(format!("{}\n{}\n", cert.trim(), key.trim()).into_bytes());
        }
        Ok(Self {
            client,
            base,
            key,
            engine: std::sync::OnceLock::new(),
        })
    }

    /// A host reached over a Unix socket.
    pub fn unix(socket: impl Into<std::path::PathBuf>) -> Self {
        let socket = socket.into();
        Self {
            key: format!("unix:{}", socket.display()),
            client: RestClient::new().timeout(CALL_TIMEOUT).unix_socket(socket),
            base: "http://localhost".to_string(),
            engine: std::sync::OnceLock::new(),
        }
    }

    /// Which engine serves this host: Podman when `GET /version` names the component "Podman Engine", else Docker.
    /// Asked once.
    ///
    /// # Errors
    ///
    /// When the host does not answer, or refuses.
    pub fn engine(&self) -> Result<Engine, Error> {
        if let Some(engine) = self.engine.get() {
            return Ok(*engine);
        }
        let response = self.send(Method::Get, "/version", None, CALL_TIMEOUT)?;
        if response.status() >= 300 {
            return Err(Self::problem("asking the host's version", &response));
        }
        let version: serde_json::Value = serde_json::from_str(response.body()).unwrap_or(serde_json::Value::Null);
        let engine = if is_podman(&version) {
            Engine::Podman
        } else {
            Engine::Docker
        };
        Ok(*self.engine.get_or_init(|| engine))
    }

    /// The HTTP client of this host, for a call this module does not make (`call#DockerApi`).
    pub fn client(&self) -> &RestClient {
        &self.client
    }

    /// The URL of `target` (a path under the API version, with its query).
    pub fn url(&self, target: &str) -> String {
        format!("{}{API}{target}", self.base)
    }

    /// One call to the host, under the API version.
    ///
    /// # Errors
    ///
    /// When the call could not be sent or its answer read; an answer of any status is `Ok`.
    pub fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&serde_json::Value>,
        timeout: Duration,
    ) -> Result<RestResponse, Error> {
        let mut call = self
            .client
            .request(method, self.url(path))
            .timeout(timeout)
            .allow_in_sim(true);
        if let Some(body) = body {
            call = call.json(body);
        }
        call.send(false)
    }

    /// The message of an API error response.
    pub fn problem(what: &str, response: &RestResponse) -> Error {
        let message = serde_json::from_str::<serde_json::Value>(response.body())
            .ok()
            .and_then(|body| body["message"].as_str().map(str::to_string))
            .unwrap_or_else(|| response.body().chars().take(300).collect());
        Error::new(format!("{what}: {} {message}", response.status()))
    }

    /// Pull `image` (`name:tag`, `latest` when it names no tag); the pulls of one host take turns.
    ///
    /// # Errors
    ///
    /// When the host refuses the pull, or its stream ends with an error.
    pub fn pull(&self, image: &str) -> Result<(), Error> {
        let turn = std::sync::Arc::clone(crate::sync::lock(&PULLS).entry(self.key.clone()).or_default());
        let _turn = crate::sync::lock(&turn);
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

    /// The paths image `image` declares as a `VOLUME` (`GET /images/{name}/json`, its `Config.Volumes`), where the
    /// engine makes an anonymous volume on the host's disk unless the container mounts something there. `None` when
    /// the host does not have the image.
    ///
    /// # Errors
    ///
    /// When the host refuses the inspection for another reason.
    pub fn image_volumes(&self, image: &str) -> Result<Option<Vec<String>>, Error> {
        let response = self.send(
            Method::Get,
            &format!("/images/{}/json", encode(image)),
            None,
            CALL_TIMEOUT,
        )?;
        if response.status() == 404 {
            return Ok(None);
        }
        if response.status() >= 300 {
            return Err(Self::problem(&format!("inspecting image {image}"), &response));
        }
        let inspected = serde_json::from_str::<serde_json::Value>(response.body())
            .map_err(|err| Error::new(format!("inspecting image {image}: the answer is not JSON: {err}")))?;
        Ok(Some(
            inspected["Config"]["Volumes"]
                .as_object()
                .map(|volumes| volumes.keys().cloned().collect())
                .unwrap_or_default(),
        ))
    }

    /// One `POST /containers/create` of `body` as the container `name`.
    ///
    /// # Errors
    ///
    /// When the host refuses it for another reason than a missing image, or its answer has no id.
    pub fn create_container(&self, name: &str, body: &serde_json::Value) -> Result<Created, Error> {
        let path = format!("/containers/create?name={}", encode(name));
        let response = self.send(Method::Post, &path, Some(body), CALL_TIMEOUT)?;
        // a 404 for something else (a network that is not there) is an error, not a reason to pull
        if response.status() == 404 && names_an_image(response.body()) {
            return Ok(Created::NoSuchImage);
        }
        if response.status() >= 300 {
            return Err(Self::problem(&format!("creating container {name}"), &response));
        }
        serde_json::from_str::<serde_json::Value>(response.body())
            .ok()
            .and_then(|body| body["Id"].as_str().map(str::to_string))
            .map(Created::Id)
            .ok_or_else(|| Error::new(format!("creating container {name}: the answer has no Id")))
    }

    /// Create a container, with `pull` pulling its image when the host does not have it. Its id.
    ///
    /// # Errors
    ///
    /// When the host refuses it, or does not have the image and `pull` is false.
    pub fn create(&self, name: &str, body: &serde_json::Value, pull: bool) -> Result<String, Error> {
        let image = body["Image"].as_str().unwrap_or_default();
        match self.create_container(name, body)? {
            Created::Id(id) => Ok(id),
            Created::NoSuchImage if !pull => Err(Error::new(format!(
                "the image {image} is not on the host, and it is not pulled on demand: pull it on the host first"
            ))),
            Created::NoSuchImage => {
                self.pull(image)?;
                match self.create_container(name, body)? {
                    Created::Id(id) => Ok(id),
                    Created::NoSuchImage => Err(Error::new(format!(
                        "creating container {name}: the image {image} is not on the host after its pull"
                    ))),
                }
            }
        }
    }

    /// Start container `id`; one already started is started.
    ///
    /// # Errors
    ///
    /// When the host refuses it.
    pub fn start(&self, id: &str) -> Result<(), Error> {
        let response = self.send(Method::Post, &format!("/containers/{id}/start"), None, CALL_TIMEOUT)?;
        // 304: already started
        if response.status() >= 300 && response.status() != 304 {
            return Err(Self::problem(&format!("starting container {id}"), &response));
        }
        Ok(())
    }

    /// Remove a container with its anonymous volumes, running or not. One that is not there is removed. This deletes
    /// no Podman secret: a container made by [`Self::create_spec`] is removed by [`Self::remove_container`].
    ///
    /// # Errors
    ///
    /// When the host refuses it.
    pub fn remove(&self, id: &str) -> Result<(), Error> {
        let response = self.send(
            Method::Delete,
            &format!("/containers/{id}?force=true&v=true"),
            None,
            CALL_TIMEOUT,
        )?;
        if response.status() >= 300 && response.status() != 404 {
            self.gone_anyway(id, &response)?;
        }
        Ok(())
    }

    /// The containers, running or not, with all of `labels` (`name=value`): each one's id and labels.
    ///
    /// # Errors
    ///
    /// When the host refuses the listing, or its answer is not JSON.
    pub fn list(&self, labels: &[String]) -> Result<Vec<Listed>, Error> {
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

    /// Make a network; `internal` reaches nothing outside it. One there already (409) is an error.
    ///
    /// # Errors
    ///
    /// When the host refuses it.
    pub fn create_network(&self, name: &str, labels: &serde_json::Value, internal: bool) -> Result<(), Error> {
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
    ///
    /// # Errors
    ///
    /// When the host refuses it.
    pub fn connect_network(&self, name: &str, id: &str) -> Result<(), Error> {
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

    /// Remove the network `name`; one that is not there is removed, and a failure is logged.
    pub fn remove_network(&self, name: &str) {
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

/// `text` when it is not empty or only white space.
fn nonempty(text: Option<&str>) -> Option<&str> {
    text.filter(|text| !text.trim().is_empty())
}

/// Whether a 404 answer's body says an image is missing (Docker's "No such image", Podman's "image not known").
pub(crate) fn names_an_image(body: &str) -> bool {
    body.to_ascii_lowercase().contains("image")
}

/// Percent-encode a query value.
pub fn encode(text: &str) -> String {
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

#[cfg(test)]
mod tests;
