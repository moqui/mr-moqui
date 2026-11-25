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

//! `moqui vault`'s requests: one cookie session with a server, signed in by the REST login with the password and,
//! when the account has one, the second factor's code, so the session is a strong, recent sign-in and the server
//! sends the wrapped personal key (plans/archive/Vault-Instances.md §9). Nothing is kept after the run: no login key, no
//! cookie. Plain `http://` is refused except to this machine.

use std::collections::BTreeMap;
use std::time::Duration;

use moqui_lib::error::Error;
use serde_json::Value;

/// What a login answered.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Login {
    SignedIn,
    /// The account has a second factor: send the password again with its code.
    CodeNeeded(String),
}

/// A cookie session with one server.
pub(crate) struct Remote {
    base: String,
    http: reqwest::blocking::Client,
    cookies: BTreeMap<String, String>,
    token: Option<String>,
}

/// `https://host[:port][/path]` from what the user typed; plain http only to this machine.
pub(crate) fn base_url(server: &str) -> Result<String, Error> {
    let url = reqwest::Url::parse(server.trim()).map_err(|err| Error::new(format!("--server {server}: {err}")))?;
    let host = url.host_str().unwrap_or("");
    match url.scheme() {
        "https" => {}
        "http" if matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1") => {}
        "http" => {
            return Err(Error::new(format!(
                "--server {server}: plain http sends the password over the network; use https, or http to localhost"
            )))
        }
        other => return Err(Error::new(format!("--server {server}: {other} is not http or https"))),
    }
    let mut base = format!("{}://{host}", url.scheme());
    if let Some(port) = url.port() {
        base.push_str(&format!(":{port}"));
    }
    base.push_str(url.path().trim_end_matches('/'));
    Ok(base)
}

impl Remote {
    pub(crate) fn new(server: &str) -> Result<Remote, Error> {
        let http = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|err| Error::new(format!("HTTP client: {err}")))?;
        Ok(Remote {
            base: base_url(server)?,
            http,
            cookies: BTreeMap::new(),
            token: None,
        })
    }

    pub(crate) fn base(&self) -> &str {
        &self.base
    }

    /// The REST login in this session, with the second factor's code when given.
    pub(crate) fn login(&mut self, username: &str, password: &str, code: &str) -> Result<Login, Error> {
        let mut body = serde_json::json!({ "username": username, "password": password });
        if !code.is_empty() {
            body["code"] = Value::String(code.to_string());
        }
        let request = self.http.post(format!("{}/rest/login", self.base)).json(&body);
        let (status, value) = self.send(request)?;
        if status < 400 && value["loggedIn"] == Value::Bool(true) {
            return Ok(Login::SignedIn);
        }
        if status < 400 && value["secondFactorRequired"] == Value::Bool(true) {
            let ask = if code.is_empty() {
                "Authentication code"
            } else {
                "Authentication code is not valid; another"
            };
            return Ok(Login::CodeNeeded(ask.to_string()));
        }
        Err(Error::new(moqui_lib::remote_login::reason(&value, status)))
    }

    /// GET a vault REST path (under `/rest/s1/vault`), with query parameters.
    pub(crate) fn get(&mut self, path: &str, query: &[(&str, &str)]) -> Result<Value, Error> {
        let url = reqwest::Url::parse_with_params(&format!("{}/rest/s1/vault{path}", self.base), query)
            .map_err(|err| Error::new(format!("{}{path}: {err}", self.base)))?;
        let request = self.http.get(url);
        self.answer(request)
    }

    /// POST a JSON body to a vault REST path.
    pub(crate) fn post(&mut self, path: &str, body: &Value) -> Result<Value, Error> {
        let request = self.http.post(format!("{}/rest/s1/vault{path}", self.base)).json(body);
        self.answer(request)
    }

    fn answer(&mut self, request: reqwest::blocking::RequestBuilder) -> Result<Value, Error> {
        let (status, value) = self.send(request)?;
        match moqui_lib::remote_login::send_refusal(status, &value) {
            Some(refused) => Err(Error::new(refused)),
            None => Ok(value),
        }
    }

    /// Sends with this session's cookies and token, and takes the answer's.
    fn send(&mut self, mut request: reqwest::blocking::RequestBuilder) -> Result<(u16, Value), Error> {
        request = request.header("Accept", "application/json");
        if !self.cookies.is_empty() {
            let cookie: Vec<String> = self
                .cookies
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect();
            request = request.header("Cookie", cookie.join("; "));
        }
        if let Some(token) = &self.token {
            request = request.header("X-CSRF-Token", token);
        }
        let response = request
            .send()
            .map_err(|err| Error::new(format!("{}: {err}", self.base)))?;
        let status = response.status().as_u16();
        for set in response.headers().get_all("set-cookie") {
            let Some((name, rest)) = set.to_str().unwrap_or("").split_once('=') else {
                continue;
            };
            let value = rest.split(';').next().unwrap_or("").to_string();
            self.cookies.insert(name.trim().to_string(), value);
        }
        for name in ["moquiSessionToken", "X-CSRF-Token"] {
            if let Some(token) = response.headers().get(name).and_then(|value| value.to_str().ok()) {
                self.token = Some(token.to_string());
            }
        }
        let text = response
            .text()
            .map_err(|err| Error::new(format!("{}: {err}", self.base)))?;
        let value = serde_json::from_str(&text).unwrap_or(Value::Null);
        Ok((status, value))
    }
}
