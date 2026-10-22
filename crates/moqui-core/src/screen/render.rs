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

//! `ScreenRender`: resolve a path, run actions down it, and send a transition response or a screen.
//!
//! The pipeline follows `ScreenRenderImpl.internalRender`. For a screen: `always-actions` for
//! every screen in the path from index 0, `pre-actions` from `renderPathDifference`, then
//! [`render_body`], which slice 1 fills with the root-section actions and an empty body. For a
//! transition: `always-actions`, the transition's actions, and the first response that holds.
//!
//! Output goes to the thread's web slot. A request from the host already installed one.
//! [`make_render`] installs its own from a parameter map, a session map, and a base URL, so it
//! needs no socket.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use moqui_lib::codec::constant_time_eq;
use moqui_lib::object::Value;

use super::cache::{canonical_render_mode, OutputKind, ScreenFacade};
use super::definition::{
    Actions, Authentication, Condition, ParameterDef, ResponseDef, ResponseKind, ScreenDef, TransitionDef,
    CONDITION_FIELD,
};
use super::dispatch;
use super::gate::{self, HeldWrite, RenderOptions, RenderOrigin, TransitionGate, TransitionOutcome};
use super::url::{encode_path, parse_last_standalone, resolve_screen_path, ScreenUrlInfo};
use crate::context::{Error, ErrorKind, ExecutionContext, ExecutionContextFactory};
use crate::script::{self, ContextFrame};
use crate::user;
use crate::web::{self, WebRequest};

const DEFAULT_ENCODING: &str = "UTF-8";
const DEFAULT_MODE: &str = "html";
/// The conf webapp a screen request belongs to.
const WEBAPP: &str = "webroot";

/// The result of a [`ScreenRender`]: what the host would send.
#[derive(Clone, Debug)]
pub struct RenderOutput {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl RenderOutput {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn content_type(&self) -> Option<&str> {
        self.header("Content-Type")
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// A screen render built with [`make_render`].
pub struct ScreenRender {
    factory: Arc<ExecutionContextFactory>,
    root: String,
    path: String,
    method: String,
    parameters: BTreeMap<String, Value>,
    session: BTreeMap<String, Value>,
    base_url: String,
    render_mode: Option<String>,
    /// Request headers the render sees, such as `X-Moqui-Client-Modes`.
    headers: Vec<(String, String)>,
    history: bool,
    options: RenderOptions,
}

/// A render that needs no socket. Set the root screen and path, then call [`ScreenRender::render`].
pub fn make_render(factory: &Arc<ExecutionContextFactory>) -> ScreenRender {
    ScreenRender {
        factory: Arc::clone(factory),
        root: super::ROOT_SCREEN.to_string(),
        path: String::new(),
        method: "GET".to_string(),
        parameters: BTreeMap::new(),
        session: BTreeMap::new(),
        base_url: String::new(),
        render_mode: None,
        headers: Vec::new(),
        history: false,
        options: RenderOptions::default(),
    }
}

impl ScreenRender {
    /// Put the screen on the user's screen history, as a browser request does. A render with no socket otherwise does
    /// not: a job or a test is not a visit. The terminal client asks for it.
    #[must_use]
    pub fn history(mut self, history: bool) -> Self {
        self.history = history;
        self
    }

    /// Location of the root screen. The default is `webroot.xml`.
    #[must_use]
    pub fn root_screen(mut self, location: impl Into<String>) -> Self {
        self.root = location.into();
        self
    }

    /// The screen path under the root, such as `/apps/Foo.csv`. A `?a=b` tail adds parameters.
    #[must_use]
    pub fn screen_path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }

    /// HTTP method a transition sees. The default is `GET`.
    #[must_use]
    pub fn method(mut self, method: impl Into<String>) -> Self {
        self.method = method.into();
        self
    }

    /// The request parameters: `ec.web.parameters`, and names in the context for actions.
    #[must_use]
    pub fn parameters(mut self, parameters: BTreeMap<String, Value>) -> Self {
        self.parameters = parameters;
        self
    }

    /// Session attributes, set before the render starts.
    #[must_use]
    pub fn session(mut self, session: BTreeMap<String, Value>) -> Self {
        self.session = session;
        self
    }

    /// What generated links start with, such as `https://example.com`.
    #[must_use]
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// A render mode that wins over the path extension and the `renderMode` parameter.
    #[must_use]
    pub fn render_mode(mut self, mode: impl Into<String>) -> Self {
        self.render_mode = Some(mode.into());
        self
    }

    /// A request header, such as `X-Moqui-Client-Modes: qvue,qjs` (the best client mode of a `.qvt` request). A header
    /// set twice keeps the last value.
    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Where the render comes from ([`RenderOrigin`]); the default is internal code.
    #[must_use]
    pub fn origin(mut self, origin: RenderOrigin) -> Self {
        self.options.origin = origin;
        self
    }

    /// The gate a transition that writes meets ([`TransitionGate`]); the default is none.
    #[must_use]
    pub fn transition_gate(mut self, gate: TransitionGate) -> Self {
        self.options.gate = gate;
        self
    }

    /// Run the render under the read fence ([`crate::entity::fence`]), except a transition the gate approved. A gate other
    /// than [`TransitionGate::Open`] turns the fence on whatever this says.
    #[must_use]
    pub fn write_fence(mut self, fence: bool) -> Self {
        self.options.write_fence = fence;
        self
    }

    /// The origin, gate and fence at once, as the in-process transport passes them on.
    #[must_use]
    pub fn options(mut self, options: RenderOptions) -> Self {
        self.options = options;
        self
    }

    /// Run the render in this thread's execution context, or in a new one when none is entered.
    ///
    /// # Errors
    ///
    /// Returns the error the render raised. Its kind says 401, 403, or 404 for the web layer.
    pub fn render(self) -> Result<RenderOutput, Error> {
        let owned = (!ExecutionContext::is_entered()).then(|| self.factory.new_context());
        let _entered = owned.as_ref().map(ExecutionContext::enter);
        let _web = web::install_with(self.parameters.clone(), self.base_url.clone());
        for (name, value) in &self.session {
            user::session::set_session_attribute(name, Some(&moqui_lib::object::to_plain_string(value)));
        }
        let mut request = WebRequest::new(self.method.clone(), self.path.clone());
        for (name, value) in &self.headers {
            request.set_header(name.clone(), value.clone());
        }
        let job = Job {
            request: &request,
            root: &self.root,
            path: &self.path,
            method: &self.method,
            parameters: &self.parameters,
            json_list: None,
            login_error: None,
            header_authenticated: true,
            render_mode: self.render_mode.as_deref(),
            enforce_request_checks: false,
            history: self.history,
            new_session: false,
            started: Instant::now(),
            options: &self.options,
        };
        // A gate means an agent's render: what the gate does not let through (the screens, read-only transitions, the
        // path of a held one) runs fenced too, and an approved transition is lifted out of it.
        let fenced = self.options.write_fence || self.options.gate != TransitionGate::Open;
        let _fence = fenced.then(|| crate::entity::fence::enter(format!("the screen render of {}", self.path)));
        run_job(&self.factory, &job)?;
        let (status, headers, body, sent) = web::take_output();
        Ok(RenderOutput {
            status: if sent { status } else { 200 },
            headers,
            body,
        })
    }
}

/// One request into the screen tree.
pub(crate) struct Job<'a> {
    pub request: &'a WebRequest,
    pub root: &'a str,
    pub path: &'a str,
    pub method: &'a str,
    pub parameters: &'a BTreeMap<String, Value>,
    pub json_list: Option<&'a [Value]>,
    pub login_error: Option<&'a str>,
    pub header_authenticated: bool,
    pub render_mode: Option<&'a str>,
    /// False for a render with no socket: no session-token check.
    pub enforce_request_checks: bool,
    /// Record the screen history although the request checks are off (the terminal client).
    pub history: bool,
    /// The session began with this request, so its token cannot have been sent yet (Java's `moqui.session.token.created`).
    pub new_session: bool,
    pub started: Instant,
    /// The origin, the transition gate and the fence: in-process options an HTTP request cannot set.
    pub options: &'a RenderOptions,
}

/// What a running render exposes to the code that writes the screen: `sri` reads this.
pub struct RenderState {
    pub factory: Arc<ExecutionContextFactory>,
    pub facade: Arc<ScreenFacade>,
    pub url_info: Arc<ScreenUrlInfo>,
    /// Canonical render mode: `html-print` for `xsl-fo`.
    pub render_mode: String,
    pub content_type: String,
    pub character_encoding: String,
    /// The mode's `screen-text-output` says it is always standalone.
    pub always_standalone: bool,
    /// How this mode writes a failed section, and whether the level is a JSON envelope.
    pub output_kind: OutputKind,
    /// A `screen-output` mode: text writes are dropped and the document below is the response.
    pub binary: bool,
    /// The workbook of the `xlsx` mode.
    pub xlsx: RefCell<super::xlsx::XlsxBook>,
    /// The macro module for the mode: the last `macro-template` of a screen in the path, else the conf's.
    pub macro_location: String,
    /// Index into `url_info.path_defs` of the screen being rendered.
    pub screen_path_index: Cell<usize>,
    /// Set by actions that sent their own response, so the screen is not rendered.
    pub dont_do_render: Cell<bool>,
    /// Locations of screens that are skipped because a required parameter is missing.
    pub stopped: RefCell<Vec<String>>,
    /// A screen rendered by `include-screen`, which is the active screen while it renders.
    pub override_def: RefCell<Option<Arc<ScreenDef>>>,
    /// Text for the end of the output, and for a script block after it.
    pub after_text: RefCell<String>,
    pub script_text: RefCell<String>,
    /// The rows of the form-lists being rendered, by handle.
    pub form_lists: RefCell<Vec<Option<super::sri::FormRows>>>,
    /// Dynamic forms being rendered, by name, which `sri` reads instead of the definition's forms.
    pub dynamic_forms: RefCell<std::collections::HashMap<String, Arc<super::form::FormInstance>>>,
    /// `sri.baseLinkUrl(url)`: the base URL relative links in rendered content resolve against, as Java's
    /// `ScreenRenderImpl.baseLinkUrl`; `None` is the current screen's path. Nothing in the port reads it yet: Java
    /// rewrites the relative links of rendered markdown with it, and the port's markdown leaves them as written.
    pub base_link_url: RefCell<Option<String>>,
    /// The first form focus id of this render (`form.field`). Empty until a form macro sets it.
    pub focus: RefCell<String>,
    /// Node ids `sri.uniqueId` has given out in this document, and the next suffix to try for each id asked for.
    pub node_ids: RefCell<(
        std::collections::HashSet<String>,
        std::collections::HashMap<String, usize>,
    )>,
    /// Keys `sri.once` has seen, so a mode writes its document wrapper only for the outermost screen.
    pub once: RefCell<std::collections::HashSet<String>>,
    /// The theme of this render and its resource values by type, found once as Java's `curThemeId`.
    pub theme_id: RefCell<Option<String>>,
    pub theme_values: RefCell<std::collections::HashMap<String, Vec<String>>>,
}

impl RenderState {
    pub(super) fn new(
        factory: &Arc<ExecutionContextFactory>,
        facade: &Arc<ScreenFacade>,
        info: Arc<ScreenUrlInfo>,
        mode: String,
        content_type: String,
        character_encoding: String,
    ) -> Self {
        let always_standalone = facade.is_always_standalone(&mode);
        let output_kind = facade.output_kind(&mode);
        let binary = facade.is_render_mode_binary(&mode);
        let macro_location = info
            .path_defs
            .iter()
            .filter_map(|def| def.macro_template_location(&mode))
            .rfind(|location| !location.is_empty())
            .map(str::to_string)
            .or_else(|| {
                facade
                    .text_output(&mode)
                    .map(|output| output.macro_template_location.clone())
            })
            .unwrap_or_default();
        Self {
            factory: Arc::clone(factory),
            facade: Arc::clone(facade),
            url_info: info,
            render_mode: mode,
            content_type,
            character_encoding,
            always_standalone,
            output_kind,
            binary,
            xlsx: RefCell::new(super::xlsx::XlsxBook::default()),
            macro_location,
            screen_path_index: Cell::new(0),
            dont_do_render: Cell::new(false),
            stopped: RefCell::new(Vec::new()),
            override_def: RefCell::new(None),
            after_text: RefCell::new(String::new()),
            script_text: RefCell::new(String::new()),
            form_lists: RefCell::new(Vec::new()),
            dynamic_forms: RefCell::new(std::collections::HashMap::new()),
            base_link_url: RefCell::new(None),
            focus: RefCell::new(String::new()),
            node_ids: RefCell::default(),
            once: RefCell::new(std::collections::HashSet::new()),
            theme_id: RefCell::new(None),
            theme_values: RefCell::new(std::collections::HashMap::new()),
        }
    }
}

thread_local! {
    static ACTIVE: RefCell<Vec<Rc<RenderState>>> = const { RefCell::new(Vec::new()) };
}

struct ActiveGuard;

impl ActiveGuard {
    fn install(state: Rc<RenderState>) -> Self {
        ACTIVE.with(|stack| stack.borrow_mut().push(state));
        Self
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        ACTIVE.with(|stack| {
            stack.borrow_mut().pop();
        });
    }
}

/// Run `body` with the render in progress on this thread, if there is one.
pub fn with_current_render<R>(body: impl FnOnce(&RenderState) -> R) -> Option<R> {
    let state = ACTIVE.with(|stack| stack.borrow().last().cloned())?;
    Some(body(&state))
}

/// `sri.sendRedirectAndStopRender`: redirect and skip the screen. A standalone mode, which a
/// client router fetches, gets 205 and `X-Redirect-To` instead of a 302. Both answers go through
/// the same location rule, including when no execution context is entered: then there is no
/// `http-host`, so an `http(s)` URL is still reduced.
pub fn send_redirect_and_stop_render(url: &str) {
    let base = web::base_url();
    let location = if url.starts_with('/') && !url.starts_with("//") {
        format!("{}{url}", base.trim_end_matches('/'))
    } else {
        url.to_string()
    };
    let standalone = with_current_render(|state| {
        state.dont_do_render.set(true);
        state.always_standalone
    })
    .unwrap_or(false);
    let factory = ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory()));
    let location = match factory.as_deref() {
        Some(factory) => http_redirect_location(factory, &location),
        None => redirect_location(&location, true),
    };
    if standalone {
        web::send_reset_content(&location);
    } else {
        web::send_redirect(&with_query(location, &BTreeMap::new()));
    }
}

