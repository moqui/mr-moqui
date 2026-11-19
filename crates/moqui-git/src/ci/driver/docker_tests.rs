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

//! The container driver's requests, against a small Docker Engine API in this test: over TCP and over a Unix socket.
//! A job on a real Docker or Podman socket is the opt-in `ci_docker` test (docs/interfaces/Pipelines.md).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::{Arc, Mutex};

use serde_json::json;

use moqui_lib::object::{to_plain_string, Value, ValueMap};

use super::{
    call_docker_api, cleanup_runs_docker, create_body, inspect_run_docker, remove_run_docker, start_run_docker,
    Hardening,
};
use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::{EntityFacade, EntityValue};

/// A run service of the driver.
type RunService =
    fn(&BTreeMap<String, Value>, &EntityFacade) -> Result<BTreeMap<String, Value>, moqui_lib::error::Error>;

/// What the fake API was asked: the method, the path with its query, and the JSON body.
type Seen = Arc<Mutex<Vec<(String, String, serde_json::Value)>>>;

fn answer(method: &str, target: &str, body: &serde_json::Value, seen: &Seen) -> (u16, String) {
    let path = target.split('?').next().unwrap_or_default();
    let created = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|(method, target, _)| method == "POST" && target.starts_with("/v1.40/containers/create"))
        .count();
    match (method, path) {
        ("POST", "/v1.40/containers/create") => {
            // an image the host does not have: 404 until it was pulled
            let pulled = seen
                .lock()
                .unwrap()
                .iter()
                .any(|(_, target, _)| target.starts_with("/v1.40/images/create"));
            if body["Image"] == "registry.example.com/ci/needs-pull:2" && !pulled {
                (
                    404,
                    json!({"message": "No such image: registry.example.com/ci/needs-pull:2"}).to_string(),
                )
            } else if body["Image"] == "broken" {
                (500, json!({"message": "the daemon is unwell"}).to_string())
            } else {
                (201, json!({"Id": format!("c{created}"), "Warnings": []}).to_string())
            }
        }
        ("POST", "/v1.40/images/create") => (200, "{\"status\":\"Pulling\"}\n{\"status\":\"Done\"}\n".to_string()),
        ("POST", "/v1.40/networks/create") if body["Name"] == "moqui-ci-TAKEN-net" => (
            409,
            json!({"message": "network with name moqui-ci-TAKEN-net already exists"}).to_string(),
        ),
        ("POST", "/v1.40/networks/create") => (201, json!({"Id": "n1"}).to_string()),
        ("POST", path) if path.starts_with("/v1.40/networks/") && path.ends_with("/connect") => (200, String::new()),
        ("GET", "/v1.40/containers/json") => (
            200,
            json!([
                {"Id": "old1", "Created": 1000, "Labels": {"moqui.ci.job": "J-ENDED", "moqui.ci.server": "this"}},
                {"Id": "live", "Created": 1000, "Labels": {"moqui.ci.job": "J-RUNNING", "moqui.ci.server": "this"}},
                {"Id": "old2", "Created": 1000, "Labels": {"moqui.ci.job": "J-LOST", "moqui.ci.server": "this"}},
                {"Id": "fresh", "Created": 9000, "Labels": {"moqui.ci.job": "J-NEW", "moqui.ci.server": "this"}},
            ])
            .to_string(),
        ),
        ("GET", "/v1.40/containers/a1/json") => (
            200,
            json!({"State": {"Status": "running", "Running": true, "ExitCode": 0}}).to_string(),
        ),
        ("GET", "/v1.40/containers/dead1/json") => (
            200,
            json!({"State": {"Status": "exited", "Running": false, "ExitCode": 137}}).to_string(),
        ),
        // a created container's own user, from its image: one image runs as root
        ("GET", path) if path.starts_with("/v1.40/containers/c") && path.ends_with("/json") => {
            let index: usize = path
                .trim_start_matches("/v1.40/containers/c")
                .trim_end_matches("/json")
                .parse()
                .unwrap_or(usize::MAX);
            let image = seen
                .lock()
                .unwrap()
                .iter()
                .filter(|(method, target, _)| method == "POST" && target.starts_with("/v1.40/containers/create"))
                .nth(index)
                .map(|(_, _, body)| body["Image"].as_str().unwrap_or_default().to_string())
                .unwrap_or_default();
            let user = match image.as_str() {
                "runs-as-root:1" => "",
                "named-user:1" => "app",
                _ => "1000:1000",
            };
            // an image that declares a VOLUME: the engine made an anonymous volume for it
            let volumes = if image == "with-volume:1" {
                json!({"/var/lib/data": {}})
            } else {
                json!(null)
            };
            (
                200,
                json!({"Config": {"User": user, "Volumes": volumes, "Labels": {"moqui.ci.server": "this"}}})
                    .to_string(),
            )
        }
        ("GET", _) => (404, json!({"message": "No such container"}).to_string()),
        ("POST", path) if path.ends_with("/start") => (204, String::new()),
        ("DELETE", path) if path.starts_with("/v1.40/networks/") => {
            (404, json!({"message": "no such network"}).to_string())
        }
        ("DELETE", _) => (204, String::new()),
        _ => (400, json!({"message": "the fake API does not know this"}).to_string()),
    }
}

