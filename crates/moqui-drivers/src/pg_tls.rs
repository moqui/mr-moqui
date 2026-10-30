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

//! The rustls client configuration of a Postgres server connection, by libpq's `sslmode` and `sslrootcert`: which
//! authorities are trusted (the `webpki-roots` public ones, or a PEM file of others, as a managed database with its
//! own authority needs) and what of the server's certificate is checked.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moqui_core::entity::datasource::TlsVerify;
use moqui_lib::error::Error;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, DigitallySignedStruct, SignatureScheme};

/// What a TLS connection checks, and the authorities it trusts.
#[derive(Clone, Debug, Default)]
pub struct TlsOptions {
    pub verify: TlsVerify,
    /// A PEM file of the authorities to trust. `None` is the `webpki-roots` public ones.
    pub root_cert: Option<PathBuf>,
}

impl TlsOptions {
    /// From a conf's `sslmode` reading and `sslrootcert` path (empty for none).
    #[must_use]
    pub fn new(verify: TlsVerify, root_cert: &str) -> Self {
        let root_cert = root_cert.trim();
        Self {
            verify,
            root_cert: (!root_cert.is_empty()).then(|| PathBuf::from(root_cert)),
        }
    }

    /// `require` with a root certificate file is `verify-ca`, as in libpq.
    fn effective_verify(&self) -> TlsVerify {
        match (self.verify, &self.root_cert) {
            (TlsVerify::Encrypt, Some(_)) => TlsVerify::Ca,
            (verify, _) => verify,
        }
    }
}

/// The client configuration for `options`.
///
/// # Errors
///
/// Returns an error when the root certificate file cannot be read or holds no certificate, or rustls refuses the
/// configuration.
pub fn client_config(options: &TlsOptions) -> Result<Arc<rustls::ClientConfig>, Error> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .map_err(|err| Error::new("postgres TLS configuration").with_source(err))?;
    let roots = || match &options.root_cert {
        Some(path) => root_store(path),
        None => Ok(rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        }),
    };
    let config = match options.effective_verify() {
        TlsVerify::Full => builder.with_root_certificates(roots()?).with_no_client_auth(),
        TlsVerify::Ca => {
            let inner = WebPkiServerVerifier::builder_with_provider(Arc::new(roots()?), Arc::clone(&provider))
                .build()
                .map_err(|err| Error::new("postgres TLS configuration").with_source(err))?;
            builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(AnyName(inner)))
                .with_no_client_auth()
        }
        TlsVerify::Encrypt => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(Unchecked(provider.signature_verification_algorithms)))
            .with_no_client_auth(),
    };
    Ok(Arc::new(config))
}

/// Every certificate in the PEM file at `path`.
fn root_store(path: &Path) -> Result<rustls::RootCertStore, Error> {
    let shown = path.display();
    let certs = CertificateDer::pem_file_iter(path)
        .map_err(|err| Error::new(format!("postgres TLS root certificate {shown}: {err}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| Error::new(format!("postgres TLS root certificate {shown}: {err}")))?;
    let mut store = rustls::RootCertStore::empty();
    let (added, _ignored) = store.add_parsable_certificates(certs);
    if added == 0 {
        return Err(Error::new(format!(
            "postgres TLS root certificate {shown} holds no certificate"
        )));
    }
    Ok(store)
}

/// `verify-ca`: the chain to a trusted authority is checked, and the name in the certificate is not.
#[derive(Debug)]
struct AnyName(Arc<WebPkiServerVerifier>);

impl ServerCertVerifier for AnyName {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match self
            .0
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
        {
            Err(rustls::Error::InvalidCertificate(
                CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. },
            )) => Ok(ServerCertVerified::assertion()),
            other => other,
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.0.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.0.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_verify_schemes()
    }
}

/// `require`: encrypted, and any certificate accepted. The handshake signatures are still checked, so the session is
/// with the holder of the certificate's key.
#[derive(Debug)]
struct Unchecked(rustls::crypto::WebPkiSupportedAlgorithms);

impl ServerCertVerifier for Unchecked {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_schemes()
    }
}
