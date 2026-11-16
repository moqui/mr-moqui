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

//! [`ScreenLink`] is the client's only way to the server: a level, a transition, a drop-down's options, the shell data,
//! a login, the JSON of a transition (search, counts, suggestions, a tree's children), a fragment (the body of a
//! dynamic dialog or container) and a file (an export). The session never calls it itself; it queues [`LinkCall`]s, and the loop runs them off the UI thread
//! (plans/archive/Screen-TUI-Impl.md 3.6). [`FakeLink`] answers from fixture files and records each call.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::layout::PAGE_ROWS;
use crate::model::{Cell, Choice, ListRow, Messages, ModelError, Node, ScreenModel, ValidationError};

pub const GALLERY_LIST_JSON: &str = include_str!("../fixtures/list.json");
pub const GALLERY_DETAIL_JSON: &str = include_str!("../fixtures/detail.json");

pub const GALLERY_LIST_PATH: &str = "/Gallery/List";
pub const GALLERY_DETAIL_PATH: &str = "/Gallery/Detail";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkError {
    Model(ModelError),
    Missing(String),
    /// The server wants a login (a 401). The session opens the login dialog and sends the call again after it.
    Auth(String),
    /// The server refused the call, or the worker did not return one.
    Remote(String),
    /// The password was right and the account needs a second factor's code: the login dialog asks for it.
    Code(CodeAsk),
}

/// What a login that needs a code says (why, and the factors a code can be sent to), and such a factor: the remote
/// sign-in's own types, which the in-process link fills too.
pub use moqui_lib::remote_login::{CodeAsk, Sendable};

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Model(err) => write!(f, "{err}"),
            Self::Missing(path) => write!(f, "no screen at {path}"),
            Self::Auth(text) | Self::Remote(text) => write!(f, "{text}"),
            Self::Code(ask) => write!(f, "{}", ask.text),
        }
    }
}

/// The server has no user accounts yet, so the login dialog creates the first administrator: the password rules it
/// asks of the account (Login.xml `initialAdminNeeded`, `UserServices.get#InitialAdminNeeded`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AdminSetup {
    pub min_length: u32,
    pub min_digits: u32,
    pub min_others: u32,
}

impl AdminSetup {
    /// The rules as the dialog says them: "at least 12 characters, 1 digit, 1 other".
    pub fn hint(&self) -> String {
        let mut parts = vec![format!("at least {} characters", self.min_length)];
        if self.min_digits > 0 {
            parts.push(format!("{} digit", self.min_digits));
        }
        if self.min_others > 0 {
            parts.push(format!("{} other", self.min_others));
        }
        parts.join(", ")
    }
}

/// The first administrator's account, as the login dialog's setup sends it to `create#InitialAdminAccount`.
#[derive(Clone, PartialEq, Eq)]
pub struct AdminAccount {
    pub username: String,
    pub password: String,
    pub verify: String,
    pub full_name: String,
    pub email: String,
}

/// The passwords are never in a log line.
impl std::fmt::Debug for AdminAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminAccount")
            .field("username", &self.username)
            .field("full_name", &self.full_name)
            .field("email", &self.email)
            .finish_non_exhaustive()
    }
}

/// What a transition posted. `screen_url` is empty when the client stays on this screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransitionOutcome {
    pub screen_url: String,
    pub messages: Messages,
    pub parameters: BTreeMap<String, String>,
}

impl TransitionOutcome {
    pub fn saved(text: &str) -> Self {
        Self {
            screen_url: String::new(),
            messages: Messages {
                info: vec![text.to_string()],
                ..Messages::default()
            },
            parameters: BTreeMap::new(),
        }
    }

    pub fn invalid(errors: Vec<ValidationError>) -> Self {
        Self {
            screen_url: String::new(),
            messages: Messages {
                validation_errors: errors,
                ..Messages::default()
            },
            parameters: BTreeMap::new(),
        }
    }
}

/// One drawer row. Children are the second level only.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuItem {
    pub title: String,
    pub url: String,
    pub children: Vec<MenuItem>,
    /// The menu data's `disableLink` (a `disable-when` that holds): shown dimmed, and not a link.
    pub disabled: bool,
    /// The menu data's `group` (a `menu-group`): the first of a run of items that share one has it as a heading above
    /// it. Empty for none.
    pub group: String,
}

