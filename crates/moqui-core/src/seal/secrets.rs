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

//! The server's secrets out of `.env` (plans/Sealed-Secrets.md §3.1, S4). A secret has a name: a conf selector
//! (`webapp-list/webapp[webroot]/session-config@cookie-key`) or one of its own (`edge-member-key`, `tls-key/<name>`).
//! The secrets needed before the database opens, the **bootstrap secrets** ([`is_bootstrap`]: the crypt passes, the
//! datasource passwords, the embedded Postgres password, which is the transactional datasource's, and the edge member
//! key), are kept in `seal.json` under a key derived from the root key (`super::bootstrap`); every other one is a
//! `moqui.security.SecretValue` row, sealed with AES-256-GCM under the SecretValue data key with "SecretValue", the
//! name and the generation as associated data. A secret the conf schema marks generated, left at `CHANGEME`, is made
//! on first use into its place, never into `.env`.
//!
//! The rule of a conf secret: a value the conf files, the environment or `runtime/.env` give wins (an operator's, read
//! as before and named in a warning when it is `.env`'s); the store answers when the conf leaves it empty or at
//! `CHANGEME`. Code reads a secret by its handle ([`get`], `ExecutionContextFactory::secret`), a zeroizing value. The
//! conf readers that still take a `String` (`conf_env::setting`, `ExecutionContextFactory::conf_value`) reach the store
//! through a transitional shim in `conf_value` until each moves to the handle.
//!
//! A boot with no seal (an unattended boot of a production instance before `moqui operator init`: a tool, a test, the
//! desktop app) holds its root key in memory only and keeps nothing under it: a put is refused, naming `moqui operator
//! init`, and a secret it makes (the crypt pass, a session key) lives in this process's memory only, logged as an error
//! once, never in `.env`. A production server never boots so: its gate makes the seal first.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use moqui_lib::node::path::{self, AttrPath};
use moqui_lib::node::MNode;
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::{lock, read, write};
use moqui_vault::keys::Key32;
use moqui_vault::rng::OsRng;
use zeroize::Zeroizing;

use super::{decode, encode, Keeper};
use crate::context::conf_schema;
use crate::context::{Error, ExecutionContextFactory};
use crate::entity::EntityFacade;
use crate::transaction::TransactionAttribute;

/// The entity of the stored secrets.
pub const ENTITY: &str = "moqui.security.SecretValue";
/// The field key's pass, a bootstrap secret until S1's keyring.
pub const CRYPT_PASS: &str = "entity-facade@crypt-pass";
/// The decrypt-only alternate pass.
pub const CRYPT_PASS_OLD: &str = "entity-facade@crypt-pass-old";
/// The transactional datasource's password, and on the embedded cluster its superuser's.
pub const DATASOURCE_PASSWORD: &str = "entity-facade/datasource[transactional]@password";
/// A cluster member's own edge link key (plans/Edge-Node2.md §4.11), which each member's `seal.json` holds.
pub const EDGE_MEMBER_KEY: &str = "edge-member-key";
/// The superuser password of a dedicated embedded search cluster (`runtime/db/search`).
pub const SEARCH_CLUSTER_PASSWORD: &str = "search-cluster-password";
/// The prefix of a TLS private key an operator puts for `runtime/cert/<name>.crt`: `tls-key/<name>`.
pub const TLS_KEY_PREFIX: &str = "tls-key/";
/// The cluster's TLS certificate and key, made together.
pub const CLUSTER_CERT: &str = "cluster-member@tls-cert";
pub const CLUSTER_KEY: &str = "cluster-member@tls-key";

/// The opened SecretValue rows of one keeper, cleared when it seals.
#[derive(Default)]
pub(crate) struct Cache {
    values: HashMap<String, Zeroizing<String>>,
    loaded: bool,
}

impl Cache {
    pub(crate) fn clear(&mut self) {
        self.values.clear();
        self.loaded = false;
    }
}

/// The generated secrets of the boots with no seal, in this process's memory only, by name. A process: two runtimes
/// in one process (a test's gate and its member) share them, as a cluster's members share their store.
static EPHEMERAL: LazyLock<Mutex<HashMap<String, Zeroizing<String>>>> = LazyLock::new(Mutex::default);

