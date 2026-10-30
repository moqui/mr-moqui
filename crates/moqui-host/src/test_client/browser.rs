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

//! The `browser` of a client spec (plans/archive/ClientTestHarness.md 5.5): headless Chromium over [`cdp`], one per spec file,
//! started on the first `newPage` and stopped after the spec, with a browser context per page so each has its own
//! cookies; a user signs in through `/Login` once a spec, and the user's later pages are given those cookies. Pages
//! navigate and settle; locators wait for their element to be attached, visible, enabled and stable before they act
//! through input events. Each page's problems (uncaught exceptions, console errors, responses of 400 and up, failed
//! loads, render-error text and the client's error notifications) fail the case they happen in unless allowed; on a
//! failure each open page leaves a screenshot and its text in the output directory.
//!
//! The page side is `locate.js`, evaluated in each new document before the page's own scripts.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine as _;
use moqui_lib::sync::lock;
use serde_json::{json, Value};

use super::cdp::{self, truthy};
use super::http::{magic, Server};
use super::load::file_part;
use super::video;
use super::{field, text_field};

const LOCATE: &str = include_str!("locate.js");
const ASSERT_JS: &str = include_str!("../../../moqui-core/src/testkit/assert.js");
const PAGE_HARNESS: &str = include_str!("page_harness.js");

/// The viewport of a page that names none.
const VIEWPORT: (u32, u32) = (1440, 1000);

/// The settle's quiet time: the page is idle this long.
const IDLE: Duration = Duration::from_millis(100);

/// The text after which a page is not settled.
const RENDER_ERROR: &str = r"Error rendering[^\n]*|Exception in render[^\n]*";

/// What the browser of a run needs from its options.
pub(crate) struct BrowserConfig {
    pub(crate) chromium: Option<String>,
    pub(crate) headed: bool,
    pub(crate) insecure: bool,
    pub(crate) runtime: Option<PathBuf>,
}

/// One open page.
struct Page {
    session: String,
    /// The browser context of its own.
    context: Option<String>,
    /// The feature it was opened in, closed after it; `None` for one of `setupSpec`, closed after the spec.
    feature: Option<String>,
    timeout: Duration,
    warnings: bool,
    allow: Vec<regex::Regex>,
    /// The problems so far, as the report shows them.
    problems: Vec<String>,
    /// Render errors already reported, by URL and line.
    reported: HashSet<String>,
    /// The user and password whose kept sign-in it was given, until its first `goto` shows the session still works.
    reused: Option<(String, String)>,
}

/// The spec's browser and its pages.
pub(crate) struct SpecBrowser {
    driver: cdp::Browser,
    pages: BTreeMap<u64, Page>,
    /// The cookies of each user's first sign-in (by user and password), given to the user's later pages so they share
    /// its server session instead of signing in through `/Login` each time.
    signed_in: HashMap<(String, String), Vec<Value>>,
    /// `--video`: the spec's recording, which each page is screencast to.
    video: Option<video::Shared>,
}

/// A page call's target: the browser and the page.
struct At<'a> {
    driver: &'a mut cdp::Browser,
    page: &'a mut Page,
}

/// A JavaScript regular expression `{ re, flags }` as a Rust one: the flags `i`, `m` and `s` carry over.
fn regex_of(source: &str, flags: &str) -> Result<regex::Regex, String> {
    let mut prefix = String::new();
    for flag in flags.chars().filter(|flag| matches!(flag, 'i' | 'm' | 's')) {
        prefix.push(flag);
    }
    let pattern = if prefix.is_empty() {
        source.to_string()
    } else {
        format!("(?{prefix}){source}")
    };
    regex::Regex::new(&pattern).map_err(|err| format!("allowProblems: /{source}/{flags} is not a pattern here: {err}"))
}

fn page_load_failure(message: impl Into<String>) -> Value {
    json!({
        "cases": [{
            "name": "in-page",
            "status": "failed",
            "kind": "error",
            "message": message.into(),
            "ms": 0
        }]
    })
}

fn page_report_json(report: &cdp::PageReport) -> Value {
    let mut status = if report.event == "pass" { "passed" } else { "failed" };
    let mut kind = report.kind.clone();
    let mut message = report.message.clone();
    let mut output = String::new();
    if status == "passed" && !report.problems.is_empty() {
        if report.allow_problems {
            output = report.problems.join("\n");
        } else {
            status = "failed";
            kind = "problems".to_string();
            message = format!("the test left {} page problem(s)", report.problems.len());
        }
    }
    json!({
        "name": report.name,
        "status": status,
        "kind": kind,
        "message": message,
        "stack": report.stack,
        "ms": report.ms,
        "problems": if status == "passed" { Vec::<String>::new() } else { report.problems.clone() },
        "output": output,
    })
}

/// A value as JavaScript source.
fn js(value: &Value) -> String {
    value.to_string()
}

/// A text as a JavaScript string.
fn js_text(text: &str) -> String {
    Value::String(text.to_string()).to_string()
}

/// The expression of a page function `source` (a function's text when `function`) called with `arg`.
fn call_expression(source: &str, function: bool, arg: &Value) -> String {
    if function {
        format!("({source})({})", js(arg))
    } else {
        source.to_string()
    }
}

