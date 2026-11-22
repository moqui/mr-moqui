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

//! Client trust for a local development server. Select the verifier at handshake time,
//! including a redirect or second origin on a reused HTTP client.
use crate::error::Error;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{pem::PemObject as _, CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use std::path::PathBuf;
use std::sync::Arc;

/// The runtime selected by the CLI or a checkout-launched client.
pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = crate::env::get_property("MOQUI_INSTALLED_RUNTIME") {
        return PathBuf::from(dir);
    }
    if let Some(dir) = std::env::var_os("MOQUI_RUNTIME").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    let rt = cwd.join("runtime");
    if !rt.join("components/base").is_dir() && cwd.join("components/base").is_dir() {
        cwd
    } else {
        rt
    }
}

/// Names and IP literals eligible for the extra development trust anchor.
pub fn loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

fn root() -> Option<CertificateDer<'static>> {
    let shared = crate::shared_runtime::directory().ok().flatten();
    std::iter::once(runtime_dir())
        .chain(shared)
        .find_map(|dir| CertificateDer::from_pem_file(dir.join("cert/dev-ca/ca.crt")).ok())
}

/// Extend the existing raw TLS root store for this destination.
pub fn add_loopback_root(roots: &mut rustls::RootCertStore, host: &str) -> Result<(), rustls::Error> {
    if loopback(host) {
        if let Some(cert) = root() {
            roots.add(cert)?;
        }
    }
    Ok(())
}

#[derive(Debug)]
struct LocalVerifier {
    normal: Arc<dyn ServerCertVerifier>,
    local: Arc<dyn ServerCertVerifier>,
}

impl ServerCertVerifier for LocalVerifier {
    fn verify_server_cert(
        &self,
        cert: &CertificateDer<'_>,
        chain: &[CertificateDer<'_>],
        name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let result = self.normal.verify_server_cert(cert, chain, name, ocsp, now);
        if result.is_err() && loopback(&name.to_str()) {
            self.local.verify_server_cert(cert, chain, name, ocsp, now)
        } else {
            result
        }
    }
    fn verify_tls12_signature(
        &self,
        msg: &[u8],
        cert: &CertificateDer<'_>,
        sig: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.normal.verify_tls12_signature(msg, cert, sig)
    }
    fn verify_tls13_signature(
        &self,
        msg: &[u8],
        cert: &CertificateDer<'_>,
        sig: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.normal.verify_tls13_signature(msg, cert, sig)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.normal.supported_verify_schemes()
    }
}

/// Prefer ordinary platform verification and try the dev root only on loopback handshakes.
pub fn platform_config() -> Result<Option<rustls::ClientConfig>, Error> {
    let Some(cert) = root() else { return Ok(None) };
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let fail = |msg: String| Error::new(format!("dev CA client: {msg}"));
    let normal = rustls_platform_verifier::Verifier::new(provider.clone()).map_err(|e| fail(e.to_string()))?;
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert).map_err(|e| fail(e.to_string()))?;
    let local = rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .build()
        .map_err(|e| fail(e.to_string()))?;
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| fail(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(LocalVerifier {
            normal: Arc::new(normal),
            local,
        }))
        .with_no_client_auth();
    Ok(Some(config))
}

pub fn builder(insecure: bool) -> Result<reqwest::blocking::ClientBuilder, Error> {
    let builder = reqwest::blocking::Client::builder();
    if insecure {
        return Ok(builder.tls_danger_accept_invalid_certs(true));
    }
    if let Some(mut config) = platform_config()? {
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(builder.tls_backend_preconfigured(config))
    } else {
        Ok(builder)
    }
}

#[cfg(test)]
mod tests {
    use super::loopback;

    #[test]
    fn the_development_root_is_available_only_to_loopback_destinations() {
        for host in ["localhost", "LOCALHOST", "127.0.0.1", "127.42.1.2", "::1", "[::1]"] {
            assert!(loopback(host), "{host}");
        }
        for host in ["example.net", "localhost.example.net", "10.0.0.1", "128.0.0.1", "::2"] {
            assert!(!loopback(host), "{host}");
        }
    }
}
