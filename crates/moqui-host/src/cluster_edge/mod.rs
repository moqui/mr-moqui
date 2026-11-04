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

//! The cluster port (plans/archive/Cluster.md): its listener, its senders, and the path a peer's topic message takes on
//! this member, whichever transport carried it.
//!
//! Empty `cluster_https_port` is 8444 on `cluster_bind_address` (default `0.0.0.0`, so other hosts reach it; the
//! listener takes only peers that present the cluster certificate). `0` is ephemeral: the listener binds port 0
//! and [`ClusterEdge::address`] is the address the kernel chose. The public HTTP and HTTPS listeners do not ask
//! for a client certificate.
//!
//! - **Protocols** (`cluster_protocol`, [`Protocol`]). On one port, TLS picks by ALPN: gRPC on HTTP/2
//!   (`moqui.cluster.v1.Cluster`, [`grpc`]) or `POST /cluster/{topic}` on HTTP/1.1, the same messages and statuses.
//!   gRPC is the default where it is built; `rest` keeps a member to HTTP/1.1, both as listener and as sender. A client
//!   that offers no ALPN, such as a member built before gRPC, gets HTTP/1.1.
//! - **Listener** ([`listen`], `listen.rs`): on the host's runtime (`crate::edge::handle`), in a build with `axum`.
//! - **Senders** ([`https_sender`]): the gRPC sender, with the blocking HTTP/1.1 client here for a peer that does not
//!   choose h2; or the HTTP/1.1 client alone.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use moqui_lib::sync::lock;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use moqui_core::cluster::Sender;
use moqui_core::context::{ErrorKind, ExecutionContextFactory};
use moqui_core::notify::TopicMessage;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use sha2::{Digest, Sha256};

use crate::tls_pin::{Pin, PinVerifier};

#[cfg(feature = "grpc")]
mod grpc;
#[cfg(feature = "axum")]
mod listen;
#[cfg(feature = "grpc")]
pub(crate) mod proto;

#[cfg(feature = "axum")]
pub(crate) use listen::{listen, ClusterEdge};

const DEFAULT_PORT: u16 = 8444;
const BODY_LIMIT: usize = moqui_core::cluster::MAX_CLUSTER_BODY;
const HEADER_LIMIT: usize = 16 * 1024;
const CLIENT_IO: Duration = Duration::from_secs(10);
/// How long a kept-alive connection may sit between exchanges. The listener closes it then, so the sender drops its
/// end too, which also lets go of a member that has left the cluster.
const CONNECTION_IDLE: Duration = Duration::from_secs(60);
/// Topics kept for [`ClusterEdge::posts`], newest last.
const POSTS_KEPT: usize = 256;
/// The first and the longest wait between submits in [`apply_topic_patiently`].
const BUSY_BACKOFF_MIN: Duration = Duration::from_millis(5);
const BUSY_BACKOFF_MAX: Duration = Duration::from_millis(250);

type HostError = moqui_lib::error::Error;
type CoreError = moqui_core::context::Error;

struct Material {
    cert: Vec<u8>,
    key: Vec<u8>,
    digest: [u8; 32],
}

/// `cluster_bind_address`, else every interface: a member on another host must reach this one.
pub(crate) fn bind_address() -> Result<IpAddr, HostError> {
    let raw = moqui_lib::env::get_prop_or_env("cluster_bind_address");
    if raw.trim().is_empty() {
        return Ok(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    }
    raw.trim()
        .parse()
        .map_err(|_| HostError::new(format!("cluster_bind_address {raw} is not an IP address")))
}

/// `cluster_https_port`. Empty is 8444. `0` is ephemeral and is passed through to `bind`.
pub(crate) fn listen_port(raw: &str) -> Result<u16, HostError> {
    if raw.is_empty() {
        return Ok(DEFAULT_PORT);
    }
    raw.parse()
        .map_err(|_| HostError::new("cluster_https_port is not a port"))
}

/// How this member speaks on the cluster port, from `cluster_protocol`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Protocol {
    /// gRPC on HTTP/2 where TLS chose h2, HTTP/1.1 otherwise: the listener offers both, and the sender posts over
    /// gRPC to a peer that chooses h2 and over HTTP/1.1 to one that does not.
    Grpc,
    /// HTTP/1.1 only: the listener offers only `http/1.1`, and the sender posts `POST /cluster/{topic}`.
    Rest,
}

