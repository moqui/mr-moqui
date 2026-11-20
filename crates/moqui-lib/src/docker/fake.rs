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

//! A fake container engine for tests (plans/Vault-Instances.md §6 M1, §7): a small HTTP server on a loopback port that
//! answers the calls of [`super::DockerHost`] as Docker or as Podman would, keeps the containers, volumes, networks and
//! Podman secrets it was asked to make, and records every request, so a test asserts what each call carried: that no
//! secret is in a container's environment, labels or command ([`FakeEngine::secret_outside_files`]), and which secret
//! files were put ([`FakeEngine::files`], [`FakeEngine::podman_secrets`]).
//!
//! What it does on purpose: an image named `missing/...` is not on the host until it is pulled; one named `broken` is
//! refused at create; a container on the network `absent-net` is refused with a 404 that names no image; on Docker a
//! container of the image `read-only-root` refuses an archive, as Docker refuses one into a read-only root; an image
//! named `with-volume...` declares the `VOLUME` `/data`.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{DockerHost, HostAddress};
use crate::sync::lock;

/// One request the fake engine answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeRequest {
    pub method: String,
    /// The path with its query, as sent.
    pub target: String,
    pub body: Vec<u8>,
}

impl FakeRequest {
    /// The path without its query.
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or_default()
    }

    /// The body as JSON, `Null` when it is not.
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

/// An entry of an archive put into a container.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeFile {
    pub path: String,
    pub directory: bool,
    pub mode: u32,
    pub uid: u64,
    pub gid: u64,
    pub bytes: Vec<u8>,
}

/// A container the fake engine holds.
#[derive(Clone, Debug, PartialEq)]
pub struct FakeContainer {
    pub id: String,
    pub name: String,
    pub image: String,
    /// The body it was created with (Docker's or Podman's SpecGenerator).
    pub body: serde_json::Value,
    /// `created`, `running` or `exited`.
    pub status: String,
    /// The archives put into it, in order: each entry, with the status it had then.
    pub files: Vec<(String, FakeFile)>,
    pub labels: BTreeMap<String, String>,
}

#[derive(Default)]
struct State {
    podman: bool,
    requests: Vec<FakeRequest>,
    containers: BTreeMap<String, FakeContainer>,
    pulled: BTreeSet<String>,
    volumes: BTreeMap<String, serde_json::Value>,
    networks: BTreeSet<String>,
    secrets: BTreeMap<String, Vec<u8>>,
    removed: Vec<(String, bool)>,
    next: u64,
}

/// A fake Docker or Podman engine on a loopback port. Its server thread ends with the test process.
pub struct FakeEngine {
    port: u16,
    state: Arc<Mutex<State>>,
}

impl FakeEngine {
    /// A fake Docker engine.
    pub fn docker() -> Self {
        Self::start(false)
    }

    /// A fake Podman engine: its `/version` names "Podman Engine", and it serves the libpod calls.
    pub fn podman() -> Self {
        Self::start(true)
    }