/// Java's `ScreenRenderImpl.sendHttpRedirect`: the `Location` of a screen redirect.
///
/// The location is sanitized (Java's `WebUtilities.sanitizeRedirectLocation`: no CR, LF or NUL, so a
/// value cannot split the header). An `http(s)` target is reduced to its path and query when the webapp
/// names no `http-host` (the default). That path is kept only when it starts with a single `/` and holds
/// no `\`, so `//phish` and `/\evil.com` become `/`. A URL Java's `URI.create` rejects — a space, or
/// another character outside the URI set — becomes `/` too, as Java's catch does. With an `http-host`
/// the absolute target is sent, as Java sends it.
///
/// A target that is not a path of this site and not an `http(s)` URL — a protocol-relative one, or another
/// scheme — goes to the root. Java sends those as they are; the port does not, and the difference is
/// recorded in `plans/Parity-Diffs.md`.
pub(super) fn http_redirect_location(factory: &ExecutionContextFactory, location: &str) -> String {
    let reduce = factory.webapp_setting(WEBAPP, "http-host").is_empty();
    redirect_location(location, reduce)
}

/// [`http_redirect_location`] with the host decision already made. `reduce_http` is true when the webapp
/// names no `http-host`, and when no execution context is entered.
fn redirect_location(location: &str, reduce_http: bool) -> String {
    let mut clean = String::with_capacity(location.len());
    for ch in location.chars() {
        if ch != '\r' && ch != '\n' && ch != '\0' {
            clean.push(ch);
        }
    }
    let clean = clean.trim();
    if clean.is_empty() {
        return "/".to_string();
    }
    if clean.starts_with("http://") || clean.starts_with("https://") {
        if reduce_http {
            if !uri_create_accepts(clean) {
                return "/".to_string();
            }
            return site_path(url_path_and_query(clean));
        }
        return clean.to_string();
    }
    if clean.starts_with("//") || has_scheme(clean) {
        return "/".to_string();
    }
    clean.to_string()
}

/// A reduced `http(s)` target stays only when its path starts with a single `/` and holds no backslash.
/// `//phish` is another host once a browser sees it, and so is `/\evil.com`.
fn site_path(path_and_query: String) -> String {
    let path = path_and_query
        .split_once('?')
        .map_or(path_and_query.as_str(), |(path, _)| path);
    if path.starts_with('/') && !path.starts_with("//") && !path.contains('\\') {
        path_and_query
    } else {
        "/".to_string()
    }
}

/// Whether Java's `URI.create` accepts `url`. The legal set is Java's unreserved, reserved and `#`,
/// plus a `%` escape and a non-ASCII character that is not a space or a control.
fn uri_create_accepts(url: &str) -> bool {
    let mut chars = url.chars();
    while let Some(ch) = chars.next() {
        if ch == '%' {
            match (chars.next(), chars.next()) {
                (Some(high), Some(low)) if high.is_ascii_hexdigit() && low.is_ascii_hexdigit() => {}
                _ => return false,
            }
            continue;
        }
        if !uri_char(ch) {
            return false;
        }
    }
    true
}

fn uri_char(ch: char) -> bool {
    if ch.is_ascii_alphanumeric() {
        return true;
    }
    if matches!(
        ch,
        '-' | '_'
            | '.'
            | '!'
            | '~'
            | '*'
            | '\''
            | '('
            | ')'
            | ','
            | ';'
            | ':'
            | '$'
            | '&'
            | '+'
            | '='
            | '?'
            | '/'
            | '['
            | ']'
            | '@'
            | '#'
    ) {
        return true;
    }
    !ch.is_ascii() && !ch.is_control() && !ch.is_whitespace()
}

/// The path and query of an `http(s)` URL, or `/` when it has no path: Java's `URI` and its
/// `getRawPath` and `getRawQuery`.
fn url_path_and_query(url: &str) -> String {
    let after = url.split_once("://").map_or("", |(_, rest)| rest);
    let end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let rest = &after[end..];
    let (path, query) = match rest.split_once('?') {
        Some((path, query)) => (path, Some(query.split('#').next().unwrap_or(""))),
        None => (rest.split('#').next().unwrap_or(""), None),
    };
    let path = if path.is_empty() {
        "/".to_string()
    } else {
        path.to_string()
    };
    match query {
        Some(query) if !query.is_empty() => format!("{path}?{query}"),
        _ => path,
    }
}

/// Whether a location names a scheme, as `URI.create` reads one: letters, then letters, digits, `+`, `-`
/// and `.`, then a colon.
fn has_scheme(location: &str) -> bool {
    let Some((name, _)) = location.split_once(':') else {
        return false;
    };
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '+' || ch == '-' || ch == '.')
}

/// `ec.web.saveScreenLastInfo`: remember `path`, or the screen being rendered, with the request parameters, for a
/// later `screen-last` response such as the one after login.
pub fn save_screen_last_info(path: Option<&str>) {
    let current = with_current_render(|state| state.url_info.full_path_url());
    if let Some(path) = path.map(str::to_string).or(current) {
        user::session::save_screen_last(&path, &plain_request_parameters());
    }
}

/// The request parameters as text, without passwords, the session token, or uploaded files.
fn plain_request_parameters() -> BTreeMap<String, String> {
    web::parameters()
        .into_iter()
        .filter(|(name, value)| {
            let lower = name.to_ascii_lowercase();
            !lower.contains("password") && lower != "moquisessiontoken" && !matches!(value, Value::Object(_))
        })
        .map(|(name, value)| (name, moqui_lib::object::to_plain_string(&value)))
        .collect()
}

