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

//! The driver for containers that stay on a real Docker or Podman API socket (plans/archive/Vault-Instances.md §4.2, §7):
//! create a container with a secret file, start it, read the file back through its logs, and remove it; the secret is
//! in no part of what the host shows of the container. Opt-in: it is not built or run by `cargo test`, only by name,
//! and does nothing without the socket.
//!
//! ```sh
//! MOQUI_CI_DOCKER_SOCKET=/var/run/docker.sock cargo test -p moqui-lib --features docker --test docker_real -- --nocapture
//! ```
//!
//! `MOQUI_CI_DOCKER_IMAGE` is the image, one with a shell (`docker.io/library/busybox:latest`); the host pulls it
//! when it does not have it.

use std::time::{Duration, Instant};

use moqui_lib::docker::{ContainerSpec, DockerHost, Engine, SecretFile};
use moqui_lib::http::Method;

#[test]
fn a_secret_file_reaches_a_real_container_and_nothing_else() {
    let Some(socket) = std::env::var("MOQUI_CI_DOCKER_SOCKET")
        .ok()
        .filter(|socket| !socket.is_empty())
    else {
        eprintln!("MOQUI_CI_DOCKER_SOCKET is not set: nothing run");
        return;
    };
    let socket = socket.trim_start_matches("unix://").to_string();
    assert!(std::path::Path::new(&socket).exists(), "no socket at {socket}");
    let image =
        std::env::var("MOQUI_CI_DOCKER_IMAGE").unwrap_or_else(|_| "docker.io/library/busybox:latest".to_string());
    let host = DockerHost::unix(&socket);
    let engine = host.engine().unwrap();
    eprintln!("engine: {engine:?}");
    let mut value = [0u8; 16];
    getrandom::fill(&mut value).unwrap();
    let value: String = value.iter().map(|byte| format!("{byte:02x}")).collect();
    let spec = ContainerSpec {
        name: format!("moqui-driver-probe-{}", std::process::id()),
        image,
        pull: true,
        cmd: vec![
            "sh".to_string(),
            "-c".to_string(),
            "cat /run/secrets/probe; echo; stat -c '%a %u %g' /run/secrets/probe".to_string(),
        ],
        env: vec![("PROBE_FILE".to_string(), "/run/secrets/probe".to_string())],
        user: Some("1000:1000".to_string()),
        secrets: vec![SecretFile::new(
            "probe",
            zeroize::Zeroizing::new(value.as_bytes().to_vec()),
            1000,
            1000,
        )],
        ..ContainerSpec::default()
    };
    let id = host.create_spec(&spec).unwrap();
    // The checks run under catch_unwind so the container is removed even when one fails.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // what the host shows of the container holds no secret
        let shown = host
            .send(
                Method::Get,
                &format!("/containers/{id}/json"),
                None,
                Duration::from_secs(30),
            )
            .unwrap();
        assert!(
            !shown.body().contains(&value),
            "the secret is in the container's inspect"
        );
        host.start(&id).unwrap();
        let started = Instant::now();
        loop {
            let info = host.inspect(&id).unwrap().expect("the container is there");
            if !info.running && info.status != "created" {
                assert_eq!(info.exit_code, Some(0), "{info:?}");
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(120),
                "the container has not ended"
            );
            std::thread::sleep(Duration::from_millis(200));
        }
        let logs = host.logs(&id, 10).unwrap();
        assert!(logs.contains(&value), "the secret file's value: {logs}");
        assert!(logs.contains("400 1000 1000"), "its mode and owner: {logs}");
    }));
    host.remove_container(&id, true).unwrap();
    assert_eq!(host.inspect(&id).unwrap(), None, "removed");
    if let Err(failure) = outcome {
        std::panic::resume_unwind(failure);
    }
    if engine == Engine::Podman {
        eprintln!("the container's Podman secret was deleted with it");
    }
}