    fn start(podman: bool) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port for the fake engine");
        let port = listener.local_addr().expect("the fake engine's port").port();
        let state = Arc::new(Mutex::new(State {
            podman,
            ..State::default()
        }));
        let shared = Arc::clone(&state);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                serve(stream, &shared);
            }
        });
        Self { port, state }
    }

    /// The host row's values that reach this engine: plain http to loopback.
    pub fn address(&self) -> HostAddress {
        HostAddress {
            protocol: Some("http".to_string()),
            address: Some("127.0.0.1".to_string()),
            port: Some(self.port.to_string()),
            ..HostAddress::default()
        }
    }

    /// A client of this engine.
    pub fn host(&self) -> DockerHost {
        DockerHost::connect(&self.address(), None).expect("the fake engine's address is loopback http")
    }

    /// Every request answered, in order.
    pub fn requests(&self) -> Vec<FakeRequest> {
        lock(&self.state).requests.clone()
    }

    /// Each request as `METHOD target`, in order.
    pub fn calls(&self) -> Vec<String> {
        lock(&self.state)
            .requests
            .iter()
            .map(|request| format!("{} {}", request.method, request.target))
            .collect()
    }

    /// Forget the requests answered so far.
    pub fn clear(&self) {
        lock(&self.state).requests.clear();
    }

    /// The containers held, by id.
    pub fn containers(&self) -> BTreeMap<String, FakeContainer> {
        lock(&self.state).containers.clone()
    }

    /// The files put into container `id` (by archive), in order, with the container's status when each was put.
    pub fn files(&self, id: &str) -> Vec<(String, FakeFile)> {
        lock(&self.state)
            .containers
            .get(id)
            .map(|container| container.files.clone())
            .unwrap_or_default()
    }

    /// The Podman secrets held, by name, with their values.
    pub fn podman_secrets(&self) -> BTreeMap<String, Vec<u8>> {
        lock(&self.state).secrets.clone()
    }

    /// The volumes held, by name.
    pub fn volumes(&self) -> BTreeMap<String, serde_json::Value> {
        lock(&self.state).volumes.clone()
    }

    /// The networks held.
    pub fn networks(&self) -> BTreeSet<String> {
        lock(&self.state).networks.clone()
    }

    /// The containers removed, each with whether its volumes were asked to go too.
    pub fn removed(&self) -> Vec<(String, bool)> {
        lock(&self.state).removed.clone()
    }

    /// The requests that carried `secret` anywhere but where a secret file goes (an archive's body, a Podman secret's
    /// body): a container's create body, whose environment, labels and command `docker inspect` shows, or any other.
    pub fn secret_outside_files(&self, secret: &[u8]) -> Vec<String> {
        lock(&self.state)
            .requests
            .iter()
            .filter(|request| {
                let files = (request.method == "PUT" && request.path().ends_with("/archive"))
                    || request.path().ends_with("/libpod/secrets/create");
                !files
                    && (request.body.windows(secret.len().max(1)).any(|window| window == secret)
                        || request
                            .target
                            .as_bytes()
                            .windows(secret.len().max(1))
                            .any(|window| window == secret))
            })
            .map(|request| format!("{} {}", request.method, request.target))
            .collect()
    }
}

/// Read one request from `stream` and answer it, then close.
fn serve(stream: impl Read + Write, state: &Mutex<State>) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let (method, target) = (
        parts.next().unwrap_or_default().to_string(),
        parts.next().unwrap_or_default().to_string(),
    );
    let mut length = 0usize;
    let mut chunked = false;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap_or(0);
            } else if name.eq_ignore_ascii_case("transfer-encoding") && value.to_ascii_lowercase().contains("chunked") {
                chunked = true;
            }
        }
    }
    let body = if chunked {
        read_chunked(&mut reader)
    } else {
        let mut body = vec![0u8; length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        body
    };
    let request = FakeRequest { method, target, body };
    let (status, content_type, text) = {
        let mut state = lock(state);
        let answer = answer(&mut state, &request);
        state.requests.push(request);
        answer
    };
    let mut stream = reader.into_inner();
    let _ = write!(
        stream,
        "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        text.len()
    );
    let _ = stream.write_all(&text);
    let _ = stream.flush();
}

fn read_chunked(reader: &mut impl BufRead) -> Vec<u8> {
    let mut body = Vec::new();
    loop {
        let mut size = String::new();
        if reader.read_line(&mut size).unwrap_or(0) == 0 {
            return body;
        }
        let size = usize::from_str_radix(size.trim().split(';').next().unwrap_or_default(), 16).unwrap_or(0);
        if size == 0 {
            let mut end = String::new();
            let _ = reader.read_line(&mut end);
            return body;
        }
        let mut chunk = vec![0u8; size];
        if reader.read_exact(&mut chunk).is_err() {
            return body;
        }
        body.extend_from_slice(&chunk);
        let mut crlf = String::new();
        let _ = reader.read_line(&mut crlf);
    }
}

