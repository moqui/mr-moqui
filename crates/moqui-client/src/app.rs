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

//! The client's state while the app runs as a client of a server, for any host (plans/Mobile-Desktop.md §4
//! configuration 3, MD3 and MD4): the accounts and their keys, their stores, the conf, the mirror, the outbox's sender,
//! the proxy and the pages' sockets. The host (the desktop app on Tauri, later the mobile app) holds a [`Client`] in an
//! `Arc` and gives it a [`ClientHost`] for what only it can do: know which account a page shows, tell its pages, and
//! check a key the server refused.
//!
//! The threads it starts (the outbox's sender, the mirror, the key renewal) are plain threads with blocking requests,
//! as the rest of this crate.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::Value;

use crate::account::{Account, Accounts};
use crate::bridge::Sockets;
use crate::conf::{ClientConf, DeviceMode};
use crate::keys::Keys;
use crate::proxy::Proxy;
use crate::store::Store;
use crate::stores::Stores;
use crate::sync::flush::Flusher;
use crate::sync::mirror::Mirror;
use crate::{modes, offline};

/// What the client asks of its host, whose pages (webviews, by label) these are.
pub trait ClientHost: Send + Sync {
    /// The id of the account the page `page` shows: the shell's is the current one, an app window's its own. None for
    /// a page that shows none.
    fn page_account(&self, page: &str) -> Option<String>;

    /// Whether `page` is the shell, whose `/notws` opening starts the mirror.
    fn is_shell(&self, page: &str) -> bool {
        page == "main"
    }

    /// Tell the account's pages of an outbox change: `event` (`{kind}`) to their `window.__moquiOutboxEvent`.
    fn notify_pages(&self, account: &str, event: &Value);

    /// An event of a socket a page holds ([`crate::bridge::event_json`]) to its `window.__moquiSocketEvent(id, event)`.
    fn socket_event(&self, page: &str, id: u64, event: &Value);

    /// The server answered 401 to a request with the account's key: check that the key still signs in, and if it does
    /// not, sign the account out here.
    fn check_key(&self, account: &Account);
}

/// The client's state, shared by the host and the client's threads.
pub struct Client {
    pub accounts: Mutex<Accounts>,
    pub keys: Keys,
    /// The accounts' stores, in `data/` of the client's directory.
    pub stores: Stores,
    /// The client's settings once read, and how the host reads them.
    conf: OnceLock<ClientConf>,
    read_conf: Box<dyn Fn() -> ClientConf + Send + Sync>,
    /// Each account's comms app (`calls.appRoot` of its client config), once known.
    comms: Mutex<HashMap<String, String>>,
    pub mirror: Mirror,
    /// Wakes the thread that sends the outboxes.
    pub flusher: Flusher,
    pub proxy: Proxy,
    pub sockets: Sockets,
}

impl Client {
    /// The client of the directory `dir` (`accounts.json`, `data/`), with its keys in `keys` and its settings read by
    /// `read_conf` the first time they are asked for.
    ///
    /// # Errors
    ///
    /// Returns an error when the proxy's HTTP client cannot be made.
    pub fn new(dir: &Path, keys: Keys, read_conf: Box<dyn Fn() -> ClientConf + Send + Sync>) -> Result<Self, String> {
        Ok(Self {
            accounts: Mutex::new(Accounts::load(dir)),
            keys,
            stores: Stores::new(dir),
            conf: OnceLock::new(),
            read_conf,
            comms: Mutex::new(HashMap::new()),
            mirror: Mirror::default(),
            flusher: Flusher::default(),
            proxy: Proxy::new().map_err(|err| err.to_string())?,
            sockets: Sockets::default(),
        })
    }

    /// The current account and its key, if it has one.
    pub fn current(&self) -> Option<(Account, Option<String>)> {
        let account = moqui_lib::sync::lock(&self.accounts).current().cloned()?;
        let key = self.keys.get(&account.id);
        Some((account, key))
    }

