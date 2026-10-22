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

//! The in-process [`ScreenLink`](moqui_tui::ScreenLink) (plans/archive/Screen-TUI-Impl.md 2.7, 4.1). Each call runs on the
//! factory's request pool in a fresh execution context, logged in as the link's user, as the SSH shell's commands are;
//! the client holds no permission logic. Until a user is logged in, every call answers [`LinkError::Auth`], so the client
//! shows its login dialog. The link holds the factory weakly, as the SSH tasks do: a session over SSH never keeps a
//! stopped server alive, and its calls then answer that the server is stopping.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use moqui_core::context::{Error, ErrorKind, ExecutionContextFactory};
use moqui_core::screen::{RenderOptions, RenderOrigin, RenderOutput};
use moqui_core::user::CodeLogin;
use moqui_lib::object::Value;
use moqui_tui::{
    counts_of, query_parameters, Choice, CodeAsk, LinkError, MenuItem, Messages, Push, PushSink, RawBody, ScreenLink,
    ScreenModel, Sendable, ShellAction, ShellInfo, Subscription, TransitionOutcome, ValidationError,
};

/// Redirects a level or a fragment follows before it gives up: a transition answers its screen with one.
const FRAGMENT_REDIRECTS: usize = 4;

/// One request of a link, as its transport sends it.
pub struct Ask {
    /// A path on the server, with no query.
    pub path: String,
    /// `GET` or `POST`.
    pub method: String,
    pub parameters: BTreeMap<String, String>,
    /// `Accept: application/json`: a transition answers JSON.
    pub json: bool,
    /// Rendered in the `tui` mode.
    pub tui: bool,
    /// A screen the user goes to, kept in the history; a fragment, options or a file are not.
    pub remember: bool,
}

/// How a [`Link`] reaches the server: in this process ([`InProcess`]) or over HTTP ([`crate::tui_remote::Http`]). Every
/// answer is a [`RenderOutput`], so the link reads both the same way.
pub trait Transport: Send + Sync {
    /// # Errors
    ///
    /// [`LinkError::Auth`] when no user is logged in, [`LinkError::Remote`] when the request cannot be made.
    fn send(&self, ask: Ask) -> Result<RenderOutput, LinkError>;

    /// # Errors
    ///
    /// [`LinkError::Remote`] with the reason the login was refused.
    fn login(&self, username: &str, password: &str, code: &str) -> Result<(), LinkError>;

    /// Send a second factor's code to `factor` for a login the password starts; the answer says where it went.
    ///
    /// # Errors
    ///
    /// [`LinkError::Remote`] with the reason no code was sent.
    fn send_code(&self, _username: &str, _password: &str, _factor: &str) -> Result<String, LinkError> {
        Err(LinkError::Remote("this connection does not send codes".into()))
    }

    /// The logged-in user's name, for the title line.
    fn user(&self) -> Option<String>;

    /// Pushed notifications for the logged-in user, until the answer drops; none when this transport cannot push.
    fn subscribe(&self, _sink: PushSink) -> Option<Subscription> {
        None
    }
}

/// The client's [`ScreenLink`] over a [`Transport`].
#[derive(Clone)]
pub struct Link<T> {
    transport: T,
    /// The path of the last level that rendered, used to resolve a relative transition or options url.
    last_path: Arc<Mutex<String>>,
}

/// The link in this process.
pub type HostLink = Link<InProcess>;

/// Each call on the factory's request pool, in a fresh execution context logged in as the link's user.
#[derive(Clone)]
pub struct InProcess {
    factory: Weak<ExecutionContextFactory>,
    /// The user the calls run as, once logged in.
    user: Arc<Mutex<Option<String>>>,
    /// That user's id, known after the first call: the notifications a push takes are routed by it.
    user_id: Arc<Mutex<Option<String>>>,
    /// The client's address (the SSH peer, the browser terminal's request), which a login checks `ipAllowed`
    /// against; empty checks nothing, as for a login outside a request.
    client_ip: Arc<Mutex<String>>,
    /// The origin, transition gate and read fence of each render (plans/LLM-A2A-Assist.md 4.7): a person's terminal,
    /// unless the owner of the link says otherwise (`agent-apps`, an agent's screen session).
    options: Arc<Mutex<RenderOptions>>,
    /// The last 409 answer of a gated render that held a write (`{"heldWrite": ..., "approvalMismatch"?: ...}`), until
    /// its owner takes it.
    held: Arc<Mutex<Option<serde_json::Value>>>,
}

