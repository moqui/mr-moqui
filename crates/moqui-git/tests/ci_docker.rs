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

//! The container driver on a real Docker or Podman API socket: one container, kept in as a job's is, run to its end
//! and removed (docs/interfaces/Pipelines.md "Tests"). Opt-in: it is not built or run by `cargo test`, only by name, and
//! does nothing without the socket.
//!
//! ```sh
//! MOQUI_CI_DOCKER_SOCKET=/var/run/docker.sock cargo test -p moqui-git --test ci_docker -- --nocapture
//! ```
//!
//! `MOQUI_CI_DOCKER_IMAGE` is the image, one with a shell (`docker.io/library/busybox:latest`); the host pulls it
//! when it does not have it.

use std::time::Duration;

use moqui_git::ci::driver::docker::DockerHost;

#[test]
fn a_container_runs_to_its_end_on_a_real_socket_and_is_removed() {
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
    let name = format!("moqui-ci-probe-{}", std::process::id());
    // not root, a root it cannot write, a tmpfs it can, and its own exit code
    let script = "test \"$(id -u)\" = 1000 || exit 11; touch /probe 2>/dev/null && exit 12; \
                  touch /tmp/probe || exit 13; exit 7";
    let exit = host
        .run_once(&name, &image, &["sh", "-c", script], Duration::from_secs(120))
        .unwrap();
    assert_eq!(exit, Some(7), "the container's exit code");
    // removed: the name is free again
    let again = host
        .run_once(&name, &image, &["sh", "-c", "exit 0"], Duration::from_secs(120))
        .unwrap();
    assert_eq!(again, Some(0));
}
