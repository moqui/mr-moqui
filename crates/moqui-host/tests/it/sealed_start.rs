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
//! and closes the listeners, and the server that waits sealed after it unseals and serves. A wrong share is refused and
//! counted and the next attempt waits longer; a conf that names another method is ignored; an automatic (`tpm`, a
//! stub `systemd-creds`) first start holds its recovery shares once, and when it fails the recovery shares unseal; a
//! recovery bundle restored on a new runtime directory unseals with the shares, then rekeys and rotates the root key.

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

/// The texts of an answer's shares.
fn shares(answer: &Value, field: &str) -> Vec<String> {
    answer[field]
        .as_array()
        .unwrap()
        .iter()
        .map(|share| share.as_str().unwrap().to_string())
        .collect()
}

/// A stub of `systemd-creds` in `dir`: base64 for encrypt and decrypt, failing while `dir/fail` is there.
fn stub_systemd_creds(dir: &Path) -> PathBuf {
    let path = dir.join("systemd-creds");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n[ -e '{}/fail' ] && {{ echo 'no TPM' >&2; exit 1; }}\ncase \"$1\" in\n  encrypt) base64 ;;\n  \
             decrypt) base64 -d ;;\n  *) exit 2 ;;\nesac\n",
            dir.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Wait until the server's thread has ended after `seal`.
fn wait_stopped(server: &moqui_host::Server) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !server.is_stopped() {
        assert!(Instant::now() < deadline, "the server did not stop on seal");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_shamir_server_counts_a_wrong_share_ignores_another_conf_method_and_unseals_by_its_shares() {
    if !crate::common::own_process() {
        return;
    }
    let _serial = crate::common::serial();
    moqui_host::operator::require_gate();
    moqui_host::operator::set_service_uid_for_tests(u32::MAX - 7);
    let sockets = socket_dir("shamir");
    let socket = sockets.0.join("op.sock");
    moqui_lib::env::set_property("moqui_conf__cluster_member__https_port", "0");
    let seeded = seeded("shamir", &seal_conf(&socket));
    let http = free_port();
    let starting = start(&seeded, http);
    wait_for(&socket);
    let made = call(&socket, &json!({"command": "init", "method": "shamir"})).unwrap();
    let method_shares = shares(&made, "method_shares");
    assert_eq!((method_shares.len(), shares(&made, "recovery_shares").len()), (5, 5));
    // init opens the seal, and the server that waited serves
    let server = starting.join().unwrap().unwrap();
    assert!(serves(http));
    call(&socket, &json!({"command": "seal"})).unwrap();
    wait_stopped(&server);
    drop(server);

    // a conf that names the dev seal now is ignored: the recorded method stands
    moqui_lib::env::set_property("moqui_conf__seal__method", "dev");
    let keeper = moqui_core::seal::keeper_of(&seeded.runtime).unwrap();
    keeper.set_base_delay(Duration::from_millis(300));
    let starting = start(&seeded, http);
    std::thread::sleep(Duration::from_millis(300));
    let waiting = status(&socket);
    assert_eq!(
        (waiting["state"].as_str(), waiting["method"].as_str()),
        (Some("sealed"), Some("shamir"))
    );
    assert!(refused(http));

    // a wrong share is refused at once and counted, and the next attempt waits twice as long
    let wrong = "share-01-AAAAA-AAAAA-AAAAA";
    let err = call(&socket, &json!({"command": "unseal", "share": wrong})).unwrap_err();
    assert!(err.to_string().contains("wrong share"), "{err}");
    let first = status(&socket);
    assert_eq!(first["failures"], 1);
    let first_wait = first["retry_after_ms"].as_u64().unwrap();
    assert!(first_wait > 0 && first_wait <= 300);
    assert!(call(&socket, &json!({"command": "unseal", "share": method_shares[0]}))
        .unwrap_err()
        .to_string()
        .contains("wait"));
    std::thread::sleep(Duration::from_millis(first_wait + 20));
    assert!(call(&socket, &json!({"command": "unseal", "share": wrong})).is_err());
    let second = status(&socket);
    assert_eq!(second["failures"], 2);
    let second_wait = second["retry_after_ms"].as_u64().unwrap();
    assert!(second_wait > 300, "the wait doubles: {second_wait}");
    std::thread::sleep(Duration::from_millis(second_wait + 20));

    for (entered, share) in method_shares[1..4].iter().enumerate() {
        let answer = call(&socket, &json!({"command": "unseal", "share": share})).unwrap();
        let expected = if entered < 2 { "unsealing" } else { "unsealed" };
        assert_eq!(answer["status"]["state"], expected);
    }
    let server = starting.join().unwrap().unwrap();
    assert!(serves(http));
    drop(server);
    moqui_lib::env::set_property("moqui_conf__seal__method", String::new());
}

#[test]
fn a_tpm_first_start_holds_its_shares_once_and_a_failed_auto_unseal_waits_for_the_recovery_shares() {
    if !crate::common::own_process() {
        return;
    }
    let _serial = crate::common::serial();
    moqui_host::operator::require_gate();
    moqui_host::operator::set_service_uid_for_tests(u32::MAX - 7);
    let sockets = socket_dir("tpm");
    let socket = sockets.0.join("op.sock");
    let stub = stub_systemd_creds(&sockets.0);
    moqui_lib::env::set_property("moqui_conf__cluster_member__https_port", "0");
    let conf = format!(
        r#"<seal method="tpm" systemd-creds-command="{}" socket="{}" operator-group="{}"/>"#,
        stub.display(),
        socket.display(),
        rustix::process::getegid().as_raw()
    );
    let seeded = seeded("tpm", &conf);
    let http = free_port();
    // a production first start with an automatic method makes its seal and serves at once
    let server = start(&seeded, http).join().unwrap().unwrap();
    assert!(serves(http));
    assert_eq!(status(&socket)["shares_waiting"], true);
    let taken = call(&socket, &json!({"command": "init", "take_shares": true})).unwrap();
    let recovery = shares(&taken, "recovery_shares");
    assert_eq!(recovery.len(), 5);
    assert!(call(&socket, &json!({"command": "init", "take_shares": true})).is_err());
    call(&socket, &json!({"command": "seal"})).unwrap();
    wait_stopped(&server);
    drop(server);

    // the TPM refuses: the server waits sealed, and the recovery shares open it
    std::fs::write(sockets.0.join("fail"), "").unwrap();
    let unseal = call(&socket, &json!({"command": "unseal"})).unwrap_err();
    assert!(unseal.to_string().contains("no TPM"), "{unseal}");
    let starting = start(&seeded, http);
    std::thread::sleep(Duration::from_millis(300));
    assert!(refused(http));
    for share in &recovery[2..5] {
        call(&socket, &json!({"command": "unseal", "recovery": true, "share": share})).unwrap();
    }
    let server = starting.join().unwrap().unwrap();
    assert!(serves(http));
    drop(server);
}

#[test]
fn a_restore_of_the_bundle_on_a_new_runtime_unseals_with_the_shares_then_rekeys_and_rotates() {
    if !crate::common::own_process() {
        return;
    }
    let _serial = crate::common::serial();
    moqui_host::operator::require_gate();
    moqui_host::operator::set_service_uid_for_tests(u32::MAX - 7);
    let sockets = socket_dir("restore");
    moqui_lib::env::set_property("moqui_conf__cluster_member__https_port", "0");

    // the first host: a shamir seal with a bootstrap secret, and its recovery bundle
    let first_socket = sockets.0.join("a.sock");
    let first = seeded("restore-a", &seal_conf(&first_socket));
    let http = free_port();
    let starting = start(&first, http);
    wait_for(&first_socket);
    let made = call(&first_socket, &json!({"command": "init", "method": "shamir"})).unwrap();
    let recovery = shares(&made, "recovery_shares");
    let server = starting.join().unwrap().unwrap();
    let first_keeper = moqui_core::seal::keeper_of(&first.runtime).unwrap();
    first_keeper
        .set_bootstrap_secret("transactional-password", b"from-the-first-host")
        .unwrap();
    let root_id = first_keeper.status().root_id;
    let bundle = call(&first_socket, &json!({"command": "export-seal"})).unwrap()["bundle"]
        .as_str()
        .unwrap()
        .to_string();
    drop(server);

    // a new runtime directory on the restored database: the bundle, then the recovery shares
    let second_socket = sockets.0.join("b.sock");
    let second = seeded("restore-b", &seal_conf(&second_socket));
    let http = free_port();
    let starting = start(&second, http);
    wait_for(&second_socket);
    let restored = call(&second_socket, &json!({"command": "init", "bundle": bundle})).unwrap();
    assert_eq!(restored["status"]["state"], "sealed");
    assert_eq!(restored["status"]["method"], "shamir");
    for share in &recovery[..3] {
        call(
            &second_socket,
            &json!({"command": "unseal", "recovery": true, "share": share}),
        )
        .unwrap();
    }
    let server = starting.join().unwrap().unwrap();
    assert!(serves(http));
    let second_keeper = moqui_core::seal::keeper_of(&second.runtime).unwrap();
    assert_eq!(second_keeper.status().root_id, root_id);
    assert_eq!(
        second_keeper
            .bootstrap_secret("transactional-password")
            .unwrap()
            .unwrap()
            .as_slice(),
        b"from-the-first-host"
    );

    // rekey keeps the root key; rotate-root makes a new one; both by the recovery shares
    let rekeyed = call(
        &second_socket,
        &json!({"command": "rekey", "method": "shamir", "shares": recovery[1..4]}),
    )
    .unwrap();
    assert_eq!(shares(&rekeyed, "method_shares").len(), 5);
    assert_eq!(rekeyed["status"]["root_id"], root_id.as_str());
    let rotated = call(
        &second_socket,
        &json!({"command": "rotate-root", "shares": recovery[2..5]}),
    )
    .unwrap();
    assert_ne!(rotated["status"]["root_id"], root_id.as_str());
    assert_eq!(
        second_keeper
            .bootstrap_secret("transactional-password")
            .unwrap()
            .unwrap()
            .as_slice(),
        b"from-the-first-host"
    );
    assert!(call(
        &second_socket,
        &json!({"command": "rotate-root", "shares": recovery[..2]})
    )
    .is_err());
    drop(server);
}
