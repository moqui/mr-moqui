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

//! The check before a name is set up (plans/Edge-Node2.md §4.8): that the name's DNS has an address, and that a TLS
//! connection to it on the challenge port (443, `acme_challenge_port`), as the authority will open, reaches this node.
//! This node opens a challenge of its own for the name with a random digest, connects to the name as the authority
//! would, offering `acme-tls/1`, and looks for that digest in the certificate it is served: only this node (or a member
//! of its cluster, or this node through its edge node's passthrough tunnel) has it. The challenge is closed after.
//!
//! A home router that cannot loop a connection back to its own public address fails this check though the authority
//! would get through; the screen offers to order without it.

use std::net::{SocketAddr, ToSocketAddrs as _};
use std::sync::Arc;
use std::time::Duration;

use moqui_core::context::ExecutionContextFactory;
use moqui_lib::error::Error;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::DigitallySignedStruct;

use super::{store, Acme, ALPN};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the check's own challenge stays open; it is closed when the check ends.
const PROBE_OPEN: Duration = Duration::from_secs(60);
/// The most the check's connections take together. It runs on the request's thread, so the screen waits for it.
const PROBE_LIMIT: Duration = Duration::from_secs(20);

/// The port the authority connects to.
pub(crate) fn challenge_port() -> u16 {
    moqui_lib::env::get_prop_or_env("acme_challenge_port")
        .trim()
        .parse()
        .unwrap_or(443)
}

/// Check that `host` reaches this node for the challenge: what was found, or why not.
pub(crate) fn check(factory: &Arc<ExecutionContextFactory>, host: &str) -> Result<String, Error> {
    let port = challenge_port();
    let addresses: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|err| Error::new(format!("{host} has no address in DNS ({err})")))?
        .collect();
    if addresses.is_empty() {
        return Err(Error::new(format!("{host} has no address in DNS")));
    }
    let listed = addresses
        .iter()
        .map(|address| address.ip().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let acme = Acme::of(factory);
    let instance = moqui_core::drain::instance_id(factory);
    if acme.is_ordering(host) || store::locked_elsewhere(factory, host, &instance)? {
        return Err(Error::new(format!(
            "{host} is being ordered now; check again when that is done"
        )));
    }
    // the listener of a node with no certificate yet starts for the check, and closes after it unless something else
    // wants it
    let _checking = acme.check_started();
    let mut digest = [0_u8; 32];
    rustls::crypto::aws_lc_rs::default_provider()
        .secure_random
        .fill(&mut digest)
        .map_err(|_| Error::new("no random bytes for the check"))?;
    store::put_challenge(factory, host, &digest, PROBE_OPEN)?;
    // every member reads the challenge rows again before the check connects, as before the authority's
    std::thread::sleep(super::SEEN_BY_EVERY_MEMBER);
    let probing = probe(addresses, host.to_string(), digest);
    let probe = crate::edge::block_on(async move {
        tokio::time::timeout(PROBE_LIMIT, probing)
            .await
            .unwrap_or_else(|_| Err(format!("no answer in {} seconds", PROBE_LIMIT.as_secs())))
    });
    if let Err(err) = store::close_challenge(factory, host, &digest) {
        tracing::error!("ACME: the check's challenge for {host} could not be closed: {err}");
    }
    match probe? {
        Ok(()) => Ok(format!(
            "{host} is at {listed}, and port {port} there answered with this node's challenge"
        )),
        Err(why) => Err(Error::new(format!(
            "{host} is at {listed}, but port {port} there did not answer with this node's challenge: {why}"
        ))),
    }
}

/// Connect to each address until one serves `digest` for `host`.
async fn probe(addresses: Vec<SocketAddr>, host: String, digest: [u8; 32]) -> Result<(), String> {
    let mut why = String::from("no address answered");
    // the listener may have just been asked to start
    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        for address in &addresses {
            match served(*address, &host).await {
                Ok(cert) if cert.windows(digest.len()).any(|window| window == digest) => return Ok(()),
                Ok(_) => why = format!("{address} served another certificate"),
                Err(err) => why = format!("{address}: {err}"),
            }
        }
    }
    Err(why)
}

/// The leaf certificate `address` serves to an `acme-tls/1` handshake for `host`.
pub(super) async fn served(address: SocketAddr, host: &str) -> Result<Vec<u8>, String> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .map_err(|err| err.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCertificate(provider)))
        .with_no_client_auth();
    config.alpn_protocols = vec![ALPN.to_vec()];
    let name = ServerName::try_from(host.to_string()).map_err(|err| err.to_string())?;
    let tcp = tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(address))
        .await
        .map_err(|_| "no connection in time".to_string())?
        .map_err(|err| err.to_string())?;
    let tls = tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio_rustls::TlsConnector::from(Arc::new(config)).connect(name, tcp),
    )
    .await
    .map_err(|_| "no TLS handshake in time".to_string())?
    .map_err(|err| format!("the TLS handshake failed ({err})"))?;
    tls.get_ref()
        .1
        .peer_certificates()
        .and_then(|chain| chain.first())
        .map(|leaf| leaf.as_ref().to_vec())
        .ok_or_else(|| "no certificate".to_string())
}

/// The check takes any certificate and any signature: what it looks for is its own random digest in the certificate,
/// which only a server that has its challenge row can send, so reaching one is what it shows and all it shows. The
/// signatures are not checked because rustls parses the certificate for that, and refuses the challenge's critical
/// `acmeIdentifier` extension as one it does not know.
#[derive(Debug)]
pub(super) struct AnyCertificate(pub(super) Arc<rustls::crypto::CryptoProvider>);

impl ServerCertVerifier for AnyCertificate {
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
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}
