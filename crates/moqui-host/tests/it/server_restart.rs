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

//! Restart from the app (plans/design-dev-aide.md 6.5): the binary's server (`moqui_host::serve`, the loop the `moqui`
//! binary runs) in a child process on a seeded dev runtime, asked by an administrator's `restart#Server` (System › Conf ›
//! Changes, Restart Now), shuts down and runs itself again with the same arguments. On Unix the process is replaced in
//! place (`exec`), so its id stays and `./moq run` keeps it: the new image starts from `main` again, boots the runtime
//! again and answers on the same port.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]

use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crate::common::{exchange, free_port, http_request, session_token, sign_in};

/// The variable that makes [`restart_server_child`] the server; the runtime is `MOQUI_RUNTIME`.
const CHILD: &str = "MOQUI_RESTART_TEST_SERVER";
const CHILD_TEST: &str = "server_restart::restart_server_child";
/// The line the child prints each time its image starts, before it boots.
const STARTED: &str = "RESTART_CHILD_STARTED pid=";
const WAIT: Duration = Duration::from_secs(60);

/// The child: this test binary run for this test alone, with [`CHILD`] set, serves the runtime as the binary's server
/// does, and serves again after a restart, since the restart runs this binary again with these arguments. Run on its
/// own, it does nothing.
#[test]
fn restart_server_child() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    // what the test binary's process sets before a gate boots, again in each image
    crate::common::test_crypt_pass();
    crate::common::share_install();
    println!("{STARTED}{}", std::process::id());
    let _ = std::io::Write::flush(&mut std::io::stdout());
    moqui_host::serve().unwrap();
}

/// The lines of the child's standard output, read on a thread so a child that hangs fails the test instead of holding it.
fn lines_of(child: &mut Child) -> Receiver<String> {
    let stdout = child.stdout.take().unwrap();
    let (tx, lines) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    lines
}

/// The next line with `prefix`, the rest of it after the prefix.
fn next_with(lines: &Receiver<String>, prefix: &str, seen: &mut Vec<String>) -> String {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let line = lines
            .recv_timeout(left)
            .unwrap_or_else(|_| panic!("no {prefix:?} line from the server: {seen:#?}"));
        seen.push(line.clone());
        if let Some(at) = line.find(prefix) {
            return line[at + prefix.len()..].trim().to_string();
        }
    }
}

/// The child's process, ended with SIGTERM (a graceful stop, which stops its embedded Postgres) when the test ends.
struct Stop(Child);

impl Drop for Stop {
    fn drop(&mut self) {
        let _ = Command::new("kill").args(["-TERM", &self.0.id().to_string()]).status();
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if matches!(self.0.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// An administrator's Restart Now on a dev instance: the server answers, stops, and comes back as a new image of the
/// same process (the binary run again), which boots and answers on its port.
#[test]
fn a_restart_runs_the_server_again() {
    if !crate::common::own_process() {
        return;
    }
    let seeded = crate::common::seeded("restart", "");
    // the binary's server reads its cluster in the runtime
    std::fs::create_dir_all(seeded.runtime.join("db")).unwrap();
    std::os::unix::fs::symlink(&seeded.cluster_dir, seeded.runtime.join("db/embedded")).unwrap();
    let port = free_port();
    let setting = moqui_core::context::conf_env::env_name_of;
    let log = std::fs::File::create(seeded.runtime.join("child-stderr.log")).unwrap();
    let child = Command::new(std::env::current_exe().unwrap())
        .args([CHILD_TEST, "--exact", "--nocapture", "--test-threads", "1"])
        .env(CHILD, "1")
        .env("MOQUI_RUNTIME", &seeded.runtime)
        .env("MOQUI_FRAMEWORK", crate::common::framework_dir())
        .env("MOQUI_SHARED_RUNTIME", "off")
        .env(setting("instance@purpose"), "dev")
        .env(
            setting("webapp-list/webapp[webroot]@http-listen-port"),
            port.to_string(),
        )
        .env(setting("webapp-list/webapp[webroot]@http-bind"), "127.0.0.1")
        .env(setting("webapp-list/webapp[webroot]@https-listen-port"), "off")
        .env(setting("cluster-member@https-port"), "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(log)
        .spawn()
        .expect("the server's process");
    let mut child = Stop(child);
    let lines = lines_of(&mut child.0);
    let mut seen = Vec::new();
    let started = Instant::now();

    let first: u32 = next_with(&lines, STARTED, &mut seen).parse().unwrap();
    assert_eq!(first, child.0.id());
    let listening = next_with(&lines, "Listening on ", &mut seen);
    let address: SocketAddr = listening
        .parse()
        .unwrap_or_else(|_| panic!("the address in {listening}"));
    assert_eq!(address.port(), port);
    let booted = started.elapsed();

    // Restart Now, as System › Conf › Changes posts it, by an administrator with a session
    let cookie = sign_in(address, "john.doe", "moqui");
    let (status, _, page) = exchange(
        address,
        &http_request("GET", "/apps/System/Conf/Changes", &[("Cookie", &cookie)], ""),
    );
    assert_eq!(status, 200, "{page}");
    let token = session_token(&page).expect("the page carries the session token");
    let (status, headers, body) = exchange(
        address,
        &http_request(
            "POST",
            "/apps/System/Conf/Changes/restartServer",
            &[
                ("Cookie", &cookie),
                ("Content-Type", "application/x-www-form-urlencoded"),
                ("Origin", "http://localhost"),
            ],
            &format!("moquiSessionToken={token}"),
        ),
    );
    assert!(status == 302 || status == 200, "{status} {headers:?} {body}");

    // the same process, a new image: it starts from main again, boots, and listens on its port
    let again: u32 = next_with(&lines, STARTED, &mut seen).parse().unwrap();
    assert_eq!(again, first, "exec keeps the process id");
    assert!(
        matches!(child.0.try_wait(), Ok(None)),
        "the process did not end: it was replaced"
    );
    let listening = next_with(&lines, "Listening on ", &mut seen);
    assert_eq!(listening.parse::<SocketAddr>().ok(), Some(address), "{seen:#?}");
    let (status, _, text) = exchange(address, &http_request("GET", "/Login", &[], ""));
    assert_eq!(status, 200, "the restarted server answers: {text}");
    // the old session went with the old process's memory or stays in the database; either way a new sign-in works
    let _ = sign_in(address, "john.doe", "moqui");
    println!(
        "server_restart: first boot {booted:?}, restarted and answering after {:?}",
        started.elapsed()
    );
}
