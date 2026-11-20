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

//! The sealed start (plans/Sealed-Secrets.md §3.1, §3.3, §3.5): a root key made once at `init`, kept wrapped in
//! `seal.json` (`file`) and in memory only while the server is unsealed, in a [`Keeper`] per runtime directory. The
//! states are sealed, unsealing (shares being entered) and unsealed, and back to sealed on `seal`, which drops the key.
//!
//! The factory runs the gate ([`GateRequest`]) right after the conf is merged, before the generated secrets, the
//! database and every listener. A server boot passes the host's hook, which opens the operator socket and waits there
//! while sealed; any other boot (a test, `moqui load`) opens the seal unattended ([`open_unattended`]). The first start
//! with no `seal.json` (§6): a purpose other than `production` makes the `dev` seal by itself; a production instance
//! waits for `moqui operator init`, unless `seal@method` names an automatic method, which then makes its seal and holds
//! the recovery shares in memory until `moqui operator init --take-shares` reads them once. The method recorded in
//! `seal.json` wins over the conf, and the `dev` seal is refused on a production instance (§3.8).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, RwLock};
use std::time::{Duration, Instant};

use base64::Engine as _;
use moqui_lib::node::MNode;
use moqui_lib::sync::{lock, read, write};
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::Digest as _;
use zeroize::Zeroizing;

use crate::context::conf_env::{conf_attribute, PURPOSE};
use crate::context::Error;

pub mod file;
pub mod methods;

pub use file::SealFile;
pub use methods::{Method, MethodKind};

/// The settings of the `seal` conf element (plans/Sealed-Secrets.md §3.3), read at the gate from the merged conf.
#[derive(Clone, Debug)]
pub struct SealConf {
    /// `seal@method`: the method `init` uses when none is named, and on a production instance's first start an
    /// automatic one makes its seal by itself. Empty for none. Once `seal.json` exists its method wins.
    pub method: String,
    /// `seal@shares` and `seal@threshold`: the recovery shares made at init, and those of the `shamir` method.
    pub shares: u8,
    pub threshold: u8,
    /// `seal@systemd-creds-command`: the `systemd-creds` program of the `tpm` method.
    pub systemd_creds_command: String,
    /// `seal@unseal-max-delay-seconds`: the longest wait the doubling delay after a wrong passphrase or share reaches.
    pub unseal_max_delay: Duration,
    /// `instance@purpose`.
    pub purpose: String,
}

impl SealConf {
    /// The settings from a merged conf.
    ///
    /// # Errors
    ///
    /// Share counts that are not numbers, a threshold under 2 or over the shares, or a method that is none.
    pub fn from_conf(conf: &MNode) -> Result<Self, Error> {
        let number = |selector: &str, default: u64| -> Result<u64, Error> {
            let text = conf_attribute(conf, selector);
            if text.is_empty() {
                return Ok(default);
            }
            text.parse()
                .map_err(|_| Error::new(format!("{selector} is {text:?}, not a whole number")))
        };
        let shares = u8::try_from(number("seal@shares", 5)?).unwrap_or(u8::MAX);
        let threshold = u8::try_from(number("seal@threshold", 3)?).unwrap_or(u8::MAX);
        if threshold < 2 || threshold > shares {
            return Err(Error::new(format!(
                "seal@threshold {threshold} must be at least 2 and at most seal@shares {shares}"
            )));
        }
        let method = conf_attribute(conf, "seal@method");
        if !method.is_empty() {
            MethodKind::parse(&method).map_err(|err| Error::new(format!("seal@method: {err}")))?;
        }
        let command = conf_attribute(conf, "seal@systemd-creds-command");
        Ok(Self {
            method,
            shares,
            threshold,
            systemd_creds_command: if command.is_empty() {
                "systemd-creds".to_string()
            } else {
                command
            },
            unseal_max_delay: Duration::from_secs(number("seal@unseal-max-delay-seconds", 300)?),
            purpose: conf_attribute(conf, PURPOSE),
        })
    }

