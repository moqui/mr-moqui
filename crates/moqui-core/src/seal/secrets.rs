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
//! desktop app) holds its root key in memory only and keeps nothing under it: an operator's put is refused, naming
//! `moqui operator init`; the crypt pass is left unset unless the conf files or the environment give one, so the first
//! encrypt or decrypt of a field fails with that error; and a session or signing key it makes lives in this process's
//! memory only, logged as an error once, never in `.env`. `moqui load` refuses such a boot. A production server never
//! boots so: its gate makes the seal first.
//!
//! The SecretValue rows are read once into memory and read again after [`REFRESH`], so a secret another member put is
//! seen within it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::{Duration, Instant};

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

/// How long the opened SecretValue rows are used before they are read again: a secret another member of the cluster
/// put or removed is seen within it.
pub const REFRESH: Duration = Duration::from_secs(30);

/// The opened SecretValue rows of one keeper, cleared when it seals.
#[derive(Default)]
pub(crate) struct Cache {
    values: HashMap<String, Zeroizing<String>>,
    loaded_at: Option<Instant>,
}

impl Cache {
    pub(crate) fn clear(&mut self) {
        self.values.clear();
        self.loaded_at = None;
    }

    fn fresh(&self) -> bool {
        self.loaded_at.is_some_and(|at| at.elapsed() < REFRESH)
    }
}

/// One generation of a stored secret at a time in this process: the database decides between members.
static GENERATING: Mutex<()> = Mutex::new(());

/// The generated secrets of the boots with no seal, in this process's memory only, by name. A process: two runtimes
/// in one process (a test's gate and its member) share them, as a cluster's members share their store.
static EPHEMERAL: LazyLock<Mutex<HashMap<String, Zeroizing<String>>>> = LazyLock::new(Mutex::default);

/// Drop the secrets a boot with no seal held in memory: `seal` drops every key.
pub(crate) fn clear_ephemeral() {
    lock(&EPHEMERAL).clear();
}

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
///
/// When there are rows and none opens, this member's seal is not the cluster's (a member that made a seal of its own
/// against a shared database): that is refused, with an alert, so it neither makes secrets of its own beside the
/// cluster's nor writes fields under another crypt pass.
fn load_all(key: &Key32, facade: &EntityFacade) -> Result<HashMap<String, Zeroizing<String>>, Error> {
    let rows = unchecked(|| Ok(facade.find(ENTITY)?.list()?))?;
    let mut out = HashMap::new();
    for row in rows.iter() {
        let name = field(row, "secretName");
        match open_value(key, &name, generation_of(row), &field(row, "sealedValue")) {
            Ok(value) => {
                note_name(&name);
                out.insert(name, value);
            }
            Err(err) => tracing::error!("{err}; it is left out"),
        }
    }
    if out.is_empty() && !rows.is_empty() {
        let message = format!(
            "none of the {} {ENTITY} rows opens under this server's seal: its seal is not the one this database's \
             secrets were kept under (a cluster member that made a seal of its own). Start it from the first member's \
             recovery bundle (moqui operator init --from) and rekey (plans/Sealed-Secrets.md S4)",
            rows.len()
        );
        tracing::error!("Alert: {message}");
        return Err(Error::new(message));
    }
    Ok(out)
}

/// The row `name` from the database itself, opened.
fn read_row(key: &Key32, facade: &EntityFacade, name: &str) -> Result<Option<Zeroizing<String>>, Error> {
    let row = unchecked(|| Ok(facade.find(ENTITY)?.eq("secretName", Value::string(name)).one()?))?;
    let Some(row) = row else {
        return Ok(None);
    };
    open_value(key, name, generation_of(&row), &field(&row, "sealedValue")).map(Some)
}

/// Write `value` as the row `name`, its generation one more than the row's, in a transaction of its own.
fn write_row(key: &Key32, facade: &EntityFacade, name: &str, value: &str) -> Result<(), Error> {
    facade
        .transactions_for(ENTITY)?
        .run(TransactionAttribute::ForceNew, None, || {
            let _authz = (moqui_entity::hooks::get().disable_authz)();
            let held = facade
                .find(ENTITY)?
                .eq("secretName", Value::string(name))
                .for_update(true)
                .one()?;
            let generation = held.as_ref().map_or(1, |row| generation_of(row) + 1);
            let mut row = facade.make_value(ENTITY)?;
            row.set("secretName", Value::string(name));
            row.set("generation", Value::I64(i64::try_from(generation).unwrap_or(i64::MAX)));
            row.set(
                "sealedValue",
                Value::string(seal_value(key, name, generation, value.as_bytes())),
            );
            if held.is_some() {
                row.update(facade)?;
            } else {
                row.create(facade)?;
            }
            Ok(())
        })?;
    note_name(name);
    Ok(())
}

