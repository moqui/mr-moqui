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
//! API, which Podman serves on its socket too, on the shared client [`moqui_lib::docker`]. An `InstanceHost` is reached
//! over a Unix socket (`hostProtocol` `unix`, `hostAddress` the socket's path), HTTPS with a client certificate
//! (`tlsCaCert`, `tlsClientCert`, and the client key from the vault under the host's environment's machine access), or
//! plain HTTP to this machine only (SEC-OUT-08). A job's container, and each of its service containers, has no
//! privilege, no capabilities, no host
//! socket and no host path, a non-root user, an init that reaps orphans, a read-only root with a sized tmpfs at `/tmp`,
//! its working directory on a sized tmpfs (or on the root filesystem the pool's `diskMb` sizes), and the pool's
//! limits. Its names carry this server's id, and what it removes has this server's label
//! (docs/interfaces/Pipelines.md "Runners and drivers").

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::json;

pub use moqui_lib::docker::DockerHost;
use moqui_lib::docker::{Created, Engine, HostAddress, CALL_TIMEOUT, NETWORK_KEYWORDS};
use moqui_lib::error::Error as LibError;
use moqui_lib::http::Method;
use moqui_lib::object::{to_plain_string, Value};

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::rust_service;

use super::super::field;
use super::network_name;

type Parameters = BTreeMap<String, Value>;
type Out = Result<BTreeMap<String, Value>, LibError>;

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    rust_service!(map, start_run_docker)?;
    rust_service!(map, inspect_run_docker)?;
    rust_service!(map, remove_run_docker)?;
    rust_service!(map, cleanup_runs_docker)?;
    Ok(())
}

/// The values of an `InstanceHost` row that say where its host is and how it is reached. The client key of an https
/// host with a client certificate is a vault item (`InstanceHost:<id>:tlsClientKey`, plans/Vault-Instances.md §4.1). A
/// CI run is unattended, so it is read under the machine access of the environment the host names
/// (`InstanceHost.environmentId`): a lease approved by "machine" for this one call, ended when the client is made
/// (`moqui_core::vault_hooks::machine_run`). A host with a client key and no environment, or an environment with no
/// machine access to the key, is not reached, and the refusal says what to give.
fn host_address(host: &EntityValue) -> Result<HostAddress, Error> {
    let protocol = field(host, "hostProtocol");
    let tls_client_cert = field(host, "tlsClientCert");
    let https = protocol
        .as_deref()
        .is_some_and(|protocol| protocol.eq_ignore_ascii_case("https"));
    let tls_client_key = if https && tls_client_cert.is_some() {
        let host_id = field(host, "instanceHostId").unwrap_or_default();
        let binding = format!("InstanceHost:{host_id}:tlsClientKey");
        let Some(environment) = field(host, "environmentId") else {
            return Err(Error::new(format!(
                "the host's client key is the vault item {binding}, which a CI run reads under machine access: name \
                 the host's environment (environmentId) and give it machine access to {binding} (reasonCode \
                 machineAccessMissing)"
            )));
        };
        let address = field(host, "hostAddress").unwrap_or_default();
        let operation = format!("CI run on InstanceHost {host_id} at {address}");
        let key =
            moqui_core::vault_hooks::machine_run(&environment, std::slice::from_ref(&binding), &operation, || {
                moqui_core::vault_hooks::leased(&binding, Some("privateKey"))
            })
            .map_err(|err| Error::new(format!("the host's client key {binding}: {}", err.message())))?;
        Some(to_plain_string(&key))
    } else {
        None
    };
    Ok(HostAddress {
        protocol,
        address: field(host, "hostAddress"),
        port: field(host, "adminPort"),
        tls_ca_cert: field(host, "tlsCaCert"),
        tls_client_cert,
        tls_client_key,
    })
}

/// The client for an `InstanceHost` row.
///
/// # Errors
///
/// When the row names no address, plain http to a host that is not this machine (SEC-OUT-08), or a client key the
/// thread's lease does not hold.
pub fn host_of(host: &EntityValue) -> Result<DockerHost, Error> {
    Ok(DockerHost::connect(&host_address(host)?, None)?)
}

