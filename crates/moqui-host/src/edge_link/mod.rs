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

//! The edge link (plans/Edge-Node.md §5.1): a local node behind NAT dials its edge node's HTTPS port and keeps a link
//! open, gRPC both ways on HTTP/2.
//!
//! - **TLS.** Two ALPN protocols on the edge node's HTTPS listener, which hands such connections here
//!   ([`accept::Acceptor`]); browsers' `http/1.1` is not touched and never asked for a client certificate.
//!   - [`ALPN_PAIR`] (`moqui-edge-pair/1`): checking and pairing, on the listener's web certificate, which the local node
//!     checks by WebPKI. Over it the edge node tells the fingerprint of its **link key**.
//!   - [`ALPN`] (`moqui-edge/1`): the up and down connections, on a self-signed certificate made from the edge node's
//!     long-lived link key (`edge_link_key`), which the local node pinned at pairing (`edge_node_pin`). A renewed web
//!     certificate does not touch the link.
//!
//!   Each local node has a key of its own, pinned by the edge node at pairing (`moqui.edge.EdgeClient.keyFingerprint`).
//!   Fingerprints are SHA-256 of the public key, so a certificate made again from the same key keeps its pin.
//! - **Connections.** After the handshake the local node sends a one-byte role ([`ROLE_CHECK`], [`ROLE_PAIR`],
//!   [`ROLE_UP`], [`ROLE_DOWN`]) and the edge node answers with a status ([`Answer`]). On **up** the local node is the HTTP/2 client
//!   of the edge node's [`proto::link_server::Link`]; on **down** the roles flip and the edge node is the client of the
//!   local node's [`proto::local_link_server::LocalLink`] and of its web app ([`down`]).
//! - **Work** (§5.2). Delegated service calls and queued items go down (`LocalLink.Delegate`, `LocalLink.Deliver`),
//!   and so do mr-pass's calls of the PASS authority (`moqui.pass.v1.PassAuthority`, §5.5, `crate::pass_authority`),
//!   the local node's outbox and replicated sets go up (`Link.Deliver`, `Link.Replicate`); the rows, receipts and
//!   change notes are `moqui_core::edge`, the delivery loops [`queue`] and [`replicate`].
//! - **Web access** (§5.3). The edge node answers for the paired nodes' public host names ([`hosts`]). A terminate
//!   host's requests go down the link as HTTP/2, WebSockets as extended CONNECT ([`web`]); a passthrough host's
//!   connection, before any TLS, and the SSH tunnel port's go down as byte tunnels ([`tunnel`]).
//! - **Generations.** Each new up connection bumps the local node's link generation on the edge node; the connections of
//!   an older one are closed and their calls refused, so a stale connection cannot answer or acknowledge.
//! - **Calls** (§5.4, E7). An edge node with call media is a media node of its local node's calls: its Hello names its
//!   media address and TURN URLs, its Hello answer carries its TURN secret, and the local node keeps its
//!   `CallMediaNode` row from the link ([`dial`]). A message on a cluster topic for the node at the other end (call
//!   signaling) goes over `Link.Signal` and `LocalLink.Signal` ([`signal`], `moqui_core::edge::signal`).
//! - **Liveness.** HTTP/2 PINGs every 15 s keep NAT mappings open; a lost link is dialed again with a backoff from 1 s to
//!   30 s ([`dial`]); a refused node waits up to 10 minutes.
//!
//! The parts take explicit configuration ([`LocalConfig`], [`PairConfig`], [`EdgeConfig`]) read from properties once,
//! so a test runs an edge end and a local end in one process. [`hook`] is what `org.moqui.impl.EdgeServices` calls
//! through `moqui_core::edge`.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use moqui_core::edge::EdgeInfo;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

pub(crate) mod accept;
pub mod dial;
pub(crate) mod down;
pub(crate) mod hook;
pub(crate) mod hosts;
mod offer;
pub mod pair;
pub(crate) mod proto;
mod queue;
mod replicate;
mod signal;
mod store;
pub(crate) mod tunnel;
pub(crate) mod web;

pub(crate) use accept::{Acceptor, Secured};
pub use dial::{connect_up, Dialer, UpLink};
pub use down::{local_web, LocalWeb};
pub(crate) use hook::{start, stopped};
pub use pair::{check, new_key, pair, PairConfig, Paired};
pub(crate) use web::{Target, WebFront};