impl<T: Transport> Link<T> {
    pub fn over(transport: T) -> Self {
        Self {
            transport,
            last_path: Arc::new(Mutex::new(String::new())),
        }
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// The drawer for `path`, from the `menuData` transition.
    ///
    /// # Errors
    ///
    /// Returns [`LinkError::Remote`] when the menu request fails or the body is not a JSON array.
    pub fn menu(&self, path: &str) -> Result<Vec<MenuItem>, LinkError> {
        let output = self.transport.send(Ask::get(menu_url(path)))?;
        menu_from(&output)
    }

    fn absolute(&self, url: &str) -> String {
        let base = self.last_path.lock().unwrap_or_else(PoisonError::into_inner).clone();
        resolve_url(&base, url)
    }

    /// `url` resolved against the level that rendered last, as each request resolves it.
    pub fn resolve(&self, url: &str) -> String {
        self.absolute(url)
    }

    /// Render `path` in the `tui` mode. A screen the user goes to is remembered for the history and as the base of
    /// relative urls; a fragment is neither.
    fn render_tui(
        &self,
        path: String,
        parameters: &BTreeMap<String, String>,
        remember: bool,
    ) -> Result<RenderOutput, LinkError> {
        self.transport.send(Ask {
            path,
            method: "GET".to_string(),
            parameters: parameters.clone(),
            json: false,
            tui: true,
            remember,
        })
    }

    /// `url` (a query included) with `parameters` over it, `method` or else `default`.
    fn ask(&self, url: &str, method: &str, default: &str, parameters: &BTreeMap<String, String>, json: bool) -> Ask {
        let url = self.absolute(url);
        let (path, query) = split_query(&url);
        let mut asked = query.map(query_parameters).unwrap_or_default();
        asked.extend(parameters.clone());
        Ask {
            path: path.to_string(),
            method: if method.is_empty() {
                default.to_string()
            } else {
                method.to_ascii_uppercase()
            },
            parameters: asked,
            json,
            tui: false,
            remember: false,
        }
    }
}

impl Ask {
    fn get(path: String) -> Self {
        Self {
            path,
            method: "GET".to_string(),
            parameters: BTreeMap::new(),
            json: false,
            tui: false,
            remember: false,
        }
    }
}

impl HostLink {
    /// A link with no user: the client logs in through its dialog first.
    pub fn new(factory: &Arc<ExecutionContextFactory>) -> Self {
        Self::weak(Arc::downgrade(factory))
    }

    /// A link with no user over a factory the caller holds weakly.
    pub fn weak(factory: Weak<ExecutionContextFactory>) -> Self {
        Link::over(InProcess {
            factory,
            user: Arc::new(Mutex::new(None)),
            user_id: Arc::new(Mutex::new(None)),
            client_ip: Arc::new(Mutex::new(String::new())),
            options: Arc::new(Mutex::new(RenderOptions {
                origin: RenderOrigin::Terminal,
                ..RenderOptions::default()
            })),
            held: Arc::new(Mutex::new(None)),
        })
    }