/// The shell data the client draws around the screens: the drawer of the current app, the user, and the theme's `tui`
/// values (section 2.6), with what the palette, quick create, the tray and the counters show. A link without a shell
/// answers the default, and the client keeps its built-in settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellInfo {
    pub menu: Vec<MenuItem>,
    pub user: String,
    /// Width class or panel name to columns: `sm`, `md`, `lg`, `xl`, `drawer`, `tray`.
    pub breakpoints: BTreeMap<String, u16>,
    /// Color role to color.
    pub colors: BTreeMap<String, String>,
    /// Action name to chords, merged over the default keymap.
    pub keys: BTreeMap<String, String>,
    /// The apps the user may open, for the palette.
    pub apps: Vec<MenuItem>,
    /// The quick create items of the apps (F6).
    pub quick_create: Vec<ShellAction>,
    /// The palette's actions.
    pub actions: Vec<ShellAction>,
    /// The tray's screens, each a label and a url: Today, Chat, Notifications.
    pub tray: Vec<(String, String)>,
    /// The badge counts: `today`, `chat`, `notifications`.
    pub counts: BTreeMap<String, i64>,
    /// The user's choice of the drawer (`TUI_DRAWER_OPEN`), None when not made.
    pub drawer_open: Option<bool>,
    /// The user's choice of the mouse (`TUI_MOUSE`), None when not made: then it is on.
    pub mouse: Option<bool>,
    /// The key scheme: the user's `TUI_KEYS`, else the theme's `STRT_TUI_KEY_SCHEME`; None for neither, then Modern.
    pub key_scheme: Option<crate::keys::KeyScheme>,
}

/// A quick create item or a palette action: what it is called, and the screen or form it opens.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellAction {
    pub label: String,
    pub url: String,
    /// The app it comes from.
    pub app: String,
}

/// The counts of a `shellCounts` answer, or of the shell data's `counts`: each number by its name.
pub fn counts_of(value: &serde_json::Value) -> BTreeMap<String, i64> {
    let counts = value.get("counts").unwrap_or(value);
    counts
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(name, count)| {
                    count
                        .as_i64()
                        .or_else(|| count.as_str().and_then(|text| text.parse().ok()))
                        .map(|n| (name.clone(), n))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkCall {
    Level {
        path: String,
        parameters: BTreeMap<String, String>,
    },
    /// A level above the leaf (`Party.tui`), loaded for the page `page` (the leaf's path, no query): a level the user may
    /// not open alone answers as that page's frame (docs/security/Attack-Surface.md, "Parent frames").
    Parent {
        path: String,
        page: String,
        parameters: BTreeMap<String, String>,
    },
    Transition {
        url: String,
        method: String,
        fields: BTreeMap<String, String>,
    },
    Options {
        url: String,
        parameters: BTreeMap<String, String>,
    },
    Shell {
        path: String,
    },
    /// The call is sent once and dropped. The dialog holds the password until the answer, and longer only while it asks
    /// for a second factor's code.
    Login {
        username: String,
        password: String,
        code: String,
    },
    /// Send a second factor's code to `factor`, for a login the password started. The password goes again, as the
    /// server keeps no state between the calls of an in-process link.
    SendCode {
        username: String,
        password: String,
        factor: String,
    },
    /// Whether the server has no accounts yet, asked as the login dialog opens: then the dialog creates the first
    /// administrator.
    InitialAdmin,
    /// Create the first administrator, sent once and dropped as a login is.
    CreateInitialAdmin(AdminAccount),
    /// A transition that answers JSON: the palette's search, the counters, an autocomplete, a display's value, a
    /// tree's children, a status bar, a preference.
    Json {
        url: String,
        method: String,
        parameters: BTreeMap<String, String>,
    },
    /// The body of a dynamic dialog or container: its transition, and the screen it answers, in the `tui` mode.
    Fragment {
        url: String,
        parameters: BTreeMap<String, String>,
    },
    /// A file as the server sends it: an export.
    Raw {
        url: String,
        parameters: BTreeMap<String, String>,
    },
}

/// A file from [`LinkCall::Raw`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawBody {
    pub content_type: String,
    /// The name the server gave it (`Content-Disposition`), else empty.
    pub file_name: String,
    pub bytes: Vec<u8>,
}

/// The answer to one [`LinkCall`], of the same kind.
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    Level(Result<ScreenModel, LinkError>),
    Transition(Result<TransitionOutcome, LinkError>),
    Options(Result<Vec<Choice>, LinkError>),
    Shell(Result<ShellInfo, LinkError>),
    Login(Result<(), LinkError>),
    /// The factor the code went to.
    SendCode(Result<String, LinkError>),
    /// The password rules while the first administrator is to be created, else None.
    InitialAdmin(Result<Option<AdminSetup>, LinkError>),
    CreateInitialAdmin(Result<(), LinkError>),
    Json(Result<serde_json::Value, LinkError>),
    Fragment(Result<ScreenModel, LinkError>),
    Raw(Result<RawBody, LinkError>),
}

