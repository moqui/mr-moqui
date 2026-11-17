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

//! The phone's dialer and Contacts calling through the app (plans/Comms-Messaging.md §9.1), the part that is Rust's:
//! the native code (`gen/android/.../DialAccount.kt`) has a managed `tel:` calling account, registered while an
//! account signed in here has a number to call from ([`DialAccounts`]), and hands each number dialed with it to the app.
//!
//! - **The link** is `org.moqui.mobile:/dial?nonce=...` (the activity's intent, `RunEvent::Opened`). It carries no
//!   number: the native code keeps the number with Rust under a nonce ([`DialLinks`], once, within [`LINK_MS`]), so
//!   another app, which may send the exported activity any link, cannot have the app call a number of its choice.
//! - **The call** is the Dialer's of the account that calls ([`DialAccounts::choose`]): the shell shows it with the
//!   number ([`dialer_path`]), and the Dialer places it once through the server (`place#PhoneCall`) when the host
//!   hands it the number (`call_take_dial`); a Dialer opened any other way only fills the number in.

// The dialer's link is Android's; it is tested on the desktop.
#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// The deep link's path a number from the phone's dialer opens the app with.
pub(crate) const DIAL_LINK_PATH: &str = "/dial";

/// How long a number handed over may be taken, in milliseconds (the native code's notification lasts as long).
pub(crate) const LINK_MS: i64 = 120_000;

/// The longest number taken from the dialer.
const NUMBER_MAX: usize = 32;

/// A number the phone's dialer hands over (a `tel:` URI's number, decoded) as the app calls it: digits with at most a
/// leading `+`, and `*` and `#`, its spaces, dashes, dots, slashes and brackets dropped; at least three digits, at most
/// [`NUMBER_MAX`] characters. None for anything else (a pause or an extension, `,` `;`, letters).
pub(crate) fn dial_number(text: &str) -> Option<String> {
    let plain: String = text
        .trim()
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '.' | '/' | '(' | ')'))
        .collect();
    let digits = plain.chars().filter(char::is_ascii_digit).count();
    let shaped = plain
        .char_indices()
        .all(|(at, c)| c.is_ascii_digit() || c == '*' || c == '#' || (c == '+' && at == 0));
    (shaped && digits >= 3 && plain.len() <= NUMBER_MAX).then_some(plain)
}

/// The numbers the native code handed over and not yet taken, by nonce.
#[derive(Default)]
pub(crate) struct DialLinks {
    made: HashMap<String, (String, i64)>,
}

impl DialLinks {
    /// A number handed over at `now` (Unix milliseconds): its nonce, for the link; an error for a number the app does
    /// not call ([`dial_number`]).
    pub(crate) fn make(&mut self, number: &str, now: i64) -> Result<String, String> {
        let number = dial_number(number).ok_or("Not a number the app calls")?;
        self.made.retain(|_, (_, made)| now - *made < LINK_MS);
        let nonce = moqui_client::store::new_client_ref().map_err(|err| err.to_string())?;
        self.made.insert(nonce.clone(), (number, now));
        Ok(nonce)
    }

    /// The number of `nonce`, taken once: none for a nonce not made, taken already or too old.
    pub(crate) fn take(&mut self, nonce: &str, now: i64) -> Option<String> {
        let (number, made) = self.made.remove(nonce)?;
        (now - made < LINK_MS && now >= made).then_some(number)
    }
}

/// Whether `url` is a dial link at all (taken or refused).
pub(crate) fn is_dial_link(url: &tauri::Url) -> bool {
    url.scheme() == "org.moqui.mobile" && url.path() == DIAL_LINK_PATH
}

/// The nonce of an `org.moqui.mobile:/dial?nonce=...` link; none for another link or no nonce.
pub(crate) fn nonce_of(url: &tauri::Url) -> Option<String> {
    if !is_dial_link(url) {
        return None;
    }
    url.query_pairs()
        .find(|(name, value)| name == "nonce" && !value.is_empty() && value.len() <= 64)
        .map(|(_, value)| value.into_owned())
}

/// The Dialer of the comms app at `root` (`/qapps/BasicComms`) with `number` ([`dial_number`]'s), which it places once
/// the host hands it over; `seq` tells one handing from the next, so a second one while the Dialer shows is new to it.
pub(crate) fn dialer_path(root: &str, number: &str, seq: u64) -> String {
    let encoded: String = number
        .chars()
        .map(|c| match c {
            '+' => "%2B".to_string(),
            '#' => "%23".to_string(),
            '*' => "%2A".to_string(),
            c => c.to_string(),
        })
        .collect();
    format!(
        "{}/Calls/Dialer?number={encoded}&dial={seq}",
        root.trim_end_matches('/')
    )
}

/// The accounts signed in here with a number to call from (the shell says so as it loads for each, `call_dial_account`),
/// in `dial.json` of the client's directory: the calling account is registered while there is one.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct DialAccounts {
    #[serde(default)]
    accounts: BTreeSet<String>,
}

impl DialAccounts {
    fn file(dir: &Path) -> PathBuf {
        dir.join("dial.json")
    }

    /// The accounts of `dir`, or none.
    pub(crate) fn load(dir: &Path) -> Self {
        std::fs::read(Self::file(dir))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Keep them.
    pub(crate) fn save(&self, dir: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec(self).map_err(|err| err.to_string())?;
        std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
        let path = Self::file(dir);
        let temp = path.with_extension("json.part");
        std::fs::write(&temp, bytes).map_err(|err| format!("{}: {err}", temp.display()))?;
        std::fs::rename(&temp, &path).map_err(|err| format!("{}: {err}", path.display()))
    }

    /// Whether `account` can call (`on`) or not (or signed out). Whether that changed.
    pub(crate) fn set(&mut self, account: &str, on: bool) -> bool {
        if on {
            self.accounts.insert(account.to_string())
        } else {
            self.accounts.remove(account)
        }
    }

    /// Whether an account here can call: the calling account is registered.
    pub(crate) fn any(&self) -> bool {
        !self.accounts.is_empty()
    }

    /// The account a number from the dialer is called from: the current one when it can call, else the first one
    /// signed in (`signed_in`) that can; none when none can (the current one's Dialer then says why).
    pub(crate) fn choose(&self, current: Option<&str>, signed_in: &[String]) -> Option<String> {
        if let Some(current) = current.filter(|current| self.accounts.contains(*current)) {
            return Some(current.to_string());
        }
        signed_in
            .iter()
            .find(|account| self.accounts.contains(*account))
            .cloned()
    }
}

#[cfg(test)]
#[path = "dial_tests.rs"]
mod tests;