type Answer = (u16, &'static str, Vec<u8>);

fn json_answer(status: u16, body: &serde_json::Value) -> Answer {
    (status, "application/json", body.to_string().into_bytes())
}

fn message(status: u16, text: &str) -> Answer {
    json_answer(status, &json!({ "message": text }))
}

fn query<'a>(target: &'a str, name: &str) -> Option<&'a str> {
    target
        .split_once('?')
        .map_or("", |(_, query)| query)
        .split('&')
        .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
}

fn decode(text: &str) -> String {
    let mut out = Vec::new();
    let mut bytes = text.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let hex: String = bytes.by_ref().take(2).map(char::from).collect();
            out.push(u8::from_str_radix(&hex, 16).unwrap_or(b'?'));
        } else {
            out.push(byte);
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn strings(value: &serde_json::Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// The archive `bytes` as entries.
fn entries(bytes: &[u8]) -> Result<Vec<FakeFile>, String> {
    let mut archive = tar::Archive::new(bytes);
    let mut files = Vec::new();
    for entry in archive.entries().map_err(|err| err.to_string())? {
        let mut entry = entry.map_err(|err| err.to_string())?;
        let header = entry.header().clone();
        let mut content = Vec::new();
        entry.read_to_end(&mut content).map_err(|err| err.to_string())?;
        files.push(FakeFile {
            path: entry.path().map_err(|err| err.to_string())?.display().to_string(),
            directory: header.entry_type().is_dir(),
            mode: header.mode().map_err(|err| err.to_string())?,
            uid: header.uid().map_err(|err| err.to_string())?,
            gid: header.gid().map_err(|err| err.to_string())?,
            bytes: content,
        });
    }
    Ok(files)
}

/// The container `reference` names, by id or name.
fn find<'a>(state: &'a mut State, reference: &str) -> Option<&'a mut FakeContainer> {
    let id = state
        .containers
        .values()
        .find(|container| container.id == reference || container.name == reference)
        .map(|container| container.id.clone())?;
    state.containers.get_mut(&id)
}

fn create(state: &mut State, name: &str, image: &str, body: &serde_json::Value, labels: &serde_json::Value) -> Answer {
    if image == "broken" {
        return message(500, "the daemon is unwell");
    }
    if body["HostConfig"]["NetworkMode"] == "absent-net" {
        return message(404, "network absent-net not found");
    }
    if image.starts_with("missing/") && !state.pulled.contains(image) {
        return message(404, &format!("No such image: {image}"));
    }
    if state.containers.values().any(|container| container.name == name) {
        return message(
            409,
            &format!("Conflict. The container name \"/{name}\" is already in use"),
        );
    }
    state.next += 1;
    let id = format!("{:064x}", state.next);
    state.containers.insert(
        id.clone(),
        FakeContainer {
            id: id.clone(),
            name: name.to_string(),
            image: image.to_string(),
            body: body.clone(),
            status: "created".to_string(),
            files: Vec::new(),
            labels: strings(labels),
        },
    );
    json_answer(201, &json!({ "Id": id, "Warnings": [] }))
}

