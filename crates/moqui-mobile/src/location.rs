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

//! Live location sent from the phone (plans/Location-Sharing.md §3.4, LL3): the Maps page's Share asks the app to
//! send a share of its account (`location_share_start`, checked here: [`ShareAsk`]), and `LocationShareService.kt`, a
//! foreground service of type `location` started only then, with the app in front, hands each fix to
//! [`report_location`] (`Native.reportLocation`, the host command `report_location`). Each share's
//! [`moqui_client::location::Reporter`] keeps only the newest point not yet sent and sends it to the account's server
//! through the client's proxy with the account's device key, as the app's other requests go, so the share's device
//! binding (`LocationShare.userLoginKeyId`) takes it; what it came to goes back to the service as JSON, which stops the
//! share at a 410, 404, another device's takeover or a sign-out. The notification's Stop stops the share on the server
//! too ([`stop_share`]); an account signed out or removed has its shares dropped here ([`forget_account`]) and the
//! service told. No position is logged.

use std::collections::HashMap;
use std::sync::Mutex;

use moqui_client::account::Account;
use moqui_client::location::{Fix, Next, Outcome, Reporter, Stopped, REPORT_PATH, STOP_PATH};
use serde_json::Value;

use crate::calls::Poster;

/// The shares sent from here, by account and share: their reporters.
static REPORTERS: Mutex<Option<HashMap<(String, String), Reporter>>> = Mutex::new(None);

/// The longest a share may be asked to be sent for: a share lasts at most `location@max-share-hours` (24 by default),
/// and an installation may allow more; a longer end is the page's mistake.
const SHARE_MAX_MS: i64 = 8 * 24 * 3_600_000;

/// A share the page asks the app to send from this phone.
#[derive(serde::Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ShareAsk {
    pub(crate) share_id: String,
    /// The share's end, epoch milliseconds.
    pub(crate) thru_date: i64,
    pub(crate) approximate: bool,
    /// The share's report interval (`location@min-interval-seconds`, or the approximate one).
    pub(crate) interval_seconds: i64,
    /// Whom it is with, for the notification ("Sam and 2 others").
    pub(crate) audience: String,
}

impl ShareAsk {
    /// The ask checked at `now_ms`: a share id of the server's (letters, digits, `_` and `-`, at most 64), an end after
    /// now and within [`SHARE_MAX_MS`], an interval of 1 to 3600 seconds, the audience's words clipped. Else why not.
    pub(crate) fn checked(mut self, now_ms: i64) -> Result<Self, String> {
        let id = self.share_id.trim();
        if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return Err("Not a share".to_string());
        }
        self.share_id = id.to_string();
        if self.thru_date <= now_ms || self.thru_date > now_ms + SHARE_MAX_MS {
            return Err("The share's end is not ahead".to_string());
        }
        if !(1..=3600).contains(&self.interval_seconds) {
            return Err("Not an interval".to_string());
        }
        self.audience = crate::commands::clip(&self.audience, 200);
        Ok(self)
    }

    /// The share as the native service takes it, with its account.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn to_json(&self, account: &str) -> String {
        serde_json::json!({
            "account": account,
            "shareId": self.share_id,
            "thruDate": self.thru_date,
            "approximate": self.approximate,
            "intervalSeconds": self.interval_seconds,
            "audience": self.audience,
        })
        .to_string()
    }
}

fn with_reporters<T>(work: impl FnOnce(&mut HashMap<(String, String), Reporter>) -> T) -> T {
    let mut held = moqui_lib::sync::lock(&REPORTERS);
    work(held.get_or_insert_with(HashMap::new))
}

/// The share `share_id` of `account` is sent from here from now on, at most once an `interval_ms`: a reporter for it,
/// a new one when it was stopped (a Send from this device after another device had it).
pub(crate) fn begin(account: &str, share_id: &str, interval_ms: i64) {
    with_reporters(|reporters| {
        let key = (account.to_string(), share_id.to_string());
        match reporters.get_mut(&key) {
            Some(reporter) if reporter.stopped().is_none() => reporter.set_interval(interval_ms),
            _ => {
                reporters.insert(key, Reporter::new(share_id, interval_ms));
            }
        }
    });
}

