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

//! The accounts of this device: a server and a user on it, each with its own device key (kept apart, in
//! [`crate::keys`]). The list and the current account are `accounts.json` in a directory the host gives, so they
//! survive a restart; the keys never go in it.

use std::path::{Path, PathBuf};

use moqui_lib::error::Error;
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

use crate::conf::DeviceMode;

/// One server and one user signed in on it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// Stable for the server and username ([`account_id`]): the key store's name and the page's storage scope.
    pub id: String,
    /// `https://host[:port][/path]`, no trailing slash ([`base_url`]).
    pub server: String,
    pub username: String,
    /// The user's `userId`, from the server's client config after sign-in.
    #[serde(default)]
    pub user_id: String,
    /// The user's name as the server shows it.
    #[serde(default)]
    pub display: String,
    /// The server's name, from its client config.
    #[serde(default)]
    pub server_name: String,
    /// `Personal` or `Shared`, chosen when the account is added and never lowered ([`Accounts::put`],
    /// [`Accounts::set_device_mode`]); none for an account from before MD4, which the host treats as `choose`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "device_mode_of"
    )]
    pub device_mode: Option<DeviceMode>,
}

/// What `accounts.json` holds.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    accounts: Vec<Account>,
    #[serde(default)]
    current: Option<String>,
}

/// The accounts of this device and which one the windows show.
#[derive(Debug)]
pub struct Accounts {
    file: PathBuf,
    stored: Stored,
}

impl Accounts {
    /// The accounts kept in `dir`; none when there is no file yet, or it does not parse (logged).
    pub fn load(dir: &Path) -> Self {
        let file = dir.join("accounts.json");
        let stored = match std::fs::read_to_string(&file) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
                tracing::error!("{} does not parse, starting with no accounts: {err}", file.display());
                Stored::default()
            }),
            Err(_) => Stored::default(),
        };
        Self { file, stored }
    }

    /// Write the list and the current account.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory or file cannot be written.
    pub fn save(&self) -> Result<(), Error> {
        let text = serde_json::to_string_pretty(&self.stored).map_err(|err| Error::new(err.to_string()))?;
        crate::files::write_private(&self.file, text.as_bytes())
            .map_err(|err| Error::new(format!("{}: {err}", self.file.display())))
    }

    pub fn list(&self) -> &[Account] {
        &self.stored.accounts
    }

    pub fn get(&self, id: &str) -> Option<&Account> {
        self.stored.accounts.iter().find(|account| account.id == id)
    }

    /// The account the windows show, if it is still in the list.
    pub fn current(&self) -> Option<&Account> {
        self.get(self.stored.current.as_deref()?)
    }

    /// Make `id` current; false when there is no such account.
    pub fn set_current(&mut self, id: &str) -> bool {
        let known = self.get(id).is_some();
        if known {
            self.stored.current = Some(id.to_string());
        }
        known
    }

    /// Add the account with its device mode, or replace the one with its id. A replaced account keeps a mode that
    /// keeps more ([`kept_mode`]): a personal account signed in again as shared stays personal.
    pub fn put(&mut self, mut account: Account) {
        account.device_mode = account.device_mode.filter(|mode| *mode != DeviceMode::Choose);
        match self.stored.accounts.iter_mut().find(|kept| kept.id == account.id) {
            Some(kept) => {
                account.device_mode = kept_mode(kept.device_mode, account.device_mode);
                *kept = account;
            }
            None => self.stored.accounts.push(account),
        }
    }

    /// Set the device mode of `id`, only raising it: an account with none takes either, a shared one may become
    /// personal, a personal one stays personal (a shared account is removed on sign-out, so it is never lowered back).
    /// `Choose` changes nothing. False when there is no such account or the mode was not changed.
    pub fn set_device_mode(&mut self, id: &str, mode: DeviceMode) -> bool {
        let Some(kept) = self.stored.accounts.iter_mut().find(|kept| kept.id == id) else {
            return false;
        };
        let wanted = Some(mode).filter(|mode| *mode != DeviceMode::Choose);
        let next = kept_mode(kept.device_mode, wanted);
        let changed = next != kept.device_mode;
        kept.device_mode = next;
        changed
    }

    /// Remove the account; when it was current, no account is.
    pub fn remove(&mut self, id: &str) {
        self.stored.accounts.retain(|account| account.id != id);
        if self.stored.current.as_deref() == Some(id) {
            self.stored.current = None;
        }
    }
}

/// The mode an account keeps when `wanted` comes for it: never lowered from personal, as personal keeps more data;
/// none (not chosen) takes what comes.
pub fn kept_mode(current: Option<DeviceMode>, wanted: Option<DeviceMode>) -> Option<DeviceMode> {
    match (current, wanted) {
        (Some(DeviceMode::Personal), _) | (_, Some(DeviceMode::Personal)) => Some(DeviceMode::Personal),
        (Some(DeviceMode::Shared), _) | (_, Some(DeviceMode::Shared)) => Some(DeviceMode::Shared),
        _ => None,
    }
}

/// `deviceMode` of `accounts.json`: `personal` or `shared`; anything else (`choose`, a mode of a newer client, a value
/// that is not text) is none, logged, rather than the whole file failing to parse, which would lose the accounts on the
/// next save.
fn device_mode_of<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<DeviceMode>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(match value.as_str().map(DeviceMode::parse) {
        Some(Some(DeviceMode::Choose)) => None,
        Some(Some(mode)) => Some(mode),
        _ if value.is_null() => None,
        _ => {
            tracing::warn!("accounts.json deviceMode {value} is not known, taken as not chosen");
            None
        }
    })
}

/// `https://host[:port][/path]` from what the user typed, refusing plain http to anything but this machine: the
/// password and then the key would cross the network in the clear.
///
/// # Errors
///
/// Returns an error for a url that does not parse, a scheme other than http(s), or `http://` to another machine.
pub fn base_url(server: &str) -> Result<String, Error> {
    let typed = server.trim();
    let with_scheme = if typed.contains("://") {
        typed.to_string()
    } else {
        format!("https://{typed}")
    };
    let url = reqwest::Url::parse(&with_scheme).map_err(|err| Error::new(format!("{typed}: {err}")))?;
    let Some(host) = url.host_str().filter(|host| !host.is_empty()) else {
        return Err(Error::new(format!("{typed}: no host")));
    };
    match url.scheme() {
        "https" => {}
        "http" if matches!(host, "localhost" | "127.0.0.1" | "[::1]") => {}
        "http" => {
            return Err(Error::new(format!(
                "{typed}: plain http sends the password over the network; use https, or http to this machine"
            )))
        }
        other => return Err(Error::new(format!("{typed}: {other} is not http or https"))),
    }
    let port = url.port().map(|port| format!(":{port}")).unwrap_or_default();
    Ok(format!(
        "{}://{host}{port}{}",
        url.scheme(),
        url.path().trim_end_matches('/')
    ))
}

/// The id of a server's account: the host for a reader, and a hash of the base url and username so two users, or two
/// servers on one host, do not share one. Letters, digits, `.` and `-` only, as a key store and a storage key take.
pub fn account_id(base: &str, username: &str) -> String {
    let host: String = reqwest::Url::parse(base)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let digest = Sha256::digest(format!("{base}\n{username}").as_bytes());
    format!("{host}-{}", &moqui_lib::codec::hex_encode(&digest)[..12])
}

#[cfg(test)]
#[path = "account_tests.rs"]
mod tests;
