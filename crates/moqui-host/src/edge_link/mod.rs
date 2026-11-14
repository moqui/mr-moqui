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

//! The edge link (plans/archive/Edge-Node.md §5.1): a local node behind NAT dials its edge node's HTTPS port and keeps a link
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
//!   connection, before any TLS, and the SSH and XMPP tunnel ports' go down as byte tunnels ([`tunnel`]).
//! - **Generations.** Each new up connection bumps the local node's link generation on the edge node; the connections of
//!   an older one are closed and their calls refused, so a stale connection cannot answer or acknowledge.
//! - **Calls** (§5.4, E7). An edge node with call media is a media node of its local node's calls: its Hello names its
//!   media address and TURN URLs, its Hello answer carries its TURN secret, and the local node keeps its
//!   `CallMediaNode` row from the link ([`dial`]). A message on a cluster topic for the node at the other end (call
//!   signaling) goes over `Link.Signal` and `LocalLink.Signal` ([`signal`], `moqui_core::edge::signal`).
//! - **Liveness.** HTTP/2 PINGs every 15 s keep NAT mappings open; a lost link is dialed again with a backoff from 1 s to
//!   30 s ([`dial`]); a refused node waits up to 10 minutes.
//! - **Edge clusters** (plans/Edge-Node2.md §4.6). Every edge member's Hello answer and Ping answer list its cluster's
//!   live members with their own link addresses (`edge_link_url`); a local node links to each, keeps the list
//!   ([`members`]) and sends its own work up one of those links.
//!
//! The parts take explicit configuration ([`LocalConfig`], [`PairConfig`], [`EdgeConfig`]) read from properties once,
//! so a test runs an edge end and a local end in one process. [`hook`] is what `org.moqui.edge.EdgeServices` calls
//! through `moqui_core::edge`.

use std::time::Duration;

use moqui_core::edge::{ClusterMemberInfo, EdgeInfo, MemberState};

pub(crate) use crate::grpc_client::H2Channel;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

pub(crate) mod accept;
pub(crate) mod balance;
pub mod dial;
pub(crate) mod down;
mod front;
pub(crate) mod hook;
pub(crate) mod hosts;
pub mod member_key;
mod members;
mod offer;
pub mod pair;
mod probe;
pub(crate) mod proto;
mod queue;
mod replicate;
mod signal;
mod store;
pub(crate) mod tunnel;
pub(crate) mod web;
pub(crate) mod xmpp_domain;

pub(crate) use accept::{Acceptor, Secured};
pub use dial::{connect_up, Dialer, FixedMember, PoolMember, UpLink};
pub use down::{local_web, LocalWeb};
pub(crate) use hook::{start, stopped};
pub use member_key::MemberKey;
pub use pair::{check, new_key, pair, PairConfig, Paired};
pub use probe::{turn_probe, xmpp_probe};
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
/// to and that connection's down token ([`DOWN_TOKEN`] bytes, from its Hello answer): the edge node is the HTTP/2 client.
pub const ROLE_DOWN: u8 = b'D';
/// The length of a down connection's token (plans/Edge-Node2.md §4.11).
pub const DOWN_TOKEN: usize = 16;

