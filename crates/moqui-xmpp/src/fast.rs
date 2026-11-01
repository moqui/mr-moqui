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

//! FAST (XEP-0484) tokens, by the HT mechanisms (draft-schmaus-kitten-sasl-ht): a client that signed in with SASL2
//! asks for a token, and signs in again with one round trip: its name and HMAC-SHA-256 of "Initiator" and the channel
//! binding data, keyed by the token; the server answers HMAC of "Responder", so the client knows the server had it.
//!
//! `HT-SHA-256-EXPR` binds to the TLS session's exporter (RFC 9266); `HT-SHA-256-NONE` does not, and then the client's
//! `count` must grow with each use, so a captured exchange cannot be replayed. A token is kept per user and client
//! (`XmppFastToken`, the client's user-agent id), encrypted at rest, as the HMAC needs it.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use hmac::{KeyInit, Mac};
use sha2::Sha256;

pub const HT_EXPR: &str = "HT-SHA-256-EXPR";
pub const HT_NONE: &str = "HT-SHA-256-NONE";

/// Whether a FAST mechanism binds to the exporter; None for one the server does not have.
pub fn binds(mechanism: &str) -> Option<bool> {
    match mechanism {
        HT_EXPR => Some(true),
        HT_NONE => Some(false),
        _ => None,
    }
}

fn mac(token: &str, label: &[u8], binding: &[u8]) -> Vec<u8> {
    let mut mac = hmac::Hmac::<Sha256>::new_from_slice(token.as_bytes()).expect("HMAC takes a key of any length");
    mac.update(label);
    mac.update(binding);
    mac.finalize().into_bytes().to_vec()
}

/// The client's initial response: its name, a zero byte, and the initiator HMAC.
pub fn initial_response(username: &str, token: &str, binding: &[u8]) -> Vec<u8> {
    let mut out = username.as_bytes().to_vec();
    out.push(0);
    out.extend(mac(token, b"Initiator", binding));
    out
}

/// The name and HMAC of an initial response.
pub fn parse_initial(data: &[u8]) -> Option<(String, Vec<u8>)> {
    let at = data.iter().position(|b| *b == 0)?;
    let name = std::str::from_utf8(&data[..at]).ok()?.to_string();
    let proof = data[at + 1..].to_vec();
    (!name.is_empty() && proof.len() == 32).then_some((name, proof))
}

/// Whether `proof` is the initiator HMAC of `token` over `binding`, in constant time.
pub fn verify(token: &str, binding: &[u8], proof: &[u8]) -> bool {
    let expected = mac(token, b"Initiator", binding);
    expected.len() == proof.len() && expected.iter().zip(proof).fold(0_u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

/// The server's additional data on success.
pub fn responder(token: &str, binding: &[u8]) -> Vec<u8> {
    mac(token, b"Responder", binding)
}

/// A new token: 32 random bytes, base64url.
pub fn new_token() -> String {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).expect("the system random source works");
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_signs_in_with_its_binding_only() {
        let token = new_token();
        let initial = initial_response("jane", &token, b"exporter");
        let (name, proof) = parse_initial(&initial).unwrap();
        assert_eq!(name, "jane");
        assert!(verify(&token, b"exporter", &proof));
        assert!(!verify(&token, b"another", &proof), "another TLS session");
        assert!(!verify(&new_token(), b"exporter", &proof), "another token");
        assert_ne!(
            responder(&token, b"exporter"),
            proof,
            "the server proves itself with its own label"
        );
        assert!(parse_initial(b"jane").is_none());
        assert_eq!(binds(HT_EXPR), Some(true));
        assert_eq!(binds("HT-SHA-256-UNIQ"), None);
    }
}