/// Every secret name this process has seen in a store, lower case, for the scripts' `System.getProperty`, which
/// answers none of them ([`is_secret_name`]).
static NAMES: LazyLock<RwLock<HashSet<String>>> = LazyLock::new(RwLock::default);

fn note_name(name: &str) {
    let lower = name.trim().to_ascii_lowercase();
    if !read(&NAMES).contains(&lower) {
        write(&NAMES).insert(lower);
    }
}

/// Whether `name` names a stored secret: a bootstrap secret, a TLS key, or a name a store of this process holds. A
/// script's `System.getProperty` answers none of them (SEC-SCR-03).
#[must_use]
pub fn is_secret_name(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    lower.starts_with(TLS_KEY_PREFIX) || is_bootstrap(&lower) || read(&NAMES).contains(&lower)
}

/// A secret's name as it is kept: a selector in its canonical spelling, else the name trimmed.
#[must_use]
pub fn canonical(name: &str) -> String {
    let name = name.trim();
    if name.contains('@') {
        if let Ok(parsed) = AttrPath::parse(name) {
            return parsed.selector();
        }
    }
    name.to_string()
}

/// Whether `name` is a bootstrap secret, kept in `seal.json` (§6 "What the root key wraps before S1"): the crypt
/// passes, a datasource's password (the embedded cluster's superuser's is the transactional one's) and the edge member
/// key.
#[must_use]
pub fn is_bootstrap(name: &str) -> bool {
    let name = canonical(name);
    if name.eq_ignore_ascii_case(EDGE_MEMBER_KEY) || name.eq_ignore_ascii_case(SEARCH_CLUSTER_PASSWORD) {
        return true;
    }
    let Ok(parsed) = AttrPath::parse(&name) else {
        return false;
    };
    let steps: Vec<&str> = parsed.steps.iter().map(|step| step.element.as_str()).collect();
    match steps.as_slice() {
        ["entity-facade"] => matches!(parsed.attribute.as_str(), "crypt-pass" | "crypt-pass-old"),
        ["entity-facade", "datasource"] => parsed.attribute == "password",
        _ => false,
    }
}

/// How a generated secret is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Made {
    /// 32 random bytes, base64.
    Random,
    /// The cluster's TLS certificate and key, together.
    ClusterPair,
}

/// How the secret `name` is made when the conf leaves it at `CHANGEME`: the conf schema marks it generated, or it is
/// half of the cluster pair. Never a bootstrap secret, which the boot makes.
fn made(name: &str) -> Option<Made> {
    if name == CLUSTER_CERT || name == CLUSTER_KEY {
        return Some(Made::ClusterPair);
    }
    if is_bootstrap(name) {
        return None;
    }
    let parsed = AttrPath::parse(name).ok()?;
    let steps: Vec<&str> = parsed.steps.iter().map(|step| step.element.as_str()).collect();
    conf_schema::unit(&steps)?
        .secrets
        .iter()
        .any(|(attribute, mark)| *attribute == parsed.attribute && *mark == conf_schema::Secret::Generated)
        .then_some(Made::Random)
}

/// Whether a conf value leaves its secret to the store: empty, or the `CHANGEME` placeholder.
#[must_use]
pub fn left_to_store(value: &str) -> bool {
    let value = value.trim();
    value.is_empty() || value.ends_with("CHANGEME")
}

/// What `moqui operator init` is needed for, in an error.
fn no_seal(what: &str) -> Error {
    Error::new(format!(
        "{what}: this boot has no seal (a production instance before `moqui operator init`) and holds its root key in \
         memory only, so it keeps no secret; start the server and run `moqui operator init`, or give the value in the \
         environment (plans/Sealed-Secrets.md S4)"
    ))
}

fn ad(name: &str, generation: u64) -> [Vec<u8>; 3] {
    [
        b"SecretValue".to_vec(),
        name.as_bytes().to_vec(),
        generation.to_be_bytes().to_vec(),
    ]
}