/// The request header in which the edge node names the client of a forwarded request (`ip:port`). The local node
/// reads it only on the down connection, and the edge node removes any copy a client sent.
pub(crate) const CLIENT_HEADER: &str = "x-moqui-edge-client";
/// The request header in which the edge node says whether it took a forwarded request over TLS: `https` or `http`.
pub(crate) const PROTO_HEADER: &str = "x-moqui-edge-proto";
/// The `:protocol` of an extended CONNECT (RFC 8441) that opens a byte tunnel down the link.
pub(crate) const TUNNEL_PROTOCOL: &str = "moqui-tunnel";
/// The request header that names a tunnel's kind: [`TUNNEL_HTTPS`], [`TUNNEL_SSH`], [`TUNNEL_XMPP`],
/// [`TUNNEL_XMPPS`] or [`TUNNEL_SMTP`]. The local node picks the listener; the edge node never names an address.
pub(crate) const TUNNEL_HEADER: &str = "x-moqui-tunnel";
/// A passthrough connection: a PROXY v2 header with the client's address, then the client's TLS, which the local node
/// ends with its own certificate.
pub(crate) const TUNNEL_HTTPS: &str = "https";
/// A connection to the edge node's SSH tunnel port, piped to the local node's SSH server.
pub(crate) const TUNNEL_SSH: &str = "ssh";
/// A connection to the edge node's XMPP port (`edge_xmpp_port`, 5222): an XMPP client stream with STARTTLS, which the
/// local node's XMPP server serves and ends the TLS of.
pub(crate) const TUNNEL_XMPP: &str = "xmpp";
/// A connection to the edge node's direct TLS XMPP port (`edge_xmpp_tls_port`, 5223, XEP-0368), likewise.
pub(crate) const TUNNEL_XMPPS: &str = "xmpps";
/// A connection to a front node's SMTP port (an `EdgePort` row, plans/Edge-Node2.md §4.7): a PROXY v2 header with the
/// client's address, then the client's SMTP, which the edge member's own SMTP listener serves (its mail edge).
pub(crate) const TUNNEL_SMTP: &str = moqui_core::edge::ports::SMTP;

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

/// Ok when the edge member's Hello names an id this node can take: a node id, not this node's own (`own`), and, on a
/// link dialed for a member of its edge cluster's list (`expected`), that member's (plans/Edge-Node2.md §4.6), else
/// what to tell the operator. Any member of the edge cluster may be named on a link to the URL this node paired with:
/// every member presents the link key this node pinned (`edge_link_key` is in the cluster's shared `.env`), and the
/// id is learned then (`edge_node_id` becomes the set of them). Whether it is a live member of this node's own cluster
/// is checked apart, with the database: an edge node that took another media node's id would get that node's call
/// signaling and media row (plans/archive/Edge-Node.md §5.4).
pub(crate) fn check_edge_member(expected: Option<&str>, seen: &str, own: &str) -> Result<(), String> {
    if seen.is_empty() {
        return Err("the edge node's Hello names no instance id".to_string());
    }
    if !valid_node_id(seen) {
        return Err(
            "the edge node's Hello names an instance id that is too long or has spaces, control characters or @"
                .to_string(),
        );
    }
    if let Some(misdirected) = misdirected(expected, seen) {
        return Err(misdirected);
    }
    if seen == own {
        return Err("the edge node names this node's own instance id".to_string());
    }
    Ok(())
}

/// Why a link dialed for edge member `expected` reached member `seen`, another one: its listed address is stale or
/// wrong (plans/Edge-Node2.md §4.6). `None` when it reached that member, or was dialed for none.
pub(crate) fn misdirected(expected: Option<&str>, seen: &str) -> Option<String> {
    expected.filter(|expected| *expected != seen).map(|expected| {
        format!("the edge member dialed as {expected} names itself {seen}: its edge_link_url reaches another member")
    })
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
        sip_address: String::new(),
        sip_tls_address: String::new(),
        sip_domain: String::new(),
        sip_registrar: false,
        node_role: String::new(),
        link_url: String::new(),
    }
}

/// A member's state and its cluster's members as the link carries them (plans/Edge-Node2.md §4.4).
pub(crate) fn status_to_proto(state: MemberState, members: &[ClusterMemberInfo]) -> proto::MemberStatus {
    proto::MemberStatus {
        state: state.as_str().to_string(),
        members: members
            .iter()
            .map(|member| proto::ClusterMember {
                instance_id: member.instance_id.clone(),
                heartbeat_age_ms: member.heartbeat_age_ms,
                live: member.live,
                version: member.version.clone(),
            })
            .collect(),
    }
}

