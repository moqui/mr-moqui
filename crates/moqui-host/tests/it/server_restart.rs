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
use std::path::{Path, PathBuf};
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
        // read to the end, the receiver gone or not, so the child never writes into a closed pipe
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = tx.send(line);
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

/// The child's process, ended with SIGTERM (a graceful stop, which stops its embedded Postgres) when the test ends; its
/// standard error is in `log`, whose end a failing test shows.
struct Stop {
    child: Child,
    log: PathBuf,
}

impl Drop for Stop {
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status();
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut ended = false;
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                ended = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if !ended {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if std::thread::panicking() {
            eprintln!("{}", log_tail(&self.log));
        }
    }
}

/// The last lines of the server's standard error, for a failure's message.
fn log_tail(log: &Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let mut tail: Vec<&str> = text.lines().rev().take(80).collect();
    tail.reverse();
    format!(
        "--- the end of the server's stderr ({}) ---\n{}",
        log.display(),
        tail.join("\n")
    )
}

/// The pid `data_dir/postmaster.pid` names, while it is there.
fn postmaster_pid(data_dir: &Path) -> Option<u32> {
    std::fs::read_to_string(data_dir.join("postmaster.pid"))
        .ok()?
        .lines()
        .next()?
        .trim()
        .parse()
        .ok()
}

/// Whatever the test's end, no postmaster of the cluster outlives it: stopped at once, as `moqui test` stops a scratch
/// runtime's (a panic before the child's signal handler, or the SIGKILL of [`Stop`], leaves one).
struct NoPostmaster {
    data_dir: PathBuf,
    install: PathBuf,
}

impl Drop for NoPostmaster {
    fn drop(&mut self) {
        moqui_host::db::embedded::stop_postmaster(&self.data_dir, &self.install, "immediate");
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
    let data_dir = seeded.cluster_dir.join("data");
    let _no_postmaster = NoPostmaster {
        data_dir: data_dir.clone(),
        install: moqui_host::db::embedded::shared_install_dir(&seeded.runtime, &seeded.cluster_dir),
    };
    let port = free_port();
    let setting = moqui_core::context::conf_env::env_name_of;
    let log_path = seeded.runtime.join("child-stderr.log");
    let log = std::fs::File::create(&log_path).unwrap();
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
    let mut child = Stop { child, log: log_path };
    let lines = lines_of(&mut child.child);
    let mut seen = Vec::new();
    let started = Instant::now();

    let first: u32 = next_with(&lines, STARTED, &mut seen).parse().unwrap();
    assert_eq!(first, child.child.id());
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
                // the transition's answer as JSON, with its messages, so a refusal shows here at once
                ("Accept", "application/json"),
                ("Origin", "http://localhost"),
            ],
            &format!("moquiSessionToken={token}"),
        ),
    );
    assert_eq!(status, 200, "{headers:?} {body}");
    let answer: serde_json::Value = serde_json::from_str(&body).unwrap_or_else(|err| panic!("{err}: {body}"));
    let errors = ["errors", "validationErrors"]
        .iter()
        .filter_map(|key| answer[key].as_array())
        .flatten()
        .count();
    assert_eq!(errors, 0, "Restart Now was refused: {body}");
    assert!(body.contains("restarting"), "the restart's message: {body}");

    // the same process, a new image: it starts from main again, boots, and listens on its port
    let again: u32 = next_with(&lines, STARTED, &mut seen).parse().unwrap();
    assert_eq!(again, first, "exec keeps the process id");
    assert!(
        matches!(child.child.try_wait(), Ok(None)),
        "the process did not end: it was replaced"
    );
    let listening = next_with(&lines, "Listening on ", &mut seen);
    assert_eq!(listening.parse::<SocketAddr>().ok(), Some(address), "{seen:#?}");
    let (status, _, text) = exchange(address, &http_request("GET", "/Login", &[], ""));
    assert_eq!(status, 200, "the restarted server answers: {text}");
    // the old session went with the old process's memory or stays in the database; either way a new sign-in works
    let _ = sign_in(address, "john.doe", "moqui");
    let restarted = started.elapsed();

    // the new image owns the cluster: its stop stops the postmaster, which outlives no image of the process
    let postmaster = postmaster_pid(&data_dir).expect("the restarted server's postmaster");
    let log = child.log.clone();
    drop(child);
    let deadline = Instant::now() + Duration::from_secs(10);
    while moqui_host::db::embedded::process_live(postmaster) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !moqui_host::db::embedded::process_live(postmaster),
        "postmaster {postmaster} outlived the server's process\n{}",
        log_tail(&log)
    );
    println!("server_restart: first boot {booted:?}, restarted and answering after {restarted:?}");
}