/// `cluster_protocol`: empty is gRPC where this build has it, else HTTP/1.1; `grpc` or `rest` sets it.
///
/// # Errors
///
/// When the value is neither, or it is `grpc` in a build without gRPC.
pub(crate) fn protocol() -> Result<Protocol, HostError> {
    let raw = moqui_lib::env::get_prop_or_env("cluster_protocol");
    match raw.trim() {
        "" if cfg!(feature = "grpc") => Ok(Protocol::Grpc),
        "" | "rest" => Ok(Protocol::Rest),
        "grpc" if cfg!(feature = "grpc") => Ok(Protocol::Grpc),
        "grpc" => Err(HostError::new(
            "cluster_protocol is grpc and gRPC is not built into this host",
        )),
        other => Err(HostError::new(format!("cluster_protocol {other} is not grpc or rest"))),
    }
}

/// The ALPN protocols the listener offers for `protocol`, h2 first.
fn alpn_protocols(protocol: Protocol) -> Vec<Vec<u8>> {
    match protocol {
        Protocol::Grpc => vec![b"h2".to_vec(), b"http/1.1".to_vec()],
        Protocol::Rest => vec![b"http/1.1".to_vec()],
    }
}

/// The HTTPS sender, which the SSH sender also uses for a peer whose base URL is `https://`: over gRPC, with the
/// HTTP/1.1 POST for a peer that does not choose h2, unless `cluster_protocol` is `rest`.
pub(crate) fn https_sender() -> Result<Arc<dyn Sender>, HostError> {
    let material = load_material()?;
    let config = client_config(&material)?;
    let rest: Arc<dyn Sender> = Arc::new(PeerSender {
        config: Arc::clone(&config),
        live: Mutex::new(HashMap::new()),
    });
    match protocol()? {
        Protocol::Rest => Ok(rest),
        Protocol::Grpc => Ok(grpc_sender(&config, rest)),
    }
}

#[cfg(feature = "grpc")]
fn grpc_sender(config: &rustls::ClientConfig, rest: Arc<dyn Sender>) -> Arc<dyn Sender> {
    let mut config = config.clone();
    config.alpn_protocols = alpn_protocols(Protocol::Grpc);
    Arc::new(grpc::GrpcSender::new(Arc::new(config), rest))
}

#[cfg(not(feature = "grpc"))]
fn grpc_sender(_config: &rustls::ClientConfig, rest: Arc<dyn Sender>) -> Arc<dyn Sender> {
    rest
}

/// The shared cluster key `cluster_tls_key`, as DER, for the SSH channel's derived login key.
#[cfg(feature = "ssh")]
pub(crate) fn cluster_key_der() -> Result<Vec<u8>, HostError> {
    let material = load_material()?;
    Ok(material.key)
}

/// One topic message from a peer, by either transport: checked, counted, and applied on a request worker. The answer
/// is an HTTP status: 204 applied, 400 a bad body, 404 an unknown topic, 503 busy, 500 failed.
pub(crate) fn apply_topic(
    factory: &Arc<ExecutionContextFactory>,
    posts: &Mutex<Vec<String>>,
    topic: &str,
    body: &[u8],
) -> u16 {
    match accept_topic(posts, topic, body) {
        Ok(message) => apply_on_pool(factory, topic, &message),
        Err(status) => status,
    }
}

/// [`apply_topic`] for a transport whose sender cannot try again, such as `NOTIFY`. While the request queue is full
/// the submit is tried again with backoff, until `stop` is set or `patience` has passed; only then is 503 returned.
pub(crate) fn apply_topic_patiently(
    factory: &Arc<ExecutionContextFactory>,
    posts: &Mutex<Vec<String>>,
    topic: &str,
    body: &[u8],
    stop: &AtomicBool,
    patience: Duration,
) -> u16 {
    let message = match accept_topic(posts, topic, body) {
        Ok(message) => message,
        Err(status) => return status,
    };
    let until = Instant::now() + patience;
    let mut backoff = BUSY_BACKOFF_MIN;
    loop {
        let status = apply_on_pool(factory, topic, &message);
        let left = until.saturating_duration_since(Instant::now());
        if status != 503 || stop.load(Ordering::Acquire) || left.is_zero() {
            return status;
        }
        thread::sleep(backoff.min(left));
        backoff = (backoff * 2).min(BUSY_BACKOFF_MAX);
    }
}