/// The ALPN protocol of the link's up and down connections on the edge node's HTTPS port: the link key's certificate.
pub const ALPN: &[u8] = b"moqui-edge/1";
/// The ALPN protocol of checking and pairing: the web certificate, checked by WebPKI.
pub const ALPN_PAIR: &[u8] = b"moqui-edge-pair/1";
/// The link protocol version. Protobuf changes are additive only; a change that cannot be is a new version, and the two
/// ends refuse a link more than one version apart ([`check_versions`]).
pub const LINK_VERSION: u32 = 1;

/// The role byte of a check connection ([`ALPN_PAIR`], no client certificate): only Probe is served.
pub const ROLE_CHECK: u8 = b'C';
/// The role byte of a pairing connection ([`ALPN_PAIR`], the new key's client certificate required): Probe and Pair.
pub const ROLE_PAIR: u8 = b'P';
/// The role byte of an up connection: the local node is the HTTP/2 client.
pub const ROLE_UP: u8 = b'U';
/// The role byte of a down connection, followed by the generation (8 bytes, big-endian) of the up connection it belongs
/// to: the edge node is the HTTP/2 client.
pub const ROLE_DOWN: u8 = b'D';

/// The request header in which the edge node names the client of a forwarded request (`ip:port`). The local node
/// reads it only on the down connection, and the edge node removes any copy a client sent.
pub(crate) const CLIENT_HEADER: &str = "x-moqui-edge-client";
/// The request header in which the edge node says whether it took a forwarded request over TLS: `https` or `http`.
pub(crate) const PROTO_HEADER: &str = "x-moqui-edge-proto";
/// The `:protocol` of an extended CONNECT (RFC 8441) that opens a byte tunnel down the link.
pub(crate) const TUNNEL_PROTOCOL: &str = "moqui-tunnel";
/// The request header that names a tunnel's kind: [`TUNNEL_HTTPS`] or [`TUNNEL_SSH`]. The local node picks the
/// listener; the edge node never names an address.
pub(crate) const TUNNEL_HEADER: &str = "x-moqui-tunnel";
/// A passthrough connection: a PROXY v2 header with the client's address, then the client's TLS, which the local node
/// ends with its own certificate.
pub(crate) const TUNNEL_HTTPS: &str = "https";
/// A connection to the edge node's SSH tunnel port, piped to the local node's SSH server.
pub(crate) const TUNNEL_SSH: &str = "ssh";

/// The `:authority` of the link's gRPC requests. The connection is already the right one, so it only has to be a
/// valid URI.
const ORIGIN: &str = "https://moqui-edge.link";

/// The edge node's answer to the role byte: a code, then the length (2 bytes, big-endian) and text of a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Answer {
    Ok = 0,
    /// The key is not paired with this edge node, or was revoked.
    Refused = 1,
    /// No up connection of that generation (a down connection).
    Stale = 2,
    /// Anything else: a bad role, an error on the edge node.
    Failed = 3,
}

impl Answer {
    fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Ok,
            1 => Self::Refused,
            2 => Self::Stale,
            _ => Self::Failed,
        }
    }
}

/// The longest message an [`Answer`] carries.
const MAX_ANSWER_TEXT: usize = 1024;

pub(crate) async fn write_answer<S: AsyncWrite + Unpin>(
    stream: &mut S,
    answer: Answer,
    message: &str,
) -> std::io::Result<()> {
    let text = message.as_bytes();
    let text = text.get(..MAX_ANSWER_TEXT).unwrap_or(text);
    let length = u16::try_from(text.len()).unwrap_or(u16::MAX);
    let mut frame = Vec::with_capacity(3 + text.len());
    frame.push(answer as u8);
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(text);
    stream.write_all(&frame).await?;
    stream.flush().await
}

pub(crate) async fn read_answer<S: AsyncRead + Unpin>(stream: &mut S) -> std::io::Result<(Answer, String)> {
    let mut head = [0_u8; 3];
    stream.read_exact(&mut head).await?;
    let length = usize::from(u16::from_be_bytes([head[1], head[2]]));
    if length > MAX_ANSWER_TEXT {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the edge node's answer is too long",
        ));
    }
    let mut text = vec![0_u8; length];
    stream.read_exact(&mut text).await?;
    Ok((Answer::from_code(head[0]), String::from_utf8_lossy(&text).into_owned()))
}

/// Ok when `theirs` is at most one link protocol version from `ours`, else what to tell the operator.
pub fn check_versions(ours: u32, theirs: u32, peer: &str) -> Result<(), String> {
    if theirs == 0 {
        return Err(format!("the {peer} sent no link protocol version"));
    }
    if ours.abs_diff(theirs) > 1 {
        let older = if theirs < ours { peer } else { "this node" };
        return Err(format!(
            "the {peer} speaks edge link protocol version {theirs} and this node version {ours}: more than one version \
             apart, so they cannot link; upgrade {older}"
        ));
    }
    Ok(())
}