/// Create the rows of one generated secret (both halves of the cluster pair together) in one transaction: none of
/// them, and false, when any is there already. A row another member creates at the same time fails the whole
/// transaction (a duplicate key), which the caller reads as a lost race.
fn create_rows(key: &Key32, facade: &EntityFacade, rows: &[(String, Zeroizing<String>)]) -> Result<bool, Error> {
    let created = facade
        .transactions_for(ENTITY)?
        .run(TransactionAttribute::ForceNew, None, || {
            let _authz = (moqui_entity::hooks::get().disable_authz)();
            for (name, _) in rows {
                if facade
                    .find(ENTITY)?
                    .eq("secretName", Value::string(name))
                    .one()?
                    .is_some()
                {
                    return Ok(false);
                }
            }
            for (name, value) in rows {
                let mut row = facade.make_value(ENTITY)?;
                row.set("secretName", Value::string(name));
                row.set("generation", Value::I64(1));
                row.set("sealedValue", Value::string(seal_value(key, name, 1, value.as_bytes())));
                row.create(facade)?;
            }
            Ok(true)
        })?;
    for (name, _) in rows {
        note_name(name);
    }
    Ok(created)
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
    if !lock(&keeper.secret_values).fresh() {
        // the data key first (it takes the keeper's state, which `seal` holds before the cache), and the rows read with
        // no lock held
        let key = keeper.secret_value_key()?;
        let values = load_all(&key, facade)?;
        let mut cache = lock(&keeper.secret_values);
        cache.values = values;
        cache.loaded_at = Some(Instant::now());
    }
    Ok(lock(&keeper.secret_values).values.get(&name).cloned())
}

/// The secret `name` read from its row now (not the opened rows kept in memory); none when there is no row.
///
/// # Errors
///
/// A bootstrap secret's name, sealed, a boot with no seal, or a row that does not open.
pub fn row_value(keeper: &Keeper, facade: &EntityFacade, name: &str) -> Result<Option<Zeroizing<String>>, Error> {
    let name = canonical(name);
    if name.is_empty() || is_bootstrap(&name) {
        return Err(Error::new(format!("{name} is not the name of a SecretValue row")));
    }
    if keeper.is_ephemeral() {
        return Err(no_seal(&format!("the secret {name} is not kept")));
    }
    if !keeper.is_unsealed() {
        return Err(Error::new("sealed: the stored secrets open only once unsealed"));
    }
    read_row(&keeper.secret_value_key()?, facade, &name)
}

/// The secret `name`, read from its row now (not the opened rows kept in memory), or made by `make` and created when
/// there is no row: create-only, so a row another cluster member made meanwhile wins and is read. A row that does not
/// open is an error and is never replaced (a key that cannot be replaced, such as a managed instance's transit key,
/// Vault-Instances M6), and a boot with no seal keeps nothing.
///
/// # Errors
///
/// A bootstrap secret's name, sealed, a boot with no seal, or a row that does not open or cannot be read or written.
pub fn row_or_create(
    keeper: &Keeper,
    facade: &EntityFacade,
    name: &str,
    make: impl FnOnce() -> Zeroizing<String>,
) -> Result<Zeroizing<String>, Error> {
    let name = canonical(name);
    if name.is_empty() || is_bootstrap(&name) {
        return Err(Error::new(format!("{name} is not the name of a SecretValue row")));
    }
    if keeper.is_ephemeral() {
        return Err(no_seal(&format!("the secret {name} is not kept")));
    }
    if !keeper.is_unsealed() {
        return Err(Error::new("sealed: the stored secrets open only once unsealed"));
    }
    let key = keeper.secret_value_key()?;
    if let Some(kept) = read_row(&key, facade, &name)? {
        return Ok(kept);
    }
    let value = make();
    if matches!(create_rows(&key, facade, &[(name.clone(), value.clone())]), Ok(true)) {
        return Ok(value);
    }
    // a row another member made at the same time: its value, which is the one kept
    read_row(&key, facade, &name)?.ok_or_else(|| Error::new(format!("the secret {name} was neither made nor found")))
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
    let key = keeper.secret_value_key()?;
    write_row(&key, facade, &name, value)?;
    let mut cache = lock(&keeper.secret_values);
    if cache.loaded_at.is_some() {
        cache.values.insert(name, Zeroizing::new(value.to_string()));
    }
    Ok(())
}

