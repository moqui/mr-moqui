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

//! The vault in the desktop and mobile apps (plans/Vault-Instances.md §3.5, §3.6, §3.12, V5): the vault crate's
//! `ops::Session` run natively, one per account, in place of the browser's wasm worker. The app carries its own copy
//! of the module and never takes it from the server; the page (Commons › Vault, the lease dialog) calls the same ops
//! through the page bridge (`VaultClient.js` finds `moquiDesktop.vault`), and the keys stay in this process.
//!
//! What the app adds to the module:
//!
//! - **Locking:** an account's session locks after its idle time with no call (the page's `idleMinutes`, 15 by
//!   default), on the page's Lock, on sign-out, revocation and removal, when another account becomes current, and on the
//!   host's word (the desktop's sleep, the phone's activity leaving the screen).
//! - **Rollback memory** (decision (d) of §9): the highest key version, revision and member-list hash accepted for each
//!   vault are kept here and handed to the module in place of the page's, and the highest revision seen of each item
//!   ([`seen`]); a lower one is told.
//! - **Device memory** (§3.5): on the user's word, the open personal key wrapped by a memory key in the host's key store
//!   ([`memory`]) for a time the user chooses, at most [`MAX_MEMORY_MINUTES`] and what the server allows. It ends at
//!   that time, on the page's Lock with forget, on sign-out, when the server no longer takes the device's key, and when
//!   the user takes the device off the server's list ([`server`]). Opening from memory first asks the server, so a
//!   revoked device opens nothing even when the app was not running at the revocation.

pub mod memory;
mod seen;
pub mod server;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use moqui_vault::keys::Key32;
use moqui_vault::ops::Session;
use moqui_vault::rng::OsRng;
use moqui_vault::text::hex;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use self::memory::{KeyError, MemoryKeys, Record, KIND_FILE};
use self::server::{MemoryServer, ServerError};
use crate::app::Client;

/// The idle time of a session when the page has not said one, in minutes (§3.6).
pub const DEFAULT_IDLE_MINUTES: i64 = 15;
/// The longest a device remembers the personal key, in minutes; the server may allow less.
pub const MAX_MEMORY_MINUTES: u32 = 24 * 60;

/// The ops whose request names a vault's member-list chain and takes the module's `remembered`.
const CHAIN_OPS: [&str; 6] = [
    "chain_verify",
    "key_version_next",
    "vault_key_open",
    "vault_rotate",
    "recovery_open_vault",
    "recovery_rekey",
];

/// An account's open session.
struct Open {
    session: Session,
    /// The last call, epoch millis.
    used: i64,
    /// When the device memory it was opened from or made ends, epoch millis: the session locks then too.
    memory_until: Option<i64>,
}

/// What an open from device memory came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restore {
    /// The personal key is open: the module's answer, as `personal_open` gives it, and when the memory ends.
    Opened(Value),
    /// The device remembers no key for the account.
    Nothing,
    /// The memory ended, and is forgotten here, for the reason given.
    Ended(String),
    /// The key store opens the memory key only after the device's lock screen or biometrics.
    NeedsUnlock,
    /// The server could not confirm the memory: it is kept, and not opened.
    Unconfirmed(String),
}

impl Restore {
    /// The answer to the page.
    pub fn to_json(&self) -> Value {
        match self {
            Self::Opened(answer) => json!({ "opened": true, "result": answer }),
            Self::Nothing => json!({ "opened": false, "reason": "none" }),
            Self::Ended(why) => json!({ "opened": false, "reason": "ended", "message": why }),
            Self::NeedsUnlock => json!({ "opened": false, "reason": "unlock" }),
            Self::Unconfirmed(why) => json!({ "opened": false, "reason": "unconfirmed", "message": why }),
        }
    }
}

/// The vault sessions of the client's accounts, and what the app keeps of the vaults in `vault/` of its directory.
pub struct Vaults {
    dir: PathBuf,
    open: Mutex<HashMap<String, Open>>,
    /// Each account's idle time, in millis, as its page said.
    idle: Mutex<HashMap<String, i64>>,
    keys: OnceLock<Box<dyn MemoryKeys>>,
    /// The sweep's thread was started ([`start`]): once for the client, however often a host starts it.
    sweeping: AtomicBool,
}

