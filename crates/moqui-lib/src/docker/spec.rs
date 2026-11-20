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

//! A container that stays (plans/Vault-Instances.md §4.2): what a managed instance's container is made of, its secret
//! files, and the check that no secret is in its environment, labels or command, where `docker inspect` and
//! `/proc/*/environ` show them. A secret is a file: put into the created container before it starts on Docker
//! ([`super::archive`]), a Podman secret on Podman ([`super::podman`]).

use std::time::Duration;

use serde_json::json;
use zeroize::Zeroizing;

use crate::error::Error;

/// The label that names a container's Podman secrets, so its removal deletes them. It holds names, never a value.
pub const SECRETS_LABEL: &str = "moqui.secrets";
/// Where the secret files are, in the container.
pub const SECRETS_DIR: &str = "/run/secrets";
/// A secret this long or longer is refused anywhere inside a value; a shorter one only as a whole value, so a short
/// secret does not refuse every value it happens to be part of.
const SUBSTRING_MIN: usize = 8;

/// A secret given to a container as the file `/run/secrets/<name>`, owned by `uid`:`gid` with `mode`. Its value is
/// zeroized when dropped, and its `Debug` does not show it.
pub struct SecretFile {
    pub name: String,
    pub bytes: Zeroizing<Vec<u8>>,
    pub uid: u32,
    pub gid: u32,
    pub mode: u32,
}

impl SecretFile {
    /// A secret file readable only by its owner (0400), owned by `uid`:`gid`.
    pub fn new(name: impl Into<String>, bytes: Zeroizing<Vec<u8>>, uid: u32, gid: u32) -> Self {
        Self {
            name: name.into(),
            bytes,
            uid,
            gid,
            mode: 0o400,
        }
    }

    /// Its path in the container.
    pub fn path(&self) -> String {
        format!("{SECRETS_DIR}/{}", self.name)
    }
}

impl std::fmt::Debug for SecretFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretFile")
            .field("name", &self.name)
            .field("bytes", &format_args!("({} bytes)", self.bytes.len()))
            .field("uid", &self.uid)
            .field("gid", &self.gid)
            .field("mode", &format_args!("{:o}", self.mode))
            .finish()
    }
}

/// What a mount's source is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountKind {
    /// A named volume of the host.
    Volume,
    /// A path of the host.
    Bind,
}

/// A volume or a host path in the container.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    pub kind: MountKind,
    /// The volume's name, or the host's path.
    pub source: String,
    /// The path in the container.
    pub target: String,
    pub read_only: bool,
}

/// A port of the container published on the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortBinding {
    pub container_port: u16,
    /// `tcp` or `udp`.
    pub protocol: String,
    /// The host's port; the engine picks one when `None`.
    pub host_port: Option<u16>,
    /// The host's address to listen on (`127.0.0.1`); every address when `None`.
    pub host_ip: Option<String>,
}

/// When the engine starts the container again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RestartPolicy {
    #[default]
    No,
    Always,
    UnlessStopped,
    /// On a non-zero exit, at most this many times (0: no limit).
    OnFailure(u32),
}

impl RestartPolicy {
    /// The policy's name in both APIs.
    pub fn name(self) -> &'static str {
        match self {
            Self::No => "no",
            Self::Always => "always",
            Self::UnlessStopped => "unless-stopped",
            Self::OnFailure(_) => "on-failure",
        }
    }

    fn retries(self) -> u32 {
        match self {
            Self::OnFailure(retries) => retries,
            _ => 0,
        }
    }
}

/// The engine's health check of the container. `test` is in the engine's form: `["CMD", "pg_isready"]`,
/// `["CMD-SHELL", "pg_isready -U app"]`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HealthCheck {
    pub test: Vec<String>,
    pub interval: Option<Duration>,
    pub timeout: Option<Duration>,
    pub retries: Option<u32>,
    pub start_period: Option<Duration>,
}

impl HealthCheck {
    fn json(&self) -> serde_json::Value {
        let nanos = |duration: Option<Duration>| duration.map_or(0, |duration| duration.as_nanos());
        json!({
            "Test": self.test,
            "Interval": u64::try_from(nanos(self.interval)).unwrap_or(u64::MAX),
            "Timeout": u64::try_from(nanos(self.timeout)).unwrap_or(u64::MAX),
            "Retries": self.retries.unwrap_or(0),
            "StartPeriod": u64::try_from(nanos(self.start_period)).unwrap_or(u64::MAX),
        })
    }
}

/// A container that stays: made by [`super::DockerHost::create_spec`], started, stopped and removed by its id.
#[derive(Debug, Default)]
pub struct ContainerSpec {
    pub name: String,
    pub image: String,
    /// Pull the image when the host does not have it.
    pub pull: bool,
    /// The command, when not the image's.
    pub cmd: Vec<String>,
    /// Settings that are not secret. A secret's value here is refused ([`Self::check`]): a secret is a file, read
    /// through the image's `*_FILE` setting (`POSTGRES_PASSWORD_FILE=/run/secrets/postgres-password`).
    pub env: Vec<(String, String)>,
    pub labels: Vec<(String, String)>,
    pub mounts: Vec<Mount>,
    /// The network it is on, when not the engine's default.
    pub network: Option<String>,
    pub ports: Vec<PortBinding>,
    pub restart: RestartPolicy,
    pub health: Option<HealthCheck>,
    /// The user it runs as (`uid:gid`), when not the image's.
    pub user: Option<String>,
    pub secrets: Vec<SecretFile>,
}