/// Resolve and run one job. Anything written is in the thread's web slot.
pub(crate) fn run_job(factory: &Arc<ExecutionContextFactory>, job: &Job<'_>) -> Result<(), Error> {
    let _origin = gate::OriginGuard::install(job.options.origin.clone(), job.header_authenticated);
    let facade = ScreenFacade::of(factory);
    // An always-standalone JSON mode (`tui`) renders only the last level. Without a `lastStandalone` the walk would
    // otherwise render the webroot shell, whose html text is not a JSON array. The mode may come from the
    // render-mode argument or a `renderMode` parameter, known now, or from the path's query or extension, known once
    // the path is parsed.
    let standalone_json = |mode: Option<String>| {
        mode.is_some_and(|mode| {
            let mode = canonical_render_mode(&mode);
            facade.is_always_standalone(mode) && facade.output_kind(mode) == OutputKind::Json
        })
    };
    let mut last_standalone = parse_last_standalone(&text_of(job.parameters.get("lastStandalone")), 0);
    if last_standalone == 0 && standalone_json(requested_mode(job, None)) {
        last_standalone = 1;
    }
    let mut info = ScreenUrlInfo::parse(factory, job.root, job.path, last_standalone)?;
    if last_standalone == 0 && standalone_json(requested_mode(job, Some(&info))) {
        info = ScreenUrlInfo::parse(factory, job.root, job.path, 1)?;
    }
    check_web_settings(&info)?;
    if job.enforce_request_checks && (redirect_to_https(factory, job, &info) || redirect_to_login(factory, job, &info)?)
    {
        return Ok(());
    }
    check_authentication(&info)?;
    // Under a screen that says include-child-content a text file or template renders inside the screens, with the
    // request in the context; a binary file is still sent.
    let child_content = info.file_resource.as_ref().filter(|file| {
        info.target_screen.include_child_content
            && (super::static_files::is_template(factory, file) || super::static_files::is_text(file))
    });
    if let Some(file) = info.file_resource.as_ref().filter(|_| child_content.is_none()) {
        let versioned = !text_of(job.parameters.get("v")).is_empty();
        let start_ms = user::now_millis();
        let started = Instant::now();
        super::static_files::respond(factory, job.request, file, versioned)?;
        if info.target_screen.track_artifact_hit && web::output_status() == 200 {
            count_content_hit(file, job.parameters, start_ms, started);
        }
        return Ok(());
    }

    // Parameters live in a context frame for the request, so nothing leaks into the next one.
    let _frame = ContextFrame::push();
    for (name, value) in job.parameters {
        script::set_context_name(name, value.clone())?;
    }
    for (name, value) in &info.inline_parameters {
        if !job.parameters.contains_key(name) {
            script::set_context_name(name, Value::string(value))?;
        }
    }

    if info.target_transition_name.is_some() {
        return run_transition(factory, job, info);
    }

    let picked = pick_render_mode(&facade, job, &info)?;
    let client = client_mode(job.request, &mode_screen(&info, &picked), &picked);
    let negotiated = client.is_some();
    let mode = client.unwrap_or(picked);
    let rendered = mode_screen(&info, &mode);
    if !rendered.allows_render_mode(&mode) {
        // A qvue-only screen still answers tui: one placeholder, so the screen list stays a document.
        if facade.output_kind(&mode) == OutputKind::Json {
            return tui_unsupported_screen(&info);
        }
        return Err(Error::of_kind(
            ErrorKind::NotFound,
            format!("render mode {mode} is not supported by {}", rendered.location),
        ));
    }
    let (content_type, encoding) = output_type(&facade, &info, &mode);
    let state = Rc::new(RenderState::new(factory, &facade, info, mode, content_type, encoding));
    let _active = ActiveGuard::install(Rc::clone(&state));
    // A screen marked server-static for qvt is sent as a template that holds no request data, from the cache.
    // Record history after the send: a later failure must not record a screen the user did not get.
    if state.render_mode == "qvt" && super::statics::serve_template(&state, job.request, job.path)? {
        record_history(job, &state);
        return Ok(());
    }
    let begin = state.url_info.begin_transaction;
    let timeout = state.url_info.transaction_timeout;
    in_transaction(begin, timeout, || render_screens(&state, &facade))?;
    add_client_component_headers(&state);
    if negotiated && web::output_sent() && web::output_status() == 200 {
        web::add_header("X-Moqui-Qvt", state.render_mode.clone());
    }
    add_server_static_header(&state);
    record_history(job, &state);
    Ok(())
}

/// The screen whose `render-modes` must name `mode`: the target, except in qvt for a target that a default subscreen item
/// chose below the screen the server renders. A qvt screen writes `m-subscreens-active` for what is under it, and the
/// client asks for each level below as its own request (the shell, `moqui.loadComponent`), so a parent's own render (its
/// menu and container) does not depend on the modes of a default subscreen levels down: then it is the deepest screen the
/// path names, or the first one that renders when the render starts below it (a `lastStandalone` render of the leaf).
/// A request that names the leaf, or renders it, is checked against the leaf.
fn mode_screen(info: &ScreenUrlInfo, mode: &str) -> Arc<ScreenDef> {
    let defaults = info.full_path.len().saturating_sub(info.minimal_path.len());
    let count = info.path_defs.len();
    if mode != "qvt" || defaults == 0 || count == 0 || defaults >= count {
        return Arc::clone(&info.target_screen);
    }
    let named = count - defaults - 1;
    Arc::clone(&info.path_defs[named.max(info.render_path_difference).min(count - 1)])
}

/// The `AT_XML_SCREEN_CONTENT` hit of a file sent from under a screen, by its content type.
fn count_content_hit(location: &str, parameters: &BTreeMap<String, Value>, start_ms: i64, started: Instant) {
    let name = location.rsplit('/').next().unwrap_or(location);
    let content_type = super::static_files::media_type(name);
    let output_size = web::output_body_len().and_then(|len| i64::try_from(len).ok());
    ExecutionContext::with_current(|ec| {
        ec.count_artifact_hit(&crate::context::Hit {
            artifact_type: "AT_XML_SCREEN_CONTENT",
            sub_type: Some(content_type),
            name: location,
            parameters: Some(parameters),
            start_ms,
            running_ms: started.elapsed().as_secs_f64() * 1000.0,
            output_size,
        });
    });
}

/// The client mode a `.qvt` request takes instead, as the `/qapps` router picks one from the menu data: `moqui.loadComponent` sends
/// `X-Moqui-Client-Modes: qvue,qjs` when it loads a screen whose render modes it does not know (an `m-dynamic-container` or
/// `m-dynamic-dialog`, a tray tab, a quick-create kind), and the first of those the target screen names in its `render-modes` is
/// rendered, with `X-Moqui-Qvt` saying which. A screen whose modes are `all` or name neither stays qvt, and a request without the
/// header gets the mode of its extension.
fn client_mode(request: &WebRequest, target: &ScreenDef, picked: &str) -> Option<String> {
    if picked != "qvt" {
        return None;
    }
    request
        .header("X-Moqui-Client-Modes")?
        .split(',')
        .map(str::trim)
        .find(|mode| matches!(*mode, "qvue" | "qjs") && target.render_modes.iter().any(|named| named == mode))
        .map(str::to_string)
}

/// A screen that says `server-static` for a mode other than the qvt family gets the cache header, as in Java: it is right only for output
/// that does not vary. The subscreens-only case the schema marks by itself is not sent that way for html, whose page holds the navbar of the user.
fn add_server_static_header(state: &RenderState) {
    let def = &state.url_info.target_screen;
    let mode = state.render_mode.as_str();
    if matches!(mode, "qvt" | "qjs" | "qvue")
        || def.server_static_auto
        || !web::output_sent()
        || web::output_status() != 200
    {
        return;
    }
    let marked = def
        .server_static
        .iter()
        .any(|name| name == "all" || canonical_render_mode(name) == mode);
    if marked {
        web::add_header("Cache-Control", super::statics::server_static_cache_control());
    }
}

/// The qvt family answers a client that builds a component from the text. A merged response is for one user and one
/// request, so it is never cached, and `X-Moqui-Qvt` names how the client handles the text: `merged-fragment` is a
/// template with the request's values in it. (A server-static template says `static-template` and caches.)
fn add_client_component_headers(state: &RenderState) {
    if !matches!(state.render_mode.as_str(), "qvt" | "qjs" | "qvue")
        || !web::output_sent()
        || web::output_status() != 200
    {
        return;
    }
    web::add_header("Cache-Control", "private, no-store");
    if state.render_mode == "qvt" {
        web::add_header("X-Moqui-Qvt", "merged-fragment");
    }
}

/// A screen the user got goes on the screen history, newest first, with its query, as Java's `saveScreenHistory`: not a
/// standalone screen, one that says `history="false"`, a fragment asked for with a negative `lastStandalone`, or a
/// render with no socket. A qvt screen the `/qapps` shell loads counts, as Java counted the vapps fetches.
fn record_history(job: &Job<'_>, state: &RenderState) {
    let target = &state.url_info.target_screen;
    if !(job.enforce_request_checks || job.history)
        || !job.method.eq_ignore_ascii_case("get")
        || state.url_info.last_standalone < 0
        || target.standalone
        || !target.history
        || matches!(state.render_mode.as_str(), "qjs" | "qvue")
        || web::output_status() != 200
    {
        return;
    }
    let mut path = state.url_info.full_path_url();
    // The sign-in pages are not places to return to.
    let first = path.trim_start_matches('/').split('/').next().unwrap_or("");
    if ["Login", "SecondFactor", "ChangePassword", "sso", "error", "rest", "rpc"].contains(&first) {
        return;
    }
    let query: Vec<(String, String)> = job
        .request
        .query
        .iter()
        .filter(|(name, _)| !matches!(name.as_str(), "moquiSessionToken" | "lastStandalone" | "renderMode"))
        .cloned()
        .collect();
    if !query.is_empty() {
        path = with_query(path, &query.into_iter().collect());
    }
    user::session::push_screen_history(&path);
}

fn render_screens(state: &RenderState, facade: &ScreenFacade) -> Result<(), Error> {
    let info = &state.url_info;
    let defs = &info.path_defs;
    let last = defs.len().saturating_sub(1);
    // The saved find of the request or the user's default for the target, before any action reads its parameters.
    let target_parameters: Vec<String> = info
        .target_screen
        .parameters
        .iter()
        .map(|parameter| parameter.name.clone())
        .collect();
    super::saved_finds::apply_on_render(&info.target_screen.location, &target_parameters, true)?;
    {
        // always-actions for every screen in the path, each under the frames of the screens above it
        let mut frames = Frames::untracked();
        for (index, def) in defs.iter().enumerate() {
            frames.enter_screen(def, index == last)?;
            state.screen_path_index.set(index);
            if let Some(actions) = &def.always_actions {
                script::run_actions(&actions.xml)?;
            }
        }
    }

    // A screen that sets saveFilename makes the response a download.
    if let Some(filename) = script::context_value("saveFilename").map(|value| text_of(Some(&value))) {
        if !filename.is_empty() {
            let safe: String = filename
                .chars()
                .map(|ch| {
                    if ch.is_ascii_graphic() && !matches!(ch, '"' | '\\' | ';') {
                        ch
                    } else {
                        '_'
                    }
                })
                .collect();
            web::add_header(
                "Content-Disposition",
                format!(
                    "attachment; filename=\"{safe}\"; filename*=utf-8''{}",
                    encode_component(&filename)
                ),
            );
        }
    }

    // Screens before the render start only sit on the stack, unless the mode is always standalone:
    // then the parents' pre-actions run too, from the third screen on (not webroot and apps).
    let first_pre =
        if info.target_render_mode.is_some() && facade.is_always_standalone(&state.render_mode) && defs.len() > 2 {
            2
        } else {
            info.render_path_difference
        };
    {
        let mut frames = Frames::untracked();
        for (index, def) in defs.iter().enumerate() {
            frames.enter_screen(def, index == last)?;
            if index >= first_pre {
                state.screen_path_index.set(index);
                if let Some(actions) = &def.pre_actions {
                    script::run_actions(&actions.xml)?;
                }
            }
        }
    }
    if state.dont_do_render.get() || web::output_sent() {
        return Ok(());
    }

    // Past always-actions and pre-actions it is too late to supply a required parameter.
    for def in info.render_defs() {
        for parameter in def.parameters.iter().filter(|parameter| parameter.required) {
            if is_empty(script::context_value(&parameter.name).as_ref()) {
                ExecutionContext::with_current(|ec| {
                    ec.message()
                        .add_error(format!("Required parameter missing ({})", parameter.name));
                });
                state.stopped.borrow_mut().push(def.location.clone());
            }
        }
    }
    // The screens above the render start stay on the artifact stack so the screens below inherit. Each must be a
    // subscreens item the user may have, as Java's doActualRender checks `isValidInCurrentContext`; so must the first
    // screen rendered (a lastStandalone render starts at the last), which no `sri.renderSubscreen` decides.
    path_items_available(info, (info.render_path_difference + 1).min(defs.len()))?;
    let mut inherited = Frames::default();
    for def in defs[..info.render_path_difference.min(defs.len())].iter() {
        inherited.push_screen(def, false)?;
        inherited.hit_detail(Some(&state.content_type), true);
    }
    state.screen_path_index.set(info.render_path_difference);
    render_body(state)
}