/// Whether `name` is a pass the database's encrypted fields or its connection depend on: the crypt passes and the
/// datasource passwords, which an operator neither removes nor replaces unknowingly.
fn guarded(name: &str) -> bool {
    is_bootstrap(name) && name != EDGE_MEMBER_KEY && name != SEARCH_CLUSTER_PASSWORD
}

/// `moqui operator secret put`: [`write_to`], refusing to replace the crypt pass unless `force`, and then keeping the
/// pass it replaces as `entity-facade@crypt-pass-old`, so the fields written under it still read.
///
/// # Errors
///
/// As [`write_to`], or the crypt pass with no `force`.
pub fn operator_put(
    keeper: &Keeper,
    facade: Option<&EntityFacade>,
    name: &str,
    value: &str,
    force: bool,
) -> Result<(), Error> {
    let name = canonical(name);
    if name == CRYPT_PASS {
        if let Some(old) = keeper.bootstrap_secret(CRYPT_PASS)?.map(text).transpose()? {
            if old.as_str() != value {
                if !force {
                    return Err(Error::new(
                        "entity-facade@crypt-pass is the key of every encrypted field: a new one needs --force, and \
                         the one it replaces becomes entity-facade@crypt-pass-old (run the rewrap after)",
                    ));
                }
                write_to(keeper, facade, CRYPT_PASS_OLD, &old)?;
            }
        }
    }
    write_to(keeper, facade, &name, value)
}

