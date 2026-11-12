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

//! Field encryption. The `known_gap_` tests pin weaknesses the threat model names
//! (docs/security/Attack-Surface.md, "Threat model: stored secrets").

use aes::Aes128;
use base64::engine::general_purpose::URL_SAFE;
use base64::Engine;
use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockModeEncrypt, KeyIvInit};
use cbc::Encryptor;
use sha2::Sha256;

use super::{decrypt, decrypt_with_alts, encrypt, CryptConfig, CONSTANT_IV};

const PASS: &str = "MoquiDefaultPassword:CHANGEME";
const PLAIN: &str = "TestEntityFieldValue";
// Java 21 javax.crypto. Plaintext TestEntityFieldValue, password MoquiDefaultPassword:CHANGEME.
const AES_STORED: &str = ":YWMS3u2Ct95vQrzfkJnX0ujHZwEyoTeuiqWrUb9dlEw=";
const AES_HEX: &str = "616312DEED82B7DE6F42BCDF9099D7D2E8C7670132A137AE8AA5AB51BF5D944C";
const DES_STORED: &str = ":4aCNeen38Bw6G6CBQ2Dq-v7RQgjSEz_9";
const AES_CUSTOM_IV: &str = "MDEyMzQ1Njc4OWFiY2RlZg==:zAixLdnc1FPnFALiPFMnDqlT92pLaOPbQmkxEQEJXOE=";

fn aes_config() -> CryptConfig {
    CryptConfig {
        pass: PASS.to_string(),
        salt: "20201202".to_string(),
        iterations: 10,
        algorithm: "PBEWithHmacSHA256AndAES_128".to_string(),
    }
}

fn des_config() -> CryptConfig {
    CryptConfig {
        pass: PASS.to_string(),
        salt: "SkcorIuqom".to_string(),
        iterations: 10,
        algorithm: "PBEWithMD5AndDES".to_string(),
    }
}

/// The ciphertext bytes of a stored `:<base64url>` value.
fn ciphertext(stored: &str) -> Vec<u8> {
    URL_SAFE.decode(stored.strip_prefix(':').unwrap()).unwrap()
}

#[test]
fn a_pass_phrase_that_is_not_ascii_is_an_error() {
    let mut config = aes_config();
    config.pass = "Moqui\u{e9}Pass".to_string();
    assert_eq!(encrypt("x", &config).unwrap_err().to_string(), "Password is not ASCII");
}

#[test]
fn debug_leaves_out_the_pass() {
    let shown = format!("{:?}", aes_config());
    assert!(!shown.contains(PASS) && shown.contains("20201202"), "{shown}");
}

#[test]
fn encrypt_matches_java_aes_and_des() {
    assert_eq!(encrypt(PLAIN, &aes_config()).unwrap(), AES_STORED);
    assert_eq!(encrypt(PLAIN, &des_config()).unwrap(), DES_STORED);
    let mut default_algo = aes_config();
    default_algo.algorithm.clear();
    assert_eq!(encrypt(PLAIN, &default_algo).unwrap(), AES_STORED);
}

#[test]
fn decrypt_java_des_hex_and_custom_iv() {
    assert_eq!(decrypt(AES_STORED, &aes_config()).unwrap(), PLAIN);
    assert_eq!(decrypt(DES_STORED, &des_config()).unwrap(), PLAIN);
    assert_eq!(decrypt(AES_HEX, &aes_config()).unwrap(), PLAIN);
    assert_eq!(decrypt(&AES_HEX.to_ascii_lowercase(), &aes_config()).unwrap(), PLAIN);
    assert_eq!(decrypt(AES_CUSTOM_IV, &aes_config()).unwrap(), PLAIN);
}

#[test]
fn wrong_or_empty_pass_fails() {
    let mut wrong = aes_config();
    wrong.pass = "wrong-pass".to_string();
    let err = decrypt(AES_STORED, &wrong).unwrap_err();
    assert!(err
        .message()
        .contains("Encryption error with algo PBEWithHmacSHA256AndAES_128"));

    let mut empty = aes_config();
    empty.pass.clear();
    assert_eq!(
        encrypt(PLAIN, &empty).unwrap_err().message(),
        "No entity-facade.@crypt-pass setting found, NOT doing encryption"
    );
}

