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

//! The local node's identity on the link and pairing (plans/archive/Edge-Node.md §5.1, §5.7): its key and the certificate made
//! from it, the TLS client settings (WebPKI at pairing, the pinned key after), and the pairing connection's Probe and
//! Pair calls.

use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use moqui_core::edge::EdgeInfo;
use rcgen::PublicKeyData as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt as _;
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;

use super::proto::{PairRequest, ProbeRequest};
use super::{
    check_versions, edge_info, hello, read_answer, Answer, LinkClient, LinkError, ALPN, ALPN_PAIR, LINK_VERSION,
    ROLE_CHECK, ROLE_PAIR,
};
use crate::tls_pin::{hex, parse_hex, Pin, PinVerifier};

/// What pairing needs: the edge node, its operator's one-use code, and this node's name in its rows.
#[derive(Clone)]
pub struct PairConfig {
    /// `https://host` or `https://host:port`.
    pub url: String,
    pub code: String,
    pub instance_id: String,
    pub node_name: String,
    /// The edge node's link key fingerprint the operator confirmed (from [`check`]); pairing is refused when it
    /// differs.
    pub expect_pin: Option<String>,
    /// Tests only: trust this certificate authority instead of the web's (webpki-roots) when checking the edge node's
    /// certificate. A deployment leaves it `None`; it never turns the check off.
    pub trust_root: Option<CertificateDer<'static>>,
    pub connect_timeout: Duration,
}

impl PairConfig {
    pub fn new(url: &str, code: &str, instance_id: &str) -> Self {
        Self {
            url: url.to_string(),
            code: code.to_string(),
            instance_id: instance_id.to_string(),
            node_name: instance_id.to_string(),
            expect_pin: None,
            trust_root: None,
            connect_timeout: Duration::from_secs(10),
        }
    }
}

impl std::fmt::Debug for PairConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairConfig")
            .field("url", &self.url)
            .field("code", &"<redacted>")
            .field("instance_id", &self.instance_id)
            .field("node_name", &self.node_name)
            .field("expect_pin", &self.expect_pin)
            .field("trust_root", &self.trust_root.is_some())
            .field("connect_timeout", &self.connect_timeout)
            .finish()
    }
}

/// What pairing made and pinned: the values for `edge_node_url`, `edge_node_key` and `edge_node_pin`.
#[derive(Clone)]
pub struct Paired {
    pub edge: EdgeInfo,
    /// The edge node's link key fingerprint (`edge_node_pin`).
    pub pin: String,
    /// This node's new private key, base64 PKCS#8 DER (`edge_node_key`).
    pub key: String,
    /// This node's public key fingerprint, as the edge node pinned it.
    pub key_fingerprint: String,
}

impl std::fmt::Debug for Paired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Paired")
            .field("edge", &self.edge)
            .field("pin", &self.pin)
            .field("key", &"<redacted>")
            .field("key_fingerprint", &self.key_fingerprint)
            .finish()
    }
}

/// The host and port of an edge node URL: `https://host[:port][/...]`, or `host[:port]`; 443 by default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EdgeUrl {
    pub host: String,
    pub port: u16,
}

pub(crate) fn parse_url(url: &str) -> Result<EdgeUrl, String> {
    let text = url.trim();
    let with_scheme = if text.contains("://") {
        text.to_string()
    } else {
        format!("https://{text}")
    };
    let uri: http::Uri = with_scheme
        .parse()
        .map_err(|_| format!("the edge node URL {text} is not a URL"))?;
    if uri.scheme_str() != Some("https") {
        return Err(format!("the edge node URL {text} is not https"));
    }
    let host = uri
        .host()
        .map(|host| host.trim_start_matches('[').trim_end_matches(']').to_string())
        .filter(|host| !host.is_empty())
        .ok_or_else(|| format!("the edge node URL {text} has no host"))?;
    Ok(EdgeUrl {
        host,
        port: uri.port_u16().unwrap_or(443),
    })
}

/// A new key for this node, base64 PKCS#8 DER: the value of `edge_node_key`.
pub fn new_key() -> Result<String, String> {
    let key = rcgen::KeyPair::generate().map_err(|err| format!("could not make a key: {err}"))?;
    Ok(STANDARD.encode(key.serialize_der()))
}

/// This node's key and the certificate it presents: made from the key each time, so only the key is kept, and the
/// edge node pins the key, not the certificate.
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
    /// From `edge_node_key` (base64 PKCS#8 DER) or `edge_link_key` (that, or the base64 32-byte seed of an Ed25519 key,
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

/// How the edge node's certificate is checked.
pub(crate) enum ServerTrust<'a> {
    /// WebPKI, against the web's roots or a test's own: at pairing.
    WebPki(Option<&'a CertificateDer<'static>>),
    /// The public key pinned at pairing.
    Pinned([u8; 32]),
}

