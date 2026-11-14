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

//! The TLS-ALPN-01 answer (RFC 8737 §3): a self-signed certificate for the name with the critical `acmeIdentifier`
//! extension holding the SHA-256 of the challenge's key authorization, served only to a handshake that offers ALPN
//! `acme-tls/1`. Each member makes its own from the challenge's row, once, on the first handshake that wants it.

use std::sync::{Arc, OnceLock};

use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;

use super::ALPN;

/// An open challenge: the digest its certificate carries, until `expires` (epoch milliseconds).
pub(crate) struct Challenge {
    digest: [u8; 32],
    expires: i64,
    /// The certificate settings, made on first use; `None` when they could not be made (logged then).
    config: OnceLock<Option<Arc<rustls::ServerConfig>>>,
}

impl Challenge {
    pub(crate) fn new(digest: [u8; 32], expires: i64) -> Self {
        Self {
            digest,
            expires,
            config: OnceLock::new(),
        }
    }

    pub(super) fn digest(&self) -> &[u8; 32] {
        &self.digest
    }

    pub(super) fn same_as(&self, other: &Self) -> bool {
        self.digest == other.digest && self.expires == other.expires
    }

    /// The settings that answer this challenge for `name`, while it is open.
    pub(super) fn config(&self, name: &str) -> Option<Arc<rustls::ServerConfig>> {
        if self.expires <= moqui_core::user::now_millis() {
            return None;
        }
        self.config
            .get_or_init(|| match server_config(name, &self.digest) {
                Ok(config) => Some(Arc::new(config)),
                Err(err) => {
                    tracing::error!("Could not make the ACME challenge certificate for {name}: {err}");
                    None
                }
            })
            .clone()
    }
}

/// The certificate that answers a challenge for `name` with `digest`.
pub(crate) fn certificate(
    name: &str,
    digest: &[u8; 32],
) -> Result<(rustls::pki_types::CertificateDer<'static>, PrivateKeyDer<'static>), String> {
    let key = rcgen::KeyPair::generate().map_err(|err| format!("a key: {err}"))?;
    let mut params = rcgen::CertificateParams::new(vec![name.to_string()]).map_err(|err| format!("the name: {err}"))?;
    params.custom_extensions = vec![rcgen::CustomExtension::new_acme_identifier(digest)];
    let cert = params.self_signed(&key).map_err(|err| format!("signing: {err}"))?;
    Ok((
        cert.der().clone(),
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
    ))
}

/// The settings that serve the challenge's certificate. Through a resolver: rustls's own single certificate parses it,
/// and refuses its critical `acmeIdentifier` extension as one it does not know.
fn server_config(name: &str, digest: &[u8; 32]) -> Result<rustls::ServerConfig, String> {
    let (cert, key) = certificate(name, digest)?;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let signing = provider
        .key_provider
        .load_private_key(key)
        .map_err(|err| err.to_string())?;
    let answer = Answer(Arc::new(CertifiedKey::new(vec![cert], signing)));
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|err| err.to_string())?
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(answer));
    config.alpn_protocols = vec![ALPN.to_vec()];
    Ok(config)
}

/// The challenge's certificate, whatever the ClientHello asks: the settings are chosen for its name already.
#[derive(Debug)]
struct Answer(Arc<CertifiedKey>);

impl ResolvesServerCert for Answer {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(Arc::clone(&self.0))
    }
}

/// A digest from its hex form, as the rows hold it.
pub(crate) fn parse_digest(hex: &str) -> Option<[u8; 32]> {
    let hex = hex.trim();
    if hex.len() != 64 || !hex.is_ascii() {
        return None;
    }
    let mut digest = [0_u8; 32];
    for (at, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(at * 2..at * 2 + 2)?, 16).ok()?;
    }
    Some(digest)
}
