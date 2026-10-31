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

use super::*;
use p256::ecdsa::signature::Verifier;

// RFC 8291 Appendix A.
const PLAINTEXT: &[u8] = b"When I grow up, I want to be a watermelon";
const AS_PRIVATE: &str = "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw";
const AS_PUBLIC: &str = "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8";
const UA_PRIVATE: &str = "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94";
const UA_PUBLIC: &str = "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";
const SALT: &str = "DGv6ra1nlYgDCS1FRnbzlw";
const BODY: &str = "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN";

fn secret(text: &str) -> SecretKey {
    SecretKey::from_slice(&decode_b64(text).unwrap()).unwrap()
}

/// The user agent's side (RFC 8291 §3.4 from the receiver), to read back what [`encrypt`] made.
fn decrypt(body: &[u8], ua_private: &SecretKey, auth: &str) -> Vec<u8> {
    let salt = &body[..16];
    let record_size = u32::from_be_bytes(body[16..20].try_into().unwrap());
    assert_eq!(record_size, 4096);
    let key_length = usize::from(body[20]);
    let as_public_bytes = &body[21..21 + key_length];
    let ciphertext = &body[21 + key_length..];
    let as_public = PublicKey::from_sec1_bytes(as_public_bytes).unwrap();
    let ua_public_bytes = ua_private.public_key().to_sec1_bytes();
    let shared = p256::ecdh::diffie_hellman(ua_private.to_nonzero_scalar(), as_public.as_affine());
    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend_from_slice(&ua_public_bytes);
    key_info.extend_from_slice(as_public_bytes);
    let mut ikm = [0_u8; 32];
    Hkdf::<Sha256>::new(Some(&decode_b64(auth).unwrap()), shared.raw_secret_bytes())
        .expand(&key_info, &mut ikm)
        .unwrap();
    let prk = Hkdf::<Sha256>::new(Some(salt), &ikm);
    let mut cek = [0_u8; 16];
    let mut nonce = [0_u8; 12];
    prk.expand(b"Content-Encoding: aes128gcm\0", &mut cek).unwrap();
    prk.expand(b"Content-Encoding: nonce\0", &mut nonce).unwrap();
    let cipher = Aes128Gcm::new_from_slice(&cek).unwrap();
    let mut plain = cipher
        .decrypt(&Nonce::try_from(&nonce[..]).unwrap(), ciphertext)
        .unwrap();
    assert_eq!(plain.pop(), Some(2), "the last record ends with the 0x02 delimiter");
    plain
}

#[test]
fn the_rfc_8291_example_encrypts_to_its_body() {
    let sender = secret(AS_PRIVATE);
    assert_eq!(encode_b64(&sender.public_key().to_sec1_bytes()), AS_PUBLIC);
    assert_eq!(encode_b64(&secret(UA_PRIVATE).public_key().to_sec1_bytes()), UA_PUBLIC);
    let salt: [u8; 16] = decode_b64(SALT).unwrap().try_into().unwrap();
    let body = encrypt_with(PLAINTEXT, UA_PUBLIC, AUTH, &sender, &salt).unwrap();
    assert_eq!(encode_b64(&body), BODY);
}

#[test]
fn a_fresh_encryption_reads_back_with_the_browser_key() {
    let body = encrypt(b"{\"title\":\"Hello\"}", UA_PUBLIC, AUTH).unwrap();
    assert_eq!(decrypt(&body, &secret(UA_PRIVATE), AUTH), b"{\"title\":\"Hello\"}");
    let again = encrypt(b"{\"title\":\"Hello\"}", UA_PUBLIC, AUTH).unwrap();
    assert_ne!(body, again, "each message has its own sender key and salt");
}

#[test]
fn a_payload_over_the_limit_is_refused() {
    let big = vec![b'x'; MAX_PAYLOAD + 1];
    assert!(encrypt(&big, UA_PUBLIC, AUTH).unwrap_err().contains("more than"));
    assert!(encrypt(&big[..MAX_PAYLOAD], UA_PUBLIC, AUTH).is_ok());
}

#[test]
fn subscription_keys_are_checked() {
    assert!(check_keys(UA_PUBLIC, AUTH).is_ok());
    assert!(check_keys("BAAA", AUTH).is_err());
    assert!(check_keys(UA_PUBLIC, "AAAA").is_err());
    // padded and standard-alphabet forms are taken
    assert!(check_keys(UA_PUBLIC, "BTBZMqHH6r4Tts7J/aSIgg==").is_ok());
}

#[test]
fn the_vapid_header_is_an_es256_token_for_the_endpoint_origin() {
    let seed = base64::engine::general_purpose::STANDARD.encode(decode_b64(AS_PRIVATE).unwrap());
    let vapid = Vapid::from_seed(&seed).unwrap();
    assert_eq!(vapid.public_key(), AS_PUBLIC);
    let header = vapid
        .authorization(
            "https://push.example.net:8443/wpush/v2/abc?x=1",
            "mailto:ops@example.com",
            1_000,
        )
        .unwrap();
    let rest = header.strip_prefix("vapid t=").unwrap();
    let (token, key) = rest.split_once(", k=").unwrap();
    assert_eq!(key, AS_PUBLIC);
    let parts: Vec<&str> = token.split('.').collect();
    assert_eq!(parts.len(), 3);
    let claims: serde_json::Value = serde_json::from_slice(&decode_b64(parts[1]).unwrap()).unwrap();
    assert_eq!(claims["aud"], "https://push.example.net:8443");
    assert_eq!(claims["sub"], "mailto:ops@example.com");
    assert_eq!(claims["exp"], 1_000 + 12 * 3600);
    let verifying = p256::ecdsa::VerifyingKey::from(secret(AS_PRIVATE).public_key());
    let signature = p256::ecdsa::Signature::from_slice(&decode_b64(parts[2]).unwrap()).unwrap();
    let signed = format!("{}.{}", parts[0], parts[1]);
    assert!(verifying.verify(signed.as_bytes(), &signature).is_ok());
}

#[test]
fn an_endpoint_must_be_https_but_for_loopback() {
    assert_eq!(
        origin("https://fcm.googleapis.com/fcm/send/x").unwrap(),
        "https://fcm.googleapis.com"
    );
    assert_eq!(origin("http://127.0.0.1:9000/push").unwrap(), "http://127.0.0.1:9000");
    assert!(origin("http://push.example.net/x").is_err());
    assert!(origin("https://user@push.example.net/x").is_err());
    assert!(origin("push.example.net").is_err());
}
