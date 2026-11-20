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

//! A link key and the certificate a node presents with it: made from the key each time, so only the key is kept, and
//! the other end pins the key (SHA-256 of its public key), not the certificate.

use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use rcgen::PublicKeyData as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};

use crate::tls_pin::hex;

/// A new link key, base64 PKCS#8 DER: the value of `edge/node@key`.
pub fn new_key() -> Result<String, String> {
    let key = rcgen::KeyPair::generate().map_err(|err| format!("could not make a key: {err}"))?;
    Ok(STANDARD.encode(key.serialize_der()))
}

/// A node's key and the certificate it presents: made from the key each time, so only the key is kept, and the other
/// end pins the key, not the certificate.
#[derive(Clone)]
pub(crate) struct Identity {
    key: Arc<Vec<u8>>,
    cert: CertificateDer<'static>,
    /// SHA-256 of the public key, hex.
    pub fingerprint: String,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

/// The PKCS#8 v1 head of an Ed25519 private key (RFC 8410): the 32-byte seed follows.
const ED25519_PKCS8_HEAD: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];

impl Identity {
    /// From `edge/node@key` (base64 PKCS#8 DER) or `edge@link-key` (that, or the base64 32-byte seed of an Ed25519 key,
    /// as a generated secret is); the certificate names `instance_id`.
    pub(crate) fn from_key(key: &str, instance_id: &str) -> Result<Self, String> {
        let raw = STANDARD
            .decode(key.trim())
            .map_err(|_| "the link key is not base64".to_string())?;
        let der = match <[u8; 32]>::try_from(raw.as_slice()) {
            Ok(seed) => [ED25519_PKCS8_HEAD.as_slice(), seed.as_slice()].concat(),
            Err(_) => raw,
        };
        let pair =
            rcgen::KeyPair::try_from(der.as_slice()).map_err(|_| "the link key is not a private key".to_string())?;
        let name = if instance_id.is_empty() {
            "moqui-local-node"
        } else {
            instance_id
        };
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new())
            .map_err(|err| format!("could not make this node's certificate: {err}"))?;
        let mut subject = rcgen::DistinguishedName::new();
        subject.push(rcgen::DnType::CommonName, name);
        params.distinguished_name = subject;
        let cert = params
            .self_signed(&pair)
            .map_err(|err| format!("could not sign this node's certificate: {err}"))?;
        let digest = Sha256::digest(pair.subject_public_key_info());
        let mut fingerprint = [0_u8; 32];
        fingerprint.copy_from_slice(&digest);
        Ok(Self {
            key: Arc::new(der),
            cert: cert.der().clone(),
            fingerprint: hex(&fingerprint),
        })
    }

    pub(crate) fn cert(&self) -> CertificateDer<'static> {
        self.cert.clone()
    }

    pub(crate) fn key_der(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key.as_ref().clone()))
    }
}