/// Refuse a path whose screens before `count` include a subscreens item limited to a user group the user is not in
/// (`SubscreensItem.userGroupId`, `isValidInCurrentContext`): the screens a render starts above or at, and every screen
/// of a transition's path.
fn path_items_available(info: &ScreenUrlInfo, count: usize) -> Result<(), Error> {
    let defs = &info.path_defs;
    for index in 1..count.min(defs.len()) {
        let item = info
            .full_path
            .get(index - 1)
            .and_then(|name| defs[index - 1].subscreens.get(name));
        if item.is_some_and(|item| !item.is_valid_in_current_context()) {
            return Err(Error::of_kind(
                ErrorKind::Unauthorized,
                format!("The screen {} is not available", defs[index].screen_name),
            ));
        }
    }
    Ok(())
}

/// Render the first screen to render, which renders the ones below it through `sri.renderSubscreen`.
/// The output opens when the first text is written, or here when nothing was.
fn render_body(state: &RenderState) -> Result<(), Error> {
    let is_target = state.url_info.path_defs.len() <= state.screen_path_index.get() + 1;
    script::with_screen_runner(|runner| super::sri::render_active_screen(state, runner, is_target))?;
    if state.binary {
        if !web::output_sent() && !state.dont_do_render.get() {
            let bytes = state.xlsx.borrow().to_bytes()?;
            web::send_bytes_with_headers(
                200,
                vec![("Content-Type".to_string(), state.content_type.clone())],
                bytes,
            );
        }
        return Ok(());
    }
    super::sri::flush_end_text(state);
    if state.output_kind == OutputKind::Json && !state.dont_do_render.get() {
        wrap_json_envelope(state);
    }
    if !web::output_sent() && !state.dont_do_render.get() {
        web::start_output(200, &super::sri::content_type_header(state));
    }
    Ok(())
}

/// A screen whose `render-modes` omit `tui`. The document is one placeholder, not an error page.
fn tui_unsupported_screen(info: &ScreenUrlInfo) -> Result<(), Error> {
    let placeholder = serde_json::json!([{ "t": "placeholder", "text": "render-mode", "url": screen_path(info) }]);
    let body = json_envelope(info, placeholder, serde_json::json!({}), "", empty_messages());
    web::send_bytes_with_headers(
        200,
        vec![("Content-Type".to_string(), "application/json".to_string())],
        body.to_string().into_bytes(),
    );
    Ok(())
}

/// The outermost `tui` render: the root array becomes `c` of one envelope. A nested unit writes its own array
/// and does not come through here.
fn wrap_json_envelope(state: &RenderState) {
    if !web::output_sent() {
        web::start_output(200, &super::sri::content_type_header(state));
    }
    let raw = web::with_body(<[u8]>::to_vec);
    let nodes = json_root_array(&raw, &state.url_info);
    let envelope = json_envelope(
        &state.url_info,
        nodes,
        envelope_parameters(),
        &state.focus.borrow(),
        envelope_messages(),
    );
    web::replace_body(envelope.to_string().into_bytes());
}

/// The `tui` envelope of section 2.3 of the plan. Every string from the request or the screen is stripped of
/// controls; the nodes were stripped as they were written.
fn json_envelope(
    info: &ScreenUrlInfo,
    nodes: serde_json::Value,
    parameters: serde_json::Value,
    focus: &str,
    messages: serde_json::Value,
) -> serde_json::Value {
    let strip = super::sri::strip_controls;
    let target = &info.target_screen;
    let title = if target.default_menu_title.is_empty() {
        &target.screen_name
    } else {
        &target.default_menu_title
    };
    let path_list: Vec<String> = info.full_path.iter().map(|name| strip(name)).collect();
    let render_modes: Vec<String> = target.render_modes.iter().map(|mode| strip(mode)).collect();
    serde_json::json!({
        "v": 1,
        "screen": {
            "path": strip(&screen_path(info)),
            "pathList": path_list,
            "title": strip(title),
            "renderModes": render_modes,
            "standalone": info.render_path_difference,
        },
        "parameters": parameters,
        "messages": messages,
        "focus": strip(focus),
        "c": nodes,
    })
}

fn screen_path(info: &ScreenUrlInfo) -> String {
    if info.full_path.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", info.full_path.join("/"))
    }
}

fn empty_messages() -> serde_json::Value {
    serde_json::json!({ "info": [], "errors": [], "validationErrors": [] })
}

/// The root array a level wrote. Bytes that are not one JSON array are logged and become one error node, so the
/// client still gets a document.
fn json_root_array(raw: &[u8], info: &ScreenUrlInfo) -> serde_json::Value {
    let trimmed = raw.trim_ascii();
    if trimmed.is_empty() {
        return serde_json::Value::Array(Vec::new());
    }
    match serde_json::from_slice::<serde_json::Value>(trimmed) {
        Ok(value) if value.is_array() => value,
        Ok(_) => {
            tracing::error!("Screen {} did not render a JSON array", screen_path(info));
            serde_json::json!([{"t":"error","text":"the screen did not render as JSON"}])
        }
        Err(err) => {
            tracing::error!("Screen {} did not render as JSON: {err}", screen_path(info));
            serde_json::json!([{"t":"error","text":"the screen did not render as JSON"}])
        }
    }
}

fn envelope_parameters() -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (name, value) in web::parameters() {
        if matches!(value, Value::Object(_)) {
            continue;
        }
        let text = super::sri::strip_controls(&moqui_lib::object::to_plain_string(&value));
        map.insert(super::sri::strip_controls(&name), serde_json::Value::String(text));
    }
    map.remove("moquiSessionToken");
    let mut value = serde_json::Value::Object(map);
    web::strip_credential_parameters(&mut value);
    value
}

fn envelope_messages() -> serde_json::Value {
    let (info, errors, validation) = ExecutionContext::with_current(|ec| {
        let facade = ec.message();
        let mut info = Vec::new();
        let mut errors = Vec::new();
        for (index, text) in facade.messages().iter().enumerate() {
            let cleaned = super::sri::strip_controls(text);
            if matches!(facade.message_type(index), "danger" | "error") {
                errors.push(cleaned);
            } else {
                info.push(cleaned);
            }
        }
        let validation_text: Vec<String> = facade
            .validation_errors()
            .iter()
            .map(|error| error.message.clone())
            .collect();
        for error in facade.errors() {
            if !validation_text.iter().any(|message| message == error) {
                errors.push(super::sri::strip_controls(error));
            }
        }
        let validation: Vec<serde_json::Value> = facade
            .validation_errors()
            .iter()
            .map(|error| {
                serde_json::json!({
                    "form": super::sri::strip_controls(&error.form),
                    "field": super::sri::strip_controls(&error.field),
                    "message": super::sri::strip_controls(&error.message),
                })
            })
            .collect();
        (info, errors, validation)
    })
    .unwrap_or_default();
    serde_json::json!({
        "info": info,
        "errors": errors,
        "validationErrors": validation,
    })
}

fn pick_render_mode(facade: &ScreenFacade, job: &Job<'_>, info: &ScreenUrlInfo) -> Result<String, Error> {
    let chosen = requested_mode(job, Some(info)).unwrap_or_else(|| DEFAULT_MODE.to_string());
    if !facade.is_render_mode_valid(&chosen) {
        return Err(Error::of_kind(
            ErrorKind::NotFound,
            format!("no such render mode: {chosen}"),
        ));
    }
    Ok(canonical_render_mode(&chosen).to_string())
}

/// The mode a request asks for, not yet checked: the render-mode argument, then a `renderMode` parameter, then the
/// path's extension. A `?renderMode=` on the screen path counts as the parameter, as Java's ScreenTest puts the path's
/// query in the request parameters. With no parsed path, only the first two.
fn requested_mode(job: &Job<'_>, info: Option<&ScreenUrlInfo>) -> Option<String> {
    let present = |text: &String| !text.is_empty();
    job.render_mode
        .map(str::to_string)
        .filter(present)
        .or_else(|| Some(text_of(job.parameters.get("renderMode"))).filter(present))
        .or_else(|| {
            info.and_then(|info| info.inline_parameters.get("renderMode").cloned())
                .filter(present)
        })
        .or_else(|| info.and_then(|info| info.target_render_mode.clone()))
}

/// Content type and encoding. A `web-settings` mime type on a screen in the render path wins over
/// the mode's `screen-text-output`, as in Java.
fn output_type(facade: &ScreenFacade, info: &ScreenUrlInfo, mode: &str) -> (String, String) {
    let mut content_type = None;
    let mut encoding = None;
    for def in info.render_defs() {
        if let Some(mime) = &def.web.mime_type {
            content_type = Some(mime.clone());
        }
        if let Some(found) = &def.web.character_encoding {
            encoding = Some(found.clone());
        }
    }
    let content_type = content_type
        .or_else(|| facade.mime_type(mode).map(str::to_string))
        .unwrap_or_else(|| "text/html".to_string());
    (content_type, encoding.unwrap_or_else(|| DEFAULT_ENCODING.to_string()))
}

fn check_web_settings(info: &ScreenUrlInfo) -> Result<(), Error> {
    for def in info.render_defs() {
        if !def.web.allow_web_request {
            return Err(Error::of_kind(
                ErrorKind::Unauthorized,
                format!(
                    "The screen {} cannot be used in a web request (allow-web-request=false)",
                    def.location
                ),
            ));
        }
    }
    Ok(())
}

/// A screen to render that does not say `require-encryption="false"`, asked for over http when the webapp has https
/// enabled, redirects to the same URL over https (Java's `checkWebappSettings`).
fn redirect_to_https(factory: &ExecutionContextFactory, job: &Job<'_>, info: &ScreenUrlInfo) -> bool {
    if job.request.https || !info.render_defs().iter().any(|def| def.web.require_encryption) {
        return false;
    }
    let Some((host, port)) = factory.webapp_https(WEBAPP) else {
        return false;
    };
    let host = if host.is_empty() {
        web::host_without_port(job.request.header("host").unwrap_or("localhost")).to_string()
    } else {
        host
    };
    let authority = if port.is_empty() || port == "443" {
        host
    } else {
        format!("{host}:{port}")
    };
    let mut location = format!(
        "https://{authority}{}",
        job.request.path.split('?').next().unwrap_or("")
    );
    let query: BTreeMap<String, String> = job.request.query.iter().cloned().collect();
    location = with_query(location, &query);
    save_messages();
    web::send_redirect(&location);
    true
}

