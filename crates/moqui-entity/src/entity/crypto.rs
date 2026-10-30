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

//! Field encryption. `EntityJavaUtil.enDeCrypt`. No database.

use aes::Aes128;
use base64::engine::general_purpose::URL_SAFE;
use base64::Engine;
use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit};
use cbc::{Decryptor, Encryptor};
use des::Des;
use md5::{Digest, Md5};
use moqui_lib::error::Error;
use sha2::Sha256;

const CONSTANT_IV: &str = "WeNeedAtLeast32CharactersFor256BitBlockSizeToHaveAConstantIVForQueryByEncryptedValue";
const _: () = assert!(CONSTANT_IV.len() >= 16);

const SALT_LEN: usize = 8;
const SALT_PAD: u8 = 0x45;
const DEFAULT_SALT: &str = "default1";
const DEFAULT_ALGO: &str = "PBEWithHmacSHA256AndAES_128";
const DES_ALGO: &str = "PBEWithMD5AndDES";
const AES_KEY_LEN: usize = 16;
const AES_BLOCK: usize = 16;
const DES_BLOCK: usize = 8;
const MISSING_PASS: &str = "No entity-facade.@crypt-pass setting found, NOT doing encryption";

#[derive(Clone)]
pub struct CryptConfig {
    pub pass: String,
    pub salt: String,
    pub iterations: u32,
    pub algorithm: String,
}

/// Leaves out the pass phrase.
impl std::fmt::Debug for CryptConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CryptConfig")
            .field("pass", &"<redacted>")
            .field("salt", &self.salt)
            .field("iterations", &self.iterations)
            .field("algorithm", &self.algorithm)
            .finish()
    }
}

pub fn encrypt(plain: &str, config: &CryptConfig) -> Result<String, Error> {
    let material = load(config)?;
    let iv = constant_iv(material.scheme);
    let ciphertext = crypt_bytes(&material, true, iv, plain.as_bytes())?;
    let encoded = URL_SAFE.encode(ciphertext);
    Ok(format!(":{encoded}"))
}

pub fn decrypt(stored: &str, config: &CryptConfig) -> Result<String, Error> {
    let material = load(config)?;
    let (iv, ciphertext) = decode_stored(stored, material.scheme, material.algorithm)?;
    let plain = crypt_bytes(&material, false, &iv, &ciphertext)?;
    String::from_utf8(plain).map_err(|err| crypt_error(material.algorithm).with_source(err))
}

pub fn decrypt_with_alts(stored: &str, primary: &CryptConfig, alts: &[CryptConfig]) -> Result<String, Error> {
    let original = match decrypt(stored, primary) {
        Ok(plain) => return Ok(plain),
        Err(err) => err,
    };
    for alt in alts {
        if let Ok(plain) = decrypt(stored, alt) {
            return Ok(plain);
        }
    }
    Err(original)
}

#[derive(Clone, Copy)]
enum Scheme {
    Aes,
    Des,
}

struct Loaded<'a> {
    password: Vec<u8>,
    salt: [u8; SALT_LEN],
    iterations: u32,
    algorithm: &'a str,
    scheme: Scheme,
}

fn load(config: &CryptConfig) -> Result<Loaded<'_>, Error> {
    if config.pass.is_empty() {
        return Err(Error::new(MISSING_PASS));
    }
    let algorithm = if config.algorithm.is_empty() {
        DEFAULT_ALGO
    } else {
        config.algorithm.as_str()
    };
    if config.iterations == 0 {
        return Err(crypt_error(algorithm));
    }
    Ok(Loaded {
        password: password_bytes(&config.pass)?,
        salt: salt_bytes(&config.salt),
        iterations: config.iterations,
        algorithm,
        scheme: scheme_for(algorithm)?,
    })
}

fn scheme_for(algorithm: &str) -> Result<Scheme, Error> {
    if algorithm.eq_ignore_ascii_case(DEFAULT_ALGO) {
        Ok(Scheme::Aes)
    } else if algorithm.eq_ignore_ascii_case(DES_ALGO) {
        Ok(Scheme::Des)
    } else {
        Err(crypt_error(algorithm))
    }
}

fn crypt_error(algorithm: &str) -> Error {
    Error::new(format!("Encryption error with algo {algorithm}"))
}

/// As Java's `PBEKey`: the pass phrase must be printable ASCII, not truncated to bytes.
fn password_bytes(pass: &str) -> Result<Vec<u8>, Error> {
    if pass.bytes().any(|byte| !(0x20..=0x7e).contains(&byte)) {
        return Err(Error::new("Password is not ASCII"));
    }
    Ok(pass.as_bytes().to_vec())
}

fn salt_bytes(salt: &str) -> [u8; SALT_LEN] {
    let source = if salt.is_empty() { DEFAULT_SALT } else { salt };
    let raw = source.as_bytes();
    let mut out = [SALT_PAD; SALT_LEN];
    let n = raw.len().min(SALT_LEN);
    out[..n].copy_from_slice(&raw[..n]);
    out
}

