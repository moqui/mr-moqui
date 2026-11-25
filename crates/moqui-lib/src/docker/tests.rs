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

//! The driver for containers that stay, against the fake engine as Docker and as Podman (plans/Vault-Instances.md §6
//! M1): secrets go in files and nowhere else, and each call says what it should. On a real socket it is the opt-in
//! `docker_real` test.

use std::time::Duration;

use serde_json::json;
use zeroize::Zeroizing;

use super::fake::{FakeEngine, RemoveFault};
use super::{
    demux_logs, ContainerSpec, DockerHost, Engine, HealthCheck, HostAddress, Mount, MountKind, PortBinding,
    RestartPolicy, SecretFile, SECRETS_LABEL,
};

const PASSWORD: &[u8] = b"s3cret-value-of-the-database";

fn secret(name: &str, bytes: &[u8]) -> SecretFile {
    SecretFile::new(name, Zeroizing::new(bytes.to_vec()), 999, 999)
}

/// A database's container as an instance has it: settings that are not secret, its password a file read through the
/// image's `*_FILE` setting.
fn database() -> ContainerSpec {
    ContainerSpec {
        name: "pg-main".to_string(),
        image: "postgres:17".to_string(),
        env: vec![
            ("POSTGRES_USER".to_string(), "moqui".to_string()),
            (
                "POSTGRES_PASSWORD_FILE".to_string(),
                "/run/secrets/postgres-password".to_string(),
            ),
        ],
        labels: vec![("moqui.instance".to_string(), "pg-main".to_string())],
        mounts: vec![Mount {
            kind: MountKind::Volume,
            source: "pg-main-data".to_string(),
            target: "/var/lib/postgresql/data".to_string(),
            read_only: false,
        }],
        network: Some("moqui-prod".to_string()),
        ports: vec![PortBinding {
            container_port: 5432,
            protocol: "tcp".to_string(),
            host_port: Some(15432),
            host_ip: Some("127.0.0.1".to_string()),
        }],
        restart: RestartPolicy::UnlessStopped,
        health: Some(HealthCheck {
            test: vec!["CMD".to_string(), "pg_isready".to_string()],
            interval: Some(Duration::from_secs(10)),
            timeout: Some(Duration::from_secs(5)),
            retries: Some(3),
            start_period: None,
        }),
        user: Some("999:999".to_string()),
        cmd: vec![
            "postgres".to_string(),
            "-c".to_string(),
            "max_connections=200".to_string(),
        ],
        entrypoint: Vec::new(),
        secrets: vec![secret("postgres-password", PASSWORD)],
        pull: false,
        allow_host_access: false,
    }
}

