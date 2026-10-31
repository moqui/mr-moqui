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

//! `SimpleSigner`: an RSA signature over a PEM private key, a port of `org.moqui.impl.util.SimpleSigner`.
//!
//! The Java class loads a PKCS#8 key from a classpath resource, signs with `SHA1withRSA`, and returns the
//! signature as Base64. QZ Tray asks the server to sign each print job string with it (`qzSign` transition in
//! `apps.xml`, `WebFacadeImpl.sendQzSignedResponse`). This port keeps that output byte for byte and also reads a
//! PKCS#1 `RSA PRIVATE KEY` PEM.
//!
//! RSA PKCS#1 v1.5 signing is deterministic, so a signature made here equals the Java one for the same key and
//! data. The `rsa` crate carries RUSTSEC-2023-0071 (Marvin, a timing leak in private-key operations). Every
//! signature here is made with blinding, using a fresh system random source, which is the mitigation the
//! advisory names. The signed strings are print job descriptions that the browser also holds, not secrets.

use std::fmt;
use std::path::Path;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::sha2::{Sha256, Sha384, Sha512};
use rsa::signature::{RandomizedSigner, SignatureEncoding};
use rsa::RsaPrivateKey;
use sha1::Sha1;

use crate::error::Error;

/// The hash half of a Java signature name such as `SHA1withRSA`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hash {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

impl Hash {
    fn parse(signature_type: &str) -> Option<Self> {
        match signature_type {
            "SHA1withRSA" => Some(Self::Sha1),
            "SHA256withRSA" => Some(Self::Sha256),
            "SHA384withRSA" => Some(Self::Sha384),
            "SHA512withRSA" => Some(Self::Sha512),
            _ => None,
        }
    }
}

/// Random source for blinding, from the operating system through `getrandom`.
struct BlindingRng;

impl rand_core::TryRng for BlindingRng {
    type Error = getrandom::Error;

    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        let mut bytes = [0_u8; 4];
        getrandom::fill(&mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        let mut bytes = [0_u8; 8];
        getrandom::fill(&mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
        getrandom::fill(dst)
    }
}

impl rand_core::TryCryptoRng for BlindingRng {}

/// Signs strings with an RSA private key. Like the Java class, a key that cannot be read does not fail
/// construction. The failure is kept, and [`SimpleSigner::sign`] reports it.
pub struct SimpleSigner {
    key_resource: String,
    hash: Hash,
    key: Option<RsaPrivateKey>,
    load_error: Option<String>,
}

impl fmt::Debug for SimpleSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SimpleSigner")
            .field("key_resource", &self.key_resource)
            .field("hash", &self.hash)
            .field("loaded", &self.key.is_some())
            .finish()
    }
}

impl SimpleSigner {
    /// A signer for PEM text with the Java defaults: key type `RSA` and `SHA1withRSA`. `key_resource` names where
    /// the text came from, for messages.
    pub fn from_pem(key_resource: &str, pem: &str) -> Self {
        Self::from_pem_with(key_resource, pem, None, None)
    }

    /// A signer with a key type and signature name, as the Java three-argument constructor. `None` keeps the
    /// default. Key type `RSA` is the only one supported, and the signature names are `SHA1withRSA`,
    /// `SHA256withRSA`, `SHA384withRSA`, and `SHA512withRSA`.
    pub fn from_pem_with(key_resource: &str, pem: &str, key_type: Option<&str>, signature_type: Option<&str>) -> Self {
        let mut signer = Self {
            key_resource: key_resource.to_string(),
            hash: Hash::Sha1,
            key: None,
            load_error: None,
        };
        let key_type = key_type.unwrap_or("RSA");
        let signature_type = signature_type.unwrap_or("SHA1withRSA");
        if !key_type.eq_ignore_ascii_case("RSA") {
            signer.load_error = Some(format!("key type {key_type} is not supported, only RSA"));
            return signer;
        }
        match Hash::parse(signature_type) {
            Some(hash) => signer.hash = hash,
            None => {
                signer.load_error = Some(format!("signature type {signature_type} is not supported"));
                return signer;
            }
        }
        match read_key(pem).and_then(|der| parse_private_key(&der)) {
            Ok(key) => signer.key = Some(key),
            Err(err) => signer.load_error = Some(err.message().to_string()),
        }
        signer
    }

