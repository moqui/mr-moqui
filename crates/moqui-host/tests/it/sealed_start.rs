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

//! The sealed start (plans/Sealed-Secrets.md §3.3, §4 S3): a seeded server in the test's own process with the operator
//! gate on, as the `moqui` binary's server has it, driven over its operator socket as `moqui operator` drives it. While
//! sealed no port answers and systemd hears `READY=1` with `STATUS=sealed`; the server's own user runs only `status`;
//! the `dev` seal is refused on a production instance; a dev instance makes its seal by itself; `seal` drops the key
//! and closes the listeners, and the server that waits sealed after it unseals and serves.

#![cfg(all(unix, feature = "embedded-postgres"))]

use std::net::{SocketAddr, TcpStream};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use moqui_host::operator::client::call;
use serde_json::{json, Value};

use crate::common::{exchange, framework_dir, free_port, http_request, seeded, DirGuard, Seeded};

/// A short directory for the operator and notify sockets: a Unix socket's path is at most 104 bytes on macOS.
fn socket_dir(label: &str) -> DirGuard {
    let dir = PathBuf::from(format!("/tmp/msk-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    DirGuard(dir)
}

/// The conf of a test runtime whose operator socket is `socket` and whose operator group is this user's own group.
fn seal_conf(socket: &Path) -> String {
    format!(
        r#"<seal socket="{}" operator-group="{}"/>"#,
        socket.display(),
        rustix::process::getegid().as_raw()
    )
}

/// The server's start on its own thread, since it blocks while sealed.
fn start(seeded: &Seeded, http: u16) -> std::thread::JoinHandle<Result<moqui_host::Server, String>> {
    let (runtime, cluster) = (seeded.runtime.clone(), seeded.cluster_dir.clone());
    std::thread::spawn(move || {
        moqui_host::start_server(
            &runtime,
            framework_dir(),
            &cluster,
            SocketAddr::from(([127, 0, 0, 1], http)),
        )
        .map_err(|err| err.to_string())
    })
}

/// Wait for `path` to be there.
fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !path.exists() {
        assert!(Instant::now() < deadline, "{} did not appear", path.display());
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn status(socket: &Path) -> Value {
    call(socket, &json!({"command": "status"})).unwrap()["status"].clone()
}

fn refused(port: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), Duration::from_millis(500)).is_err()
}

/// Whether the server answers HTTP on `port`.
fn serves(port: u16) -> bool {
    let (code, _, _) = exchange(
        SocketAddr::from(([127, 0, 0, 1], port)),
        &http_request("GET", "/Login", &[], ""),
    );
    code == 200
}

#[test]
fn a_sealed_server_opens_no_port_and_its_own_user_runs_only_status() {
    if !crate::common::own_process() {
        return;
    }
    let _serial = crate::common::serial();
    moqui_host::operator::require_gate();
    let sockets = socket_dir("sealed");
    let socket = sockets.0.join("op.sock");
    let notify_path = sockets.0.join("notify.sock");
    let notify = UnixDatagram::bind(&notify_path).unwrap();
    notify.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    std::env::set_var("NOTIFY_SOCKET", &notify_path);
    let cluster_port = free_port();
    moqui_lib::env::set_property("moqui_conf__cluster_member__https_port", cluster_port.to_string());
    let seeded = seeded("sealed", &seal_conf(&socket));
    let http = free_port();
    let starting = start(&seeded, http);
    wait_for(&socket);

    // a production instance with no seal waits for init, and says so to systemd once its socket is open
    let mut buf = [0_u8; 256];
    let read = notify.recv(&mut buf).unwrap();
    assert_eq!(&buf[..read], b"READY=1\nSTATUS=sealed\n");
    let sealed = status(&socket);
    assert_eq!(sealed["state"], "sealed");
    assert_eq!(sealed["initialized"], false);
    assert!(refused(http) && refused(cluster_port), "no port answers while sealed");
    assert!(!starting.is_finished());

    // this test's user is the server's own user: status only
    for command in ["init", "unseal", "seal"] {
        let err = call(&socket, &json!({"command": command, "method": "dev"})).unwrap_err();
        assert!(err.to_string().contains("only status"), "{command}: {err}");
    }
    // as an operator (the operator group), the dev seal is still refused on a production instance
    moqui_host::operator::set_service_uid_for_tests(u32::MAX - 7);
    let err = call(&socket, &json!({"command": "init", "method": "dev"})).unwrap_err();
    assert!(err.to_string().contains("refused on a production instance"), "{err}");
    assert!(!seeded.runtime.join("db/seal.json").exists());
    assert!(refused(http));

    // a server stopping while sealed gives up the wait
    moqui_core::seal::keeper_of(&seeded.runtime).unwrap().abandon();
    let stopped = starting.join().unwrap();
    assert!(stopped.is_err_and(|err| err.contains("stopped while sealed")));
    std::env::remove_var("NOTIFY_SOCKET");
}

#[test]
fn a_dev_instance_makes_its_seal_and_seal_closes_the_listeners_until_unseal() {
    if !crate::common::own_process() {
        return;
    }
    let _serial = crate::common::serial();
    moqui_host::operator::require_gate();
    moqui_host::operator::set_service_uid_for_tests(u32::MAX - 7);
    let sockets = socket_dir("dev");
    let socket = sockets.0.join("op.sock");
    moqui_lib::env::set_property("moqui_conf__instance__purpose", "dev");
    moqui_lib::env::set_property("moqui_conf__cluster_member__https_port", "0");
    let seeded = seeded("dev-seal", &seal_conf(&socket));
    let http = free_port();
    let server = start(&seeded, http).join().unwrap().unwrap();
    assert!(serves(http));
    let seal = std::fs::read_to_string(seeded.runtime.join("db/seal.json")).unwrap();
    assert!(seal.contains("\"kind\": \"dev\""), "{seal}");
    assert_eq!(status(&socket)["state"], "unsealed");

    // seal: the key leaves memory and the listener closes
    let answer = call(&socket, &json!({"command": "seal"})).unwrap();
    assert_eq!(answer["status"]["state"], "sealed");
    let deadline = Instant::now() + Duration::from_secs(60);
    while !server.is_stopped() {
        assert!(Instant::now() < deadline, "the server did not stop on seal");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(refused(http));
    assert!(!moqui_core::seal::keeper_of(&seeded.runtime).unwrap().is_unsealed());
    drop(server);

    // the server boots again and waits sealed, even with the automatic dev method, until an operator unseals it
    let starting = start(&seeded, http);
    std::thread::sleep(Duration::from_millis(500));
    assert!(!starting.is_finished());
    assert!(refused(http));
    let answer = call(&socket, &json!({"command": "unseal"})).unwrap();
    assert_eq!(answer["status"]["state"], "unsealed");
    let server = starting.join().unwrap().unwrap();
    assert!(serves(http));
    drop(server);
}