/// `value` sealed under `key` as the row `name` of `generation` keeps it, base64.
fn seal_value(key: &Key32, name: &str, generation: u64, value: &[u8]) -> String {
    let ad = ad(name, generation);
    let parts: Vec<&[u8]> = ad.iter().map(Vec::as_slice).collect();
    encode(&moqui_vault::aead::seal(key, &parts, value, &mut OsRng))
}

/// The value a row keeps.
fn open_value(key: &Key32, name: &str, generation: u64, sealed: &str) -> Result<Zeroizing<String>, Error> {
    let ad = ad(name, generation);
    let parts: Vec<&[u8]> = ad.iter().map(Vec::as_slice).collect();
    let opened = moqui_vault::aead::open(key, &parts, &decode(sealed)?).map_err(|_| {
        Error::new(format!(
            "the SecretValue {name} does not open (moved, changed, or of another root key)"
        ))
    })?;
    text(opened)
}

fn text(bytes: Zeroizing<Vec<u8>>) -> Result<Zeroizing<String>, Error> {
    match std::str::from_utf8(&bytes) {
        Ok(text) => Ok(Zeroizing::new(text.to_string())),
        Err(_) => Err(Error::new("a stored secret is not text")),
    }
}

fn field(row: &crate::entity::EntityValue, name: &str) -> String {
    row.get(name).map(to_plain_string).unwrap_or_default()
}

fn generation_of(row: &crate::entity::EntityValue) -> u64 {
    field(row, "generation").trim().parse().unwrap_or(0)
}

/// Run `body` with authorization off: the store is the server's own, whoever's request reads a secret.
fn unchecked<T>(body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    let _authz = (moqui_entity::hooks::get().disable_authz)();
    body()
}

/// Every row of the store, opened, by name. A row that does not open is logged and left out (failure tolerance): the
/// others still serve.
fn load_all(keeper: &Keeper, facade: &EntityFacade) -> Result<HashMap<String, Zeroizing<String>>, Error> {
    let key = keeper.secret_value_key()?;
    let rows = unchecked(|| Ok(facade.find(ENTITY)?.list()?))?;
    let mut out = HashMap::new();
    for row in rows.iter() {
        let name = field(row, "secretName");
        match open_value(&key, &name, generation_of(row), &field(row, "sealedValue")) {
            Ok(value) => {
                note_name(&name);
                out.insert(name, value);
            }
            Err(err) => tracing::error!("{err}; it is left out"),
        }
    }
    Ok(out)
}

/// The row `name` from the database itself, opened.
fn read_row(keeper: &Keeper, facade: &EntityFacade, name: &str) -> Result<Option<Zeroizing<String>>, Error> {
    let row = unchecked(|| Ok(facade.find(ENTITY)?.eq("secretName", Value::string(name)).one()?))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let key = keeper.secret_value_key()?;
    open_value(&key, name, generation_of(&row), &field(&row, "sealedValue")).map(Some)
}

/// Write `value` as the row `name`, its generation one more than the row's, in a transaction of its own. With
/// `only_new`, a row already there is kept and false answered (another member made it first).
fn write_row(keeper: &Keeper, facade: &EntityFacade, name: &str, value: &str, only_new: bool) -> Result<bool, Error> {
    let key = keeper.secret_value_key()?;
    let written = facade
        .transactions_for(ENTITY)?
        .run(TransactionAttribute::ForceNew, None, || {
            let _authz = (moqui_entity::hooks::get().disable_authz)();
            let held = facade
                .find(ENTITY)?
                .eq("secretName", Value::string(name))
                .for_update(true)
                .one()?;
            if held.is_some() && only_new {
                return Ok(false);
            }
            let generation = held.as_ref().map_or(1, |row| generation_of(row) + 1);
            let mut row = facade.make_value(ENTITY)?;
            row.set("secretName", Value::string(name));
            row.set("generation", Value::I64(i64::try_from(generation).unwrap_or(i64::MAX)));
            row.set(
                "sealedValue",
                Value::string(seal_value(&key, name, generation, value.as_bytes())),
            );
            if held.is_some() {
                row.update(facade)?;
            } else {
                row.create(facade)?;
            }
            Ok(true)
        })?;
    note_name(name);
    Ok(written)
}

