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

//! Headless Chromium smokes of the staged browser Wasm package.
//!
//! Skips when the package directory is absent (`target/browser/wasm-pkg/debug`, or `MOQUI_WASM_PKG`) or when no
//! Chromium is installed. Does not run `scripts/wasm-build.sh`. The Chrome smoke is `js/smoke.html` (OPFS reload,
//! `/echo`, `/fail`, `/slow`). The WebSocket smoke is `js/ws-smoke.html` against a `/notws` mock on the same origin.

#![cfg(feature = "test-client")]

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use moqui_host::test_client::cdp::{Browser, Launch};
use tungstenite::protocol::Role;
use tungstenite::{Message, WebSocket};

const SMOKE_WAIT: Duration = Duration::from_secs(120);

/// The package directory and a Chromium binary, or a printed skip.
fn prepared() -> Option<(PathBuf, PathBuf)> {
    let pkg = match std::env::var("MOQUI_WASM_PKG") {
        Ok(path) => PathBuf::from(path),
        Err(_) => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/browser/wasm-pkg/debug"),
    };
    if !pkg.is_dir() {
        eprintln!(
            "skipping: no Wasm package at {} (build it with scripts/wasm-build.sh, or set MOQUI_WASM_PKG)",
            pkg.display()
        );
        return None;
    }
    let runtime = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime");
    let configured = std::env::var("MOQUI_CHROMIUM").unwrap_or_default();
    match moqui_host::chromium::find_test_browser(Some(&runtime), &configured) {
        Ok(binary) => Some((pkg, binary)),
        Err(reason) => {
            eprintln!("skipping: {reason}");
            None
        }
    }
}

fn launch(binary: PathBuf) -> Result<Browser, String> {
    Browser::launch(&Launch {
        binary,
        headed: false,
        window: (1280, 900),
        profile_prefix: "moqui-wasm-smoke".to_string(),
        extra_args: Vec::new(),
    })
}

/// Navigate to `url` and poll `#result` until it leaves `running` or [`SMOKE_WAIT`] passes. The text of `#log`.
///
/// The smoke pages are module scripts, so the load event waits until the smoke itself finishes. Poll `#result`
/// instead of waiting for that event (the same as the Node drivers, which allow 120s). A same-URL reload keeps the
/// previous document until the new one commits, so the poll waits until a mark on the old document is gone.
fn run_page(browser: &mut Browser, url: &str) -> Result<(String, String), String> {
    let _ = browser.try_eval("document.documentElement.dataset.moquiSmoke = '1'");
    let started = browser.call("Page.navigate", serde_json::json!({ "url": url }))?;
    if let Some(error) = started.get("errorText") {
        return Err(format!("navigate {url}: {error}"));
    }
    let deadline = Instant::now() + SMOKE_WAIT;
    let mut result = "running".to_string();
    let mut fresh = false;
    while Instant::now() < deadline {
        thread::sleep(Duration::from_millis(500));
        if !fresh {
            fresh = matches!(
                browser.try_eval("document.documentElement.dataset.moquiSmoke !== '1'"),
                Ok(serde_json::Value::Bool(true))
            );
            if !fresh {
                continue;
            }
        }
        if let Ok(value) = browser.try_eval("document.getElementById('result')?.textContent ?? 'running'") {
            if let Some(text) = value.as_str() {
                result = text.to_string();
            }
        }
        if result != "running" {
            break;
        }
    }
    let log = browser
        .try_eval("document.getElementById('log')?.textContent ?? ''")
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default();
    Ok((result, log))
}

struct Head {
    method: String,
    path: String,
    query: String,
    headers: Vec<(String, String)>,
    rest: Vec<u8>,
}

/// The package files plus `/echo`, `/fail`, `/slow`, and the `/notws` mock, on `127.0.0.1:0`.
struct SmokeServer {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    joins: Arc<Mutex<Vec<JoinHandle<()>>>>,
    accept: Option<JoinHandle<()>>,
    received: Arc<Mutex<Vec<String>>>,
    connections: Arc<AtomicUsize>,
}

impl Drop for SmokeServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.addr);
        if let Some(handle) = self.accept.take() {
            let _ = handle.join();
        }
        let handles = std::mem::take(&mut *lock(&self.joins));
        for handle in handles {
            let _ = handle.join();
        }
    }
}