    /// The settings with their defaults, for a purpose.
    #[must_use]
    pub fn with_purpose(purpose: &str) -> Self {
        Self {
            method: String::new(),
            shares: 5,
            threshold: 3,
            systemd_creds_command: "systemd-creds".to_string(),
            unseal_max_delay: Duration::from_secs(300),
            purpose: purpose.to_string(),
        }
    }

    /// Whether the instance's purpose is `production`.
    #[must_use]
    pub fn production(&self) -> bool {
        self.purpose == "production"
    }

    /// The method `seal@method` names, if any.
    #[must_use]
    pub fn method_kind(&self) -> Option<MethodKind> {
        MethodKind::parse(&self.method).ok()
    }
}

/// The root key: 256 random bits, in memory only while unsealed, cleared when dropped. `Debug` shows nothing of it.
pub struct RootKey(Zeroizing<[u8; 32]>);

impl RootKey {
    /// A new random key.
    ///
    /// # Errors
    ///
    /// The system has no random bytes.
    pub fn random() -> Result<Self, Error> {
        let mut key = Zeroizing::new([0_u8; 32]);
        getrandom::fill(key.as_mut()).map_err(|err| Error::new(format!("no random bytes for a root key: {err}")))?;
        Ok(Self(key))
    }

    /// A key from its 32 bytes.
    ///
    /// # Errors
    ///
    /// Not 32 bytes.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, Error> {
        let array: [u8; 32] = bytes
            .try_into()
            .map_err(|_| Error::new(format!("a root key is 32 bytes, not {}", bytes.len())))?;
        Ok(Self(Zeroizing::new(array)))
    }

    /// The key's bytes, for a method that wraps it.
    #[must_use]
    pub fn expose(&self) -> &[u8; 32] {
        &self.0
    }

    /// Names the key without giving it: the first 16 bytes of SHA-256 over a label and the key, hex.
    #[must_use]
    pub fn id(&self) -> String {
        let mut digest = sha2::Sha256::new();
        digest.update(b"moqui-seal root id\0");
        digest.update(self.0.as_ref());
        digest.finalize()[..16]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

impl std::fmt::Debug for RootKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RootKey(..)")
    }
}

/// Bytes as the base64 seal.json keeps them in.
#[must_use]
pub fn encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Bytes from seal.json's base64.
///
/// # Errors
///
/// Text that is not base64.
pub fn decode(text: &str) -> Result<Vec<u8>, Error> {
    base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|err| Error::new(format!("not base64: {err}")))
}

/// The three states (§3.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Sealed,
    Unsealing,
    Unsealed,
}

impl State {
    /// The state's name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Sealed => "sealed",
            Self::Unsealing => "unsealing",
            Self::Unsealed => "unsealed",
        }
    }
}

/// What `moqui operator status` shows: nothing secret.
#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub state: State,
    /// Whether this runtime has a `seal.json`.
    pub initialized: bool,
    /// The method `seal.json` records, empty before init.
    pub method: String,
    /// Shares entered so far while unsealing, and how many are needed.
    pub entered: usize,
    pub needed: usize,
    /// Which shares are being entered: `method` (the `shamir` method's) or `recovery`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entering: Option<&'static str>,
    /// The recovery shares' threshold and count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<(u8, u8)>,
    /// Why the server is sealed, when there is more to say (`auto-unseal failed: ...`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// How long until the next unseal attempt is taken, after a wrong passphrase or share.
    pub retry_after_ms: u64,
    /// Wrong passphrases or shares since the last unseal.
    pub failures: u32,
    /// An automatic init holds recovery shares that `moqui operator init --take-shares` has not read.
    pub shares_waiting: bool,
    /// The seal's generation and root id (both public).
    pub generation: u64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub root_id: String,
    /// The boot runs with a root key in memory only, no seal made (an unattended production boot with no seal.json).
    pub ephemeral: bool,
}

