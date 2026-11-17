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
//!   number: the native code keeps the number with Rust under a nonce ([`DialLinks`], once, within [`LINK_MS`]), so a
//!   link sent to the exported activity by itself opens nothing. The nonce proves only that the number came through
//!   Telecom to the calling account: any app may place a call with that account (`placeCall`, `ACTION_CALL`) and so
//!   choose the number. So nothing is called by itself.
//! - **The call** is the user's: the Dialer of the account that calls ([`DialAccounts::choose`]) shows the number
//!   ([`dialer_path`]) and the user presses Call, once, which places it through the server (`place#PhoneCall`). When
//!   the current account cannot call and several others can, the user picks which ([`DialChoice`]); the app does not
//!   switch accounts for a number on its own.

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

/// The Dialer of the comms app at `root` (`/qapps/BasicComms`) with `number` ([`dial_number`]'s) filled in; with
/// `choose`, it asks the user which account calls ([`DialChoice`]).
pub(crate) fn dialer_path(root: &str, number: &str, choose: bool) -> String {
    let encoded: String = number
        .chars()
        .map(|c| match c {
            '+' => "%2B".to_string(),
            '#' => "%23".to_string(),
            '*' => "%2A".to_string(),
            c => c.to_string(),
        })
        .collect();
    let ask = if choose { "&chooseAccount=true" } else { "" };
    format!("{}/Calls/Dialer?number={encoded}{ask}", root.trim_end_matches('/'))
}

/// Which account a number from the phone's dialer is called from ([`DialAccounts::choose`]).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Chosen {
    /// The current account: it can call, or none can (its Dialer then says why).
    Current,
    /// The one account signed in here that can call: made current.
    Switch(String),
    /// Several can, the current one not: the user picks.
    Ask(Vec<String>),
}

/// A number from the phone's dialer waiting for the user's pick of the account that calls it: shown on the Dialer of
/// `shown_on` (the current account then), for [`LINK_MS`] from `at`, once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DialChoice {
    pub(crate) number: String,
    pub(crate) accounts: Vec<String>,
    pub(crate) shown_on: Option<String>,
    pub(crate) at: i64,
}

impl DialChoice {
    /// The choice, while it lasts and for the account it was shown on.
    pub(crate) fn current(&self, account: Option<&str>, now: i64) -> bool {
        now >= self.at && now - self.at < LINK_MS && self.shown_on.as_deref() == account
    }
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

    /// The account a number from the dialer is called from: the current one when it can call; else the one signed in
    /// (`signed_in`) that can, or the user's pick of several; the current one when none can.
    pub(crate) fn choose(&self, current: Option<&str>, signed_in: &[String]) -> Chosen {
        if current.is_some_and(|current| self.accounts.contains(current)) {
            return Chosen::Current;
        }
        let able: Vec<String> = signed_in
            .iter()
            .filter(|account| self.accounts.contains(*account))
            .cloned()
            .collect();
        match able.len() {
            0 => Chosen::Current,
            1 => Chosen::Switch(able.into_iter().next().unwrap_or_default()),
            _ => Chosen::Ask(able),
        }
    }

    /// Only the accounts still signed in here (`signed_in`): a key revoked elsewhere, or an account gone, is dropped at
    /// the next start. Whether that changed anything.
    pub(crate) fn keep_only(&mut self, signed_in: &[String]) -> bool {
        let before = self.accounts.len();
        self.accounts.retain(|account| signed_in.contains(account));
        self.accounts.len() != before
    }
}

#[cfg(test)]
#[path = "dial_tests.rs"]
mod tests;