impl Reply {
    pub fn error(&self) -> Option<&LinkError> {
        match self {
            Self::Level(Err(err))
            | Self::Transition(Err(err))
            | Self::Options(Err(err))
            | Self::Shell(Err(err))
            | Self::Login(Err(err))
            | Self::SendCode(Err(err))
            | Self::InitialAdmin(Err(err))
            | Self::CreateInitialAdmin(Err(err))
            | Self::Json(Err(err))
            | Self::Fragment(Err(err))
            | Self::Raw(Err(err)) => Some(err),
            _ => None,
        }
    }
}

/// A notification the server pushed to the logged-in user: the counters change, and its title is worth a line. The
/// `message` is the notification's JSON payload (an `AgentRun` signal's `runId`, `conversationId` and `event`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Push {
    pub topic: String,
    pub title: String,
    pub message: serde_json::Value,
}

/// Called on the link's own thread with each push.
pub type PushSink = Arc<dyn Fn(Push) + Send + Sync>;

/// Holds a push subscription open; dropping it ends the subscription.
pub type Subscription = Box<dyn Send>;

/// The server as the client sees it. Calls run on worker threads, so a link is `Send + Sync`.
pub trait ScreenLink: Send + Sync {
    fn level(&self, path: &str, parameters: &BTreeMap<String, String>) -> Result<ScreenModel, LinkError>;

    /// Follow a navigation link. A server may answer with an attachment instead of a screen; then the answer is
    /// [`Reply::Raw`] and the session keeps its current screen while the caller saves the file.
    fn navigation(&self, path: &str, parameters: &BTreeMap<String, String>) -> Reply {
        Reply::Level(self.level(path, parameters))
    }

    /// A level above the leaf, loaded for the page `page`: a link that can name the page to the server does
    /// ([`LinkCall::Parent`]); this one asks for the level alone.
    fn parent(&self, path: &str, _page: &str, parameters: &BTreeMap<String, String>) -> Reply {
        self.navigation(path, parameters)
    }

    fn transition(
        &self,
        url: &str,
        method: &str,
        fields: &BTreeMap<String, String>,
    ) -> Result<TransitionOutcome, LinkError>;

    /// Post a form. A transition may answer with an attachment (a download that writes the response itself, as
    /// `DocumentExport.json`); then the answer is [`Reply::Raw`] and the caller saves the file, as for
    /// [`Self::navigation`].
    fn transition_reply(&self, url: &str, method: &str, fields: &BTreeMap<String, String>) -> Reply {
        Reply::Transition(self.transition(url, method, fields))
    }

    fn options(&self, url: &str, parameters: &BTreeMap<String, String>) -> Result<Vec<Choice>, LinkError>;

    /// The drawer for `path` and the theme values.
    fn shell(&self, _path: &str) -> Result<ShellInfo, LinkError> {
        Ok(ShellInfo::default())
    }

    /// Log in, after a call answered [`LinkError::Auth`].
    fn login(&self, _username: &str, _password: &str, _code: &str) -> Result<(), LinkError> {
        Err(LinkError::Remote("this connection does not log in".to_string()))
    }

    /// Send a code to a second factor of the user, after a login answered [`LinkError::Code`]; the answer is the factor.
    fn send_code(&self, _username: &str, _password: &str, _factor: &str) -> Result<String, LinkError> {
        Err(LinkError::Remote("this connection does not send codes".to_string()))
    }