/// Check and count one topic message. The error is the HTTP status: 404 an unknown topic, 400 a bad body.
fn accept_topic(posts: &Mutex<Vec<String>>, topic: &str, body: &[u8]) -> Result<TopicMessage, u16> {
    if body.len() > moqui_core::cluster::MAX_CLUSTER_BODY {
        return Err(413);
    }
    if !moqui_core::notify::TOPICS.contains(&topic) {
        return Err(404);
    }
    let message = topic_message(body).map_err(|err| {
        tracing::debug!(error = %err, "cluster body was rejected");
        400_u16
    })?;
    // Count before apply so the peer's post returns with this request already visible. Only the newest are kept.
    let mut kept = lock(posts);
    if kept.len() >= POSTS_KEPT {
        kept.remove(0);
    }
    kept.push(topic.to_string());
    Ok(message)
}

fn cluster_topic<'a>(method: &str, path: &'a str) -> Option<&'a str> {
    if method != "POST" {
        return None;
    }
    let topic = path.strip_prefix("/cluster/")?;
    moqui_core::notify::TOPICS.contains(&topic).then_some(topic)
}

fn topic_message(body: &[u8]) -> Result<TopicMessage, String> {
    let value: serde_json::Value = serde_json::from_slice(body).map_err(|err| err.to_string())?;
    let origin_id = value.get("origin_id").and_then(serde_json::Value::as_str).unwrap_or("");
    let message_id = value
        .get("message_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if origin_id.is_empty() || message_id.is_empty() {
        return Err("cluster body needs origin_id and message_id".to_string());
    }
    let body = value.get("body").cloned().unwrap_or(serde_json::Value::Null);
    Ok(TopicMessage {
        origin_id: origin_id.to_string(),
        message_id: message_id.to_string(),
        body,
    })
}

fn apply_on_pool(factory: &Arc<ExecutionContextFactory>, topic: &str, message: &TopicMessage) -> u16 {
    let topic = topic.to_string();
    let message = message.clone();
    let owner = Arc::clone(factory);
    let submitted = factory.request_pool().try_submit(Arc::clone(factory), move || {
        let facade = owner
            .entity()
            .ok_or_else(|| CoreError::new("the entity facade is not installed"))?;
        moqui_core::cluster::apply_remote(&facade, &topic, &message)
    });
    let rx = match submitted {
        Ok(rx) => rx,
        Err(err) if err.kind() == ErrorKind::Unavailable => return 503,
        Err(err) => {
            tracing::warn!(error = %err, "cluster apply was not queued");
            return 500;
        }
    };
    match rx.recv() {
        Ok(Ok(())) => 204,
        Ok(Err(err)) => {
            tracing::warn!(error = %err, "cluster apply failed");
            500
        }
        Err(_) => 500,
    }
}

type PeerStream = rustls::StreamOwned<rustls::ClientConnection, TcpStream>;

struct PeerSender {
    config: Arc<rustls::ClientConfig>,
    /// The kept-alive connection to each peer and when it was last used.
    live: Mutex<HashMap<String, (PeerStream, Instant)>>,
}

impl PeerSender {
    /// The kept connection to `base_url`, when it has not sat idle past [`CONNECTION_IDLE`]. Every connection that has
    /// is dropped, so one to a member that left is not kept.
    fn take_live(&self, base_url: &str) -> Option<PeerStream> {
        let mut live = lock(&self.live);
        live.retain(|_, (_, used)| used.elapsed() < CONNECTION_IDLE);
        live.remove(base_url).map(|(stream, _)| stream)
    }

    fn keep(&self, base_url: &str, stream: PeerStream) {
        lock(&self.live).insert(base_url.to_string(), (stream, Instant::now()));
    }
}

impl Sender for PeerSender {
    fn post(&self, base_url: &str, topic: &str, payload: &[u8]) -> Result<moqui_core::cluster::PostOutcome, CoreError> {
        if payload.len() > moqui_core::cluster::MAX_CLUSTER_BODY {
            return Ok(moqui_core::cluster::PostOutcome::Rejected(413));
        }
        if !topic_segment(topic) {
            return Err(CoreError::new("cluster topic is not a path segment"));
        }
        if let Some(mut stream) = self.take_live(base_url) {
            if let Ok(outcome) = write_post(&mut stream, base_url, topic, payload) {
                self.keep(base_url, stream);
                return Ok(outcome);
            }
            // The peer may close a connection early; a fresh one is tried once before the post fails.
        }
        let mut stream = connect_peer(&self.config, base_url)?;
        let sent = write_post(&mut stream, base_url, topic, payload);
        if sent.is_ok() {
            self.keep(base_url, stream);
        }
        sent
    }
}

fn topic_segment(topic: &str) -> bool {
    !topic.is_empty() && !topic.contains('/') && !topic.contains(' ') && !topic.contains('\r') && !topic.contains('\n')
}

