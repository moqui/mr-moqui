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

use rcgen::PublicKeyData as _;
use rustls::pki_types::CertificateDer;
use sha2::{Digest, Sha256};

use super::{hex, parse_hex, public_key_sha256, Pin, PinVerifier};

fn cert_of(key: &rcgen::KeyPair) -> CertificateDer<'static> {
    let params = rcgen::CertificateParams::new(vec!["node.test".to_string()]).unwrap();
    params.self_signed(key).unwrap().der().clone()
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0_u8; 32];
    out.copy_from_slice(&Sha256::digest(bytes));
    out
}

#[test]
fn a_public_key_pin_holds_across_certificates_made_from_the_same_key() {
    let key = rcgen::KeyPair::generate().unwrap();
    let (first, second) = (cert_of(&key), cert_of(&key));
    assert_ne!(first, second, "rcgen signs each certificate afresh");
    let pin = digest(&key.subject_public_key_info());
    assert_eq!(public_key_sha256(&first), Some(pin));
    assert_eq!(public_key_sha256(&second), Some(pin));
    let provider = rustls::crypto::aws_lc_rs::default_provider();
    let by_key = PinVerifier::new(Pin::PublicKey(pin), true, &provider);
    assert!(by_key.trusted(&first).is_ok() && by_key.trusted(&second).is_ok());
    let other = cert_of(&rcgen::KeyPair::generate().unwrap());
    assert!(by_key.trusted(&other).is_err());
    let by_cert = PinVerifier::new(Pin::Certificate(digest(first.as_ref())), true, &provider);
    assert!(by_cert.trusted(&first).is_ok());
    assert!(by_cert.trusted(&second).is_err());
    let deferred = PinVerifier::new(Pin::Deferred, false, &provider);
    assert!(deferred.trusted(&other).is_ok());
    assert!(deferred.trusted(&CertificateDer::from(vec![1, 2, 3])).is_err());
}

#[test]
fn fingerprints_print_as_hex_and_read_back_with_separators() {
    let pin = digest(b"edge");
    let text = hex(&pin);
    assert_eq!(text.len(), 64);
    assert_eq!(parse_hex(&text), Some(pin));
    let pasted: Vec<String> = text
        .as_bytes()
        .chunks(2)
        .map(|pair| String::from_utf8_lossy(pair).to_uppercase())
        .collect();
    assert_eq!(parse_hex(&pasted.join(":")), Some(pin));
    assert_eq!(parse_hex("abc"), None);
    assert_eq!(parse_hex(&"zz".repeat(32)), None);
}