/// A browser page that needs a user, asked for with none, saves itself as the screen to come back to and redirects
/// to the last `login-path` of the path, `/Login` by default (Java's `checkWebappSettings`). A fragment, which a client
/// fetches with `lastStandalone`, a transition, and a `tui` request (the terminal client shows its own login dialog) get
/// the 401 instead.
fn redirect_to_login(factory: &ExecutionContextFactory, job: &Job<'_>, info: &ScreenUrlInfo) -> Result<bool, Error> {
    if info.file_resource.is_some()
        || super::statics::report_active()
        || user::current_username()?.is_some()
        || user::is_anonymous()?
        || !info
            .render_defs()
            .iter()
            .any(|def| def.authentication == Authentication::Required)
    {
        return Ok(false);
    }
    let path = job.path.split(['?', '#']).next().unwrap_or("");
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    user::session::save_screen_last(&path, &plain_request_parameters());
    save_messages();
    if info.last_standalone != 0
        || info.target_transition_name.is_some()
        || requested_mode(job, Some(info)).as_deref() == Some("tui")
    {
        return Ok(false);
    }
    let login_path = info
        .path_defs
        .iter()
        .filter_map(|def| def.login_path.clone())
        .next_back()
        .unwrap_or_else(|| "/Login".to_string());
    let location = format!(
        "{}/{}",
        web::base_url().trim_end_matches('/'),
        login_path.trim_start_matches('/')
    );
    web::send_redirect(&http_redirect_location(factory, &location));
    Ok(true)
}

/// Every screen that renders and requires authentication needs a user. Anonymous access screens
/// and `require-authentication="false"` screens do not. As in Java, an anonymous login (a service such
/// as `send#EmailTemplate` that is `anonymous-view` rendering an email body) counts.
fn check_authentication(info: &ScreenUrlInfo) -> Result<(), Error> {
    // The compatibility report looks at screens, not at who may see them.
    if super::statics::report_active() || user::current_username()?.is_some() || user::is_anonymous()? {
        return Ok(());
    }
    for def in info.render_defs() {
        if def.authentication == Authentication::Required {
            return Err(Error::of_kind(
                ErrorKind::AuthenticationRequired,
                "Authentication required",
            ));
        }
    }
    Ok(())
}

/// Artifact frames pushed for a request, popped in reverse when this drops. Anonymous-access
/// screens log the session in as anonymous until then.
#[derive(Default)]
pub(super) struct Frames {
    pushed: usize,
    anonymous: bool,
    /// The frames are not artifact hits: a check, or the screens that run actions before the render.
    untracked: bool,
    /// The base of the isolated check these frames are, restored when they drop ([`Frames::check`]).
    check_base: Option<usize>,
}

impl Frames {
    /// Frames that are not counted as artifact hits, Java's `setTrackArtifactHit(false)`.
    pub(super) fn untracked() -> Self {
        Self {
            untracked: true,
            ..Self::default()
        }
    }

    /// Frames of an authorization check, Java's `ScreenUrlInfo.isPermitted`: not artifact hits, and decided against a
    /// fresh stack, so the frames of the render the check runs in neither grant nor deny anything (a menu screen's
    /// inheritable grant does not reach the screens it lists).
    pub(super) fn check() -> Self {
        Self {
            untracked: true,
            check_base: ExecutionContext::with_current(ExecutionContext::begin_isolated_check),
            ..Self::default()
        }
    }

    pub(super) fn push(
        &mut self,
        artifact_type: &str,
        name: &str,
        action: &str,
        requires_authz: bool,
    ) -> Result<(), Error> {
        ExecutionContext::with_current(|ec| {
            ec.push_checked(artifact_type, name, action, requires_authz, false, None)?;
            if self.untracked {
                ec.untrack_hit();
            }
            Ok::<(), Error>(())
        })
        .unwrap_or_else(|| {
            Err(Error::new(
                "no execution context is entered; the screen artifact is not checked",
            ))
        })?;
        self.pushed += 1;
        Ok(())
    }

    /// The hit sub-type of the frame pushed last, and whether it is counted at all.
    pub(super) fn hit_detail(&self, sub_type: Option<&str>, track: bool) {
        ExecutionContext::with_current(|ec| {
            ec.set_hit_detail(sub_type, None);
            if !track {
                ec.untrack_hit();
            }
        });
    }

    pub(super) fn push_screen(&mut self, def: &ScreenDef, requires_authz: bool) -> Result<(), Error> {
        self.push("AT_XML_SCREEN", &def.location, "view", requires_authz)
    }

    /// Push a screen's frame, with authorization required only for the last screen of the path
    /// when it requires authentication, and apply `anonymous-view` and `anonymous-all`.
    pub(super) fn enter_screen(&mut self, def: &ScreenDef, is_last: bool) -> Result<(), Error> {
        self.push_screen(def, is_last && def.authentication == Authentication::Required)?;
        self.apply_anonymous(def)
    }

    fn apply_anonymous(&mut self, def: &ScreenDef) -> Result<(), Error> {
        let all = match def.authentication {
            Authentication::AnonymousAll => true,
            Authentication::AnonymousView => false,
            _ => return Ok(()),
        };
        if user::login_anonymous()? {
            self.anonymous = true;
        }
        ExecutionContext::with_current(|ec| ec.set_anonymous_authorized(all, ""));
        Ok(())
    }
}

impl Drop for Frames {
    fn drop(&mut self) {
        for _ in 0..self.pushed {
            ExecutionContext::with_current(|ec| {
                let _ = ec.pop();
            });
        }
        if let Some(previous) = self.check_base {
            ExecutionContext::with_current(|ec| ec.end_isolated_check(previous));
        }
        if self.anonymous {
            user::logout_anonymous_only();
        }
    }
}

/// What the transition run decided: the response was already sent, or this response holds.
enum Step {
    Sent,
    Respond(ResponseDef),
}

fn run_transition(
    factory: &Arc<ExecutionContextFactory>,
    job: &Job<'_>,
    info: Arc<ScreenUrlInfo>,
) -> Result<(), Error> {
    let Some(transition) = info.transition(Some(job.method)).cloned() else {
        let name = info.target_transition_name.clone().unwrap_or_default();
        let allow: Vec<String> = info
            .target_screen
            .transitions
            .iter()
            .filter(|item| item.name == name)
            .map(|item| item.method.to_ascii_uppercase())
            .collect();
        web::send_json_error(405, &format!("Method {} is not supported", job.method));
        web::add_header("Allow", allow.join(", "));
        return Ok(());
    };
    if job.enforce_request_checks {
        if let Some(reason) = insecure_transition(factory, job, &transition, &info.inline_parameters) {
            web::send_json_error(400, reason);
            return Ok(());
        }
        if !job.header_authenticated && !job.new_session && csrf_rejected(job, &transition)? {
            web::send_json_error(401, "Session token is missing or is not valid");
            return Ok(());
        }
        // a cookie session that presented its token: a person at a browser, who may decide an agent's approval
        if !job.header_authenticated && session_token_presented(job)? {
            gate::note_session_token_checked();
        }
    }

    let observer = gate::observer().filter(|observer| {
        let user_id = user::current_user_id().ok().flatten();
        observer.wants(user_id.as_deref())
    });
    let gated = !transition.read_only && runs_actions(&transition);
    let held = gated.then(|| held_write(job, &info, &transition)).flatten();
    let result = match &held {
        Some((write, mismatch)) => {
            send_held(write, mismatch.as_deref());
            Ok(())
        }
        None => {
            // An approved write runs outside the render's read fence; any other stays inside it.
            let _lifted = (gated && matches!(job.options.gate, TransitionGate::Approved { .. }))
                .then(crate::entity::fence::lift);
            run_gated_transition(factory, job, &info, &transition)
        }
    };
    if let Some(observer) = observer {
        observer.observed(&transition_outcome(job, &info, &transition, held.is_some(), &result));
    }
    result
}

/// The write the gate holds, with the approval that did not match it: `None` when the transition may run.
fn held_write(job: &Job<'_>, info: &ScreenUrlInfo, transition: &TransitionDef) -> Option<(HeldWrite, Option<String>)> {
    let approved = match &job.options.gate {
        TransitionGate::Open => return None,
        TransitionGate::Hold => None,
        TransitionGate::Approved { approval_id, args_hash } => Some((approval_id, args_hash)),
    };
    let write = HeldWrite {
        screen_location: transition.screen_location.clone(),
        transition: transition.name.clone(),
        service: transition.single_service.clone(),
        authz_action: transition_authz_action(transition),
        method: job.method.to_ascii_uppercase(),
        path: info.full_path_url(),
        extra_path: info.extra_path.clone(),
        path_parameters: transition
            .path_parameters
            .iter()
            .zip(info.extra_path.iter())
            .map(|(name, value)| (name.clone(), serde_json::Value::String(value.clone())))
            .collect(),
        inline_parameters: gate::gate_inline_parameters(&info.inline_parameters),
        parameters: gate::gate_parameters(job.parameters),
    };
    match approved {
        Some((_, args_hash)) if constant_time_eq(write.hash().as_bytes(), args_hash.as_bytes()) => None,
        Some((approval_id, _)) => {
            tracing::warn!(
                "the write {}#{} does not match approval {approval_id}, so it is held",
                write.screen_location,
                write.transition
            );
            Some((write, Some(approval_id.clone())))
        }
        None => Some((write, None)),
    }
}

/// The 409 answer of a held write: `{"heldWrite": {...}}`, with `approvalMismatch` when an approval did not match.
fn send_held(write: &HeldWrite, mismatch: Option<&str>) {
    let mut body = serde_json::json!({"heldWrite": write.to_json()});
    if let Some(approval_id) = mismatch {
        body["approvalMismatch"] = serde_json::json!({"approvalId": approval_id});
    }
    web::send_bytes_with_headers(
        409,
        vec![("Content-Type".to_string(), "application/json; charset=UTF-8".to_string())],
        body.to_string().into_bytes(),
    );
}