    /// The write the transition gate held on the last request, if it held one: the 409 body, `heldWrite` with its
    /// `argsHash`, and `approvalMismatch` when an approval did not match. Taken once.
    pub fn take_held(&self) -> Option<serde_json::Value> {
        self.transport
            .held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// The link with these render options from now on.
    #[must_use]
    pub fn with_render_options(self, options: RenderOptions) -> Self {
        self.set_render_options(options);
        self
    }

    /// A link of the same user, with the same last level to resolve urls against, whose renders take options of their
    /// own: an agent's session sends its lookups (options, values, fragments) through one that runs them fenced, while
    /// its presses go through one that holds writes.
    #[must_use]
    pub fn sibling(&self, options: RenderOptions) -> Self {
        Self {
            transport: InProcess {
                options: Arc::new(Mutex::new(options)),
                held: Arc::new(Mutex::new(None)),
                ..self.transport.clone()
            },
            last_path: Arc::clone(&self.last_path),
        }
    }

    /// The render options of the next calls: an agent's screen session sets the gate for each command.
    pub fn set_render_options(&self, options: RenderOptions) {
        *self.transport.options.lock().unwrap_or_else(PoisonError::into_inner) = options;
    }

    pub fn render_options(&self) -> RenderOptions {
        self.transport
            .options
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The address the dialog's login is checked against (`ipAllowed`).
    #[must_use]
    pub fn from_address(self, client_ip: impl Into<String>) -> Self {
        *self.transport.client_ip.lock().unwrap_or_else(PoisonError::into_inner) = client_ip.into();
        self
    }

    /// A link already logged in as `username`: `moqui tui --user` on a dev instance, and the tests.
    pub fn as_user(factory: &Arc<ExecutionContextFactory>, username: impl Into<String>) -> Self {
        Self::weak_as_user(Arc::downgrade(factory), username)
    }

    /// [`Self::as_user`] over a factory held weakly: an SSH session, logged in by the SSH server.
    pub fn weak_as_user(factory: Weak<ExecutionContextFactory>, username: impl Into<String>) -> Self {
        let link = Self::weak(factory);
        *link.transport.user.lock().unwrap_or_else(PoisonError::into_inner) = Some(username.into());
        link
    }
}

impl InProcess {
    /// The factory, or [`LinkError::Remote`] once the server stopped.
    fn factory(&self) -> Result<Arc<ExecutionContextFactory>, LinkError> {
        self.factory
            .upgrade()
            .ok_or_else(|| LinkError::Remote("the server is stopping".into()))
    }

    /// Run `work` on the request pool with no user, as a login does, waiting for its answer.
    fn on_pool_anonymous<R, F>(&self, work: F) -> Result<R, LinkError>
    where
        R: Send + 'static,
        F: FnOnce() -> Result<R, Error> + Send + 'static,
    {
        let factory = self.factory()?;
        let client_ip = self.client_ip.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let rx = factory
            .request_pool()
            .try_submit(Arc::clone(&factory), move || {
                moqui_core::user::with_client_ip(&client_ip, work)
            })
            .map_err(link_error)?;
        match rx.recv() {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(err)) => Err(LinkError::Remote(err.message().to_string())),
            Err(_) => Err(LinkError::Remote("request worker dropped the result".into())),
        }
    }

    /// Run `work` on the request pool as the link's user, waiting for its answer. The client calls this off its UI
    /// thread, so the wait holds a worker of the client's, never the keys.
    fn on_pool<R, F>(&self, work: F) -> Result<R, LinkError>
    where
        R: Send + 'static,
        F: FnOnce() -> Result<R, Error> + Send + 'static,
    {
        let Some(username) = self.user() else {
            return Err(LinkError::Auth("Authentication required".into()));
        };
        let factory = self.factory()?;
        let user_id = Arc::clone(&self.user_id);
        let rx = factory
            .request_pool()
            .try_submit(Arc::clone(&factory), move || {
                moqui_core::user::internal_login_user(&username)?;
                let id = moqui_core::user::current_user_id()?;
                *user_id.lock().unwrap_or_else(PoisonError::into_inner) = id;
                work()
            })
            .map_err(link_error)?;
        match rx.recv() {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(err)) => Err(link_error(err)),
            Err(_) => Err(LinkError::Remote("request worker dropped the result".into())),
        }
    }
}

impl Transport for InProcess {
    fn send(&self, ask: Ask) -> Result<RenderOutput, LinkError> {
        let factory = self.factory()?;
        let values = value_map(&ask.parameters);
        let options = self.options.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let gated = options.gate != moqui_core::screen::TransitionGate::Open;
        let output = self.on_pool(move || {
            let mut render = moqui_core::screen::make_render(&factory)
                .screen_path(ask.path)
                .method(ask.method)
                .parameters(values)
                .options(options);
            if ask.json {
                render = render.header("Accept", "application/json");
            }
            if ask.tui {
                render = render.render_mode("tui").history(ask.remember);
            }
            render.render()
        })?;
        if gated && output.status == 409 {
            if let Ok(body) = serde_json::from_str::<serde_json::Value>(&output.body_text()) {
                if body.get("heldWrite").is_some() {
                    *self.held.lock().unwrap_or_else(PoisonError::into_inner) = Some(body);
                }
            }
        }
        Ok(output)
    }

    /// The password, and for an account with a second factor the code, in one request: each call is its own context,
    /// so nothing is left pre-authenticated between the dialog's tries.
    fn login(&self, username: &str, password: &str, code: &str) -> Result<(), LinkError> {
        let (name, secret, code) = (username.to_string(), password.to_string(), code.to_string());
        let answer = self.on_pool_anonymous(move || moqui_core::user::login_with_code(&name, &secret, &code))?;
        match answer {
            CodeLogin::LoggedIn => {
                *self.user.lock().unwrap_or_else(PoisonError::into_inner) = Some(username.to_string());
                Ok(())
            }
            CodeLogin::CodeNeeded { text, sendable } => Err(LinkError::Code(CodeAsk {
                text,
                sendable: sendable
                    .into_iter()
                    .map(|factor| Sendable {
                        id: factor.factor_id,
                        label: factor.label,
                    })
                    .collect(),
            })),
        }
    }

    fn send_code(&self, username: &str, password: &str, factor: &str) -> Result<String, LinkError> {
        let (name, secret, id) = (username.to_string(), password.to_string(), factor.to_string());
        self.on_pool_anonymous(move || moqui_core::user::send_login_code(&name, &secret, &id))?;
        Ok(factor.to_string())
    }