    /// The password rules when the server has no user accounts yet and the login dialog is to create the first
    /// administrator; None once any account exists, and from a link that does not log in.
    fn initial_admin(&self) -> Result<Option<AdminSetup>, LinkError> {
        Ok(None)
    }

    /// Create the first administrator (`create#InitialAdminAccount`, which refuses once any account exists); the
    /// dialog then logs in with it.
    fn create_initial_admin(&self, _account: &AdminAccount) -> Result<(), LinkError> {
        Err(LinkError::Remote(
            "this connection does not create accounts".to_string(),
        ))
    }

    /// A transition's JSON answer.
    fn json(
        &self,
        url: &str,
        _method: &str,
        _parameters: &BTreeMap<String, String>,
    ) -> Result<serde_json::Value, LinkError> {
        Err(LinkError::Missing(url.to_string()))
    }

    /// The screen a dynamic dialog's or container's transition answers, as a model.
    fn fragment(&self, url: &str, parameters: &BTreeMap<String, String>) -> Result<ScreenModel, LinkError> {
        self.level(url, parameters)
    }

    /// A file the server sends.
    fn raw(&self, url: &str, _parameters: &BTreeMap<String, String>) -> Result<RawBody, LinkError> {
        Err(LinkError::Missing(url.to_string()))
    }

    /// Pushes for the logged-in user, delivered to `sink` until the answer drops. None when the link cannot push, and
    /// the client reads the counters each minute instead.
    fn subscribe(&self, _sink: PushSink) -> Option<Subscription> {
        None
    }

    /// Run one call.
    fn call(&self, call: &LinkCall) -> Reply {
        match call {
            LinkCall::Level { path, parameters } => self.navigation(path, parameters),
            LinkCall::Parent { path, page, parameters } => self.parent(path, page, parameters),
            LinkCall::Transition { url, method, fields } => self.transition_reply(url, method, fields),
            LinkCall::Options { url, parameters } => Reply::Options(self.options(url, parameters)),
            LinkCall::Shell { path } => Reply::Shell(self.shell(path)),
            LinkCall::Login {
                username,
                password,
                code,
            } => Reply::Login(self.login(username, password, code)),
            LinkCall::SendCode {
                username,
                password,
                factor,
            } => Reply::SendCode(self.send_code(username, password, factor)),
            LinkCall::InitialAdmin => Reply::InitialAdmin(self.initial_admin()),
            LinkCall::CreateInitialAdmin(account) => Reply::CreateInitialAdmin(self.create_initial_admin(account)),
            LinkCall::Json {
                url,
                method,
                parameters,
            } => Reply::Json(self.json(url, method, parameters)),
            LinkCall::Fragment { url, parameters } => Reply::Fragment(self.fragment(url, parameters)),
            LinkCall::Raw { url, parameters } => Reply::Raw(self.raw(url, parameters)),
        }
    }
}

impl<L: ScreenLink + ?Sized> ScreenLink for Arc<L> {
    fn level(&self, path: &str, parameters: &BTreeMap<String, String>) -> Result<ScreenModel, LinkError> {
        (**self).level(path, parameters)
    }

    fn navigation(&self, path: &str, parameters: &BTreeMap<String, String>) -> Reply {
        (**self).navigation(path, parameters)
    }

    fn parent(&self, path: &str, page: &str, parameters: &BTreeMap<String, String>) -> Reply {
        (**self).parent(path, page, parameters)
    }

    fn transition(
        &self,
        url: &str,
        method: &str,
        fields: &BTreeMap<String, String>,
    ) -> Result<TransitionOutcome, LinkError> {
        (**self).transition(url, method, fields)
    }

    fn transition_reply(&self, url: &str, method: &str, fields: &BTreeMap<String, String>) -> Reply {
        (**self).transition_reply(url, method, fields)
    }

    fn options(&self, url: &str, parameters: &BTreeMap<String, String>) -> Result<Vec<Choice>, LinkError> {
        (**self).options(url, parameters)
    }

    fn shell(&self, path: &str) -> Result<ShellInfo, LinkError> {
        (**self).shell(path)
    }