/// The features compiled into this binary that the other end of a link may use.
pub(crate) fn features() -> Vec<String> {
    let built = [
        ("rtc", cfg!(feature = "rtc")),
        ("sip", cfg!(feature = "sip")),
        ("xmpp", cfg!(feature = "xmpp")),
        ("inbound-smtp", cfg!(feature = "inbound-smtp")),
        ("channels", cfg!(feature = "channels")),
    ];
    built
        .iter()
        .filter(|(_, on)| *on)
        .map(|(name, _)| (*name).to_string())
        .collect()
}

/// This node's Hello.
pub(crate) fn hello(
    instance_id: &str,
    node_name: &str,
    capabilities: &[String],
    public_names: &[String],
) -> proto::Hello {
    proto::Hello {
        binary_version: env!("CARGO_PKG_VERSION").to_string(),
        link_version: LINK_VERSION,
        features: features(),
        capabilities: capabilities.to_vec(),
        public_names: public_names.to_vec(),
        instance_id: instance_id.to_string(),
        node_name: node_name.to_string(),
        media_address: String::new(),
        turn_urls: Vec::new(),
        media_started_ms: 0,
    }
}

/// An edge node's Hello as the screens show it, with the fingerprint of its key.
pub(crate) fn edge_info(hello: &proto::Hello, fingerprint: &str) -> EdgeInfo {
    let checked = check_versions(LINK_VERSION, hello.link_version, "edge node");
    EdgeInfo {
        name: hello.node_name.clone(),
        fingerprint: fingerprint.to_string(),
        binary_version: hello.binary_version.clone(),
        link_version: hello.link_version,
        features: hello.features.clone(),
        capabilities: hello.capabilities.clone(),
        public_names: hello.public_names.clone(),
        instance_id: hello.instance_id.clone(),
        media_address: hello.media_address.clone(),
        turn_urls: hello.turn_urls.clone(),
        media_started_ms: i64::try_from(hello.media_started_ms).unwrap_or(0),
        compatible: checked.is_ok(),
        message: checked.err().unwrap_or_default(),
    }
}

/// The delays between attempts to dial a lost link: `first`, doubling up to `max`, each with jitter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backoff {
    pub first: Duration,
    pub max: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            first: Duration::from_secs(1),
            max: Duration::from_secs(30),
        }
    }
}

impl Backoff {
    /// The waits of a node the edge node refused: dialing again helps only once its operator acts.
    pub fn refused() -> Self {
        Self {
            first: Duration::from_secs(30),
            max: Duration::from_secs(600),
        }
    }
}

/// The longest node id: the `id` type of `moqui.edge.EdgeClient.pairedNodeId`.
const MAX_NODE_ID: usize = 40;

/// Whether `id` can be a paired node's id: not empty, at most 40 bytes, no whitespace or control characters.
pub(crate) fn valid_node_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_NODE_ID && !id.chars().any(|ch| ch.is_whitespace() || ch.is_control())
}

impl Backoff {
    /// The wait before attempt `attempt` (0 is the first after a loss): half the doubled delay plus a random part of
    /// the other half (`jitter` in 0 to 1), so nodes that lost their links together do not dial together.
    pub fn delay(&self, attempt: u32, jitter: f64) -> Duration {
        let doubled = self.first.saturating_mul(1_u32 << attempt.min(16));
        let base = doubled.min(self.max).max(Duration::from_millis(1));
        let half = base / 2;
        half + half.mul_f64(jitter.clamp(0.0, 1.0))
    }
}

/// A number from 0 to 1 that differs from call to call, for [`Backoff::delay`].
pub(crate) fn jitter() -> f64 {
    use std::hash::{BuildHasher as _, Hasher as _};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos()),
    );
    (hasher.finish() % 10_000) as f64 / 10_000.0
}

/// An HTTP/2 connection's sender as a tower service, which the generated gRPC clients take: the up connection on the
/// local node and the down connection on the edge node, each a connection the other end dialed or answered.
#[derive(Clone, Debug)]
pub(crate) struct H2Channel(pub(crate) hyper::client::conn::http2::SendRequest<tonic::body::Body>);

