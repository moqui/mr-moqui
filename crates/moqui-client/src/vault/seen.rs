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

//! What the app has seen of each vault (plans/Vault-Instances.md §3.12, decision (d) of §9), kept per account in
//! `vault/<account>.seen.json`: the highest key version and member-list revision it accepted and the hash of that
//! member list (the module's `remembered`), and the highest revision of each item. The server can hand back older rows;
//! a browser forgets, the app does not.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One account's memory of its vaults.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Seen {
    /// The module's `remembered` (`{version, revision, member_list_hash}`) by vault id.
    #[serde(default)]
    pub vaults: BTreeMap<String, Value>,
    /// The highest revision of each item, by vault id and item id.
    #[serde(default)]
    pub items: BTreeMap<String, BTreeMap<String, u64>>,
}

/// The most vaults whose items are kept for an account, and the most items of a vault: past them, a new vault's or
/// item's revision is not kept (logged), so a server that lists endless ids does not grow the file without bound.
pub(crate) const MAX_VAULTS: usize = 1_000;
pub(crate) const MAX_ITEMS: usize = 20_000;

impl Seen {
    /// The highest revision of `item` of `vault` seen.
    pub(crate) fn item(&self, vault: &str, item: &str) -> Option<u64> {
        self.items.get(vault).and_then(|items| items.get(item)).copied()
    }

    /// Keep `revision` of `item` when it is higher than the one seen; never lowers one.
    pub(crate) fn raise(&mut self, vault: &str, item: &str, revision: u64) {
        if !self.items.contains_key(vault) && self.items.len() >= MAX_VAULTS {
            tracing::warn!("Item revisions are kept for {MAX_VAULTS} vaults at most; this one's are not");
            return;
        }
        let items = self.items.entry(vault.to_string()).or_default();
        let full = items.len() >= MAX_ITEMS;
        match items.get_mut(item) {
            Some(kept) => *kept = (*kept).max(revision),
            None if full => {
                tracing::warn!("Item revisions are kept for {MAX_ITEMS} items of a vault at most; this one's is not");
            }
            None => {
                items.insert(item.to_string(), revision);
            }
        }
    }
}

fn path(dir: &Path, account: &str) -> Option<PathBuf> {
    super::memory::plain_name(account).then(|| dir.join(format!("{account}.seen.json")))
}

/// What `account` has seen; nothing for none kept. One that does not parse is logged and taken as nothing: the rows
/// seen from now on are kept, as a browser's would be.
pub(crate) fn load(dir: &Path, account: &str) -> Seen {
    let Some(text) = path(dir, account).and_then(|path| std::fs::read_to_string(path).ok()) else {
        return Seen::default();
    };
    serde_json::from_str(&text).unwrap_or_else(|err| {
        tracing::error!("What this app has seen of {account}'s vaults does not parse; started again: {err}");
        Seen::default()
    })
}

/// Keep what `account` has seen.
pub(crate) fn save(dir: &Path, account: &str, seen: &Seen) -> Result<(), String> {
    let path = path(dir, account).ok_or_else(|| format!("not an account id: {account}"))?;
    let text = serde_json::to_string(seen).map_err(|err| err.to_string())?;
    crate::files::write_private(&path, text.as_bytes()).map_err(|err| format!("{}: {err}", path.display()))
}

/// Forget what `account` has seen (the account removed from the device).
pub(crate) fn remove(dir: &Path, account: &str) {
    if let Some(path) = path(dir, account) {
        if let Err(err) = std::fs::remove_file(&path) {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("{}: {err}", path.display());
            }
        }
    }
}

/// `(version, revision)` of a `remembered`, for comparing.
pub(crate) fn order(remembered: &Value) -> Option<(u64, u64)> {
    Some((remembered["version"].as_u64()?, remembered["revision"].as_u64()?))
}