impl At<'_> {
    fn eval(&mut self, expression: &str) -> Result<Value, String> {
        self.driver.use_tab(&self.page.session);
        self.driver.try_eval(expression)
    }

    /// Install `locate.js` in a document that did not get it (one that loaded before the page was set up).
    fn ensure_locate(&mut self) -> Result<(), String> {
        if !truthy(&self.eval("!!window.__moquiTest")?) {
            self.eval(LOCATE)?;
        }
        Ok(())
    }

    /// Move the page's problems from the driver into its list: warnings only when asked, and none that `allow`
    /// matches.
    fn gather(&mut self) {
        for problem in self.driver.take_problems_of(&self.page.session) {
            if problem.is_warning() && !self.page.warnings {
                continue;
            }
            let text = format!("{}: {}", problem.kind, problem.text);
            self.add_problem(text);
        }
    }

    fn add_problem(&mut self, text: String) {
        if !self.page.allow.iter().any(|allow| allow.is_match(&text)) {
            self.page.problems.push(text);
        }
    }

    /// Wait until the Moqui client is idle for [`IDLE`] (no load pending, no loading bar, no fetch in flight), for up
    /// to `timeout`; then take the client's error notifications and any render-error text as problems. Whether it
    /// settled.
    fn settle(&mut self, timeout: Duration) -> Result<bool, String> {
        let deadline = Instant::now() + timeout;
        let mut idle_since: Option<Instant> = None;
        let mut injected = false;
        let settled = loop {
            let state = self.eval(
                "window.__moquiTest ? window.__moquiTest.busy() : (document.readyState === 'complete' ? 'none' : 'document')",
            );
            let now = Instant::now();
            match state.as_ref().ok().and_then(Value::as_str) {
                Some("") => {
                    if now.duration_since(*idle_since.get_or_insert(now)) >= IDLE {
                        break true;
                    }
                }
                Some("none") if !injected => {
                    injected = true;
                    self.ensure_locate()?;
                    idle_since = None;
                }
                _ => idle_since = None,
            }
            if now >= deadline {
                break false;
            }
            self.driver.pump(Duration::from_millis(25))?;
        };
        self.check_page()?;
        Ok(settled)
    }

    /// The client's error notifications and a render error in the text, as problems.
    fn check_page(&mut self) -> Result<(), String> {
        let found = self.eval(&format!(
            "(() => {{ const T = window.__moquiTest; const errors = T && T.takeErrors ? T.takeErrors() : []; \
             const text = document.body ? document.body.innerText : ''; const m = new RegExp({}).exec(text); \
             return {{ errors, render: m ? m[0].slice(0, 300) : null, url: location.href }}; }})()",
            js_text(RENDER_ERROR)
        ));
        let Ok(found) = found else {
            // a navigation under way: the next settle looks
            return Ok(());
        };
        let url = text_field(&found, "url");
        for error in found["errors"].as_array().into_iter().flatten() {
            let text = error.as_str().unwrap_or("").to_string();
            self.add_problem(format!("notification: {text} at {url}"));
        }
        if let Some(line) = found["render"].as_str() {
            if self.page.reported.insert(format!("{url} {line}")) {
                self.add_problem(format!("render error: {line} at {url}"));
            }
        }
        self.gather();
        Ok(())
    }

    fn page_text(&mut self) -> String {
        self.eval("document.body ? document.body.innerText : ''")
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default()
    }

    /// The end of a wait that timed out: what waited, why, the problems and the start of the page text.
    fn timed_out(&mut self, what: &str, why: &str, timeout: Duration) -> String {
        self.gather();
        let text: String = self.page_text().chars().take(1500).collect();
        let url = self
            .eval("location.href")
            .ok()
            .and_then(|url| url.as_str().map(str::to_string))
            .unwrap_or_default();
        let mut out = format!("{what}: {why} within {}s at {url}", timeout.as_secs_f64());
        if !self.page.problems.is_empty() {
            out.push_str(&format!("\nproblems: {}", self.page.problems.join("; ")));
        }
        out.push_str(&format!("\npage text: {text}"));
        out
    }

    fn goto(&mut self, server: &Server, args: &Value) -> Result<Value, String> {
        let path = text_field(args, "path");
        let url = if self.driver.is_offline() {
            super::offline::url(&path)
        } else {
            server.join(&path)
        };
        self.open_url(&url)
    }

    fn open_url(&mut self, url: &str) -> Result<Value, String> {
        self.driver.use_tab(&self.page.session);
        let timeout = self.page.timeout.max(Duration::from_secs(30));
        let loader = self.driver.navigate_within(url, timeout)?;
        let status = self.driver.document_status(&loader);
        self.settle(self.page.timeout)?;
        let now = self.eval("location.href").unwrap_or(Value::Null);
        Ok(json!({ "status": status.map(|(status, _)| status), "url": now }))
    }

    /// `page.setContent(html)`: navigate to the offline origin and fulfill that document with `html`.
    fn set_content(&mut self, html: &str) -> Result<Value, String> {
        if !self.driver.is_offline() {
            return Err("setContent is for a spec with server: false".to_string());
        }
        self.driver.set_offline_content(html);
        self.open_url(&super::offline::url("/"))
    }

    /// `page.addScript({ path } | { content })`. A path loads on the page's origin and the call waits for it.
    fn add_script(&mut self, args: &Value) -> Result<Value, String> {
        self.add_asset(args, true)
    }

    /// `page.addStyle({ path } | { content })`.
    fn add_style(&mut self, args: &Value) -> Result<Value, String> {
        self.add_asset(args, false)
    }

    fn add_asset(&mut self, args: &Value, script: bool) -> Result<Value, String> {
        self.driver.use_tab(&self.page.session);
        let expression = if let Some(content) = field(args, "content").as_str() {
            if script {
                format!(
                    "(() => {{ const s = document.createElement('script'); s.text = {}; (document.head || document.documentElement).appendChild(s); return true; }})()",
                    js_text(content)
                )
            } else {
                format!(
                    "(() => {{ const s = document.createElement('style'); s.textContent = {}; (document.head || document.documentElement).appendChild(s); return true; }})()",
                    js_text(content)
                )
            }
        } else {
            let path = text_field(args, "path");
            if path.is_empty() {
                return Err(if script {
                    "addScript needs path or content".to_string()
                } else {
                    "addStyle needs path or content".to_string()
                });
            }
            let src = if self.driver.is_offline() {
                super::offline::url(&path)
            } else if path.starts_with('/') {
                path
            } else {
                format!("/{path}")
            };
            if script {
                format!(
                    "(() => {{ const s = document.createElement('script'); s.async = false; s.src = {}; \
                     const done = new Promise((resolve) => {{ s.onload = () => resolve(true); s.onerror = () => resolve(false); }}); \
                     (document.head || document.documentElement).appendChild(s); return done; }})()",
                    js_text(&src)
                )
            } else {
                format!(
                    "(() => {{ const s = document.createElement('link'); s.rel = 'stylesheet'; s.href = {}; \
                     const done = new Promise((resolve) => {{ s.onload = () => resolve(true); s.onerror = () => resolve(false); }}); \
                     (document.head || document.documentElement).appendChild(s); return done; }})()",
                    js_text(&src)
                )
            }
        };
        let loaded = self.driver.try_eval(&expression)?;
        if loaded == Value::Bool(false) {
            return Err(if script {
                "addScript: the script did not load".to_string()
            } else {
                "addStyle: the stylesheet did not load".to_string()
            });
        }
        Ok(Value::Bool(true))
    }

    /// Sign in through the form at `/Login`, as a user does; an error when the page stays there.
    fn login(&mut self, server: &Server, user: &str, password: &str) -> Result<(), String> {
        if self.driver.is_offline() {
            return Err("login needs a server; this spec has server: false".to_string());
        }
        self.goto(server, &json!({ "path": "/Login" }))?;
        self.driver.clear_events();
        let submitted = self.eval(&format!(
            "(() => {{ const form = document.getElementById('login_form'); if (!form) return false; form.username.value = {}; \
             form.password.value = {}; form.requestSubmit(); return true; }})()",
            js_text(user),
            js_text(password)
        ))?;
        if !truthy(&submitted) {
            return Err(format!("login of {user}: /Login has no login_form"));
        }
        let timeout = self.page.timeout.max(Duration::from_secs(30));
        self.driver.wait_event("Page.loadEventFired", timeout)?;
        self.settle(self.page.timeout)?;
        let path = self.eval("location.pathname")?;
        if path.as_str().is_some_and(|path| path.starts_with("/Login")) {
            let text: String = self.page_text().chars().take(400).collect();
            return Err(format!("login of {user} failed: the page is still /Login: {text}"));
        }
        Ok(())
    }

    /// The cookies of the page's URL, as `Network.setCookies` takes them.
    fn cookies(&mut self) -> Result<Vec<Value>, String> {
        self.driver.use_tab(&self.page.session);
        let found = self.driver.call("Network.getCookies", json!({}))?;
        let mut cookies = Vec::new();
        for cookie in found["cookies"].as_array().into_iter().flatten() {
            let mut param = json!({});
            for name in [
                "name", "value", "domain", "path", "secure", "httpOnly", "sameSite", "priority",
            ] {
                if let Some(value) = cookie.get(name) {
                    param[name] = value.clone();
                }
            }
            if cookie["session"] != true {
                if let Some(expires) = cookie.get("expires") {
                    param["expires"] = expires.clone();
                }
            }
            cookies.push(param);
        }
        Ok(cookies)
    }

    fn set_cookies(&mut self, cookies: Vec<Value>) -> Result<(), String> {
        self.driver.use_tab(&self.page.session);
        self.driver.call("Network.setCookies", json!({ "cookies": cookies }))?;
        Ok(())
    }

    /// Wait for the first element of `chain` to be ready for `action`, then act and settle.
    fn act(&mut self, args: &Value) -> Result<Value, String> {
        let chain = js(field(args, "chain"));
        let what = text_field(args, "desc");
        let action = text_field(args, "action");
        let timeout = seconds(args, "timeout").unwrap_or(self.page.timeout);
        let deadline = Instant::now() + timeout;
        let mut last_box: Option<Value> = None;
        let state = loop {
            let state = self.eval(&format!("window.__moquiTest.actionable({chain}, true)"));
            let why = match &state {
                Err(err) if err.contains("__moquiTest") => {
                    self.ensure_locate()?;
                    "the page was loading".to_string()
                }
                Err(err) => err.clone(),
                Ok(state) => {
                    let count = state["count"].as_u64().unwrap_or(0);
                    let stable = last_box.as_ref().is_some_and(|last| same_box(last, &state["box"]));
                    last_box = Some(state["box"].clone());
                    if count == 0 {
                        "no element matches".to_string()
                    } else if state["visible"] != true {
                        "the element is not visible".to_string()
                    } else if state["enabled"] != true && action != "hover" && action != "focus" {
                        "the element is not enabled".to_string()
                    } else if action == "fill" && state["editable"] != true {
                        "the element is not editable".to_string()
                    } else if !stable {
                        "the element is moving".to_string()
                    } else if (action == "click" || action == "hover") && state["hit"] != true {
                        format!(
                            "{} would get the click",
                            state["hit"].as_str().unwrap_or("another element")
                        )
                    } else {
                        break state.clone();
                    }
                }
            };
            if Instant::now() >= deadline {
                return Err(self.timed_out(&format!("{action} {what}"), &why, timeout));
            }
            self.driver.pump(Duration::from_millis(50))?;
        };
        let rect = &state["box"];
        let x = rect["x"].as_f64().unwrap_or(0.0) + rect["width"].as_f64().unwrap_or(0.0) / 2.0;
        let y = rect["y"].as_f64().unwrap_or(0.0) + rect["height"].as_f64().unwrap_or(0.0) / 2.0;
        match action.as_str() {
            "click" => self.driver.click_at(x, y)?,
            "hover" => self.driver.hover_at(x, y)?,
            "focus" => {
                self.eval(&format!("window.__moquiTest.focus({chain})"))?;
            }
            "fill" => {
                let value = text_field(args, "value");
                if !truthy(&self.eval(&format!("window.__moquiTest.focusForFill({chain})"))?) {
                    return Err(format!("fill {what}: the element did not take the focus"));
                }
                if value.is_empty() {
                    self.eval(&format!("window.__moquiTest.clear({chain})"))?;
                } else {
                    self.driver.call("Input.insertText", json!({ "text": value }))?;
                }
            }
            "press" => {
                self.eval(&format!("window.__moquiTest.focus({chain})"))?;
                self.driver.press_chord(&text_field(args, "key"))?;
            }
            other => return Err(format!("{other} is not a locator action")),
        }
        self.driver.pump(Duration::from_millis(50))?;
        self.settle(self.page.timeout)?;
        Ok(Value::Null)
    }

    /// `locator.waitFor({ state })`: visible (the default), hidden, attached or detached.
    fn wait_locator(&mut self, args: &Value) -> Result<Value, String> {
        let chain = js(field(args, "chain"));
        let state = field(args, "state").as_str().unwrap_or("visible").to_string();
        let timeout = seconds(args, "timeout").unwrap_or(self.page.timeout);
        let deadline = Instant::now() + timeout;
        let expression = match state.as_str() {
            "visible" => format!("window.__moquiTest.read({chain}, 'visible')"),
            "hidden" => format!("!window.__moquiTest.read({chain}, 'visible')"),
            "attached" => format!("window.__moquiTest.read({chain}, 'count') > 0"),
            "detached" => format!("window.__moquiTest.read({chain}, 'count') === 0"),
            other => {
                return Err(format!(
                    "waitFor: state is visible, hidden, attached or detached, not {other}"
                ))
            }
        };
        loop {
            match self.eval(&expression) {
                Ok(value) if truthy(&value) => return Ok(Value::Bool(true)),
                Err(err) if err.contains("__moquiTest") => self.ensure_locate()?,
                _ => {}
            }
            if Instant::now() >= deadline {
                let what = format!("waitFor {} {}", text_field(args, "desc"), state);
                return Err(self.timed_out(&what, &format!("not {state}"), timeout));
            }
            self.driver.pump(Duration::from_millis(50))?;
        }
    }

    fn read_locator(&mut self, args: &Value) -> Result<Value, String> {
        let chain = js(field(args, "chain"));
        let what = text_field(args, "what");
        let value = self.eval(&format!(
            "window.__moquiTest.read({chain}, {}, {})",
            js_text(&what),
            js(field(args, "arg"))
        ));
        let value = match value {
            Err(err) if err.contains("__moquiTest") => {
                self.ensure_locate()?;
                self.eval(&format!(
                    "window.__moquiTest.read({chain}, {}, {})",
                    js_text(&what),
                    js(field(args, "arg"))
                ))?
            }
            other => other?,
        };
        if value.get("missing") == Some(&Value::Bool(true)) {
            return Err(format!("{what} of {}: no element matches", text_field(args, "desc")));
        }
        Ok(value)
    }

    fn evaluate_locator(&mut self, args: &Value) -> Result<Value, String> {
        let chain = js(field(args, "chain"));
        let source = text_field(args, "source");
        let expression = format!(
            "(() => {{ const el = window.__moquiTest.element({chain}); if (!el) throw new Error('no element matches'); \
             return ({source})(el, {}); }})()",
            js(field(args, "arg"))
        );
        self.eval(&expression)
            .map_err(|err| format!("evaluate on {}: {err}", text_field(args, "desc")))
    }

    fn wait_page(&mut self, args: &Value) -> Result<Value, String> {
        let expression = call_expression(
            &text_field(args, "source"),
            field(args, "function") == true,
            field(args, "arg"),
        );
        let timeout = seconds(args, "timeout").unwrap_or(self.page.timeout);
        let deadline = Instant::now() + timeout;
        loop {
            let last = match self.eval(&expression) {
                Ok(value) if truthy(&value) => return Ok(value),
                Ok(value) => value,
                Err(err) => Value::String(err),
            };
            if Instant::now() >= deadline {
                let why = format!("not true (the last value was {last})");
                return Err(self.timed_out("waitFor", &why, timeout));
            }
            self.driver.pump(Duration::from_millis(100))?;
        }
    }

    /// A PNG of the page into `path`: the viewport, or the whole page with `full`.
    fn screenshot(&mut self, path: &Path, full: bool) -> Result<(), String> {
        self.driver.use_tab(&self.page.session);
        let mut params = json!({ "format": "png" });
        if full {
            let metrics = self.driver.call("Page.getLayoutMetrics", json!({}))?;
            let size = &metrics["cssContentSize"];
            let width = size["width"].as_f64().unwrap_or(f64::from(VIEWPORT.0)).ceil();
            let height = size["height"]
                .as_f64()
                .unwrap_or(f64::from(VIEWPORT.1))
                .ceil()
                .min(16_000.0);
            params["captureBeyondViewport"] = Value::Bool(true);
            params["clip"] = json!({ "x": 0, "y": 0, "width": width, "height": height, "scale": 1 });
        }
        let shot = self.driver.call("Page.captureScreenshot", params)?;
        write_base64(path, shot["data"].as_str().unwrap_or(""))
    }

    fn pdf(&mut self, path: &Path, landscape: bool) -> Result<Value, String> {
        self.driver.use_tab(&self.page.session);
        let printed = self.driver.call(
            "Page.printToPDF",
            json!({ "printBackground": true, "preferCSSPageSize": true, "landscape": landscape }),
        )?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(printed["data"].as_str().unwrap_or(""))
            .map_err(|err| format!("pdf: {err}"))?;
        std::fs::write(path, &bytes).map_err(|err| format!("{}: {err}", path.display()))?;
        Ok(json!({ "path": path.display().to_string(), "size": bytes.len(), "magic": magic(&bytes) }))
    }

    fn set_viewport(&mut self, width: f64, height: f64) -> Result<(), String> {
        if !(width >= 1.0 && height >= 1.0) {
            return Err(format!("setViewport: {width} × {height} is not a size"));
        }
        self.driver.use_tab(&self.page.session);
        self.driver.call(
            "Emulation.setDeviceMetricsOverride",
            json!({ "width": width.round(), "height": height.round(), "deviceScaleFactor": 1, "mobile": false }),
        )?;
        Ok(())
    }
}