#[test]
fn a_secret_in_the_environment_labels_or_command_is_refused_before_anything_is_sent() {
    let engine = FakeEngine::docker();
    let host = engine.host();
    let text = String::from_utf8(PASSWORD.to_vec()).unwrap();
    type Put<'a> = Box<dyn Fn(&mut ContainerSpec) + 'a>;
    let places: Vec<(&str, Put)> = vec![
        (
            "environment (POSTGRES_PASSWORD)",
            Box::new(|spec| spec.env.push(("POSTGRES_PASSWORD".to_string(), text.clone()))),
        ),
        // inside a longer value too
        (
            "environment (DATABASE_URL)",
            Box::new(|spec| {
                spec.env
                    .push(("DATABASE_URL".to_string(), format!("postgres://moqui:{text}@db/moqui")))
            }),
        ),
        (
            "labels (moqui.password)",
            Box::new(|spec| spec.labels.push(("moqui.password".to_string(), text.clone()))),
        ),
        ("command", Box::new(|spec| spec.cmd.push(format!("--password={text}")))),
        (
            "entrypoint",
            Box::new(|spec| spec.entrypoint = vec!["sh".to_string(), "-c".to_string(), format!("P={text} exec x")]),
        ),
        (
            "health check",
            Box::new(|spec| {
                spec.health.as_mut().unwrap().test = vec!["CMD-SHELL".to_string(), format!("check {text}")]
            }),
        ),
    ];
    for (place, put) in places {
        let mut spec = database();
        put(&mut spec);
        let err = host.create_spec(&spec).unwrap_err();
        assert!(err.message().contains(&format!("is in its {place}")), "{place}: {err}");
        assert!(err.message().contains("postgres-password"), "{err}");
        assert!(
            !err.message().contains(&text),
            "the value is never in the message: {err}"
        );
    }
    // a short secret only as a whole value, so it does not refuse every value it is part of
    let mut spec = database();
    spec.secrets = vec![secret("pin", b"200")];
    assert!(
        spec.check().is_ok(),
        "200 is part of the command's max_connections=200, not a whole value"
    );
    spec.env.push(("PIN".to_string(), "200".to_string()));
    assert!(spec.check().unwrap_err().message().contains("environment (PIN)"));
    // a long one anywhere in a value
    let mut spec = database();
    spec.secrets = vec![secret("max", b"max_connections")];
    assert!(spec.check().unwrap_err().message().contains("is in its command"));
    // names that would leave /run/secrets, two of one name, no value, the driver's own label
    let long = "a".repeat(65);
    for name in ["../etc/passwd", "a/b", ".hidden", "", &long] {
        let mut spec = database();
        spec.secrets = vec![secret(name, PASSWORD)];
        assert!(
            spec.check()
                .unwrap_err()
                .message()
                .contains("is not a secret file's name"),
            "{name}"
        );
    }
    let mut spec = database();
    spec.secrets.push(secret("postgres-password", b"another-long-value"));
    assert!(spec.check().unwrap_err().message().contains("two secrets"));
    let mut spec = database();
    spec.secrets = vec![secret("empty", b"")];
    assert!(spec.check().unwrap_err().message().contains("has no value"));
    let mut spec = database();
    spec.labels.push((SECRETS_LABEL.to_string(), "x".to_string()));
    assert!(spec.check().unwrap_err().message().contains("the driver's own"));
    let mut spec = database();
    spec.name = "../containers/x".to_string();
    assert!(spec.check().is_err());
    assert!(engine.requests().is_empty(), "nothing sent: {:?}", engine.calls());
    // a secret's Debug shows its size, not its value
    let shown = format!("{:?}", secret("postgres-password", PASSWORD));
    assert!(shown.contains("28 bytes") && !shown.contains("s3cret"), "{shown}");
}