/// Whether `name` may be a container's, a volume's or a secret's name, and so a part of an API path or an archive's
/// entry: a letter or digit, then letters, digits, `_`, `.` and `-`.
pub fn valid_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= 128
        && bytes.next().is_some_and(|first| first.is_ascii_alphanumeric())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

/// Whether `secret` is in `value`: anywhere in it when the secret is [`SUBSTRING_MIN`] bytes or longer, else as the
/// whole value. Not constant time: both sides are this process's own, and nothing outside it times the check.
fn holds(value: &[u8], secret: &[u8]) -> bool {
    if secret.len() < SUBSTRING_MIN {
        return value == secret;
    }
    value.windows(secret.len()).any(|window| window == secret)
}

impl ContainerSpec {
    /// Refuse a spec that would put a secret where `docker inspect` shows it: a secret's value in an environment
    /// setting (name or value), a label (name or value), the command or the health check's command. Also refuse a
    /// name that is not a valid name, two secrets of one name, a secret with no value or a mode that is not a file
    /// mode, and a label of the driver's own ([`SECRETS_LABEL`]).
    ///
    /// # Errors
    ///
    /// The first thing refused, naming the secret and where it is but never its value.
    pub fn check(&self) -> Result<(), Error> {
        if !valid_name(&self.name) {
            return Err(Error::new(format!("{:?} is not a container name", self.name)));
        }
        if self.image.trim().is_empty() || self.image.contains(char::is_whitespace) {
            return Err(Error::new(format!(
                "container {}: {:?} is not an image",
                self.name, self.image
            )));
        }
        if self.labels.iter().any(|(name, _)| name == SECRETS_LABEL) {
            return Err(Error::new(format!(
                "container {}: the label {SECRETS_LABEL} is the driver's own",
                self.name
            )));
        }
        for (index, secret) in self.secrets.iter().enumerate() {
            // 64 at most, so its tar path fits a ustar header
            if !valid_name(&secret.name) || secret.name.len() > 64 {
                return Err(Error::new(format!(
                    "container {}: {:?} is not a secret file's name",
                    self.name, secret.name
                )));
            }
            if self.secrets[..index].iter().any(|other| other.name == secret.name) {
                return Err(Error::new(format!(
                    "container {}: two secrets are named {}",
                    self.name, secret.name
                )));
            }
            if secret.bytes.is_empty() {
                return Err(Error::new(format!(
                    "container {}: the secret {} has no value",
                    self.name, secret.name
                )));
            }
            if secret.mode > 0o777 {
                return Err(Error::new(format!(
                    "container {}: the secret {} has mode {:o}, which is not a file's",
                    self.name, secret.name, secret.mode
                )));
            }
            if let Some(place) = self.place_of(&secret.bytes) {
                return Err(Error::new(format!(
                    "container {}: the value of the secret {} is in its {place}; a secret is a file, never in a \
                     container's environment, labels or command (plans/Vault-Instances.md section 4.2)",
                    self.name, secret.name
                )));
            }
        }
        Ok(())
    }

    /// Where `secret` is in what `docker inspect` shows, if anywhere.
    fn place_of(&self, secret: &[u8]) -> Option<String> {
        for (name, value) in &self.env {
            if holds(name.as_bytes(), secret) || holds(value.as_bytes(), secret) {
                return Some(format!("environment ({name})"));
            }
        }
        for (name, value) in &self.labels {
            if holds(name.as_bytes(), secret) || holds(value.as_bytes(), secret) {
                return Some(format!("labels ({name})"));
            }
        }
        if self.cmd.iter().any(|word| holds(word.as_bytes(), secret)) {
            return Some("command".to_string());
        }
        let health = self.health.as_ref().map_or(&[][..], |health| &health.test[..]);
        if health.iter().any(|word| holds(word.as_bytes(), secret)) {
            return Some("health check".to_string());
        }
        None
    }

    fn env_list(&self) -> Vec<String> {
        self.env.iter().map(|(name, value)| format!("{name}={value}")).collect()
    }

    fn label_map(&self, extra: Option<(&str, String)>) -> serde_json::Map<String, serde_json::Value> {
        let mut labels: serde_json::Map<String, serde_json::Value> = self
            .labels
            .iter()
            .map(|(name, value)| (name.clone(), json!(value)))
            .collect();
        if let Some((name, value)) = extra {
            labels.insert(name.to_string(), json!(value));
        }
        labels
    }