fn answer(state: &mut State, request: &FakeRequest) -> Answer {
    let path = request.path().to_string();
    let method = request.method.as_str();
    let body = request.json();
    if let Some(rest) = path.strip_prefix(super::podman::LIBPOD) {
        if !state.podman {
            return message(404, "page not found");
        }
        return libpod(state, method, rest, request, &body);
    }
    let Some(rest) = path.strip_prefix(super::API) else {
        return message(404, "page not found");
    };
    let segments: Vec<&str> = rest.trim_start_matches('/').split('/').collect();
    match (method, segments.as_slice()) {
        ("GET", ["version"]) if state.podman => json_answer(
            200,
            &json!({"Version": "5.2.0", "ApiVersion": "1.41", "Components": [{"Name": "Podman Engine", "Version": "5.2.0"}]}),
        ),
        ("GET", ["version"]) => json_answer(
            200,
            &json!({"Version": "27.3.1", "ApiVersion": "1.47", "Components": [{"Name": "Engine", "Version": "27.3.1"}]}),
        ),
        ("POST", ["containers", "create"]) => {
            let name = decode(query(&request.target, "name").unwrap_or_default());
            let image = body["Image"].as_str().unwrap_or_default().to_string();
            create(state, &name, &image, &body, &body["Labels"])
        }
        ("POST", ["images", "create"]) => {
            let name = decode(query(&request.target, "fromImage").unwrap_or_default());
            let tag = decode(query(&request.target, "tag").unwrap_or("latest"));
            state.pulled.insert(format!("{name}:{tag}"));
            state.pulled.insert(name);
            (
                200,
                "application/json",
                b"{\"status\":\"Pulling\"}\n{\"status\":\"Done\"}\n".to_vec(),
            )
        }
        ("GET", ["images", id, "json"]) => {
            let image = decode(id);
            if image.starts_with("missing/") && !state.pulled.contains(&image) {
                return message(404, &format!("No such image: {image}"));
            }
            // an image named `with-volume...` declares a VOLUME, as a database's official image does
            let volumes = if image.starts_with("with-volume") {
                json!({"/data": {}})
            } else {
                json!(null)
            };
            json_answer(
                200,
                &json!({
                    "Id": format!("sha256:{id}"),
                    "RepoDigests": [format!("example.com/app@sha256:{id}")],
                    "Config": {"Volumes": volumes},
                }),
            )
        }
        ("PUT", ["containers", reference, "archive"]) => {
            let Some(container) = find(state, reference) else {
                return message(404, "No such container");
            };
            if container.image == "read-only-root" {
                return message(500, "container rootfs is marked read-only");
            }
            match entries(&request.body) {
                Ok(files) => {
                    let status = container.status.clone();
                    container
                        .files
                        .extend(files.into_iter().map(|file| (status.clone(), file)));
                    (200, "application/json", Vec::new())
                }
                Err(err) => message(400, &format!("not a tar: {err}")),
            }
        }
        ("POST", ["containers", reference, action]) => {
            let Some(container) = find(state, reference) else {
                return message(404, "No such container");
            };
            match *action {
                "start" if container.status == "running" => (304, "application/json", Vec::new()),
                "start" | "restart" => {
                    container.status = "running".to_string();
                    (204, "application/json", Vec::new())
                }
                "stop" if container.status != "running" => (304, "application/json", Vec::new()),
                "stop" => {
                    container.status = "exited".to_string();
                    (204, "application/json", Vec::new())
                }
                _ => message(400, "the fake engine does not know this"),
            }
        }
        ("GET", ["containers", reference, "json"]) => {
            let Some(container) = find(state, reference) else {
                return message(404, "No such container");
            };
            let checked = !container.body["Healthcheck"].is_null() || !container.body["healthconfig"].is_null();
            let mut answer = json!({
                "Id": container.id,
                "Name": format!("/{}", container.name),
                "State": {
                    "Status": container.status,
                    "Running": container.status == "running",
                    "ExitCode": if container.status == "exited" { 143 } else { 0 },
                },
                "Image": format!("sha256:{}", "ab".repeat(32)),
                "Config": { "Labels": container.labels, "Env": container.body["Env"] },
            });
            if checked {
                answer["State"]["Health"] = json!({ "Status": "healthy" });
            }
            json_answer(200, &answer)
        }
        ("GET", ["containers", reference, "logs"]) => {
            let Some(container) = find(state, reference) else {
                return message(404, "No such container");
            };
            let mut frames = Vec::new();
            for (stream, text) in [
                (1u8, format!("{} started\n", container.name)),
                (2u8, "ready\n".to_string()),
            ] {
                frames.extend_from_slice(&[stream, 0, 0, 0]);
                frames.extend_from_slice(&u32::try_from(text.len()).unwrap_or(0).to_be_bytes());
                frames.extend_from_slice(text.as_bytes());
            }
            (200, "application/vnd.docker.raw-stream", frames)
        }
        ("DELETE", ["containers", reference]) => {
            let volumes = query(&request.target, "v") == Some("true");
            let Some(id) = find(state, reference).map(|container| container.id.clone()) else {
                return message(404, "No such container");
            };
            state.containers.remove(&id);
            state.removed.push((id, volumes));
            (204, "application/json", Vec::new())
        }
        ("POST", ["volumes", "create"]) => {
            let name = body["Name"].as_str().unwrap_or_default().to_string();
            let volume = state.volumes.entry(name.clone()).or_insert_with(|| {
                json!({"Name": name, "Driver": "local", "Mountpoint": format!("/var/lib/docker/volumes/{name}/_data"), "Labels": body["Labels"]})
            });
            json_answer(201, volume)
        }
        ("GET", ["volumes", name]) => match state.volumes.get(*name) {
            Some(volume) => json_answer(200, volume),
            None => message(404, "no such volume"),
        },
        ("DELETE", ["volumes", name]) => match state.volumes.remove(*name) {
            Some(_) => (204, "application/json", Vec::new()),
            None => message(404, "no such volume"),
        },
        ("POST", ["networks", "create"]) => {
            let name = body["Name"].as_str().unwrap_or_default().to_string();
            if !state.networks.insert(name.clone()) {
                return message(409, &format!("network with name {name} already exists"));
            }
            json_answer(201, &json!({ "Id": name }))
        }
        ("POST", ["networks", name, "connect"]) if state.networks.contains(*name) => {
            (200, "application/json", Vec::new())
        }
        ("POST", ["networks", _, "connect"]) => message(404, "no such network"),
        ("DELETE", ["networks", name]) => {
            if state.networks.remove(*name) {
                (204, "application/json", Vec::new())
            } else {
                message(404, "no such network")
            }
        }
        _ => message(404, "the fake engine does not know this"),
    }
}