impl tower::Service<http::Request<tonic::body::Body>> for H2Channel {
    type Response = http::Response<hyper::body::Incoming>;
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.0.poll_ready(cx)
    }

    fn call(&mut self, request: http::Request<tonic::body::Body>) -> Self::Future {
        Box::pin(self.0.send_request(request))
    }
}

/// The `:authority` of the link's gRPC calls, the PASS authority's on a down connection too.
pub(crate) fn origin() -> http::Uri {
    http::Uri::from_static(ORIGIN)
}

/// The largest gRPC message either end of the link sends or takes: 16 MiB, well over a queued item's payload
/// (`moqui_core::edge::queue::MAX_PAYLOAD_BYTES`) and a replication batch (`replica::MAX_BATCH_BYTES`).
pub(crate) const MAX_MESSAGE: usize = 16 * 1024 * 1024;

/// The edge node's [`proto::link_client::LinkClient`] on a connection, with the link's message limits.
pub(crate) fn link_client(send: hyper::client::conn::http2::SendRequest<tonic::body::Body>) -> LinkClient {
    proto::link_client::LinkClient::with_origin(H2Channel(send), origin())
        .max_decoding_message_size(MAX_MESSAGE)
        .max_encoding_message_size(MAX_MESSAGE)
}

/// The local node's [`proto::local_link_client::LocalLinkClient`] on a down connection, with the link's message
/// limits.
pub(crate) fn local_link_client(send: hyper::client::conn::http2::SendRequest<tonic::body::Body>) -> LocalLinkClient {
    proto::local_link_client::LocalLinkClient::with_origin(H2Channel(send), origin())
        .max_decoding_message_size(MAX_MESSAGE)
        .max_encoding_message_size(MAX_MESSAGE)
}

pub(crate) type LinkClient = proto::link_client::LinkClient<H2Channel>;
pub(crate) type LocalLinkClient = proto::local_link_client::LocalLinkClient<H2Channel>;

/// The edge node's settings, read from properties once at boot.
#[derive(Clone, Debug)]
pub struct EdgeConfig {
    /// This node's instance id, in its Hello.
    pub instance_id: String,
    /// The name local nodes show for this edge node: `webapp_http_host`, else the instance id.
    pub name: String,
    pub capabilities: Vec<String>,
    /// The public host names this edge node answers for: `webapp_http_host` for now; E2 and later set more.
    pub public_names: Vec<String>,
    /// HTTP/2 PING interval, and how long an unanswered one waits.
    pub keepalive: Duration,
    /// From the TLS handshake to the role byte, from the up connection's start to its Hello, and for a down
    /// connection's Hello.
    pub handshake_timeout: Duration,
    /// The longest a check or pairing connection stays open.
    pub pairing_lifetime: Duration,
    /// The calls a check or pairing connection may make.
    pub pairing_calls: u32,
    /// The Pair calls one address may make in `pair_window`.
    pub pair_attempts: u32,
    pub pair_window: Duration,
    /// The port that pipes SSH down a link (`edge_ssh_tunnel_port` on `edge_ssh_tunnel_host`), none when empty.
    pub ssh_tunnel: Option<std::net::SocketAddr>,
    /// The paired node the SSH tunnel goes to (`edge_ssh_tunnel_node`); empty for the only paired node.
    pub ssh_tunnel_node: String,
    /// The page a terminate host answers with while its node's link is down (`edge_offline_page`, else the default).
    pub offline_page: std::sync::Arc<str>,
    /// How long opening a tunnel or forwarding a request head down a link may take.
    pub open_timeout: Duration,
}

impl EdgeConfig {
    pub(crate) fn from_properties(instance_id: &str) -> Self {
        let host = moqui_lib::env::get_prop_or_env("webapp_http_host").trim().to_string();
        Self {
            instance_id: instance_id.to_string(),
            name: if host.is_empty() {
                instance_id.to_string()
            } else {
                host.clone()
            },
            capabilities: Vec::new(),
            public_names: (!host.is_empty()).then_some(host).into_iter().collect(),
            keepalive: Duration::from_secs(15),
            handshake_timeout: Duration::from_secs(10),
            pairing_lifetime: Duration::from_secs(30),
            pairing_calls: 4,
            pair_attempts: 10,
            pair_window: Duration::from_secs(600),
            ssh_tunnel: ssh_tunnel(),
            ssh_tunnel_node: moqui_lib::env::get_prop_or_env("edge_ssh_tunnel_node")
                .trim()
                .to_string(),
            offline_page: std::sync::Arc::from(web::DEFAULT_OFFLINE_PAGE),
            open_timeout: Duration::from_secs(10),
        }
    }
}