/// The secret `name` of a store: a bootstrap secret from `seal.json`, else a SecretValue row (all rows are read once
/// and kept in memory until the keeper seals). `None` when there is none.
///
/// # Errors
///
/// The server is sealed, or the store cannot be read.
pub fn read_from(keeper: &Keeper, facade: &EntityFacade, name: &str) -> Result<Option<Zeroizing<String>>, Error> {
    let name = canonical(name);
    if is_bootstrap(&name) {
        return keeper.bootstrap_secret(&name)?.map(text).transpose();
    }
    if keeper.is_ephemeral() {
        return Ok(lock(&EPHEMERAL).get(&name).cloned());
    }
    if !keeper.is_unsealed() {
        return Err(Error::new("sealed: the stored secrets open only once unsealed"));
    }
    let mut cache = lock(&keeper.secret_values);
    if !cache.loaded {
        cache.values = load_all(keeper, facade)?;
        cache.loaded = true;
    }
    Ok(cache.values.get(&name).cloned())
}

/// Keep `value` as the secret `name`: a bootstrap secret in `seal.json`, else a SecretValue row.
///
/// # Errors
///
/// The server is sealed or has no seal, or the store cannot be written.
pub fn write_to(keeper: &Keeper, facade: Option<&EntityFacade>, name: &str, value: &str) -> Result<(), Error> {
    let name = canonical(name);
    if name.is_empty() {
        return Err(Error::new("a secret has a name"));
    }
    if keeper.is_ephemeral() {
        return Err(no_seal(&format!("the secret {name} is not kept")));
    }
    if is_bootstrap(&name) {
        keeper.set_bootstrap_secret(&name, value.as_bytes())?;
        note_name(&name);
        return Ok(());
    }
    let facade = facade.ok_or_else(|| Error::new(format!("the secret {name} is kept once the database is open")))?;
    write_row(keeper, facade, &name, value, false)?;
    let mut cache = lock(&keeper.secret_values);
    if cache.loaded {
        cache.values.insert(name, Zeroizing::new(value.to_string()));
    }
    Ok(())
}

/// Remove the secret `name`: false when there was none.
///
/// # Errors
///
/// The server is sealed, or the store cannot be written.
pub fn remove_from(keeper: &Keeper, facade: Option<&EntityFacade>, name: &str) -> Result<bool, Error> {
    let name = canonical(name);
    if keeper.is_ephemeral() {
        return Ok(lock(&EPHEMERAL).remove(&name).is_some());
    }
    if is_bootstrap(&name) {
        return keeper.remove_bootstrap_secret(&name);
    }
    let facade = facade.ok_or_else(|| Error::new(format!("the secret {name} is removed once the database is open")))?;
    let removed = facade
        .transactions_for(ENTITY)?
        .run(TransactionAttribute::ForceNew, None, || {
            let _authz = (moqui_entity::hooks::get().disable_authz)();
            let Some(row) = facade.find(ENTITY)?.eq("secretName", Value::string(&name)).one()? else {
                return Ok(false);
            };
            row.delete(facade)?;
            Ok(true)
        })?;
    lock(&keeper.secret_values).values.remove(&name);
    Ok(removed)
}

/// A stored secret as `moqui operator secret list` shows it: never its value.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Listed {
    pub name: String,
    /// `bootstrap` (in `seal.json`) or `value` (a SecretValue row).
    pub kind: &'static str,
    pub generation: u64,
}

/// The stored secrets' names, kinds and generations, by name.
///
/// # Errors
///
/// The server is sealed, or the store cannot be read.
pub fn list_from(keeper: &Keeper, facade: Option<&EntityFacade>) -> Result<Vec<Listed>, Error> {
    let mut out: BTreeMap<String, Listed> = BTreeMap::new();
    for (name, generation) in keeper.bootstrap_entries()? {
        note_name(&name);
        out.insert(
            name.clone(),
            Listed {
                name,
                kind: "bootstrap",
                generation,
            },
        );
    }
    if let Some(facade) = facade.filter(|_| !keeper.is_ephemeral()) {
        let rows = unchecked(|| Ok(facade.find(ENTITY)?.list()?))?;
        for row in rows.iter() {
            let name = field(row, "secretName");
            note_name(&name);
            out.insert(
                name.clone(),
                Listed {
                    name,
                    kind: "value",
                    generation: generation_of(row),
                },
            );
        }
    }
    Ok(out.into_values().collect())
}