/// What the observer is told about a transition.
fn transition_outcome(
    job: &Job<'_>,
    info: &ScreenUrlInfo,
    transition: &TransitionDef,
    held: bool,
    result: &Result<(), Error>,
) -> TransitionOutcome {
    let (errors, validation_errors, messages) = ExecutionContext::with_current(|ec| {
        let message = ec.message();
        (
            message.errors().to_vec(),
            message
                .validation_errors()
                .iter()
                .map(|error| format!("{}: {}", error.field, error.message))
                .collect(),
            message.messages().to_vec(),
        )
    })
    .unwrap_or_default();
    TransitionOutcome {
        origin: job.options.origin.clone(),
        user_id: user::current_user_id().ok().flatten(),
        screen_location: transition.screen_location.clone(),
        transition: transition.name.clone(),
        method: job.method.to_ascii_uppercase(),
        parameters: gate::gate_parameters(job.parameters),
        extra_path: info.extra_path.clone(),
        inline_parameters: gate::gate_inline_parameters(&info.inline_parameters),
        held,
        header_authenticated: (job.header_authenticated && job.options.origin == RenderOrigin::Browser)
            || user::header_login_session(),
        status: if web::output_sent() { web::output_status() } else { 200 },
        location: web::response_header_values("Location").into_iter().next(),
        error: result.as_ref().err().map(|error| error.message().to_string()),
        errors,
        validation_errors,
        messages,
    }
}

/// The transition past the gate: its path's always-actions, its actions and its response, in one transaction.
fn run_gated_transition(
    factory: &Arc<ExecutionContextFactory>,
    job: &Job<'_>,
    info: &Arc<ScreenUrlInfo>,
    transition: &TransitionDef,
) -> Result<(), Error> {
    let mode = DEFAULT_MODE.to_string();
    let facade = ScreenFacade::of(factory);
    let (content_type, encoding) = output_type(&facade, info, &mode);
    let state = Rc::new(RenderState::new(
        factory,
        &facade,
        Arc::clone(info),
        mode,
        content_type,
        encoding,
    ));
    let _active = ActiveGuard::install(Rc::clone(&state));
    let json = wants_json(job, info);
    // The REST screen's transitions and the implicit actions transition manage their own transactions. Any other
    // transition runs the always-actions of its path, its actions and its responses' conditions in one, as Java's
    // ScreenRenderImpl does, so a write in an always-action rolls back with the transition.
    let own_transaction =
        transition.implicit == super::Implicit::Actions || transition.screen_location.ends_with("/webroot/rest.xml");
    let begin = transition.begin_transaction && !own_transaction;
    let step = in_transaction(begin, None, || run_transition_steps(job, info, &state, transition))?;
    match step {
        Step::Sent => Ok(()),
        Step::Respond(response) => respond(factory, info, &response, json),
    }
}

/// What `formSelectColumns` goes back to the screen with: the request parameters without the column dialog's own.
const SELECT_COLUMNS_PARAMETERS: &str = "(() => { const p = Object.assign({}, ec.web.parameters); \
for (const k of ['columnsTree', 'formLocation', 'ResetColumns', 'SaveColumns', '_uiType', 'moquiSessionToken']) delete p[k]; \
return p; })()";

/// A redirect to `url` for an automatic transition.
fn implicit_response(url: &str) -> ResponseDef {
    ResponseDef {
        kind: ResponseKind::Url,
        url: url.to_string(),
        url_type: "screen-path".to_string(),
        condition: None,
        parameters: Vec::new(),
        parameter_map: None,
        save_current_screen: false,
        save_parameters: false,
    }
}

fn run_transition_steps(
    job: &Job<'_>,
    info: &Arc<ScreenUrlInfo>,
    state: &Rc<RenderState>,
    transition: &TransitionDef,
) -> Result<Step, Error> {
    // Screens above the transition are only on the stack, so they inherit but are not decided; a screen limited to a
    // user group the user is not in refuses its transitions as it refuses its render.
    path_items_available(info, info.path_defs.len())?;
    let mut frames = Frames::default();
    for (index, def) in info.path_defs.iter().enumerate() {
        frames.push_screen(def, false)?;
        frames.apply_anonymous(def)?;
        state.screen_path_index.set(index);
        if let Some(actions) = &def.always_actions {
            script::run_actions(&actions.xml)?;
        }
    }
    let screen = Arc::clone(&info.target_screen);
    frames.push(
        "AT_XML_SCREEN_TRANS",
        &format!("{}/{}", screen.location, transition.name),
        &transition_authz_action(transition),
        screen.authentication == Authentication::Required,
    )?;
    frames.hit_detail(Some(&state.content_type), true);
    let hit = TransitionHit::start(&screen, transition, job.parameters);

    set_all_parameters(info, transition)?;
    // After the path parameters and `from` values are in, it is too late to supply a required parameter.
    if let Some(name) = missing_parameter(transition) {
        web::send_json_error(400, &format!("Missing required parameter {name}"));
        return Ok(Step::Sent);
    }
    if let Some(condition) = &transition.condition {
        if !check_condition(condition)? {
            ExecutionContext::with_current(|ec| {
                ec.message().add_error(format!(
                    "Condition failed for transition {}/{}, not running actions or redirecting",
                    screen.location, transition.name
                ));
            });
            let response = transition
                .error_response
                .as_ref()
                .unwrap_or(&transition.default_response);
            hit.count(response_type(response));
            return Ok(Step::Respond(response.clone()));
        }
    }

    match transition.implicit {
        super::Implicit::Actions => {
            in_transaction(transition.begin_transaction, None, || {
                super::statics::run_actions_transition(state)
            })?;
            hit.count("");
            return Ok(Step::Sent);
        }
        super::Implicit::FormSaveFind => {
            hit.count("url");
            // The screen with only the saved find, which the render applies; else the screen as it was.
            let shown = super::saved_finds::process_saved_find(&transition.screen_location)?;
            let mut response = implicit_response(".");
            if let Some(id) = shown {
                response.parameters.push(ParameterDef {
                    name: "formListFindId".to_string(),
                    required: false,
                    from: None,
                    value: Some(super::saved_finds::template_literal(&id)),
                });
            }
            return Ok(Step::Respond(response));
        }
        super::Implicit::FormSelectColumns => {
            super::saved_finds::save_form_config(&transition.screen_location)?;
            hit.count("url");
            let mut response = implicit_response(".");
            response.parameter_map = Some(SELECT_COLUMNS_PARAMETERS.to_string());
            return Ok(Step::Respond(response));
        }
        super::Implicit::ScreenDoc => {
            // The terminal asks with renderMode=tui (or screenDoc.tui) and reads the document as an html node.
            let tui = text_of(job.parameters.get("renderMode")) == "tui"
                || info.target_transition_extension.as_deref() == Some("tui");
            match super::saved_finds::screen_document(&transition.screen_location)? {
                super::saved_finds::ScreenDocument::Text(text) if tui => {
                    let mut node = super::html_text::html_text(&text).to_json();
                    node["t"] = "html".into();
                    node["id"] = "screenDoc".into();
                    web::send_bytes_with_headers(
                        200,
                        vec![(
                            "Content-Type".to_string(),
                            "application/json; charset=UTF-8".to_string(),
                        )],
                        node.to_string().into_bytes(),
                    );
                }
                super::saved_finds::ScreenDocument::Text(text) => web::send_bytes_with_headers(
                    200,
                    vec![("Content-Type".to_string(), "text/html; charset=UTF-8".to_string())],
                    text.into_bytes(),
                ),
                super::saved_finds::ScreenDocument::Missing(message) => web::send_json_error(404, &message),
            }
            hit.count("");
            return Ok(Step::Sent);
        }
        super::Implicit::None => {}
    }
    let wired = dispatch::run(&dispatch::Call {
        request: job.request,
        transition,
        extra: &info.extra_path,
        parameters: job.parameters,
        json_list: job.json_list,
        login_error: job.login_error,
        started: job.started,
    })?;
    if wired.is_some() {
        hit.count("");
        return Ok(Step::Sent);
    }
    if let Some(message) = &transition.actions_error {
        return Err(Error::new(message.clone()));
    }
    reject_html_for_entity_auto(transition);
    // The service-call runs only when there is no error and no validation error. `<actions>` always
    // run, so they can handle an error (ScreenDefinition.groovy). The two are separate scripts, so
    // the call's `if (ec.message.hasError()) return` cannot skip the actions.
    if let Some(actions) = &transition.service_actions {
        if !screen_has_error() {
            run_transition_actions(actions)?;
        }
    }
    if let Some(actions) = &transition.actions {
        run_transition_actions(actions)?;
    }
    let response = pick_response(transition)?;
    hit.count(response_type(response));
    Ok(Step::Respond(response.clone()))
}

/// The authorization action of a transition: its `authz-action`, else that of its single service (the service's own, else
/// by its verb), else view for a read-only transition and update otherwise (Java's `resolveAuthzAction`).
pub(super) fn transition_authz_action(transition: &TransitionDef) -> String {
    if !transition.authz_explicit {
        if let Some(service) = &transition.single_service {
            return crate::service::call::service_authz_action(service);
        }
    }
    transition.authz_action.clone()
}

/// Whether a transition runs something that can change data, as Java's `hasActionsOrSingleService`: its actions (or
/// actions that did not compile), or the implicit saved find and column transitions, which write rows.
fn runs_actions(transition: &TransitionDef) -> bool {
    transition.service_actions.is_some()
        || transition.actions.is_some()
        || transition.actions_error.is_some()
        || matches!(
            transition.implicit,
            super::Implicit::FormSaveFind | super::Implicit::FormSelectColumns
        )
}

/// A transition that writes is refused from a request that is not encrypted when https is on, or that carries its
/// parameters in the URL, where they are not protected (Java's ScreenRenderImpl). A read-only one is not.
/// `inline_parameters` are the `/~name=value/` segments. A `path-parameter` extra path is not one of those.
fn insecure_transition(
    factory: &ExecutionContextFactory,
    job: &Job<'_>,
    transition: &TransitionDef,
    inline_parameters: &BTreeMap<String, String>,
) -> Option<&'static str> {
    if transition.read_only || !runs_actions(transition) {
        return None;
    }
    if !job.request.https && factory.webapp_https(WEBAPP).is_some() {
        return Some(
            "Cannot run screen transition with actions from a non-secure request; use https or declare the transition read-only",
        );
    }
    let query = job
        .request
        .query
        .iter()
        .any(|(name, _)| !matches!(name.as_str(), "lastStandalone" | "renderMode"));
    // Java also refuses when `getPathInfoParameterMap` is non-empty. `lastStandalone` and `renderMode` stay
    // allowed on the query: screen links put them there for CSV, XLS, print, and text export.
    if query || !inline_parameters.is_empty() {
        return Some(
            "Cannot run screen transition with actions with URL parameters; send them in the body of a form or declare the transition read-only",
        );
    }
    None
}

/// An implicit entity-auto service filters its input for HTML as a defined service does by default: a field value with
/// `<` is a validation error (Java's TransitionItem.run). A service with a parameter that allows HTML is the way around it.
fn reject_html_for_entity_auto(transition: &TransitionDef) {
    let Some(service) = &transition.single_service else {
        return;
    };
    let Some(entity_name) = crate::service::call::entity_auto_noun(service) else {
        return;
    };
    let Ok(shape) = super::form::entity_shape(&entity_name) else {
        return;
    };
    for (field, _, _) in &shape.fields {
        if let Some(Value::String(text)) = script::context_value(field) {
            if text.contains('<') {
                ExecutionContext::with_current(|ec| {
                    ec.message().add_validation_error(
                        "",
                        field,
                        service,
                        &crate::l10n::localize(
                            "HTML not allowed including less-than (<), greater-than (>), etc symbols",
                        )
                        .unwrap_or_else(|_| "HTML not allowed".to_string()),
                    );
                });
            }
        }
    }
}