/// Serve one connection: one request, then close.
fn serve(stream: impl Read + Write, seen: &Seen) {
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
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    let (status, text) = answer(&method, &target, &body, seen);
    seen.lock().unwrap().push((method, target, body));
    let mut stream = reader.into_inner();
    write!(
        stream,
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    )
    .unwrap();
    let _ = stream.flush();
}

struct Api {
    seen: Seen,
    context: ExecutionContext,
    entity: EntityFacade,
}

impl Api {
    /// A context with one instance host `H`, reached as `protocol` says.
    fn new(protocol: &str, address: &str, port: Option<u16>, seen: Seen) -> Self {
        Self::with_conf(protocol, address, port, seen, r#"<git-ci server-id="this"/>"#)
    }

    fn with_conf(protocol: &str, address: &str, port: Option<u16>, seen: Seen, conf: &str) -> Self {
        let context = ExecutionContextFactory::test_context_in(
            &format!("<moqui-conf>{conf}</moqui-conf>"),
            moqui_core::service::ServicePools::idle(),
            std::path::PathBuf::from("."),
        );
        let entity = moqui_core::testing::open();
        context.factory().set_entity(entity.clone());
        {
            let _entered = context.enter();
            let _authz = context.disable_authz();
            let mut host = EntityValue::make(&entity, "moqui.server.instance.InstanceHost").unwrap();
            host.set("instanceHostId", Value::string("H"));
            host.set("hostTypeId", Value::string("docker"));
            host.set("hostProtocol", Value::string(protocol));
            host.set("hostAddress", Value::string(address));
            if let Some(port) = port {
                host.set("adminPort", Value::I64(i64::from(port)));
            }
            host.create(&entity).unwrap();
        }
        Self { seen, context, entity }
    }

    fn tcp() -> Self {
        Self::tcp_with(r#"<git-ci server-id="this"/>"#)
    }

    fn tcp_with(conf: &str) -> Self {
        Self::tcp_at("127.0.0.1", conf)
    }

    /// A fake API on a loopback port, the host row naming it as `address`.
    fn tcp_at(address: &str, conf: &str) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen: Seen = Seen::default();
        let shared = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                serve(stream, &shared);
            }
        });
        Self::with_conf("http", address, Some(port), seen, conf)
    }

    /// Call a run service of the driver. Its result, and the errors it left.
    fn call(&self, service: RunService, parameters: &[(&str, Value)]) -> (BTreeMap<String, Value>, String) {
        let _entered = self.context.enter();
        let _authz = self.context.disable_authz();
        let mut parameters: BTreeMap<String, Value> = parameters
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect();
        parameters.insert("instanceHostId".to_string(), Value::string("H"));
        let out = service(&parameters, &self.entity).unwrap();
        let errors = ExecutionContext::with_current(|ec| {
            let text = ec.message().errors_text().join("\n");
            ec.message().clear();
            text
        })
        .unwrap_or_default();
        (out, errors)
    }

    fn seen(&self) -> Vec<(String, String, serde_json::Value)> {
        self.seen.lock().unwrap().clone()
    }
}

fn map(pairs: &[(&str, &str)]) -> Value {
    let map: ValueMap = pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), Value::string(*value)))
        .collect();
    Value::Map(map)
}

