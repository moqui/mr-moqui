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

//! The vault in the desktop and mobile apps (plans/archive/Vault-Instances.md §3.5, §3.6, §3.12, V5): the vault crate's
//! `ops::Session` run natively in place of the browser's wasm worker, one session per page (an account's webview, by
//! its label), as a browser has one worker per tab. The app carries its own copy of the module and never takes it from
//! the server; the vault screens call the same ops through the page bridge (`VaultClient.js` finds
//! `moquiDesktop.vault`), and the keys stay in this process. Which page may call is the host's check (only a vault
//! screen's document, `moqui_client_tauri::vault`).
//!
//! What the app adds to the module:
//!
//! - **Locking:** a page's session locks after its idle time with no call but `item_otp`, which a list of codes makes on
//!   a timer (the page's `idleMinutes`, 15 by default, at
//!   most [`MAX_IDLE_MINUTES`]), on the page's Lock, when the page leaves the vault screens, on sign-out, revocation and
//!   removal, when another account becomes current, and on the host's word (the device's sleep or screen lock, the
//!   phone's activity leaving the screen).
//! - **Rollback memory** (decision (d) of §9): the highest key version, revision and member-list hash accepted for each
//!   vault are kept here and handed to the module in place of the page's, and the highest revision seen of each item
//!   ([`seen`]): `item_open` of a lower revision is refused, and `revisions_seen` tells a list's. Never lowered.
//! - **Device memory** (§3.5): on the user's word, the open personal key wrapped for a time the user chooses, at most
//!   [`MAX_MEMORY_MINUTES`] and what the server allows, under a key made of two halves: the device's, in the host's key
//!   store ([`memory`]), and the server record's secret, which the server hands out only while the record is live
//!   ([`server`]). So the memory cannot open once the record ends: at its time, taken off the user's list, the personal
//!   key replaced, or the device's key no longer taken. It also ends on the page's Lock with forget and on sign-out, and
//!   while a session opened from it is open the app asks the server every few minutes and locks when the record ends.

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
use moqui_vault::personal::device_wrap_key;
use moqui_vault::rng::OsRng;
use moqui_vault::text::hex;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use self::memory::{KeyError, MemoryKeys, Record, KIND_FILE};
use self::server::{MemoryServer, ServerError};
use crate::app::Client;

/// The idle time of a session when the page has not said one, in minutes (§3.6).
pub const DEFAULT_IDLE_MINUTES: i64 = 15;
/// The longest idle time a page may set, in minutes.
pub const MAX_IDLE_MINUTES: i64 = 60;
/// The longest a device remembers the personal key, in minutes; the server may allow less.
pub const MAX_MEMORY_MINUTES: u32 = 24 * 60;
/// How often a session opened from device memory asks the server whether the memory is still live.
pub const MEMORY_CHECK_EVERY: Duration = Duration::from_secs(5 * 60);

/// The ops whose request names a vault's member-list chain and takes the module's `remembered`.
const CHAIN_OPS: [&str; 6] = [
    "chain_verify",
    "key_version_next",
    "vault_key_open",
    "vault_rotate",
    "recovery_open_vault",
    "recovery_rekey",
];

/// The ops that open an item, whose revision (when the page names it) is checked against the highest seen.
const ITEM_OPS: [&str; 3] = ["item_open", "item_summary", "item_otp"];

/// The ops a page makes on a timer, not on the user's word: they do not count as use, so a list of one-time codes left
/// open still locks at the idle time.
const PASSIVE_OPS: [&str; 1] = ["item_otp"];

/// A page's open session.
struct Open {
    session: Session,
    /// The last call, epoch millis.
    used: i64,
    /// The device memory it was opened from or made, and when that ends (epoch millis): the session locks then too.
    memory: Option<(String, i64)>,
}

/// A session by account and page (the webview's label).
type Slot = (String, String);

/// What an open from device memory came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restore {
    /// The personal key is open: the module's answer, as `personal_open` gives it, and when the memory ends.
    Opened(Value),
    /// The device remembers no key for the account.
    Nothing,
    /// The memory ended, and is forgotten here, for the reason given.
    Ended(String),
    /// The key store opens the memory key only once the user confirms with the device's lock screen or biometrics,
    /// and they did not.
    NeedsUnlock,
    /// The server could not confirm the memory, or the page locked while it opened: it is kept, and not opened.
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

