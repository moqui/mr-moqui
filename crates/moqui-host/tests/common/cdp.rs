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

//! A small Chrome DevTools Protocol client for the Gate 3 browser smoke: one headless Chromium with a temporary profile, a
//! page attached with a flat session (more in windows of their own, [`Browser::open_tab`], one of them the current at a time),
//! `Runtime.evaluate`, and the console errors, uncaught exceptions and log entries the pages report.
//! It is not `PdfRenderer` and does not use `chromiumoxide`.

use std::net::TcpStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket, connect};

use super::{DirGuard, temp_dir};

/// The browser of the smoke: `pdf-chromium-path` when it is set, else the Chromium of `./moq chromium-download` in
/// `runtime/chromium`, else `chromium`, `chromium-browser` or `google-chrome` on the `PATH`. A missing binary fails
/// the smoke.
pub fn chromium_binary() -> PathBuf {
    for name in ["pdf-chromium-path", "pdf_chromium_path"] {
        let configured = moqui_lib::env::get_prop_or_env(name);
        if !configured.is_empty() {
            let path = PathBuf::from(&configured);
            assert!(path.is_file(), "{name} names {configured}, which is not a file");
            return path;
        }
    }
    if let Some(found) = super::downloaded_chromium() {
        return found;
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    for name in ["chromium", "chromium-browser", "google-chrome"] {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    panic!(
        "no Chromium for the Gate 3 browser smoke: set pdf-chromium-path, run ./moq chromium-download, or put chromium, \
         chromium-browser or google-chrome on the PATH"
    );
}

/// What the page said while it ran: console errors and warnings, uncaught exceptions, and log entries.
#[derive(Clone, Debug)]
pub struct Problem {
    pub kind: String,
    pub text: String,
}

pub struct Browser {
    child: Child,
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
    session: String,
    /// The first page's session once another was opened.
    first: String,
    next_id: u64,
    events: Vec<Value>,
    problems: Vec<Problem>,
    _profile: DirGuard,
}

impl Browser {
    pub fn launch() -> Browser {
        let binary = chromium_binary();
        let profile = temp_dir("moqui-gate3-chromium");
        std::fs::create_dir_all(&profile).unwrap();
        let profile_guard = DirGuard(profile.clone());
        let child = Command::new(&binary)
            .args([
                "--headless=new",
                "--remote-debugging-address=127.0.0.1",
                "--remote-debugging-port=0",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-gpu",
                "--disable-extensions",
                "--disable-background-networking",
                "--disable-component-update",
                "--disable-sync",
                "--metrics-recording-only",
                "--password-store=basic",
                "--use-mock-keychain",
                "--window-size=1280,900",
            ])
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap_or_else(|err| panic!("could not start {}: {err}", binary.display()));
        let mut browser_child = ChildGuard(Some(child));
        let url = devtools_url(&profile, browser_child.0.as_mut().unwrap());
        let (ws, _) = connect(url.as_str()).unwrap_or_else(|err| panic!("devtools socket {url}: {err}"));
        let mut browser = Browser {
            child: browser_child.0.take().unwrap(),
            ws,
            session: String::new(),
            first: String::new(),
            next_id: 0,
            events: Vec::new(),
            problems: Vec::new(),
            _profile: profile_guard,
        };
        browser.set_timeout(Duration::from_millis(200));
        browser.session = browser.attach_page(false);
        browser
    }

    /// A new page attached with its own session: in a window of its own when `window`, so each is visible (a page in a
    /// background tab is hidden).
    fn attach_page(&mut self, window: bool) -> String {
        let target = self.call_browser(
            "Target.createTarget",
            json!({"url": "about:blank", "newWindow": window}),
        );
        let target_id = target["targetId"].as_str().unwrap().to_string();
        let attached = self.call_browser("Target.attachToTarget", json!({"targetId": target_id, "flatten": true}));
        let session = attached["sessionId"].as_str().unwrap().to_string();
        for domain in ["Runtime.enable", "Log.enable", "Page.enable"] {
            self.send(domain, json!({}), Some(&session));
        }
        session
    }

    /// Open another page (a second tab of the same profile, so the same sign-in, in a window of its own) and make it the
    /// current one. The answer names it for [`Browser::use_tab`]; the first page is [`Browser::first_tab`].
    pub fn open_tab(&mut self) -> String {
        let first = self.session.clone();
        if self.first.is_empty() {
            self.first = first;
        }
        self.session = self.attach_page(true);
        self.session.clone()
    }

    /// The page [`Browser::launch`] opened.
    pub fn first_tab(&self) -> String {
        if self.first.is_empty() {
            self.session.clone()
        } else {
            self.first.clone()
        }
    }

    /// Make `tab` the page that the commands, the waits and the clicks go to.
    pub fn use_tab(&mut self, tab: &str) {
        self.session = tab.to_string();
    }

    fn set_timeout(&mut self, timeout: Duration) {
        if let MaybeTlsStream::Plain(stream) = self.ws.get_mut() {
            stream.set_read_timeout(Some(timeout)).unwrap();
        }
    }

    fn call_browser(&mut self, method: &str, params: Value) -> Value {
        self.send(method, params, None)
    }

    /// A command to the page.
    pub fn call(&mut self, method: &str, params: Value) -> Value {
        let session = self.session.clone();
        self.send(method, params, Some(&session))
    }

    fn send(&mut self, method: &str, params: Value, session: Option<&str>) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let mut message = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            message["sessionId"] = Value::String(session.to_string());
        }
        self.ws.send(Message::Text(message.to_string().into())).unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            assert!(Instant::now() < deadline, "no answer to {method}");
            let Some(message) = self.read() else { continue };
            if message["id"].as_u64() == Some(id) {
                if let Some(error) = message.get("error") {
                    panic!("{method}: {error}");
                }
                return message["result"].clone();
            }
        }
    }

    /// One message, or `None` when nothing came within the read timeout. Events are kept.
    fn read(&mut self) -> Option<Value> {
        match self.ws.read() {
            Ok(Message::Text(text)) => {
                let message: Value = serde_json::from_str(&text).unwrap();
                if message.get("method").is_some() {
                    self.record(&message);
                }
                Some(message)
            }
            Ok(_) => None,
            Err(tungstenite::Error::Io(err))
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                None
            }
            Err(err) => panic!("devtools socket: {err}"),
        }
    }

    fn record(&mut self, event: &Value) {
        let params = &event["params"];
        match event["method"].as_str().unwrap_or("") {
            "Runtime.exceptionThrown" => {
                let details = &params["exceptionDetails"];
                let text = details["exception"]["description"]
                    .as_str()
                    .or_else(|| details["text"].as_str())
                    .unwrap_or("")
                    .to_string();
                self.problems.push(Problem {
                    kind: "exception".to_string(),
                    text: format!("{text} at {}:{}", details["url"], details["lineNumber"]),
                });
            }
            "Runtime.consoleAPICalled" => {
                let kind = params["type"].as_str().unwrap_or("");
                if matches!(kind, "error" | "assert" | "warning" | "warn") {
                    let text = params["args"]
                        .as_array()
                        .map(|args| {
                            args.iter()
                                .map(|arg| {
                                    arg["value"]
                                        .as_str()
                                        .map(str::to_string)
                                        .or_else(|| arg["description"].as_str().map(str::to_string))
                                        .unwrap_or_else(|| arg["value"].to_string())
                                })
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                        .unwrap_or_default();
                    self.problems.push(Problem {
                        kind: format!("console.{kind}"),
                        text,
                    });
                }
            }
            "Log.entryAdded" => {
                let entry = &params["entry"];
                let level = entry["level"].as_str().unwrap_or("");
                if matches!(level, "error" | "warning") {
                    self.problems.push(Problem {
                        kind: format!("log.{level}"),
                        text: format!(
                            "{} {}",
                            entry["text"].as_str().unwrap_or(""),
                            entry["url"].as_str().unwrap_or("")
                        ),
                    });
                }
            }
            _ => {}
        }
        self.events.push(event.clone());
    }

    /// Read events for `duration`.
    pub fn pump(&mut self, duration: Duration) {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            self.read();
        }
    }

    /// Go to `url` and wait for its load event.
    pub fn navigate(&mut self, url: &str) {
        self.events.clear();
        let result = self.call("Page.navigate", json!({"url": url}));
        if let Some(error) = result.get("errorText") {
            panic!("navigate {url}: {error}");
        }
        self.wait_event("Page.loadEventFired", Duration::from_secs(60));
    }

    /// Load the current page again and wait for its load event.
    pub fn reload(&mut self) {
        self.events.clear();
        self.call("Page.reload", json!({}));
        self.wait_event("Page.loadEventFired", Duration::from_secs(60));
    }

    /// Wait for an event of the current page named `method` that came after the last [`Browser::navigate`] or
    /// [`Browser::clear_events`].
    pub fn wait_event(&mut self, method: &str, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            if self.events.iter().any(|event| {
                event["method"] == method
                    && event["sessionId"]
                        .as_str()
                        .is_none_or(|session| session == self.session)
            }) {
                return;
            }
            assert!(Instant::now() < deadline, "no {method} within {timeout:?}");
            self.read();
        }
    }

    pub fn clear_events(&mut self) {
        self.events.clear();
    }

    /// The value of a JavaScript expression in the page, after its promise settles. An exception is an `Err` with its text.
    pub fn try_eval(&mut self, expression: &str) -> Result<Value, String> {
        let result = self.call(
            "Runtime.evaluate",
            json!({"expression": expression, "returnByValue": true, "awaitPromise": true, "userGesture": true}),
        );
        if let Some(details) = result.get("exceptionDetails") {
            let text = details["exception"]["description"]
                .as_str()
                .or_else(|| details["text"].as_str())
                .unwrap_or("exception")
                .to_string();
            return Err(text);
        }
        Ok(result["result"]["value"].clone())
    }

    pub fn eval(&mut self, expression: &str) -> Value {
        self.try_eval(expression)
            .unwrap_or_else(|err| panic!("{expression}: {err}"))
    }

    /// Poll `expression` until it is truthy. The failure names `what` and the last value.
    pub fn wait_for(&mut self, what: &str, expression: &str, timeout: Duration) -> Value {
        let deadline = Instant::now() + timeout;
        let mut last;
        loop {
            match self.try_eval(expression) {
                Ok(value) if truthy(&value) => return value,
                Ok(value) => last = value,
                Err(err) => last = Value::String(err),
            }
            if Instant::now() >= deadline {
                let text = self.eval("document.body ? document.body.innerText.slice(0, 3000) : ''");
                panic!(
                    "{what}: not true within {timeout:?} (last {last}); problems {:?}\npage text: {text}",
                    self.problems
                );
            }
            self.pump(Duration::from_millis(100));
        }
    }

    /// A mouse click at the middle of the element `element` (an expression), after it is scrolled into view, as a user's click:
    /// Quasar opens menus and popups on pointer events that `element.click()` does not send.
    pub fn click(&mut self, what: &str, element: &str) {
        let rect = self.wait_for(
            what,
            &format!(
                "(() => {{ const el = ({element}); if (!el) return null; el.scrollIntoView({{block: 'center'}}); const r = el.getBoundingClientRect(); \
                 return r.width > 0 && r.height > 0 ? [r.left + r.width / 2, r.top + r.height / 2] : null; }})()"
            ),
            Duration::from_secs(20),
        );
        let (x, y) = (rect[0].as_f64().unwrap(), rect[1].as_f64().unwrap());
        for kind in ["mouseMoved", "mousePressed", "mouseReleased"] {
            self.call(
                "Input.dispatchMouseEvent",
                json!({"type": kind, "x": x, "y": y, "button": "left", "buttons": if kind == "mousePressed" { 1 } else { 0 }, "clickCount": 1}),
            );
        }
        self.pump(Duration::from_millis(150));
    }

    /// Type text into the focused element.
    pub fn type_text(&mut self, text: &str) {
        self.call("Input.insertText", json!({"text": text}));
        self.pump(Duration::from_millis(100));
    }

    /// Press a key such as `Enter` or `Escape` on the focused element.
    pub fn press(&mut self, key: &str) {
        for kind in ["keyDown", "keyUp"] {
            self.call(
                "Input.dispatchKeyEvent",
                json!({"type": kind, "key": key, "code": key, "windowsVirtualKeyCode": match key { "Enter" => 13, "Escape" => 27, _ => 0 }}),
            );
        }
        self.pump(Duration::from_millis(150));
    }

    /// The problems reported since the last call, and forget them.
    pub fn take_problems(&mut self) -> Vec<Problem> {
        self.pump(Duration::from_millis(300));
        std::mem::take(&mut self.problems)
    }
}