/// What an operator enters at `unseal`.
pub enum UnsealInput {
    /// An automatic method's unseal, after `seal` or a failed start.
    Automatic,
    /// The `passphrase` method's passphrase.
    Passphrase(Zeroizing<String>),
    /// One share of the `shamir` method.
    Share(Zeroizing<String>),
    /// One recovery share, which opens any method's seal.
    Recovery(Zeroizing<String>),
}

/// What `init` makes the seal with.
pub struct Init {
    pub method: MethodKind,
    /// The `passphrase` method's passphrase.
    pub passphrase: Option<Zeroizing<String>>,
}

/// What `init`, `rekey` and `rotate-root` hand out once: printed to the operator, never stored on the server.
#[derive(Default)]
pub struct Issued {
    /// The recovery shares (made at init only; `rekey` and `rotate-root` keep them).
    pub recovery_shares: Vec<Zeroizing<String>>,
    /// The `shamir` method's shares.
    pub method_shares: Vec<Zeroizing<String>>,
}

impl std::fmt::Debug for Issued {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Issued")
            .field("recovery_shares", &self.recovery_shares.len())
            .field("method_shares", &self.method_shares.len())
            .finish()
    }
}

/// What a start comes to.
#[derive(Debug, PartialEq, Eq)]
pub enum Start {
    /// The root key is in memory.
    Unsealed,
    /// The server waits for an operator, for this reason.
    Waiting(String),
}

/// Who opens the seal at a start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// A boot with no operator socket (a test, `moqui load`): an automatic method or nothing. A production instance with
    /// no seal runs with a root key in memory only, and none is made: the server's start makes it.
    Unattended,
    /// A server boot: the host's socket is open and the boot waits there while sealed.
    Operator,
}

/// A callback run when the keeper is sealed ([`Keeper::on_seal`]).
pub type SealCallback = Arc<dyn Fn() + Send + Sync>;

/// The keeper of one runtime directory's seal: its file, its state and, while unsealed, the root key.
pub struct Keeper {
    path: PathBuf,
    conf: RwLock<SealConf>,
    inner: Mutex<Inner>,
    changed: Condvar,
    /// One unseal attempt at a time (§3.3, Unseal attempts).
    attempt: Mutex<()>,
    /// The first wait after a wrong passphrase or share; it doubles with each one after.
    base_delay: Mutex<Duration>,
    watchers: Mutex<Vec<(u64, SealCallback)>>,
    next_watch: AtomicU64,
}

#[derive(Default)]
struct Inner {
    file: Option<SealFile>,
    root: Option<RootKey>,
    /// Shares entered toward an unseal, and which kind.
    entered: Vec<Zeroizing<String>>,
    entering: Option<&'static str>,
    failures: u32,
    retry_at: Option<Instant>,
    reason: String,
    /// The recovery shares an automatic init made, until `init --take-shares` reads them.
    held_shares: Option<Vec<Zeroizing<String>>>,
    /// `seal` ran: a start waits for `unseal` even with an automatic method.
    operator_sealed: bool,
    ephemeral: bool,
    /// The wait is given up (a test or a server stopping while sealed).
    abandoned: bool,
}

impl std::fmt::Debug for Keeper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keeper")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// The keepers of this process, by runtime directory.
static KEEPERS: LazyLock<Mutex<HashMap<PathBuf, Arc<Keeper>>>> = LazyLock::new(Mutex::default);

/// The keeper of `runtime_dir`, made on first use with `conf`; a later call gives it the newer conf.
///
/// # Errors
///
/// An existing `seal.json` that cannot be read.
pub fn keeper_for(runtime_dir: &Path, conf: SealConf) -> Result<Arc<Keeper>, Error> {
    let mut keepers = lock(&KEEPERS);
    if let Some(keeper) = keepers.get(runtime_dir) {
        *write(&keeper.conf) = conf;
        return Ok(Arc::clone(keeper));
    }
    let keeper = Arc::new(Keeper::open(&file::path_in(runtime_dir), conf)?);
    keepers.insert(runtime_dir.to_path_buf(), Arc::clone(&keeper));
    Ok(keeper)
}

