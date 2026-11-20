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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use base64::Engine as _;
use moqui_lib::node::MNode;
use moqui_lib::sync::{lock, read, write};
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::Digest as _;
use zeroize::Zeroizing;

use crate::context::conf_env::{conf_attribute, Override, Source, PURPOSE};
use crate::context::Error;

mod bootstrap;
pub mod export;
pub mod file;
pub mod methods;
mod recovery;
pub mod rotate;

pub use file::SealFile;
pub use methods::{Method, MethodKind};
pub use rotate::{Rekey, RotateRoot};

/// The settings of the `seal` conf element (plans/Sealed-Secrets.md §3.3), read at the gate from the merged conf.
#[derive(Clone, Debug)]
pub struct SealConf {
    /// `seal@method`: the method `init` uses when none is named, and on a production instance's first start an
    /// automatic one makes its seal by itself. Empty for none. Once `seal.json` exists its method wins.
    pub method: String,
    /// `seal@shares` and `seal@threshold`: the recovery shares made at init, and those of the `shamir` method.
    pub shares: u8,
    pub threshold: u8,
    /// `seal@systemd-creds-command`: the `systemd-creds` program of the `tpm` method, an absolute path; empty for the
    /// first of [`methods::tpm::PROGRAMS`] there is.
    pub systemd_creds_command: String,
    /// `seal@socket`: where a server with no socket handed to it by root binds its own operator socket (a development
    /// instance; not a protection boundary). Empty for `db/operator.sock` in the runtime.
    pub socket: String,
    /// `seal@unseal-max-delay-seconds`: the longest wait the doubling delay after a wrong passphrase or share reaches.
    pub unseal_max_delay: Duration,
    /// `instance@purpose`.
    pub purpose: String,
    /// What names the database a recovery bundle goes with: `instance@id` and the transactional datasource's
    /// database conf, host, name and schema (never its password).
    pub database_identity: Value,
}

impl SealConf {
    /// The settings from a merged conf, with no environment names applied over it.
    ///
    /// # Errors
    ///
    /// As [`Self::from_gate`].
    pub fn from_conf(conf: &MNode) -> Result<Self, Error> {
        Self::from_gate(conf, &[])
    }

    /// The settings from the merged conf of a boot and the environment names it applied. The settings that place the
    /// socket, pace the unseal attempts and name the program the `tpm` method runs come only from the conf files (the
    /// server's user can write `runtime/.env`, and its children set the environment): one an environment name or
    /// `runtime/.env` set is ignored, with an error logged, and its default is used.
    ///
    /// # Errors
    ///
    /// Share counts that are not numbers, a threshold under 2 or over the shares, a method that is none, or a
    /// `systemd-creds` program that is not an absolute path.
    pub fn from_gate(conf: &MNode, overrides: &[Override]) -> Result<Self, Error> {
        let protected = |selector: &str| -> String {
            let set_outside = overrides.iter().any(|applied| {
                applied.applied()
                    && applied.selector.as_deref() == Some(selector)
                    && matches!(applied.source, Source::Environment | Source::DotEnv)
            });
            if set_outside {
                tracing::error!(
                    "{selector} is set by an environment name or runtime/.env, which the server's own user can write: \
                     it is ignored and its default used; set it in a conf file (plans/Sealed-Secrets.md §3.3)"
                );
                return String::new();
            }
            conf_attribute(conf, selector)
        };
        let number = |selector: &str, default: u64, text: String| -> Result<u64, Error> {
            if text.is_empty() {
                return Ok(default);
            }
            text.parse()
                .map_err(|_| Error::new(format!("{selector} is {text:?}, not a whole number")))
        };
        let shares = u8::try_from(number("seal@shares", 5, conf_attribute(conf, "seal@shares"))?).unwrap_or(u8::MAX);
        let threshold =
            u8::try_from(number("seal@threshold", 3, conf_attribute(conf, "seal@threshold"))?).unwrap_or(u8::MAX);
        if threshold < 2 || threshold > shares {
            return Err(Error::new(format!(
                "seal@threshold {threshold} must be at least 2 and at most seal@shares {shares}"
            )));
        }
        let method = conf_attribute(conf, "seal@method");
        if !method.is_empty() {
            MethodKind::parse(&method).map_err(|err| Error::new(format!("seal@method: {err}")))?;
        }
        let command = protected("seal@systemd-creds-command");
        if !command.is_empty() && !Path::new(&command).is_absolute() {
            return Err(Error::new(format!(
                "seal@systemd-creds-command {command:?} is not an absolute path"
            )));
        }
        let max_delay = protected("seal@unseal-max-delay-seconds");
        Ok(Self {
            method,
            shares,
            threshold,
            systemd_creds_command: command,
            socket: protected("seal@socket"),
            unseal_max_delay: Duration::from_secs(number("seal@unseal-max-delay-seconds", 300, max_delay)?),
            purpose: conf_attribute(conf, PURPOSE),
            database_identity: database_identity(conf),
        })
    }