    fn user(&self) -> Option<String> {
        self.user.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The notification topic of this process, filtered to the link's user as `/notws` filters a socket's.
    fn subscribe(&self, sink: PushSink) -> Option<Subscription> {
        let user_id = Arc::clone(&self.user_id);
        let listener = move |message: &moqui_core::notify::TopicMessage| {
            let Some(me) = user_id.lock().unwrap_or_else(PoisonError::into_inner).clone() else {
                return;
            };
            let routed = message
                .body
                .get(moqui_core::notify::ROUTE_USER_IDS)
                .and_then(serde_json::Value::as_array)
                .is_some_and(|users| users.iter().any(|user| user.as_str() == Some(me.as_str())));
            if routed {
                sink(push_of(&message.body));
            }
        };
        match moqui_core::notify::subscribe(moqui_core::notify::NOTIFICATION_MESSAGE, Arc::new(listener)) {
            Ok(subscription) => Some(Box::new(subscription)),
            Err(err) => {
                tracing::warn!("TUI notifications not pushed: {err}");
                None
            }
        }
    }
}

/// A notification's topic, title and payload, from the body `/notws` sends and the topic carries.
pub(crate) fn push_of(body: &serde_json::Value) -> Push {
    Push {
        topic: json_field(body, "topic"),
        title: json_field(body, "title"),
        message: body.get("message").cloned().unwrap_or_default(),
    }
}

impl<T: Transport> ScreenLink for Link<T> {
    /// A screen, or a transition a link goes to (an anchor to `runJob`), whose redirect to its screen is followed as the
    /// browser follows it.
    fn level(&self, path: &str, parameters: &BTreeMap<String, String>) -> Result<ScreenModel, LinkError> {
        let mut path = self.absolute(path);
        let mut parameters = parameters.clone();
        let mut carried = Messages::default();
        for _ in 0..FRAGMENT_REDIRECTS {
            let output = self.render_tui(path.clone(), &parameters, true)?;
            if let Some(redirect) = redirect_of(&output)? {
                (path, parameters) = (redirect.path, redirect.parameters);
                carry(&mut carried, redirect.messages);
                continue;
            }
            let mut model = model_from(&output)?;
            carry(&mut carried, std::mem::take(&mut model.messages));
            model.messages = carried;
            *self.last_path.lock().unwrap_or_else(PoisonError::into_inner) = model.screen.path.clone();
            return Ok(model);
        }
        Err(LinkError::Remote(format!("{path} redirected too many times")))
    }

    /// A transition that answers JSON, or text (a default value, a value url's label), as a JSON string.
    fn json(
        &self,
        url: &str,
        method: &str,
        parameters: &BTreeMap<String, String>,
    ) -> Result<serde_json::Value, LinkError> {
        let ask = self.ask(url, method, "GET", parameters, true);
        let result = self.transport.send(ask).and_then(|output| {
            check_status(&output)?;
            let text = output.body_text();
            Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text)))
        });
        if let Err(err) = &result {
            tracing::warn!("TUI request to {url} failed: {err}");
        }
        result
    }

    /// The body of a dynamic dialog or container: its url rendered in the `tui` mode. A transition answers with a
    /// redirect to its screen, which is followed.
    fn fragment(&self, url: &str, parameters: &BTreeMap<String, String>) -> Result<ScreenModel, LinkError> {
        let mut path = self.absolute(url);
        let mut parameters = parameters.clone();
        let mut carried = Messages::default();
        for _ in 0..FRAGMENT_REDIRECTS {
            let (bare, query) = split_query(&path);
            let mut asked = query.map(query_parameters).unwrap_or_default();
            asked.extend(parameters.clone());
            let output = self.render_tui(bare.to_string(), &asked, false)?;
            let Some(redirect) = redirect_of(&output)? else {
                let mut model = model_from(&output)?;
                carry(&mut carried, std::mem::take(&mut model.messages));
                model.messages = carried;
                return Ok(model);
            };
            (path, parameters) = (redirect.path, redirect.parameters);
            carry(&mut carried, redirect.messages);
        }
        Err(LinkError::Remote(format!("{url} redirected too many times")))
    }

    /// A file: an export, as the server renders it, with its type and name.
    fn raw(&self, url: &str, parameters: &BTreeMap<String, String>) -> Result<RawBody, LinkError> {
        let output = self.transport.send(self.ask(url, "", "GET", parameters, false))?;
        check_status(&output)?;
        let disposition = output.header("Content-Disposition").unwrap_or("");
        let file_name = disposition
            .split(';')
            .filter_map(|part| part.trim().strip_prefix("filename="))
            .map(|name| name.trim_matches('"').to_string())
            .next()
            .unwrap_or_default();
        Ok(RawBody {
            content_type: output.header("Content-Type").unwrap_or("").to_string(),
            file_name,
            bytes: output.body,
        })
    }

    fn transition(
        &self,
        url: &str,
        method: &str,
        fields: &BTreeMap<String, String>,
    ) -> Result<TransitionOutcome, LinkError> {
        let absolute = self.absolute(url);
        let output = self.transport.send(self.ask(url, method, "POST", fields, true))?;
        let text = output.body_text();
        let trimmed = text.trim_start();
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            return outcome_from(&output);
        }
        // Not JSON: an error page, or a form whose transition is the screen itself, which answers the screen; then the
        // posted fields are the parameters of that screen.
        check_status(&output)?;
        Ok(screen_repost(&absolute, fields))
    }

    fn options(&self, url: &str, parameters: &BTreeMap<String, String>) -> Result<Vec<Choice>, LinkError> {
        let result = self
            .transport
            .send(self.ask(url, "", "GET", parameters, true))
            .and_then(|output| choices_from(&output));
        if let Err(err) = &result {
            tracing::warn!("TUI options from {url} failed: {err}");
        }
        result
    }

    fn shell(&self, path: &str) -> Result<ShellInfo, LinkError> {
        let mut shell = ShellInfo {
            user: self.transport.user().unwrap_or_default(),
            ..ShellInfo::default()
        };
        // The drawer of the current app; a screen without one has none, which is not an error.
        match self.menu(path) {
            Ok(menu) => shell.menu = app_menu(menu, path),
            Err(LinkError::Auth(text)) => return Err(LinkError::Auth(text)),
            Err(err) => tracing::warn!("TUI menu for {path} failed: {err}"),
        }
        let output = self.transport.send(Ask::get("/shellData".to_string()))?;
        match serde_json::from_str::<serde_json::Value>(&output.body_text()) {
            Ok(data) => {
                tui_values(&data, &mut shell);
                shell_values(&data, &mut shell);
            }
            Err(err) => tracing::warn!("TUI shell data was not JSON: {err}"),
        }
        Ok(shell)
    }

    fn login(&self, username: &str, password: &str, code: &str) -> Result<(), LinkError> {
        self.transport.login(username, password, code)
    }

    fn send_code(&self, username: &str, password: &str, factor: &str) -> Result<String, LinkError> {
        self.transport.send_code(username, password, factor)
    }

    fn subscribe(&self, sink: PushSink) -> Option<Subscription> {
        self.transport.subscribe(sink)
    }
}