    fn login(&self, username: &str, password: &str, code: &str) -> Result<(), LinkError> {
        (**self).login(username, password, code)
    }

    fn send_code(&self, username: &str, password: &str, factor: &str) -> Result<String, LinkError> {
        (**self).send_code(username, password, factor)
    }

    fn initial_admin(&self) -> Result<Option<AdminSetup>, LinkError> {
        (**self).initial_admin()
    }

    fn create_initial_admin(&self, account: &AdminAccount) -> Result<(), LinkError> {
        (**self).create_initial_admin(account)
    }

    fn json(
        &self,
        url: &str,
        method: &str,
        parameters: &BTreeMap<String, String>,
    ) -> Result<serde_json::Value, LinkError> {
        (**self).json(url, method, parameters)
    }

    fn fragment(&self, url: &str, parameters: &BTreeMap<String, String>) -> Result<ScreenModel, LinkError> {
        (**self).fragment(url, parameters)
    }

    fn raw(&self, url: &str, parameters: &BTreeMap<String, String>) -> Result<RawBody, LinkError> {
        (**self).raw(url, parameters)
    }

    fn subscribe(&self, sink: PushSink) -> Option<Subscription> {
        (**self).subscribe(sink)
    }
}

/// One canned JSON answer: for these parameters, or for any with an empty map.
type JsonAnswer = (BTreeMap<String, String>, serde_json::Value);

/// One canned options answer. An empty parameter map is the default for that url.
#[derive(Clone, Debug)]
struct OptionAnswer {
    parameters: BTreeMap<String, String>,
    choices: Vec<Choice>,
}

#[derive(Debug, Default)]
struct Answers {
    log: Vec<LinkCall>,
    transitions: BTreeMap<String, TransitionOutcome>,
    options: BTreeMap<String, Vec<OptionAnswer>>,
    shell: ShellInfo,
    /// JSON answers by url; an answer that is a list of `(parameters, value)` picks by the parameters, as options do.
    json: BTreeMap<String, Vec<JsonAnswer>>,
    raw: BTreeMap<String, RawBody>,
    /// Calls answered [`LinkError::Auth`] until a login with this password.
    password: Option<String>,
    /// With the password, the code a login needs, and the factors a code can be sent to.
    code: Option<(String, Vec<Sendable>)>,
    /// The server has no accounts yet: these rules, until an administrator is created.
    admin: Option<AdminSetup>,
    /// The refusal of the next creation of the administrator.
    admin_refusal: Option<String>,
}

/// Fixture files keyed by `screen.path`. Clones share the answers and the log.
#[derive(Clone, Debug)]
pub struct FakeLink {
    levels: Arc<BTreeMap<String, ScreenModel>>,
    answers: Arc<Mutex<Answers>>,
}

impl FakeLink {
    pub fn from_json(files: &[&str]) -> Result<Self, LinkError> {
        let mut levels = BTreeMap::new();
        for file in files {
            let model = ScreenModel::from_json(file).map_err(LinkError::Model)?;
            levels.insert(model.screen.path.clone(), model);
        }
        Ok(Self {
            levels: Arc::new(levels),
            answers: Arc::new(Mutex::new(Answers::default())),
        })
    }

    fn answers(&self) -> MutexGuard<'_, Answers> {
        self.answers.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn calls(&self) -> Vec<LinkCall> {
        self.answers().log.clone()
    }

    pub fn answer_transition(&self, url: &str, outcome: TransitionOutcome) {
        self.answers().transitions.insert(url.to_string(), outcome);
    }

    pub fn answer_options(&self, url: &str, parameters: BTreeMap<String, String>, choices: Vec<Choice>) {
        self.answers()
            .options
            .entry(url.to_string())
            .or_default()
            .push(OptionAnswer { parameters, choices });
    }

    pub fn answer_shell(&self, shell: ShellInfo) {
        self.answers().shell = shell;
    }

    /// Answer `Json` calls to `url` with `value`: for these `parameters`, or for any with an empty map.
    pub fn answer_json(&self, url: &str, parameters: BTreeMap<String, String>, value: serde_json::Value) {
        self.answers()
            .json
            .entry(url.to_string())
            .or_default()
            .push((parameters, value));
    }

