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

//! Password sign-in for a device key: the REST `login` with `createLoginKey=true` and the device's name, as
//! `moqui tui --server` signs in, with the second factor's code when the account needs one. The password is posted
//! once and only the key is kept. Sign-out revokes the key on the server before the host forgets it.

use moqui_lib::error::Error;

use crate::http::{client, form_body, reason};

/// What a sign-in came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignIn {
    /// Signed in: the device key for the `login_key` header.
    Key(String),
    /// The account needs a second factor: ask for its code (and offer to send one), then sign in again with it.
    Code(CodeAsk),
}

/// The second factor the server asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeAsk {
    /// What to show: the kinds of code, or that the one given was not valid.
    pub text: String,
    /// Factors the server can send a code to (email, SMS).
    pub sendable: Vec<Sendable>,
}

/// A factor a code can be sent to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sendable {
    pub id: String,
    pub label: String,
}

/// Sign in to `base` as `username`, for a key named `device` (shown in the user's device list).
///
/// # Errors
///
/// Returns the server's reason for a refused sign-in, or the transport's for a server that did not answer.
pub fn sign_in(base: &str, username: &str, password: &str, code: &str, device: &str) -> Result<SignIn, Error> {
    let mut form = vec![
        ("username", username),
        ("password", password),
        ("createLoginKey", "true"),
        ("deviceName", device),
    ];
    if !code.is_empty() {
        form.push(("code", code));
    }
    let (status, value) = post_json(&client()?, &format!("{base}/rest/login"), &form, &[])?;
    signed_in(status, &value, !code.is_empty())
}

/// The answer of `/rest/login` as a [`SignIn`].
fn signed_in(status: u16, value: &serde_json::Value, code_given: bool) -> Result<SignIn, Error> {
    let key = value.get("loginKey").and_then(serde_json::Value::as_str).unwrap_or("");
    if status < 400 && value.get("loggedIn").and_then(serde_json::Value::as_bool) == Some(true) {
        if key.is_empty() {
            return Err(Error::new("the server signed in but made no device key"));
        }
        return Ok(SignIn::Key(key.to_string()));
    }
    if status < 400 && value.get("secondFactorRequired").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(SignIn::Code(code_ask(value, code_given)));
    }
    Err(Error::new(reason(value, status)))
}

/// Have the server send a code to `factor`: `/rest/login` to pre-authenticate, then `sendOtp` in that session, with
/// its cookie and its CSRF token.
///
/// # Errors
///
/// Returns the server's reason, or the transport's.
pub fn send_code(base: &str, username: &str, password: &str, factor: &str) -> Result<(), Error> {
    let http = client()?;
    let url = format!("{base}/rest/login");
    let response = http
        .post(&url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_body(&[("username", username), ("password", password)]))
        .send()
        .map_err(|err| Error::new(format!("{url}: {err}")))?;
    let status = response.status().as_u16();
    if status >= 400 {
        return Err(Error::new(format!("HTTP {status}")));
    }
    let cookies: Vec<String> = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|cookie| cookie.split(';').next())
        .map(str::to_string)
        .collect();
    let token = response
        .headers()
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let cookie = cookies.join("; ");
    let (status, value) = post_json(
        &http,
        &format!("{base}/rest/sendOtp"),
        &[("factorId", factor)],
        &[("Cookie", cookie.as_str()), ("X-CSRF-Token", token.as_str())],
    )?;
    let errors = ["errors", "error"]
        .iter()
        .find_map(|name| value.get(*name).and_then(serde_json::Value::as_str))
        .filter(|text| !text.is_empty());
    match errors {
        Some(text) => Err(Error::new(text)),
        None if status >= 400 => Err(Error::new(format!("HTTP {status}"))),
        None => Ok(()),
    }
}

/// Revoke the device key on the server (`/rest/logout` with `revokeLoginKey=true`). The host forgets the key whether
/// or not this succeeds; a server that cannot be reached keeps the key until it expires or the user revokes it from
/// another device.
///
/// # Errors
///
/// Returns the transport's error, or the status of a refused logout.
pub fn sign_out(base: &str, key: &str) -> Result<(), Error> {
    let url = format!("{base}/rest/logout");
    let response = client()?
        .post(&url)
        .header("login_key", key)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_body(&[("revokeLoginKey", "true")]))
        .send()
        .map_err(|err| Error::new(format!("{url}: {err}")))?;
    let status = response.status().as_u16();
    if status >= 400 && status != 401 {
        return Err(Error::new(format!("{url}: HTTP {status}")));
    }
    Ok(())
}

/// The server's client config for the user the key signs in (`/qapps/clientConfig`): the user, the server's name and
/// the shell's settings. `Ok(None)` when the key is no longer valid (401).
///
/// # Errors
///
/// Returns the transport's error, or the status of another refusal.
pub fn client_config(base: &str, key: &str) -> Result<Option<serde_json::Value>, Error> {
    let url = format!("{base}/qapps/clientConfig");
    let response = client()?
        .get(&url)
        .header("Accept", "application/json")
        .header("login_key", key)
        .send()
        .map_err(|err| Error::new(format!("{url}: {err}")))?;
    let status = response.status().as_u16();
    if status == 401 {
        return Ok(None);
    }
    let text = response.text().unwrap_or_default();
    if status >= 400 {
        return Err(Error::new(format!("{url}: HTTP {status}")));
    }
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|err| Error::new(format!("{url}: {err}")))
}