    /// The account and key of an account id.
    pub fn account(&self, id: &str) -> Option<(Account, Option<String>)> {
        let account = moqui_lib::sync::lock(&self.accounts).get(id).cloned()?;
        let key = self.keys.get(&account.id);
        Some((account, key))
    }

    /// The account and key of the account a page shows ([`ClientHost::page_account`]).
    pub fn account_of(&self, host: &dyn ClientHost, page: &str) -> Option<(Account, Option<String>)> {
        self.account(&host.page_account(page)?)
    }

    /// The client's settings, read the first time they are asked for.
    pub fn conf(&self) -> &ClientConf {
        self.conf.get_or_init(|| (self.read_conf)())
    }

    /// Run `work` on the store of the account `id`, opened if it is not yet; none for an account no longer kept here
    /// (so a late answer does not make the store of a forgotten account again), or a store that does not open.
    pub fn with_store<T>(&self, id: &str, work: impl FnOnce(&Store) -> T) -> Option<T> {
        // asked under the stores' lock, so an account removed before its store is forgotten is never opened again
        let kept = || moqui_lib::sync::lock(&self.accounts).get(id).is_some();
        self.stores.with(id, self.conf().store_max_bytes(), kept, work)
    }

    /// The device mode an account's data is kept by ([`modes::effective`]).
    pub fn mode_of(&self, id: &str) -> DeviceMode {
        let mode = moqui_lib::sync::lock(&self.accounts)
            .get(id)
            .and_then(|account| account.device_mode);
        modes::effective(mode, self.conf().device_mode)
    }

    /// The comms app of an account's server: `calls.appRoot` of its kept client config, else the default.
    pub fn comms_root_of(&self, id: &str) -> String {
        if let Some(root) = moqui_lib::sync::lock(&self.comms).get(id) {
            return root.clone();
        }
        let root = self
            .with_store(id, |store| store.answer("/qapps/clientConfig", ""))
            .flatten()
            .and_then(|config| comms_root_in(&config.body))
            .unwrap_or_else(|| offline::DEFAULT_COMMS_ROOT.to_string());
        moqui_lib::sync::lock(&self.comms).insert(id.to_string(), root.clone());
        root
    }

    /// The comms app of an account as noted in memory, never read from the store or the conf (for the main thread).
    pub fn noted_comms_root(&self, id: &str) -> Option<String> {
        moqui_lib::sync::lock(&self.comms).get(id).cloned()
    }

    /// Forget the noted comms app of an account (it was removed).
    pub fn forget_comms_root(&self, id: &str) {
        moqui_lib::sync::lock(&self.comms).remove(id);
    }

    /// A client config of an account was answered: its comms app noted.
    pub fn config_seen(&self, id: &str, body: &[u8]) {
        let root = comms_root_in(body).unwrap_or_else(|| offline::DEFAULT_COMMS_ROOT.to_string());
        moqui_lib::sync::lock(&self.comms).insert(id.to_string(), root);
    }

    /// Read the conf and the current account's comms app on a thread of their own, so neither is first read on the
    /// host's main thread (a deep link) or holds up the shell's first request.
    pub fn warm(self: &Arc<Self>) {
        let client = Arc::clone(self);
        let started = std::thread::Builder::new()
            .name("client-warm".to_string())
            .spawn(move || {
                client.conf();
                let current = moqui_lib::sync::lock(&client.accounts)
                    .current()
                    .map(|account| account.id.clone());
                if let Some(id) = current {
                    client.comms_root_of(&id);
                }
            });
        if let Err(err) = started {
            tracing::warn!("The client's conf was not read ahead: {err}");
        }
    }
}

/// `calls.appRoot` of a client config's JSON.
fn comms_root_in(config: &[u8]) -> Option<String> {
    let config: Value = serde_json::from_slice(config).ok()?;
    config["calls"]["appRoot"]
        .as_str()
        .filter(|root| !root.is_empty())
        .map(str::to_string)
}