/// The vault sessions of the client's pages, and what the app keeps of the vaults in `vault/` of its directory.
pub struct Vaults {
    dir: PathBuf,
    open: Mutex<HashMap<Slot, Open>>,
    /// Each page's idle time, in millis, as it said.
    idle: Mutex<HashMap<Slot, i64>>,
    /// Each account's lock count: an open from memory that ran across a lock does not leave a session behind.
    locks: Mutex<HashMap<String, u64>>,
    /// One device memory change at a time for an account (remember, restore, forget, the sweep).
    memory_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// One change of what was seen at a time.
    seen_lock: Mutex<()>,
    keys: OnceLock<Box<dyn MemoryKeys>>,
    /// The sweep's thread was started ([`start`]): once for the client, however often a host starts it.
    sweeping: AtomicBool,
}

/// The time now, epoch millis.
pub fn now_ms() -> i64 {
    crate::sync::outbox::now_ms()
}

fn slot(account: &str, page: &str) -> Slot {
    (account.to_string(), page.to_string())
}

impl Vaults {
    /// The vaults of the client whose directory is `client_dir`.
    pub fn new(client_dir: &Path) -> Self {
        Self {
            dir: client_dir.join("vault"),
            open: Mutex::new(HashMap::new()),
            idle: Mutex::new(HashMap::new()),
            locks: Mutex::new(HashMap::new()),
            memory_locks: Mutex::new(HashMap::new()),
            seen_lock: Mutex::new(()),
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

    /// The lock that serializes `account`'s device memory changes.
    fn memory_lock(&self, account: &str) -> Arc<Mutex<()>> {
        Arc::clone(
            moqui_lib::sync::lock(&self.memory_locks)
                .entry(account.to_string())
                .or_default(),
        )
    }

    fn lock_count(&self, account: &str) -> u64 {
        moqui_lib::sync::lock(&self.locks).get(account).copied().unwrap_or(0)
    }

    fn locked(&self, account: &str) {
        *moqui_lib::sync::lock(&self.locks)
            .entry(account.to_string())
            .or_insert(0) += 1;
    }

    /// One call of `account`'s page `page`: an op of the module (`ops::Session`), or the app's own `configure`
    /// (`{idle_minutes}`) and `revisions_seen` (`{vault_id, items: [{item_id, revision}]}`, answering `{older}`).
    ///
    /// # Errors
    ///
    /// The module's refusal, as its text.
    pub fn call(&self, account: &str, page: &str, op: &str, args: Value) -> Result<Value, String> {
        self.call_at(account, page, op, args, now_ms())
    }

    /// [`Self::call`] at the time `now` (epoch millis).
    ///
    /// # Errors
    ///
    /// The module's refusal, as its text.
    pub fn call_at(&self, account: &str, page: &str, op: &str, mut args: Value, now: i64) -> Result<Value, String> {
        if !memory::plain_name(account) || page.is_empty() {
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
                    .clamp(1, MAX_IDLE_MINUTES);
                moqui_lib::sync::lock(&self.idle).insert(slot(account, page), minutes * 60_000);
                return Ok(json!({ "idle_minutes": minutes }));
            }
            "lock" => {
                self.lock_page(account, page);
                return Ok(json!({ "locked": true }));
            }
            "revisions_seen" => return self.revisions_seen(account, &args),
            _ => {}
        }
        let chain_vault = CHAIN_OPS.contains(&op).then(|| vault_of(&args)).flatten();
        let item = ITEM_OPS.contains(&op).then(|| item_of(&args)).flatten();
        {
            let _seen = moqui_lib::sync::lock(&self.seen_lock);
            let seen = seen::load(&self.dir, account);
            if let Some(kept) = chain_vault.as_ref().and_then(|vault| seen.vaults.get(vault)) {
                // the app's memory, never the page's, which the server's code could have set
                args["remembered"] = kept.clone();
            }
            if let Some((vault, item, revision)) = &item {
                let kept = seen.items.get(vault).and_then(|items| items.get(item)).copied();
                if kept.is_some_and(|kept| kept > *revision) {
                    return Err("refused: an older revision of the item than one already seen".to_string());
                }
            }
        }
        args["op"] = Value::String(op.to_string());
        let request = Zeroizing::new(args.to_string());
        moqui_vault::ops::shapes::scrub(&mut args);
        let answer = {
            let mut open = moqui_lib::sync::lock(&self.open);
            let entry = open.entry(slot(account, page)).or_insert_with(|| Open {
                session: Session::default(),
                used: now,
                memory: None,
            });
            if !PASSIVE_OPS.contains(&op) {
                entry.used = now;
            }
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
                // read again under the lock: another call may have kept a higher one since, which stays
                self.change_seen(account, |seen| {
                    let kept = seen.vaults.get(&vault).and_then(seen::order);
                    if kept.is_none_or(|kept| order >= kept) {
                        seen.vaults.insert(vault.clone(), remembered.clone());
                    }
                });
            }
        }
        if let Some((vault, item, revision)) = item {
            self.change_seen(account, |seen| seen.raise(&vault, &item, revision));
        }
        Ok(result)
    }