#[test]
fn on_docker_the_secret_files_are_put_into_the_created_container_before_it_starts() {
    let engine = FakeEngine::docker();
    let host = engine.host();
    let id = host.create_spec(&database()).unwrap();
    assert_eq!(
        engine.calls(),
        [
            "GET /v1.40/version".to_string(),
            "POST /v1.40/containers/create?name=pg-main".to_string(),
            format!("PUT /v1.40/containers/{id}/archive?path=%2F"),
        ]
    );
    // the create body: no secret anywhere in it, the rest as the spec says
    assert!(
        engine.secret_outside_files(PASSWORD).is_empty(),
        "{:?}",
        engine.secret_outside_files(PASSWORD)
    );
    let body = engine.requests()[1].json();
    assert_eq!(
        body["Env"],
        json!([
            "POSTGRES_USER=moqui",
            "POSTGRES_PASSWORD_FILE=/run/secrets/postgres-password"
        ])
    );
    assert_eq!(body["Labels"], json!({"moqui.instance": "pg-main"}));
    assert_eq!(body["Cmd"], json!(["postgres", "-c", "max_connections=200"]));
    assert_eq!(body["User"], "999:999");
    assert_eq!(body["ExposedPorts"], json!({"5432/tcp": {}}));
    let host_config = &body["HostConfig"];
    assert_eq!(
        host_config["PortBindings"],
        json!({"5432/tcp": [{"HostIp": "127.0.0.1", "HostPort": "15432"}]})
    );
    assert_eq!(
        host_config["Mounts"],
        json!([{"Type": "volume", "Source": "pg-main-data", "Target": "/var/lib/postgresql/data", "ReadOnly": false}])
    );
    assert_eq!(
        host_config["RestartPolicy"],
        json!({"Name": "unless-stopped", "MaximumRetryCount": 0})
    );
    assert_eq!(host_config["NetworkMode"], "moqui-prod");
    assert_eq!(body["Healthcheck"]["Test"], json!(["CMD", "pg_isready"]));
    assert_eq!(body["Healthcheck"]["Interval"], 10_000_000_000u64);
    // the file: its directory, then the secret at 0400, owned as asked, put while the container was only created
    let files = engine.files(&id);
    assert_eq!(files.len(), 2, "{files:?}");
    assert_eq!(files[0].1.path, "run/secrets/");
    assert!(files[0].1.directory);
    assert_eq!(files[0].1.mode, 0o755);
    let (status, file) = &files[1];
    assert_eq!(status, "created");
    assert_eq!(file.path, "run/secrets/postgres-password");
    assert_eq!((file.mode, file.uid, file.gid), (0o400, 999, 999));
    assert_eq!(file.bytes, PASSWORD);
    assert_eq!(engine.containers()[&id].status, "created", "made, not started");

    // start, inspect (state, health, image), logs, stop, restart
    host.start(&id).unwrap();
    let info = host.inspect(&id).unwrap().unwrap();
    assert_eq!(info.name, "pg-main");
    assert_eq!(info.status, "running");
    assert!(info.running);
    assert_eq!(info.health.as_deref(), Some("healthy"));
    assert_eq!(info.image_id, format!("sha256:{}", "ab".repeat(32)));
    assert_eq!(
        info.repo_digests,
        [format!("example.com/app@sha256:{}", "ab".repeat(32))]
    );
    assert_eq!(info.labels["moqui.instance"], "pg-main");
    assert_eq!(host.logs(&id, 50).unwrap(), "pg-main started\nready\n");
    assert!(engine
        .calls()
        .last()
        .unwrap()
        .ends_with("/logs?stdout=true&stderr=true&tail=50"));
    host.stop(&id, Duration::from_secs(10)).unwrap();
    assert!(engine.calls().last().unwrap().ends_with("/stop?t=10"));
    assert_eq!(host.inspect(&id).unwrap().unwrap().status, "exited");
    host.stop(&id, Duration::from_secs(10)).unwrap();
    host.restart(&id, Duration::from_secs(5)).unwrap();
    assert!(host.inspect(&id).unwrap().unwrap().running);

    // remove without, then with, the anonymous volumes; one not there is removed
    host.remove_container(&id, false).unwrap();
    assert_eq!(engine.removed(), [(id.clone(), false)]);
    assert_eq!(host.inspect(&id).unwrap(), None);
    host.remove_container(&id, true).unwrap();
    let again = host.create_spec(&database()).unwrap();
    host.remove_container(&again, true).unwrap();
    assert_eq!(engine.removed().last().unwrap(), &(again, true));
    assert!(engine.calls().last().unwrap().ends_with("?force=true&v=true"));
    // an id that is not one goes into no path
    for bad in ["..", "a/b", "x?force=true", ""] {
        engine.clear();
        assert!(host.inspect(bad).is_err(), "{bad}");
        assert!(host.stop(bad, Duration::ZERO).is_err(), "{bad}");
        assert!(host.remove_container(bad, true).is_err(), "{bad}");
        assert!(host.logs(bad, 1).is_err(), "{bad}");
        assert!(engine.requests().is_empty(), "{bad}: {:?}", engine.calls());
    }
}

#[test]
fn on_docker_a_container_whose_files_are_not_put_is_removed() {
    let engine = FakeEngine::docker();
    let host = engine.host();
    let mut spec = database();
    spec.image = "read-only-root".to_string();
    let err = host.create_spec(&spec).unwrap_err();
    assert!(
        err.message()
            .contains("putting the secret files into container pg-main: 500"),
        "{err}"
    );
    assert!(engine.containers().is_empty(), "removed: {:?}", engine.containers());
    // an image the host does not have is pulled only when asked
    let mut spec = database();
    spec.image = "missing/postgres:17".to_string();
    let err = host.create_spec(&spec).unwrap_err();
    assert!(err.message().contains("is not on the host"), "{err}");
    spec.pull = true;
    host.create_spec(&spec).unwrap();
    assert!(engine
        .calls()
        .iter()
        .any(|call| call == "POST /v1.40/images/create?fromImage=missing%2Fpostgres&tag=17"));
}