fn link_error(err: Error) -> LinkError {
    if err.kind() == ErrorKind::AuthenticationRequired {
        return LinkError::Auth("Authentication required".into());
    }
    LinkError::Remote(err.to_string())
}

fn value_map(fields: &BTreeMap<String, String>) -> BTreeMap<String, Value> {
    fields
        .iter()
        .map(|(name, value)| (name.clone(), Value::string(value)))
        .collect()
}

/// A 401 is a login; another status of 400 or more is an error with the start of the body.
fn check_status(output: &RenderOutput) -> Result<(), LinkError> {
    if output.status == 401 {
        return Err(LinkError::Auth("Authentication required".into()));
    }
    if output.status >= 400 {
        return Err(LinkError::Remote(format!(
            "HTTP {}: {}",
            output.status,
            snippet(&output.body_text())
        )));
    }
    Ok(())
}

/// Where a level's request went on to: a transition's redirect, as a 3xx or as the JSON a `tui` request gets, with the
/// messages the transition left (`runJob`'s, a save's), which only the JSON form carries.
struct Redirect {
    path: String,
    parameters: BTreeMap<String, String>,
    messages: Messages,
}

fn redirect_of(output: &RenderOutput) -> Result<Option<Redirect>, LinkError> {
    if (300..400).contains(&output.status) {
        let Some(location) = output.header("Location").map(local_path) else {
            return Err(LinkError::Remote(format!("HTTP {} with no Location", output.status)));
        };
        let (bare, query) = split_query(&location);
        return Ok(Some(Redirect {
            path: bare.to_string(),
            parameters: query.map(query_parameters).unwrap_or_default(),
            messages: Messages::default(),
        }));
    }
    let text = output.body_text();
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Ok(None);
    };
    // A screen document has its envelope; a redirect has a screenUrl or redirectUrl and none.
    if map.contains_key("v") || !(map.contains_key("screenUrl") || map.contains_key("redirectUrl")) {
        return Ok(None);
    }
    let outcome = outcome_from(output)?;
    if outcome.screen_url.is_empty() {
        let mut errors = outcome.messages.errors;
        errors.extend(
            outcome
                .messages
                .validation_errors
                .into_iter()
                .map(|error| error.message),
        );
        return Err(LinkError::Remote(errors.join("; ")));
    }
    let location = local_path(&outcome.screen_url);
    let (bare, query) = split_query(&location);
    let mut parameters = outcome.parameters;
    parameters.extend(query.map(query_parameters).unwrap_or_default());
    Ok(Some(Redirect {
        path: bare.to_string(),
        parameters,
        messages: outcome.messages,
    }))
}

/// Add `more` after the messages already carried.
fn carry(carried: &mut Messages, more: Messages) {
    carried.info.extend(more.info);
    carried.errors.extend(more.errors);
    carried.validation_errors.extend(more.validation_errors);
}