fn serve(pkg: &Path) -> std::io::Result<SmokeServer> {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    let addr = listener.local_addr()?;
    let stop = Arc::new(AtomicBool::new(false));
    let joins = Arc::new(Mutex::new(Vec::new()));
    let received = Arc::new(Mutex::new(Vec::new()));
    let connections = Arc::new(AtomicUsize::new(0));
    let next_id = Arc::new(AtomicU64::new(1));
    let stop_accept = Arc::clone(&stop);
    let joins_accept = Arc::clone(&joins);
    let pkg_accept = pkg.to_path_buf();
    let received_accept = Arc::clone(&received);
    let connections_accept = Arc::clone(&connections);
    let next_id_accept = Arc::clone(&next_id);
    let accept = thread::spawn(move || {
        while !stop_accept.load(Ordering::Relaxed) {
            let Ok((stream, _)) = listener.accept() else {
                break;
            };
            if stop_accept.load(Ordering::Relaxed) {
                break;
            }
            let pkg = pkg_accept.clone();
            let stop = Arc::clone(&stop_accept);
            let received = Arc::clone(&received_accept);
            let connections = Arc::clone(&connections_accept);
            let next_id = Arc::clone(&next_id_accept);
            let handle = thread::spawn(move || handle_client(stream, pkg, stop, received, connections, next_id));
            lock(&joins_accept).push(handle);
        }
    });
    Ok(SmokeServer {
        addr,
        stop,
        joins,
        accept: Some(accept),
        received,
        connections,
    })
}

fn handle_client(
    mut stream: TcpStream,
    pkg: PathBuf,
    stop: Arc<AtomicBool>,
    received: Arc<Mutex<Vec<String>>>,
    connections: Arc<AtomicUsize>,
    next_id: Arc<AtomicU64>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let Ok(head) = read_head(&mut stream) else {
        return;
    };
    if head.path == "/notws" {
        if is_upgrade(&head) {
            let _ = serve_socket(stream, head, &stop, &received, &connections, &next_id);
        } else {
            let _ = write_response(&mut stream, 404, "text/plain", b"not found");
        }
        return;
    }
    if head.path == "/echo" {
        let body = serde_json::json!({
            "echo": "ok",
            "method": head.method,
            "note": query_get(&head.query, "note"),
        })
        .to_string();
        let _ = write_response(&mut stream, 200, "application/json", body.as_bytes());
        return;
    }
    if head.path == "/fail" {
        let _ = write_response(&mut stream, 500, "text/plain", b"no");
        return;
    }
    if head.path == "/slow" {
        answer_slow(stream, &stop);
        return;
    }
    match file_under(&pkg, &head.path) {
        Some(body) => {
            let kind = content_type(&head.path);
            let _ = write_response(&mut stream, 200, kind, &body);
        }
        None => {
            let _ = write_response(&mut stream, 404, "text/plain", b"not found");
        }
    }
}

/// `/slow` answers `{}` after 20s, and stops when the client disconnects or the server is dropped.
fn answer_slow(mut stream: TcpStream, stop: &AtomicBool) {
    let start = Instant::now();
    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
    let mut buf = [0u8; 8];
    while start.elapsed() < Duration::from_secs(20) {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        match stream.peek(&mut buf) {
            Ok(0) => return,
            Err(err) if !timed_out(&err) => return,
            Ok(_) => thread::sleep(Duration::from_millis(200)),
            Err(_) => {}
        }
    }
    if !stop.load(Ordering::Relaxed) {
        let _ = write_response(&mut stream, 200, "application/json", b"{}");
    }
}

fn serve_socket(
    mut stream: TcpStream,
    head: Head,
    stop: &AtomicBool,
    received: &Mutex<Vec<String>>,
    connections: &AtomicUsize,
    next_id: &AtomicU64,
) -> Result<(), String> {
    let key = header_value(&head, "sec-websocket-key").ok_or("no Sec-WebSocket-Key")?;
    let accept = tungstenite::handshake::derive_accept_key(key.as_bytes());
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    stream.write_all(response.as_bytes()).map_err(|err| err.to_string())?;
    stream.flush().map_err(|err| err.to_string())?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(400)));
    let mut socket = WebSocket::from_partially_read(stream, head.rest, Role::Server, None);
    let n = connections.fetch_add(1, Ordering::Relaxed) + 1;
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match socket.read() {
            Ok(Message::Text(text)) => {
                let text = text.as_str().to_string();
                lock(received).push(text.clone());
                if text == "subscribe:ALL" {
                    let id = next_id.fetch_add(1, Ordering::Relaxed);
                    let _ = socket.send(Message::text(notification("Hello", &format!("connection {n}"), id)));
                    let _ = socket.send(Message::binary(vec![1, 2, 3]));
                } else if let Some(echoed) = text.strip_prefix("echo:") {
                    let _ = socket.send(Message::text(echoed));
                } else if text == "from-module" {
                    let id = next_id.fetch_add(1, Ordering::Relaxed);
                    let _ = socket.send(Message::text(notification("Ack", "module frame received", id)));
                } else if text == "drop" {
                    let raw = socket.into_inner();
                    let _ = raw.shutdown(Shutdown::Both);
                    return Ok(());
                }
            }
            Ok(Message::Binary(payload)) => {
                lock(received).push(format!("binary:{}", payload.len()));
                let _ = socket.send(Message::binary(payload));
            }
            Ok(Message::Ping(payload)) => {
                let _ = socket.send(Message::Pong(payload));
            }
            Ok(Message::Close(_)) => {
                let _ = socket.send(Message::Close(None));
                break;
            }
            Ok(Message::Pong(_) | Message::Frame(_)) => {}
            Err(tungstenite::Error::Io(err)) if timed_out(&err) => {}
            Err(_) => break,
        }
    }
    Ok(())
}

