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

//! The app's push keys and what the server pushes, opened (plans/Comms-Messaging.md §7.2 rule 5, §7.4 "Device key"):
//! the server encrypts every push to the app (RFC 8291, the `aes128gcm` content coding of RFC 8188) for FCM as for
//! UnifiedPush, so Google sees only that a push came. Each account has a P-256 key pair and 16-byte auth secret of its
//! own, made at its first registration and kept as the device keys are, sealed by the Android Keystore
//! (`crate::keystore`, `KeyVault.kt`), under [`key_entry`]; their public half goes with the token to that account's
//! server (`registerClientPush`, `p256dhKey` and `authSecret`). So a push that opens with an account's keys is that
//! account's server's: two servers' registration ids may be the same, and a server cannot make a push that opens as
//! another account's. [`PushKeys::open`] is the receiving side of RFC 8291 §3.4, which the native code calls
//! (`Native.openPush`, and the UnifiedPush connector's key manager) before it shows a notification or rings.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Nonce};
use base64::Engine;
use hkdf::Hkdf;
use p256::{PublicKey, SecretKey};
use sha2::Sha256;

/// The suffix of an account's push keys entry in the key store (`<account>.push-keys`, beside `<account>` and
/// `<account>.chat-store`).
pub(crate) const KEY_SUFFIX: &str = "push-keys";

/// The longest push body taken (a push service takes 4096 bytes).
const MAX_BODY: usize = 4096;

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// base64url with or without padding, or the standard alphabet.
pub(crate) fn unb64(text: &str) -> Result<Vec<u8>, String> {
    let normalized: String = text
        .trim()
        .trim_end_matches('=')
        .chars()
        .map(|c| match c {
            '+' => '-',
            '/' => '_',
            other => other,
        })
        .collect();
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(normalized.as_bytes())
        .map_err(|err| format!("not base64url: {err}"))
}

/// The app's push key pair and auth secret.
pub(crate) struct PushKeys {
    secret: SecretKey,
    auth: [u8; 16],
}

impl PushKeys {
    /// A new pair and secret, from the system's random source.
    pub(crate) fn generate() -> Result<Self, String> {
        let mut auth = [0_u8; 16];
        getrandom::fill(&mut auth).map_err(|err| format!("no random bytes: {err}"))?;
        for _ in 0..4 {
            let mut seed = [0_u8; 32];
            getrandom::fill(&mut seed).map_err(|err| format!("no random bytes: {err}"))?;
            // a random 32 bytes is a valid scalar but for about one in 2^32
            if let Ok(secret) = SecretKey::from_slice(&seed) {
                return Ok(Self { secret, auth });
            }
        }
        Err("no push key could be made".to_string())
    }

    /// From what [`Self::to_text`] kept; none for text that is not one.
    pub(crate) fn from_text(text: &str) -> Option<Self> {
        let (secret, auth) = text.trim().split_once('.')?;
        let secret = SecretKey::from_slice(&unb64(secret).ok()?).ok()?;
        let auth: [u8; 16] = unb64(auth).ok()?.try_into().ok()?;
        Some(Self { secret, auth })
    }

    /// The private scalar and the auth secret, base64url, joined by a dot: what the key store keeps sealed.
    pub(crate) fn to_text(&self) -> String {
        format!("{}.{}", b64(&self.secret.to_bytes()), b64(&self.auth))
    }

    /// The public key a server encrypts to (`p256dhKey`): the uncompressed point, base64url.
    pub(crate) fn public_b64(&self) -> String {
        b64(&self.secret.public_key().to_sec1_bytes())
    }

    /// The auth secret (`authSecret`), base64url.
    pub(crate) fn auth_b64(&self) -> String {
        b64(&self.auth)
    }

    /// Open an `aes128gcm` body encrypted to these keys (RFC 8291 §3.4, one record): the plaintext.
    pub(crate) fn open(&self, body: &[u8]) -> Result<Vec<u8>, String> {
        if body.len() > MAX_BODY || body.len() < 21 {
            return Err("not a push body".to_string());
        }
        let (salt, rest) = body.split_at(16);
        let (_record_size, rest) = rest.split_at(4);
        let (&key_length, rest) = rest.split_first().ok_or("no key length")?;
        let key_length = usize::from(key_length);
        let (as_public_bytes, ciphertext) = rest.split_at_checked(key_length).ok_or("a short push body")?;
        let as_public =
            PublicKey::from_sec1_bytes(as_public_bytes).map_err(|_| "the sender's key is not a P-256 point")?;
        let shared = p256::ecdh::diffie_hellman(self.secret.to_nonzero_scalar(), as_public.as_affine());
        let mut key_info = b"WebPush: info\0".to_vec();
        key_info.extend_from_slice(&self.secret.public_key().to_sec1_bytes());
        key_info.extend_from_slice(as_public_bytes);
        let mut ikm = [0_u8; 32];
        Hkdf::<Sha256>::new(Some(&self.auth), shared.raw_secret_bytes())
            .expand(&key_info, &mut ikm)
            .map_err(|_| "HKDF failed")?;
        let prk = Hkdf::<Sha256>::new(Some(salt), &ikm);
        let mut cek = [0_u8; 16];
        let mut nonce = [0_u8; 12];
        prk.expand(b"Content-Encoding: aes128gcm\0", &mut cek)
            .map_err(|_| "HKDF failed")?;
        prk.expand(b"Content-Encoding: nonce\0", &mut nonce)
            .map_err(|_| "HKDF failed")?;
        let cipher = Aes128Gcm::new_from_slice(&cek).map_err(|_| "bad content key")?;
        let nonce = Nonce::try_from(&nonce[..]).map_err(|_| "bad nonce")?;
        let mut plain = cipher
            .decrypt(&nonce, ciphertext)
            .map_err(|_| "the push does not open with this device's key".to_string())?;
        // the last record: its padding ends at the 0x02 delimiter
        while plain.last() == Some(&0) {
            plain.pop();
        }
        if plain.pop() != Some(2) {
            return Err("the push's padding is not the last record's".to_string());
        }
        Ok(plain)
    }