/// The path of a URL, with its query: `/Login?x=1` of `http://host:8080/Login?x=1`.
fn url_path(url: &str) -> &str {
    match url.find("://") {
        Some(at) => url[at + 3..].find('/').map_or("/", |slash| &url[at + 3 + slash..]),
        None => url,
    }
}

fn same_box(a: &Value, b: &Value) -> bool {
    ["x", "y", "width", "height"].iter().all(|key| {
        let (a, b) = (a[key].as_f64(), b[key].as_f64());
        matches!((a, b), (Some(a), Some(b)) if (a - b).abs() < 1.0)
    })
}

fn seconds(args: &Value, name: &str) -> Option<Duration> {
    field(args, name)
        .as_f64()
        .filter(|seconds| *seconds >= 0.0 && seconds.is_finite())
        .map(Duration::from_secs_f64)
}

fn write_base64(path: &Path, data: &str) -> Result<(), String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|err| format!("{}: {err}", path.display()))?;
    std::fs::write(path, bytes).map_err(|err| format!("{}: {err}", path.display()))
}

/// `name` as a file in `dir` with the extension `ext` unless it has one: a plain name, made safe.
fn output_file(dir: &Path, name: &str, ext: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
        return Err(format!("'{name}' is not a plain file name"));
    }
    std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let mut file = file_part(name);
    if !file.to_ascii_lowercase().ends_with(&format!(".{ext}")) {
        file.push('.');
        file.push_str(ext);
    }
    Ok(dir.join(file))
}

