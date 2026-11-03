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

use super::*;
use std::net::{Ipv4Addr, TcpListener};
use std::sync::mpsc::Receiver;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("moqui-desktop-sidecar-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_binary_is_the_variable_then_the_builds_or_beside_the_app() {
    let dir = temp_dir("find");
    let name = format!("moqui{}", std::env::consts::EXE_SUFFIX);
    let target = dir.join("target");
    let exe_dir = dir.join("app");
    let explicit = dir.join("custom-moqui");
    assert!(find_binary(Some(explicit.clone()), true, &target, Some(&exe_dir))
        .unwrap_err()
        .contains("MOQUI_DESKTOP_MOQUI"));
    std::fs::write(&explicit, "").unwrap();
    assert_eq!(
        find_binary(Some(explicit.clone()), false, &target, None).unwrap(),
        explicit
    );

    assert!(find_binary(None, true, &target, Some(&exe_dir))
        .unwrap_err()
        .contains("./moq build"));
    std::fs::create_dir_all(target.join("debug")).unwrap();
    std::fs::write(target.join("debug").join(&name), "").unwrap();
    assert_eq!(
        find_binary(None, true, &target, None).unwrap(),
        target.join("debug").join(&name)
    );
    std::fs::create_dir_all(target.join("release")).unwrap();
    std::fs::write(target.join("release").join(&name), "").unwrap();
    assert_eq!(
        find_binary(None, true, &target, None).unwrap(),
        target.join("release").join(&name)
    );

    assert!(
        find_binary(None, false, &target, Some(&exe_dir)).is_err(),
        "a bundle does not look in target"
    );
    std::fs::create_dir_all(&exe_dir).unwrap();
    std::fs::write(exe_dir.join(&name), "").unwrap();
    assert_eq!(
        find_binary(None, false, &target, Some(&exe_dir)).unwrap(),
        exe_dir.join(&name)
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_node_gets_the_listen_properties_and_stops_on_stdin_close() {
    let args = args(51234, &[]);
    assert!(args.chunks(2).all(|pair| pair[0] == "-D"));
    assert!(args.contains(&"webapp_http_port=51234".to_string()));
    assert!(args.contains(&"webapp_http_bind=127.0.0.1".to_string()));
    assert!(args.contains(&"moqui_stop_on_stdin_close=true".to_string()));
}

#[test]
fn the_node_gets_the_backup_properties_as_defines() {
    let args = args(51234, &[("embedded_db_backup_dir", "/b/MR Moqui".to_string())]);
    let at = args
        .iter()
        .position(|arg| arg == "embedded_db_backup_dir=/b/MR Moqui")
        .unwrap();
    assert_eq!(args[at - 1], "-D");
}

#[test]
fn the_listening_line_gives_the_address() {
    assert_eq!(
        listening_address("Listening on 127.0.0.1:51234"),
        Some("127.0.0.1:51234".parse().unwrap())
    );
    assert_eq!(
        listening_address("Listening on 127.0.0.1:8080 (pid 42)"),
        Some("127.0.0.1:8080".parse().unwrap())
    );
    assert_eq!(listening_address("INFO Listening on nothing"), None);
    assert_eq!(listening_address("Listening on soon"), None);
}

#[test]
fn only_a_ready_200_is_ready() {
    let ok = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\r\n{\"ready\":true}";
    assert!(health_ready(ok));
    assert!(!health_ready(
        "HTTP/1.1 503 Service Unavailable\r\n\r\n{\"ready\":false}"
    ));
    assert!(!health_ready(
        "HTTP/1.1 503 Service Unavailable\r\n\r\n{\"ready\":false,\"draining\":true}"
    ));
    assert!(!health_ready("HTTP/1.1 404 Not Found\r\n\r\nnot found"));
    assert!(!health_ready(""));
}

/// A `/health` that answers not ready `not_ready` times, then ready, on a loopback port.
fn health_server(not_ready: usize) -> SocketAddr {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for (count, stream) in listener.incoming().enumerate() {
            let Ok(mut stream) = stream else { continue };
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let response = if count < not_ready {
                "HTTP/1.1 503 Service Unavailable\r\nconnection: close\r\n\r\n{\"ready\":false}"
            } else {
                "HTTP/1.1 200 OK\r\nconnection: close\r\n\r\n{\"ready\":true}"
            };
            let _ = stream.write_all(response.as_bytes());
        }
    });
    address
}

#[test]
fn readiness_waits_for_health_and_gives_up_on_cancel_or_timeout() {
    assert_eq!(
        wait_ready(health_server(2), Duration::from_secs(10), || false),
        Ok(true)
    );
    assert_eq!(
        wait_ready(health_server(0), Duration::from_secs(10), || true),
        Ok(false)
    );
    let nobody = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap();
    assert!(wait_ready(nobody, Duration::from_millis(300), || false).is_err());
}

#[cfg(unix)]
fn sh(script: &str) -> (Mutex<Child>, Option<ChildStdin>) {
    let mut child = Command::new("sh")
        .args(["-c", script])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take();
    (Mutex::new(child), stdin)
}

#[cfg(unix)]
#[test]
fn a_node_that_reads_stdin_stops_when_it_closes() {
    use std::os::unix::process::ExitStatusExt as _;
    let (child, stdin) = sh("cat >/dev/null");
    let started = Instant::now();
    let status = stop_child(&child, stdin, Duration::from_secs(10), Duration::from_secs(10)).unwrap();
    assert!(status.success(), "{status:?}");
    assert!(status.signal().is_none());
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[cfg(unix)]
#[test]
fn a_node_that_ignores_stdin_gets_sigterm_then_a_kill() {
    use std::os::unix::process::ExitStatusExt as _;
    let (child, stdin) = sh("exec sleep 30");
    let status = stop_child(&child, stdin, Duration::from_millis(200), Duration::from_secs(10)).unwrap();
    assert_eq!(status.signal(), Some(libc::SIGTERM));

    let (child, stdin) = sh("trap '' TERM; while :; do sleep 0.1; done");
    let status = stop_child(&child, stdin, Duration::from_millis(200), Duration::from_millis(300)).unwrap();
    assert_eq!(status.signal(), Some(libc::SIGKILL));
}

/// A script in place of `moqui`, as `Node::start_binary` runs it.
#[cfg(unix)]
fn fake_moqui(name: &str, script: &str) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = temp_dir(name);
    let binary = dir.join("moqui");
    std::fs::write(&binary, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    (dir, binary)
}

fn events() -> (impl Fn(Event) + Send + Sync + 'static, Receiver<Event>) {
    let (sender, receiver) = channel();
    let sender = Mutex::new(sender);
    (move |event| drop(lock(&sender).send(event)), receiver)
}

#[cfg(unix)]
#[test]
fn a_sidecar_starts_at_its_listening_line_once_ready_and_stops_with_its_stdin() {
    let address = health_server(1);
    let (dir, binary) = fake_moqui(
        "start",
        &format!("test \"$MOQUI_RUNTIME\" = /runtime || exit 9\necho \"Listening on {address}\"\ncat >/dev/null"),
    );
    let (events, heard) = events();
    let node = Node::start_binary(&binary, Path::new("/runtime"), address.port(), &[], events).unwrap();
    match heard.recv_timeout(Duration::from_secs(10)).unwrap() {
        Event::Started(started) => assert_eq!(started, address),
        Event::Failed(err) => panic!("failed: {err}"),
        Event::Stopped { .. } => panic!("stopped"),
    }
    let started = Instant::now();
    node.stop();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the stop waited for SIGTERM"
    );
    assert!(
        heard.recv_timeout(Duration::from_millis(500)).is_err(),
        "a stop is not a failure"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn a_sidecar_that_ends_by_itself_fails_with_its_last_lines() {
    let (dir, binary) = fake_moqui("ends", "echo 'the runtime has no conf' >&2\nexit 3");
    let (events, heard) = events();
    let node = Node::start_binary(&binary, Path::new("/runtime"), 1, &[], events).unwrap();
    match heard.recv_timeout(Duration::from_secs(10)).unwrap() {
        Event::Failed(err) => {
            assert!(err.contains("the runtime has no conf"), "{err}");
            assert!(err.contains('3'), "{err}");
        }
        _ => panic!("not a failure"),
    }
    node.stop();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_message_has_the_status_and_the_warnings_and_errors_or_the_last_lines() {
    let boot = |count: usize| -> VecDeque<String> {
        (0..count)
            .map(|n| format!("2026-11-03 10:00:00.000  INFO   [main] core  line {n}"))
            .collect()
    };
    let quiet = boot(50);
    let message = ended_message("signal: 9 (SIGKILL)", &quiet);
    assert!(
        message.starts_with("The node ended (signal: 9 (SIGKILL)):"),
        "{message}"
    );
    let lines = summary(&quiet);
    assert_eq!(lines.len(), 5);
    assert!(lines[4].ends_with("line 49"));

    let mut troubled = boot(40);
    for n in 0..10 {
        troubled.push_back(format!("2026-11-03 10:00:01.000  WARN   [main] core  warning {n}"));
        troubled.push_back(format!("2026-11-03 10:00:01.000  INFO   [main] core  after {n}"));
    }
    troubled.push_back("2026-11-03 10:00:02.000  ERROR  [main] host  the database did not open".to_string());
    troubled.push_back("Error: could not start".to_string());
    let lines = summary(&troubled);
    assert_eq!(lines.len(), 8);
    assert!(lines[0].ends_with("warning 4"), "{lines:?}");
    assert!(lines[6].ends_with("the database did not open"));
    assert_eq!(lines[7], "Error: could not start");
    assert!(lines.iter().all(|line| !line.contains(" INFO ")));

    let long: VecDeque<String> = [format!("ERROR {}", "é".repeat(500))].into_iter().collect();
    let lines = summary(&long);
    assert_eq!(lines[0].chars().count(), 301);
    assert!(lines[0].ends_with('…'));

    assert_eq!(
        ended_message("exit status: 3", &VecDeque::new()),
        "The node ended (exit status: 3)."
    );
}