    pub fn answer_raw(&self, url: &str, body: RawBody) {
        self.answers().raw.insert(url.to_string(), body);
    }

    /// Answer every call with [`LinkError::Auth`] until a login with `password`.
    pub fn require_login(&self, password: &str) {
        self.answers().password = Some(password.to_string());
    }

    /// With [`Self::require_login`], a login needs `code` too; without it the login answers [`LinkError::Code`] naming
    /// `sendable`.
    pub fn require_code(&self, code: &str, sendable: Vec<Sendable>) {
        self.answers().code = Some((code.to_string(), sendable));
    }

    /// The server has no accounts yet: [`ScreenLink::initial_admin`] answers `setup` until an administrator is created,
    /// whose password a login then takes.
    pub fn need_initial_admin(&self, setup: AdminSetup) {
        self.answers().admin = Some(setup);
    }

    /// The next [`ScreenLink::create_initial_admin`] is refused with `text`. With `accounts`, someone else created the
    /// first one: setup is no longer needed.
    pub fn refuse_initial_admin(&self, text: &str, accounts: bool) {
        let mut answers = self.answers();
        answers.admin_refusal = Some(text.to_string());
        if accounts {
            answers.admin = None;
        }
    }

    fn record(&self, call: LinkCall) -> Result<(), LinkError> {
        let mut answers = self.answers();
        let login = matches!(
            call,
            LinkCall::Login { .. }
                | LinkCall::SendCode { .. }
                | LinkCall::InitialAdmin
                | LinkCall::CreateInitialAdmin(_)
        );
        answers.log.push(call);
        if !login && answers.password.is_some() {
            return Err(LinkError::Auth("Authentication required".to_string()));
        }
        Ok(())
    }
}

impl FakeLink {
    /// The fixture of the level `path`, with `parameters`.
    fn level_model(&self, path: &str, parameters: &BTreeMap<String, String>) -> Result<ScreenModel, LinkError> {
        // A level asked for as its own document (`Party.tui`) is the fixture of `Party`.
        let (screen, query) = path.split_once('?').unwrap_or((path, ""));
        let screen = screen.strip_suffix(".tui").unwrap_or(screen);
        let model = self
            .levels
            .get(screen)
            .cloned()
            .ok_or_else(|| LinkError::Missing(screen.to_string()))?;
        let mut parameters = parameters.clone();
        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            parameters.entry(name.to_string()).or_insert_with(|| value.to_string());
        }
        Ok(page_model(model, &parameters))
    }
}

impl ScreenLink for FakeLink {
    fn level(&self, path: &str, parameters: &BTreeMap<String, String>) -> Result<ScreenModel, LinkError> {
        self.record(LinkCall::Level {
            path: path.to_string(),
            parameters: parameters.clone(),
        })?;
        self.level_model(path, parameters)
    }

    /// Recorded as a parent; answered from the level's fixture.
    fn parent(&self, path: &str, page: &str, parameters: &BTreeMap<String, String>) -> Reply {
        let recorded = self.record(LinkCall::Parent {
            path: path.to_string(),
            page: page.to_string(),
            parameters: parameters.clone(),
        });
        Reply::Level(recorded.and_then(|()| self.level_model(path, parameters)))
    }

    fn transition(
        &self,
        url: &str,
        method: &str,
        fields: &BTreeMap<String, String>,
    ) -> Result<TransitionOutcome, LinkError> {
        self.record(LinkCall::Transition {
            url: url.to_string(),
            method: method.to_string(),
            fields: fields.clone(),
        })?;
        Ok(self
            .answers()
            .transitions
            .get(url)
            .cloned()
            .unwrap_or_else(|| TransitionOutcome::saved("Saved")))
    }

    /// A transition with a raw answer ([`FakeLink::answer_raw`]) and no transition answer sends that file.
    fn transition_reply(&self, url: &str, method: &str, fields: &BTreeMap<String, String>) -> Reply {
        let file = {
            let answers = self.answers();
            (!answers.transitions.contains_key(url))
                .then(|| answers.raw.get(url).cloned())
                .flatten()
        };
        match file {
            Some(body) => match self.record(LinkCall::Transition {
                url: url.to_string(),
                method: method.to_string(),
                fields: fields.clone(),
            }) {
                Ok(()) => Reply::Raw(Ok(body)),
                Err(err) => Reply::Transition(Err(err)),
            },
            None => Reply::Transition(self.transition(url, method, fields)),
        }
    }