/// The `AT_XML_SCREEN_TRANS` hit Java's `recursiveRunTransition` counts once the actions ran, named
/// `screen#transition` with the request parameters, besides the hit of the transition's frame.
struct TransitionHit<'a> {
    name: String,
    track: bool,
    parameters: &'a BTreeMap<String, Value>,
    start_ms: i64,
    started: Instant,
}

impl<'a> TransitionHit<'a> {
    fn start(screen: &ScreenDef, transition: &TransitionDef, parameters: &'a BTreeMap<String, Value>) -> Self {
        Self {
            name: format!("{}#{}", screen.location, transition.name),
            track: screen.track_artifact_hit,
            parameters,
            start_ms: user::now_millis(),
            started: Instant::now(),
        }
    }

    fn count(&self, response_type: &str) {
        if !self.track {
            return;
        }
        ExecutionContext::with_current(|ec| {
            ec.count_artifact_hit(&crate::context::Hit {
                artifact_type: "AT_XML_SCREEN_TRANS",
                sub_type: Some(response_type),
                name: &self.name,
                parameters: Some(self.parameters),
                start_ms: self.start_ms,
                running_ms: self.started.elapsed().as_secs_f64() * 1000.0,
                output_size: None,
            });
        });
    }
}

/// The response `type`, Java's `ResponseItem.type`.
fn response_type(response: &ResponseDef) -> &'static str {
    match response.kind {
        ResponseKind::None => "none",
        ResponseKind::Url => "url",
        ResponseKind::ScreenLast => "screen-last",
        ResponseKind::ScreenLastNoParam => "screen-last-noparam",
    }
}

/// A transition asked for with `.json` or by a client that accepts JSON (the qvt `m-form`) answers a redirect as JSON, as Java's
/// `sendJsonRedirect`.
/// A `.json` transition, an `Accept: application/json` request, or a `tui` one: the terminal client asks for each screen
/// as its own request with no session between them, so a link to a transition (`runJob`) gets its redirect as JSON,
/// messages and all, rather than a Location whose flash messages the next request would never see.
fn wants_json(job: &Job<'_>, info: &ScreenUrlInfo) -> bool {
    info.target_transition_extension.as_deref() == Some("json")
        || requested_mode(job, Some(info)).as_deref() == Some("tui")
        || job
            .request
            .header("accept")
            .is_some_and(|accept| accept.contains("application/json"))
}

fn run_transition_actions(actions: &Actions) -> Result<(), Error> {
    script::run_actions(&actions.xml)
}

/// Java's `messageFacade.hasError()` on the screen path: the error list or a validation error.
/// `service::util::has_error` and the script's `ec.message.hasError` stay the error list only.
fn screen_has_error() -> bool {
    ExecutionContext::with_current(|ctx| {
        let messages = ctx.message();
        !messages.errors().is_empty() || !messages.validation_errors().is_empty()
    })
    .unwrap_or(false)
}

/// The error response when there are errors, else the last conditional response that holds (Java checks them all
/// and keeps the last), else the default response.
fn pick_response(transition: &TransitionDef) -> Result<&ResponseDef, Error> {
    if screen_has_error() {
        if let Some(response) = &transition.error_response {
            return Ok(response);
        }
    }
    let mut chosen = None;
    for response in &transition.conditional_responses {
        let holds = match &response.condition {
            Some(condition) => check_condition(condition)?,
            None => true,
        };
        if holds {
            chosen = Some(response);
        }
    }
    Ok(chosen.unwrap_or(&transition.default_response))
}

/// Bind the extra path names to the transition's `path-parameter` names, then evaluate the
/// screen and transition `parameter` entries into the context.
fn set_all_parameters(info: &ScreenUrlInfo, transition: &TransitionDef) -> Result<(), Error> {
    for (name, value) in transition.path_parameters.iter().zip(info.extra_path.iter()) {
        script::set_context_name(name, Value::string(value))?;
    }
    for parameter in info.target_screen.parameters.iter().chain(transition.parameters.iter()) {
        if let Some(value) = parameter_value(parameter)? {
            script::set_context_name(&parameter.name, value)?;
        }
    }
    Ok(())
}

pub(super) fn parameter_value(parameter: &ParameterDef) -> Result<Option<Value>, Error> {
    if let Some(from) = &parameter.from {
        return Ok(Some(script::eval_value(from)?));
    }
    if let Some(value) = &parameter.value {
        return Ok(Some(Value::string(script::expand(value)?)));
    }
    Ok(script::context_value(&parameter.name).or_else(|| web::parameter(&parameter.name)))
}

fn respond(
    factory: &ExecutionContextFactory,
    info: &ScreenUrlInfo,
    response: &ResponseDef,
    json: bool,
) -> Result<(), Error> {
    if response.save_current_screen {
        // Java's `saveScreenLastInfo(full path, null)`: the target a later `screen-last` returns to,
        // with the request parameters. Screen history is a different list (`record_history`).
        let path = info.full_path_url();
        user::session::save_screen_last(&path, &plain_request_parameters());
    }
    if response.kind == ResponseKind::None {
        if response.save_parameters {
            save_parameters();
        }
        if screen_has_error() {
            save_error_parameters();
        }
        return Ok(());
    }
    if response.save_parameters {
        save_parameters();
    }
    // A JSON answer carries the messages itself, so they are not kept for the next page as well.
    if !json {
        save_messages();
    }
    if screen_has_error() {
        save_error_parameters();
    }
    let mut url = script::expand(&response.url)?;
    let mut url_type = response.url_type.clone();
    let mut carried = BTreeMap::new();
    if matches!(
        response.kind,
        ResponseKind::ScreenLast | ResponseKind::ScreenLastNoParam
    ) {
        let with_parameters = response.kind == ResponseKind::ScreenLast;
        url_type = "screen-path".to_string();
        if let Some(saved) = user::session::take_screen_last_path() {
            // The saved screen, such as the page asked for before login, as long as it is on this site.
            if saved.starts_with("http://") || saved.starts_with("https://") {
                if same_origin(&saved) {
                    url_type = "plain".to_string();
                    url = saved;
                } else {
                    tracing::warn!("Ignoring off-site screen-last path [{saved}], redirecting to /");
                    url = "/".to_string();
                }
            } else if saved.starts_with('/') && !saved.starts_with("//") {
                url = saved;
            } else {
                url = "/".to_string();
            }
            user::session::remove_screen_last_parameters(with_parameters);
        } else {
            // The newest history entry, with its query or without, else the root, so a login never ends up stuck on itself.
            url = user::screen_history()
                .ok()
                .and_then(|history| history.first().cloned())
                .filter(|path| path.starts_with('/') && !path.starts_with("//"))
                .map(|path| {
                    if with_parameters {
                        path
                    } else {
                        path.split('?').next().unwrap_or("/").to_string()
                    }
                })
                .unwrap_or_else(|| "/".to_string());
        }
        if with_parameters {
            for (name, value) in user::saved_parameters().unwrap_or_default() {
                carried.insert(name, value);
            }
        }
    }
    let mut parameters = carried;
    for parameter in &response.parameters {
        if let Some(value) = parameter_value(parameter)? {
            parameters.insert(parameter.name.clone(), moqui_lib::object::to_plain_string(&value));
        }
    }
    if let Some(expression) = &response.parameter_map {
        for (name, value) in script::eval_map(expression)? {
            parameters.insert(name, moqui_lib::object::to_plain_string(&value));
        }
    }
    let mut screen_path = None;
    // An http(s) value is a plain URL even when url-type is screen or transition. Java checks this in getScreenUrlInfo.
    let external = url.starts_with("http:") || url.starts_with("https:");
    let location = if url_type == "plain" || external {
        url
    } else {
        // Keep the page and sort a list was on, so a form submit returns to the same view.
        let request_parameters = web::parameters();
        for name in ["pageIndex", "orderByField"] {
            if let Some(value) = request_parameters.get(name) {
                parameters
                    .entry(name.to_string())
                    .or_insert_with(|| moqui_lib::object::to_plain_string(value));
            }
        }
        let root = info.path_defs.first().map(|def| def.location.as_str()).unwrap_or("");
        let prepared = match resolve_screen_path(factory, root, &info.pre_transition_path, &url) {
            Ok(prepared) => prepared,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                // A missing `//` path is `#`, as Java's getUrlWithBase when the target does not exist.
                let location = "#".to_string();
                if json {
                    send_json_redirect(None, &http_redirect_location(factory, &location), &parameters);
                } else {
                    let location = with_query(location, &parameters);
                    web::send_redirect(&http_redirect_location(factory, &location));
                }
                return Ok(());
            }
            Err(err) => return Err(err),
        };
        // A query or `~` on the response url becomes parameters. Response parameters already in the map win.
        for (name, value) in prepared.inline {
            parameters.entry(name).or_insert(value);
        }
        // The parameters of the screens on the target path from the context, as Java's `UrlInstance.getParameterMap`: a transition of
        // a screen with a required parameter redirects back with it.
        for (name, value) in path_parameters(info, &prepared.names)? {
            parameters.entry(name).or_insert(value);
        }
        let base = web::base_url();
        let location = format!("{}{}", base.trim_end_matches('/'), encode_path(&prepared.names));
        screen_path = Some(prepared.names);
        location
    };
    if json {
        // The client follows the URL in the JSON, so it is held to the same rule as the Location of a
        // redirect: a target that is not a path of this site goes to the root.
        send_json_redirect(screen_path, &http_redirect_location(factory, &location), &parameters);
    } else {
        let location = with_query(location, &parameters);
        web::send_redirect(&http_redirect_location(factory, &location));
    }
    Ok(())
}

/// Whether an absolute url names the host this request came to, as Java's `isSameOriginRedirect`.
fn same_origin(url: &str) -> bool {
    let after = url.split_once("://").map_or("", |(_, rest)| rest);
    let authority = after.split(['/', '?', '#']).next().unwrap_or("");
    let host = web::host_name(true);
    !host.is_empty() && authority.eq_ignore_ascii_case(&host)
}

