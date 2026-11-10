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

//! The Rust part of `org.moqui.chat.ChatKeyServices` (the rest is `ChatKeys.js`): `derive#ChatStoreKey`, the passphrase a
//! browser encrypts a crypto device's store with (docs/interfaces/Chat-Keys.md, "At rest"). It is derived, not stored:
//! HMAC-SHA-256 under the platform secret `chat_store_key_secret` (an `is-secret` default-property, generated when unset)
//! of `userId|deviceId|revision`, as 43 characters of URL-safe base64. So no table holds it: an administrator who reads
//! the entities in Tools, or an export, does not get it. A revoke bumps the device's revision.

use std::collections::BTreeMap;

use base64::Engine;
use hmac::{KeyInit, Mac};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use super::util::param_text;
use crate::entity::EntityFacade;

/// The platform secret the store keys are derived under.
const SECRET_PROPERTY: &str = "chat_store_key_secret";

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, derive_store_key)?;
    Ok(())
}

/// The store key of `user_id`'s device `device_id` at `revision`, under `secret`: 43 characters of URL-safe base64.
fn store_key(secret: &str, user_id: &str, device_id: &str, revision: i64) -> Option<String> {
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(format!("{user_id}|{device_id}|{revision}").as_bytes());
    Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

/// `derive#ChatStoreKey` (internal: `ChatKeys.js` answers it only to the device's own user): `storeKey`, or an error when
/// the secret is not set (a placeholder left as it is).
fn derive_store_key(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let secret = moqui_lib::env::get_prop_or_env(SECRET_PROPERTY);
    let secret = secret.trim();
    if secret.is_empty() || secret.ends_with("CHANGEME") {
        return Err(LibError::new(format!(
            "{SECRET_PROPERTY} is not set: no chat store key can be made"
        )));
    }
    let user_id = param_text(parameters, "userId");
    let device_id = param_text(parameters, "deviceId");
    if user_id.is_empty() || device_id.is_empty() {
        return Err(LibError::new("userId and deviceId are required"));
    }
    let revision = param_text(parameters, "revision")
        .trim()
        .parse::<i64>()
        .unwrap_or(1)
        .max(1);
    let key = store_key(secret, &user_id, &device_id, revision)
        .ok_or_else(|| LibError::new("the chat store key could not be made"))?;
    Ok([("storeKey".to_string(), Value::string(key))].into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::store_key;

    /// The key is 43 URL-safe characters, the same for the same inputs, and another for another user, device, revision
    /// or secret.
    #[test]
    fn a_store_key_is_stable_and_changes_with_each_input() {
        let key = store_key("s3cret", "ada", "ABCDEFGHIJ", 1).unwrap();
        assert_eq!(key.len(), 43);
        assert!(key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'));
        assert_eq!(store_key("s3cret", "ada", "ABCDEFGHIJ", 1).unwrap(), key);
        for other in [
            store_key("s3cret", "bob", "ABCDEFGHIJ", 1),
            store_key("s3cret", "ada", "ABCDEFGHIK", 1),
            store_key("s3cret", "ada", "ABCDEFGHIJ", 2),
            store_key("other", "ada", "ABCDEFGHIJ", 1),
        ] {
            assert_ne!(other.unwrap(), key);
        }
    }
}
