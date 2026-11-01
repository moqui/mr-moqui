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

//! A TLS certificate verifier that trusts a pinned fingerprint instead of a certificate authority, for either end of a
//! connection: the cluster listener and its peers (one shared certificate, `cluster_edge.rs`), and the edge link
//! (plans/Edge-Node.md §5.1: a key per local node, pinned both ways). The handshake's signature is still checked against
//! the presented certificate, so a pin proves possession of the key.

// the public key pins and their hex form are the edge link's
#![cfg_attr(not(feature = "edge"), allow(dead_code))]

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::DigitallySignedStruct;
use sha2::{Digest, Sha256};

/// What a presented certificate must match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pin {
    /// SHA-256 of the whole certificate (the cluster's shared certificate).
    Certificate([u8; 32]),
    /// SHA-256 of the certificate's public key (SubjectPublicKeyInfo), which stays the same when the certificate is
    /// made again from the same key (the edge link).
    PublicKey([u8; 32]),
    /// Any certificate, checked after the handshake against the pins the caller keeps (the edge node, which pins one
    /// key per local node in its database).
    Deferred,
}

pub(crate) struct PinVerifier {
    pin: Pin,
    /// A client must present a certificate (server side only).
    mandatory: bool,
    schemes: rustls::crypto::WebPkiSupportedAlgorithms,
    hints: Vec<rustls::DistinguishedName>,
}

impl std::fmt::Debug for PinVerifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("PinVerifier").finish_non_exhaustive()
    }
}

impl PinVerifier {
    /// A verifier of `pin` with the signature algorithms of `provider`. `mandatory` is for the server side: a client
    /// with no certificate fails the handshake; without it a client may present none, and one presented is still checked.
    pub(crate) fn new(pin: Pin, mandatory: bool, provider: &rustls::crypto::CryptoProvider) -> Arc<Self> {
        Arc::new(Self {
            pin,
            mandatory,
            schemes: provider.signature_verification_algorithms,
            hints: Vec::new(),
        })
    }

    fn trusted(&self, end_entity: &CertificateDer<'_>) -> Result<(), rustls::Error> {
        let matches = match self.pin {
            Pin::Certificate(digest) => Sha256::digest(end_entity.as_ref()).as_slice() == digest,
            Pin::PublicKey(digest) => public_key_sha256(end_entity) == Some(digest),
            Pin::Deferred => public_key_sha256(end_entity).is_some(),
        };
        if matches {
            Ok(())
        } else {
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::UnknownIssuer,
            ))
        }
    }
}

/// SHA-256 of a certificate's SubjectPublicKeyInfo, or `None` when it does not parse.
pub(crate) fn public_key_sha256(cert: &CertificateDer<'_>) -> Option<[u8; 32]> {
    let parsed = webpki::EndEntityCert::try_from(cert).ok()?;
    let digest = Sha256::digest(parsed.subject_public_key_info().as_ref());
    let mut out = [0_u8; 32];
    out.copy_from_slice(&digest);
    Some(out)
}

/// A fingerprint as lowercase hex, as it is stored and shown.
pub(crate) fn hex(digest: &[u8; 32]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(64);
    for byte in digest {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// A fingerprint from hex (colons and spaces allowed, as an operator may paste it), or `None`.
pub(crate) fn parse_hex(text: &str) -> Option<[u8; 32]> {
    let digits: Vec<u8> = text
        .bytes()
        .filter(|byte| !matches!(byte, b':' | b' ' | b'-'))
        .collect();
    if digits.len() != 64 {
        return None;
    }
    let mut out = [0_u8; 32];
    let (pairs, _) = digits.as_chunks::<2>();
    for (slot, pair) in out.iter_mut().zip(pairs) {
        let text = std::str::from_utf8(pair).ok()?;
        *slot = u8::from_str_radix(text, 16).ok()?;
    }
    Some(out)
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        self.trusted(end_entity)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.schemes)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.schemes)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.schemes.supported_schemes()
    }
}

impl ClientCertVerifier for PinVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        self.mandatory
    }

    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &self.hints
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        self.trusted(end_entity)?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.schemes)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.schemes)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.schemes.supported_schemes()
    }
}

#[cfg(test)]
#[path = "tls_pin_tests.rs"]
mod tests;