/// Java's `sendJsonRedirect`: `screenUrl` (the path with its parameters), `screenPathList` and `screenParameters` for a screen, else
/// `redirectUrl`, with the messages and the current parameters. The errors make it a 500, as for any JSON answer.
fn send_json_redirect(screen_path: Option<Vec<String>>, location: &str, parameters: &BTreeMap<String, String>) {
    use moqui_lib::object::Value as V;
    let mut map = indexmap::IndexMap::new();
    match screen_path {
        Some(names) => {
            map.insert(
                "screenPathList".to_string(),
                V::List(names.iter().map(V::string).collect()),
            );
            let mut shown = serde_json::Value::Object(
                parameters
                    .iter()
                    .map(|(name, value)| (name.clone(), serde_json::Value::String(value.clone())))
                    .collect(),
            );
            web::strip_credential_parameters(&mut shown);
            let screen_parameters = shown
                .as_object()
                .map(|object| {
                    object
                        .iter()
                        .map(|(name, value)| (name.clone(), V::string(value.as_str().unwrap_or(""))))
                        .collect()
                })
                .unwrap_or_default();
            map.insert("screenParameters".to_string(), V::Map(screen_parameters));
            map.insert(
                "screenUrl".to_string(),
                V::string(with_query(encode_path(&names), parameters)),
            );
        }
        None => {
            map.insert(
                "redirectUrl".to_string(),
                V::string(with_query(location.to_string(), parameters)),
            );
        }
    }
    let (messages, validation) = ExecutionContext::with_current(|ec| {
        let facade = ec.message();
        let messages: Vec<(String, String)> = facade
            .messages()
            .iter()
            .enumerate()
            .map(|(index, text)| (text.clone(), facade.message_type(index).to_string()))
            .collect();
        (messages, facade.validation_errors().to_vec())
    })
    .unwrap_or_default();
    if !messages.is_empty() {
        map.insert(
            "messageInfos".to_string(),
            V::List(
                messages
                    .iter()
                    .map(|(message, kind)| {
                        let mut info = indexmap::IndexMap::new();
                        info.insert("message".to_string(), V::string(message));
                        info.insert("type".to_string(), V::string(kind));
                        V::Map(info)
                    })
                    .collect(),
            ),
        );
    }
    if !validation.is_empty() {
        map.insert(
            "validationErrors".to_string(),
            V::List(
                validation
                    .iter()
                    .map(|error| {
                        V::Map(
                            validation_map(error)
                                .into_iter()
                                .map(|(key, value)| (key, V::string(value)))
                                .collect(),
                        )
                    })
                    .collect(),
            ),
        );
    }
    let mut current = serde_json::Value::Object(
        web::parameters()
            .into_iter()
            .filter(|(_, value)| !matches!(value, V::Object(_)))
            .map(|(name, value)| {
                (
                    name,
                    serde_json::Value::String(moqui_lib::object::to_plain_string(&value)),
                )
            })
            .collect(),
    );
    web::strip_credential_parameters(&mut current);
    if let Some(object) = current.as_object() {
        map.insert(
            "currentParameters".to_string(),
            V::Map(
                object
                    .iter()
                    .filter(|(name, _)| name.as_str() != "moquiSessionToken")
                    .map(|(name, value)| (name.clone(), V::string(value.as_str().unwrap_or(""))))
                    .collect(),
            ),
        );
    }
    web::send_json_response(V::Map(map));
}

/// The non-empty values of the `parameter` entries of the screens on the path `names`, from the context or the request.
fn path_parameters(info: &ScreenUrlInfo, names: &[String]) -> Result<BTreeMap<String, String>, Error> {
    let mut out = BTreeMap::new();
    let (Some(root), Some(factory)) = (
        info.path_defs.first(),
        ExecutionContext::with_current(|ec| Arc::clone(ec.factory())),
    ) else {
        return Ok(out);
    };
    let target = match ScreenUrlInfo::resolve(&factory, &root.location, names, info.last_standalone) {
        Ok(target) => target,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(out),
        Err(err) => return Err(err),
    };
    for item in target.parameter_items.values() {
        if let Some(value) = parameter_value(item)? {
            let text = moqui_lib::object::to_plain_string(&value);
            if !text.is_empty() {
                out.insert(item.name.clone(), text);
            }
        }
    }
    Ok(out)
}

/// Messages ride the session to the next screen, which shows them, as Java's `saveMessagesToSession`: each message
/// with its type, the errors, and the validation errors for the fields of the form that was submitted.
fn save_messages() {
    let (messages, errors, validation) = ExecutionContext::with_current(|ec| {
        let facade = ec.message();
        let messages: Vec<(String, String)> = facade
            .messages()
            .iter()
            .enumerate()
            .map(|(index, text)| (text.clone(), facade.message_type(index).to_string()))
            .collect();
        (messages, facade.errors().to_vec(), facade.validation_errors().to_vec())
    })
    .unwrap_or_default();
    for (message, kind) in &messages {
        user::session::add_flash_message_typed(message, kind);
    }
    // A validation error with a field is shown by that field; its text is not also an error line at the top.
    let as_text: Vec<String> = validation
        .iter()
        .filter(|error| !error.field.is_empty())
        .map(|error| error.message.clone())
        .collect();
    for message in errors.iter().filter(|message| !as_text.contains(message)) {
        user::session::add_flash_error(message);
    }
    for error in &validation {
        user::session::add_flash_validation_error(&validation_map(error));
    }
}

/// Java's `ValidationError.getMap()`: the names, their `camelCaseToPretty` forms, and the message.
fn validation_map(error: &crate::context::ValidationError) -> BTreeMap<String, String> {
    let pretty = moqui_lib::string::camel_case_to_pretty;
    BTreeMap::from([
        ("form".to_string(), error.form.clone()),
        ("formPretty".to_string(), pretty(&error.form)),
        ("field".to_string(), error.field.clone()),
        ("fieldPretty".to_string(), pretty(&error.field)),
        ("serviceName".to_string(), error.service.clone()),
        ("serviceNamePretty".to_string(), pretty(&error.service)),
        ("message".to_string(), error.message.clone()),
    ])
}

/// The request parameters of a failed submit ride the session so the form shows what was typed. A
/// password or the token is not kept.
fn save_error_parameters() {
    for (name, value) in web::parameters() {
        let lower = name.to_ascii_lowercase();
        if lower.contains("password") || lower == "moquisessiontoken" || matches!(value, Value::Object(_)) {
            continue;
        }
        user::session::set_error_parameter(&name, &moqui_lib::object::to_plain_string(&value));
    }
}

fn save_parameters() {
    for (name, value) in web::parameters() {
        user::session::save_parameter(&name, &moqui_lib::object::to_plain_string(&value));
    }
}

pub(super) fn with_query(mut location: String, parameters: &BTreeMap<String, String>) -> String {
    let mut query = String::new();
    for (name, value) in parameters {
        if value.is_empty() {
            continue;
        }
        if !query.is_empty() {
            query.push('&');
        }
        query.push_str(&encode_component(name));
        query.push('=');
        query.push_str(&encode_component(value));
    }
    if !query.is_empty() {
        location.push(if location.contains('?') { '&' } else { '?' });
        location.push_str(&query);
    }
    // A header cannot carry a line break.
    location.retain(|ch| ch != '\r' && ch != '\n');
    location
}

pub(super) fn encode_component(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Whether `condition` holds in the current context.
pub(crate) fn check_condition(condition: &Condition) -> Result<bool, Error> {
    match condition {
        Condition::Expression(expression) => script::eval_condition(expression),
        Condition::Wrapped(actions) => {
            let _frame = ContextFrame::push();
            script::run_actions(&actions.xml)?;
            Ok(matches!(
                script::context_value(CONDITION_FIELD),
                Some(Value::Bool(true))
            ))
        }
    }
}

/// Run `body`, in a transaction when `begin` is set. The transaction joins one already in place.
/// Errors the actions left in the message facade roll it back without failing the call.
fn in_transaction<T>(begin: bool, timeout: Option<i64>, body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    let facade = if begin {
        ExecutionContext::with_current(|ec| ec.factory().entity()).flatten()
    } else {
        None
    };
    let Some(facade) = facade else {
        return body();
    };
    let tx = facade.transactions_for(crate::service::semaphore_entity_name())?;
    let timeout = timeout
        .and_then(|seconds| u64::try_from(seconds).ok())
        .map(std::time::Duration::from_secs);
    let mut rolled_back = false;
    let mut kept = None;
    let outcome = tx.run(crate::transaction::TransactionAttribute::UseOrBegin, timeout, || {
        let _deadline = script::DeadlineGuard::new(tx.time_remaining());
        kept = Some(body()?);
        if screen_has_error() {
            rolled_back = true;
            tx.set_rollback_only("Errors running screen actions")?;
        }
        Ok(())
    });
    match outcome {
        Err(err) if rolled_back && ErrorKind::of(&err) == ErrorKind::RollbackOnly => {}
        other => other.map_err(Error::from)?,
    }
    kept.ok_or_else(|| Error::new("the transaction body did not finish"))
}

fn missing_parameter(transition: &TransitionDef) -> Option<String> {
    transition
        .required_parameters()
        .into_iter()
        .find(|name| is_empty(script::context_value(name).or_else(|| web::parameter(name)).as_ref()))
        .map(str::to_string)
}

pub(super) fn is_empty(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::String(text)) => text.is_empty(),
        Some(_) => false,
    }
}

fn csrf_rejected(job: &Job<'_>, transition: &TransitionDef) -> Result<bool, Error> {
    if !matches!(
        job.method.to_ascii_lowercase().as_str(),
        "post" | "put" | "patch" | "delete"
    ) {
        return Ok(false);
    }
    // Only a transition that runs something can change data.
    if !runs_actions(transition) {
        return Ok(false);
    }
    let required = match transition.require_session_token {
        Some(flag) => flag,
        None => web::require_session_token(),
    };
    if !required {
        return Ok(false);
    }
    let expected = user::csrf_token()?;
    if expected.is_empty() {
        return Ok(false);
    }
    let presented = text_of(job.parameters.get("moquiSessionToken"));
    let presented = if presented.is_empty() {
        ["moquisessiontoken", "sessiontoken", "x-csrf-token"]
            .iter()
            .find_map(|name| job.request.header(name).filter(|text| !text.is_empty()))
            .unwrap_or("")
            .to_string()
    } else {
        presented
    };
    Ok(!constant_time_eq(presented.as_bytes(), expected.as_bytes()))
}

/// Whether the request presented the session's token (a parameter or a header) and it matches.
fn session_token_presented(job: &Job<'_>) -> Result<bool, Error> {
    let expected = user::csrf_token()?;
    if expected.is_empty() {
        return Ok(false);
    }
    let presented = text_of(job.parameters.get("moquiSessionToken"));
    let presented = if presented.is_empty() {
        ["moquisessiontoken", "sessiontoken", "x-csrf-token"]
            .iter()
            .find_map(|name| job.request.header(name).filter(|text| !text.is_empty()))
            .unwrap_or("")
            .to_string()
    } else {
        presented
    };
    Ok(!presented.is_empty() && constant_time_eq(presented.as_bytes(), expected.as_bytes()))
}

pub(super) fn text_of(value: Option<&Value>) -> String {
    value.map(moqui_lib::object::to_plain_string).unwrap_or_default()
}