fn start_parameters(image: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("name", Value::string("moqui-ci-J1")),
        ("image", Value::string(image)),
        (
            "command",
            Value::List(vec![Value::string("moqui"), Value::string("ci-run")]),
        ),
        (
            "envMap",
            map(&[
                ("MOQUI_CI_SERVER_URL", "https://moqui.example.com"),
                ("MOQUI_CI_TOKEN", "mqcj_abc"),
            ]),
        ),
        ("labels", map(&[("moqui.ci.job", "J1"), ("moqui.ci.server", "this")])),
        ("cpuLimit", Value::string("1.5")),
        ("memoryMb", Value::I64(2048)),
        ("diskMb", Value::I64(10_000)),
        ("pidsLimit", Value::I64(512)),
        ("networkMode", Value::string("ci-egress")),
        ("pullImages", Value::Bool(true)),
    ]
}

#[test]
fn a_jobs_container_has_limits_and_no_way_out() {
    let parameters: BTreeMap<String, Value> = start_parameters("moqui-build:1")
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect();
    let body = create_body(&parameters, &Hardening::default(), None);
    assert_eq!(body["Image"], "moqui-build:1");
    assert_eq!(body["Cmd"], json!(["moqui", "ci-run"]));
    assert_eq!(body["User"], "1000:1000");
    assert_eq!(body["WorkingDir"], "/workspace");
    // the working directory is no unbounded volume: with diskMb it is on the sized root filesystem
    assert!(body.get("Volumes").is_none());
    assert_eq!(body["Labels"], json!({"moqui.ci.job": "J1", "moqui.ci.server": "this"}));
    let env: Vec<&str> = body["Env"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|value| value.as_str())
        .collect();
    assert!(env.contains(&"MOQUI_CI_TOKEN=mqcj_abc") && env.contains(&"MOQUI_CI_SERVER_URL=https://moqui.example.com"));
    let host = &body["HostConfig"];
    assert_eq!(host["Privileged"], false);
    assert_eq!(host["CapDrop"], json!(["ALL"]));
    assert_eq!(host["SecurityOpt"], json!(["no-new-privileges"]));
    assert_eq!(host["Init"], true, "an init reaps orphans");
    // the root the pool's diskMb sizes holds the working directory, so it is writable
    assert_eq!(host["ReadonlyRootfs"], false);
    assert!(host["Tmpfs"].get("/workspace").is_none());
    // no host path and no host socket is mounted, and no port is published
    assert_eq!(host["Binds"], json!([]));
    assert_eq!(host["PublishAllPorts"], false);
    assert!(host.get("Mounts").is_none() && host.get("PortBindings").is_none());
    assert_eq!(host["NanoCpus"], 1_500_000_000i64);
    assert_eq!(host["Memory"], 2048i64 * 1024 * 1024);
    assert_eq!(host["MemorySwap"], host["Memory"]);
    assert_eq!(host["PidsLimit"], 512);
    assert_eq!(host["StorageOpt"], json!({"size": "10000m"}));
    assert_eq!(host["NetworkMode"], "ci-egress");
    assert!(host["Tmpfs"]["/tmp"]
        .as_str()
        .unwrap()
        .contains("size=1024m,uid=1000,gid=1000"));
    // with no diskMb the root is read only and the working directory a sized tmpfs
    let mut no_disk = parameters.clone();
    no_disk.remove("diskMb");
    let body = create_body(&no_disk, &Hardening::default(), None);
    assert_eq!(body["HostConfig"]["ReadonlyRootfs"], true);
    assert!(body["HostConfig"]["Tmpfs"]["/workspace"]
        .as_str()
        .unwrap()
        .contains("size=4096m,uid=1000,gid=1000"));
    // never the host's network, another container's or a namespace by path (SEC-GIT-93): only bridge, none or a network
    // of the host by its name
    for rule in [
        "host",
        "HOST",
        "container:abc",
        "ns:/proc/1/ns/net",
        "slirp4netns:allow_host_loopback=true",
        "pasta:-T,22",
        "private:x",
    ] {
        let mut escaping = parameters.clone();
        escaping.insert("networkMode".to_string(), Value::string(rule));
        assert_eq!(
            create_body(&escaping, &Hardening::default(), None)["HostConfig"]["NetworkMode"],
            "none"
        );
        assert!(!super::valid_egress(rule), "{rule}");
    }
    for rule in ["bridge", "none", "ci-egress", "podman"] {
        assert!(super::valid_egress(rule), "{rule}");
    }
    // root in any form is no container user
    for user in ["0", "0:0", "00", "0:1000", "root", "root:x", "ROOT"] {
        assert!(super::is_root_user(user), "{user}");
    }
    for user in ["1000:1000", "nobody", "10"] {
        assert!(!super::is_root_user(user), "{user}");
    }
    // container-user="image" takes only a numeric uid other than 0: a name may be uid 0 in the image's passwd
    for user in ["1000", "1000:1000", " 10 ", "65534:0"] {
        assert!(super::is_numeric_user(user), "{user}");
    }
    for user in ["", "0", "0:1000", "00", "root", "app", "nobody", "1000x", ":1000"] {
        assert!(!super::is_numeric_user(user), "{user}");
    }
    // with service containers the job is on its own network; with no limits none are sent; the image's user by conf
    let bare: BTreeMap<String, Value> = [("image".to_string(), Value::string("i"))].into_iter().collect();
    let relaxed = Hardening {
        user: None,
        read_only_root: false,
        ..Hardening::default()
    };
    let body = create_body(&bare, &relaxed, Some("moqui-ci-J1-net"));
    assert_eq!(body["HostConfig"]["NetworkMode"], "moqui-ci-J1-net");
    assert_eq!(body["HostConfig"]["ReadonlyRootfs"], false);
    assert_eq!(body["HostConfig"]["Privileged"], false);
    for limit in ["NanoCpus", "Memory", "PidsLimit", "StorageOpt"] {
        assert!(body["HostConfig"].get(limit).is_none(), "{limit}");
    }
    assert!(body.get("User").is_none() && body.get("Cmd").is_none());
    assert_eq!(
        create_body(&bare, &Hardening::default(), None)["HostConfig"]["NetworkMode"],
        "bridge"
    );
}