    /// The settings with their defaults, for a purpose.
    #[must_use]
    pub fn with_purpose(purpose: &str) -> Self {
        Self {
            method: String::new(),
            shares: 5,
            threshold: 3,
            systemd_creds_command: String::new(),
            socket: String::new(),
            unseal_max_delay: Duration::from_secs(300),
            purpose: purpose.to_string(),
            database_identity: Value::Null,
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

/// The bundle's database identity from a merged conf.
fn database_identity(conf: &MNode) -> Value {
    let datasource =
        |attribute: &str| conf_attribute(conf, &format!("entity-facade/datasource[transactional]@{attribute}"));
    serde_json::json!({
        "instance_id": conf_attribute(conf, "instance@id"),
        "database_conf_name": datasource("database-conf-name"),
        "schema_name": datasource("schema-name"),
        "host": conf_attribute(conf, "entity-facade/datasource[transactional]/inline-other@host"),
        "database": conf_attribute(conf, "entity-facade/datasource[transactional]/inline-other@database"),
    })
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    #[default]
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
#[derive(Clone, Debug, Default, Serialize)]
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
    /// A server boot where there is no operator socket (not a Unix host): as unattended, but a production instance
    /// with no seal, or a seal that needs an operator, does not start.
    NoSocket,
}

/// A callback run when the keeper is sealed ([`Keeper::on_seal`]).
pub type SealCallback = Arc<dyn Fn() + Send + Sync>;

/// The keeper of one runtime directory's seal: its file, its state and, while unsealed, the root key.
pub struct Keeper {
    path: PathBuf,
    conf: RwLock<SealConf>,
    inner: Mutex<Inner>,
    /// What `status` shows, published whenever the state changes, so `status` never waits on a slow unseal
    /// (Argon2id, `systemd-creds`) that holds `inner`.
    snapshot: RwLock<(Status, Option<Instant>)>,
    /// Whether the root key is in memory, and whether the wait for it is given up: read with no lock, by the signal
    /// path among others.
    unsealed: AtomicBool,
    abandoned: AtomicBool,
    /// The mutex `changed` waits with.
    wake: Mutex<()>,
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
    /// `seal` ran: a start waits for `unseal` even with an automatic method. Kept on disk beside seal.json
    /// ([`Keeper::marker`]), so a restart stays sealed too.
    operator_sealed: bool,
    ephemeral: bool,
}

/// The keeper's state, locked; publishes what `status` shows when dropped.
struct InnerGuard<'a> {
    keeper: &'a Keeper,
    guard: MutexGuard<'a, Inner>,
}

impl std::ops::Deref for InnerGuard<'_> {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.guard
    }
}

impl std::ops::DerefMut for InnerGuard<'_> {
    fn deref_mut(&mut self) -> &mut Inner {
        &mut self.guard
    }
}

impl Drop for InnerGuard<'_> {
    fn drop(&mut self) {
        let status = self.keeper.status_of(&self.guard);
        self.keeper.unsealed.store(self.guard.root.is_some(), Ordering::SeqCst);
        *write(&self.keeper.snapshot) = (status, self.guard.retry_at);
    }
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

/// What the factory's gate is given: the runtime directory, the merged conf, and the environment names applied over it.
pub struct GateRequest<'a> {
    pub runtime_dir: &'a Path,
    pub conf: &'a MNode,
    pub overrides: &'a [Override],
}

impl GateRequest<'_> {
    /// The keeper of this runtime, with the conf's seal settings.
    ///
    /// # Errors
    ///
    /// Settings that do not parse, or a `seal.json` that cannot be read.
    pub fn keeper(&self) -> Result<Arc<Keeper>, Error> {
        keeper_for(self.runtime_dir, SealConf::from_gate(self.conf, self.overrides)?)
    }
}