#[test]
fn on_podman_secrets_are_podman_secrets_and_go_with_their_container() {
    let engine = FakeEngine::podman();
    let host = engine.host();
    assert_eq!(host.engine().unwrap(), Engine::Podman);
    let id = host.create_spec(&database()).unwrap();
    let calls = engine.calls();
    assert_eq!(calls[0], "GET /v1.40/version");
    assert!(
        calls[1].starts_with("POST /v4.0.0/libpod/secrets/create?name=moqui-"),
        "{calls:?}"
    );
    assert_eq!(calls[2], "POST /v4.0.0/libpod/containers/create");
    assert_eq!(calls.len(), 3, "no archive on Podman: {calls:?}");
    assert!(
        engine.secret_outside_files(PASSWORD).is_empty(),
        "{:?}",
        engine.secret_outside_files(PASSWORD)
    );
    let secrets = engine.podman_secrets();
    let (source, value) = secrets.iter().next().unwrap();
    assert_eq!(value, PASSWORD);
    // a short name of its own, whatever the container's and the file's names
    assert_eq!(source.len(), "moqui-".len() + 24, "{source}");
    let body = engine.requests()[2].json();
    assert_eq!(
        body["secrets"],
        json!([{"source": source, "target": "postgres-password", "uid": 999, "gid": 999, "mode": 0o400}])
    );
    assert_eq!(
        body["env"],
        json!({"POSTGRES_USER": "moqui", "POSTGRES_PASSWORD_FILE": "/run/secrets/postgres-password"})
    );
    assert_eq!(body["labels"][SECRETS_LABEL], json!(source));
    assert_eq!(body["command"], json!(["postgres", "-c", "max_connections=200"]));
    assert_eq!(
        body["volumes"],
        json!([{"Name": "pg-main-data", "Dest": "/var/lib/postgresql/data", "Options": []}])
    );
    assert_eq!(
        body["portmappings"],
        json!([{"container_port": 5432, "protocol": "tcp", "host_port": 15432, "host_ip": "127.0.0.1"}])
    );
    assert_eq!(body["restart_policy"], "unless-stopped");
    assert_eq!(body["Networks"], json!({"moqui-prod": {}}));
    assert_eq!(body["healthconfig"]["Test"], json!(["CMD", "pg_isready"]));
    // the calls after create are the compatible API's
    host.start(&id).unwrap();
    assert!(host.inspect(&id).unwrap().unwrap().running);
    // removed: the container, then its secrets
    host.remove_container(&id, true).unwrap();
    assert!(engine.containers().is_empty());
    assert!(engine.podman_secrets().is_empty(), "{:?}", engine.podman_secrets());
    assert!(engine
        .calls()
        .last()
        .unwrap()
        .starts_with("DELETE /v4.0.0/libpod/secrets/moqui-"));

    // a container of the longest name, with a long file name: its secret still goes with it
    let mut spec = database();
    spec.name = format!("pg-{}", "x".repeat(125));
    spec.secrets = vec![secret(&"p".repeat(64), PASSWORD)];
    let id = host.create_spec(&spec).unwrap();
    assert_eq!(engine.podman_secrets().len(), 1);
    host.remove_container(&id, false).unwrap();
    assert!(engine.podman_secrets().is_empty(), "{:?}", engine.podman_secrets());

    // a label naming a secret this driver did not make (another's, or a forged label) deletes nothing
    let port = engine.address().port.unwrap();
    let made = host
        .client()
        .post(format!(
            "http://127.0.0.1:{port}/v4.0.0/libpod/secrets/create?name=keep-me"
        ))
        .text("application/octet-stream", "another's")
        .send(false)
        .unwrap();
    assert!(made.status() < 300);
    let forged = host
        .create(
            "forged",
            &json!({"Image": "postgres:17", "Labels": {SECRETS_LABEL: "keep-me,moqui-not-hex"}}),
            false,
        )
        .unwrap();
    host.remove_container(&forged, true).unwrap();
    assert!(engine.podman_secrets().contains_key("keep-me"));
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call.contains("/secrets/keep-me") && call.starts_with("DELETE")));

    // the host's network, when allowed, is Podman's host namespace
    let mut spec = database();
    spec.network = Some("host".to_string());
    spec.allow_host_access = true;
    assert_eq!(spec.podman_body(&[])["netns"], json!({"nsmode": "host"}));
    assert!(spec.podman_body(&[]).get("Networks").is_none());

    // a container Podman refuses leaves no secret behind
    let mut spec = database();
    spec.image = "broken".to_string();
    let err = host.create_spec(&spec).unwrap_err();
    assert!(err.message().contains("creating container pg-main: 500"), "{err}");
    assert!(
        engine.podman_secrets().keys().all(|name| name == "keep-me"),
        "{:?}",
        engine.podman_secrets()
    );
    // Docker is Docker
    assert_eq!(FakeEngine::docker().host().engine().unwrap(), Engine::Docker);
}

fn bind(source: &str, target: &str) -> Mount {
    Mount {
        kind: MountKind::Bind,
        source: source.to_string(),
        target: target.to_string(),
        read_only: true,
    }
}