impl Drop for Browser {
    /// Ask the browser to close, so it stops its helper processes, then kill what is left of its process group and remove the profile,
    /// which a helper may still be writing to for a moment.
    fn drop(&mut self) {
        self.next_id += 1;
        let close = json!({"id": self.next_id, "method": "Browser.close", "params": {}});
        let _ = self.ws.send(Message::Text(close.to_string().into()));
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && matches!(self.child.try_wait(), Ok(None)) {
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{}", self.child.id())])
            .stderr(Stdio::null())
            .status();
        let _ = self.child.kill();
        let _ = self.child.wait();
        for _ in 0..20 {
            if std::fs::remove_dir_all(&self._profile.0).is_ok() || !self._profile.0.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// Kills the browser when the launch stops before the `Browser` owns it.
struct ChildGuard(Option<Child>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = Command::new("kill")
                .args(["-KILL", &format!("-{}", child.id())])
                .stderr(Stdio::null())
                .status();
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The browser socket from `DevToolsActivePort` in the profile, which Chromium writes once it listens.
fn devtools_url(profile: &Path, child: &mut Child) -> String {
    let file = profile.join("DevToolsActivePort");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(text) = std::fs::read_to_string(&file) {
            let mut lines = text.lines();
            if let (Some(port), Some(path)) = (lines.next(), lines.next()) {
                return format!("ws://127.0.0.1:{}{}", port.trim(), path.trim());
            }
        }
        if let Ok(Some(status)) = child.try_wait() {
            panic!("chromium exited before it listened: {status}");
        }
        assert!(Instant::now() < deadline, "chromium did not write {}", file.display());
        std::thread::sleep(Duration::from_millis(50));
    }
}