fn model_from(output: &RenderOutput) -> Result<ScreenModel, LinkError> {
    check_status(output)?;
    let text = output.body_text();
    ScreenModel::from_json(&text).map_err(|err| LinkError::Remote(format!("{err}: {}", snippet(&text))))
}

/// A screen rendered in place of a transition: show that screen with the posted fields.
fn screen_repost(url: &str, fields: &BTreeMap<String, String>) -> TransitionOutcome {
    let (path, query) = split_query(url);
    let mut parameters = query.map(query_parameters).unwrap_or_default();
    for (name, value) in fields {
        parameters.insert(name.clone(), value.clone());
    }
    let screen_url = match form_query(&parameters) {
        query if query.is_empty() => path.to_string(),
        query => format!("{path}?{query}"),
    };
    TransitionOutcome {
        screen_url,
        messages: Messages::default(),
        parameters,
    }
}

fn form_query(fields: &BTreeMap<String, String>) -> String {
    fields
        .iter()
        .map(|(name, value)| format!("{}={}", encode_query(name), encode_query(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn encode_query(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(char::from(byte)),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// A transition's JSON answer: `send_json_redirect`'s `screenUrl`, `messageInfos` and `validationErrors`, or
/// `sendJsonResponse`'s `messages` and `errors`. A status of 400 or more is never a navigation: its errors stay on the
/// screen with the edits.
fn outcome_from(output: &RenderOutput) -> Result<TransitionOutcome, LinkError> {
    if output.status == 401 {
        return Err(LinkError::Auth("Authentication required".into()));
    }
    let text = output.body_text();
    let value = match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(value) => value,
        Err(_) if output.status >= 400 => {
            return Err(LinkError::Remote(format!("HTTP {}: {}", output.status, snippet(&text))));
        }
        Err(err) => return Err(LinkError::Remote(format!("transition JSON: {err}"))),
    };
    let mut outcome = outcome_of(&value);
    if output.status >= 400 {
        outcome.screen_url.clear();
        if outcome.messages.errors.is_empty() && outcome.messages.validation_errors.is_empty() {
            outcome
                .messages
                .errors
                .push(format!("HTTP {}: {}", output.status, snippet(&text)));
        }
    }
    Ok(outcome)
}

fn outcome_of(value: &serde_json::Value) -> TransitionOutcome {
    let mut messages = Messages::default();
    if let Some(list) = value.get("messageInfos").and_then(serde_json::Value::as_array) {
        for item in list {
            let text = json_field(item, "message");
            if text.is_empty() {
                continue;
            }
            if is_error_type(&json_field(item, "type")) {
                messages.errors.push(text);
            } else {
                messages.info.push(text);
            }
        }
    }
    for (key, errors) in [("messages", false), ("errors", true)] {
        match value.get(key) {
            Some(serde_json::Value::String(text)) if !text.is_empty() => {
                if errors {
                    &mut messages.errors
                } else {
                    &mut messages.info
                }
                .push(text.clone());
            }
            Some(serde_json::Value::Array(items)) => {
                for text in items.iter().filter_map(json_text).filter(|text| !text.is_empty()) {
                    if errors {
                        &mut messages.errors
                    } else {
                        &mut messages.info
                    }
                    .push(text);
                }
            }
            _ => {}
        }
    }
    messages.validation_errors = validation_of(value.get("validationErrors"));
    TransitionOutcome {
        screen_url: screen_url_of(value),
        messages,
        parameters: string_map(value.get("screenParameters")),
    }
}

fn is_error_type(kind: &str) -> bool {
    kind.eq_ignore_ascii_case("danger") || kind.to_ascii_lowercase().contains("error")
}

fn screen_url_of(value: &serde_json::Value) -> String {
    let screen = json_field(value, "screenUrl");
    if screen.is_empty() {
        json_field(value, "redirectUrl")
    } else {
        screen
    }
}

fn validation_of(value: Option<&serde_json::Value>) -> Vec<ValidationError> {
    let Some(list) = value.and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .map(|item| ValidationError {
            form: json_field(item, "form"),
            field: json_field(item, "field"),
            message: json_field(item, "message"),
        })
        .collect()
}

fn choices_from(output: &RenderOutput) -> Result<Vec<Choice>, LinkError> {
    check_status(output)?;
    let text = output.body_text();
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|err| LinkError::Remote(format!("options JSON: {err}: {}", snippet(&text))))?;
    let list =
        option_list(&value).ok_or_else(|| LinkError::Remote(format!("options were not a list: {}", snippet(&text))))?;
    Ok(list.iter().map(choice_of).collect())
}

fn option_list(value: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    if let Some(list) = value.as_array() {
        return Some(list);
    }
    value
        .get("options")
        .and_then(serde_json::Value::as_array)
        .or_else(|| value.get("resultList").and_then(serde_json::Value::as_array))
}

fn choice_of(value: &serde_json::Value) -> Choice {
    if let Some(text) = value.as_str() {
        return Choice {
            value: text.to_string(),
            label: text.to_string(),
        };
    }
    let picked = first_text(value, &["value", "geoId", "key", "id"]);
    let mut label = first_text(value, &["label", "text", "geoName", "description"]);
    if label.is_empty() {
        label.clone_from(&picked);
    }
    Choice { value: picked, label }
}

fn menu_from(output: &RenderOutput) -> Result<Vec<MenuItem>, LinkError> {
    check_status(output)?;
    let text = output.body_text();
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|err| LinkError::Remote(format!("menu JSON: {err}")))?;
    let Some(list) = value.as_array() else {
        return Err(LinkError::Remote("menu was not a list".into()));
    };
    Ok(list.iter().filter_map(menu_item).collect())
}

/// The drawer shows the app the screen is in: the menu level of the app root and its screens.
fn app_menu(menu: Vec<MenuItem>, path: &str) -> Vec<MenuItem> {
    let names: Vec<&str> = path
        .split('?')
        .next()
        .unwrap_or(path)
        .split('/')
        .filter(|n| !n.is_empty())
        .collect();
    if names.len() < 2 {
        return menu;
    }
    let app = format!("/{}/{}", names[0], names[1]);
    let found: Vec<MenuItem> = menu
        .iter()
        .filter(|item| item.url.split('?').next() == Some(app.as_str()))
        .cloned()
        .collect();
    if found.is_empty() {
        menu
    } else {
        found
    }
}

fn menu_item(value: &serde_json::Value) -> Option<MenuItem> {
    let title = json_field(value, "title");
    let url = menu_path(value);
    if title.is_empty() && url.is_empty() {
        return None;
    }
    let children = value
        .get("subscreens")
        .and_then(serde_json::Value::as_array)
        .map(|list| list.iter().filter_map(menu_child).collect())
        .unwrap_or_default();
    Some(MenuItem { title, url, children })
}

/// One child level. A grandchild menu is not walked.
fn menu_child(value: &serde_json::Value) -> Option<MenuItem> {
    let title = json_field(value, "title");
    let url = menu_path(value);
    if title.is_empty() && url.is_empty() {
        None
    } else {
        Some(MenuItem {
            title,
            url,
            children: Vec::new(),
        })
    }
}

fn menu_path(value: &serde_json::Value) -> String {
    let with_params = json_field(value, "pathWithParams");
    if with_params.is_empty() {
        json_field(value, "path")
    } else {
        with_params
    }
}

fn menu_url(path: &str) -> String {
    let path = path.split('?').next().unwrap_or(path);
    if path.is_empty() || path == "/" {
        "/menuData".to_string()
    } else if let Some(rest) = path.strip_prefix('/') {
        format!("/menuData/{rest}")
    } else {
        format!("/menuData/{path}")
    }
}

/// The `tui` values of the shell data: breakpoints, colors and keys (section 2.6).
fn tui_values(data: &serde_json::Value, shell: &mut ShellInfo) {
    let Some(tui) = data.get("tui") else {
        return;
    };
    if let Some(map) = tui.get("breakpoints").and_then(serde_json::Value::as_object) {
        for (name, value) in map {
            if let Some(columns) = value.as_u64().and_then(|n| u16::try_from(n).ok()) {
                shell.breakpoints.insert(name.clone(), columns);
            }
        }
    }
    shell.colors = string_map(tui.get("colors"));
    shell.keys = string_map(tui.get("keys"));
}

/// The apps, their quick create items and palette actions, the tray's screens and the counts of the shell data.
fn shell_values(data: &serde_json::Value, shell: &mut ShellInfo) {
    let list =
        |value: Option<&serde_json::Value>| value.and_then(serde_json::Value::as_array).cloned().unwrap_or_default();
    for app in list(data.get("apps")) {
        let title = json_field(&app, "title");
        let path = json_field(&app, "path");
        if !path.is_empty() {
            shell.apps.push(MenuItem {
                title: if title.is_empty() {
                    json_field(&app, "name")
                } else {
                    title.clone()
                },
                url: path,
                children: Vec::new(),
            });
        }
        let providers = app.get("providers");
        for (key, out) in [
            ("quickCreate", &mut shell.quick_create),
            ("paletteActions", &mut shell.actions),
        ] {
            for item in list(providers.and_then(|providers| providers.get(key))) {
                let url = json_field(&item, "url");
                if url.is_empty() {
                    continue;
                }
                out.push(ShellAction {
                    label: json_field(&item, "label"),
                    url,
                    app: title.clone(),
                });
            }
        }
    }
    if let Some(values) = data.get("shell") {
        for (key, label) in [
            ("trayToday", "Today"),
            ("trayChat", "Chat"),
            ("trayNotifications", "Notifications"),
        ] {
            let url = json_field(values, key);
            if !url.is_empty() {
                shell.tray.push((label.to_string(), url));
            }
        }
    }
    if let Some(counts) = data.get("counts").filter(|counts| !counts.is_null()) {
        shell.counts = counts_of(counts);
    }
    shell.drawer_open = data
        .get("tui")
        .and_then(|tui| tui.get("drawerOpen"))
        .and_then(serde_json::Value::as_bool);
    shell.mouse = data
        .get("tui")
        .and_then(|tui| tui.get("mouse"))
        .and_then(serde_json::Value::as_bool);
}

/// The path and query of a redirect's location: a full url keeps what follows its host.
fn local_path(location: &str) -> String {
    match location.split_once("://") {
        Some((_, rest)) => rest
            .find('/')
            .map_or_else(|| "/".to_string(), |at| rest[at..].to_string()),
        None => location.to_string(),
    }
}

fn json_field(value: &serde_json::Value, name: &str) -> String {
    value.get(name).and_then(json_text).unwrap_or_default()
}

fn first_text(value: &serde_json::Value, names: &[&str]) -> String {
    for name in names {
        if let Some(text) = value.get(*name).and_then(json_text) {
            if !text.is_empty() {
                return text;
            }
        }
    }
    String::new()
}

fn json_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        serde_json::Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn string_map(value: Option<&serde_json::Value>) -> BTreeMap<String, String> {
    let Some(object) = value.and_then(serde_json::Value::as_object) else {
        return BTreeMap::new();
    };
    object
        .iter()
        .filter_map(|(name, item)| json_text(item).map(|text| (name.clone(), text)))
        .collect()
}

fn snippet(text: &str) -> String {
    text.chars().take(400).collect::<String>().replace('\n', " ")
}

/// Resolve `url` against the directory of `base`. An absolute url keeps its own path. `..` applies after the last
/// segment of `base` is dropped, so `/apps/gallery/List` plus `../Detail` is `/apps/Detail`.
pub fn resolve_url(base: &str, url: &str) -> String {
    if url.is_empty() {
        return String::new();
    }
    let (path, query) = split_query(url);
    let mut segments = if path.starts_with('/') {
        Vec::new()
    } else {
        let (base_path, _) = split_query(base);
        let mut segments = split_segments(base_path);
        segments.pop();
        segments
    };
    segments.extend(split_segments(path));
    let mut stack = Vec::new();
    for segment in segments {
        match segment {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            other => stack.push(other),
        }
    }
    let mut out = format!("/{}", stack.join("/"));
    if stack.is_empty() {
        out = "/".to_string();
    }
    if let Some(query) = query {
        out.push('?');
        out.push_str(query);
    }
    out
}

fn split_query(url: &str) -> (&str, Option<&str>) {
    match url.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (url, None),
    }
}

fn split_segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|segment| !segment.is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_url_drops_the_last_segment_before_dot_dot() {
        assert_eq!(resolve_url("/apps/gallery/List", "../Detail"), "/apps/Detail");
        assert_eq!(
            resolve_url("/apps/SimpleScreens/Party/FindParty", "../EditParty?partyId=1"),
            "/apps/SimpleScreens/EditParty?partyId=1"
        );
        assert_eq!(
            resolve_url("/apps/gallery/List", "/apps/gallery/Detail?enumId=A"),
            "/apps/gallery/Detail?enumId=A"
        );
        assert_eq!(
            resolve_url("/apps/gallery/List?pageIndex=1", "detail"),
            "/apps/gallery/detail"
        );
        assert_eq!(resolve_url("/apps/gallery/List", "./Detail"), "/apps/gallery/Detail");
    }

    #[test]
    fn a_failed_transition_is_never_a_navigation_and_keeps_its_errors() {
        let output = |status: u16, body: &str| RenderOutput {
            status,
            headers: Vec::new(),
            body: body.as_bytes().to_vec(),
        };
        let failed = outcome_from(&output(
            500,
            r#"{"errorCode":500,"errors":"Party is locked","screenUrl":"/x"}"#,
        ))
        .unwrap();
        assert!(failed.screen_url.is_empty());
        assert_eq!(failed.messages.errors, ["Party is locked"]);
        assert!(matches!(outcome_from(&output(401, "{}")), Err(LinkError::Auth(_))));
        let saved = outcome_from(&output(200, r#"{"messages":"Saved","screenUrl":"/y"}"#)).unwrap();
        assert_eq!(saved.messages.info, ["Saved"]);
        assert_eq!(saved.screen_url, "/y");
        let repost = screen_repost("/apps/x?a=%C3%A9&b=100%", &BTreeMap::new());
        assert_eq!(repost.parameters.get("a").map(String::as_str), Some("é"));
        assert_eq!(repost.parameters.get("b").map(String::as_str), Some("100%"));
    }
}