/// A secret named by its place when it is a setting's name, found with its newline trimmed; no mount where the secret
/// files go; no engine socket or host network unless the spec allows it; a 404 that names no image is no pull.
#[test]
fn names_mounts_sockets_and_the_hosts_network_are_held_to_the_rules() {
    let text = String::from_utf8(PASSWORD.to_vec()).unwrap();
    // in a setting's or a label's name: its place, never the name
    let mut spec = database();
    spec.env.push((text.clone(), "x".to_string()));
    let err = spec.check().unwrap_err();
    assert!(err.message().contains("the name of setting #2"), "{err}");
    assert!(!err.message().contains(&text), "{err}");
    let mut spec = database();
    spec.labels.push((format!("x.{text}"), "x".to_string()));
    let err = spec.check().unwrap_err();
    assert!(
        err.message().contains("the name of label #1") && !err.message().contains(&text),
        "{err}"
    );
    // a value read from a file with its newline is found without it
    let mut spec = database();
    spec.secrets = vec![secret("postgres-password", format!("{text}\n").as_bytes())];
    spec.env.push(("POSTGRES_PASSWORD".to_string(), text.clone()));
    assert!(spec
        .check()
        .unwrap_err()
        .message()
        .contains("environment (POSTGRES_PASSWORD)"));

    // a volume or a bind at, inside or around /run/secrets
    for target in [
        "/run/secrets",
        "/run/secrets/",
        "/run/secrets/postgres-password",
        "/run",
        "/",
        "//run/./secrets",
    ] {
        let mut spec = database();
        spec.mounts.push(bind("/srv/data", target));
        assert!(
            spec.check()
                .unwrap_err()
                .message()
                .contains("where its secret files go"),
            "{target}"
        );
    }
    for target in ["run/x", "/data/../run/secrets"] {
        let mut spec = database();
        spec.mounts.push(bind("/srv/data", target));
        assert!(
            spec.check().unwrap_err().message().contains("not an absolute path"),
            "{target}"
        );
    }
    let mut spec = database();
    spec.mounts.push(bind("/srv/data", "/run/secret-other"));
    assert!(spec.check().is_ok());

    // an engine's socket, or a directory holding one, and the host's network: only when allowed
    for source in [
        "/var/run/docker.sock",
        "/var/run",
        "/run/podman/podman.sock",
        "/run/user/1000/podman/podman.sock",
        "/run/user/1000",
        "/home/app/.docker/run/docker.sock",
        "/",
    ] {
        let mut spec = database();
        spec.mounts.push(bind(source, "/data"));
        assert!(spec.check().unwrap_err().message().contains("API socket"), "{source}");
        spec.allow_host_access = true;
        assert!(spec.check().is_ok(), "{source} allowed");
    }
    let mut spec = database();
    spec.mounts.push(bind("/srv/app/config", "/config"));
    assert!(spec.check().is_ok());
    // only bridge, none or a network's name: never the host's, another container's, a namespace by path or Podman's
    // user-mode networks, with or without options (SEC-GIT-93)
    for network in [
        "host",
        "HOST",
        "container:abc",
        "ns:/proc/1/ns/net",
        "slirp4netns",
        "slirp4netns:allow_host_loopback=true",
        "pasta",
        "pasta:-T,22",
        "private",
        "Container",
        "ns",
    ] {
        let mut spec = database();
        spec.network = Some(network.to_string());
        assert!(
            spec.check()
                .unwrap_err()
                .message()
                .contains("host's or another container's"),
            "{network}"
        );
        assert!(!super::valid_network(network), "{network}");
    }
    for network in ["bridge", "none", "moqui-prod", "podman"] {
        let mut spec = database();
        spec.network = Some(network.to_string());
        assert!(spec.check().is_ok(), "{network}");
    }
    let mut spec = database();
    spec.network = Some("host".to_string());
    spec.allow_host_access = true;
    assert!(spec.check().is_ok());

    // Docker's 404 for a network that is not there is an error, not a pull
    let engine = FakeEngine::docker();
    let mut spec = database();
    spec.network = Some("absent-net".to_string());
    spec.pull = true;
    let err = engine.host().create_spec(&spec).unwrap_err();
    assert!(err.message().contains("404 network absent-net not found"), "{err}");
    assert!(
        !engine.calls().iter().any(|call| call.contains("/images/create")),
        "{:?}",
        engine.calls()
    );
}