    fn options(&self, url: &str, parameters: &BTreeMap<String, String>) -> Result<Vec<Choice>, LinkError> {
        self.record(LinkCall::Options {
            url: url.to_string(),
            parameters: parameters.clone(),
        })?;
        let answers = self.answers();
        let Some(list) = answers.options.get(url) else {
            return Ok(Vec::new());
        };
        let matched = list
            .iter()
            .find(|answer| answer.parameters == *parameters)
            .or_else(|| list.iter().find(|answer| answer.parameters.is_empty()));
        Ok(matched.map(|answer| answer.choices.clone()).unwrap_or_default())
    }

    fn shell(&self, path: &str) -> Result<ShellInfo, LinkError> {
        self.record(LinkCall::Shell { path: path.to_string() })?;
        Ok(self.answers().shell.clone())
    }

    fn json(
        &self,
        url: &str,
        method: &str,
        parameters: &BTreeMap<String, String>,
    ) -> Result<serde_json::Value, LinkError> {
        self.record(LinkCall::Json {
            url: url.to_string(),
            method: method.to_string(),
            parameters: parameters.clone(),
        })?;
        let answers = self.answers();
        let list = answers
            .json
            .get(url)
            .ok_or_else(|| LinkError::Missing(url.to_string()))?;
        list.iter()
            .find(|(asked, _)| asked == parameters)
            .or_else(|| list.iter().find(|(asked, _)| asked.is_empty()))
            .map(|(_, value)| value.clone())
            .ok_or_else(|| LinkError::Missing(url.to_string()))
    }

    fn fragment(&self, url: &str, parameters: &BTreeMap<String, String>) -> Result<ScreenModel, LinkError> {
        self.record(LinkCall::Fragment {
            url: url.to_string(),
            parameters: parameters.clone(),
        })?;
        let (screen, _) = url.split_once('?').unwrap_or((url, ""));
        self.levels
            .get(screen)
            .cloned()
            .ok_or_else(|| LinkError::Missing(screen.to_string()))
    }

    fn raw(&self, url: &str, parameters: &BTreeMap<String, String>) -> Result<RawBody, LinkError> {
        self.record(LinkCall::Raw {
            url: url.to_string(),
            parameters: parameters.clone(),
        })?;
        self.answers()
            .raw
            .get(url)
            .cloned()
            .ok_or_else(|| LinkError::Missing(url.to_string()))
    }

    fn login(&self, username: &str, password: &str, code: &str) -> Result<(), LinkError> {
        self.record(LinkCall::Login {
            username: username.to_string(),
            password: String::new(),
            code: code.to_string(),
        })?;
        let mut answers = self.answers();
        match &answers.password {
            Some(expected) if expected != password => Err(LinkError::Remote("Password incorrect".to_string())),
            _ => {
                if let Some((wanted, sendable)) = &answers.code {
                    if wanted != code {
                        let text = if code.is_empty() {
                            "Enter the code from your authenticator app"
                        } else {
                            "Authentication code is not valid"
                        };
                        return Err(LinkError::Code(CodeAsk {
                            text: text.to_string(),
                            sendable: sendable.clone(),
                        }));
                    }
                }
                answers.password = None;
                answers.code = None;
                Ok(())
            }
        }
    }

    fn send_code(&self, username: &str, _password: &str, factor: &str) -> Result<String, LinkError> {
        self.record(LinkCall::SendCode {
            username: username.to_string(),
            password: String::new(),
            factor: factor.to_string(),
        })?;
        let answers = self.answers();
        let sendable = answers
            .code
            .as_ref()
            .map(|(_, sendable)| sendable.as_slice())
            .unwrap_or_default();
        sendable
            .iter()
            .find(|each| each.id == factor)
            .map(|each| each.label.clone())
            .ok_or_else(|| LinkError::Remote(format!("no factor {factor}")))
    }

