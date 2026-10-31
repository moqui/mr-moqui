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

//! Web Push message encryption (RFC 8291, the `aes128gcm` content coding of RFC 8188) and VAPID (RFC 8292), on
//! RustCrypto: P-256 ECDH and ES256 from `p256`, HKDF-SHA-256 and AES-128-GCM. The `web-push` feature; without it
//! `push_crypto_off.rs` stands in, with the same API, and reports that push is not built.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Nonce};
use base64::Engine;
use hkdf::Hkdf;
use p256::ecdsa::signature::Signer;
use p256::{PublicKey, SecretKey};
use sha2::Sha256;

/// Web Push is built into this host.
pub(crate) const BUILT: bool = true;

/// The record size in the `aes128gcm` header. One record holds the whole message, which is far smaller.
const RECORD_SIZE: u32 = 4096;
/// The longest payload: a push service takes 4096 bytes of body, less the 86-byte header, the padding delimiter and
/// the 16-byte tag.
pub(crate) const MAX_PAYLOAD: usize = 3993;
/// A VAPID token lives at most 24 hours (RFC 8292 §2); this one 12.
const TOKEN_SECONDS: u64 = 12 * 3600;

/// Decode base64url with or without padding; also accept the standard alphabet, as some clients send it.
pub(crate) fn decode_b64(text: &str) -> Result<Vec<u8>, String> {
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

pub(crate) fn encode_b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The application server's VAPID key pair.
pub(crate) struct Vapid {
    secret: SecretKey,
    public_b64: String,
}

impl Vapid {
    /// The pair from the 32-byte private scalar, in base64 (the generated `web_push_vapid_key` property).
    pub(crate) fn from_seed(seed: &str) -> Result<Self, String> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(seed.trim())
            .or_else(|_| decode_b64(seed))
            .map_err(|_| "the VAPID key is not base64".to_string())?;
        let secret =
            SecretKey::from_slice(&bytes).map_err(|_| "the VAPID key is not a P-256 private key".to_string())?;
        let public_b64 = encode_b64(&secret.public_key().to_sec1_bytes());
        Ok(Self { secret, public_b64 })
    }

    /// The public key the browser subscribes with (`applicationServerKey`): the uncompressed point, base64url.
    pub(crate) fn public_key(&self) -> &str {
        &self.public_b64
    }

    /// The `Authorization` header for a push to `endpoint`: `vapid t=<ES256 JWT>, k=<public key>`, the audience the
    /// endpoint's origin.
    pub(crate) fn authorization(&self, endpoint: &str, subject: &str, now_secs: u64) -> Result<String, String> {
        let audience = origin(endpoint)?;
        let header = encode_b64(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = serde_json::json!({
            "aud": audience,
            "exp": now_secs + TOKEN_SECONDS,
            "sub": subject,
        });
        let claims = encode_b64(claims.to_string().as_bytes());
        let signing_input = format!("{header}.{claims}");
        let key = p256::ecdsa::SigningKey::from(&self.secret);
        let signature: p256::ecdsa::Signature = key.sign(signing_input.as_bytes());
        let token = format!("{signing_input}.{}", encode_b64(&signature.to_bytes()));
        Ok(format!("vapid t={token}, k={}", self.public_b64))
    }
}

/// `https://host[:port]` of an https URL; an http URL only for a loopback host (a local test push service).
pub(crate) fn origin(endpoint: &str) -> Result<String, String> {
    let (scheme, rest) = endpoint
        .split_once("://")
        .ok_or_else(|| "the push endpoint is not a URL".to_string())?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return Err("the push endpoint has no host".to_string());
    }
    let host = authority.rsplit_once(':').map_or(authority, |(host, _)| host);
    let loopback = host == "localhost" || host == "127.0.0.1" || host == "[::1]";
    match scheme.to_ascii_lowercase().as_str() {
        "https" => Ok(format!("https://{}", authority.to_ascii_lowercase())),
        "http" if loopback => Ok(format!("http://{}", authority.to_ascii_lowercase())),
        _ => Err("the push endpoint must be an https URL".to_string()),
    }
}