/// The secret `name`, made now when the store has none and `conf_value` (the conf's own) leaves it at `CHANGEME`
/// ([`made`]). Under the keeper's cache lock, and checked against the database itself, so two members starting at once
/// keep one value: the second's write finds the first's row and takes it.
fn generate(keeper: &Keeper, facade: &EntityFacade, name: &str, how: Made) -> Result<Option<Zeroizing<String>>, Error> {
    let mut cache = lock(&keeper.secret_values);
    if let Some(found) = read_row(keeper, facade, name)? {
        cache.values.insert(name.to_string(), found.clone());
        return Ok(Some(found));
    }
    let made = make(name, how)?;
    for (made_name, value) in &made {
        if write_row(keeper, facade, made_name, value, true)? {
            tracing::info!("Generated secret {made_name}, kept in {ENTITY} (plans/Sealed-Secrets.md S4)");
        }
        // what is there now: this one, or another member's made first
        if let Some(kept) = read_row(keeper, facade, made_name)? {
            cache.values.insert(made_name.clone(), kept);
        }
    }
    Ok(cache.values.get(name).cloned())
}

/// The values the generated secret `name` is made of, by name: itself, or both halves of the cluster pair.
fn make(name: &str, how: Made) -> Result<Vec<(String, Zeroizing<String>)>, Error> {
    match how {
        Made::Random => Ok(vec![(name.to_string(), random_secret()?)]),
        Made::ClusterPair => cluster_pair(),
    }
}

#[cfg(feature = "cluster-tls")]
fn cluster_pair() -> Result<Vec<(String, Zeroizing<String>)>, Error> {
    let (cert, key) = crate::cluster::generate_cert_pair()?;
    Ok(vec![
        (CLUSTER_CERT.to_string(), Zeroizing::new(cert)),
        (CLUSTER_KEY.to_string(), Zeroizing::new(key)),
    ])
}

// without cluster-tls (the browser build) the pair stays unset; the cluster's start fails on it, nothing else reads it
#[cfg(not(feature = "cluster-tls"))]
fn cluster_pair() -> Result<Vec<(String, Zeroizing<String>)>, Error> {
    Ok(Vec::new())
}

/// 32 random bytes in base64: a generated secret.
///
/// # Errors
///
/// The system has no random bytes.
pub fn random_secret() -> Result<Zeroizing<String>, Error> {
    let mut bytes = Zeroizing::new([0_u8; 32]);
    getrandom::fill(bytes.as_mut()).map_err(|err| Error::new(format!("no random bytes for a secret: {err}")))?;
    Ok(Zeroizing::new(encode(bytes.as_ref())))
}

/// A generated secret of a boot with no seal: in this process's memory only, made once and logged.
fn ephemeral(name: &str, how: Made) -> Result<Option<Zeroizing<String>>, Error> {
    let mut held = lock(&EPHEMERAL);
    if let Some(found) = held.get(name) {
        return Ok(Some(found.clone()));
    }
    for (made_name, value) in make(name, how)? {
        tracing::error!(
            "Generated secret {made_name} in this process's memory only: this boot has no seal to keep it (a production \
             instance before `moqui operator init`)"
        );
        held.insert(made_name, value);
    }
    Ok(held.get(name).cloned())
}

/// The keeper of `factory`'s runtime, when it has one (a test's factory has none).
fn keeper_of(factory: &ExecutionContextFactory) -> Option<Arc<Keeper>> {
    factory.seal_keeper()
}