/// The mock notification. `userIds` is the server-side routing field the module must strip.
fn notification(topic: &str, title: &str, id: u64) -> String {
    serde_json::json!({
        "topic": topic,
        "title": title,
        "notificationMessageId": format!("MOCK{id}"),
        "type": "info",
        "userIds": ["someone"],
    })
    .to_string()
}

fn read_head(stream: &mut TcpStream) -> std::io::Result<Head> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 2048];
    let split = loop {
        if let Some(at) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            break at + 4;
        }
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "request closed"));
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.len() > 65_536 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "request headers are too large",
            ));
        }
    };
    let header = String::from_utf8_lossy(&buf[..split - 4]).into_owned();
    let rest = buf[split..].to_vec();
    let mut lines = header.split("\r\n");
    let mut parts = lines.next().unwrap_or("").split(' ');
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/");
    if method.is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "no request line"));
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut headers = Vec::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    Ok(Head {
        method,
        path: percent_decode(path),
        query: query.to_string(),
        headers,
        rest,
    })
}

fn write_response(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn file_under(pkg: &Path, url_path: &str) -> Option<Vec<u8>> {
    let rel = url_path.trim_start_matches('/');
    if rel.is_empty() || rel.split('/').any(|part| part.is_empty() || part == "..") {
        return None;
    }
    let pkg = pkg.canonicalize().ok()?;
    let file = pkg.join(rel).canonicalize().ok()?;
    if file.starts_with(&pkg) && file.is_file() {
        std::fs::read(file).ok()
    } else {
        None
    }
}

fn content_type(path: &str) -> &'static str {
    match Path::new(path).extension().and_then(|ext| ext.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn is_upgrade(head: &Head) -> bool {
    head.headers
        .iter()
        .any(|(name, value)| name.eq_ignore_ascii_case("upgrade") && value.eq_ignore_ascii_case("websocket"))
}

fn header_value<'a>(head: &'a Head, name: &str) -> Option<&'a str> {
    head.headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn query_get(query: &str, name: &str) -> Option<String> {
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if percent_decode(key) == name {
            return Some(percent_decode(value));
        }
    }
    None
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&text[index + 1..index + 3], 16) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn timed_out(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    )
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// First load restores 0 OPFS rows, the second load in the same profile restores 1. A second tab that also gets
/// `SMOKE OK` is a warning: there is no lock between tabs.
#[test]
fn chrome_smoke_restores_one_opfs_row_on_reload() {
    let Some((pkg, binary)) = prepared() else {
        return;
    };
    let server = serve(&pkg).expect("smoke server");
    let url = format!("http://127.0.0.1:{}/smoke.html", server.addr.port());
    let mut browser = launch(binary).expect("chromium");
    let (first_result, first_log) = run_page(&mut browser, &url).expect("first load");
    println!("{first_log}{first_result}");
    assert!(
        first_log.contains("jspi true")
            && first_log.contains("restored 0 row(s) from OPFS")
            && first_result == "SMOKE OK",
        "first load was {first_result}:\n{first_log}"
    );
    let (second_result, second_log) = run_page(&mut browser, &url).expect("second load");
    println!("{second_log}{second_result}");
    assert!(
        second_result == "SMOKE OK" && second_log.contains("restored 1 row(s) from OPFS"),
        "second load was {second_result}:\n{second_log}"
    );
    browser.open_tab().expect("second tab");
    match run_page(&mut browser, &url) {
        Ok((result, log)) => {
            println!("[second tab] {log}{result}");
            if result == "SMOKE OK" {
                eprintln!(
                    "WARN a second tab opened and wrote the database the first tab has open: there is no lock between tabs"
                );
            } else {
                eprintln!("second tab: {result}");
            }
        }
        Err(err) => panic!("second tab: {err}"),
    }
    println!("CHROME SMOKE OK (JSPI fetch, rollback, cancel, OPFS durable across reload)");
}

/// `/notws`: subscribe pushes a notification and a binary frame, echo and binary come back, `from-module` is acked,
/// and `drop` closes the TCP connection with no close frame.
#[test]
fn websocket_smoke_speaks_notws() {
    let Some((pkg, binary)) = prepared() else {
        return;
    };
    let server = serve(&pkg).expect("smoke server");
    let url = format!("http://127.0.0.1:{}/ws-smoke.html", server.addr.port());
    let mut browser = launch(binary).expect("chromium");
    let (result, log) = run_page(&mut browser, &url).expect("ws page");
    println!("{log}");
    let received = lock(&server.received).clone();
    let connections = server.connections.load(Ordering::Relaxed);
    println!("mock: {connections} connection(s), received {received:?}");
    assert!(
        result == "WS SMOKE OK"
            && connections >= 2
            && received.iter().any(|line| line == "from-module")
            && received.iter().any(|line| line == "binary:4"),
        "WS SMOKE FAILED ({result}); connections {connections}; received {received:?}\n{log}"
    );
    println!("WS SMOKE OK");
}