/// Podman's own calls: secrets, and a container made from a SpecGenerator.
fn libpod(state: &mut State, method: &str, rest: &str, request: &FakeRequest, body: &serde_json::Value) -> Answer {
    let segments: Vec<&str> = rest.trim_start_matches('/').split('/').collect();
    match (method, segments.as_slice()) {
        ("POST", ["secrets", "create"]) => {
            let name = decode(query(&request.target, "name").unwrap_or_default());
            if state.secrets.contains_key(&name) {
                return message(409, &format!("{name}: secret name in use"));
            }
            state.secrets.insert(name.clone(), request.body.clone());
            json_answer(200, &json!({ "ID": format!("id-{name}") }))
        }
        ("DELETE", ["secrets", name]) => {
            if state.secrets.remove(&decode(name)).is_some() {
                (204, "application/json", Vec::new())
            } else {
                message(404, "no such secret")
            }
        }
        ("POST", ["containers", "create"]) => {
            let sources: Vec<String> = body["secrets"]
                .as_array()
                .map(|secrets| {
                    secrets
                        .iter()
                        .filter_map(|secret| secret["source"].as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            if let Some(unknown) = sources.iter().find(|source| !state.secrets.contains_key(*source)) {
                return message(404, &format!("{unknown}: no such secret"));
            }
            let name = body["name"].as_str().unwrap_or_default().to_string();
            let image = body["image"].as_str().unwrap_or_default().to_string();
            if image.starts_with("missing/") && !state.pulled.contains(&image) {
                return message(404, &format!("{image}: image not known"));
            }
            create(state, &name, &image, body, &body["labels"])
        }
        _ => message(404, "the fake engine does not know this"),
    }
}