#[test]
fn the_run_services_speak_the_docker_api() {
    let api = Api::tcp();
    // start: create by name, then start
    let (out, errors) = api.call(start_run_docker, &start_parameters("moqui-build:1"));
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(to_plain_string(&out["instanceRef"]), "c0");
    let seen = api.seen();
    assert_eq!(
        seen.iter()
            .map(|(method, target, _)| format!("{method} {target}"))
            .collect::<Vec<_>>(),
        [
            "POST /v1.40/containers/create?name=moqui-ci-J1",
            "GET /v1.40/containers/c0/json",
            "POST /v1.40/containers/c0/start"
        ]
    );
    assert_eq!(seen[0].2["HostConfig"]["Privileged"], false);
    assert_eq!(seen[0].2["HostConfig"]["Memory"], 2048i64 * 1024 * 1024);
    assert_eq!(seen[0].2["Labels"]["moqui.ci.job"], "J1");

    // an image the host does not have is pulled, then created again
    api.seen.lock().unwrap().clear();
    let (out, errors) = api.call(
        start_run_docker,
        &start_parameters("registry.example.com/ci/needs-pull:2"),
    );
    assert!(errors.is_empty(), "{errors}");
    assert!(!to_plain_string(&out["instanceRef"]).is_empty());
    let targets: Vec<String> = api
        .seen()
        .iter()
        .map(|(method, target, _)| format!("{method} {target}"))
        .collect();
    assert_eq!(targets[0], "POST /v1.40/containers/create?name=moqui-ci-J1");
    assert_eq!(
        targets[1],
        "POST /v1.40/images/create?fromImage=registry.example.com%2Fci%2Fneeds-pull&tag=2"
    );
    assert_eq!(targets[2], "POST /v1.40/containers/create?name=moqui-ci-J1");
    assert!(targets[4].ends_with("/start"), "{targets:?}");

    // an untrusted pool's run pulls nothing: an image the host does not have is an error
    api.seen.lock().unwrap().clear();
    let mut untrusted = start_parameters("registry.example.com/ci/needs-pull:2");
    untrusted.retain(|(name, _)| *name != "pullImages");
    let (out, errors) = api.call(start_run_docker, &untrusted);
    assert!(out.is_empty());
    assert!(errors.contains("is not on the host"), "{errors}");
    assert!(!api
        .seen()
        .iter()
        .any(|(_, target, _)| target.starts_with("/v1.40/images/create")));

    // service containers: a network of the job's own, each service on it by its name, then the job
    api.seen.lock().unwrap().clear();
    let mut with_service = start_parameters("moqui-build:1");
    let mut service = ValueMap::default();
    service.insert("name".to_string(), Value::string("db"));
    service.insert("image".to_string(), Value::string("postgres:16"));
    service.insert("envMap".to_string(), map(&[("POSTGRES_PASSWORD", "x")]));
    with_service.push(("services", Value::List(vec![Value::Map(service)])));
    let (_, errors) = api.call(start_run_docker, &with_service);
    assert!(errors.is_empty(), "{errors}");
    let seen = api.seen();
    assert_eq!(seen[0].1, "/v1.40/networks/create");
    assert_eq!(seen[0].2["Name"], "moqui-ci-J1-net");
    // the pool's egress is a network of the host: the job's own network reaches nothing outside it
    assert_eq!(seen[0].2["Internal"], true);
    assert_eq!(seen[1].1, "/v1.40/containers/create?name=moqui-ci-J1-db");
    assert_eq!(seen[1].2["Image"], "postgres:16");
    assert_eq!(seen[1].2["Labels"]["moqui.ci.job"], "J1");
    // kept in as the job is
    let service_host = &seen[1].2["HostConfig"];
    assert_eq!(service_host["Privileged"], false);
    assert_eq!(service_host["CapDrop"], json!(["ALL"]));
    assert!(service_host.get("CapAdd").is_none());
    assert_eq!(service_host["Init"], true);
    assert_eq!(seen[1].2["User"], "1000:1000");
    assert_eq!(service_host["NanoCpus"], 1_500_000_000i64);
    assert_eq!(service_host["MemorySwap"], 2048i64 * 1024 * 1024);
    assert_eq!(service_host["StorageOpt"], json!({"size": "10000m"}));
    assert_eq!(service_host["PidsLimit"], 512);
    assert_eq!(
        seen[1].2["NetworkingConfig"]["EndpointsConfig"]["moqui-ci-J1-net"]["Aliases"],
        json!(["db"])
    );
    assert!(seen[2].1.ends_with("/json") && seen[3].1.ends_with("/start"));
    assert_eq!(seen[4].1, "/v1.40/containers/create?name=moqui-ci-J1");
    assert_eq!(seen[4].2["HostConfig"]["NetworkMode"], "moqui-ci-J1-net");
    // and the job on the pool's network too, so its egress is the pool's
    assert_eq!(seen[6].1, "/v1.40/networks/ci-egress/connect");
    assert!(seen[6].2["Container"].as_str().is_some_and(|id| id.starts_with('c')));
    assert!(seen[7].1.ends_with("/start"));
    // a pool on the bridge: the job's network reaches out as the bridge does
    api.seen.lock().unwrap().clear();
    let mut bridged = with_service.clone();
    bridged.retain(|(name, _)| *name != "networkMode");
    let (_, errors) = api.call(start_run_docker, &bridged);
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(api.seen()[0].2["Internal"], false);
    assert!(!api.seen().iter().any(|(_, target, _)| target.ends_with("/connect")));
    // a network of that name there already is another run's: an error, nothing started
    api.seen.lock().unwrap().clear();
    let mut taken = with_service.clone();
    taken.retain(|(name, _)| *name != "name");
    taken.push(("name", Value::string("moqui-ci-TAKEN")));
    let (out, errors) = api.call(start_run_docker, &taken);
    assert!(out.is_empty());
    assert!(errors.contains("409"), "{errors}");

    // a host that refuses: the daemon's words, and nothing started
    api.seen.lock().unwrap().clear();
    let (out, errors) = api.call(start_run_docker, &start_parameters("broken"));
    assert!(out.is_empty());
    assert!(
        errors.contains("creating container moqui-ci-J1: 500 the daemon is unwell"),
        "{errors}"
    );

    // inspect: running, exited with its code, missing
    for (id, state, exit) in [("a1", "running", ""), ("dead1", "exited", "137"), ("ff", "missing", "")] {
        let (out, errors) = api.call(inspect_run_docker, &[("instanceRef", Value::string(id))]);
        assert!(errors.is_empty(), "{errors}");
        assert_eq!(to_plain_string(&out["state"]), state);
        let code = out
            .get("exitCode")
            .filter(|value| !matches!(value, Value::Null))
            .map(to_plain_string);
        assert_eq!(code.unwrap_or_default(), exit, "{id}");
    }

    // remove: forced, with its volumes; the containers of the job's label and its network with it
    api.seen.lock().unwrap().clear();
    let (_, errors) = api.call(
        remove_run_docker,
        &[
            ("instanceRef", Value::string("c0")),
            ("labelName", Value::string("moqui.ci.job")),
            ("labelValue", Value::string("J1")),
            ("filterLabels", map(&[("moqui.ci.server", "this")])),
            ("networkName", Value::string("moqui-ci-J1-net")),
        ],
    );
    assert!(errors.is_empty(), "{errors}");
    let targets: Vec<String> = api
        .seen()
        .iter()
        .map(|(method, target, _)| format!("{method} {target}"))
        .collect();
    // the container named is asked for its labels first: only one this server made is removed
    assert_eq!(targets[0], "GET /v1.40/containers/c0/json", "{targets:?}");
    assert!(
        targets[1].starts_with("GET /v1.40/containers/json?all=true&filters="),
        "{targets:?}"
    );
    assert!(targets[1].contains("moqui.ci.job%3DJ1"), "{targets:?}");
    // only this server's containers of that job run
    assert!(targets[1].contains("moqui.ci.server%3Dthis"), "{targets:?}");
    assert!(
        targets.contains(&"DELETE /v1.40/containers/c0?force=true&v=true".to_string()),
        "{targets:?}"
    );
    assert_eq!(targets.last().unwrap(), "DELETE /v1.40/networks/moqui-ci-J1-net");

    // SEC-GIT-17: an instanceRef that is not a container id goes into no path, and a container without this server's
    // label is not removed
    for bad in ["../images/x", "c0?force=true", "c0/../../info"] {
        api.seen.lock().unwrap().clear();
        let (_, errors) = api.call(inspect_run_docker, &[("instanceRef", Value::string(bad))]);
        assert!(errors.contains("is not a container id"), "{errors}");
        let (_, errors) = api.call(remove_run_docker, &[("instanceRef", Value::string(bad))]);
        assert!(errors.contains("is not a container id"), "{errors}");
        assert!(api.seen().is_empty(), "{:?}", api.seen());
    }
    api.seen.lock().unwrap().clear();
    let (_, errors) = api.call(remove_run_docker, &[("instanceRef", Value::string("dead1"))]);
    assert!(errors.is_empty(), "{errors}");
    assert!(
        !api.seen().iter().any(|(method, _, _)| method == "DELETE"),
        "{:?}",
        api.seen()
    );
    // a listing is held to this server's label whatever filterLabels says
    api.seen.lock().unwrap().clear();
    let (_, errors) = api.call(
        cleanup_runs_docker,
        &[
            ("labelName", Value::string("moqui.ci.job")),
            ("filterLabels", map(&[("moqui.ci.server", "another")])),
        ],
    );
    assert!(errors.is_empty(), "{errors}");
    assert!(api.seen()[0].1.contains("moqui.ci.server%3Dthis"), "{:?}", api.seen());

    // cleanup: this server's containers whose job run is not kept
    api.seen.lock().unwrap().clear();
    let (out, errors) = api.call(
        cleanup_runs_docker,
        &[
            ("labelName", Value::string("moqui.ci.job")),
            ("keepValues", Value::List(vec![Value::string("J-RUNNING")])),
            ("filterLabels", map(&[("moqui.ci.server", "this")])),
            ("createdBefore", Value::I64(5000)),
        ],
    );
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(to_plain_string(&out["removedCount"]), "2");
    let targets: Vec<String> = api
        .seen()
        .iter()
        .map(|(method, target, _)| format!("{method} {target}"))
        .collect();
    assert!(targets[0].contains("moqui.ci.server%3Dthis"), "{targets:?}");
    assert!(targets.contains(&"DELETE /v1.40/containers/old1?force=true&v=true".to_string()));
    assert!(targets.contains(&"DELETE /v1.40/containers/old2?force=true&v=true".to_string()));
    // kept, and made after the list of running jobs was read (a job that just started)
    assert!(
        !targets
            .iter()
            .any(|target| target.contains("/containers/live") || target.contains("/containers/fresh")),
        "{targets:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_host_is_reached_over_a_unix_socket() {
    let path = std::env::temp_dir().join(format!("mq-ci-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    let seen: Seen = Seen::default();
    let shared = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            serve(stream, &shared);
        }
    });
    // as Docker and Podman name their sockets, with or without the scheme
    let api = Api::new("unix", &format!("unix://{}", path.display()), None, seen);
    let (out, errors) = api.call(start_run_docker, &start_parameters("moqui-build:1"));
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(to_plain_string(&out["instanceRef"]), "c0");
    let (out, _) = api.call(inspect_run_docker, &[("instanceRef", Value::string("a1"))]);
    assert_eq!(to_plain_string(&out["state"]), "running");
    assert_eq!(api.seen()[0].1, "/v1.40/containers/create?name=moqui-ci-J1");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_host_row_that_cannot_be_reached_says_why() {
    let api = Api::new("ftp", "docker.example.com", Some(2376), Seen::default());
    let (out, errors) = api.call(start_run_docker, &start_parameters("moqui-build:1"));
    assert!(out.is_empty());
    assert!(
        errors.contains("hostProtocol ftp is not unix, http or https"),
        "{errors}"
    );
    // a TLS host with a client certificate that is not PEM is refused before anything is sent
    let api = Api::new("https", "docker.example.com", Some(2376), Seen::default());
    {
        let _entered = api.context.enter();
        let _authz = api.context.disable_authz();
        api.entity
            .find("moqui.server.instance.InstanceHost")
            .unwrap()
            .eq("instanceHostId", Value::string("H"))
            .update_all([
                ("tlsClientCert".to_string(), Value::string("not a certificate")),
                ("tlsClientKey".to_string(), Value::string("not a key")),
            ])
            .unwrap();
    }
    let (out, errors) = api.call(start_run_docker, &start_parameters("moqui-build:1"));
    assert!(out.is_empty());
    assert!(
        errors.contains("the client certificate and key are not PEM"),
        "{errors}"
    );
}

/// SEC-OUT-08: plain http goes to this machine only. A host row with http to another address is refused before
/// anything is sent, for a CI run and for `call#DockerApi`; `0.0.0.0`, which Linux connects to this machine, is not
/// taken as loopback either.
#[test]
fn plain_http_to_another_host_is_refused_before_anything_is_sent() {
    for address in ["192.0.2.10", "docker.example.com", "0.0.0.0", "::ffff:192.0.2.10"] {
        let started = std::time::Instant::now();
        let api = Api::new("http", address, Some(2375), Seen::default());
        let (out, errors) = api.call(start_run_docker, &start_parameters("moqui-build:1"));
        assert!(out.is_empty(), "{address}: {out:?}");
        assert!(errors.contains("not reached over plain http"), "{address}: {errors}");
        assert!(errors.contains("tlsClientCert"), "{address}: {errors}");
        let (out, errors) = api.call(
            call_docker_api,
            &[("method", Value::string("GET")), ("path", Value::string("/version"))],
        );
        assert!(out.is_empty(), "{address}: {out:?}");
        assert!(errors.contains("not reached over plain http"), "{address}: {errors}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "{address}: nothing waited on"
        );
    }
    // an empty hostProtocol is http
    let api = Api::new("", "192.0.2.10", Some(2375), Seen::default());
    let (_, errors) = api.call(inspect_run_docker, &[("instanceRef", Value::string("a1"))]);
    assert!(errors.contains("not reached over plain http"), "{errors}");
}

/// Loopback in any form takes plain http: `localhost`, 127.0.0.0/8 and the IPv4-mapped `::ffff:127.0.0.1`.
#[test]
fn plain_http_to_this_machine_is_accepted_in_its_mapped_form_too() {
    for address in ["localhost", "::ffff:127.0.0.1"] {
        let api = Api::tcp_at(address, r#"<git-ci server-id="this"/>"#);
        let (out, errors) = api.call(inspect_run_docker, &[("instanceRef", Value::string("a1"))]);
        assert!(errors.is_empty(), "{address}: {errors}");
        assert_eq!(to_plain_string(&out["state"]), "running", "{address}");
        assert_eq!(api.seen()[0].1, "/v1.40/containers/a1/json", "{address}");
    }
}

/// `call#DockerApi` sends one call on the host's client and answers its status and parsed body; without
/// `INSTANCE_ADMIN` (authorization on, no such user) it sends nothing.
#[test]
fn the_docker_api_call_answers_the_host_and_needs_instance_admin() {
    let api = Api::tcp();
    let (out, errors) = api.call(
        call_docker_api,
        &[
            ("method", Value::string("post")),
            ("path", Value::string("/containers/create")),
            ("query", map(&[("name", "a b")])),
            ("body", Value::string(r#"{"Image":"moqui-build:1"}"#)),
            ("registryAuth", Value::string("e30=")),
        ],
    );
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(to_plain_string(&out["statusCode"]), "201");
    assert_eq!(to_plain_string(&out["reasonPhrase"]), "Created");
    let Some(Value::Map(body)) = out.get("responseBody") else {
        panic!("{out:?}")
    };
    assert_eq!(to_plain_string(&body["Id"]), "c0");
    let seen = api.seen();
    assert_eq!(seen[0].0, "POST");
    assert_eq!(seen[0].1, "/v1.40/containers/create?name=a%20b");
    assert_eq!(seen[0].2["Image"], "moqui-build:1");
    // a path that is not one under the API version
    let (out, errors) = api.call(call_docker_api, &[("path", Value::string("/../version"))]);
    assert!(out.is_empty() && errors.contains("is not a path"), "{errors}");

    let api = Api::tcp();
    let _entered = api.context.enter();
    let parameters: BTreeMap<String, Value> = [
        ("instanceHostId".to_string(), Value::string("H")),
        ("path".to_string(), Value::string("/version")),
    ]
    .into_iter()
    .collect();
    let out = call_docker_api(&parameters, &api.entity).unwrap();
    assert!(out.is_empty(), "{out:?}");
    let errors = ExecutionContext::with_current(|ec| ec.message().errors_text().join("\n")).unwrap_or_default();
    assert!(errors.contains("INSTANCE_ADMIN"), "{errors}");
    assert!(api.seen().is_empty(), "nothing sent: {:?}", api.seen());
}

/// With `container-user="image"` the container runs as its image's user, which is inspected before it starts: an
/// image with no user, root or a user name (which may be uid 0 in the image) is refused and what was made is removed;
/// one with a numeric uid of its own starts.
#[test]
fn an_image_user_that_is_root_is_refused_before_it_starts() {
    let api = Api::tcp_with(r#"<git-ci server-id="this" container-user="image"/>"#);
    let (out, errors) = api.call(start_run_docker, &start_parameters("runs-as-root:1"));
    assert!(!out.contains_key("instanceRef"), "{out:?}");
    assert!(errors.contains("may run as root"), "{errors}");
    let seen = api.seen();
    assert!(
        !seen.iter().any(|(_, target, _)| target.ends_with("/start")),
        "never started: {seen:?}"
    );
    assert!(seen
        .iter()
        .any(|(method, target, _)| method == "DELETE" && target.contains("/containers/c0")));
    assert!(seen[0].2.get("User").is_none(), "no user set: the image's");

    let api = Api::tcp_with(r#"<git-ci server-id="this" container-user="image"/>"#);
    let (out, errors) = api.call(start_run_docker, &start_parameters("named-user:1"));
    assert!(!out.contains_key("instanceRef"), "{out:?}");
    assert!(errors.contains("numeric uid"), "{errors}");

    let api = Api::tcp_with(r#"<git-ci server-id="this" container-user="image"/>"#);
    let (out, errors) = api.call(start_run_docker, &start_parameters("moqui-build:1"));
    assert!(errors.is_empty(), "{errors}");
    assert!(out.contains_key("instanceRef"));
}

/// SEC-GIT-93: an image that declares a VOLUME is refused before its container starts, whatever the container user,
/// and the container (with the volume the engine made) is removed: the volume would be on the host's disk, outside
/// the job's limits.
#[test]
fn an_image_with_a_volume_is_refused_before_it_starts() {
    let api = Api::tcp();
    let (out, errors) = api.call(start_run_docker, &start_parameters("with-volume:1"));
    assert!(!out.contains_key("instanceRef"), "{out:?}");
    assert!(errors.contains("VOLUME (/var/lib/data)"), "{errors}");
    let seen = api.seen();
    assert!(
        !seen.iter().any(|(_, target, _)| target.ends_with("/start")),
        "never started: {seen:?}"
    );
    assert!(seen.iter().any(|(method, target, _)| method == "DELETE"
        && target.contains("/containers/c0")
        && target.contains("v=true")));
}