/// What a member's status says: its state (`None` for one this end does not know) and its cluster's members, as sent;
/// the caller checks the list as input (`moqui_core::edge::pool::check_member_list`).
pub(crate) fn status_from_proto(status: &proto::MemberStatus) -> (Option<MemberState>, Vec<ClusterMemberInfo>) {
    let members = status
        .members
        .iter()
        .map(|member| ClusterMemberInfo {
            instance_id: member.instance_id.clone(),
            heartbeat_age_ms: member.heartbeat_age_ms,
            live: member.live,
            version: member.version.clone(),
        })
        .collect();
    (MemberState::parse(&status.state), members)
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
        sip_address: hello.sip_address.clone(),
        sip_tls_address: hello.sip_tls_address.clone(),
        sip_domain: hello.sip_domain.clone(),
        sip_registrar: hello.sip_registrar,
        role: hello.node_role.clone(),
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

/// Whether `id` can be a paired node's id: not empty, at most 40 bytes, no whitespace or control characters, and no
/// `@`, which a TURN username puts between a leg and its pairing (`moqui_rtc::turn::PAIRING_SEPARATOR`, E16).
pub(crate) fn valid_node_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_NODE_ID
        && !id.chars().any(|ch| ch.is_whitespace() || ch.is_control() || ch == '@')
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
    /// The port that pipes SSH down a link (`edge_ssh_tunnel_port` on `edge_ssh_tunnel_host`), none when empty: to
    /// the one node linked. A port per pairing is an `EdgePort` row (E16, `tunnel.rs`).
    pub ssh_tunnel: Option<std::net::SocketAddr>,
    /// The ports that pipe XMPP clients down a link: STARTTLS (`edge_xmpp_port`) and direct TLS
    /// (`edge_xmpp_tls_port`), both on `edge_xmpp_host`; each none when empty. Each connection goes to the pairing
    /// whose XMPP domain it names (E16, `tunnel.rs`).
    pub xmpp_tunnel: Option<std::net::SocketAddr>,
    pub xmpp_tls_tunnel: Option<std::net::SocketAddr>,
    /// The page a terminate host answers with while its node's link is down (`edge_offline_page`, else the default).
    pub offline_page: std::sync::Arc<str>,
    /// How long opening a tunnel or forwarding a request head down a link may take.
    pub open_timeout: Duration,
    /// This node has the front set (plans/Edge-Node2.md §4.3): it passes passthrough hosts' connections down links. An
    /// edge node behind a front node (`node_role=edge`) does not.
    pub front: bool,
    /// This node has the edge set (plans/Edge-Node2.md §4.3): the queue, delegated calls, replication, call signaling
    /// and PASS go over its links. A front node (`node_role=front`) has none of them and refuses each.
    pub edge: bool,
    /// Where local nodes dial this member of an edge cluster (`edge_link_url`, plans/Edge-Node2.md §4.6); empty when
    /// they dial only the URL they paired with.
    pub link_url: String,
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
            ssh_tunnel: tunnel_bind(TUNNEL_PORTS[0].0, TUNNEL_PORTS[0].1, TUNNEL_PORTS[0].2),
            xmpp_tunnel: tunnel_bind(TUNNEL_PORTS[1].0, TUNNEL_PORTS[1].1, TUNNEL_PORTS[1].2),
            xmpp_tls_tunnel: tunnel_bind(TUNNEL_PORTS[2].0, TUNNEL_PORTS[2].1, TUNNEL_PORTS[2].2),
            offline_page: std::sync::Arc::from(web::DEFAULT_OFFLINE_PAGE),
            open_timeout: Duration::from_secs(10),
            front: moqui_core::edge::node_role_kind().has_front(),
            edge: moqui_core::edge::node_role_kind().has_edge(),
            link_url: moqui_lib::env::get_prop_or_env(moqui_core::cluster::EDGE_LINK_URL_PROPERTY)
                .trim()
                .to_string(),
        }
    }
}