    /// Load what `account` has seen, change it, and keep it, under the one lock.
    fn change_seen(&self, account: &str, change: impl FnOnce(&mut seen::Seen)) {
        let _seen = moqui_lib::sync::lock(&self.seen_lock);
        let mut seen = seen::load(&self.dir, account);
        change(&mut seen);
        if let Err(err) = seen::save(&self.dir, account, &seen) {
            tracing::error!("What this app has seen of a vault was not kept: {err}");
        }
    }

    /// The items of a list whose revision is lower than one seen before (`older`), and the highest of each kept.
    fn revisions_seen(&self, account: &str, args: &Value) -> Result<Value, String> {
        let vault = args["vault_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("vault_id is required")?;
        let _seen = moqui_lib::sync::lock(&self.seen_lock);
        let mut seen = seen::load(&self.dir, account);
        let mut older = Vec::new();
        for row in args["items"].as_array().map(Vec::as_slice).unwrap_or_default() {
            let (Some(id), Some(revision)) = (row["item_id"].as_str(), row["revision"].as_u64()) else {
                continue;
            };
            if seen.item(vault, id).is_some_and(|kept| kept > revision) {
                older.push(Value::String(id.to_string()));
            } else {
                seen.raise(vault, id, revision);
            }
        }
        seen::save(&self.dir, account, &seen)?;
        Ok(json!({ "older": older }))
    }

    /// Lock one page's session: its keys are dropped, which zeroes them.
    pub fn lock_page(&self, account: &str, page: &str) {
        moqui_lib::sync::lock(&self.open).remove(&slot(account, page));
        self.locked(account);
    }

    /// Lock the sessions of the page `page` of any account (the page left the vault screens, or closed).
    pub fn lock_label(&self, page: &str) {
        let mut accounts = Vec::new();
        moqui_lib::sync::lock(&self.open).retain(|(account, label), _| {
            let gone = label == page;
            if gone {
                accounts.push(account.clone());
            }
            !gone
        });
        for account in accounts {
            self.locked(&account);
        }
    }

    /// Lock every session of `account`.
    pub fn lock(&self, account: &str) {
        moqui_lib::sync::lock(&self.open).retain(|(id, _), _| id != account);
        self.locked(account);
    }

    /// Lock every session (the device locked or slept, the app's pages left the screen).
    pub fn lock_all(&self) {
        let accounts: Vec<String> = {
            let mut open = moqui_lib::sync::lock(&self.open);
            let accounts = open.keys().map(|(account, _)| account.clone()).collect();
            open.clear();
            accounts
        };
        let mut locks = moqui_lib::sync::lock(&self.locks);
        for account in accounts {
            *locks.entry(account).or_insert(0) += 1;
        }
        for count in locks.values_mut() {
            *count += 1;
        }
    }

    /// Lock every session but `account`'s (another account became current).
    pub fn lock_others(&self, account: &str) {
        let others: Vec<String> = {
            let mut open = moqui_lib::sync::lock(&self.open);
            let others = open
                .keys()
                .filter(|(id, _)| id != account)
                .map(|(id, _)| id.clone())
                .collect();
            open.retain(|(id, _), _| id == account);
            others
        };
        for other in others {
            self.locked(&other);
        }
    }

    /// Whether `account`'s page `page` has its personal key open.
    pub fn is_open(&self, account: &str, page: &str) -> bool {
        moqui_lib::sync::lock(&self.open)
            .get(&slot(account, page))
            .is_some_and(|open| open.session.personal_key_id().is_some())
    }

    /// `account` signed out here, or its device key is no longer taken: its sessions lock and its device memory is
    /// forgotten. The memory's id, for the server's record when it can still be told.
    pub fn signed_out(&self, account: &str) -> Option<String> {
        self.lock(account);
        moqui_lib::sync::lock(&self.idle).retain(|(id, _), _| id != account);
        self.forget(account)
    }

    /// `account` removed from the device: as [`Self::signed_out`], and what was seen of its vaults goes too.
    pub fn removed(&self, account: &str) -> Option<String> {
        let forgotten = self.signed_out(account);
        let _seen = moqui_lib::sync::lock(&self.seen_lock);
        seen::remove(&self.dir, account);
        forgotten
    }

    /// Forget `account`'s device memory here: its file and its key. The memory's id, for the server's record.
    pub fn forget(&self, account: &str) -> Option<String> {
        let held = self.memory_lock(account);
        let _held = moqui_lib::sync::lock(&held);
        self.forget_held(account)
    }

    /// [`Self::forget`] with the account's memory lock held.
    fn forget_held(&self, account: &str) -> Option<String> {
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

    /// End what is due at `now`: each device memory past its time (its file and key forgotten, the record read again
    /// under the account's memory lock, so a newer one stays), each session idle past its page's idle time, and each
    /// session opened under a memory that has ended.
    pub fn sweep_at(&self, now: i64) {
        for account in memory::accounts(&self.dir) {
            if !memory::load(&self.dir, &account).is_some_and(|record| record.until <= now) {
                continue;
            }
            let held = self.memory_lock(&account);
            let _held = moqui_lib::sync::lock(&held);
            if memory::load(&self.dir, &account).is_some_and(|record| record.until <= now) {
                tracing::info!("The vault device memory of {account} reached its time and is forgotten");
                self.forget_held(&account);
            }
        }
        let idle = moqui_lib::sync::lock(&self.idle).clone();
        let mut ended = Vec::new();
        moqui_lib::sync::lock(&self.open).retain(|slot, open| {
            let limit = idle.get(slot).copied().unwrap_or(DEFAULT_IDLE_MINUTES * 60_000);
            let keep =
                open.used.saturating_add(limit) > now && open.memory.as_ref().is_none_or(|(_, until)| *until > now);
            if !keep {
                ended.push(slot.0.clone());
            }
            keep
        });
        for account in ended {
            self.locked(&account);
        }
    }

    /// The device memories that open sessions came from, by account: what the app asks the server about while they are
    /// open ([`MEMORY_CHECK_EVERY`]).
    pub fn memories_in_use(&self) -> Vec<(String, String)> {
        let mut used: Vec<(String, String)> = moqui_lib::sync::lock(&self.open)
            .iter()
            .filter_map(|((account, _), open)| open.memory.as_ref().map(|(id, _)| (account.clone(), id.clone())))
            .collect();
        used.sort();
        used.dedup();
        used
    }

    /// The server said `account`'s memory `memory_id` is no longer live: the sessions opened from it lock, and it is
    /// forgotten here (when it is still the one kept).
    pub fn memory_ended(&self, account: &str, memory_id: &str) {
        moqui_lib::sync::lock(&self.open).retain(|(id, _), open| {
            id != account || open.memory.as_ref().is_none_or(|(memory, _)| memory != memory_id)
        });
        self.locked(account);
        let held = self.memory_lock(account);
        let _held = moqui_lib::sync::lock(&held);
        if memory::load(&self.dir, account).is_some_and(|record| record.memory_id == memory_id) {
            self.forget_held(account);
        }
    }

    /// What the page shows of device memory: whether it can be offered here, how the key is kept, when the account's
    /// memory ends, and whether the page's personal key is open.
    pub fn memory_state_at(&self, account: &str, page: &str, now: i64) -> Value {
        self.sweep_at(now);
        let until = memory::load(&self.dir, account).map(|record| record.until);
        json!({ "available": self.memory_kind().is_some(), "kind": self.memory_kind(), "until": until,
                "open": self.is_open(account, page), "maxMinutes": MAX_MEMORY_MINUTES })
    }

    /// Remember the personal key open in `account`'s page `page` on this device for `minutes`, recorded on its server
    /// as `device_name`. Replaces a memory it had. Answers when it ends.
    ///
    /// # Errors
    ///
    /// Refused with no OS key store, with the personal key locked (or locked while this ran), for minutes out of range,
    /// or when the server or the key store does not take it.
    pub fn remember_at(
        &self,
        account: &str,
        page: &str,
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
        let held = self.memory_lock(account);
        let _held = moqui_lib::sync::lock(&held);
        let personal_key_id = moqui_lib::sync::lock(&self.open)
            .get(&slot(account, page))
            .and_then(|open| open.session.personal_key_id().map(str::to_string))
            .ok_or("Open your vault key first")?;
        let mut id_bytes = [0_u8; 16];
        moqui_vault::rng::Rng::fill(&mut OsRng, &mut id_bytes);
        let memory_id = hex(&id_bytes);
        let registered = server
            .register(&memory_id, &personal_key_id, device_name, minutes)
            .map_err(|err| err.to_string())?;
        let until = registered.until.min(now.saturating_add(i64::from(minutes) * 60_000));
        let device_key = Key32::random(&mut OsRng);
        let wrap_key = device_wrap_key(&device_key, &registered.secret, &memory_id);
        drop(registered);
        // the page's key, sealed only if it is still the one open: a lock while the server answered leaves nothing
        let sealed = {
            let open = moqui_lib::sync::lock(&self.open);
            open.get(&slot(account, page))
                .filter(|open| open.session.personal_key_id() == Some(personal_key_id.as_str()))
                .map(|open| open.session.device_seal(&memory_id, until, &wrap_key, &mut OsRng))
        };
        let sealed = match sealed {
            Some(Ok(sealed)) => sealed,
            Some(Err(err)) => {
                forget_on(server, &memory_id);
                return Err(err.to_string());
            }
            None => {
                forget_on(server, &memory_id);
                return Err("The vault locked before the key was remembered".to_string());
            }
        };
        let earlier = self.forget_held(account);
        let kept = keys
            .put(&memory::entry(account), &device_key)
            .and_then(|()| memory::save(&self.dir, account, &Record::new(&memory_id, &sealed)));
        if let Err(err) = kept {
            self.forget_held(account);
            forget_on(server, &memory_id);
            return Err(format!("The key was not remembered: {err}"));
        }
        if let Some(earlier) = earlier {
            forget_on(server, &earlier);
        }
        if let Some(open) = moqui_lib::sync::lock(&self.open).get_mut(&slot(account, page)) {
            open.memory = Some((memory_id, until));
        }
        Ok(json!({ "until": until, "kind": keys.kind() }))
    }

    /// Open `account`'s personal key from device memory into its page `page`, with the server's half of the memory's
    /// key, which the server gives only while the memory is live. `signed_in`: whether the account still has its device
    /// key here.
    pub fn restore_at(
        &self,
        account: &str,
        page: &str,
        signed_in: bool,
        server: &dyn MemoryServer,
        now: i64,
    ) -> Restore {
        if !memory::plain_name(account) || page.is_empty() {
            return Restore::Nothing;
        }
        self.sweep_at(now);
        let held = self.memory_lock(account);
        let _held = moqui_lib::sync::lock(&held);
        let locks_before = self.lock_count(account);
        let Some(record) = memory::load(&self.dir, account) else {
            return Restore::Nothing;
        };
        if !signed_in {
            self.forget_held(account);
            return Restore::Ended("the account is signed out on this device".to_string());
        }
        let secret = match server.check(&record.memory_id) {
            Ok(Some(secret)) => secret,
            Ok(None) => {
                self.forget_held(account);
                return Restore::Ended(
                    "the server no longer has this device's memory (taken off the list, past its time, or the key replaced)"
                        .to_string(),
                );
            }
            Err(ServerError::Revoked) => {
                self.forget_held(account);
                return Restore::Ended("the server no longer takes this device's key".to_string());
            }
            Err(err) => return Restore::Unconfirmed(err.to_string()),
        };
        let Some(keys) = self.keys.get() else {
            return Restore::Unconfirmed("this device has no key store".to_string());
        };
        let device_key = match keys.take(&memory::entry(account)) {
            Ok(Some(key)) => key,
            Ok(None) => {
                self.forget_held(account);
                forget_on(server, &record.memory_id);
                return Restore::Ended("the key store no longer holds the memory key".to_string());
            }
            Err(KeyError::NeedsUnlock) => return Restore::NeedsUnlock,
            Err(KeyError::Failed(err)) => return Restore::Unconfirmed(format!("the key store did not answer: {err}")),
        };
        let wrap_key = device_wrap_key(&device_key, &secret, &record.memory_id);
        drop((device_key, secret));
        let opened = record.sealed().and_then(|sealed| {
            let mut session = Session::default();
            let answer = session
                .device_open(&sealed, &record.memory_id, &wrap_key)
                .map_err(|err| err.to_string())?;
            Ok((session, answer))
        });
        match opened {
            Ok((session, mut answer)) => {
                if self.lock_count(account) != locks_before {
                    return Restore::Unconfirmed("the vault was locked while the key opened".to_string());
                }
                answer["until"] = json!(record.until);
                moqui_lib::sync::lock(&self.open).insert(
                    slot(account, page),
                    Open {
                        session,
                        used: now,
                        memory: Some((record.memory_id.clone(), record.until)),
                    },
                );
                Restore::Opened(answer)
            }
            Err(err) => {
                tracing::error!("The vault device memory of {account} does not open, and is forgotten: {err}");
                self.forget_held(account);
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

/// The vault, item and revision an item op names, when the page gives the revision.
fn item_of(args: &Value) -> Option<(String, String, u64)> {
    let vault = args["vault_id"].as_str().filter(|id| !id.is_empty())?;
    let item = args["item_id"].as_str().filter(|id| !id.is_empty())?;
    let revision = args["revision"].as_u64()?;
    Some((vault.to_string(), item.to_string(), revision))
}

/// How far the wall clock may run ahead of the monotonic one between two sweeps before it is a sleep.
const SLEEP_GAP: Duration = Duration::from_secs(60);

/// Whether the device slept between two sweeps, by the wall clock moving `wall` and the monotonic one, which stands
/// still while the device sleeps, `monotonic` (macOS, Linux and Android; Windows' counts through a sleep, so there the
/// host's sleep notice is all).
pub(crate) fn slept(wall: Option<Duration>, monotonic: Duration) -> bool {
    wall.and_then(|wall| wall.checked_sub(monotonic))
        .is_some_and(|ahead| ahead > SLEEP_GAP)
}

/// Ask each account's server about the device memories that open sessions came from, and lock those that ended.
fn check_memories(client: &Client) {
    for (account, memory_id) in client.vault().memories_in_use() {
        let Some((record, Some(key))) = client.account(&account) else {
            client.vault().memory_ended(&account, &memory_id);
            continue;
        };
        let server = server::RestMemoryServer {
            server: &record.server,
            key: &key,
        };
        match server.check(&memory_id) {
            Ok(Some(_)) => {}
            Ok(None) | Err(ServerError::Revoked) => {
                tracing::info!("The vault device memory of {account} ended on the server: its sessions lock");
                client.vault().memory_ended(&account, &memory_id);
            }
            Err(err) => tracing::info!("The vault device memory of {account} was not checked: {err}"),
        }
    }
}

/// Sweep the client's vaults every half minute, on a thread of its own, until the client is shut down: a memory ends at
/// its time, an idle session locks, whether or not a page calls, and every [`MEMORY_CHECK_EVERY`] the memories open
/// sessions came from are checked with their servers. A sleep of the device locks every session (§3.6), as the clocks
/// tell it; the hosts also lock on the OS's own notices.
pub fn start(client: &Arc<Client>) {
    if client.vault().sweeping.swap(true, Ordering::SeqCst) {
        return;
    }
    let client = Arc::clone(client);
    let started = std::thread::Builder::new()
        .name("client-vault".to_string())
        .spawn(move || {
            let mut checked = std::time::Instant::now();
            loop {
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
                if checked.elapsed() >= MEMORY_CHECK_EVERY {
                    checked = std::time::Instant::now();
                    check_memories(&client);
                }
            }
        });
    if let Err(err) = started {
        tracing::error!("The vault's sweep did not start: {err}");
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