/// The keeper of `runtime_dir`, when a gate has made one in this process.
#[must_use]
pub fn keeper_of(runtime_dir: &Path) -> Option<Arc<Keeper>> {
    let keepers = lock(&KEEPERS);
    keepers.get(runtime_dir).cloned().or_else(|| {
        // the factory keys its keeper by the canonical directory
        let canonical = std::fs::canonicalize(runtime_dir).ok()?;
        keepers.get(&canonical).cloned()
    })
}

/// What the factory's gate is given: the runtime directory and the merged conf.
pub struct GateRequest<'a> {
    pub runtime_dir: &'a Path,
    pub conf: &'a MNode,
}

impl GateRequest<'_> {
    /// The keeper of this runtime, with the conf's seal settings.
    ///
    /// # Errors
    ///
    /// Settings that do not parse, or a `seal.json` that cannot be read.
    pub fn keeper(&self) -> Result<Arc<Keeper>, Error> {
        keeper_for(self.runtime_dir, SealConf::from_conf(self.conf)?)
    }
}

/// The hook in `BootOptions::unseal`: a server's gate, which returns once the seal is open (or the boot is to stop).
pub type UnsealHook = Arc<dyn Fn(&GateRequest<'_>) -> Result<(), Error> + Send + Sync>;

/// The gate of a boot with no hook: open the seal with no operator ([`Mode::Unattended`]).
///
/// # Errors
///
/// A seal that needs an operator, a failed automatic unseal, or the `dev` seal on a production instance.
pub fn open_unattended(request: &GateRequest<'_>) -> Result<(), Error> {
    // the browser host keeps no server keys, and its runtime is not a directory it writes
    if cfg!(target_arch = "wasm32") {
        return Ok(());
    }
    match request.keeper()?.start(Mode::Unattended)? {
        Start::Unsealed => Ok(()),
        Start::Waiting(reason) => Err(Error::new(reason)),
    }
}

/// Removes its callback from the keeper when dropped.
#[must_use = "the callback is removed when the watch drops"]
pub struct SealWatch {
    keeper: Arc<Keeper>,
    id: u64,
}

impl Drop for SealWatch {
    fn drop(&mut self) {
        lock(&self.keeper.watchers).retain(|(id, _)| *id != self.id);
    }
}

/// The error of the dev seal on a production instance (§3.8).
fn dev_refused() -> Error {
    Error::new(
        "the dev seal is refused on a production instance (plans/Sealed-Secrets.md §3.8): instance@purpose is \
         production; seal it with another method (moqui operator rekey, with the recovery shares) or set the purpose",
    )
}

impl Keeper {
    /// A keeper of the seal at `path`, loaded when the file is there.
    ///
    /// # Errors
    ///
    /// The file cannot be read or parsed.
    pub fn open(path: &Path, conf: SealConf) -> Result<Self, Error> {
        file::remove_stale_temps(path);
        let loaded = file::load(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            conf: RwLock::new(conf),
            inner: Mutex::new(Inner {
                file: loaded,
                ..Inner::default()
            }),
            changed: Condvar::new(),
            attempt: Mutex::new(()),
            base_delay: Mutex::new(Duration::from_secs(1)),
            watchers: Mutex::new(Vec::new()),
            next_watch: AtomicU64::new(1),
        })
    }

    /// Where the seal file is.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The seal settings this keeper was last given.
    #[must_use]
    pub fn conf(&self) -> SealConf {
        read(&self.conf).clone()
    }

    /// The first wait after a wrong passphrase or share (one second); tests shorten it.
    #[doc(hidden)]
    pub fn set_base_delay(&self, delay: Duration) {
        *lock(&self.base_delay) = delay;
    }

    /// The state, and what an operator needs to know.
    #[must_use]
    pub fn status(&self) -> Status {
        let inner = lock(&self.inner);
        self.status_of(&inner)
    }

    fn status_of(&self, inner: &Inner) -> Status {
        let state = if inner.root.is_some() {
            State::Unsealed
        } else if inner.entered.is_empty() {
            State::Sealed
        } else {
            State::Unsealing
        };
        let needed = match (inner.entering, inner.file.as_ref()) {
            (Some("recovery"), Some(file)) => file.recovery.as_ref().map_or(0, |recovery| recovery.threshold.into()),
            (Some(_), Some(file)) => file
                .method
                .get("threshold")
                .and_then(Value::as_u64)
                .and_then(|threshold| usize::try_from(threshold).ok())
                .unwrap_or(0),
            _ => 0,
        };
        let retry_after_ms = inner
            .retry_at
            .and_then(|at| at.checked_duration_since(Instant::now()))
            .map_or(0, |wait| u64::try_from(wait.as_millis()).unwrap_or(u64::MAX));
        Status {
            state,
            initialized: inner.file.is_some(),
            method: inner
                .file
                .as_ref()
                .map(|file| file.method_kind().to_string())
                .unwrap_or_default(),
            entered: inner.entered.len(),
            needed,
            entering: inner.entering,
            recovery: inner
                .file
                .as_ref()
                .and_then(|file| file.recovery.as_ref())
                .map(|recovery| (recovery.threshold, recovery.shares)),
            reason: inner.reason.clone(),
            retry_after_ms,
            failures: inner.failures,
            shares_waiting: inner.held_shares.is_some(),
            generation: inner.file.as_ref().map_or(0, |file| file.generation),
            root_id: inner.file.as_ref().map(|file| file.root_id.clone()).unwrap_or_default(),
            ephemeral: inner.ephemeral,
        }
    }

    /// Whether the root key is in memory.
    #[must_use]
    pub fn is_unsealed(&self) -> bool {
        lock(&self.inner).root.is_some()
    }

    /// The seal file as this keeper has it, if any.
    #[must_use]
    pub fn seal_file(&self) -> Option<SealFile> {
        lock(&self.inner).file.clone()
    }

    /// Open the seal at a start (§3.3, §6 "A first start with no seal.json").
    ///
    /// # Errors
    ///
    /// The `dev` seal on a production instance, a file that cannot be written, or, unattended, a failed automatic
    /// unseal.
    pub fn start(&self, mode: Mode) -> Result<Start, Error> {
        let conf = self.conf();
        let mut inner = lock(&self.inner);
        if inner.root.is_some() {
            return Ok(Start::Unsealed);
        }
        if inner.file.is_none() {
            // a file another process wrote since this keeper was made
            inner.file = file::load(&self.path)?;
        }
        let Some(kind) = inner.file.as_ref().map(|file| file.method_kind().to_string()) else {
            return self.first_start(&mut inner, &conf, mode);
        };
        let kind = MethodKind::parse(&kind).map_err(|err| Error::new(format!("{}: {err}", self.path.display())))?;
        if !conf.method.is_empty() && conf.method != kind.name() {
            tracing::warn!(
                "seal@method names {}, but this runtime's seal.json records {}: the recorded method is used, and only \
                 `moqui operator rekey` changes it (plans/Sealed-Secrets.md §3.3)",
                conf.method,
                kind.name()
            );
        }
        if kind == MethodKind::Dev && conf.production() {
            return Err(dev_refused());
        }
        if inner.operator_sealed {
            return Ok(Start::Waiting(
                "sealed by `moqui operator seal`: waiting for `moqui operator unseal`".to_string(),
            ));
        }
        if !kind.automatic() {
            return match mode {
                Mode::Operator => Ok(Start::Waiting(format!(
                    "sealed: waiting for `moqui operator unseal` ({})",
                    kind.name()
                ))),
                Mode::Unattended => Err(Error::new(format!(
                    "this runtime's seal ({}) is opened by an operator on the server's operator socket: start the \
                     server and run `moqui operator unseal`",
                    kind.name()
                ))),
            };
        }
        match self.unwrap_automatic(&inner, kind, &conf) {
            Ok(root) => {
                self.opened(&mut inner, root);
                tracing::info!("Unsealed by the {} method", kind.name());
                Ok(Start::Unsealed)
            }
            Err(err) => {
                inner.reason = format!("auto-unseal failed: {err}");
                tracing::error!(
                    "The {} method did not unseal this server: {err}. It stays sealed; an operator can unseal it with \
                     the recovery shares (moqui operator unseal --recovery)",
                    kind.name()
                );
                match mode {
                    Mode::Operator => Ok(Start::Waiting(inner.reason.clone())),
                    Mode::Unattended => Err(Error::new(inner.reason.clone())),
                }
            }
        }
    }

    /// A start with no `seal.json`.
    fn first_start(&self, inner: &mut Inner, conf: &SealConf, mode: Mode) -> Result<Start, Error> {
        if !conf.production() {
            if conf.method_kind().is_some_and(|kind| kind != MethodKind::Dev) {
                tracing::info!(
                    "seal@method is {}, but a first start on a {} instance makes the dev seal; `moqui operator rekey` \
                     moves it to another method",
                    conf.method,
                    if conf.purpose.is_empty() {
                        "unset"
                    } else {
                        &conf.purpose
                    }
                );
            }
            return self.init_locked(inner, conf, MethodKind::Dev, None).map(|_| {
                tracing::info!(
                    "Made the dev seal in {} (not a production instance)",
                    self.path.display()
                );
                Start::Unsealed
            });
        }
        if let Some(kind) = conf
            .method_kind()
            .filter(|kind| kind.automatic() && *kind != MethodKind::Dev)
        {
            let issued = self.init_locked(inner, conf, kind, None)?;
            if !issued.recovery_shares.is_empty() {
                inner.held_shares = Some(issued.recovery_shares);
                tracing::error!(
                    "Made the {} seal at this first start: its recovery shares are held in memory until `moqui \
                     operator init --take-shares` reads them, and are lost if the server stops first",
                    kind.name()
                );
            }
            return Ok(Start::Unsealed);
        }
        match mode {
            Mode::Operator => Ok(Start::Waiting(
                "no seal yet: waiting for `moqui operator init` (a production instance)".to_string(),
            )),
            Mode::Unattended => {
                inner.root = Some(RootKey::random()?);
                inner.ephemeral = true;
                tracing::info!(
                    "No seal.json on this production instance: this boot holds a root key in memory only; the server's \
                     start waits for `moqui operator init`"
                );
                Ok(Start::Unsealed)
            }
        }
    }

    /// Block until the seal is open.
    ///
    /// # Errors
    ///
    /// The wait was given up ([`Self::abandon`]).
    pub fn wait_unsealed(&self) -> Result<(), Error> {
        let mut inner = lock(&self.inner);
        loop {
            if inner.root.is_some() {
                return Ok(());
            }
            if inner.abandoned {
                inner.abandoned = false;
                return Err(Error::new("stopped while sealed"));
            }
            inner = moqui_lib::sync::wait(&self.changed, inner);
        }
    }

    /// Give up a [`Self::wait_unsealed`] in progress: a server stopping while sealed.
    pub fn abandon(&self) {
        lock(&self.inner).abandoned = true;
        self.changed.notify_all();
    }

    /// Make the seal: a new root key, wrapped by `request.method`, and `seal.json` written where none was.
    ///
    /// # Errors
    ///
    /// The runtime has a seal already, the method cannot wrap, the `dev` seal on a production instance, or the file
    /// cannot be written.
    pub fn init(&self, request: Init) -> Result<Issued, Error> {
        let conf = self.conf();
        let mut inner = lock(&self.inner);
        if inner.file.is_none() {
            inner.file = file::load(&self.path)?;
        }
        if inner.file.is_some() {
            return Err(Error::new(
                "this runtime has a seal already: `moqui operator rekey` changes its method",
            ));
        }
        self.init_locked(&mut inner, &conf, request.method, request.passphrase)
    }

    fn init_locked(
        &self,
        inner: &mut Inner,
        conf: &SealConf,
        kind: MethodKind,
        passphrase: Option<Zeroizing<String>>,
    ) -> Result<Issued, Error> {
        if kind == MethodKind::Dev && conf.production() {
            return Err(dev_refused());
        }
        let root = RootKey::random()?;
        let (method, issued) = self.wrap_method(&root, kind, passphrase.as_deref().map(String::as_str), conf)?;
        let seal = SealFile {
            format: file::FORMAT.to_string(),
            version: file::VERSION,
            created: jiff::Timestamp::now().to_string(),
            generation: 1,
            root_id: root.id(),
            method,
            recovery: None,
            bootstrap: std::collections::BTreeMap::new(),
            secret_value_key: None,
        };
        if !file::stage(&self.path, &seal)?.commit_new()? {
            // another process made this runtime's seal first: use that one
            inner.file = file::load(&self.path)?;
            let other = inner
                .file
                .as_ref()
                .map(|file| file.method_kind().to_string())
                .unwrap_or_default();
            let other = MethodKind::parse(&other)?;
            if other.automatic() {
                let root = self.unwrap_automatic(inner, other, conf)?;
                self.opened(inner, root);
                return Ok(Issued::default());
            }
            return Err(Error::new("another process made this runtime's seal at the same time"));
        }
        inner.file = Some(seal);
        self.opened(inner, root);
        Ok(issued)
    }

    /// The method's metadata for `root`, with `kind` in it, and the shares a method issues.
    fn wrap_method(
        &self,
        root: &RootKey,
        kind: MethodKind,
        _passphrase: Option<&str>,
        conf: &SealConf,
    ) -> Result<(Map<String, Value>, Issued), Error> {
        let mut metadata = methods::automatic(kind)?.wrap(root, conf)?;
        metadata.insert("kind".to_string(), Value::String(kind.name().to_string()));
        Ok((metadata, Issued::default()))
    }

    fn unwrap_automatic(&self, inner: &Inner, kind: MethodKind, conf: &SealConf) -> Result<RootKey, Error> {
        let file = inner
            .file
            .as_ref()
            .ok_or_else(|| Error::new("this runtime has no seal yet"))?;
        let root = methods::automatic(kind)?.unwrap(&file.method, conf)?;
        check_root(file, &root)?;
        Ok(root)
    }

    /// The root key is open: keep it, clear what led to it, and wake the waiters.
    fn opened(&self, inner: &mut Inner, root: RootKey) {
        inner.root = Some(root);
        inner.entered.clear();
        inner.entering = None;
        inner.failures = 0;
        inner.retry_at = None;
        inner.reason.clear();
        inner.operator_sealed = false;
        self.changed.notify_all();
    }

    /// One unseal step: an automatic method's unseal, a passphrase, or a share.
    ///
    /// # Errors
    ///
    /// Another attempt is running, the wait after a wrong entry has not passed, the runtime has no seal, or the entry
    /// is wrong (counted, and the next wait doubles).
    pub fn unseal(&self, input: UnsealInput) -> Result<Status, Error> {
        let Some(_attempt) = moqui_lib::sync::try_lock(&self.attempt) else {
            return Err(Error::new("another unseal attempt is running; try again when it ends"));
        };
        let conf = self.conf();
        let mut inner = lock(&self.inner);
        if inner.root.is_some() {
            return Ok(self.status_of(&inner));
        }
        if let Some(wait) = inner.retry_at.and_then(|at| at.checked_duration_since(Instant::now())) {
            return Err(Error::new(format!(
                "wait {:.1} s before the next unseal attempt (after a wrong entry)",
                wait.as_secs_f64()
            )));
        }
        if inner.file.is_none() {
            inner.file = file::load(&self.path)?;
        }
        let kind = match inner.file.as_ref() {
            Some(file) => MethodKind::parse(file.method_kind())?,
            None => {
                return Err(Error::new(
                    "this runtime has no seal yet: `moqui operator init` makes it",
                ))
            }
        };
        if kind == MethodKind::Dev && conf.production() {
            return Err(dev_refused());
        }
        let opened = match input {
            UnsealInput::Automatic if kind.automatic() => self.unwrap_automatic(&inner, kind, &conf).map(Some),
            UnsealInput::Automatic => Err(Error::new(format!(
                "this seal ({}) is opened by an operator: give the passphrase or a share",
                kind.name()
            ))),
            UnsealInput::Passphrase(_) | UnsealInput::Share(_) | UnsealInput::Recovery(_) => Err(Error::new(format!(
                "the {} method takes no passphrase or share here",
                kind.name()
            ))),
        };
        match opened {
            Ok(Some(root)) => {
                self.opened(&mut inner, root);
                tracing::info!("Unsealed by an operator ({})", kind.name());
            }
            Ok(None) => {}
            Err(err) => return Err(err),
        }
        Ok(self.status_of(&inner))
    }

    /// Count a wrong passphrase or share: the wait before the next attempt doubles (§3.3), and an alert is logged.
    #[allow(dead_code)]
    fn wrong_entry(&self, inner: &mut Inner, conf: &SealConf, what: &str) -> Error {
        inner.failures = inner.failures.saturating_add(1);
        let base = *lock(&self.base_delay);
        let doubled = base.saturating_mul(1_u32 << (inner.failures - 1).min(20));
        let wait = doubled.min(conf.unseal_max_delay.max(base));
        inner.retry_at = Some(Instant::now() + wait);
        tracing::error!(
            "Alert: a wrong {what} was entered to unseal this server ({} in a row); the next attempt waits {:.1} s",
            inner.failures,
            wait.as_secs_f64()
        );
        Error::new(format!(
            "wrong {what}: refused; the next attempt waits {:.1} s",
            wait.as_secs_f64()
        ))
    }

    /// Drop the root key and every share entered from memory now (§3.3, §3.5), and tell the watchers, which close the
    /// listeners. A start then waits for `unseal`, whatever the method.
    pub fn seal(&self) {
        {
            let mut inner = lock(&self.inner);
            inner.root = None;
            inner.entered.clear();
            inner.entering = None;
            inner.operator_sealed = true;
            inner.ephemeral = false;
            self.changed.notify_all();
        }
        tracing::warn!("Sealed by an operator: the root key is dropped from memory and the listeners close");
        let watchers: Vec<_> = lock(&self.watchers)
            .iter()
            .map(|(_, watcher)| Arc::clone(watcher))
            .collect();
        for watcher in watchers {
            watcher();
        }
    }

    /// Run `callback` when the keeper is sealed, until the returned watch drops.
    pub fn on_seal(self: &Arc<Self>, callback: SealCallback) -> SealWatch {
        let id = self.next_watch.fetch_add(1, Ordering::Relaxed);
        lock(&self.watchers).push((id, callback));
        SealWatch {
            keeper: Arc::clone(self),
            id,
        }
    }

    /// The recovery shares an automatic init held in memory, once (§6 "A first start with no seal.json").
    ///
    /// # Errors
    ///
    /// None are held: read already, or this seal was made by `moqui operator init`.
    pub fn take_shares(&self) -> Result<Vec<Zeroizing<String>>, Error> {
        lock(&self.inner)
            .held_shares
            .take()
            .ok_or_else(|| Error::new("no recovery shares are held: they were read already, or never held"))
            .inspect(|_| tracing::info!("The recovery shares held since this server's first start were read"))
    }
}

/// Check that `root` is the root key `file` names.
fn check_root(file: &SealFile, root: &RootKey) -> Result<(), Error> {
    use subtle::ConstantTimeEq as _;
    if bool::from(root.id().as_bytes().ct_eq(file.root_id.as_bytes())) {
        Ok(())
    } else {
        Err(Error::new("the unwrapped key is not this seal's root key"))
    }
}

#[cfg(test)]
mod tests;