/// The time now, epoch millis.
pub fn now_ms() -> i64 {
    crate::sync::outbox::now_ms()
}

impl Vaults {
    /// The vaults of the client whose directory is `client_dir`.
    pub fn new(client_dir: &Path) -> Self {
        Self {
            dir: client_dir.join("vault"),
            open: Mutex::new(HashMap::new()),
            idle: Mutex::new(HashMap::new()),
            keys: OnceLock::new(),
            sweeping: AtomicBool::new(false),
        }
    }

    /// Where the memory keys are kept: set once by the host. With none, device memory is not offered.
    pub fn set_memory_keys(&self, keys: Box<dyn MemoryKeys>) {
        if self.keys.set(keys).is_err() {
            tracing::warn!("The vault's memory key store was set already; the first stays");
        }
    }

    /// Whether the host set the memory key store.
    pub fn has_memory_keys(&self) -> bool {
        self.keys.get().is_some()
    }

    /// How the memory keys are kept, when device memory can be offered here (not in a plain file).
    pub fn memory_kind(&self) -> Option<&'static str> {
        self.keys
            .get()
            .map(|keys| keys.kind())
            .filter(|kind| *kind != KIND_FILE)
    }

    /// One call of `account`'s page: an op of the module (`ops::Session`), or the app's own `configure`
    /// (`{idle_minutes}`) and `revisions_seen` (`{vault_id, items: [{item_id, revision}]}`, answering `{older}`).
    ///
    /// # Errors
    ///
    /// The module's refusal, as its text.
    pub fn call(&self, account: &str, op: &str, args: Value) -> Result<Value, String> {
        self.call_at(account, op, args, now_ms())
    }

    /// [`Self::call`] at the time `now` (epoch millis).
    ///
    /// # Errors
    ///
    /// The module's refusal, as its text.
    pub fn call_at(&self, account: &str, op: &str, mut args: Value, now: i64) -> Result<Value, String> {
        if !memory::plain_name(account) {
            return Err("not an account id".to_string());
        }
        self.sweep_at(now);
        if !args.is_object() {
            args = json!({});
        }
        match op {
            "configure" => {
                let minutes = args["idle_minutes"]
                    .as_i64()
                    .unwrap_or(DEFAULT_IDLE_MINUTES)
                    .clamp(1, 24 * 60);
                moqui_lib::sync::lock(&self.idle).insert(account.to_string(), minutes * 60_000);
                return Ok(json!({ "idle_minutes": minutes }));
            }
            "lock" => {
                self.lock(account);
                return Ok(json!({ "locked": true }));
            }
            "revisions_seen" => return self.revisions_seen(account, &args),
            _ => {}
        }
        let chain_vault = CHAIN_OPS.contains(&op).then(|| vault_of(&args)).flatten();
        let mut seen = seen::load(&self.dir, account);
        if let Some(kept) = chain_vault.as_ref().and_then(|vault| seen.vaults.get(vault)) {
            // the app's memory, never the page's, which the server's code could have set
            args["remembered"] = kept.clone();
        }
        args["op"] = Value::String(op.to_string());
        let request = Zeroizing::new(args.to_string());
        moqui_vault::ops::shapes::scrub(&mut args);
        let answer = {
            let mut open = moqui_lib::sync::lock(&self.open);
            let entry = open.entry(account.to_string()).or_insert_with(|| Open {
                session: Session::default(),
                used: now,
                memory_until: None,
            });
            entry.used = now;
            Zeroizing::new(entry.session.call(&request, &mut OsRng))
        };
        let mut parsed: Value = serde_json::from_str(&answer).map_err(|err| err.to_string())?;
        if parsed["ok"] != Value::Bool(true) {
            return Err(parsed["error"]
                .as_str()
                .unwrap_or("the vault module failed")
                .to_string());
        }
        let result = parsed["result"].take();
        if let Some(vault) = chain_vault {
            let remembered = if op == "chain_verify" {
                &result
            } else {
                &result["remembered"]
            };
            if let Some(order) = seen::order(remembered) {
                let higher = seen
                    .vaults
                    .get(&vault)
                    .and_then(seen::order)
                    .is_none_or(|kept| order >= kept);
                if higher {
                    seen.vaults.insert(vault, remembered.clone());
                    if let Err(err) = seen::save(&self.dir, account, &seen) {
                        tracing::error!("What this app has seen of a vault was not kept: {err}");
                    }
                }
            }
        }
        Ok(result)
    }

    /// The items of a list whose revision is lower than one seen before (`older`), and the highest of each kept.
    fn revisions_seen(&self, account: &str, args: &Value) -> Result<Value, String> {
        let vault = args["vault_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("vault_id is required")?;
        let mut seen = seen::load(&self.dir, account);
        let items = seen.items.entry(vault.to_string()).or_default();
        let mut older = Vec::new();
        for row in args["items"].as_array().map(Vec::as_slice).unwrap_or_default() {
            let (Some(id), Some(revision)) = (row["item_id"].as_str(), row["revision"].as_u64()) else {
                continue;
            };
            let kept = items.entry(id.to_string()).or_insert(revision);
            if *kept > revision {
                older.push(Value::String(id.to_string()));
            } else {
                *kept = revision;
            }
        }
        seen::save(&self.dir, account, &seen)?;
        Ok(json!({ "older": older }))
    }

    /// Lock `account`'s session: its keys are dropped, which zeroes them.
    pub fn lock(&self, account: &str) {
        moqui_lib::sync::lock(&self.open).remove(account);
    }

    /// Lock every account's session (the device locked or slept, the app's pages left the screen).
    pub fn lock_all(&self) {
        moqui_lib::sync::lock(&self.open).clear();
    }

    /// Lock every session but `account`'s (another account became current).
    pub fn lock_others(&self, account: &str) {
        moqui_lib::sync::lock(&self.open).retain(|id, _| id == account);
    }

    /// Whether `account`'s personal key is open.
    pub fn is_open(&self, account: &str) -> bool {
        moqui_lib::sync::lock(&self.open)
            .get(account)
            .is_some_and(|open| open.session.personal_key_id().is_some())
    }

    /// `account` signed out here, or its device key is no longer taken: its session locks and its device memory is
    /// forgotten. The memory's id, for the server's record when it can still be told.
    pub fn signed_out(&self, account: &str) -> Option<String> {
        self.lock(account);
        moqui_lib::sync::lock(&self.idle).remove(account);
        self.forget(account)
    }

    /// `account` removed from the device: as [`Self::signed_out`], and what was seen of its vaults goes too.
    pub fn removed(&self, account: &str) -> Option<String> {
        let forgotten = self.signed_out(account);
        seen::remove(&self.dir, account);
        forgotten
    }

    /// Forget `account`'s device memory here: its file and its key. The memory's id, for the server's record.
    pub fn forget(&self, account: &str) -> Option<String> {
        let record = memory::load(&self.dir, account);
        if let Err(err) = memory::remove(&self.dir, account) {
            tracing::error!("The vault device memory of {account} was not removed: {err}");
        }
        if let Some(keys) = self.keys.get() {
            if let Err(err) = keys.delete(&memory::entry(account)) {
                tracing::error!("The vault memory key of {account} was not removed from the key store: {err}");
            }
        }
        record.map(|record| record.memory_id)
    }

    /// End what is due at `now`: each device memory past its time (its file and key forgotten), each session idle past
    /// its account's idle time, and each session opened under a memory that has ended.
    pub fn sweep_at(&self, now: i64) {
        for account in memory::accounts(&self.dir) {
            if memory::load(&self.dir, &account).is_some_and(|record| record.until <= now) {
                tracing::info!("The vault device memory of {account} reached its time and is forgotten");
                self.forget(&account);
            }
        }
        let idle = moqui_lib::sync::lock(&self.idle).clone();
        moqui_lib::sync::lock(&self.open).retain(|account, open| {
            let limit = idle.get(account).copied().unwrap_or(DEFAULT_IDLE_MINUTES * 60_000);
            open.used.saturating_add(limit) > now && open.memory_until.is_none_or(|until| until > now)
        });
    }

    /// What the page shows of device memory: whether it can be offered here, how the key is kept, when the account's
    /// memory ends, and whether the personal key is open.
    pub fn memory_state_at(&self, account: &str, now: i64) -> Value {
        self.sweep_at(now);
        let until = memory::load(&self.dir, account).map(|record| record.until);
        json!({ "available": self.memory_kind().is_some(), "kind": self.memory_kind(), "until": until,
                "open": self.is_open(account), "maxMinutes": MAX_MEMORY_MINUTES })
    }

    /// Remember `account`'s open personal key on this device for `minutes`, recorded on its server as `device_name`.
    /// Replaces a memory it had. Answers when it ends.
    ///
    /// # Errors
    ///
    /// Refused with no OS key store, with the personal key locked, for minutes out of range, or when the server or the
    /// key store does not take it.
    pub fn remember_at(
        &self,
        account: &str,
        minutes: u32,
        device_name: &str,
        server: &dyn MemoryServer,
        now: i64,
    ) -> Result<Value, String> {
        if !memory::plain_name(account) {
            return Err("not an account id".to_string());
        }
        let keys = self
            .keys
            .get()
            .filter(|keys| keys.kind() != KIND_FILE)
            .ok_or("This device has no key store to remember the key in")?;
        if minutes == 0 || minutes > MAX_MEMORY_MINUTES {
            return Err(format!("Remember the key for 1 to {MAX_MEMORY_MINUTES} minutes"));
        }
        self.sweep_at(now);
        if !self.is_open(account) {
            return Err("Open your vault key first".to_string());
        }
        let mut id_bytes = [0_u8; 16];
        moqui_vault::rng::Rng::fill(&mut OsRng, &mut id_bytes);
        let memory_id = hex(&id_bytes);
        let device_key = Key32::random(&mut OsRng);
        let sealed = {
            let open = moqui_lib::sync::lock(&self.open);
            let entry = open.get(account).ok_or("Open your vault key first")?;
            entry
                .session
                .device_seal(&memory_id, &device_key, &mut OsRng)
                .map_err(|err| err.to_string())?
        };
        let server_until = server
            .register(&memory_id, device_name, minutes)
            .map_err(|err| err.to_string())?;
        let until = server_until.min(now.saturating_add(i64::from(minutes) * 60_000));
        let earlier = self.forget(account);
        let kept = keys
            .put(&memory::entry(account), &device_key)
            .and_then(|()| memory::save(&self.dir, account, &Record::new(&memory_id, &sealed, until)));
        if let Err(err) = kept {
            self.forget(account);
            forget_on(server, &memory_id);
            return Err(format!("The key was not remembered: {err}"));
        }
        if let Some(earlier) = earlier {
            forget_on(server, &earlier);
        }
        if let Some(open) = moqui_lib::sync::lock(&self.open).get_mut(account) {
            open.memory_until = Some(until);
        }
        Ok(json!({ "until": until, "kind": keys.kind() }))
    }

    /// Open `account`'s personal key from device memory, once its server confirms the memory. `signed_in`: whether the
    /// account still has its device key here.
    pub fn restore_at(&self, account: &str, signed_in: bool, server: &dyn MemoryServer, now: i64) -> Restore {
        if !memory::plain_name(account) {
            return Restore::Nothing;
        }
        self.sweep_at(now);
        let Some(record) = memory::load(&self.dir, account) else {
            return Restore::Nothing;
        };
        if !signed_in {
            self.forget(account);
            return Restore::Ended("the account is signed out on this device".to_string());
        }
        match server.check(&record.memory_id) {
            Ok(true) => {}
            Ok(false) => {
                self.forget(account);
                return Restore::Ended(
                    "this device was taken off the list of devices that remember the key".to_string(),
                );
            }
            Err(ServerError::Revoked) => {
                self.forget(account);
                return Restore::Ended("the server no longer takes this device's key".to_string());
            }
            Err(err) => return Restore::Unconfirmed(err.to_string()),
        }
        let Some(keys) = self.keys.get() else {
            return Restore::Unconfirmed("this device has no key store".to_string());
        };
        let device_key = match keys.take(&memory::entry(account)) {
            Ok(Some(key)) => key,
            Ok(None) => {
                self.forget(account);
                return Restore::Ended("the key store no longer holds the memory key".to_string());
            }
            Err(KeyError::NeedsUnlock) => return Restore::NeedsUnlock,
            Err(KeyError::Failed(err)) => return Restore::Unconfirmed(format!("the key store did not answer: {err}")),
        };
        let opened = record.sealed().and_then(|sealed| {
            let mut session = Session::default();
            let answer = session
                .device_open(&sealed, &record.memory_id, &device_key)
                .map_err(|err| err.to_string())?;
            Ok((session, answer))
        });
        match opened {
            Ok((session, mut answer)) => {
                answer["until"] = json!(record.until);
                moqui_lib::sync::lock(&self.open).insert(
                    account.to_string(),
                    Open {
                        session,
                        used: now,
                        memory_until: Some(record.until),
                    },
                );
                Restore::Opened(answer)
            }
            Err(err) => {
                tracing::error!("The vault device memory of {account} does not open, and is forgotten: {err}");
                self.forget(account);
                forget_on(server, &record.memory_id);
                Restore::Ended("the memory does not open".to_string())
            }
        }
    }
}

