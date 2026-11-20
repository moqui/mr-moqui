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

//! AWS Signature Version 4 (docs.aws.amazon.com, "Create a signed AWS API request"): the canonical request, the string
//! to sign and the `Authorization` header, for the S3 client (`crate::s3`) and the seal's AWS KMS method
//! (`moqui_core::seal::methods::kms`). No SDK.

use hmac::{KeyInit as _, Mac as _};
use sha2::{Digest as _, Sha256};

/// SHA-256 of an empty payload, hex.
pub const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// The keys a request is signed with. Its `Debug` shows the access key id only.
pub struct Credentials<'a> {
    pub access_key: &'a str,
    pub secret_key: &'a str,
}

impl std::fmt::Debug for Credentials<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("access_key", &self.access_key)
            .finish_non_exhaustive()
    }
}

/// What a signature is for: the region and the service (`s3`, `kms`).
#[derive(Clone, Copy, Debug)]
pub struct Scope<'a> {
    pub region: &'a str,
    pub service: &'a str,
}

/// SigV4's URI encoding: unreserved characters as they are, every other byte `%XX`; `/` too when `slash`.
#[must_use]
pub fn uri_encode(text: &str, slash: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') || (byte == b'/' && !slash) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The canonical query string: each name and value encoded, sorted, joined with `&`.
#[must_use]
pub fn canonical_query(query: &[(&str, String)]) -> String {
    let mut pairs: Vec<(String, String)> = query
        .iter()
        .map(|(name, value)| (uri_encode(name, true), uri_encode(value, true)))
        .collect();
    pairs.sort();
    pairs
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// Bytes as lower-case hex.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// SHA-256 of `bytes`, hex: a payload's hash.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// HMAC-SHA256 of `data` under `key`.
#[must_use]
pub fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    // HMAC takes a key of any length
    let mut mac = hmac::Hmac::<Sha256>::new_from_slice(key).unwrap_or_else(|_| unreachable!());
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// The parts of a request that are signed: `path` and `query` already encoded, `headers` with `host` and every
/// `x-amz-` header, all of them signed.
#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub query: &'a str,
    pub headers: &'a [(String, String)],
    pub payload_hash: &'a str,
}

/// The `Authorization` header of `request`, `amz_date` as `x-amz-date` gives it (`20150830T123600Z`).
#[must_use]
pub fn authorization(request: &Request<'_>, credentials: &Credentials<'_>, scope: Scope<'_>, amz_date: &str) -> String {
    let Request {
        method,
        path,
        query,
        headers,
        payload_hash,
    } = *request;
    let mut signed: Vec<(String, String)> = headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    signed.sort();
    let canonical_headers: String = signed.iter().map(|(name, value)| format!("{name}:{value}\n")).collect();
    let signed_names = signed
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let canonical = format!("{method}\n{path}\n{query}\n{canonical_headers}\n{signed_names}\n{payload_hash}");
    let date = amz_date.get(..8).unwrap_or(amz_date);
    let credential_scope = format!("{date}/{}/{}/aws4_request", scope.region, scope.service);
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{}",
        sha256_hex(canonical.as_bytes())
    );
    // the secret and the keys derived from it are cleared when dropped
    let seed = zeroize::Zeroizing::new(format!("AWS4{}", credentials.secret_key));
    let mut key = zeroize::Zeroizing::new(hmac(seed.as_bytes(), date));
    for part in [scope.region, scope.service, "aws4_request"] {
        key = zeroize::Zeroizing::new(hmac(&key, part));
    }
    let signature = hex(&hmac(&key, &to_sign));
    format!(
        "AWS4-HMAC-SHA256 Credential={}/{credential_scope}, SignedHeaders={signed_names}, Signature={signature}",
        credentials.access_key
    )
}

#[cfg(test)]
#[path = "sigv4_tests.rs"]
mod tests;