/// A file in `dir` named `stem.ext`, or `stem-2.ext` and on when that is taken.
fn fresh_file(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let mut path = dir.join(format!("{stem}.{ext}"));
    let mut number = 2;
    while path.exists() {
        path = dir.join(format!("{stem}-{number}.{ext}"));
        number += 1;
    }
    path
}

impl SpecBrowser {
    /// Start the spec's browser.
    pub(crate) fn launch(config: &BrowserConfig) -> Result<SpecBrowser, String> {
        let binary =
            crate::chromium::find_test_browser(config.runtime.as_deref(), config.chromium.as_deref().unwrap_or(""))?;
        let launch = cdp::Launch {
            binary,
            headed: config.headed,
            window: VIEWPORT,
            profile_prefix: "moqui-test-chromium".to_string(),
            extra_args: if config.insecure {
                vec!["--ignore-certificate-errors".to_string()]
            } else {
                Vec::new()
            },
        };
        let driver = cdp::Browser::start(&launch)?;
        Ok(SpecBrowser {
            driver,
            pages: BTreeMap::new(),
            signed_in: HashMap::new(),
            video: None,
        })
    }

    /// Screencast each page opened from now into `shared`'s recording (`--video`).
    pub(crate) fn record_to(&mut self, shared: video::Shared) {
        self.video = Some(shared);
    }