/// `moqui operator secret rm`: [`remove_from`], refused for a crypt pass or a datasource password, which the encrypted
/// fields and the database connection depend on.
///
/// # Errors
///
/// As [`remove_from`], or a guarded name.
pub fn operator_remove(keeper: &Keeper, facade: Option<&EntityFacade>, name: &str) -> Result<bool, Error> {
    let name = canonical(name);
    if guarded(&name) {
        return Err(Error::new(format!(
            "{name} is not removed: the encrypted fields or the database connection depend on it (put another with \
             `moqui operator secret put`)"
        )));
    }
    remove_from(keeper, facade, &name)
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
    let _one = lock(&GENERATING);
    let key = keeper.secret_value_key()?;
    if let Some(found) = read_row(&key, facade, name)? {
        lock(&keeper.secret_values)
            .values
            .insert(name.to_string(), found.clone());
        return Ok(Some(found));
    }
    let made = make(name, how)?;
    let created = create_rows(&key, facade, &made);
    // what is there now: these, or another member's made first (its transaction won, and this one's left nothing)
    let mut kept = Vec::with_capacity(made.len());
    for (made_name, _) in &made {
        if let Some(value) = read_row(&key, facade, made_name)? {
            kept.push((made_name.clone(), value));
        }
    }
    match created {
        Ok(true) => {
            for (made_name, _) in &made {
                tracing::info!("Generated secret {made_name}, kept in {ENTITY} (plans/Sealed-Secrets.md S4)");
            }
        }
        Ok(false) => {}
        Err(err) if kept.len() < made.len() => return Err(err),
        Err(_) => {}
    }
    let mut cache = lock(&keeper.secret_values);
    for (made_name, value) in kept {
        cache.values.insert(made_name, value);
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

/// 32 random bytes in hex: a password that goes into a URL as it is (the embedded cluster's superuser's, which its
/// tools put in a connection URL unencoded), where base64's `/` and `+` would not.
///
/// # Errors
///
/// The system has no random bytes.
pub fn random_password() -> Result<Zeroizing<String>, Error> {
    let mut bytes = Zeroizing::new([0_u8; 32]);
    getrandom::fill(bytes.as_mut()).map_err(|err| Error::new(format!("no random bytes for a password: {err}")))?;
    Ok(Zeroizing::new(bytes.iter().map(|byte| format!("{byte:02x}")).collect()))
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
        if is_bootstrap(&name) {
            // nothing is made under a memory-only root: the conf files or the environment give it, or it is unset
            return Ok(None);
        }
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
/// start, the embedded cluster's password at its first start, a member's edge key).
///
/// # Errors
///
/// A boot with no seal (naming `moqui operator init`: nothing is made under a memory-only root), the server is sealed,
/// or `seal.json` cannot be written.
pub fn bootstrap_or_make(
    keeper: &Keeper,
    name: &str,
    make: impl FnOnce() -> Result<Zeroizing<String>, Error>,
) -> Result<Zeroizing<String>, Error> {
    let name = canonical(name);
    if keeper.is_ephemeral() {
        return Err(no_seal(&format!("the bootstrap secret {name} is not made")));
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

/// The boot's part (in place of writing `.env`): the cluster pair is set together or not at all, and the crypt pass,
/// left at `CHANGEME` by the conf and the environment, is made into `seal.json` at the first start. A conf secret that
/// `runtime/.env` sets is ignored once this runtime has a seal other than the `dev` seal, as the seal's own settings
/// are: the server's user can write `.env`, so it would choose a key; it is put back to `CHANGEME` in `conf`, so the
/// store answers it, and named in an error. Before the seal (a boot with no seal), and under the `dev` seal (a dev
/// checkout's, a test run's), it is still read, and named in a warning; a boot with no seal makes no crypt pass.
/// Nothing is written to `.env`.
///
/// # Errors
///
/// Half a cluster pair, or a crypt pass that cannot be kept.
pub fn prepare_boot(
    conf: &MNode,
    overrides: &[crate::context::conf_env::Override],
    keeper: Option<&Keeper>,
) -> Result<(), Error> {
    use crate::context::conf_env::{conf_attribute, Source};
    let sealed = keeper.is_some_and(Keeper::refuses_dotenv_secrets);
    for item in overrides {
        if !(item.secret && item.applied() && item.source == Source::DotEnv) {
            continue;
        }
        let selector = item.selector.as_deref().unwrap_or_default();
        if sealed {
            tracing::error!(
                "Conf secret {selector} is set in runtime/.env ({}), which the server's own user can write: it is \
                 ignored, and the seal's store answers it; put it with `moqui operator secret put {selector}`, or in \
                 the environment, and remove it from runtime/.env (plans/Sealed-Secrets.md S4)",
                item.name
            );
            if let Ok(parsed) = AttrPath::parse(selector) {
                if let Ok(target) = path::resolve(conf, &parsed, &conf_schema::keying, false) {
                    target
                        .node
                        .set_attribute(target.attribute.clone(), "CHANGEME".to_string());
                }
            }
        } else {
            tracing::warn!(
                "Conf secret {selector} is read from runtime/.env ({}): secrets are kept in seal.json and {ENTITY} \
                 now; once this runtime has its seal, put it with `moqui operator secret put {selector}` and remove it \
                 from runtime/.env",
                item.name
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
        if let Some(keeper) = keeper.filter(|keeper| !keeper.is_ephemeral()) {
            bootstrap_or_make(keeper, CRYPT_PASS, random_secret)?;
        }
    }
    Ok(())
}

/// Whether `factory`'s boot has a memory-only root (a production instance before `moqui operator init`) and no crypt
/// pass the conf files or the environment give: what `moqui load` refuses.
#[must_use]
pub fn unsealed_without_crypt_pass(factory: &ExecutionContextFactory) -> bool {
    keeper_of(factory).is_some_and(|keeper| keeper.is_ephemeral())
        && factory
            .conf_value_files(CRYPT_PASS)
            .is_none_or(|found| left_to_store(&found.value))
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
        // no crypt pass under a memory-only root: the first encrypt or decrypt fails, naming the remedy
        let refused = format!(
            "{}{}",
            crate::entity::crypto::REFUSED_PASS_PREFIX,
            no_seal("entity-facade@crypt-pass is not set").message()
        );
        vec![(CRYPT_PASS.to_string(), Some(Zeroizing::new(refused)))]
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