/// `edge_ssh_tunnel_port` on `edge_ssh_tunnel_host` (default all addresses), none when the port is empty or not a
/// port.
fn ssh_tunnel() -> Option<std::net::SocketAddr> {
    let port = moqui_lib::env::get_prop_or_env("edge_ssh_tunnel_port");
    let port = port.trim();
    if port.is_empty() {
        return None;
    }
    let Ok(port) = port.parse::<u16>() else {
        tracing::error!("edge_ssh_tunnel_port {port} is not a port; the SSH tunnel is off");
        return None;
    };
    let host = moqui_lib::env::get_prop_or_env("edge_ssh_tunnel_host");
    let ip = match host.trim() {
        "" => std::net::IpAddr::from([0, 0, 0, 0]),
        host => match host.parse() {
            Ok(ip) => ip,
            Err(_) => {
                tracing::error!("edge_ssh_tunnel_host {host} is not an IP address; the SSH tunnel is off");
                return None;
            }
        },
    };
    Some(std::net::SocketAddr::new(ip, port))
}

/// A paired local node's link settings: from `edge_node_url`, `edge_node_key` and `edge_node_pin`, or from a test.
#[derive(Clone)]
pub struct LocalConfig {
    /// The edge node: `https://host` or `https://host:port`.
    pub url: String,
    /// This node's private key, base64 PKCS#8 DER.
    pub key: String,
    /// SHA-256 of the edge node's link key, hex.
    pub pin: String,
    /// This node's instance id, which the edge node keys its rows by.
    pub instance_id: String,
    pub node_name: String,
    pub backoff: Backoff,
    /// The waits after the edge node refused this node.
    pub refused_backoff: Backoff,
    /// HTTP/2 PING interval, and how long an unanswered one waits.
    pub keepalive: Duration,
    /// How often a Ping on the up connection measures the round trip.
    pub ping_every: Duration,
    /// TCP connect and TLS handshake, and each call of the link's own.
    pub connect_timeout: Duration,
    /// What the down connection serves besides the link's own calls: this node's web app and its listeners for
    /// tunnels ([`local_web`]). With none, a forwarded request is answered 503 and a tunnel refused, and the link
    /// carries no queue, delegated calls or replication (their server is the web app's).
    pub web: Option<LocalWeb>,
}

impl LocalConfig {
    /// A link to `url` with the default timings.
    pub fn new(url: &str, key: &str, pin: &str, instance_id: &str) -> Self {
        Self {
            url: url.to_string(),
            key: key.to_string(),
            pin: pin.to_string(),
            instance_id: instance_id.to_string(),
            node_name: instance_id.to_string(),
            backoff: Backoff::default(),
            refused_backoff: Backoff::refused(),
            keepalive: Duration::from_secs(15),
            ping_every: Duration::from_secs(15),
            connect_timeout: Duration::from_secs(10),
            web: None,
        }
    }

    /// From the properties, or `None` when `edge_node_url` is not set (this node is not paired).
    pub(crate) fn from_properties(instance_id: &str) -> Option<Self> {
        let get = |name: &str| moqui_lib::env::get_prop_or_env(name).trim().to_string();
        let url = get(moqui_core::edge::NODE_URL);
        if url.is_empty() {
            return None;
        }
        let mut config = Self::new(
            &url,
            &get(moqui_core::edge::NODE_KEY),
            &get(moqui_core::edge::NODE_PIN),
            instance_id,
        );
        let host = get("webapp_http_host");
        if !host.is_empty() {
            config.node_name = host;
        }
        Some(config)
    }
}

impl std::fmt::Debug for LocalConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalConfig")
            .field("url", &self.url)
            .field("key", &"<redacted>")
            .field("pin", &self.pin)
            .field("instance_id", &self.instance_id)
            .field("node_name", &self.node_name)
            .field("backoff", &self.backoff)
            .field("refused_backoff", &self.refused_backoff)
            .field("keepalive", &self.keepalive)
            .field("ping_every", &self.ping_every)
            .field("connect_timeout", &self.connect_timeout)
            .field("web", &self.web.is_some())
            .finish()
    }
}

/// Why a link could not be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkError {
    /// The edge node refused this node: its key is not paired or was revoked, or the versions are too far apart.
    /// Dialing again does not help until the operator acts.
    Refused(String),
    /// Anything else: the network, a timeout, the edge node down.
    Failed(String),
}

impl LinkError {
    pub fn message(&self) -> &str {
        match self {
            Self::Refused(message) | Self::Failed(message) => message,
        }
    }
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
