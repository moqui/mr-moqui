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

//! The server's record of a device memory (`org.moqui.security.VaultServices`, `/rest/s1/vault/deviceMemories`): made
//! when the user asks the device to remember the key, read before the device opens the key from memory, and ended when
//! the device forgets it. The user's Vault key settings list the devices that remember the key, and one taken off the
//! list there, or a device key the server no longer takes, is a memory that does not open again.

use moqui_lib::error::Error;
use serde_json::{json, Value};

/// What the server said, or why it did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerError {
    /// 401: the server no longer takes the account's device key (signed out elsewhere, revoked, run out).
    Revoked,
    /// The server could not be reached.
    Unreachable(String),
    /// The server refused, with its reason.
    Refused(String),
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Revoked => f.write_str("the server no longer takes this device's key"),
            Self::Unreachable(why) => write!(f, "the server could not be reached: {why}"),
            Self::Refused(why) => write!(f, "the server refused: {why}"),
        }
    }
}

/// The server's side of device memory, for an account.
pub trait MemoryServer {
    /// Record the memory `memory_id` of this device, named `device_name`, for `minutes`; when the server says it ends
    /// (epoch millis), which may be sooner than asked (its `vault@device-memory-max-hours`).
    ///
    /// # Errors
    ///
    /// Returns why it was not recorded.
    fn register(&self, memory_id: &str, device_name: &str, minutes: u32) -> Result<i64, ServerError>;

    /// Whether the memory is still on the server's list.
    ///
    /// # Errors
    ///
    /// [`ServerError::Revoked`] when the device key is no longer taken, else why the server did not answer.
    fn check(&self, memory_id: &str) -> Result<bool, ServerError>;

    /// End the memory on the server.
    ///
    /// # Errors
    ///
    /// Returns why it did not.
    fn forget(&self, memory_id: &str) -> Result<(), ServerError>;
}

/// The vault REST of the account's server, with its device key.
pub struct RestMemoryServer<'a> {
    /// The server's base url.
    pub server: &'a str,
    pub key: &'a str,
}

impl RestMemoryServer<'_> {
    fn url(&self, memory_id: Option<&str>) -> String {
        let base = format!("{}/rest/s1/vault/deviceMemories", self.server.trim_end_matches('/'));
        match memory_id {
            Some(id) => format!(
                "{base}/{}",
                url::form_urlencoded::byte_serialize(id.as_bytes()).collect::<String>()
            ),
            None => base,
        }
    }

    fn send(&self, request: reqwest::blocking::RequestBuilder) -> Result<Value, ServerError> {
        let response = request
            .header("Accept", "application/json")
            .header("login_key", self.key)
            .send()
            .map_err(|err| ServerError::Unreachable(crate::http::transport(err)))?;
        let status = response.status().as_u16();
        if status == 401 {
            return Err(ServerError::Revoked);
        }
        let text = response.text().unwrap_or_default();
        let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if status >= 400 {
            let reason = body["errors"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("HTTP {status}"));
            return Err(ServerError::Refused(reason));
        }
        Ok(body)
    }

    fn client() -> Result<reqwest::blocking::Client, ServerError> {
        crate::http::client().map_err(|err: Error| ServerError::Unreachable(err.to_string()))
    }
}

impl MemoryServer for RestMemoryServer<'_> {
    fn register(&self, memory_id: &str, device_name: &str, minutes: u32) -> Result<i64, ServerError> {
        let body = json!({ "deviceMemoryId": memory_id, "deviceName": device_name, "minutes": minutes });
        let request = Self::client()?
            .post(self.url(None))
            .header("Content-Type", "application/json")
            .body(body.to_string());
        let answer = self.send(request)?;
        answer["thruStamp"]
            .as_i64()
            .ok_or_else(|| ServerError::Refused("the server did not say when the memory ends".to_string()))
    }

    fn check(&self, memory_id: &str) -> Result<bool, ServerError> {
        let answer = self.send(Self::client()?.get(self.url(Some(memory_id))))?;
        Ok(answer["live"].as_bool() == Some(true))
    }

    fn forget(&self, memory_id: &str) -> Result<(), ServerError> {
        self.send(Self::client()?.delete(self.url(Some(memory_id)))).map(|_| ())
    }
}