/// The hook in `BootOptions::unseal`: a server's gate, which returns once the seal is open (or the boot is to stop).
pub type UnsealHook = Arc<dyn Fn(&GateRequest<'_>) -> Result<(), Error> + Send + Sync>;

/// The gate of a server boot on a host with no operator socket ([`Mode::NoSocket`]).
///
/// # Errors
///
/// A production instance with no seal, a seal that needs an operator, or a failed automatic unseal.
pub fn open_without_socket(request: &GateRequest<'_>) -> Result<(), Error> {
    match request.keeper()?.start(Mode::NoSocket)? {
        Start::Unsealed => Ok(()),
        Start::Waiting(reason) => Err(Error::new(reason)),
    }
}

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
        let operator_sealed = marker_of(path).exists();
        let keeper = Self {
            path: path.to_path_buf(),
            conf: RwLock::new(conf),
            inner: Mutex::new(Inner {
                file: loaded,
                operator_sealed,
                ..Inner::default()
            }),
            snapshot: RwLock::new((Status::default(), None)),
            unsealed: AtomicBool::new(false),
            abandoned: AtomicBool::new(false),
            wake: Mutex::new(()),
            changed: Condvar::new(),
            attempt: Mutex::new(()),
            base_delay: Mutex::new(Duration::from_secs(1)),
            watchers: Mutex::new(Vec::new()),
            next_watch: AtomicU64::new(1),
        };
        drop(keeper.inner());
        Ok(keeper)
    }

    /// The state, locked.
    fn inner(&self) -> InnerGuard<'_> {
        InnerGuard {
            keeper: self,
            guard: lock(&self.inner),
        }
    }

    /// The marker of a seal an operator closed: present, a start waits for `unseal` whatever the method.
    fn marker(&self) -> PathBuf {
        marker_of(&self.path)
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
        let (mut status, retry_at) = read(&self.snapshot).clone();
        status.retry_after_ms = retry_at
            .and_then(|at| at.checked_duration_since(Instant::now()))
            .map_or(0, |wait| u64::try_from(wait.as_millis()).unwrap_or(u64::MAX));
        status
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
        self.unsealed.load(Ordering::SeqCst)
    }

    /// The seal file as this keeper has it, if any.
    #[must_use]
    pub fn seal_file(&self) -> Option<SealFile> {
        self.inner().file.clone()
    }

    /// Open the seal at a start (§3.3, §6 "A first start with no seal.json").
    ///
    /// # Errors
    ///
    /// The `dev` seal on a production instance, a file that cannot be written, or, unattended, a failed automatic
    /// unseal.
    pub fn start(&self, mode: Mode) -> Result<Start, Error> {
        let conf = self.conf();
        let mut inner = self.inner();
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
                Mode::Unattended | Mode::NoSocket => Err(Error::new(format!(
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
                    Mode::Unattended | Mode::NoSocket => Err(Error::new(inner.reason.clone())),
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
        // a boot that is not the server's (moqui load, a tool, a test) never makes a production seal: the recovery
        // shares of one made here would end with the process
        if mode == Mode::Unattended {
            inner.root = Some(RootKey::random()?);
            inner.ephemeral = true;
            tracing::info!(
                "No seal.json on this production instance: this boot holds a root key in memory only and makes no \
                 seal; the server's start makes it"
            );
            return Ok(Start::Unsealed);
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
            Mode::NoSocket | Mode::Unattended => Err(Error::new(
                "a production server starts only with its seal, and this host has no operator socket to make one: set \
                 seal@method to an automatic method, or make the seal on a Unix host and bring its seal.json",
            )),
        }
    }

    /// Block until the seal is open.
    ///
    /// # Errors
    ///
    /// The wait was given up ([`Self::abandon`]).
    pub fn wait_unsealed(&self) -> Result<(), Error> {
        loop {
            if self.unsealed.load(Ordering::SeqCst) {
                return Ok(());
            }
            if self.abandoned.swap(false, Ordering::SeqCst) {
                return Err(Error::new("stopped while sealed"));
            }
            // a short wait, so a wake that came between the checks and the wait is not missed for long
            let wake = lock(&self.wake);
            drop(moqui_lib::sync::wait_timeout(
                &self.changed,
                wake,
                Duration::from_millis(200),
            ));
        }
    }

    /// Give up a [`Self::wait_unsealed`] in progress: a server stopping while sealed. Takes no lock, so a signal
    /// handler can call it while an unseal attempt runs.
    pub fn abandon(&self) {
        self.abandoned.store(true, Ordering::SeqCst);
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
        let mut inner = self.inner();
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
        let (method, mut issued) = self.wrap_method(&root, kind, passphrase.as_deref().map(String::as_str), conf)?;
        // recovery shares for every method (§3.3); the dev seal keeps its key in the file, so has nothing to recover
        let recovery = if kind == MethodKind::Dev {
            None
        } else {
            let (recovery, shares) = recovery::make(&root, conf)?;
            issued.recovery_shares = shares;
            Some(recovery)
        };
        let held = file::lock(&self.path)?;
        let seal = SealFile {
            format: file::FORMAT.to_string(),
            version: file::VERSION,
            created: jiff::Timestamp::now().to_string(),
            generation: 1,
            root_id: root.id(),
            method,
            recovery,
            bootstrap: std::collections::BTreeMap::new(),
            secret_value_key: None,
        };
        if !file::stage(&self.path, &seal)?.commit_new(&held)? {
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

    /// Under the seal file's lock: take the file on disk as this keeper's, refusing one whose root key or generation
    /// another process changed (a rekey or rotate-root there), so no write here undoes one there.
    fn refresh(&self, inner: &mut Inner, _held: &file::FileLock) -> Result<(), Error> {
        let disk = file::load(&self.path)?.ok_or_else(|| Error::new("this runtime has no seal yet"))?;
        if let Some(held) = inner.file.as_ref() {
            if held.root_id != disk.root_id || held.generation != disk.generation {
                return Err(Error::new(
                    "seal.json was changed by another process (a rekey or rotate-root there): restart this server",
                ));
            }
        }
        inner.file = Some(disk);
        Ok(())
    }

    /// The method's metadata for `root`, with `kind` in it, and the shares a method issues.
    fn wrap_method(
        &self,
        root: &RootKey,
        kind: MethodKind,
        passphrase: Option<&str>,
        conf: &SealConf,
    ) -> Result<(Map<String, Value>, Issued), Error> {
        let (mut metadata, issued) = match kind {
            MethodKind::Passphrase => {
                let passphrase = passphrase
                    .filter(|text| text.chars().count() >= 12)
                    .ok_or_else(|| Error::new("the passphrase method needs a passphrase of at least 12 characters"))?;
                (methods::passphrase::wrap(root, passphrase, conf)?, Issued::default())
            }
            MethodKind::Shamir => {
                let wrapped = methods::shamir::wrap(root, conf)?;
                (
                    wrapped.metadata,
                    Issued {
                        method_shares: wrapped.shares,
                        ..Issued::default()
                    },
                )
            }
            _ => (methods::automatic(kind)?.wrap(root, conf)?, Issued::default()),
        };
        metadata.insert("kind".to_string(), Value::String(kind.name().to_string()));
        Ok((metadata, issued))
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
        let marker = self.marker();
        if marker.exists() {
            if let Err(err) = std::fs::remove_file(&marker) {
                tracing::error!("Could not remove {}: {err}", marker.display());
            }
        }
        self.unsealed.store(true, Ordering::SeqCst);
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
        let mut inner = self.inner();
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
            UnsealInput::Passphrase(passphrase) if kind == MethodKind::Passphrase => {
                let file = inner
                    .file
                    .as_ref()
                    .ok_or_else(|| Error::new("this runtime has no seal yet"))?;
                match methods::passphrase::unwrap(&file.method, &passphrase, &conf)? {
                    Some(root) => check_root(file, &root).map(|()| Some(root)),
                    None => Err(self.wrong_entry(&mut inner, &conf, "passphrase")),
                }
            }
            UnsealInput::Passphrase(_) => Err(Error::new(format!(
                "this seal ({}) takes no passphrase; a recovery share opens any seal (--recovery)",
                kind.name()
            ))),
            UnsealInput::Share(share) if kind == MethodKind::Shamir => {
                self.enter_share(&mut inner, &conf, share, "method")
            }
            UnsealInput::Share(_) => Err(Error::new(format!(
                "this seal ({}) takes no method share; a recovery share opens any seal (--recovery)",
                kind.name()
            ))),
            UnsealInput::Recovery(share) => self.enter_share(&mut inner, &conf, share, "recovery"),
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

    /// One share toward an unseal: the `shamir` method's (`kind` `method`) or a recovery share. A share that is not
    /// one of the split's is refused at once and counted; the root key once the threshold is entered.
    fn enter_share(
        &self,
        inner: &mut Inner,
        conf: &SealConf,
        share: Zeroizing<String>,
        kind: &'static str,
    ) -> Result<Option<RootKey>, Error> {
        let file = inner
            .file
            .clone()
            .ok_or_else(|| Error::new("this runtime has no seal yet"))?;
        let (salt, hashes, threshold) = if kind == "recovery" {
            let recovery = file
                .recovery
                .as_ref()
                .ok_or_else(|| Error::new("this seal has no recovery shares (the dev seal)"))?;
            (
                recovery.salt.clone(),
                recovery.share_hashes.clone(),
                usize::from(recovery.threshold),
            )
        } else {
            let (salt, hashes) = methods::shamir::hashes(&file.method)?;
            let threshold = file
                .method
                .get("threshold")
                .and_then(Value::as_u64)
                .and_then(|threshold| usize::try_from(threshold).ok())
                .ok_or_else(|| Error::new("seal.json's shamir method has no threshold"))?;
            (salt, hashes, threshold)
        };
        if inner.entering != Some(kind) {
            // shares of the other kind entered before are dropped: the two do not combine
            inner.entered.clear();
            inner.entering = Some(kind);
        }
        let what = if kind == "recovery" { "recovery share" } else { "share" };
        let Some(checked) = recovery::check(&share, &salt, &hashes)? else {
            if inner.entered.is_empty() {
                inner.entering = None;
            }
            return Err(self.wrong_entry(inner, conf, what));
        };
        for entered in &inner.entered {
            if recovery::check(entered, &salt, &hashes)?.is_some_and(|other| other.index == checked.index) {
                return Err(Error::new(format!("{what} {} is already entered", checked.index)));
            }
        }
        inner.entered.push(share);
        if inner.entered.len() < threshold {
            return Ok(None);
        }
        let mut shares = Vec::with_capacity(inner.entered.len());
        for entered in &inner.entered {
            if let Some(share) = recovery::check(entered, &salt, &hashes)? {
                shares.push(share);
            }
        }
        inner.entered.clear();
        inner.entering = None;
        let root = if kind == "recovery" {
            let recovery = file
                .recovery
                .as_ref()
                .ok_or_else(|| Error::new("this seal has no recovery shares"))?;
            recovery::open_root(&recovery::combine(&shares)?, recovery)?
        } else {
            methods::shamir::unwrap(&file.method, &shares)?
        };
        check_root(&file, &root)?;
        Ok(Some(root))
    }

    /// Count a wrong passphrase or share: the wait before the next attempt doubles (§3.3), and an alert is logged.
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
            let mut inner = self.inner();
            inner.root = None;
            inner.entered.clear();
            inner.entering = None;
            inner.operator_sealed = true;
            inner.ephemeral = false;
            if inner.held_shares.take().is_some() {
                tracing::error!(
                    "Sealed with the recovery shares of this server's first start still unread: they are dropped with \
                     every other key, and no copy is left; make new ones with rekey or rotate-root once unsealed"
                );
            }
            self.unsealed.store(false, Ordering::SeqCst);
            if inner.file.is_some() {
                let marker = self.marker();
                if let Err(err) = std::fs::write(&marker, "sealed by moqui operator seal\n") {
                    tracing::error!(
                        "Could not write {} ({err}): a restart with an automatic method unseals again",
                        marker.display()
                    );
                }
            }
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
        self.inner()
            .held_shares
            .take()
            .ok_or_else(|| Error::new("no recovery shares are held: they were read already, or never held"))
            .inspect(|_| tracing::info!("The recovery shares held since this server's first start were read"))
    }
}

/// The marker beside the seal file at `path` that an operator sealed it.
fn marker_of(path: &Path) -> PathBuf {
    path.with_file_name("sealed-by-operator")
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