fn connect_peer(
    config: &Arc<rustls::ClientConfig>,
    base_url: &str,
) -> Result<rustls::StreamOwned<rustls::ClientConnection, TcpStream>, CoreError> {
    let (host, port) = split_base(base_url)?;
    let addr = (host.as_str(), port)
        .to_socket_addrs_one()
        .ok_or_else(|| CoreError::new(format!("could not resolve {base_url}")))?;
    let tcp = TcpStream::connect_timeout(&addr, CLIENT_IO)
        .map_err(|err| CoreError::new(format!("could not connect to {base_url}: {err}")))?;
    let _ = tcp.set_read_timeout(Some(CLIENT_IO));
    let _ = tcp.set_write_timeout(Some(CLIENT_IO));
    let _ = tcp.set_nodelay(true);
    let name = server_name(&host)?;
    let conn = rustls::ClientConnection::new(Arc::clone(config), name)
        .map_err(|err| CoreError::new(format!("cluster tls handshake with {base_url} failed: {err}")))?;
    Ok(rustls::StreamOwned::new(conn, tcp))
}

trait OneAddr {
    fn to_socket_addrs_one(&self) -> Option<SocketAddr>;
}

impl OneAddr for (&str, u16) {
    fn to_socket_addrs_one(&self) -> Option<SocketAddr> {
        std::net::ToSocketAddrs::to_socket_addrs(self).ok()?.next()
    }
}

fn write_post(
    stream: &mut rustls::StreamOwned<rustls::ClientConnection, TcpStream>,
    base_url: &str,
    topic: &str,
    payload: &[u8],
) -> Result<moqui_core::cluster::PostOutcome, CoreError> {
    let (host, port) = split_base(base_url)?;
    let head = format!(
        "POST /cluster/{topic} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
        payload.len()
    );
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(payload))
        .and_then(|()| stream.flush())
        .map_err(|err| CoreError::new(format!("cluster post to {base_url} failed: {err}")))?;
    let mut buf = Vec::new();
    let exchange = match read_exchange(stream, &mut buf) {
        Ok(Some(exchange)) => exchange,
        Ok(None) => return Err(CoreError::new(format!("cluster peer {base_url} closed the connection"))),
        Err(IoFail::Timeout) => return Err(CoreError::new(format!("cluster post to {base_url} timed out"))),
        Err(IoFail::Closed) => return Err(CoreError::new(format!("cluster peer {base_url} closed the connection"))),
        Err(IoFail::TooLarge) => return Err(CoreError::new("cluster response is too large")),
        Err(IoFail::Bad(message)) => {
            return Err(CoreError::new(format!("cluster post to {base_url} failed: {message}")))
        }
    };
    let status = status_code(&exchange.start);
    moqui_core::cluster::PostOutcome::from_status(status)
}

fn split_base(base_url: &str) -> Result<(String, u16), CoreError> {
    let rest = base_url.trim().trim_end_matches('/');
    let Some(rest) = rest.strip_prefix("https://") else {
        return Err(CoreError::new("cluster base url is not https"));
    };
    let Some((host, port)) = rest.rsplit_once(':') else {
        return Err(CoreError::new("cluster base url has no port"));
    };
    if host.is_empty() || host.contains('/') {
        return Err(CoreError::new("cluster base url host is empty"));
    }
    let port = port
        .parse::<u16>()
        .map_err(|_| CoreError::new("cluster base url port is not a port"))?;
    Ok((host.to_string(), port))
}

fn server_name(host: &str) -> Result<ServerName<'static>, CoreError> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(ServerName::from(ip));
    }
    ServerName::try_from(host.to_string()).map_err(|_| CoreError::new(format!("cluster host {host} is not a DNS name")))
}

/// A peer's answer to a post: its status line. The body, if any, is read and dropped.
struct Exchange {
    start: String,
}

enum IoFail {
    Timeout,
    Closed,
    TooLarge,
    Bad(String),
}

fn read_exchange(stream: &mut impl Read, buf: &mut Vec<u8>) -> Result<Option<Exchange>, IoFail> {
    loop {
        if let Some(split) = find_header_end(buf) {
            return take_exchange(stream, buf, split);
        }
        if buf.len() > HEADER_LIMIT {
            return Err(IoFail::Bad("cluster headers are too large".to_string()));
        }
        let read = read_some(stream, buf)?;
        if read == 0 && find_header_end(buf).is_none() {
            return Err(IoFail::Closed);
        }
    }
}