/// An image's `VOLUME` paths, read before a container of it is made; an image the host does not have is `None`.
#[test]
fn an_images_volumes_are_read() {
    let engine = FakeEngine::docker();
    let host = engine.host();
    assert_eq!(
        host.image_volumes("with-volume:1").unwrap(),
        Some(vec!["/data".to_string()])
    );
    assert_eq!(host.image_volumes("postgres:16").unwrap(), Some(Vec::new()));
    assert_eq!(host.image_volumes("missing/app:1").unwrap(), None);
    host.pull("missing/app:1").unwrap();
    assert_eq!(host.image_volumes("missing/app:1").unwrap(), Some(Vec::new()));
    assert!(
        engine
            .calls()
            .iter()
            .any(|call| call.contains("/images/with-volume%3A1/json")),
        "{:?}",
        engine.calls()
    );
}

#[test]
fn volumes_and_networks_are_made_inspected_and_removed() {
    let engine = FakeEngine::docker();
    let host = engine.host();
    host.create_volume("pg-main-data", &[("moqui.instance".to_string(), "pg-main".to_string())])
        .unwrap();
    let volume = host.inspect_volume("pg-main-data").unwrap().unwrap();
    assert_eq!(volume.name, "pg-main-data");
    assert_eq!(volume.driver, "local");
    assert_eq!(volume.labels["moqui.instance"], "pg-main");
    // made again: kept as it is
    host.create_volume("pg-main-data", &[]).unwrap();
    host.remove_volume("pg-main-data").unwrap();
    assert_eq!(host.inspect_volume("pg-main-data").unwrap(), None);
    host.remove_volume("pg-main-data").unwrap();
    assert!(host.create_volume("../x", &[]).is_err());

    host.create_network("moqui-prod", &json!({}), false).unwrap();
    assert!(
        host.create_network("moqui-prod", &json!({}), false).is_err(),
        "there already"
    );
    let id = host.create_spec(&database()).unwrap();
    host.connect_network("moqui-prod", &id).unwrap();
    host.delete_network("moqui-prod").unwrap();
    assert!(engine.networks().is_empty());
    host.delete_network("moqui-prod").unwrap();
}

#[test]
fn logs_are_read_framed_or_raw_and_a_host_address_shows_no_key() {
    let mut framed = vec![1, 0, 0, 0, 0, 0, 0, 3];
    framed.extend_from_slice(b"one");
    framed.extend_from_slice(&[2, 0, 0, 0, 0, 0, 0, 4]);
    framed.extend_from_slice(b"two\n");
    assert_eq!(demux_logs(&framed), "onetwo\n");
    // a terminal's output is not framed
    assert_eq!(demux_logs(b"plain text\n"), "plain text\n");
    // a frame cut short keeps what came
    assert_eq!(demux_logs(&[1, 0, 0, 0, 0, 0, 0, 9, b'a', b'b']), "ab");
    let address = HostAddress {
        protocol: Some("https".to_string()),
        address: Some("docker.example.com".to_string()),
        tls_client_key: Some("-----BEGIN PRIVATE KEY-----".to_string()),
        ..HostAddress::default()
    };
    assert!(!format!("{address:?}").contains("PRIVATE"), "{address:?}");
    // plain http goes to this machine only (SEC-OUT-08)
    let remote = HostAddress {
        address: Some("192.0.2.10".to_string()),
        ..HostAddress::default()
    };
    assert!(DockerHost::connect(&remote, Some(2375))
        .unwrap_err()
        .message()
        .contains("not reached over plain http"));
}

/// A removal the engine answers with an error is taken as done when the container is gone all the same, as rootless
/// Podman leaves it when only its network's cleanup fails (Ubuntu 26.04's AppArmor profile for pasta refuses Podman's
/// signal); one the engine refused, the container still there, is the engine's error.
#[test]
fn a_removal_whose_container_is_gone_is_done_though_its_network_cleanup_failed() {
    let engine = FakeEngine::podman();
    let host = engine.host();
    let id = host.create_spec(&database()).unwrap();
    engine.remove_fault(Some(RemoveFault::Refused));
    let err = host.remove_container(&id, true).unwrap_err();
    assert!(err.message().contains("device or resource busy"), "{err}");
    assert!(err.message().contains("500"), "{err}");
    assert!(host.inspect(&id).unwrap().is_some());
    engine.remove_fault(Some(RemoveFault::NetworkCleanup));
    host.remove_container(&id, true).unwrap();
    assert!(host.inspect(&id).unwrap().is_none());
    // the plain removal too
    engine.remove_fault(None);
    let other = host
        .create_spec(&ContainerSpec {
            name: "pg-other".to_string(),
            ..database()
        })
        .unwrap();
    engine.remove_fault(Some(RemoveFault::NetworkCleanup));
    host.remove(&other).unwrap();
    assert!(host.inspect(&other).unwrap().is_none());
}