/// The newest `server.clientApi` of `/qapps/clientConfig` this client knows: 1 the first, 2 added `renewClientDevice`.
/// A server that sends a newer one may expect what this client does not do, so the shell asks for an update.
pub const CLIENT_API: u64 = 2;

/// The server's `clientApi` in its client config, 1 when it sends none.
#[must_use]
pub fn server_client_api(config: &serde_json::Value) -> u64 {
    config
        .pointer("/server/clientApi")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1)
}

/// The lifetime of this device's key, from the user's device list (`/qapps/clientDevices`, the row marked
/// `current`): its `fromDate` and `thruDate` in milliseconds. `Ok(None)` when the key is no longer valid (401) or
/// the list has no such row.
///
/// # Errors
///
/// Returns the transport's error, or the status of another refusal.
pub fn key_period(base: &str, key: &str) -> Result<Option<(i64, i64)>, Error> {
    let url = format!("{base}/qapps/clientDevices");
    let response = client()?
        .get(&url)
        .header("Accept", "application/json")
        .header("login_key", key)
        .send()
        .map_err(|err| Error::new(format!("{url}: {err}")))?;
    let status = response.status().as_u16();
    if status == 401 {
        return Ok(None);
    }
    if status >= 400 {
        return Err(Error::new(format!("{url}: HTTP {status}")));
    }
    let value: serde_json::Value = serde_json::from_str(&response.text().unwrap_or_default())
        .map_err(|err| Error::new(format!("{url}: {err}")))?;
    Ok(current_period(&value))
}

/// The `current` row's `fromDate` and `thruDate` in a `clientDevices` answer.
fn current_period(value: &serde_json::Value) -> Option<(i64, i64)> {
    let row = value
        .get("deviceList")?
        .as_array()?
        .iter()
        .find(|row| row.get("current").and_then(serde_json::Value::as_bool) == Some(true))?;
    let millis = |name: &str| match row.get(name)? {
        serde_json::Value::Number(number) => number.as_i64(),
        serde_json::Value::String(text) => text.parse().ok(),
        _ => None,
    };
    Some((millis("fromDate")?, millis("thruDate")?))
}

/// Whether a key good from `from` to `thru` is renewed at `now`: when less than a third of its life is left.
#[must_use]
pub fn renew_due(from: i64, thru: i64, now: i64) -> bool {
    thru.saturating_sub(now) < thru.saturating_sub(from) / 3
}

/// A new key for this device (`/qapps/renewClientDevice`), with the same name and a full lifetime; the old key ends
/// a few minutes later. `Ok(None)` when the key is no longer valid (401).
///
/// # Errors
///
/// Returns the transport's error, or the reason of a refusal (a server older than `clientApi` 2 has no such page).
pub fn renew(base: &str, key: &str) -> Result<Option<String>, Error> {
    let url = format!("{base}/qapps/renewClientDevice");
    let (status, value) = post_json(&client()?, &url, &[], &[("login_key", key)])?;
    if status == 401 {
        return Ok(None);
    }
    match value.get("loginKey").and_then(serde_json::Value::as_str) {
        Some(renewed) if status < 400 && !renewed.is_empty() => Ok(Some(renewed.to_string())),
        _ => Err(Error::new(format!("{url}: HTTP {status}"))),
    }
}

/// POST a form for JSON; a body that is not JSON is `null`.
fn post_json(
    http: &reqwest::blocking::Client,
    url: &str,
    form: &[(&str, &str)],
    headers: &[(&str, &str)],
) -> Result<(u16, serde_json::Value), Error> {
    let mut request = http
        .post(url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = request
        .body(form_body(form))
        .send()
        .map_err(|err| Error::new(format!("{url}: {err}")))?;
    let status = response.status().as_u16();
    let value = serde_json::from_str(&response.text().unwrap_or_default()).unwrap_or(serde_json::Value::Null);
    Ok((status, value))
}

/// The factor info `/rest/login` answers for an account that needs a code.
fn code_ask(value: &serde_json::Value, code_given: bool) -> CodeAsk {
    let kinds: Vec<&str> = value
        .get("factorTypeDescriptions")
        .and_then(serde_json::Value::as_array)
        .map(|list| list.iter().filter_map(serde_json::Value::as_str).collect())
        .unwrap_or_default();
    let sendable = value
        .get("sendableFactors")
        .and_then(serde_json::Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|factor| {
                    let id = factor.get("factorId").and_then(serde_json::Value::as_str)?;
                    let label = factor
                        .get("factorOption")
                        .and_then(serde_json::Value::as_str)
                        .filter(|text| !text.is_empty())
                        .unwrap_or(id);
                    Some(Sendable {
                        id: id.to_string(),
                        label: label.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let text = if code_given {
        "Authentication code is not valid".to_string()
    } else if kinds.is_empty() {
        "Enter the authentication code".to_string()
    } else {
        format!("Enter the authentication code ({})", kinds.join(", "))
    };
    CodeAsk { text, sendable }
}

#[cfg(test)]
#[path = "login_tests.rs"]
mod tests;