/// The TLS client settings of an edge connection: ALPN `moqui-edge-pair/1` with WebPKI, `moqui-edge/1` with the pinned
/// link key; this node's certificate when it has one.
pub(crate) fn client_config(
    trust: &ServerTrust<'_>,
    identity: Option<&Identity>,
) -> Result<Arc<rustls::ClientConfig>, String> {
    let alpn = match trust {
        ServerTrust::WebPki(_) => ALPN_PAIR,
        ServerTrust::Pinned(_) => ALPN,
    };
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .map_err(|err| format!("edge link TLS: {err}"))?;
    let builder = match trust {
        ServerTrust::WebPki(root) => {
            let mut roots = rustls::RootCertStore::empty();
            match root {
                Some(root) => roots
                    .add((*root).clone())
                    .map_err(|err| format!("edge link TLS trust root: {err}"))?,
                None => roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned()),
            }
            builder.with_root_certificates(roots)
        }
        ServerTrust::Pinned(pin) => builder.dangerous().with_custom_certificate_verifier(PinVerifier::new(
            Pin::PublicKey(*pin),
            true,
            &provider,
        )),
    };
    let mut config = match identity {
        Some(identity) => builder
            .with_client_auth_cert(vec![identity.cert.clone()], identity.key_der())
            .map_err(|err| format!("edge link TLS client certificate: {err}"))?,
        None => builder.with_no_client_auth(),
    };
    config.alpn_protocols = vec![alpn.to_vec()];
    Ok(Arc::new(config))
}

/// Dial the edge node, make the TLS handshake and check that it speaks the link.
pub(crate) async fn dial_tls(
    url: &EdgeUrl,
    config: Arc<rustls::ClientConfig>,
    timeout: Duration,
) -> Result<TlsStream<TcpStream>, LinkError> {
    let name = ServerName::try_from(url.host.clone())
        .map_err(|_| LinkError::Failed(format!("{} is not a host name", url.host)))?;
    let place = format!("{}:{}", url.host, url.port);
    let alpn: &[u8] = config.alpn_protocols.first().map_or(ALPN, Vec::as_slice);
    let connect = async {
        let tcp = TcpStream::connect((url.host.as_str(), url.port))
            .await
            .map_err(|err| LinkError::Failed(format!("could not connect to the edge node at {place}: {err}")))?;
        let _ = tcp.set_nodelay(true);
        tokio_rustls::TlsConnector::from(Arc::clone(&config))
            .connect(name, tcp)
            .await
            .map_err(|err| tls_failure(&err, &place, alpn == ALPN))
    };
    let stream = tokio::time::timeout(timeout, connect)
        .await
        .map_err(|_| LinkError::Failed(format!("the edge node at {place} did not answer in time")))??;
    if stream.get_ref().1.alpn_protocol() != Some(alpn) {
        return Err(LinkError::Failed(format!(
            "the server at {place} is not an edge node: it does not speak the edge link (ALPN {})",
            String::from_utf8_lossy(alpn)
        )));
    }
    Ok(stream)
}

/// What a link connection's TLS failure means. On a link connection a certificate that is not the pinned link key
/// means the edge node's link key changed, which only pairing again fixes.
fn tls_failure(err: &std::io::Error, place: &str, pinned: bool) -> LinkError {
    let invalid_certificate = err
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustls::Error>())
        .is_some_and(|tls| matches!(tls, rustls::Error::InvalidCertificate(_)));
    if pinned && invalid_certificate {
        return LinkError::Refused(format!(
            "the edge node at {place} presented a link key other than the one pinned at pairing: its link key \
             changed, so pair again"
        ));
    }
    LinkError::Failed(format!("TLS with the edge node at {place} failed: {err}"))
}

/// A check or pairing connection (`role`), as far as the HTTP/2 client: the role sent and answered. The task drives
/// the connection.
async fn pairing_client(
    url: &EdgeUrl,
    config: Arc<rustls::ClientConfig>,
    timeout: Duration,
    role: u8,
) -> Result<(LinkClient, tokio::task::JoinHandle<()>), LinkError> {
    let mut stream = dial_tls(url, config, timeout).await?;
    let roled = async {
        stream.write_all(&[role]).await?;
        stream.flush().await?;
        read_answer(&mut stream).await
    };
    let (answer, message) = tokio::time::timeout(timeout, roled)
        .await
        .map_err(|_| LinkError::Failed("the edge node did not answer the pairing connection".to_string()))?
        .map_err(|err| LinkError::Failed(format!("the pairing connection failed: {err}")))?;
    if answer != Answer::Ok {
        return Err(LinkError::Refused(message));
    }
    let (send, connection) = hyper::client::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new())
        .timer(hyper_util::rt::TokioTimer::new())
        .handshake(hyper_util::rt::TokioIo::new(stream))
        .await
        .map_err(|err| LinkError::Failed(format!("HTTP/2 with the edge node failed: {err}")))?;
    let driver = tokio::spawn(async move {
        let _ = connection.await;
    });
    Ok((super::link_client(send), driver))
}