/// The tunnel ports: the port property, the host property, and what the tunnel is for its errors.
const TUNNEL_PORTS: [(&str, &str, &str); 3] = [
    ("edge_ssh_tunnel_port", "edge_ssh_tunnel_host", "the SSH tunnel"),
    ("edge_xmpp_port", "edge_xmpp_host", "the XMPP tunnel"),
    ("edge_xmpp_tls_port", "edge_xmpp_host", "the XMPP direct TLS tunnel"),
];

/// The startup report's name for the tunnel listeners.
pub(crate) const TUNNEL_LISTENER: &str = "Edge tunnels";

/// A tunnel port's address: the port property on the host property (default all addresses), none when the port is
/// empty, or not a port, or the host not an IP address (logged as `what` being off).
fn tunnel_bind(port_name: &str, host_name: &str, what: &str) -> Option<std::net::SocketAddr> {
    match tunnel_address(port_name, host_name, what)? {
        Ok(bind) => Some(bind),
        Err(reason) => {
            tracing::error!("{reason}");
            None
        }
    }
}

/// [`tunnel_bind`] without the log: None while the port is empty, else the address or why the tunnel is off.
fn tunnel_address(port_name: &str, host_name: &str, what: &str) -> Option<Result<std::net::SocketAddr, String>> {
    let port = moqui_lib::env::get_prop_or_env(port_name);
    let port = port.trim();
    if port.is_empty() {
        return None;
    }
    let Ok(port) = port.parse::<u16>() else {
        return Some(Err(format!("{port_name} {port} is not a port; {what} is off")));
    };
    let host = moqui_lib::env::get_prop_or_env(host_name);
    let ip = match host.trim() {
        "" => std::net::IpAddr::from([0, 0, 0, 0]),
        host => match host.parse() {
            Ok(ip) => ip,
            Err(_) => return Some(Err(format!("{host_name} {host} is not an IP address; {what} is off"))),
        },
    };
    Some(Ok(std::net::SocketAddr::new(ip, port)))
}

/// The tunnel ports an edge or all-in-one node will bind, for the port check before the framework is built; none on
/// a node that takes no links.
pub(crate) fn tunnel_targets() -> Vec<crate::port_check::Target> {
    // the ports are the front set's (plans/Edge-Node2.md §4.3): an edge node behind a front binds none
    if !moqui_core::edge::node_role_kind().has_front() {
        return Vec::new();
    }
    TUNNEL_PORTS
        .iter()
        .filter_map(|(port_name, host_name, what)| {
            let bind = tunnel_address(port_name, host_name, what)?.ok()?;
            Some(crate::port_check::Target::optional(
                TUNNEL_LISTENER,
                port_name,
                bind.ip(),
                bind.port(),
                crate::port_check::Proto::Tcp,
            ))
        })
        .collect()
}