/// The handle of the secret `name` (plans/Sealed-Secrets.md §3.1, Secret handles): a bootstrap secret from `seal.json`,
/// else a SecretValue row, made now when the conf schema marks it generated and the conf leaves it at `CHANGEME`. `None`
/// when the store has none, `factory` has no seal (a test's factory), or the database is not open yet. This reads the
/// store only; [`ExecutionContextFactory::secret`] applies the conf rule first.
///
/// # Errors
///
/// The server is sealed, or the store cannot be read or written.
pub fn get(factory: &ExecutionContextFactory, name: &str) -> Result<Option<Zeroizing<String>>, Error> {
    let Some(keeper) = keeper_of(factory) else {
        return Ok(None);
    };
    let name = canonical(name);
    let generated = || {
        made(&name).filter(|_| {
            factory
                .conf_value_files(&name)
                .is_some_and(|found| found.value.trim().ends_with("CHANGEME"))
        })
    };
    if keeper.is_ephemeral() {
        return match generated() {
            Some(how) => ephemeral(&name, how),
            None => Ok(lock(&EPHEMERAL).get(&name).cloned()),
        };
    }
    let facade = factory.entity();
    if is_bootstrap(&name) {
        return keeper.bootstrap_secret(&name)?.map(text).transpose();
    }
    let Some(facade) = facade else {
        return Ok(None);
    };
    if let Some(found) = read_from(&keeper, &facade, &name)? {
        return Ok(Some(found));
    }
    match generated() {
        Some(how) => generate(&keeper, &facade, &name, how),
        None => Ok(None),
    }
}

/// Keep `value` as the secret `name` of `factory`'s runtime ([`write_to`]).
///
/// # Errors
///
/// `factory` has no seal, the server is sealed, or the store cannot be written.
pub fn put(factory: &ExecutionContextFactory, name: &str, value: &str) -> Result<(), Error> {
    let keeper = keeper_of(factory).ok_or_else(|| Error::new("this factory has no seal to keep a secret under"))?;
    if keeper.is_ephemeral() {
        // what the server's code keeps at run time (a pairing's key) lives as long as this process; only an
        // operator's put is refused ([`write_to`])
        let name = canonical(name);
        tracing::error!(
            "The secret {name} is held in this process's memory only: this boot has no seal to keep it (a production \
             instance before `moqui operator init`)"
        );
        lock(&EPHEMERAL).insert(name, Zeroizing::new(value.to_string()));
        return Ok(());
    }
    write_to(&keeper, factory.entity().as_ref(), name, value)
}

/// Remove the secret `name` of `factory`'s runtime ([`remove_from`]).
///
/// # Errors
///
/// `factory` has no seal, the server is sealed, or the store cannot be written.
pub fn remove(factory: &ExecutionContextFactory, name: &str) -> Result<bool, Error> {
    let keeper = keeper_of(factory).ok_or_else(|| Error::new("this factory has no seal"))?;
    remove_from(&keeper, factory.entity().as_ref(), name)
}

/// The bootstrap secret `name`, made by `make` and kept in `seal.json` when there is none (the crypt pass at the first
/// start, the embedded cluster's password at its first start, a member's edge key). A boot with no seal holds it in this
/// process's memory only (logged as an error).
///
/// # Errors
///
/// The server is sealed, or `seal.json` cannot be written.
pub fn bootstrap_or_make(
    keeper: &Keeper,
    name: &str,
    make: impl FnOnce() -> Result<Zeroizing<String>, Error>,
) -> Result<Zeroizing<String>, Error> {
    let name = canonical(name);
    if keeper.is_ephemeral() {
        let mut held = lock(&EPHEMERAL);
        if let Some(found) = held.get(&name) {
            return Ok(found.clone());
        }
        let made = make()?;
        tracing::error!(
            "Made the bootstrap secret {name} in this process's memory only: this boot has no seal to keep it (a \
             production instance before `moqui operator init`), so what is written under it is lost with the process"
        );
        held.insert(name, made.clone());
        return Ok(made);
    }
    let made = keeper.bootstrap_or_init(&name, || make().map(|value| value.as_bytes().to_vec()))?;
    note_name(&name);
    text(made)
}

/// Any secret's handle by name from code with no factory at hand (the host's TLS key lookup): the current execution
/// context's factory, else the server factory this process booted last ([`get`]). `None` with neither, or on an error
/// (logged).
#[must_use]
pub fn named(name: &str) -> Option<Zeroizing<String>> {
    let factory = crate::context::ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
        .or_else(ExecutionContextFactory::running)?;
    get(&factory, name).unwrap_or_else(|err| {
        tracing::debug!("The secret {name} could not be read: {err}");
        None
    })
}