    /// The body of Docker's `POST /containers/create`, which Podman's compatibility API takes too. No secret is in it:
    /// the secret files are put after it is made ([`super::archive`]).
    pub fn docker_body(&self) -> serde_json::Value {
        let mut exposed = serde_json::Map::new();
        let mut bindings = serde_json::Map::new();
        for port in &self.ports {
            let key = format!("{}/{}", port.container_port, port.protocol);
            exposed.insert(key.clone(), json!({}));
            let binding = json!({
                "HostIp": port.host_ip.clone().unwrap_or_default(),
                "HostPort": port.host_port.map(|port| port.to_string()).unwrap_or_default(),
            });
            match bindings.get_mut(&key) {
                Some(serde_json::Value::Array(list)) => list.push(binding),
                _ => {
                    bindings.insert(key, json!([binding]));
                }
            }
        }
        let mounts: Vec<serde_json::Value> = self
            .mounts
            .iter()
            .map(|mount| {
                json!({
                    "Type": match mount.kind { MountKind::Volume => "volume", MountKind::Bind => "bind" },
                    "Source": mount.source,
                    "Target": mount.target,
                    "ReadOnly": mount.read_only,
                })
            })
            .collect();
        let mut host = json!({
            "Mounts": mounts,
            "PortBindings": bindings,
            "RestartPolicy": { "Name": self.restart.name(), "MaximumRetryCount": self.restart.retries() },
        });
        if let Some(network) = &self.network {
            host["NetworkMode"] = json!(network);
        }
        let mut body = json!({
            "Image": self.image,
            "Env": self.env_list(),
            "Labels": self.label_map(None),
            "ExposedPorts": exposed,
            "HostConfig": host,
        });
        if !self.cmd.is_empty() {
            body["Cmd"] = json!(self.cmd);
        }
        if let Some(user) = &self.user {
            body["User"] = json!(user);
        }
        if let Some(health) = &self.health {
            body["Healthcheck"] = health.json();
        }
        body
    }

    /// The body of Podman's `POST /libpod/containers/create` (its SpecGenerator), with each secret file as the Podman
    /// secret `sources[i]` mounted at `/run/secrets/<name>`, and the names of those secrets in [`SECRETS_LABEL`].
    pub fn podman_body(&self, sources: &[String]) -> serde_json::Value {
        let env: serde_json::Map<String, serde_json::Value> = self
            .env
            .iter()
            .map(|(name, value)| (name.clone(), json!(value)))
            .collect();
        let labels = self.label_map((!sources.is_empty()).then(|| (SECRETS_LABEL, sources.join(","))));
        let secrets: Vec<serde_json::Value> = self
            .secrets
            .iter()
            .zip(sources)
            .map(|(secret, source)| {
                // a target that is not a path is the file's name under /run/secrets, in every Podman that has secrets
                json!({
                    "source": source,
                    "target": secret.name,
                    "uid": secret.uid,
                    "gid": secret.gid,
                    "mode": secret.mode,
                })
            })
            .collect();
        let volumes: Vec<serde_json::Value> = self
            .mounts
            .iter()
            .filter(|mount| mount.kind == MountKind::Volume)
            .map(|mount| {
                let options: Vec<&str> = if mount.read_only { vec!["ro"] } else { Vec::new() };
                json!({ "Name": mount.source, "Dest": mount.target, "Options": options })
            })
            .collect();
        let binds: Vec<serde_json::Value> = self
            .mounts
            .iter()
            .filter(|mount| mount.kind == MountKind::Bind)
            .map(|mount| {
                let mut options = vec!["rbind"];
                if mount.read_only {
                    options.push("ro");
                }
                json!({
                    "destination": mount.target,
                    "type": "bind",
                    "source": mount.source,
                    "options": options,
                })
            })
            .collect();
        let ports: Vec<serde_json::Value> = self
            .ports
            .iter()
            .map(|port| {
                let mut mapping = json!({
                    "container_port": port.container_port,
                    "protocol": port.protocol,
                });
                if let Some(host_port) = port.host_port {
                    mapping["host_port"] = json!(host_port);
                }
                if let Some(host_ip) = &port.host_ip {
                    mapping["host_ip"] = json!(host_ip);
                }
                mapping
            })
            .collect();
        let mut body = json!({
            "name": self.name,
            "image": self.image,
            "env": env,
            "labels": labels,
            "secrets": secrets,
            "volumes": volumes,
            "mounts": binds,
            "portmappings": ports,
            "restart_policy": self.restart.name(),
        });
        if let RestartPolicy::OnFailure(retries) = self.restart {
            body["restart_tries"] = json!(retries);
        }
        if !self.cmd.is_empty() {
            body["command"] = json!(self.cmd);
        }
        if let Some(user) = &self.user {
            body["user"] = json!(user);
        }
        if let Some(health) = &self.health {
            body["healthconfig"] = health.json();
        }
        if let Some(network) = &self.network {
            body["netns"] = json!({ "nsmode": "bridge" });
            body["Networks"] = json!({ network.clone(): {} });
        }
        body
    }
}