#[test]
fn decrypt_with_alts_keeps_the_original_error() {
    let aes = aes_config();
    let des = des_config();
    assert_eq!(
        decrypt_with_alts(DES_STORED, &aes, std::slice::from_ref(&des)).unwrap(),
        PLAIN
    );
    assert_eq!(
        decrypt_with_alts(AES_STORED, &aes, std::slice::from_ref(&des)).unwrap(),
        PLAIN
    );

    let mut wrong_aes = aes.clone();
    wrong_aes.pass = "wrong-pass".to_string();
    let mut wrong_des = des.clone();
    wrong_des.pass = "wrong-pass".to_string();
    let original = decrypt(DES_STORED, &wrong_aes).unwrap_err();
    let from_alts = decrypt_with_alts(DES_STORED, &wrong_aes, &[wrong_des]).unwrap_err();
    assert_eq!(from_alts.message(), original.message());
    assert!(from_alts.message().contains("PBEWithHmacSHA256AndAES_128"));
    assert!(!from_alts.message().contains("PBEWithMD5AndDES"));

    let mut empty = des.clone();
    empty.pass.clear();
    assert_eq!(decrypt_with_alts(DES_STORED, &empty, &[des]).unwrap(), PLAIN);
    let err = decrypt_with_alts(DES_STORED, &empty, &[]).unwrap_err();
    assert_eq!(
        err.message(),
        "No entity-facade.@crypt-pass setting found, NOT doing encryption"
    );
}

/// A stolen database without the pass shows no plaintext: neither the stored text nor its bytes hold it.
#[test]
fn a_stored_value_does_not_hold_its_plaintext() {
    let secret = "smtp-password-Zq7!smtp-password-Zq7!";
    let stored = encrypt(secret, &aes_config()).unwrap();
    assert!(
        !stored.contains(secret) && !stored.contains("smtp-password"),
        "{stored}"
    );
    let bytes = ciphertext(&stored);
    assert!(!bytes.windows(4).any(|window| window == b"smtp"));
    // PKCS7: whole blocks, with at least one byte of padding
    assert_eq!(bytes.len(), (secret.len() / 16 + 1) * 16);
    assert_eq!(decrypt(&stored, &aes_config()).unwrap(), secret);
}

/// Another install's pass, or the right pass with another salt or iteration count, does not decrypt.
#[test]
fn only_the_same_pass_salt_and_iterations_decrypt() {
    let stored = encrypt("refresh-token-0001", &aes_config()).unwrap();
    let mut other_pass = aes_config();
    other_pass.pass = "AnotherInstallsGeneratedPass==".to_string();
    let mut other_salt = aes_config();
    other_salt.salt = "20201203".to_string();
    let mut other_count = aes_config();
    other_count.iterations = 11;
    for config in [other_pass, other_salt, other_count] {
        assert_ne!(
            decrypt(&stored, &config).ok().as_deref(),
            Some("refresh-token-0001"),
            "{config:?}"
        );
    }
}

/// Encryption is deterministic: equal plaintexts give equal stored values, in any row and any field, so a reader of
/// the database sees which secrets are equal and can confirm a guess with the pass. Lookup by value
/// (`CalendarShare.publishToken`) needs this.
///
/// When this is fixed, this test fails: update the threat model in Attack-Surface.md.
#[test]
fn known_gap_equal_plaintexts_give_equal_ciphertext() {
    let first = encrypt("hunter2-hunter2", &aes_config()).unwrap();
    let second = encrypt("hunter2-hunter2", &aes_config()).unwrap();
    assert_eq!(first, second);
    assert_ne!(first, encrypt("hunter2-hunter3", &aes_config()).unwrap());
}

/// The IV is a constant (`CONSTANT_IV`), so two values that share their first 16 bytes share their first ciphertext
/// block: a reader of the database learns that two secrets share a prefix.
///
/// When this is fixed, this test fails: update the threat model in Attack-Surface.md.
#[test]
fn known_gap_a_shared_16_byte_prefix_gives_a_shared_first_block() {
    let a = ciphertext(&encrypt("https://api.example/a-token-1", &aes_config()).unwrap());
    let b = ciphertext(&encrypt("https://api.example/b-token-2", &aes_config()).unwrap());
    assert_eq!(a[..16], b[..16]);
    assert_ne!(a[16..], b[16..]);
}