/// [`put`] by the current execution context's factory, else the server factory this process booted last.
///
/// # Errors
///
/// No factory, or as [`put`].
pub fn put_named(name: &str, value: &str) -> Result<(), Error> {
    let factory = crate::context::ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
        .or_else(ExecutionContextFactory::running)
        .ok_or_else(|| Error::new(format!("no running server to keep the secret {name}")))?;
    put(&factory, name, value)
}

/// The boot's part (in place of writing `.env`): a conf secret that `runtime/.env` sets is still read and named in a
/// warning; the cluster pair is set together or not at all; and the crypt pass, left at `CHANGEME` by the conf and the
/// environment, is made into `seal.json` at the first start. Nothing is written to `.env` or set in `conf`.
///
/// # Errors
///
/// Half a cluster pair, or a crypt pass that cannot be kept (a boot with no seal: naming `moqui operator init`).
pub fn prepare_boot(
    conf: &MNode,
    overrides: &[crate::context::conf_env::Override],
    keeper: Option<&Keeper>,
) -> Result<(), Error> {
    use crate::context::conf_env::{conf_attribute, Source};
    for item in overrides {
        if item.secret && item.applied() && item.source == Source::DotEnv {
            tracing::warn!(
                "Conf secret {} is read from runtime/.env ({}): secrets are kept in seal.json and {ENTITY} now; put it \
                 with `moqui operator secret put {}` and remove it from runtime/.env",
                item.selector.as_deref().unwrap_or_default(),
                item.name,
                item.selector.as_deref().unwrap_or_default()
            );
        }
    }
    let (cert, key) = (conf_attribute(conf, CLUSTER_CERT), conf_attribute(conf, CLUSTER_KEY));
    if left_to_store(&cert) != left_to_store(&key) {
        return Err(Error::new(format!(
            "{CLUSTER_CERT} and {CLUSTER_KEY} must be set together"
        )));
    }
    if conf_attribute(conf, CRYPT_PASS).ends_with("CHANGEME") {
        if let Some(keeper) = keeper {
            bootstrap_or_make(keeper, CRYPT_PASS, random_secret)?;
        }
    }
    Ok(())
}

/// The running conf of `factory` with its bootstrap secrets in place, for the readers that open the database with the
/// conf itself (the entity facade's crypt pass, the datasources' passwords): each the conf leaves empty or at
/// `CHANGEME` is set from `seal.json`. Dropped after use; the factory's own conf keeps none.
///
/// # Errors
///
/// The conf does not parse, or a bootstrap secret does not open.
pub fn conf_with_secrets(factory: &ExecutionContextFactory) -> Result<MNode, Error> {
    let conf = MNode::parse_shared("moqui-conf", &factory.conf_text())?;
    let Some(keeper) = keeper_of(factory).filter(|keeper| keeper.is_unsealed()) else {
        return Ok(conf);
    };
    let entries: Vec<(String, Option<Zeroizing<String>>)> = if keeper.is_ephemeral() {
        lock(&EPHEMERAL)
            .iter()
            .filter(|(name, _)| is_bootstrap(name))
            .map(|(name, value)| (name.clone(), Some(value.clone())))
            .collect()
    } else {
        keeper
            .bootstrap_entries()?
            .into_iter()
            .map(|(name, _)| (name, None))
            .collect()
    };
    for (name, in_memory) in entries {
        let Ok(parsed) = AttrPath::parse(&name) else {
            continue;
        };
        let Ok(target) = path::resolve(&conf, &parsed, &conf_schema::keying, true) else {
            continue;
        };
        let held = target.node.attribute(&target.attribute).unwrap_or_default();
        if !left_to_store(&held) {
            continue;
        }
        let value = match in_memory {
            Some(value) => Some(value),
            None => keeper.bootstrap_secret(&name)?.map(text).transpose()?,
        };
        if let Some(value) = value {
            target.node.set_attribute(target.attribute.clone(), value.to_string());
        }
    }
    Ok(conf)
}

#[cfg(test)]
#[path = "secrets_tests.rs"]
mod tests;