    /// Move the screencast frames read so far into the recording.
    fn feed_video(&mut self) {
        let frames = self.driver.take_frames();
        if frames.is_empty() {
            return;
        }
        if let Some(shared) = &self.video {
            if let Some(recorder) = lock(shared).as_mut() {
                for frame in frames {
                    recorder.frame(frame);
                }
            }
        }
    }

    /// The end of the spec's recording: the frames still on their way, then each screencast stopped.
    pub(crate) fn stop_video(&mut self) {
        if self.video.is_none() {
            return;
        }
        let _ = self.driver.pump(Duration::from_millis(150));
        self.feed_video();
        for session in self.driver.screencasting() {
            self.driver.stop_screencast(&session);
        }
        self.feed_video();
    }

    /// Later pages fulfill `https://moqui.test` from `roots`. Call this before the first page.
    pub(crate) fn use_offline(&mut self, roots: super::offline::Roots) {
        self.driver.use_offline(roots);
    }

    fn at(&mut self, args: &Value) -> Result<At<'_>, String> {
        let id = field(args, "page").as_u64().unwrap_or(0);
        let page = self.pages.get_mut(&id).ok_or_else(|| format!("page {id} is closed"))?;
        Ok(At {
            driver: &mut self.driver,
            page,
        })
    }

    /// `browser.newPage(opts)`: a page in a context of its own, its viewport and color scheme set, signed in with
    /// `user`.
    pub(crate) fn new_page(
        &mut self,
        id: u64,
        server: &Server,
        args: &Value,
        default_timeout: Duration,
    ) -> Result<(), String> {
        let context = self.driver.new_context()?;
        let session = match self.driver.new_page(Some(&context), true, &[LOCATE]) {
            Ok(session) => session,
            Err(err) => {
                self.driver.close_page("", Some(&context));
                return Err(err);
            }
        };
        if let Some(shared) = &self.video {
            match self.driver.start_screencast(&session) {
                Ok(()) => {
                    if let Some(recorder) = lock(shared).as_mut() {
                        recorder.page_opened(&session);
                    }
                }
                Err(err) => tracing::error!("--video: the screencast of page {id}: {err}"),
            }
        }
        let problems = field(args, "problems");
        let page = Page {
            session,
            context: Some(context),
            feature: field(args, "feature").as_str().map(str::to_string),
            timeout: seconds(args, "timeout").unwrap_or(default_timeout),
            warnings: field(problems, "warnings") == true,
            allow: Vec::new(),
            problems: Vec::new(),
            reported: HashSet::new(),
            reused: None,
        };
        self.pages.insert(id, page);
        let opened = self.set_up(id, server, args);
        if opened.is_err() {
            self.close(id);
        }
        self.feed_video();
        opened
    }

    fn set_up(&mut self, id: u64, server: &Server, args: &Value) -> Result<(), String> {
        let sign_in = field(args, "user").as_str().map(|user| {
            let password = field(args, "password").as_str().unwrap_or("moqui");
            (user.to_string(), password.to_string())
        });
        let kept = sign_in.as_ref().and_then(|key| self.signed_in.get(key).cloned());
        let mut at = self.at(&json!({ "page": id }))?;
        let viewport = field(args, "viewport");
        let width = viewport["width"].as_f64().unwrap_or(f64::from(VIEWPORT.0));
        let height = viewport["height"].as_f64().unwrap_or(f64::from(VIEWPORT.1));
        at.set_viewport(width, height)?;
        if let Some(scheme) = field(args, "colorScheme").as_str() {
            if !matches!(scheme, "light" | "dark" | "no-preference") {
                return Err(format!(
                    "newPage: colorScheme is light, dark or no-preference, not {scheme}"
                ));
            }
            at.driver.call(
                "Emulation.setEmulatedMedia",
                json!({ "features": [{ "name": "prefers-color-scheme", "value": scheme }] }),
            )?;
        }
        let Some((user, password)) = sign_in else {
            return Ok(());
        };
        if let Some(cookies) = kept {
            at.set_cookies(cookies)?;
            at.page.reused = Some((user, password));
            return Ok(());
        }
        at.login(server, &user, &password)?;
        // what the sign-in left is not the feature's
        at.gather();
        at.page.problems.clear();
        let cookies = at.cookies()?;
        self.signed_in.insert((user, password), cookies);
        Ok(())
    }

    /// `page.goto`; on the first of a page given a kept sign-in, when it lands on `/Login` (the shared session ended,
    /// by a sign-out or its expiry), sign in again, keep the new cookies and go again.
    fn goto(&mut self, server: &Server, args: &Value) -> Result<Value, String> {
        let mut at = self.at(args)?;
        let went = at.goto(server, args)?;
        let Some((user, password)) = at.page.reused.take() else {
            return Ok(went);
        };
        let landed = url_path(went["url"].as_str().unwrap_or(""));
        if !landed.starts_with("/Login") || text_field(args, "path").starts_with("/Login") {
            return Ok(went);
        }
        at.login(server, &user, &password)?;
        at.gather();
        at.page.problems.clear();
        let cookies = at.cookies()?;
        let went = at.goto(server, args)?;
        self.signed_in.insert((user, password), cookies);
        Ok(went)
    }

    fn close(&mut self, id: u64) {
        if let Some(page) = self.pages.remove(&id) {
            self.feed_video();
            self.driver.close_page(&page.session, page.context.as_deref());
            if let Some(shared) = &self.video {
                if let Some(recorder) = lock(shared).as_mut() {
                    recorder.page_closed(&page.session);
                }
            }
        }
    }

    /// `browser.runInPage`: a fresh offline page, the scripts in order, then the test file. Each `test()` is one case.
    /// The page is closed before this returns.
    pub(crate) fn run_in_page(
        &mut self,
        id: u64,
        server: &Server,
        args: &Value,
        default_timeout: Duration,
    ) -> Result<Value, String> {
        let timeout = args["timeout"]
            .as_f64()
            .filter(|seconds| *seconds > 0.0)
            .map(Duration::from_secs_f64)
            .unwrap_or(default_timeout)
            .max(Duration::from_secs(1));
        if let Err(err) = self.new_page(id, server, &json!({ "timeout": timeout.as_secs_f64() }), timeout) {
            self.close(id);
            return Ok(page_load_failure(err));
        }
        let result = self.drive_page_file(id, args, timeout);
        self.close(id);
        result
    }

    fn drive_page_file(&mut self, id: u64, args: &Value, timeout: Duration) -> Result<Value, String> {
        let mut at = self.at(&json!({ "page": id }))?;
        if let Err(err) = at.set_content("<!doctype html><html><head></head><body></body></html>") {
            return Ok(page_load_failure(err));
        }
        if let Err(err) = at.driver.call("Runtime.addBinding", json!({ "name": "moquiPageCase" })) {
            return Ok(page_load_failure(err));
        }
        if let Err(err) = at.add_script(&json!({ "content": ASSERT_JS })) {
            return Ok(page_load_failure(err));
        }
        if let Err(err) = at.add_script(&json!({ "content": PAGE_HARNESS })) {
            return Ok(page_load_failure(err));
        }
        for style in args["styles"].as_array().into_iter().flatten() {
            let path = style.as_str().unwrap_or("");
            if let Err(err) = at.add_style(&json!({ "path": path })) {
                return Ok(page_load_failure(format!("{path}: {err}")));
            }
        }
        for script in args["scripts"].as_array().into_iter().flatten() {
            let path = script.as_str().unwrap_or("");
            if let Err(err) = at.add_script(&json!({ "path": path })) {
                return Ok(page_load_failure(format!("{path}: {err}")));
            }
        }
        let test = args["test"].as_str().unwrap_or("");
        if let Err(err) = at.add_script(&json!({ "path": test })) {
            return Ok(page_load_failure(format!("{test}: {err}")));
        }
        let filter = args["filter"].as_str().unwrap_or("");
        let expression = format!(
            "__moquiRunPageTests({})",
            serde_json::to_string(filter).unwrap_or_else(|_| "\"\"".to_string())
        );
        at.driver.clear_page_cases();
        let started = Instant::now();
        let ran = at.driver.try_eval_for(&expression, timeout);
        let (mut reports, hung) = at.driver.take_page_cases();
        if let Err(err) = ran {
            reports.push(cdp::PageReport {
                name: hung.unwrap_or_else(|| "in-page".to_string()),
                event: "fail".to_string(),
                message: err,
                stack: String::new(),
                kind: "error".to_string(),
                ms: started.elapsed().as_secs_f64() * 1000.0,
                allow_problems: false,
                problems: Vec::new(),
            });
        } else if let Some(name) = hung {
            reports.push(cdp::PageReport {
                name,
                event: "fail".to_string(),
                message: "the test did not finish".to_string(),
                stack: String::new(),
                kind: "error".to_string(),
                ms: started.elapsed().as_secs_f64() * 1000.0,
                allow_problems: false,
                problems: Vec::new(),
            });
        }
        let cases: Vec<Value> = reports.iter().map(page_report_json).collect();
        Ok(json!({ "cases": cases }))
    }

    /// One `page.*` or `locator.*` call; the frames it brought go to the recording.
    pub(crate) fn call(&mut self, name: &str, args: &Value, server: &Server, out_dir: &Path) -> Result<Value, String> {
        let answer = self.call_page(name, args, server, out_dir);
        self.feed_video();
        answer
    }

    fn call_page(&mut self, name: &str, args: &Value, server: &Server, out_dir: &Path) -> Result<Value, String> {
        if name == "page.close" {
            let id = field(args, "page").as_u64().unwrap_or(0);
            self.close(id);
            return Ok(Value::Null);
        }
        if name == "page.goto" {
            return self.goto(server, args);
        }
        let mut at = self.at(args)?;
        match name {
            "page.url" => at.eval("location.href"),
            "page.title" => at.eval("document.title"),
            "page.text" => at.eval("document.body ? document.body.innerText : ''"),
            "page.html" => at.eval("document.documentElement ? document.documentElement.outerHTML : ''"),
            "page.evaluate" => {
                let expression = call_expression(
                    &text_field(args, "source"),
                    field(args, "function") == true,
                    field(args, "arg"),
                );
                at.eval(&expression)
            }
            "page.waitFor" => at.wait_page(args),
            "page.settle" => {
                let timeout = seconds(args, "timeout").unwrap_or(at.page.timeout);
                if at.settle(timeout)? {
                    Ok(Value::Bool(true))
                } else {
                    Err(at.timed_out("settle", "the Moqui client did not go idle", timeout))
                }
            }
            "page.press" => {
                at.driver.use_tab(&at.page.session);
                at.driver.press_chord(&text_field(args, "key"))?;
                at.driver.pump(Duration::from_millis(50))?;
                at.settle(at.page.timeout)?;
                Ok(Value::Null)
            }
            "page.type" => {
                at.driver.use_tab(&at.page.session);
                at.driver.type_keys(&text_field(args, "text"))?;
                Ok(Value::Bool(true))
            }
            "page.setContent" => at.set_content(&text_field(args, "html")),
            "page.addScript" => at.add_script(args),
            "page.addStyle" => at.add_style(args),
            "page.screenshot" => {
                let path = output_file(out_dir, &text_field(args, "name"), "png")
                    .map_err(|err| format!("screenshot: {err}"))?;
                at.screenshot(&path, field(args, "fullPage") == true)?;
                Ok(Value::String(path.display().to_string()))
            }
            "page.pdf" => {
                let path =
                    output_file(out_dir, &text_field(args, "name"), "pdf").map_err(|err| format!("pdf: {err}"))?;
                at.pdf(&path, field(args, "landscape") == true)
            }
            "page.problems" => {
                at.gather();
                Ok(json!(at.page.problems))
            }
            "page.takeProblems" => {
                at.gather();
                Ok(json!(std::mem::take(&mut at.page.problems)))
            }
            "page.allowProblems" => {
                let allow = regex_of(&text_field(args, "re"), &text_field(args, "flags"))?;
                at.gather();
                at.page.problems.retain(|problem| !allow.is_match(problem));
                at.page.allow.push(allow);
                Ok(Value::Null)
            }
            "page.setViewport" => {
                let width = field(args, "width").as_f64().unwrap_or(0.0);
                let height = field(args, "height").as_f64().unwrap_or(0.0);
                at.set_viewport(width, height)?;
                Ok(Value::Null)
            }
            "locator.act" => at.act(args),
            "locator.read" => at.read_locator(args),
            "locator.waitFor" => at.wait_locator(args),
            "locator.evaluate" => at.evaluate_locator(args),
            other => Err(format!("{other} is not a call of the browser")),
        }
    }

    /// After a case: each page's problems, prefixed with the page when there are several; on a failure (a block that
    /// failed, or problems that are not allowed), each page's screenshot and text in `out_dir`. The pages the case
    /// opened close.
    pub(crate) fn after_case(
        &mut self,
        failed: bool,
        allow_problems: bool,
        feature: &str,
        out_dir: &Path,
    ) -> (Vec<String>, Vec<String>) {
        let _ = self.driver.pump(Duration::from_millis(100));
        let ids: Vec<u64> = self.pages.keys().copied().collect();
        let several = ids.len() > 1;
        let mut taken: Vec<(u64, Vec<String>)> = Vec::new();
        for id in &ids {
            if let Ok(mut at) = self.at(&json!({ "page": id })) {
                at.gather();
                taken.push((*id, std::mem::take(&mut at.page.problems)));
            }
        }
        let failed = failed || (!allow_problems && taken.iter().any(|(_, list)| !list.is_empty()));
        let mut problems = Vec::new();
        let mut attachments = Vec::new();
        for (number, (id, list)) in taken.into_iter().enumerate() {
            if failed {
                if let Ok(mut at) = self.at(&json!({ "page": id })) {
                    attachments.extend(failure_files(&mut at, feature, number + 1, &list, out_dir));
                }
            }
            for problem in list {
                problems.push(if several {
                    format!("page {}: {problem}", number + 1)
                } else {
                    problem
                });
            }
        }
        let closing: Vec<u64> = self
            .pages
            .iter()
            .filter(|(_, page)| page.feature.is_some())
            .map(|(id, _)| *id)
            .collect();
        for id in closing {
            self.close(id);
        }
        self.feed_video();
        (problems, attachments)
    }
}