/// The link key fingerprint in a Probe or Pair answer.
fn link_fingerprint(text: &str) -> Result<[u8; 32], LinkError> {
    parse_hex(text).ok_or_else(|| LinkError::Failed("the edge node told no link key fingerprint".to_string()))
}

/// Probe on a check or pairing connection: the edge node's Hello and link key.
async fn probe(client: &mut LinkClient, timeout: Duration) -> Result<(super::proto::Hello, [u8; 32]), LinkError> {
    let reply = tokio::time::timeout(timeout, client.probe(ProbeRequest {}))
        .await
        .map_err(|_| LinkError::Failed("the edge node did not answer Probe".to_string()))?
        .map_err(|status| LinkError::Failed(format!("Probe: {}", status.message())))?
        .into_inner();
    let fingerprint = link_fingerprint(&reply.link_fingerprint)?;
    Ok((reply.hello.unwrap_or_default(), fingerprint))
}

/// The edge node at `url`, checked by WebPKI (or `trust_root`, in a test), without pairing: its Hello and fingerprint
/// for the operator to confirm.
pub async fn check(
    url: &str,
    trust_root: Option<&CertificateDer<'static>>,
    timeout: Duration,
) -> Result<EdgeInfo, LinkError> {
    let place = parse_url(url).map_err(LinkError::Failed)?;
    let config = client_config(&ServerTrust::WebPki(trust_root), None).map_err(LinkError::Failed)?;
    let (mut client, driver) = pairing_client(&place, config, timeout, ROLE_CHECK).await?;
    let probed = probe(&mut client, timeout).await;
    driver.abort();
    let (hello, fingerprint) = probed?;
    Ok(edge_info(&hello, &hex(&fingerprint)))
}

/// Pair with the edge node: a new key for this node, presented on a pairing connection with the operator's code; the
/// edge node pins it and this node pins the edge node's (checked by WebPKI first).
pub async fn pair(config: &PairConfig) -> Result<Paired, LinkError> {
    let place = parse_url(&config.url).map_err(LinkError::Failed)?;
    let expect =
        match config
            .expect_pin
            .as_deref()
            .map(str::trim)
            .filter(|pin| !pin.is_empty())
        {
            Some(text) => Some(parse_hex(text).ok_or_else(|| {
                LinkError::Failed("the confirmed fingerprint is not a SHA-256 fingerprint".to_string())
            })?),
            None => None,
        };
    let key = new_key().map_err(LinkError::Failed)?;
    let identity = Identity::from_key(&key, &config.instance_id).map_err(LinkError::Failed)?;
    let tls =
        client_config(&ServerTrust::WebPki(config.trust_root.as_ref()), Some(&identity)).map_err(LinkError::Failed)?;
    let (mut client, driver) = pairing_client(&place, tls, config.connect_timeout, ROLE_PAIR).await?;
    let fingerprint = match probe(&mut client, config.connect_timeout).await {
        Ok((_, fingerprint)) => fingerprint,
        Err(err) => {
            driver.abort();
            return Err(err);
        }
    };
    if expect.is_some_and(|expect| expect != fingerprint) {
        driver.abort();
        return Err(LinkError::Refused(format!(
            "the edge node's link key fingerprint is {}, not the one confirmed",
            hex(&fingerprint)
        )));
    }
    let request = PairRequest {
        code: config.code.trim().to_string(),
        hello: Some(hello(&config.instance_id, &config.node_name, &[], &[])),
    };
    let reply = tokio::time::timeout(config.connect_timeout, client.pair(request)).await;
    driver.abort();
    let reply = reply
        .map_err(|_| LinkError::Failed("the edge node did not answer Pair".to_string()))?
        .map_err(|status| LinkError::Refused(status.message().to_string()))?
        .into_inner();
    let edge_hello = reply.hello.unwrap_or_default();
    check_versions(LINK_VERSION, edge_hello.link_version, "edge node").map_err(LinkError::Refused)?;
    if link_fingerprint(&reply.link_fingerprint)? != fingerprint {
        return Err(LinkError::Failed("the edge node told two link keys".to_string()));
    }
    if reply.key_fingerprint != identity.fingerprint {
        return Err(LinkError::Failed(
            "the edge node pinned a key that is not this node's".to_string(),
        ));
    }
    let pin = hex(&fingerprint);
    Ok(Paired {
        edge: edge_info(&edge_hello, &pin),
        pin,
        key,
        key_fingerprint: identity.fingerprint,
    })
}