fn take_exchange(stream: &mut impl Read, buf: &mut Vec<u8>, split: usize) -> Result<Option<Exchange>, IoFail> {
    let head = String::from_utf8_lossy(&buf[..split]).into_owned();
    let length = content_length(&head)?;
    if length > BODY_LIMIT {
        return Err(IoFail::TooLarge);
    }
    let total = split + 4 + length;
    while buf.len() < total {
        if read_some(stream, buf)? == 0 {
            return Err(IoFail::Closed);
        }
    }
    buf.drain(..total);
    Ok(Some(parse_exchange(&head)?))
}

fn read_some(stream: &mut impl Read, buf: &mut Vec<u8>) -> Result<usize, IoFail> {
    let mut tmp = [0_u8; 4096];
    match stream.read(&mut tmp) {
        Ok(0) => {
            if buf.is_empty() {
                Err(IoFail::Closed)
            } else {
                Ok(0)
            }
        }
        Ok(count) => {
            buf.extend_from_slice(&tmp[..count]);
            Ok(count)
        }
        // With part of a request read too, the connection loop decides how long it may wait.
        Err(err) if err.kind() == std::io::ErrorKind::TimedOut || err.kind() == std::io::ErrorKind::WouldBlock => {
            Err(IoFail::Timeout)
        }
        Err(err) if err.kind() == std::io::ErrorKind::Interrupted => read_some(stream, buf),
        Err(_) => Err(IoFail::Closed),
    }
}

fn parse_exchange(head: &str) -> Result<Exchange, IoFail> {
    let start = head.split("\r\n").next().unwrap_or("").to_string();
    if start.split_whitespace().nth(1).is_none() {
        return Err(IoFail::Bad("cluster status line is empty".to_string()));
    }
    Ok(Exchange { start })
}

fn content_length(head: &str) -> Result<usize, IoFail> {
    for line in head.split("\r\n").skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            return value
                .trim()
                .parse()
                .map_err(|_| IoFail::Bad("cluster content-length is not a number".to_string()));
        }
    }
    // an answer with no body, such as a 204, has none
    Ok(0)
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|window| window == b"\r\n\r\n")
}

fn status_code(start: &str) -> u16 {
    start
        .split_whitespace()
        .nth(1)
        .and_then(|text| text.parse().ok())
        .unwrap_or(0)
}

fn load_material() -> Result<Material, HostError> {
    let cert_b64 = require_configured("cluster_tls_cert")?;
    let key_b64 = require_configured("cluster_tls_key")?;
    let cert = decode_der("cluster_tls_cert", &cert_b64)?;
    let key = decode_der("cluster_tls_key", &key_b64)?;
    let hashed = Sha256::digest(&cert);
    let mut digest = [0_u8; 32];
    digest.copy_from_slice(&hashed);
    Ok(Material { cert, key, digest })
}

fn require_configured(name: &str) -> Result<String, HostError> {
    let value = moqui_lib::env::get_prop_or_env(name);
    if value.is_empty() || value.ends_with("CHANGEME") {
        return Err(HostError::new(format!("{name} is not configured")));
    }
    Ok(value)
}

fn decode_der(name: &str, text: &str) -> Result<Vec<u8>, HostError> {
    STANDARD
        .decode(text.trim())
        .map_err(|_| HostError::new(format!("{name} is not base64 der")))
}

fn private_key(bytes: &[u8]) -> Result<PrivateKeyDer<'static>, HostError> {
    PrivateKeyDer::try_from(bytes.to_vec()).map_err(|_| HostError::new("cluster_tls_key is not a private key"))
}

fn client_config(material: &Material) -> Result<Arc<rustls::ClientConfig>, HostError> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let verifier = fingerprint_verifier(material, &provider);
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|err| HostError::new(format!("cluster tls client: {err}")))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(
            vec![CertificateDer::from(material.cert.clone())],
            private_key(&material.key)?,
        )
        .map_err(|err| HostError::new(format!("cluster tls client certificate: {err}")))?;
    Ok(Arc::new(config))
}

fn server_config(material: &Material) -> Result<rustls::ServerConfig, HostError> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let verifier = fingerprint_verifier(material, &provider);
    rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|err| HostError::new(format!("cluster tls listener: {err}")))?
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![CertificateDer::from(material.cert.clone())],
            private_key(&material.key)?,
        )
        .map_err(|err| HostError::new(format!("cluster tls server certificate: {err}")))
}

fn fingerprint_verifier(material: &Material, provider: &rustls::crypto::CryptoProvider) -> Arc<PinVerifier> {
    PinVerifier::new(Pin::Certificate(material.digest), true, provider)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
