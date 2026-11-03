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

//! Device keys renewed before they run out (plans/Mobile-Desktop.md §4, "Remote applications and authentication"): when
//! the client starts and every few hours, each account signed in here whose key has less than a third of its life left
//! takes a new one (`/qapps/renewClientDevice`), and its pages' sockets open again with it. The server ends the old key
//! a few minutes later, so requests already sent with it still pass.
//!
//! One account that fails (its server down, or older than `clientApi` 2) is logged and the others go on; its key is
//! tried again on the next pass, and a key that runs out sends the user to the sign-in as a revoked one does.

use std::time::Duration;

use tauri::{AppHandle, Manager};

use super::Client;

/// How often the keys are looked at.
const EVERY: Duration = Duration::from_secs(6 * 3600);

/// Look at the keys now and every few hours, on a thread of its own (reqwest's blocking client).
pub(crate) fn start(app: &AppHandle) {
    let app = app.clone();
    let started = std::thread::Builder::new()
        .name("client-renewal".to_string())
        .spawn(move || loop {
            renew_due(&app);
            std::thread::sleep(EVERY);
        });
    if let Err(err) = started {
        tracing::error!("The device key renewal did not start: {err}");
    }
}

/// Renew each account's key that is due.
fn renew_due(app: &AppHandle) {
    let Some(client) = app.try_state::<Client>() else {
        return;
    };
    let accounts = moqui_lib::sync::lock(&client.accounts).list().to_vec();
    for account in accounts {
        let Some(key) = client.keys.get(&account.id) else {
            continue;
        };
        match renew_one(&client, &account.server, &account.id, &key) {
            Ok(true) => {
                let reopened = client.sockets.reopen_account(app, &client, &account.id);
                super::flush::release_held(&client, &account.id);
                tracing::info!(
                    "Renewed the device key of {} ({reopened} sockets open again)",
                    account.id
                );
            }
            Ok(false) => {}
            Err(err) => tracing::warn!("The device key of {} was not renewed: {err}", account.id),
        }
    }
}

/// Renew one key if it is due; whether it was.
fn renew_one(client: &Client, server: &str, id: &str, key: &str) -> Result<bool, String> {
    // a key the server no longer takes is the 401 path's to handle, when a page next asks
    let Some((from, thru)) = moqui_client::login::key_period(server, key).map_err(|err| err.to_string())? else {
        return Ok(false);
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX));
    if !moqui_client::login::renew_due(from, thru, now) {
        return Ok(false);
    }
    let Some(renewed) = moqui_client::login::renew(server, key).map_err(|err| err.to_string())? else {
        return Ok(false);
    };
    // the old key still signs in for a few minutes; a new key that cannot be kept is revoked, not left in the device
    // list, and the user signs in again once the old one ends
    if let Err(err) = client.keys.set(id, &renewed) {
        if let Err(revoke) = moqui_client::login::sign_out(server, &renewed) {
            tracing::warn!("The new key of {id}, not kept, was not revoked: {revoke}");
        }
        return Err(format!("the new key was not kept: {err}"));
    }
    Ok(true)
}