/// The host command `report_location` (`Native.reportLocation`): the fix in `fix_json` (none: send a point that waited)
/// of the share `share_id` of `account`, sent now when it may go, through `poster` with the account and its device key
/// (`signed_in`, none when the account is not signed in here, which stops it). Its [`Outcome`] as JSON. The report is
/// posted with no lock held, so a stop or a sign-out meanwhile is not held up; the reporter takes the answer unless it
/// went meanwhile.
pub(crate) fn report_location(
    account: &str,
    share_id: &str,
    fix_json: Option<&str>,
    signed_in: Option<(&Account, &str)>,
    poster: &dyn Poster,
    now_ms: i64,
) -> String {
    let fix = fix_json
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .and_then(|json| Fix::from_json(&json));
    let entry = (account.to_string(), share_id.to_string());
    let next = with_reporters(|reporters| {
        let Some(reporter) = reporters.get_mut(&entry) else {
            // never begun here, or forgotten (a sign-out): the service stops it
            return Err(Outcome::Stopped(Stopped::SignedOut));
        };
        if signed_in.is_none() {
            return Err(reporter.stop(Stopped::SignedOut));
        }
        if let Some(fix) = fix {
            reporter.offer(fix);
        }
        match reporter.next(now_ms) {
            Next::Send(sent, form) => Ok((sent, form)),
            Next::Wait(ms) => Err(Outcome::Waiting(ms)),
            Next::Idle => Err(Outcome::Idle),
            Next::Stopped(why) => Err(Outcome::Stopped(why)),
        }
    });
    let outcome = match (next, signed_in) {
        (Err(outcome), _) => outcome,
        (Ok((sent, form)), Some((server, key))) => {
            let fields: Vec<(&str, &str)> = form
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();
            let answer = poster.post(server, key, REPORT_PATH, &fields);
            if let Err(err) = &answer {
                // the share and the failure only: never the position
                log::info!("Location: a report of share {share_id} did not reach {account}: {err}");
            }
            with_reporters(|reporters| match reporters.get_mut(&entry) {
                Some(reporter) => reporter.answered(sent, answer, now_ms),
                None => Outcome::Stopped(Stopped::SignedOut),
            })
        }
        (Ok(_), None) => Outcome::Stopped(Stopped::SignedOut),
    };
    if let Outcome::Stopped(why) = outcome {
        log::info!(
            "Location: share {share_id} of {account} is no longer sent from here ({})",
            why.as_str()
        );
        with_reporters(|reporters| reporters.remove(&entry));
    }
    outcome.to_json().to_string()
}

/// The notification's Stop, or the page's (`stop_on_server` false: the page stopped it itself): the share is no longer
/// sent from here, and with `stop_on_server` it is stopped on the account's server (`stop#Share`), so its audience is
/// told at once. Whether the server took the stop (true with nothing to tell it).
pub(crate) fn stop_share(
    account: &str,
    share_id: &str,
    stop_on_server: bool,
    signed_in: Option<(&Account, &str)>,
    poster: &dyn Poster,
) -> bool {
    with_reporters(|reporters| reporters.remove(&(account.to_string(), share_id.to_string())));
    if !stop_on_server {
        return true;
    }
    let Some((server, key)) = signed_in else {
        return false;
    };
    match poster.post(server, key, STOP_PATH, &[("shareId", share_id)]) {
        Ok((status, json)) if status < 400 && json.get("stopped").and_then(Value::as_bool) == Some(true) => true,
        Ok((status, _)) => {
            log::info!("Location: {account} did not take the stop of share {share_id}: HTTP {status}");
            false
        }
        Err(err) => {
            log::info!("Location: the stop of share {share_id} did not reach {account}: {err}");
            false
        }
    }
}

/// The share `share_id` of `account` is no longer sent from here, with nothing told to its server.
pub(crate) fn forget_share(account: &str, share_id: &str) {
    with_reporters(|reporters| reporters.remove(&(account.to_string(), share_id.to_string())));
}

/// `account` is signed out or removed here: none of its shares is sent from here any more. The shares it had.
pub(crate) fn forget_account(account: &str) -> Vec<String> {
    with_reporters(|reporters| {
        let gone: Vec<String> = reporters
            .keys()
            .filter(|(of, _)| of == account)
            .map(|(_, share)| share.clone())
            .collect();
        reporters.retain(|(of, _), _| of != account);
        gone
    })
}

/// The shares of `account` sent from here now.
pub(crate) fn sending(account: &str) -> Vec<String> {
    with_reporters(|reporters| {
        let mut shares: Vec<String> = reporters
            .iter()
            .filter(|((of, _), reporter)| of == account && reporter.stopped().is_none())
            .map(|((_, share), _)| share.clone())
            .collect();
        shares.sort();
        shares
    })
}

#[cfg(test)]
#[path = "location_tests.rs"]
mod tests;