    /// A signer for the PEM file at `path`. A file that cannot be read is kept as a load failure.
    pub fn from_file(path: &Path) -> Self {
        let label = path.display().to_string();
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_pem(&label, &text),
            Err(err) => Self {
                key_resource: label,
                hash: Hash::Sha1,
                key: None,
                load_error: Some(format!("could not read signing key: {err}")),
            },
        }
    }

    /// True when the key was read.
    pub fn is_loaded(&self) -> bool {
        self.key.is_some()
    }

    /// Why the key could not be read, when it could not.
    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    /// The signature of the UTF-8 bytes of `data`, Base64 with padding and no line breaks.
    ///
    /// # Errors
    ///
    /// Returns an error when the key was not loaded, or when the system random source fails.
    pub fn sign(&self, data: &str) -> Result<String, Error> {
        let Some(key) = &self.key else {
            let reason = self.load_error.as_deref().unwrap_or("unknown error");
            return Err(Error::new(format!(
                "Cannot sign message, key could not be loaded from resource {}: {reason}",
                self.key_resource
            )));
        };
        let mut rng = BlindingRng;
        let bytes = data.as_bytes();
        let signature = match self.hash {
            Hash::Sha1 => SigningKey::<Sha1>::new(key.clone()).try_sign_with_rng(&mut rng, bytes),
            Hash::Sha256 => SigningKey::<Sha256>::new(key.clone()).try_sign_with_rng(&mut rng, bytes),
            Hash::Sha384 => SigningKey::<Sha384>::new(key.clone()).try_sign_with_rng(&mut rng, bytes),
            Hash::Sha512 => SigningKey::<Sha512>::new(key.clone()).try_sign_with_rng(&mut rng, bytes),
        }
        .map_err(|err| Error::new(format!("Could not sign message: {err}")))?;
        Ok(STANDARD.encode(signature.to_bytes()))
    }
}

/// The DER bytes of a PEM key. Lines with `-----BEGIN`, `-----END`, or `: ` are skipped and the rest are
/// trimmed and joined, as `SimpleSigner.readKey` does, then strict Base64 decoded.
///
/// # Errors
///
/// Returns an error when the text is not Base64.
pub fn read_key(pem: &str) -> Result<Vec<u8>, Error> {
    let mut joined = String::new();
    for line in pem.split('\n') {
        if line.contains("-----BEGIN") || line.contains("-----END") || line.contains(": ") {
            continue;
        }
        joined.push_str(line.trim());
    }
    STANDARD
        .decode(joined.as_bytes())
        .map_err(|err| Error::new(format!("signing key is not Base64: {err}")))
}