/// `failed-<feature>-<n>.png` and `.txt` (the URL, the problems and the text) for page `n`; the paths written. A
/// screenshot that fails is logged, and the text is still written.
fn failure_files(at: &mut At<'_>, feature: &str, number: usize, problems: &[String], out_dir: &Path) -> Vec<String> {
    let mut written = Vec::new();
    if let Err(err) = std::fs::create_dir_all(out_dir) {
        tracing::error!("failure files: {}: {err}", out_dir.display());
        return written;
    }
    let stem = format!("failed-{}-{number}", file_part(feature));
    let shot = fresh_file(out_dir, &stem, "png");
    match at.screenshot(&shot, true) {
        Ok(()) => written.push(shot.display().to_string()),
        Err(err) => tracing::error!("the failure screenshot of page {number} of {feature}: {err}"),
    }
    let url = at
        .eval("location.href")
        .ok()
        .and_then(|url| url.as_str().map(str::to_string))
        .unwrap_or_default();
    let mut text = format!("URL: {url}\n\nProblems:\n");
    if problems.is_empty() {
        text.push_str("(none)\n");
    }
    for problem in problems {
        text.push_str(&format!("- {problem}\n"));
    }
    text.push_str("\nText:\n");
    text.push_str(&at.page_text());
    text.push('\n');
    let path = shot.with_extension("txt");
    match std::fs::write(&path, text) {
        Ok(()) => written.push(path.display().to_string()),
        Err(err) => tracing::error!("{}: {err}", path.display()),
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_js_pattern_carries_its_flags() {
        let re = regex_of("favicon\\.ico", "i").unwrap();
        assert!(re.is_match("response: 404 Not Found http://x/FAVICON.ico"));
        assert!(regex_of("(", "").is_err());
    }

    #[test]
    fn output_names_are_plain_and_get_their_extension() {
        let dir = std::env::temp_dir().join(format!("moqui-browser-names-{}", std::process::id()));
        assert_eq!(
            output_file(&dir, "Find Order", "png").unwrap(),
            dir.join("Find-Order.png")
        );
        assert_eq!(output_file(&dir, "order.PDF", "pdf").unwrap(), dir.join("order.PDF"));
        assert!(output_file(&dir, "../x", "png").is_err());
        assert!(output_file(&dir, ".hidden", "png").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