/// Run `command` in a container of `image` on `host` to its end, kept in as a job's container is, and remove it: the
/// container's exit code. For the opt-in test on a real Docker or Podman socket.
///
/// # Errors
///
/// When the host refuses a call, or the container has not ended after `wait`.
pub fn run_once(
    host: &DockerHost,
    name: &str,
    image: &str,
    command: &[&str],
    wait: Duration,
) -> Result<Option<i64>, Error> {
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
    let id = create(
        host,
        name,
        create_body(&parameters, &Hardening::default(), None),
        true,
        &Hardening::default(),
    )?;
    let outcome = (|| -> Result<Option<i64>, Error> {
        host.start(&id)?;
        let started = std::time::Instant::now();
        loop {
            let response = host.send(Method::Get, &format!("/containers/{id}/json"), None, CALL_TIMEOUT)?;
            if response.status() >= 300 {
                return Err(DockerHost::problem(&format!("inspecting container {id}"), &response).into());
            }
            let body: serde_json::Value = serde_json::from_str(response.body()).unwrap_or(serde_json::Value::Null);
            if body["State"]["Running"].as_bool() != Some(true) && body["State"]["Status"].as_str() != Some("created") {
                return Ok(body["State"]["ExitCode"].as_i64());
            }
            if started.elapsed() > wait {
                return Err(Error::new(format!("container {id} has not ended after {wait:?}")));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    })();
    let removed = host.remove(&id);
    let exit = outcome?;
    removed?;
    Ok(exit)
}

/// Create a job's container of `body` (made with `hardening`), with `pull` pulling its image when the host does not
/// have it, and a sized tmpfs over each `VOLUME` its image declares ([`cover_volumes`]): the image is inspected first
/// ([`DockerHost::image_volumes`]), so a path that cannot be covered is refused before anything is made. Its id.
fn create(
    host: &DockerHost,
    name: &str,
    mut body: serde_json::Value,
    pull: bool,
    hardening: &Hardening,
) -> Result<String, Error> {
    let image = body["Image"].as_str().unwrap_or_default().to_string();
    let volumes = match host.image_volumes(&image)? {
        Some(volumes) => volumes,
        None if !pull => return Err(not_on_host(&image)),
        None => {
            host.pull(&image)?;
            host.image_volumes(&image)?.ok_or_else(|| {
                Error::new(format!(
                    "creating container {name}: the image {image} is not on the host after its pull"
                ))
            })?
        }
    };
    cover_volumes(&mut body, &volumes, hardening).map_err(|why| Error::new(format!("container {name}: {why}")))?;
    if host.engine()? == Engine::Podman {
        podman_tmpfs(&mut body);
    }
    if pull {
        return Ok(host.create(name, &body, true)?);
    }
    match host.create_container(name, &body)? {
        Created::Id(id) => Ok(id),
        Created::NoSuchImage => Err(not_on_host(&image)),
    }
}

/// A body's tmpfs options as Podman takes them. Podman refuses `uid=` and `gid=` on a tmpfs ("unknown mount option"),
/// which Docker passes to the kernel; its `U` makes the mount the container user's, the same uid and gid as the body's
/// `User` ([`Hardening::tmpfs`] names them only for a numeric user). A mount with neither is left as it is.
fn podman_tmpfs(body: &mut serde_json::Value) {
    let Some(mounts) = body["HostConfig"]["Tmpfs"].as_object_mut() else {
        return;
    };
    for options in mounts.values_mut() {
        let Some(text) = options.as_str() else {
            continue;
        };
        let kept: Vec<&str> = text
            .split(',')
            .filter(|option| !option.starts_with("uid=") && !option.starts_with("gid="))
            .collect();
        if kept.len() != text.split(',').count() {
            *options = json!(format!("{},U", kept.join(",")));
        }
    }
}

/// With `container-user="image"`, a container made from an image whose user is not a numeric uid other than 0 (its
/// `Config.User` empty, root, or a name) is refused before it starts: a job never runs as root, in any form, and a
/// name is whatever the image's own `/etc/passwd` says, which may be uid 0 under another name
/// ([`is_numeric_user`]).
///
/// Whatever the user, a container with a volume mount is refused too: each `VOLUME` of its image was covered by a
/// tmpfs ([`cover_volumes`]), so a volume the engine made anyway would be writable on the host's disk, sized by
/// neither `StorageOpt` nor a tmpfs, there even with a read-only root (SEC-GIT-93). The checks read the made
/// container once, before it starts.
fn check_made(host: &DockerHost, id: &str, name: &str, image_user: bool) -> Result<(), Error> {
    let response = host.send(Method::Get, &format!("/containers/{id}/json"), None, CALL_TIMEOUT)?;
    if response.status() >= 300 {
        return Err(DockerHost::problem(&format!("inspecting container {name}"), &response).into());
    }
    let made = serde_json::from_str::<serde_json::Value>(response.body()).unwrap_or(serde_json::Value::Null);
    let volumes: Vec<String> = made["Mounts"]
        .as_array()
        .map(|mounts| {
            mounts
                .iter()
                .filter(|mount| mount["Type"] == "volume")
                .filter_map(|mount| mount["Destination"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if !volumes.is_empty() {
        return Err(Error::new(format!(
            "container {name} has a volume on the host's disk ({}), outside the job's limits, which its tmpfs did not \
             cover",
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

/// Whether container `id` carries this server's label (`moqui.ci.server` of [`super::server_id`]): what this server
/// made, and so may remove. One that is not there does not.
fn ours(host: &DockerHost, id: &str) -> Result<bool, Error> {
    let response = host.send(Method::Get, &format!("/containers/{id}/json"), None, CALL_TIMEOUT)?;
    if response.status() == 404 {
        return Ok(false);
    }
    if response.status() >= 300 {
        return Err(DockerHost::problem(&format!("inspecting container {id}"), &response).into());
    }
    let body: serde_json::Value = serde_json::from_str(response.body()).unwrap_or(serde_json::Value::Null);
    Ok(body["Config"]["Labels"][super::LABEL_SERVER].as_str() == Some(super::server_id().as_str()))
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
/// digits, dot, dash and underscore) that is no engine keyword ([`NETWORK_KEYWORDS`], which a container that stays is
/// held to as well: [`moqui_lib::docker::valid_network`]). A list of what is refused would miss a mode: on Podman
/// `ns:<path>` joins the network namespace at a path, the host's included (`ns:/proc/1/ns/net`), and `slirp4netns`
/// and `pasta` (with or without options) and `private` are modes of their own; `host` and `container:...` are the
/// host's network and another container's (SEC-GIT-93).
pub fn valid_egress(rule: &str) -> bool {
    let rule = rule.trim();
    if NETWORK_KEYWORDS
        .iter()
        .any(|keyword| rule.eq_ignore_ascii_case(keyword))
    {
        return false;
    }
    rule == "bridge" || rule == "none" || crate::ci::file::valid_name(rule)
}

fn not_on_host(image: &str) -> Error {
    Error::new(format!(
        "the image {image} is not on the host, and the pool takes untrusted runs, whose images are not pulled on \
         demand: pull it on the host first"
    ))
}

/// Put a tmpfs at each `VOLUME` path of a container's image, sized `container-tmp-mb` and owned by the container's
/// user as `/tmp` is, so the engine makes no anonymous volume there on the host's disk (SEC-GIT-93): a database
/// service (postgres, mysql, redis) writes its data in memory the pool's memory limit bounds. A path a tmpfs already
/// covers (`/tmp`, the working directory) is left. A path that is not absolute cannot be covered, and is refused, and so
/// is any `VOLUME` on a pool with no `memoryMb`, where nothing would bound the tmpfs.
///
/// # Errors
///
/// The refusal of a path that cannot be covered.
pub fn cover_volumes(body: &mut serde_json::Value, volumes: &[String], hardening: &Hardening) -> Result<(), String> {
    // a tmpfs is memory: only the pool's memory limit (memoryMb) bounds what the volumes hold
    if !volumes.is_empty() && body["HostConfig"]["Memory"].as_i64().is_none_or(|memory| memory <= 0) {
        return Err(format!(
            "its image declares a VOLUME ({}), which runs on a tmpfs that only the pool's memoryMb bounds, and the pool \
             sets none: give the pool a memoryMb",
            volumes.join(", ")
        ));
    }
    for volume in volumes {
        let path = volume.trim_end_matches('/');
        if !path.starts_with('/') || path.contains(',') || path.contains(':') {
            return Err(format!(
                "its image declares a VOLUME {volume} that cannot be covered by a tmpfs, and the engine would make it on \
                 the host's disk outside the job's limits"
            ));
        }
        let host = &mut body["HostConfig"];
        if !host["Tmpfs"].is_object() {
            host["Tmpfs"] = json!({});
        }
        let tmpfs = &mut host["Tmpfs"];
        if tmpfs.get(path).is_none() {
            tmpfs[path] = json!(hardening.tmpfs(hardening.tmp_mb));
        }
    }
    Ok(())
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
    match super::host_row(facade, &host_id).and_then(|row| host_of(&row)) {
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
                let id = create(
                    &host,
                    &format!("{name}-{service_name}"),
                    service_body(service, &labels, network, parameters, &hardening),
                    pull,
                    &hardening,
                )?;
                made.push(id.clone());
                check_made(&host, &id, &format!("{name}-{service_name}"), hardening.user.is_none())?;
                host.start(&id)?;
            }
        }
        let id = create(
            &host,
            &name,
            create_body(parameters, &hardening, network.as_deref()),
            pull,
            &hardening,
        )?;
        made.push(id.clone());
        check_made(&host, &id, &name, hardening.user.is_none())?;
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
        match ours(&host, &id) {
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

#[cfg(test)]
#[path = "docker_tests.rs"]
mod tests;