/// Check a subscription's keys: a P-256 public point and a 16-byte auth secret.
pub(crate) fn check_keys(p256dh: &str, auth: &str) -> Result<(), String> {
    let point = decode_b64(p256dh).map_err(|_| "p256dh is not base64url".to_string())?;
    PublicKey::from_sec1_bytes(&point).map_err(|_| "p256dh is not a P-256 public key".to_string())?;
    let secret = decode_b64(auth).map_err(|_| "auth is not base64url".to_string())?;
    if secret.len() != 16 {
        return Err("auth is not 16 bytes".to_string());
    }
    Ok(())
}

/// Encrypt `payload` for the subscription's keys, with a fresh sender key and salt: the request body.
pub(crate) fn encrypt(payload: &[u8], p256dh: &str, auth: &str) -> Result<Vec<u8>, String> {
    let mut seed = [0_u8; 32];
    let mut salt = [0_u8; 16];
    getrandom::fill(&mut seed).map_err(|err| format!("no random bytes: {err}"))?;
    getrandom::fill(&mut salt).map_err(|err| format!("no random bytes: {err}"))?;
    // A random 32 bytes is a valid scalar but for about one in 2^32; draw again then.
    let sender = match SecretKey::from_slice(&seed) {
        Ok(key) => key,
        Err(_) => {
            getrandom::fill(&mut seed).map_err(|err| format!("no random bytes: {err}"))?;
            SecretKey::from_slice(&seed).map_err(|_| "could not make a sender key".to_string())?
        }
    };
    encrypt_with(payload, p256dh, auth, &sender, &salt)
}

/// [`encrypt`] with a given sender key and salt (RFC 8291 §3.4 and Appendix A).
pub(crate) fn encrypt_with(
    payload: &[u8],
    p256dh: &str,
    auth: &str,
    sender: &SecretKey,
    salt: &[u8; 16],
) -> Result<Vec<u8>, String> {
    if payload.len() > MAX_PAYLOAD {
        return Err(format!(
            "the push payload is {} bytes, more than {MAX_PAYLOAD}",
            payload.len()
        ));
    }
    let ua_public_bytes = decode_b64(p256dh)?;
    let ua_public =
        PublicKey::from_sec1_bytes(&ua_public_bytes).map_err(|_| "p256dh is not a P-256 key".to_string())?;
    let auth_secret = decode_b64(auth)?;
    let as_public_bytes = sender.public_key().to_sec1_bytes();
    let shared = p256::ecdh::diffie_hellman(sender.to_nonzero_scalar(), ua_public.as_affine());

    // IKM = HKDF(auth_secret, ecdh_secret, "WebPush: info" || 0x00 || ua_public || as_public, 32)
    let mut key_info = Vec::with_capacity(14 + 65 + 65);
    key_info.extend_from_slice(b"WebPush: info\0");
    key_info.extend_from_slice(&ua_public_bytes);
    key_info.extend_from_slice(&as_public_bytes);
    let mut ikm = [0_u8; 32];
    Hkdf::<Sha256>::new(Some(&auth_secret), shared.raw_secret_bytes())
        .expand(&key_info, &mut ikm)
        .map_err(|_| "HKDF failed".to_string())?;

    let prk = Hkdf::<Sha256>::new(Some(salt), &ikm);
    let mut cek = [0_u8; 16];
    let mut nonce = [0_u8; 12];
    prk.expand(b"Content-Encoding: aes128gcm\0", &mut cek)
        .map_err(|_| "HKDF failed".to_string())?;
    prk.expand(b"Content-Encoding: nonce\0", &mut nonce)
        .map_err(|_| "HKDF failed".to_string())?;

    // One record, the last: the payload and the 0x02 delimiter, no further padding.
    let mut plaintext = Vec::with_capacity(payload.len() + 1);
    plaintext.extend_from_slice(payload);
    plaintext.push(2);
    let cipher = Aes128Gcm::new_from_slice(&cek).map_err(|_| "bad content key".to_string())?;
    let nonce = Nonce::try_from(&nonce[..]).map_err(|_| "bad nonce".to_string())?;
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_slice())
        .map_err(|_| "AES-GCM failed".to_string())?;

    let mut body = Vec::with_capacity(16 + 4 + 1 + as_public_bytes.len() + ciphertext.len());
    body.extend_from_slice(salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(u8::try_from(as_public_bytes.len()).map_err(|_| "bad key length".to_string())?);
    body.extend_from_slice(&as_public_bytes);
    body.extend_from_slice(&ciphertext);
    Ok(body)
}

#[cfg(test)]
#[path = "push_crypto_tests.rs"]
mod tests;