    /// Open a push's `enc` (base64url): its JSON object.
    pub(crate) fn open_b64(&self, enc: &str) -> Result<serde_json::Value, String> {
        let plain = self.open(&unb64(enc)?)?;
        match serde_json::from_slice::<serde_json::Value>(&plain) {
            Ok(value) if value.is_object() => Ok(value),
            _ => Err("the push is not a JSON object".to_string()),
        }
    }
}

/// The sending side of RFC 8291, as a server seals to these keys (the tests' server): one `aes128gcm` record.
#[cfg(test)]
pub(crate) fn seal(to: &PushKeys, plain: &[u8]) -> Vec<u8> {
    let sender = PushKeys::generate().unwrap();
    let ua_public = to.secret.public_key().to_sec1_bytes();
    let as_public = sender.secret.public_key().to_sec1_bytes();
    let shared = p256::ecdh::diffie_hellman(sender.secret.to_nonzero_scalar(), to.secret.public_key().as_affine());
    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend_from_slice(&ua_public);
    key_info.extend_from_slice(&as_public);
    let mut ikm = [0_u8; 32];
    Hkdf::<Sha256>::new(Some(&to.auth), shared.raw_secret_bytes())
        .expand(&key_info, &mut ikm)
        .unwrap();
    let salt = [9_u8; 16];
    let prk = Hkdf::<Sha256>::new(Some(&salt), &ikm);
    let (mut cek, mut nonce) = ([0_u8; 16], [0_u8; 12]);
    prk.expand(b"Content-Encoding: aes128gcm\0", &mut cek).unwrap();
    prk.expand(b"Content-Encoding: nonce\0", &mut nonce).unwrap();
    let mut padded = plain.to_vec();
    padded.push(2);
    let cipher = Aes128Gcm::new_from_slice(&cek).unwrap();
    let sealed = cipher
        .encrypt(&Nonce::try_from(&nonce[..]).unwrap(), padded.as_slice())
        .unwrap();
    let mut body = salt.to_vec();
    body.extend_from_slice(&4096_u32.to_be_bytes());
    body.push(u8::try_from(as_public.len()).unwrap());
    body.extend_from_slice(&as_public);
    body.extend_from_slice(&sealed);
    body
}

/// The key store entry of `account`'s push keys: an account id is a host and a hex digest, so this never names another
/// account's device key (as `<account>.chat-store`).
pub(crate) fn key_entry(account: &str) -> String {
    format!("{account}.{KEY_SUFFIX}")
}

/// `account`'s push keys, read only: none when it has none or they cannot be read now. Opening a push never makes keys,
/// so a store that fails for a moment never replaces the keys a server encrypts to.
pub(crate) fn kept_keys(keys: &moqui_client::keys::Keys, account: &str) -> Option<PushKeys> {
    keys.get(&key_entry(account)).as_deref().and_then(PushKeys::from_text)
}

/// `account`'s push keys for its registration: the kept ones, else new ones made and kept, but only when none were
/// registered (`registered_public`, the public key its registration was made with): keys that were registered and
/// cannot be read now are an error, tried again later, never replaced.
pub(crate) fn keys_for(
    keys: &moqui_client::keys::Keys,
    account: &str,
    registered_public: Option<&str>,
) -> Result<PushKeys, String> {
    if let Some(kept) = kept_keys(keys, account) {
        return Ok(kept);
    }
    if registered_public.is_some() {
        return Err(format!("the push keys of {account} cannot be read now"));
    }
    let made = PushKeys::generate()?;
    keys.set(&key_entry(account), &made.to_text())
        .map_err(|err| format!("the push keys of {account} were not kept: {err}"))?;
    log::info!("Push: push keys were made for {account}");
    Ok(made)
}

/// Forget `account`'s push keys (it signed out here); a failure is logged.
pub(crate) fn forget_keys(keys: &moqui_client::keys::Keys, account: &str) {
    if let Err(err) = keys.delete(&key_entry(account)) {
        log::warn!("Push: the push keys of {account} were not deleted: {err}");
    }
}

#[cfg(test)]
#[path = "push_tests.rs"]
mod tests;