/// PKCS#8 first, which is what Java reads, then PKCS#1.
fn parse_private_key(der: &[u8]) -> Result<RsaPrivateKey, Error> {
    match RsaPrivateKey::from_pkcs8_der(der) {
        Ok(key) => Ok(key),
        Err(pkcs8_err) => RsaPrivateKey::from_pkcs1_der(der)
            .map_err(|_| Error::new(format!("signing key is neither PKCS#8 nor PKCS#1 RSA: {pkcs8_err}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PKCS8: &str = include_str!("../testdata/qz-test-key-pkcs8.pem");
    const PKCS1: &str = include_str!("../testdata/qz-test-key-pkcs1.pem");

    // Produced by Java 21 running the body of SimpleSigner (PKCS8EncodedKeySpec, SHA1withRSA, Base64) on
    // PKCS8, and cross-checked with `openssl dgst -sha1 -sign key.pem | base64`.
    const MSG1: &str = "Hello QZ";
    const MSG2: &str = "caf\u{e9} \u{20ac} 1/2 print job {\"call\":\"print\",\"params\":{}}";
    const SHA1_MSG1: &str = concat!(
        "O42KHicKJoEdDMDXBzv9QGNQnRg0jgPrQTsU7PRDOYWmAtRUd5rc4IChsjYL7GzW",
        "YwRS1p3egjHzijdnzGyssd3pWAy80rUzeeTPi1jeeyTqApzC8/pHdRSsYwBk8Hwj",
        "JAHi8kGJBjFXgZ7seDJVxhFs+sBTdsBx8lCUc+SWqcYx/fxYOrBQXnQ/K6fXaq9U",
        "PU03Lj+cdcT3Ms6mxJUcwXwU7AsuaExo68F+DaICtQXEyz7PeO7bbGlNhDXz9a1U",
        "cbfRhFJTW9p/94w51io97Yk7BkTVJa3ncyU+qRZDeHIORHalcMFG1SVSmXEP3qIM",
        "2r2+ua1rzVy2KufZPgCAmQ=="
    );
    const SHA1_MSG2: &str = concat!(
        "oQdatLT109kEJiYz0VbJbWRmmub3Znq56343q5P6u40oacbSDRFaPckC4eZVfMYg",
        "6piEQ88/9txP05+cooOOmM+ueyRZv3d4Q/3Ke/EEly11mShWlb2WdOlVNvPdiuNd",
        "LBlSpn/rWvuS1kG1gPEuPkMJVwyzDwqNb2YrdyuUs+iS3pGdHGnjNuxsLZalpRAD",
        "9FTJ/nAlJs2b8vAVW4fHWAHIkRDY+iF7jFR9+txCqxFlVX1L8X9dYrLRZpPPmM6h",
        "/KPbZkzLMle4JuFS4c7Wzxjr4Xft6T1aRJ/CkXd1D5LnHREx4xy7nZzcMZGIpuPx",
        "Aes6woTbTgHS3oaM1illOA=="
    );
    const SHA1_EMPTY: &str = concat!(
        "MkpOicZ6GdR9xziQP0lcB1S86VQO+k+TpX+wxe+APhU052RgZkJaepTvlVWrqlsY",
        "8K4tHukGSVSpxkI5oVVqhRGaGYMAMxAJVRFy0yiXf9oP8iOtsz16M6JRP6KzxTOm",
        "dr63lHuzt3QmajGmYHXAxWAN8vnSYl3Y7yS6O2UPcmDyruEL0xqGE5q7hwugP5oq",
        "LpInmHjdqeqI0H9ZtkxhOymlqKsA6VoklD1WbNPCULS8X/qzu1vkdJL9IRcGSwys",
        "0mmY1UcIgkd1szRLyiWJe4xo742gU9dXhqW0VTGAkYY0XxEJ2sEBZWSY4ZtM193d",
        "QX0J2y2VA8uZ0rRWfidewA=="
    );
    const SHA256_MSG1: &str = concat!(
        "KP+R/BzcNnGqgUBJwlcQaKg3omVYhm/GxcYyJmBRg9YoTIClHdi1EXJRdNKwNw32",
        "NEZTE8YpVWe29MiL90pNeYEsA+8FJTHMxAAoBRjFhx41Gzbdb5Y6ziwuSpM8FybD",
        "0vlTY2TlTLrHCleC//mQi70eli/jEkGUDQtQT97xLA/bal5w3d3UHDp2wNbr5fuh",
        "ADevZQYiQFv4CIoJ/lr8cRuS2sJxyD2t/VidF1TwTndjuaL0qWzfsCpxzax/sr9w",
        "FqgDdWHpLbvfNgqs2b19Arwps4b6srTyGcB34VpBGIXk/Kpo5PUg+cXJ5ijW3wSI",
        "h826n4Ky7G4YZ08Ajdn3oQ=="
    );
    const SHA256_MSG2: &str = concat!(
        "OYAAZVeTm6Na5sxW5UkQjHCnNA443FP5cN8nPphABW/yGvJZVMkoQDV64CuUdQOf",
        "a1cKi6yfvT340VvYoZf8fXCcFvx3eLW+sITMu7O5r21TuzAd7E7ZkFSkQ2zIBftD",
        "IlRlwmjhWhcixA4wvb6O+nsZt9arKsrkbYT04uPMIA8z/jdWQqNFBy3u/mzBYpX0",
        "z43BkUdFw3Hr6YEu1HuoKkmKc324ZttiJtdTI3Ja9T+JBf+e4wcPhzHv3Zlmvc0t",
        "9ehWubNWiMzqbma0X3qv7exTN5acBAeLgNObHgG0Auw74XIEAtg3MHo7wAvNBZYF",
        "vlHynF3e8EvrB5hNcImo6w=="
    );

    #[test]
    fn sha1_with_rsa_matches_java_pkcs8() {
        let signer = SimpleSigner::from_pem("test", PKCS8);
        assert!(signer.is_loaded(), "{:?}", signer.load_error());
        assert_eq!(signer.sign(MSG1).unwrap(), SHA1_MSG1);
        assert_eq!(signer.sign(MSG2).unwrap(), SHA1_MSG2);
        assert_eq!(signer.sign("").unwrap(), SHA1_EMPTY);
    }

    #[test]
    fn pkcs1_pem_signs_the_same() {
        let signer = SimpleSigner::from_pem("test", PKCS1);
        assert!(signer.is_loaded(), "{:?}", signer.load_error());
        assert_eq!(signer.sign(MSG1).unwrap(), SHA1_MSG1);
    }

    #[test]
    fn sha256_with_rsa_matches_java() {
        let signer = SimpleSigner::from_pem_with("test", PKCS8, Some("RSA"), Some("SHA256withRSA"));
        assert_eq!(signer.sign(MSG1).unwrap(), SHA256_MSG1);
        assert_eq!(signer.sign(MSG2).unwrap(), SHA256_MSG2);
    }

    #[test]
    fn crlf_and_indented_pem_still_reads() {
        let crlf = PKCS8.replace('\n', "\r\n");
        assert_eq!(SimpleSigner::from_pem("crlf", &crlf).sign(MSG1).unwrap(), SHA1_MSG1);
        let bare: String = PKCS8
            .lines()
            .filter(|l| !l.starts_with("-----"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(SimpleSigner::from_pem("bare", &bare).sign(MSG1).unwrap(), SHA1_MSG1);
    }

    #[test]
    fn a_bad_key_fails_at_sign_not_at_construction() {
        let signer = SimpleSigner::from_pem(
            "classpath://qz-private-key.pem",
            "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n",
        );
        assert!(!signer.is_loaded());
        let err = signer.sign("x").unwrap_err();
        assert!(
            err.message()
                .contains("Cannot sign message, key could not be loaded from resource classpath://qz-private-key.pem"),
            "{}",
            err.message()
        );
        assert!(SimpleSigner::from_pem("t", "not base64 !!").sign("x").is_err());
        assert!(SimpleSigner::from_pem_with("t", PKCS8, Some("DSA"), None)
            .sign("x")
            .is_err());
        assert!(SimpleSigner::from_pem_with("t", PKCS8, None, Some("MD5withRSA"))
            .sign("x")
            .is_err());
        assert!(SimpleSigner::from_file(Path::new("/nonexistent/qz.pem"))
            .sign("x")
            .is_err());
    }
}