/// There is no MAC: a value changed in the database decrypts without an error to other text. CBC lets a writer who
/// knows the plaintext of the first block set it to text of their choice by storing an IV of their own
/// (`<iv>:<ct>`, a form decrypt accepts), and dropping the first block turns a value into its tail.
///
/// When this is fixed, this test fails: update the threat model in Attack-Surface.md.
#[test]
fn known_gap_a_changed_ciphertext_is_not_detected() {
    let plain = "role=user;limit=00000100;note=kept";
    let stored = encrypt(plain, &aes_config()).unwrap();
    let body = stored.strip_prefix(':').unwrap();

    // chosen change of block one, no key: IV' = IV xor old xor new
    let mut iv = CONSTANT_IV.as_bytes()[..16].to_vec();
    for (index, (old, new)) in "role=user;limit=".bytes().zip("role=root;limit=".bytes()).enumerate() {
        iv[index] ^= old ^ new;
    }
    let forged = format!("{}:{body}", URL_SAFE.encode(iv));
    assert_eq!(
        decrypt(&forged, &aes_config()).unwrap(),
        "role=root;limit=00000100;note=kept"
    );

    // the first block as the IV of the rest
    let bytes = ciphertext(&stored);
    let tail = format!("{}:{}", URL_SAFE.encode(&bytes[..16]), URL_SAFE.encode(&bytes[16..]));
    assert_eq!(decrypt(&tail, &aes_config()).unwrap(), "00000100;note=kept");
}

/// The key is AES-128 from PBKDF2-HMAC-SHA256 of the pass with the conf's salt (`20201202`) and 10 iterations, and
/// the IV is the first 16 bytes of `CONSTANT_IV`. Whoever has the pass has every field's key; one guess of the pass
/// costs 10 HMACs.
///
/// When this is fixed, this test fails: update the threat model in Attack-Surface.md.
#[test]
fn known_gap_the_key_is_aes_128_from_10_pbkdf2_iterations_and_a_fixed_salt() {
    let key = pbkdf2::pbkdf2_hmac_array::<Sha256, 16>(PASS.as_bytes(), b"20201202", 10);
    let iv: [u8; 16] = CONSTANT_IV.as_bytes()[..16].try_into().unwrap();
    let expected = Encryptor::<Aes128>::new(&key.into(), &iv.into()).encrypt_padded_vec::<Pkcs7>(PLAIN.as_bytes());
    assert_eq!(ciphertext(&encrypt(PLAIN, &aes_config()).unwrap()), expected);
}

/// PBEWithMD5AndDES (56-bit DES, MD5 key derivation) still decrypts through the alternates, so a value written as DES
/// stays readable and is not refused.
///
/// When this is fixed, this test fails: update the threat model in Attack-Surface.md.
#[test]
fn known_gap_des_ciphertext_still_decrypts_through_the_alternates() {
    assert_eq!(
        decrypt_with_alts(DES_STORED, &aes_config(), &[des_config()]).unwrap(),
        PLAIN
    );
    assert_eq!(encrypt(PLAIN, &des_config()).unwrap(), DES_STORED);
}

/// The default conf keeps `entity_ds_crypt_pass_old` at its public sentinel and decrypts with it as an alternate
/// (`MoquiDefaultConf.xml`, `decrypt-alt`). A writer to the database with no key stores a value sealed under the
/// public pass, and a read with the generated pass takes it as valid.
///
/// When this is fixed, this test fails: update the threat model in Attack-Surface.md.
#[test]
fn known_gap_a_value_sealed_under_the_public_default_pass_is_read_as_valid() {
    let mut generated = aes_config();
    generated.pass = "q3J0b2tlbi1nZW5lcmF0ZWQtYnktc3RhcnR1cC0zMmI=".to_string();
    let public_old = aes_config();
    let planted = encrypt("attacker-chosen-smtp-host-password", &public_old).unwrap();
    assert_eq!(
        decrypt_with_alts(&planted, &generated, &[des_config(), public_old]).unwrap(),
        "attacker-chosen-smtp-host-password"
    );
}