/// A paired local node's link settings: from `edge_node_url`, `edge_node_key` and `edge_node_pin`, or from a test.
/// Keeps an edge node's instance id once a first link pinned it ([`LocalConfig::on_pin`]).
pub type OnPin = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone)]
pub struct LocalConfig {
    /// The edge node: `https://host` or `https://host:port`.
    pub url: String,
    /// This node's private key, base64 PKCS#8 DER.
    pub key: String,
    /// SHA-256 of the edge node's link key, hex.
    pub pin: String,
    /// The edge node's instance ids (`edge_node_id`, comma separated): the one pairing pinned, and the members of its
    /// cluster learned since (plans/Edge-Node2.md §4.6). Empty for a node paired before E7.
    pub edge_id: String,
    /// Told each edge member id a link learns (the hook keeps `edge_node_id` the set of them, and writes the first to
    /// `runtime/.env` when pairing wrote none); none keeps them only for the dialer's life.
    pub on_pin: Option<OnPin>,
    /// This node's instance id: the member its links name (plans/Edge-Node2.md §4.4). The edge node keys its rows by
    /// the pairing its key is for, which every member of a cluster shares.
    pub instance_id: String,
    pub node_name: String,
    /// Where the edge cluster's list is kept (plans/Edge-Node2.md §4.6): a server's `runtime/db/edge_members`; none
    /// keeps it only for the dialer's life.
    pub members_file: Option<std::path::PathBuf>,
    /// This member's own key (plans/Edge-Node2.md §4.11): a server's from `runtime/db/edge_member_key`
    /// ([`MemberKey::load_or_create`]), else one made for this config. Clones of a config share it.
    pub member_key: Option<std::sync::Arc<MemberKey>>,
    /// What this member tells its edge node of itself and its cluster: none for its server's
    /// ([`dial::ServerMember`] with [`LocalConfig::web`]), else this node alone and live.
    pub member: Option<std::sync::Arc<dyn PoolMember>>,
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
    /// The node dialed is a front node (plans/Edge-Node2.md §4.7): this is the front pairing (`front_node_*`), whose
    /// links carry web access and ports only. No outbox, replication, call or SIP rows or TURN secret go up them, and
    /// the down connection takes no queued item, delegated call or PASS call. A link whose Hello names another role
    /// than the pairing expects is refused, so pair again.
    pub front: bool,
    /// On the front pairing of a node that takes links (plans/Edge-Node2.md §4.7): its own `edge_link_url`, which its
    /// Hello names, so the front passes that name's connections (its local nodes' links) through to this member.
    pub link_url: String,
}

impl LocalConfig {
    /// A link to `url` with the default timings.
    pub fn new(url: &str, key: &str, pin: &str, instance_id: &str) -> Self {
        Self {
            url: url.to_string(),
            key: key.to_string(),
            pin: pin.to_string(),
            edge_id: String::new(),
            on_pin: None,
            instance_id: instance_id.to_string(),
            node_name: instance_id.to_string(),
            member: None,
            members_file: None,
            member_key: MemberKey::generate().ok().map(std::sync::Arc::new),
            backoff: Backoff::default(),
            refused_backoff: Backoff::refused(),
            keepalive: Duration::from_secs(15),
            ping_every: Duration::from_secs(15),
            connect_timeout: Duration::from_secs(10),
            web: None,
            front: false,
            link_url: String::new(),
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
        config.edge_id = get(moqui_core::edge::NODE_ID);
        let host = get("webapp_http_host");
        if !host.is_empty() {
            config.node_name = host;
        }
        Some(config)
    }

    /// The front pairing's from the properties (`front_node_url`, `front_node_key`, `front_node_pin`,
    /// `front_node_id`; plans/Edge-Node2.md §4.7), or `None` when `front_node_url` is not set.
    pub(crate) fn front_from_properties(instance_id: &str) -> Option<Self> {
        let get = |name: &str| moqui_lib::env::get_prop_or_env(name).trim().to_string();
        let url = get(moqui_core::edge::FRONT_URL);
        if url.is_empty() {
            return None;
        }
        let mut config = Self::new(
            &url,
            &get(moqui_core::edge::FRONT_KEY),
            &get(moqui_core::edge::FRONT_PIN),
            instance_id,
        );
        config.edge_id = get(moqui_core::edge::FRONT_ID);
        config.front = true;
        config.link_url = get(moqui_core::cluster::EDGE_LINK_URL_PROPERTY);
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
            .field("edge_id", &self.edge_id)
            .field("instance_id", &self.instance_id)
            .field("node_name", &self.node_name)
            .field("member", &self.member.is_some())
            .field("members_file", &self.members_file)
            .field("backoff", &self.backoff)
            .field("refused_backoff", &self.refused_backoff)
            .field("keepalive", &self.keepalive)
            .field("ping_every", &self.ping_every)
            .field("connect_timeout", &self.connect_timeout)
            .field("web", &self.web.is_some())
            .field("front", &self.front)
            .field("link_url", &self.link_url)
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