fn constant_iv(scheme: Scheme) -> &'static [u8] {
    match scheme {
        Scheme::Aes => &CONSTANT_IV.as_bytes()[..AES_BLOCK],
        Scheme::Des => &CONSTANT_IV.as_bytes()[..DES_BLOCK],
    }
}

fn decode_stored(stored: &str, scheme: Scheme, algorithm: &str) -> Result<(Vec<u8>, Vec<u8>), Error> {
    if let Some((prefix, body)) = stored.split_once(':') {
        let iv = if prefix.is_empty() {
            constant_iv(scheme).to_vec()
        } else {
            URL_SAFE
                .decode(prefix)
                .map_err(|err| crypt_error(algorithm).with_source(err))?
        };
        let ciphertext = URL_SAFE
            .decode(body)
            .map_err(|err| crypt_error(algorithm).with_source(err))?;
        return Ok((iv, ciphertext));
    }
    let ciphertext = parse_hex(stored).ok_or_else(|| crypt_error(algorithm))?;
    Ok((constant_iv(scheme).to_vec(), ciphertext))
}

fn parse_hex(value: &str) -> Option<Vec<u8>> {
    let bytes = value.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.as_chunks::<2>().0 {
        let hi = hex_nibble(pair[0])?;
        let lo = hex_nibble(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn crypt_bytes(material: &Loaded<'_>, encrypting: bool, iv: &[u8], input: &[u8]) -> Result<Vec<u8>, Error> {
    match material.scheme {
        Scheme::Aes => {
            let key = pbes2_aes_key(&material.password, &material.salt, material.iterations);
            let iv = array_from::<AES_BLOCK>(iv, material.algorithm)?;
            if encrypting {
                Ok(aes_encrypt(key, iv, input))
            } else {
                aes_decrypt(key, iv, input, material.algorithm)
            }
        }
        Scheme::Des => {
            let (key, derived_iv) = pbes1_md5_des(&material.password, &material.salt, material.iterations);
            if encrypting {
                Ok(des_encrypt(key, derived_iv, input))
            } else {
                des_decrypt(key, derived_iv, input, material.algorithm)
            }
        }
    }
}

fn array_from<const N: usize>(bytes: &[u8], algorithm: &str) -> Result<[u8; N], Error> {
    <[u8; N]>::try_from(bytes).map_err(|err| crypt_error(algorithm).with_source(err))
}

fn pbes2_aes_key(password: &[u8], salt: &[u8], iterations: u32) -> [u8; AES_KEY_LEN] {
    pbkdf2::pbkdf2_hmac_array::<Sha256, AES_KEY_LEN>(password, salt, iterations)
}

fn pbes1_md5_des(password: &[u8], salt: &[u8], iterations: u32) -> ([u8; DES_BLOCK], [u8; DES_BLOCK]) {
    let mut hasher = Md5::new();
    hasher.update(password);
    hasher.update(salt);
    let mut digest = hasher.finalize();
    for _ in 1..iterations {
        digest = Md5::digest(digest);
    }
    let mut key = [0u8; DES_BLOCK];
    let mut iv = [0u8; DES_BLOCK];
    key.copy_from_slice(&digest[..DES_BLOCK]);
    iv.copy_from_slice(&digest[DES_BLOCK..]);
    (key, iv)
}

fn aes_encrypt(key: [u8; AES_KEY_LEN], iv: [u8; AES_BLOCK], plain: &[u8]) -> Vec<u8> {
    Encryptor::<Aes128>::new(&key.into(), &iv.into()).encrypt_padded_vec::<Pkcs7>(plain)
}

fn aes_decrypt(
    key: [u8; AES_KEY_LEN],
    iv: [u8; AES_BLOCK],
    ciphertext: &[u8],
    algorithm: &str,
) -> Result<Vec<u8>, Error> {
    Decryptor::<Aes128>::new(&key.into(), &iv.into())
        .decrypt_padded_vec::<Pkcs7>(ciphertext)
        .map_err(|err| crypt_error(algorithm).with_source(err))
}

fn des_encrypt(key: [u8; DES_BLOCK], iv: [u8; DES_BLOCK], plain: &[u8]) -> Vec<u8> {
    Encryptor::<Des>::new(&key.into(), &iv.into()).encrypt_padded_vec::<Pkcs7>(plain)
}

fn des_decrypt(
    key: [u8; DES_BLOCK],
    iv: [u8; DES_BLOCK],
    ciphertext: &[u8],
    algorithm: &str,
) -> Result<Vec<u8>, Error> {
    Decryptor::<Des>::new(&key.into(), &iv.into())
        .decrypt_padded_vec::<Pkcs7>(ciphertext)
        .map_err(|err| crypt_error(algorithm).with_source(err))
}

#[cfg(test)]
mod tests {
    use super::{decrypt, decrypt_with_alts, encrypt, CryptConfig};

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
}
