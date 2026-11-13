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

//! The binary on an empty runtime (crates/moqui-embedded, docs/Run-Deploy.md "Built-in framework files"): `moqui load`
//! writes a skeleton runtime and loads its seed data, and the server on it answers `/status`, the placeholder at `/`,
//! `/Login`, the first admin account, a REST login and logout and the entity REST interface, with no file reported as differing.
//! An acceptance test: it starts the embedded Postgres twice (`cargo test -p moqui --test cli -- --ignored skeleton`).

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use super::repo;

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The server, stopped by SIGTERM (so it stops its Postgres) when dropped.
struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = Command::new("kill").arg("-TERM").arg(self.0.id().to_string()).status();
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            if matches!(self.0.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn moqui(runtime: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_moqui"));
    command
        .current_dir(runtime.parent().unwrap())
        .env("MOQUI_RUNTIME", runtime)
        .env("MOQUI_INSTALL", repo().join("runtime/install"))
        .env_remove("MOQUI_FRAMEWORK")
        .env_remove("MOQUI_CONF");
    command
}

struct Response {
    status: u16,
    cookies: Vec<String>,
    body: String,
}

/// One HTTP/1.1 request on its own connection; `cookies` go in a Cookie header.
fn request(
    port: u16,
    method: &str,
    path: &str,
    cookies: &BTreeMap<String, String>,
    body: Option<(&str, &str)>,
) -> Response {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n");
    if !cookies.is_empty() {
        let pairs: Vec<String> = cookies.iter().map(|(name, value)| format!("{name}={value}")).collect();
        head.push_str(&format!("Cookie: {}\r\n", pairs.join("; ")));
    }
    if let Some((content_type, body)) = body {
        head.push_str(&format!(
            "Content-Type: {content_type}\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).unwrap();
    if let Some((_, body)) = body {
        stream.write_all(body.as_bytes()).unwrap();
    }
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let text = String::from_utf8_lossy(&raw).to_string();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.split(' ').nth(1).and_then(|code| code.parse().ok()).unwrap_or(0);
    let cookies = head
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("set-cookie")
                .then(|| value.trim().split(';').next().unwrap_or("").to_string())
        })
        .collect();
    let body = if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
        unchunk(body)
    } else {
        body.to_string()
    };
    Response { status, cookies, body }
}

fn unchunk(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some((size, after)) = rest.split_once("\r\n") {
        let size = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if size == 0 || after.len() < size {
            break;
        }
        out.push_str(&after[..size]);
        rest = after[size..].trim_start_matches("\r\n");
    }
    out
}

fn keep_cookies(jar: &mut BTreeMap<String, String>, response: &Response) {
    for cookie in &response.cookies {
        if let Some((name, value)) = cookie.split_once('=') {
            jar.insert(name.to_string(), value.to_string());
        }
    }
}

fn form(pairs: &[(&str, &str)]) -> String {
    let encode = |text: &str| {
        text.bytes()
            .map(|byte| match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (byte as char).to_string(),
                _ => format!("%{byte:02X}"),
            })
            .collect::<String>()
    };
    pairs
        .iter()
        .map(|(name, value)| format!("{}={}", encode(name), encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

#[test]
#[ignore = "acceptance: a complete scenario, a load and a server on the embedded Postgres of a skeleton runtime"]
fn a_lone_binary_builds_a_skeleton_runtime_that_loads_and_serves() {
    if !repo().join("runtime/install/postgres").is_dir() {
        eprintln!("skipped: no runtime/install/postgres");
        return;
    }
    let scratch = Scratch(PathBuf::from("/tmp").join(format!("mcs-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&scratch.0);
    std::fs::create_dir_all(&scratch.0).unwrap();
    let runtime = scratch.0.join("rt");

    let load = moqui(&runtime).arg("load").output().unwrap();
    let load_log = format!(
        "{}{}",
        String::from_utf8_lossy(&load.stdout),
        String::from_utf8_lossy(&load.stderr)
    );
    assert!(load.status.success(), "{load_log}");
    assert!(load_log.contains("wrote a skeleton runtime of"), "{load_log}");
    assert!(load_log.contains("match the ones built into this binary"), "{load_log}");
    assert!(runtime.join("base-component/framework/component.xml").is_file());
    assert!(runtime
        .join("base-component/webroot/screen/webroot/qapps.xml")
        .is_file());

    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut child = moqui(&runtime)
        .args([
            "-p",
            &port.to_string(),
            "--https-port",
            "off",
            "-D",
            "cluster_https_port=0",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let server = Server(child);
    // both streams are read to the end, so neither pipe fills; the log is what both printed
    let log = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    for (stream, ready) in [
        (Box::new(stdout) as Box<dyn Read + Send>, Some(ready_tx)),
        (Box::new(stderr) as Box<dyn Read + Send>, None),
    ] {
        let log = std::sync::Arc::clone(&log);
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                if line.starts_with("Listening on") {
                    if let Some(ready) = &ready {
                        let _ = ready.send(());
                    }
                }
                log.lock().unwrap().push_str(&format!("{line}\n"));
            }
        });
    }
    let started = ready_rx.recv_timeout(Duration::from_secs(180));
    assert!(started.is_ok(), "the server did not start:\n{}", log.lock().unwrap());

    let none = BTreeMap::new();
    let status = request(port, "GET", "/status", &none, None);
    assert_eq!(status.status, 200, "{}", status.body);

    let home = request(port, "GET", "/", &none, None);
    assert_eq!(home.status, 200, "{}", home.body);
    assert!(home.body.contains("skeleton runtime"), "{}", home.body);

    // the first admin account, from the Login page's form with its session token
    let mut jar = BTreeMap::new();
    let login_page = request(port, "GET", "/Login", &jar, None);
    assert_eq!(login_page.status, 200, "{}", login_page.body);
    keep_cookies(&mut jar, &login_page);
    let token = login_page
        .body
        .split("name=\"moquiSessionToken\" content=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("no session token in /Login:\n{}", login_page.body))
        .to_string();
    let created = request(
        port,
        "POST",
        "/Login/createInitialAdminAccount",
        &jar,
        Some((
            "application/x-www-form-urlencoded",
            &form(&[
                ("moquiSessionToken", &token),
                ("username", "skeleton.admin"),
                ("newPassword", "Skeleton-Admin-1"),
                ("newPasswordVerify", "Skeleton-Admin-1"),
                ("emailAddress", "skeleton.admin@example.com"),
            ]),
        )),
    );
    assert!(
        matches!(created.status, 200 | 302 | 303),
        "{} {}",
        created.status,
        created.body
    );

    let mut rest_jar = BTreeMap::new();
    let login = request(
        port,
        "POST",
        "/rest/login",
        &rest_jar,
        Some((
            "application/json",
            r#"{"username":"skeleton.admin","password":"Skeleton-Admin-1"}"#,
        )),
    );
    assert_eq!(login.status, 200, "{}", login.body);
    assert!(
        login
            .body
            .replace(char::is_whitespace, "")
            .contains("\"loggedIn\":true"),
        "{}",
        login.body
    );
    keep_cookies(&mut rest_jar, &login);
    // The entity REST interface answers the signed-in admin with the entity's authorization (ADMIN has no grant on
    // moqui.basic.Uom in the seed data, as in a full runtime) and an anonymous request with 401.
    let usd = request(port, "GET", "/rest/e1/moqui.basic.Uom/USD", &rest_jar, None);
    assert_eq!(usd.status, 403, "{}", usd.body);
    assert!(
        usd.body.contains("not authorized for AT_ENTITY [moqui.basic.Uom]"),
        "{}",
        usd.body
    );
    let anonymous = request(port, "GET", "/rest/e1/moqui.basic.Uom/USD", &none, None);
    assert_eq!(anonymous.status, 401, "{}", anonymous.body);
    let logout = request(port, "GET", "/rest/logout", &rest_jar, None);
    assert_eq!(logout.body.trim(), "successful", "{}", logout.body);

    drop(server);
    let log = log.lock().unwrap();
    assert!(log.contains("match the ones built into this binary"), "{log}");
    assert!(!log.contains("differ from the ones built into this binary"), "{log}");
}
