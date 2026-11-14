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

use super::{cleanup_runs_docker, create_body, inspect_run_docker, remove_run_docker, start_run_docker, Hardening};
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
        ("POST", "/v1.40/networks/create") => (201, json!({"Id": "n1"}).to_string()),
        ("GET", "/v1.40/containers/json") => (
            200,
            json!([
                {"Id": "old1", "Labels": {"moqui.ci.job": "J-ENDED", "moqui.ci.server": "this"}},
                {"Id": "live", "Labels": {"moqui.ci.job": "J-RUNNING", "moqui.ci.server": "this"}},
                {"Id": "old2", "Labels": {"moqui.ci.job": "J-LOST", "moqui.ci.server": "this"}},
            ])
            .to_string(),
        ),
        ("GET", "/v1.40/containers/running1/json") => (
            200,
            json!({"State": {"Status": "running", "Running": true, "ExitCode": 0}}).to_string(),
        ),
        ("GET", "/v1.40/containers/dead1/json") => (
            200,
            json!({"State": {"Status": "exited", "Running": false, "ExitCode": 137}}).to_string(),
        ),
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
        let context = ExecutionContextFactory::test_context_in(
            r#"<moqui-conf><git-ci server-id="this"/></moqui-conf>"#,
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
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen: Seen = Seen::default();
        let shared = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                serve(stream, &shared);
            }
        });
        Self::new("http", "127.0.0.1", Some(port), seen)
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
    assert_eq!(body["Volumes"], json!({"/workspace": {}}));
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
    assert_eq!(host["ReadonlyRootfs"], true);
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
    assert!(host["Tmpfs"]["/tmp"].as_str().unwrap().contains("size=1024m"));
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
    assert!(targets[3].ends_with("/start"), "{targets:?}");

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
    assert_eq!(seen[1].1, "/v1.40/containers/create?name=moqui-ci-J1-db");
    assert_eq!(seen[1].2["Image"], "postgres:16");
    assert_eq!(seen[1].2["Labels"]["moqui.ci.job"], "J1");
    assert_eq!(seen[1].2["HostConfig"]["Privileged"], false);
    assert_eq!(
        seen[1].2["NetworkingConfig"]["EndpointsConfig"]["moqui-ci-J1-net"]["Aliases"],
        json!(["db"])
    );
    assert!(seen[2].1.ends_with("/start"));
    assert_eq!(seen[3].1, "/v1.40/containers/create?name=moqui-ci-J1");
    assert_eq!(seen[3].2["HostConfig"]["NetworkMode"], "moqui-ci-J1-net");

    // a host that refuses: the daemon's words, and nothing started
    api.seen.lock().unwrap().clear();
    let (out, errors) = api.call(start_run_docker, &start_parameters("broken"));
    assert!(out.is_empty());
    assert!(
        errors.contains("creating container moqui-ci-J1: 500 the daemon is unwell"),
        "{errors}"
    );

    // inspect: running, exited with its code, missing
    for (id, state, exit) in [
        ("running1", "running", ""),
        ("dead1", "exited", "137"),
        ("nope", "missing", ""),
    ] {
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
        ],
    );
    assert!(errors.is_empty(), "{errors}");
    let targets: Vec<String> = api
        .seen()
        .iter()
        .map(|(method, target, _)| format!("{method} {target}"))
        .collect();
    assert!(
        targets[0].starts_with("GET /v1.40/containers/json?all=true&filters="),
        "{targets:?}"
    );
    assert!(targets[0].contains("moqui.ci.job%3DJ1"), "{targets:?}");
    assert!(
        targets.contains(&"DELETE /v1.40/containers/c0?force=true&v=true".to_string()),
        "{targets:?}"
    );
    assert_eq!(targets.last().unwrap(), "DELETE /v1.40/networks/moqui-ci-J1-net");

    // cleanup: this server's containers whose job run is not kept
    api.seen.lock().unwrap().clear();
    let (out, errors) = api.call(
        cleanup_runs_docker,
        &[
            ("labelName", Value::string("moqui.ci.job")),
            ("keepValues", Value::List(vec![Value::string("J-RUNNING")])),
            ("filterLabels", map(&[("moqui.ci.server", "this")])),
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
    assert!(
        !targets.iter().any(|target| target.contains("/containers/live")),
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
    let (out, _) = api.call(inspect_run_docker, &[("instanceRef", Value::string("running1"))]);
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