    fn initial_admin(&self) -> Result<Option<AdminSetup>, LinkError> {
        self.record(LinkCall::InitialAdmin)?;
        Ok(self.answers().admin.clone())
    }

    /// Recorded without the passwords. A creation while setup is needed and none is refused makes the account: a login
    /// then takes its password.
    fn create_initial_admin(&self, account: &AdminAccount) -> Result<(), LinkError> {
        self.record(LinkCall::CreateInitialAdmin(AdminAccount {
            password: String::new(),
            verify: String::new(),
            ..account.clone()
        }))?;
        let mut answers = self.answers();
        if let Some(text) = answers.admin_refusal.take() {
            return Err(LinkError::Remote(text));
        }
        if answers.admin.take().is_none() {
            return Err(LinkError::Remote(
                "Can only create initial admin account if there are no UserAccount records".to_string(),
            ));
        }
        if answers.password.is_some() {
            answers.password = Some(account.password.clone());
        }
        Ok(())
    }
}

pub fn gallery_link() -> FakeLink {
    FakeLink::from_json(&[GALLERY_LIST_JSON, GALLERY_DETAIL_JSON]).expect("gallery fixtures parse")
}

/// One page of each list, as the server sends it: `pageIndex`, else the first.
fn page_model(mut model: ScreenModel, parameters: &BTreeMap<String, String>) -> ScreenModel {
    let index = parameters
        .get("pageIndex")
        .and_then(|text| text.parse::<usize>().ok())
        .unwrap_or(0);
    page_nodes(&mut model.c, index);
    model
}

fn page_nodes(nodes: &mut [Node], index: usize) {
    for node in nodes {
        if let Node::FormList { list } = node {
            let size = if list.paging.size == 0 {
                PAGE_ROWS
            } else {
                usize::try_from(list.paging.size).unwrap_or(PAGE_ROWS).max(1)
            };
            if list.rows.len() > size {
                if list.paging.count == 0 {
                    list.paging.count = u32::try_from(list.rows.len()).unwrap_or(u32::MAX);
                }
                let start = index.saturating_mul(size);
                list.rows = list.rows.iter().skip(start).take(size).cloned().collect();
            }
            list.paging.index = u32::try_from(index).unwrap_or(u32::MAX);
            if list.paging.size == 0 {
                list.paging.size = u32::try_from(size).unwrap_or(u32::MAX);
            }
        }
        page_nodes(node.children_mut(), index);
    }
}

/// The link Right and Enter open on a row, as its id and url: its first navigation link to another screen, else its first
/// link back to the screen shown (`here`, its path), as a list beside its detail selects a row with one. A link to the
/// screen shown is often a filter or a toggle that stages a change, so a link that opens the record wins over it. A row
/// action that posts or writes stays on its own key and on F10. None for the show-total row.
pub fn row_link_target<'a>(row: &'a ListRow, here: &str) -> Option<(&'a str, &'a str)> {
    if row.total {
        return None;
    }
    let mut links = row.cells.iter().flat_map(Cell::nodes).filter_map(|node| match node {
        Node::Link { id, url, .. } if navigates(node) => Some((id.as_str(), url.as_str())),
        _ => None,
    });
    let first = links.next()?;
    if !same_screen(first.1, here) {
        return Some(first);
    }
    Some(links.find(|(_, url)| !same_screen(url, here)).unwrap_or(first))
}

/// A link that goes somewhere with a GET: not disabled, not a write, and not a hidden form's post.
pub fn navigates(node: &Node) -> bool {
    matches!(node, Node::Link {
        url,
        disabled,
        writes,
        kind,
        method,
        ..
    } if !url.is_empty()
        && !disabled
        && !writes
        && kind != "hidden-form"
        && kind != "hidden-form-link"
        && !method.eq_ignore_ascii_case("post"))
}

/// Whether `url` is the screen at the path `here`, whatever its parameters.
pub fn same_screen(url: &str, here: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path == "." || path.trim_end_matches('/') == here.trim_end_matches('/')
}

/// The url of the row's first navigation link, whatever screen it goes to; Right and Enter open
/// [`row_link_target`], which prefers a link to another screen.
pub fn row_link(row: &ListRow) -> Option<&str> {
    row_link_target(row, "").map(|(_, url)| url)
}
