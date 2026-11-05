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

//! The client side of a remote server's password sign-in for a login key, with the second factor's code: the REST
//! `login` with `createLoginKey=true` (and a `deviceName` for a key in the user's device list), its answer decoded,
//! and `sendOtp` in the session a pre-authenticating `login` opened, with its cookie and CSRF token. `moqui tui
//! --server` (`moqui-host`'s `tui_remote`) and the desktop and mobile client (`moqui-client`'s `login`) both sign in
//! through it, each with its own HTTP client and so its own TLS policy, and each keeps the key its own way.
//!
//! The answer's decoding needs no HTTP; the requests are under the `http` feature, on a caller's
//! `reqwest::blocking::Client`.

use crate::error::Error;

/// What a sign-in came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignIn {
    /// Signed in: the key for the `login_key` header.
    Key(String),
    /// The account needs a second factor: ask for its code (and offer to send one), then sign in again with it.
    Code(CodeAsk),
}

/// The second factor the server asks for: why (the kinds of code, or that the one given was not valid), and the
/// factors a code can be sent to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodeAsk {
    /// What to show.
    pub text: String,
    /// Factors the server can send a code to (email, SMS).
    pub sendable: Vec<Sendable>,
}

/// A factor the server sends a code to: its id, and what to show (the masked address or number).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sendable {
    pub id: String,
    pub label: String,
}

/// The refusal of a sign-in the server answered as done but with no key: a client that keeps only the key cannot
/// stay signed in, so it is an error and not a session.
pub const NO_KEY: &str = "the server signed in but made no login key";

/// The answer of `/rest/login` (its status and JSON, `null` when it is not JSON) as a [`SignIn`]. `code_given` says a
/// code went with the password, so a second ask means it was not valid.
///
/// # Errors
///
/// Returns the server's reason for a refused sign-in, or [`NO_KEY`] for a sign-in with no key.
pub fn signed_in(status: u16, value: &serde_json::Value, code_given: bool) -> Result<SignIn, Error> {
    if status < 400 && value.get("loggedIn").and_then(serde_json::Value::as_bool) == Some(true) {
        let key = value.get("loginKey").and_then(serde_json::Value::as_str).unwrap_or("");
        if key.is_empty() {
            return Err(Error::new(NO_KEY));
        }
        return Ok(SignIn::Key(key.to_string()));
    }
    if status < 400 && value.get("secondFactorRequired").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(SignIn::Code(code_ask(value, code_given)));
    }
    Err(Error::new(reason(value, status)))
}

/// The text of an error answer: `errors`, `error` or `message`, else the status.
#[must_use]
pub fn reason(value: &serde_json::Value, status: u16) -> String {
    ["errors", "error", "message"]
        .iter()
        .find_map(|name| value.get(*name).and_then(serde_json::Value::as_str))
        .filter(|text| !text.is_empty())
        .map_or_else(|| format!("HTTP {status}"), str::to_string)
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

/// Whether a `sendOtp` answer was a refusal: its `errors` or `error` text, else the status of a failed request.
#[must_use]
pub fn send_refusal(status: u16, value: &serde_json::Value) -> Option<String> {
    let errors = ["errors", "error"]
        .iter()
        .find_map(|name| value.get(*name).and_then(serde_json::Value::as_str))
        .filter(|text| !text.is_empty());
    match errors {
        Some(text) => Some(text.to_string()),
        None if status >= 400 => Some(format!("HTTP {status}")),
        None => None,
    }
}

#[cfg(feature = "http")]
pub use self::requests::{form_body, post_form, send_code, sign_in};

#[cfg(feature = "http")]
mod requests {
    use super::{send_refusal, signed_in, SignIn};
    use crate::error::Error;

    /// Sign in to `base` (`https://host:port[/path]`, no trailing slash) as `username`. With `device` the key is that
    /// device's, shown in the user's device list, where the user can revoke it; `code` is the second factor's, or
    /// empty.
    ///
    /// # Errors
    ///
    /// Returns the server's reason for a refused sign-in, [`super::NO_KEY`] for a sign-in with no key, or the
    /// transport's error for a server that did not answer.
    pub fn sign_in(
        http: &reqwest::blocking::Client,
        base: &str,
        username: &str,
        password: &str,
        code: &str,
        device: Option<&str>,
    ) -> Result<SignIn, Error> {
        let mut form = vec![
            ("username", username),
            ("password", password),
            ("createLoginKey", "true"),
        ];
        if let Some(device) = device.filter(|device| !device.is_empty()) {
            form.push(("deviceName", device));
        }
        if !code.is_empty() {
            form.push(("code", code));
        }
        let (status, value) = post_form(http, &format!("{base}/rest/login"), &form, &[])?;
        signed_in(status, &value, !code.is_empty())
    }

    /// Have the server send a code to `factor`: `/rest/login` to pre-authenticate, then `sendOtp` in that session,
    /// with its cookie and its CSRF token.
    ///
    /// # Errors
    ///
    /// Returns the server's reason, or the transport's error.
    pub fn send_code(
        http: &reqwest::blocking::Client,
        base: &str,
        username: &str,
        password: &str,
        factor: &str,
    ) -> Result<(), Error> {
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
        let cookie = response
            .headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter_map(|cookie| cookie.split(';').next())
            .collect::<Vec<_>>()
            .join("; ");
        let token = response
            .headers()
            .get("x-csrf-token")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        let (status, value) = post_form(
            http,
            &format!("{base}/rest/sendOtp"),
            &[("factorId", factor)],
            &[("Cookie", cookie.as_str()), ("X-CSRF-Token", token.as_str())],
        )?;
        match send_refusal(status, &value) {
            Some(text) => Err(Error::new(text)),
            None => Ok(()),
        }
    }

    /// POST a form for JSON, with `headers` added: the status and the body, `null` when it is not JSON.
    ///
    /// # Errors
    ///
    /// Returns the transport's error, with the url.
    pub fn post_form(
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

    /// `a=1&b=x%20y`, as a form posts it.
    #[must_use]
    pub fn form_body<K: AsRef<str>, V: AsRef<str>>(pairs: &[(K, V)]) -> String {
        let mut url = reqwest::Url::parse("http://form/").unwrap_or_else(|_| unreachable!("a fixed url parses"));
        url.query_pairs_mut()
            .extend_pairs(pairs.iter().map(|(key, value)| (key.as_ref(), value.as_ref())));
        url.query().unwrap_or("").to_string()
    }
}

#[cfg(test)]
#[path = "remote_login_tests.rs"]
mod tests;