/// End `memory_id` on the server; a failure is logged (the record ends at its time all the same).
pub fn forget_on(server: &dyn MemoryServer, memory_id: &str) {
    if let Err(err) = server.forget(memory_id) {
        tracing::info!("The server's record of a vault device memory was not ended: {err}");
    }
}

/// The vault a chain op names: its entries' vault id.
fn vault_of(args: &Value) -> Option<String> {
    args["entries"]
        .as_array()
        .and_then(|entries| entries.first())
        .and_then(|entry| entry["vault_id"].as_str())
        .or_else(|| args["vault_id"].as_str())
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// How far the wall clock may run ahead of the monotonic one between two sweeps before it is a sleep.
const SLEEP_GAP: Duration = Duration::from_secs(60);

/// Whether the device slept between two sweeps, by the wall clock moving `wall` and the monotonic one, which stands
/// still while the device sleeps, `monotonic`.
pub(crate) fn slept(wall: Option<Duration>, monotonic: Duration) -> bool {
    wall.and_then(|wall| wall.checked_sub(monotonic))
        .is_some_and(|ahead| ahead > SLEEP_GAP)
}

/// Sweep the client's vaults every half minute, on a thread of its own, until the client is shut down: a memory ends at
/// its time, and an idle session locks, whether or not a page calls. A sleep of the device locks every session (§3.6,
/// the device's lock), as the clocks tell it.
pub fn start(client: &Arc<Client>) {
    if client.vault().sweeping.swap(true, Ordering::SeqCst) {
        return;
    }
    let client = Arc::clone(client);
    let started = std::thread::Builder::new()
        .name("client-vault".to_string())
        .spawn(move || loop {
            let (wall_before, before) = (std::time::SystemTime::now(), std::time::Instant::now());
            if client.wait_stopped(Duration::from_secs(30)) {
                client.vault().lock_all();
                return;
            }
            if slept(
                std::time::SystemTime::now().duration_since(wall_before).ok(),
                before.elapsed(),
            ) {
                tracing::info!("The device slept: the vault sessions are locked");
                client.vault().lock_all();
            }
            client.vault().sweep_at(now_ms());
        });
    if let Err(err) = started {
        tracing::error!("The vault's sweep did not start: {err}");
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
