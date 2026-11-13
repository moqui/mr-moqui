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

//! The SIP user agent and B2BUA toward trunk providers (plans/archive/Comms-Framework.md §5.2; D-V1: rsipstack), on Tokio.
//!
//! [`Ua::start`] binds the listeners (UDP, TCP, TLS) and serves them; the host hears what happens through a channel of
//! [`UaEvent`]s and acts through [`Ua`]'s methods. It runs no business logic: whether a call may ring a user, which
//! trunk a number uses and whether a destination is allowed are the voice services', asked by the host. What is checked
//! here is what protects the listener and the trunks whatever the services say:
//!
//! - **Who may call in.** An INVITE is taken from a trunk's signaling addresses (its host's addresses and the
//!   `inbound` ones its profile lists), or, for a PBX trunk only ([`TrunkConfig::inbound_digest`]), with a digest
//!   answer for its username. A provider trunk's credentials are its account's at the provider, so they never open the
//!   listener: anyone could guess them online. Anything else is challenged ([`crate::digest::NonceBook`]: expiring
//!   nonces, nonce counts that cannot be replayed), and each failed answer counts against its source address
//!   ([`crate::guard`]): at the limit everything from that address is dropped unanswered, before a transaction is made.
//!   No account is locked. An `inbound` range wider than /8 (IPv4) or /32 (IPv6) is ignored.
//! - **Floods.** New requests are held to a rate per source, and dropped while the endpoint runs its most
//!   transactions; stream connections past their cap are closed ([`crate::guard::RequestGate`]). A TLS client is
//!   counted from its accept: at most [`MAX_TLS_HANDSHAKES`] handshakes at once, [`TLS_HANDSHAKES_PER_SOURCE`] from one
//!   source, each within [`TLS_HANDSHAKE`] or closed (plans/archive/High-Level-Review.md HL-031); this UA accepts TLS itself
//!   and hands the stream to rsipstack, whose own TLS listener has no handshake deadline.
//! - **Credentials.** A trunk's digest credentials answer only its realm and the algorithms its profile allows
//!   ([`crate::digest::answer`]); a challenge for anything else fails the registration or the call, with the reason.
//! - **Outbound.** CANCEL while ringing, forked answers (the first wins, the others are ACKed and ended with BYE) and
//!   a 2xx that crosses a CANCEL (ACK and BYE) are rsipstack's dialog layer; [`crate::race`] decides the outcome the
//!   host is told, and the ring timeout.
//! - **Trunk hosts.** A trunk's host is looked up by SRV when it has no port ([`crate::resolve`]); registration, pings
//!   and calls go to its targets in order and fail over to the next, and calls in are taken from each. TLS names the
//!   trunk's host, never an SRV target.
//! - **Phones (V4).** With [`UaConfig::registrar`], desk and soft phones register here, ring over the flows they
//!   opened, and place calls signed with their credentials (`ua_phones.rs`).
//!
//! Each leg has the id the host gives it (the call's `callLegId`). Hold is a re-INVITE either way: the other side's
//! comes to the host as [`UaEvent::Reoffer`] for its answer; the host's goes out with [`Ua::reinvite`].

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use moqui_lib::sync::lock;
use rsipstack::dialog::dialog::{Dialog, DialogState, DialogStateReceiver, TerminatedReason};
use rsipstack::dialog::dialog_layer::DialogLayer;
use rsipstack::dialog::invitation::InviteOption;
use rsipstack::dialog::invite_dialog::InviteDialog;
use rsipstack::sip::prelude::{HeadersExt, ToTypedHeader};
use rsipstack::sip::{Header, Method, Request, Response, SipMessage, StatusCode, StatusCodeKind, Uri};
use rsipstack::transaction::endpoint::{Endpoint, EndpointInner, TransportEventInspector};
use rsipstack::transaction::key::{TransactionKey, TransactionRole};
use rsipstack::transaction::transaction::Transaction;
use rsipstack::transaction::{make_tag, random_text};
use rsipstack::transport::connection::TransportEvent;
use rsipstack::transport::tcp_listener::TcpListenerConnection;
use rsipstack::transport::tls::TlsConnection;
use rsipstack::transport::udp::UdpConnection;
use rsipstack::transport::{SipAddr, SipConnection, TlsConfig, TransportLayer};
use rsipstack::EndpointBuilder;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::digest::{self, Algorithm, Authorization, Challenge, Credentials, NonceBook, Verdict};
use crate::guard::{Admit, Failure, GuardPolicy, RequestGate, RequestLimits, SourceGuard};
use crate::race::{self, Race};
use crate::registrar::{Binding, Registrar};
use crate::resolve::{self, Resolver, SystemResolver};

#[path = "ua_phones.rs"]
mod phones;
pub use phones::{aor_key, BindingChange, DeviceDirectory, DeviceRing, PhoneDevice, RegistrarConfig};
use phones::{Caller, Phones};

/// How long a challenge's nonce lasts.
const NONCE_SECONDS: u64 = 300;
/// How long a TLS client has from its accept to the end of its handshake ([`UaConfig::tls_handshake`] in a test).
pub const TLS_HANDSHAKE: Duration = Duration::from_secs(10);
/// TLS handshakes under way at once; a connection past it is closed at once.
pub const MAX_TLS_HANDSHAKES: usize = 128;
/// Of [`MAX_TLS_HANDSHAKES`], those from one source (an IPv6 /64 is one).
pub const TLS_HANDSHAKES_PER_SOURCE: usize = 8;
/// How long the host has to decide an incoming call.
const DECIDE_WAIT: Duration = Duration::from_secs(10);
/// The registration interval asked of a trunk, and the longest wait after a failure.
const REGISTER_EXPIRES: u32 = 300;
const MAX_BACKOFF: Duration = Duration::from_secs(300);
/// How long a REGISTER waits for its answer.
const REGISTER_WAIT: Duration = Duration::from_secs(32);
/// How often a trunk that does not register is pinged with OPTIONS, unless [`UaConfig::trunk_ping`] says otherwise.
const TRUNK_PING: Duration = Duration::from_secs(60);
/// How long an OPTIONS ping waits for its answer, at most.
const PING_WAIT: Duration = Duration::from_secs(8);
/// The OPTIONS pings in a row with no answer before a trunk is told failed: a failed trunk is left out of the emergency
/// readiness and the calls' capabilities, so one lost ping must not do that.
const PING_FAILURES: u32 = 3;
/// The early answers kept at most, and how long one waits for its leg.
const MAX_EARLY: usize = 1_000;
const EARLY_TTL: Duration = Duration::from_secs(30);
/// The widest `inbound` ranges taken: an IPv4 /8, an IPv6 /32.
const MIN_V4_PREFIX: u32 = 8;
const MIN_V6_PREFIX: u32 = 32;
/// How long a TCP or TLS connection to one of a trunk's targets may take before the next is tried.
const CONNECT_WAIT: Duration = Duration::from_secs(5);

/// A signaling transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Udp,
    Tcp,
    Tls,
}

impl Transport {
    pub fn parse(text: &str) -> Option<Transport> {
        match text.trim().to_ascii_lowercase().as_str() {
            "udp" => Some(Transport::Udp),
            "tcp" => Some(Transport::Tcp),
            "tls" => Some(Transport::Tls),
            _ => None,
        }
    }

    fn sip(self) -> rsipstack::sip::Transport {
        match self {
            Transport::Udp => rsipstack::sip::Transport::Udp,
            Transport::Tcp => rsipstack::sip::Transport::Tcp,
            Transport::Tls => rsipstack::sip::Transport::Tls,
        }
    }

    pub(crate) fn default_port(self) -> u16 {
        if self == Transport::Tls {
            5061
        } else {
            5060
        }
    }

    fn param(self) -> &'static str {
        match self {
            Transport::Udp => "udp",
            Transport::Tcp => "tcp",
            Transport::Tls => "tls",
        }
    }
}

/// A trunk, as the voice services give it (`get#EngineConfig`).
#[derive(Debug, Clone)]
pub struct TrunkConfig {
    pub id: String,
    pub host: String,
    /// `None`: the ports of the host's SRV records, else the transport's default ([`crate::resolve`]).
    pub port: Option<u16>,
    pub transport: Transport,
    pub credentials: Credentials,
    /// The domain of From and To when it is not the host.
    pub domain: Option<String>,
    pub register: bool,
    /// Addresses (or CIDR ranges) the provider sends calls from, besides the host's own.
    pub inbound: Vec<String>,
    /// Calls in may sign in with this trunk's digest credentials from any address: a PBX's profile only. A provider's
    /// calls come only from its addresses (or to its registration).
    pub inbound_digest: bool,
    /// Its media must be SRTP.
    pub srtp: bool,
}

impl TrunkConfig {
    fn port(&self) -> u16 {
        self.port.unwrap_or(self.transport.default_port())
    }

    /// A number at the trunk: its host with any port that is not the transport's default, and the transport.
    fn uri(&self, number: &str) -> String {
        let host = if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let port = if self.port() == self.transport.default_port() {
            String::new()
        } else {
            format!(":{}", self.port())
        };
        let transport = if self.transport == Transport::Udp {
            String::new()
        } else {
            format!(";transport={}", self.transport.param())
        };
        format!("sip:{number}@{host}{port};user=phone{transport}")
    }

    fn domain(&self) -> &str {
        self.domain.as_deref().filter(|d| !d.is_empty()).unwrap_or(&self.host)
    }

    fn has_credentials(&self) -> bool {
        !self.credentials.username.is_empty() && !self.credentials.password.is_empty()
    }
}

/// Where the UA listens, and how it introduces itself.
#[derive(Clone, Default)]
pub struct UaConfig {
    pub udp: Option<SocketAddr>,
    pub tcp: Option<SocketAddr>,
    /// The TLS listener with its certificate chain and key, PEM.
    pub tls: Option<(SocketAddr, Vec<u8>, Vec<u8>)>,
    /// The host name or address in Contact headers (the bound address by default).
    pub public_host: Option<String>,
    /// The CA certificates (PEM) for TLS to trunks; without them a trunk's certificate cannot be verified.
    pub ca_pem: Option<Vec<u8>>,
    /// The realm of this UA's challenges.
    pub realm: String,
    pub guard: GuardPolicy,
    pub limits: RequestLimits,
    pub user_agent: String,
    /// The registrar for desk and soft phones (V4); `None` keeps it off, and a REGISTER is refused.
    pub registrar: Option<RegistrarConfig>,
    /// How often a trunk that does not register is pinged with OPTIONS, so its status shows it up or down (60 seconds
    /// when `None`).
    pub trunk_ping: Option<Duration>,
    /// How long a TLS client has to finish its handshake ([`TLS_HANDSHAKE`] when `None`).
    pub tls_handshake: Option<Duration>,
    /// The DNS lookups of trunk hosts ([`SystemResolver`] when `None`; fixed records in a test).
    pub resolver: Option<Arc<dyn Resolver>>,
}

/// The addresses the listeners bound.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Bound {
    pub udp: Option<SocketAddr>,
    pub tcp: Option<SocketAddr>,
    pub tls: Option<SocketAddr>,
}

/// A trunk's registration state, or for a trunk that does not register its answer to OPTIONS pings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrunkState {
    Registered,
    /// A trunk that does not register answered an OPTIONS ping (any final answer: the server is there).
    Reachable,
    Failed(String),
    Unregistered,
}

/// Why a leg ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// The other side hung up.
    Bye,
    /// This side hung up.
    Hangup,
    /// The caller gave up before an answer (either side).
    Canceled,
    /// It rang out.
    TimedOut,
    /// 486 Busy Here: this side is busy (a phone on another call); the call may still be answered elsewhere.
    Busy,
    /// 603 Decline or 600 Busy Everywhere: the person refused the call, on every device (V3).
    Declined,
    /// Refused or failed with this status, or with none.
    Failed(Option<u16>),
}

/// An incoming call for the host to decide.
#[derive(Debug)]
pub struct IncomingCall {
    pub trunk_id: String,
    /// The number called (the Request-URI's user part).
    pub to: String,
    pub from: String,
    pub from_name: Option<String>,
    pub call_id: String,
    /// The caller's SDP offer.
    pub offer: String,
    pub source: SocketAddr,
    /// A registered phone's call (its `sipDeviceId`): `trunk_id` is empty and `to` is the number or user it dialed.
    pub device: Option<String>,
    pub reply: oneshot::Sender<Decision>,
}

/// The host's decision on an incoming call.
#[derive(Debug)]
pub enum Decision {
    /// Ring (180) as this leg; [`Ua::answer`] sends the answer SDP with the 200 later.
    Ring {
        leg: String,
        answer: String,
    },
    Reject(u16),
}

/// What happened, for the host.
#[derive(Debug)]
pub enum UaEvent {
    Incoming(IncomingCall),
    /// An outbound leg rings at the other end.
    Ringing {
        leg: String,
    },
    /// An outbound leg was answered, with the other side's SDP answer.
    Answered {
        leg: String,
        answer: String,
    },
    /// A leg ended, and how; `code` is the SIP status of a failure.
    Ended {
        leg: String,
        reason: EndReason,
    },
    /// The other side's re-INVITE (hold, resume, a new address): the host replies with its answer, or `None` to
    /// refuse it (488).
    Reoffer {
        leg: String,
        offer: String,
        reply: oneshot::Sender<Option<String>>,
    },
    TrunkStatus {
        trunk: String,
        state: TrunkState,
    },
    /// A failed digest from `source`; `blocked` when it just blocked the address.
    AuthFailure {
        source: IpAddr,
        username: String,
        blocked: bool,
    },
    /// A phone's binding was added, refreshed or removed (it expired, it unregistered, its flow closed, or its device
    /// was rotated or revoked).
    Binding {
        change: BindingChange,
        binding: Binding,
    },
    /// A leg's far end sent REFER (a desk phone's blind transfer): the user part of its `Refer-To`, a username or a
    /// number. The host answers `reply` with whether the transfer was taken, before the REFER is answered: 202 and a
    /// NOTIFY of `200 OK` when it was, 403 when it was refused or no answer came in time.
    Refer {
        leg: String,
        target: String,
        reply: oneshot::Sender<bool>,
    },
    /// A device leg ([`Ua::ring_device`]) was answered by one of its bindings, which signals from `source`.
    DeviceAnswered {
        leg: String,
        answer: String,
        source: SocketAddr,
    },
}

/// An outbound call for a trunk.
#[derive(Debug, Clone)]
pub struct DialRequest {
    pub leg: String,
    pub trunk: String,
    /// The number called, E.164 (or an emergency number as dialed).
    pub to: String,
    /// The number presented, E.164.
    pub from: String,
    pub from_name: Option<String>,
    pub offer: String,
    pub ring_timeout: Duration,
    /// Extra headers, as `(name, value)` (an emergency call's callback and location, for example).
    pub headers: Vec<(String, String)>,
}

enum LegKind {
    Outbound,
    Inbound,
}

struct Leg {
    kind: LegKind,
    dialog: Option<InviteDialog>,
    /// The host hung up before an answer.
    give_up: Option<oneshot::Sender<()>>,
    /// An inbound leg's answer SDP for its 200.
    answer: Option<String>,
    answered: bool,
}

struct TrunkEntry {
    config: TrunkConfig,
    /// Where its host resolved to, in the order they are tried ([`resolve::targets`]): its calls go there, and calls
    /// in are taken from each of their addresses.
    targets: Vec<SocketAddr>,
    stop: CancellationToken,
}

struct Inner {
    endpoint: Arc<EndpointInner>,
    dialogs: Arc<DialogLayer>,
    events: mpsc::UnboundedSender<UaEvent>,
    trunks: Mutex<HashMap<String, TrunkEntry>>,
    legs: Mutex<HashMap<String, Leg>>,
    /// Answers given before their leg's decision arrived, with when; past [`EARLY_TTL`] they are dropped.
    early: Mutex<HashMap<String, Instant>>,
    guard: Arc<Mutex<SourceGuard>>,
    nonces: Mutex<NonceBook>,
    /// The registrar's other realms (lower case), each with its own nonces: device domains besides the configured
    /// realm ([`Ua::set_device_realms`]).
    device_realms: Mutex<HashMap<String, NonceBook>>,
    bound: Bound,
    public_host: Option<String>,
    stop: CancellationToken,
    /// The registrar, when it is on.
    phones: Option<Phones>,
    /// How often a trunk that does not register is pinged.
    trunk_ping: Duration,
    resolver: Arc<dyn Resolver>,
    /// The CA certificates for TLS to trunks.
    ca_pem: Option<Vec<u8>>,
    /// The TLS connections opened to trunks, by address and the host name their handshake named.
    tls_flows: Mutex<HashMap<(SocketAddr, String), SipConnection>>,
}

/// The running user agent. Dropping it does not stop it: call [`Ua::shutdown`].
#[derive(Clone)]
pub struct Ua {
    inner: Arc<Inner>,
}

/// Drops what blocked sources send, before rsipstack makes a transaction of it.
/// Drops what a blocked source sends, and holds new requests and stream connections to their limits
/// ([`RequestGate`]), before rsipstack makes a transaction of them.
struct BlockInspector {
    guard: Arc<Mutex<SourceGuard>>,
    gate: Arc<Mutex<RequestGate>>,
    /// The endpoint, once built, for its running transactions.
    endpoint: Arc<std::sync::OnceLock<std::sync::Weak<EndpointInner>>>,
    /// The registrar's bindings, which end with the stream connection they came over.
    phones: Option<(Arc<Mutex<Registrar>>, mpsc::UnboundedSender<UaEvent>)>,
}

/// A request outside a dialog (no To tag), other than the ACK and CANCEL of one this side already has.
fn is_new_request(message: &SipMessage) -> bool {
    let SipMessage::Request(request) = message else {
        return false;
    };
    if matches!(request.method, Method::Ack | Method::Cancel) {
        return false;
    }
    request
        .to_header()
        .map(|to| !to.value().contains(";tag="))
        .unwrap_or(true)
}

#[async_trait::async_trait]
impl TransportEventInspector for BlockInspector {
    async fn handle(&self, event: TransportEvent) -> Option<TransportEvent> {
        match &event {
            TransportEvent::Incoming(message, _, from) => {
                let Ok(addr) = from.get_socketaddr() else {
                    return Some(event);
                };
                let now = Instant::now();
                if lock(&self.guard).is_blocked(addr.ip(), now) {
                    return None;
                }
                if is_new_request(message) {
                    let running = self
                        .endpoint
                        .get()
                        .and_then(std::sync::Weak::upgrade)
                        .map(|e| e.get_stats().running_transactions)
                        .unwrap_or(0);
                    match lock(&self.gate).admit(addr.ip(), running, now) {
                        Admit::Pass => {}
                        Admit::RateLimited => {
                            tracing::debug!("SIP: {} sends past its rate, dropped", addr.ip());
                            return None;
                        }
                        Admit::Busy => {
                            tracing::warn!("SIP: {running} transactions running, a new request from {addr} dropped");
                            return None;
                        }
                    }
                }
            }
            TransportEvent::New(connection) if connection.is_reliable() => {
                if !lock(&self.gate).connection_opened(remote_ip(connection)) {
                    tracing::warn!("SIP: too many connections, {} closed", connection.get_addr());
                    let connection = connection.clone();
                    tokio::spawn(async move {
                        let _ = connection.close().await;
                    });
                }
            }
            TransportEvent::Closed(connection) if connection.is_reliable() => {
                lock(&self.gate).connection_closed(remote_ip(connection));
                if let Some((registrar, events)) = &self.phones {
                    phones::connection_closed(registrar, events, connection);
                }
            }
            _ => {}
        }
        Some(event)
    }
}

/// The address a stream connection is from.
fn remote_ip(connection: &SipConnection) -> Option<IpAddr> {
    connection
        .get_remote_addr()
        .and_then(|remote| remote.get_socketaddr().ok())
        .map(|addr| addr.ip())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn sip_err(err: impl std::fmt::Display) -> String {
    err.to_string()
}

/// Whether `ip` is `entry` (an address or a CIDR range).
/// A range wider than /8 (IPv4) or /32 (IPv6), `0.0.0.0/0` and `::/0` among them, matches nothing: it would take calls
/// from most of the internet without a digest. The voice services refuse such an entry when it is saved.
fn matches_address(ip: IpAddr, entry: &str) -> bool {
    let Some((net, bits)) = inbound_range(entry) else {
        return false;
    };
    match (ip, net) {
        (IpAddr::V4(a), IpAddr::V4(b)) => {
            let mask = u32::MAX << (32 - bits);
            u32::from(a) & mask == u32::from(b) & mask
        }
        (IpAddr::V6(a), IpAddr::V6(b)) => {
            let mask = u128::MAX << (128 - bits);
            u128::from(a) & mask == u128::from(b) & mask
        }
        _ => false,
    }
}

/// An `inbound` entry as an address and prefix length, or `None` when it is not one or is too wide.
pub fn inbound_range(entry: &str) -> Option<(IpAddr, u32)> {
    let entry = entry.trim();
    let (net, bits) = match entry.split_once('/') {
        Some((net, bits)) => (net, Some(bits.trim().parse::<u32>().ok()?)),
        None => (entry, None),
    };
    let net = net.trim().parse::<IpAddr>().ok()?;
    let (most, least) = match net {
        IpAddr::V4(_) => (32, MIN_V4_PREFIX),
        IpAddr::V6(_) => (128, MIN_V6_PREFIX),
    };
    let bits = bits.unwrap_or(most);
    (least..=most).contains(&bits).then_some((net, bits))
}

fn header_values(headers: &rsipstack::sip::Headers, name: &str) -> Vec<String> {
    headers
        .iter()
        .filter(|h| h.name().eq_ignore_ascii_case(name))
        .map(|h| h.value().to_string())
        .collect()
}

/// The target of a REFER's `Refer-To` (a blind transfer): the user part of its SIP or tel URI, a username or a number;
/// `None` for one with `Replaces` (an attended transfer, not taken) or no user.
pub fn refer_target(value: &str) -> Option<String> {
    let target = rsipstack::sip::event_packages::parse_refer_to(value);
    if target.replaces.is_some() {
        return None;
    }
    let uri = target.uri.trim();
    let rest = ["sips:", "sip:", "tel:"]
        .iter()
        .find_map(|scheme| {
            uri.get(..scheme.len())
                .filter(|s| s.eq_ignore_ascii_case(scheme))
                .map(|_| &uri[scheme.len()..])
        })
        .unwrap_or(uri);
    let user = rest.split(['@', ';', '?']).next().unwrap_or("").trim();
    (!user.is_empty()).then(|| user.to_string())
}

fn user_of(uri: &Uri) -> String {
    uri.auth.as_ref().map(|a| a.user.clone()).unwrap_or_default()
}

fn status_code(code: u16) -> StatusCode {
    StatusCode::from(code)
}

/// The SIP UDP socket, answering a phone's STUN Binding keepalive (RFC 5626 §4.4.2, [`crate::stun`]) itself and handing
/// every other datagram to the SIP stack.
struct StunAnswering(tokio::net::UdpSocket);

#[async_trait::async_trait]
impl rsipstack::platform::net::UdpSocket for StunAnswering {
    async fn recv_from(&self, buf: &mut [u8]) -> rsipstack::Result<(usize, SocketAddr)> {
        loop {
            let (len, from) = self.0.recv_from(buf).await?;
            match crate::stun::binding_response(&buf[..len], from) {
                Some(answer) => {
                    if let Err(err) = self.0.send_to(&answer, from).await {
                        tracing::debug!("SIP UDP: the STUN keepalive answer to {from} was not sent: {err}");
                    }
                }
                None => return Ok((len, from)),
            }
        }
    }

    async fn send_to(&self, buf: &[u8], addr: SocketAddr) -> rsipstack::Result<usize> {
        Ok(self.0.send_to(buf, addr).await?)
    }

    fn local_addr(&self) -> rsipstack::Result<SocketAddr> {
        Ok(self.0.local_addr()?)
    }
}

impl Ua {
    /// Binds the listeners that are set and starts serving them. A listener that does not bind is an error: the host
    /// logs it and starts without SIP.
    pub async fn start(config: UaConfig, events: mpsc::UnboundedSender<UaEvent>) -> Result<Ua, String> {
        crate::install_crypto();
        let stop = CancellationToken::new();
        let transport_layer = TransportLayer::new(stop.child_token());
        let mut bound = Bound::default();
        if let Some(addr) = config.udp {
            // a UDP phone's STUN keepalive (RFC 5626) is answered on this socket before the SIP stack sees it
            let socket = tokio::net::UdpSocket::bind(addr)
                .await
                .map_err(|e| format!("SIP UDP {addr}: {e}"))?;
            let socket: Arc<dyn rsipstack::platform::net::UdpSocket> = Arc::new(StunAnswering(socket));
            let udp = UdpConnection::from_socket(socket, None, Some(stop.child_token()))
                .await
                .map_err(|e| format!("SIP UDP {addr}: {e}"))?;
            bound.udp = udp.get_addr().get_socketaddr().ok();
            transport_layer.add_transport(udp.into());
        }
        if let Some(addr) = config.tcp {
            let addr = free_port(addr).await?;
            let tcp = TcpListenerConnection::new(addr, None).await.map_err(sip_err)?;
            bound.tcp = Some(addr);
            transport_layer.add_transport(SipConnection::TcpListener(tcp));
        }
        let mut tls_listener = None;
        if let Some((addr, cert, key)) = &config.tls {
            let acceptor = tls_acceptor(cert, key)?;
            let listener = tokio::net::TcpListener::bind(addr)
                .await
                .map_err(|e| format!("SIP TLS {addr}: {e}"))?;
            let addr = listener.local_addr().map_err(|e| format!("SIP TLS {addr}: {e}"))?;
            bound.tls = Some(addr);
            tls_listener = Some((listener, acceptor, addr));
        }
        transport_layer.set_tls_config(TlsConfig {
            ca_certs: config.ca_pem.clone(),
            ..Default::default()
        });
        let guard = Arc::new(Mutex::new(SourceGuard::new(config.guard)));
        let user_agent = if config.user_agent.is_empty() {
            "Moqui"
        } else {
            &config.user_agent
        };
        let endpoint_slot = Arc::new(std::sync::OnceLock::new());
        let phones = config.registrar.as_ref().map(Phones::new);
        let endpoint: Endpoint = EndpointBuilder::new()
            .with_user_agent(user_agent)
            .with_transport_layer(transport_layer)
            .with_cancel_token(stop.child_token())
            .with_transport_inspector(Box::new(BlockInspector {
                guard: Arc::clone(&guard),
                gate: Arc::new(Mutex::new(RequestGate::new(config.limits))),
                endpoint: Arc::clone(&endpoint_slot),
                phones: phones.as_ref().map(|p| (Arc::clone(&p.registrar), events.clone())),
            }))
            .build();
        let mut incoming = endpoint.incoming_transactions().map_err(sip_err)?;
        let endpoint_inner = endpoint.inner.clone();
        if let Some((listener, acceptor, local)) = tls_listener {
            tokio::spawn(accept_tls(
                listener,
                acceptor,
                local,
                Arc::downgrade(&endpoint_inner),
                config.tls_handshake.unwrap_or(TLS_HANDSHAKE),
                stop.child_token(),
            ));
        }
        let _ = endpoint_slot.set(Arc::downgrade(&endpoint_inner));
        {
            let serve = endpoint_inner.clone();
            tokio::spawn(async move {
                if let Err(err) = serve.serve().await {
                    tracing::error!("SIP endpoint stopped: {err}");
                }
            });
        }
        let inner = Arc::new(Inner {
            dialogs: Arc::new(DialogLayer::new(endpoint_inner.clone())),
            endpoint: endpoint_inner,
            events,
            trunks: Mutex::new(HashMap::new()),
            legs: Mutex::new(HashMap::new()),
            early: Mutex::new(HashMap::new()),
            guard,
            nonces: Mutex::new(NonceBook::new(
                if config.realm.is_empty() {
                    "moqui"
                } else {
                    &config.realm
                },
                NONCE_SECONDS,
            )),
            device_realms: Mutex::new(HashMap::new()),
            bound,
            public_host: config.public_host.filter(|h| !h.trim().is_empty()),
            stop: stop.clone(),
            phones,
            trunk_ping: config.trunk_ping.filter(|d| !d.is_zero()).unwrap_or(TRUNK_PING),
            resolver: config.resolver.unwrap_or_else(|| Arc::new(SystemResolver::default())),
            ca_pem: config.ca_pem,
            tls_flows: Mutex::new(HashMap::new()),
        });
        phones::sweep(&inner);
        {
            let inner = Arc::clone(&inner);
            tokio::spawn(async move {
                loop {
                    let tx = tokio::select! {
                        tx = incoming.recv() => tx,
                        _ = inner.stop.cancelled() => None,
                    };
                    let Some(tx) = tx else { break };
                    let inner = Arc::clone(&inner);
                    tokio::spawn(async move { inner.incoming(tx).await });
                }
            });
        }
        Ok(Ua { inner })
    }

    pub fn bound(&self) -> Bound {
        self.inner.bound
    }

    /// Replaces the trunks: their registrations start over, a trunk left out stops (and unregisters). A trunk whose
    /// host does not resolve is logged and kept: its registration or pings say it failed, and the others load.
    pub async fn set_trunks(&self, trunks: Vec<TrunkConfig>) {
        let mut entries = HashMap::new();
        for trunk in trunks {
            let targets = resolve::targets(&*self.inner.resolver, &trunk.host, trunk.port, trunk.transport).await;
            if targets.is_empty() {
                tracing::warn!("SIP trunk {} ({}) did not resolve", trunk.id, trunk.host);
            }
            let stop = self.inner.stop.child_token();
            if trunk.register && trunk.has_credentials() {
                let inner = Arc::clone(&self.inner);
                let config = trunk.clone();
                let token = stop.clone();
                tokio::spawn(async move { inner.register_loop(config, token).await });
            } else {
                // one that does not register is pinged, so it still shows up or down before a call fails
                let inner = Arc::clone(&self.inner);
                let config = trunk.clone();
                let token = stop.clone();
                tokio::spawn(async move { inner.ping_loop(config, token).await });
            }
            entries.insert(
                trunk.id.clone(),
                TrunkEntry {
                    config: trunk,
                    targets,
                    stop,
                },
            );
        }
        let old = std::mem::replace(&mut *lock(&self.inner.trunks), entries);
        for (_, entry) in old {
            entry.stop.cancel();
        }
    }

    /// Blocks a source until `until` (a block the voice services kept), or clears its block.
    pub fn set_block(&self, source: IpAddr, until: Option<Instant>) {
        lock(&self.inner.guard).set_block(source, until);
    }

    /// The registrar's realms besides the configured one: a REGISTER or INVITE whose domain (the To or From URI's host)
    /// is one of them is challenged in that realm, and its answer checked by the directory for that realm
    /// ([`DeviceDirectory::verify`]). The phones of each realm are kept apart ([`aor_key`]). A realm kept from the list
    /// before keeps its nonces; one no longer listed fails the answers to its challenges.
    pub fn set_device_realms(&self, realms: &[String]) {
        let primary = lock(&self.inner.nonces).realm().to_ascii_lowercase();
        let mut books = lock(&self.inner.device_realms);
        let wanted: Vec<String> = realms
            .iter()
            .map(|realm| realm.trim().trim_end_matches('.').to_ascii_lowercase())
            .filter(|realm| !realm.is_empty() && *realm != primary)
            .collect();
        books.retain(|realm, _| wanted.contains(realm));
        for realm in wanted {
            books
                .entry(realm.clone())
                .or_insert_with(|| NonceBook::new(&realm, NONCE_SECONDS));
        }
    }

    pub fn set_guard_policy(&self, policy: GuardPolicy) {
        lock(&self.inner.guard).set_policy(policy);
    }

    /// Places an outbound call; what happens comes as events for `request.leg`.
    pub fn dial(&self, request: DialRequest) -> Result<(), String> {
        let trunk = lock(&self.inner.trunks)
            .get(&request.trunk)
            .map(|t| (t.config.clone(), t.targets.clone()))
            .ok_or_else(|| format!("no trunk {}", request.trunk))?;
        let (give_up, gave_up) = oneshot::channel();
        lock(&self.inner.legs).insert(
            request.leg.clone(),
            Leg {
                kind: LegKind::Outbound,
                dialog: None,
                give_up: Some(give_up),
                answer: None,
                answered: false,
            },
        );
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move { inner.outbound(request, trunk.0, trunk.1, gave_up).await });
        Ok(())
    }

    /// Answers an inbound leg that rings (200 with its answer SDP).
    /// An answer that comes before the host's decision reached the leg is kept and sent as it rings.
    pub fn answer(&self, leg: &str) -> Result<(), String> {
        if !lock(&self.inner.legs).contains_key(leg) {
            let mut early = lock(&self.inner.early);
            let now = Instant::now();
            early.retain(|_, at| now.duration_since(*at) < EARLY_TTL);
            if early.len() < MAX_EARLY {
                early.insert(leg.to_string(), now);
            }
            return Ok(());
        }
        self.inner.accept(leg)
    }

    /// Ends a leg from this side: BYE once answered, CANCEL or a refusal before.
    pub fn hangup(&self, leg: &str) {
        let Some(mut entry) = lock(&self.inner.legs).remove(leg) else {
            return;
        };
        let inner = Arc::clone(&self.inner);
        let leg = leg.to_string();
        tokio::spawn(async move {
            match (&entry.kind, entry.answered, entry.dialog.take()) {
                (_, true, Some(dialog)) => {
                    if let Err(err) = dialog.bye().await {
                        tracing::debug!("SIP leg {leg}: BYE failed: {err}");
                    }
                    inner.emit(UaEvent::Ended {
                        leg,
                        reason: EndReason::Hangup,
                    });
                }
                (LegKind::Outbound, false, _) => {
                    // the outbound task cancels and reports the outcome
                    if let Some(give_up) = entry.give_up.take() {
                        let _ = give_up.send(());
                    }
                }
                (LegKind::Inbound, false, Some(dialog)) => {
                    let _ = dialog.reject(Some(StatusCode::BusyHere), None);
                    inner.emit(UaEvent::Ended {
                        leg,
                        reason: EndReason::Hangup,
                    });
                }
                _ => {}
            }
        });
    }

    /// Sends a re-INVITE with a new offer (hold, resume); the other side's answer.
    pub async fn reinvite(&self, leg: &str, offer: String) -> Result<Option<String>, String> {
        let dialog = lock(&self.inner.legs)
            .get(leg)
            .and_then(|l| l.dialog.clone())
            .ok_or("noLeg")?;
        let resp = dialog
            .reinvite(
                Some(vec![Header::ContentType("application/sdp".into())]),
                Some(offer.into_bytes()),
            )
            .await
            .map_err(sip_err)?;
        Ok(resp
            .filter(|r| r.status_code.kind() == StatusCodeKind::Successful)
            .map(|r| String::from_utf8_lossy(&r.body).into_owned()))
    }

    /// Stops the listeners, the registrations and the legs.
    pub fn shutdown(&self) {
        for (_, entry) in lock(&self.inner.trunks).drain() {
            entry.stop.cancel();
        }
        self.inner.stop.cancel();
    }
}

/// A port of 0 for a stream listener is picked here, since rsipstack binds stream listeners when it serves them and
/// would not tell the port.
/// The TLS server settings of the listener: the certificate chain and key, PEM.
fn tls_acceptor(cert: &[u8], key: &[u8]) -> Result<tokio_rustls::TlsAcceptor, String> {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let certs = CertificateDer::pem_slice_iter(cert)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("SIP TLS certificate: {e}"))?;
    let key = PrivateKeyDer::from_pem_slice(key).map_err(|e| format!("SIP TLS key: {e}"))?;
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("SIP TLS: {e}"))?;
    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
}

/// Accept TLS clients on `listener` until `stop`: each counted against the handshake slots before its handshake,
/// which must end within `deadline`; then its stream is rsipstack's, as its own TLS listener would hand it over.
async fn accept_tls(
    listener: tokio::net::TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
    local: SocketAddr,
    endpoint: std::sync::Weak<EndpointInner>,
    deadline: Duration,
    stop: CancellationToken,
) {
    let slots = moqui_lib::source_guard::SourceSlots::new(MAX_TLS_HANDSHAKES, TLS_HANDSHAKES_PER_SOURCE);
    let local = SipAddr {
        r#type: Some(rsipstack::sip::transport::Transport::Tls),
        addr: local.into(),
    };
    loop {
        let (stream, remote) = tokio::select! {
            () = stop.cancelled() => return,
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(err) => {
                    tracing::debug!("SIP TLS accept failed: {err}");
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    continue;
                }
            },
        };
        let Ok(slot) = slots.take(remote.ip()) else {
            tracing::debug!("SIP TLS: a connection from {remote} closed: too many handshakes from there or in all");
            continue;
        };
        let (acceptor, local, endpoint, stop) = (acceptor.clone(), local.clone(), endpoint.clone(), stop.clone());
        tokio::spawn(async move {
            let handshake = tokio::time::timeout(deadline, acceptor.accept(stream)).await;
            drop(slot);
            let tls = match handshake {
                Ok(Ok(tls)) => tls,
                Ok(Err(err)) => {
                    tracing::debug!("SIP TLS handshake from {remote} failed: {err}");
                    return;
                }
                Err(_) => {
                    tracing::debug!("SIP TLS: {remote} did not finish its handshake within {deadline:?}");
                    return;
                }
            };
            let Some(endpoint) = endpoint.upgrade() else { return };
            let remote = SipAddr {
                r#type: Some(rsipstack::sip::transport::Transport::Tls),
                addr: remote.into(),
            };
            match TlsConnection::from_server_stream(tls, local, remote.clone(), Some(stop.child_token())).await {
                Ok(connection) => endpoint.transport_layer.add_connection(SipConnection::Tls(connection)),
                Err(err) => tracing::warn!("SIP TLS connection from {remote} not served: {err}"),
            }
        });
    }
}

async fn free_port(addr: SocketAddr) -> Result<SocketAddr, String> {
    if addr.port() != 0 {
        return Ok(addr);
    }
    let probe = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("SIP listener {addr}: {e}"))?;
    probe.local_addr().map_err(|e| e.to_string())
}

impl Inner {
    fn emit(&self, event: UaEvent) {
        let _ = self.events.send(event);
    }

    /// Sends an inbound leg's 200 with its answer SDP.
    fn accept(&self, leg: &str) -> Result<(), String> {
        let mut legs = lock(&self.legs);
        let entry = legs.get_mut(leg).ok_or("noLeg")?;
        if entry.answered {
            return Ok(());
        }
        let dialog = entry.dialog.clone().ok_or("noDialog")?;
        let body = entry.answer.clone().unwrap_or_default().into_bytes();
        dialog
            .accept(Some(vec![Header::ContentType("application/sdp".into())]), Some(body))
            .map_err(sip_err)?;
        entry.answered = true;
        Ok(())
    }

    /// The Contact host and port for a transport.
    fn contact(&self, transport: Transport, user: &str) -> String {
        let bound = match transport {
            Transport::Udp => self.bound.udp,
            Transport::Tcp => self.bound.tcp,
            Transport::Tls => self.bound.tls,
        }
        .or(self.bound.udp)
        .or(self.bound.tcp)
        .or(self.bound.tls);
        let port = bound.map(|b| b.port()).unwrap_or(transport.default_port());
        let host = self
            .public_host
            .clone()
            .or_else(|| bound.map(|b| b.ip().to_string()))
            .unwrap_or_else(|| "127.0.0.1".to_string());
        let host = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]")
        } else {
            host
        };
        let user = if user.is_empty() { "moqui" } else { user };
        if transport == Transport::Udp {
            format!("sip:{user}@{host}:{port}")
        } else {
            format!("sip:{user}@{host}:{port};transport={}", transport.param())
        }
    }

    /// A trunk's targets, looked up again (its registration and pings do, so they follow its DNS). They replace the
    /// ones kept for its calls and the calls it sends in, while the trunk kept is still this host.
    async fn targets(&self, trunk: &TrunkConfig) -> Vec<SocketAddr> {
        let targets = resolve::targets(&*self.resolver, &trunk.host, trunk.port, trunk.transport).await;
        if !targets.is_empty() {
            if let Some(entry) = lock(&self.trunks).get_mut(&trunk.id) {
                let kept = &entry.config;
                if kept.host == trunk.host && kept.port == trunk.port && kept.transport == trunk.transport {
                    entry.targets.clone_from(&targets);
                }
            }
        }
        targets
    }

    /// The connection to `addr`, one of a trunk's targets. UDP goes out of the listener. A TCP or TLS connection is
    /// opened (or the open one used) within [`CONNECT_WAIT`], so a target that does not take it gives way to the next.
    /// TLS names the trunk's host, for SNI and the certificate check, never an SRV target or the address.
    async fn connect(&self, trunk: &TrunkConfig, addr: SocketAddr) -> Result<(SipConnection, SipAddr), String> {
        let target = SipAddr {
            r#type: Some(trunk.transport.sip()),
            addr: addr.into(),
        };
        let open = |c: &SipConnection| c.cancel_token().is_some_and(|t| !t.is_cancelled());
        if trunk.transport == Transport::Tls {
            let key = (addr, resolve::bare(&trunk.host));
            if !lock(&self.tls_flows).get(&key).is_some_and(open) {
                let config = TlsConfig {
                    ca_certs: self.ca_pem.clone(),
                    sni_hostname: Some(key.1.clone()),
                    ..Default::default()
                };
                let connecting = TlsConnection::connect(&target, Some(&config), None, Some(self.stop.child_token()));
                let connection = tokio::time::timeout(CONNECT_WAIT, connecting)
                    .await
                    .map_err(|_| format!("{addr}: no TLS connection in time"))?
                    .map_err(|e| format!("{addr}: {e}"))?;
                let connection = SipConnection::Tls(connection);
                // the transport layer keeps it by its address: the requests to this target go over it
                self.endpoint.transport_layer.add_connection(connection.clone());
                let mut flows = lock(&self.tls_flows);
                flows.retain(|_, c| open(c));
                flows.insert(key, connection);
            }
        }
        let (connection, _) = tokio::time::timeout(CONNECT_WAIT, self.endpoint.transport_layer.lookup(&target, None))
            .await
            .map_err(|_| format!("{addr}: no connection in time"))?
            .map_err(|e| format!("{addr}: {e}"))?;
        Ok((connection, target))
    }

    /// The first of `rest` that connects; the ones that do not are logged and passed.
    async fn connect_next(
        &self,
        trunk: &TrunkConfig,
        rest: &mut std::slice::Iter<'_, SocketAddr>,
    ) -> Result<(SipConnection, SipAddr), String> {
        let mut last = format!("trunk {} has no address", trunk.id);
        for &addr in rest.by_ref() {
            match self.connect(trunk, addr).await {
                Ok(found) => return Ok(found),
                Err(err) => {
                    tracing::warn!("SIP trunk {}: {err}", trunk.id);
                    last = err;
                }
            }
        }
        Err(last)
    }

    // ---------- registration ----------

    async fn register_loop(self: Arc<Self>, trunk: TrunkConfig, stop: CancellationToken) {
        let call_id = format!("{}@moqui", random_text(16));
        let mut seq = 0u32;
        let mut failures = 0u32;
        loop {
            let outcome = self.register_once(&trunk, &call_id, &mut seq, REGISTER_EXPIRES).await;
            // a loop that set_trunks replaced while its REGISTER was on its way tells nothing: the new loop does
            if stop.is_cancelled() {
                break;
            }
            let wait = match outcome {
                Ok(expires) => {
                    failures = 0;
                    self.emit(UaEvent::TrunkStatus {
                        trunk: trunk.id.clone(),
                        state: TrunkState::Registered,
                    });
                    Duration::from_secs(u64::from(expires.max(30)) * 4 / 5)
                }
                Err(err) => {
                    failures += 1;
                    tracing::warn!("SIP trunk {} did not register: {err}", trunk.id);
                    self.emit(UaEvent::TrunkStatus {
                        trunk: trunk.id.clone(),
                        state: TrunkState::Failed(err),
                    });
                    (Duration::from_secs(5) * 2u32.saturating_pow(failures.min(8))).min(MAX_BACKOFF)
                }
            };
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = stop.cancelled() => break,
            }
        }
        // a trunk taken out: let the registration go. One kept by set_trunks (the same id, loaded again) is left to
        // its new loop, which registers it again: a REGISTER with expires 0 here would remove that binding too, and an
        // `Unregistered` after the new loop's answer would show a working trunk as down.
        let kept = lock(&self.trunks).get(&trunk.id).is_some_and(|entry| {
            let new = &entry.config;
            new.register
                && new.host == trunk.host
                && new.port == trunk.port
                && new.transport == trunk.transport
                && new.credentials.username == trunk.credentials.username
        });
        if !self.stop.is_cancelled() && !kept {
            let _ = tokio::time::timeout(
                Duration::from_secs(5),
                self.register_once(&trunk, &call_id, &mut seq, 0),
            )
            .await;
            self.emit(UaEvent::TrunkStatus {
                trunk: trunk.id.clone(),
                state: TrunkState::Unregistered,
            });
        }
    }

    /// Pings a trunk that does not register with OPTIONS: its status (reachable, or failed with why once
    /// [`PING_FAILURES`] pings in a row had no answer) is told when it changes, and again every ten pings.
    async fn ping_loop(self: Arc<Self>, trunk: TrunkConfig, stop: CancellationToken) {
        let call_id = format!("{}@moqui", random_text(16));
        let mut seq = 0u32;
        let mut last: Option<TrunkState> = None;
        let mut unchanged = 0u32;
        let mut failures = 0u32;
        let wait = PING_WAIT.min(self.trunk_ping / 2);
        loop {
            let state = match tokio::time::timeout(wait, self.ping_once(&trunk, &call_id, &mut seq)).await {
                Ok(Ok(())) => TrunkState::Reachable,
                Ok(Err(err)) => TrunkState::Failed(format!("no answer to OPTIONS: {err}")),
                Err(_) => TrunkState::Failed("no answer to OPTIONS in time".to_string()),
            };
            // a loop that set_trunks replaced tells nothing: the new loop does
            if stop.is_cancelled() {
                break;
            }
            failures = if matches!(state, TrunkState::Failed(_)) {
                failures + 1
            } else {
                0
            };
            unchanged += 1;
            if (failures == 0 || failures >= PING_FAILURES) && (last.as_ref() != Some(&state) || unchanged >= 10) {
                if let TrunkState::Failed(err) = &state {
                    if last.as_ref() != Some(&state) {
                        tracing::warn!("SIP trunk {}: {err}", trunk.id);
                    }
                }
                self.emit(UaEvent::TrunkStatus {
                    trunk: trunk.id.clone(),
                    state: state.clone(),
                });
                last = Some(state);
                unchanged = 0;
            }
            tokio::select! {
                _ = tokio::time::sleep(self.trunk_ping) => {}
                _ = stop.cancelled() => break,
            }
        }
    }

    /// One OPTIONS to a trunk: any final answer (a 405 or a challenge too) is the server answering.
    /// A target that does not take a connection gives way to the next; one that takes it is pinged.
    async fn ping_once(&self, trunk: &TrunkConfig, call_id: &str, seq: &mut u32) -> Result<(), String> {
        let targets = self.targets(trunk).await;
        let (connection, target) = self.connect_next(trunk, &mut targets.iter()).await?;
        let domain = trunk.domain();
        let user = if trunk.credentials.username.is_empty() {
            "moqui"
        } else {
            trunk.credentials.username.as_str()
        };
        let request_uri = format!("sip:{domain}");
        let ours: Uri = Uri::try_from(format!("sip:{user}@{domain}").as_str()).map_err(sip_err)?;
        *seq += 1;
        let via = self
            .endpoint
            .get_via(Some(connection.get_addr().clone()), None)
            .map_err(sip_err)?;
        let from = rsipstack::sip::typed::From {
            display_name: None,
            uri: ours.clone(),
            params: vec![],
        }
        .with_tag(make_tag());
        let to = rsipstack::sip::typed::To {
            display_name: None,
            uri: Uri::try_from(request_uri.as_str()).map_err(sip_err)?,
            params: vec![],
        };
        let request = self.endpoint.make_request(
            Method::Options,
            Uri::try_from(request_uri.as_str()).map_err(sip_err)?,
            via,
            from,
            to,
            *seq,
            Some(call_id.to_string().into()),
        );
        self.send_request(request, &target).await.map(|_| ())
    }

    /// One REGISTER (answering one challenge); the expiry the registrar granted. The trunk's targets are tried in
    /// order: the next one when a target takes no connection, gives no answer in time or answers 503 (RFC 3263 §4.3).
    async fn register_once(
        &self,
        trunk: &TrunkConfig,
        call_id: &str,
        seq: &mut u32,
        expires: u32,
    ) -> Result<u32, String> {
        let targets = self.targets(trunk).await;
        let mut last = format!("trunk {} has no address", trunk.id);
        for &addr in &targets {
            match self.register_at(trunk, addr, call_id, seq, expires).await {
                Ok(granted) => return Ok(granted),
                Err(Miss::Unreachable(err)) => {
                    tracing::warn!("SIP trunk {}: {addr}: {err}", trunk.id);
                    last = err;
                }
                Err(Miss::Refused(err)) => return Err(err),
            }
        }
        Err(last)
    }

    /// One REGISTER to one of a trunk's targets.
    async fn register_at(
        &self,
        trunk: &TrunkConfig,
        addr: SocketAddr,
        call_id: &str,
        seq: &mut u32,
        expires: u32,
    ) -> Result<u32, Miss> {
        let (connection, target) = self.connect(trunk, addr).await.map_err(Miss::Unreachable)?;
        let domain = trunk.domain();
        let user = &trunk.credentials.username;
        let request_uri = format!("sip:{domain}");
        let aor: Uri = Uri::try_from(format!("sip:{user}@{domain}").as_str()).map_err(sip_err)?;
        let contact = self.contact(trunk.transport, user);
        let tag = make_tag();
        let mut authorization: Option<(bool, String)> = None;
        for _attempt in 0..2 {
            *seq += 1;
            let via = self
                .endpoint
                .get_via(Some(connection.get_addr().clone()), None)
                .map_err(sip_err)?;
            let from = rsipstack::sip::typed::From {
                display_name: None,
                uri: aor.clone(),
                params: vec![],
            }
            .with_tag(tag.clone());
            let to = rsipstack::sip::typed::To {
                display_name: None,
                uri: aor.clone(),
                params: vec![],
            };
            let mut request = self.endpoint.make_request(
                Method::Register,
                Uri::try_from(request_uri.as_str()).map_err(sip_err)?,
                via,
                from,
                to,
                *seq,
                Some(call_id.to_string().into()),
            );
            request.headers.push(Header::Contact(format!("<{contact}>").into()));
            request.headers.push(Header::Expires(expires.to_string().into()));
            if let Some((proxy, value)) = &authorization {
                request.headers.push(if *proxy {
                    Header::ProxyAuthorization(value.clone().into())
                } else {
                    Header::Authorization(value.clone().into())
                });
            }
            let resp = self.send_request(request, &target).await.map_err(Miss::Unreachable)?;
            let code = u16::from(resp.status_code.clone());
            match code {
                200..=299 => return Ok(granted_expires(&resp).unwrap_or(expires)),
                401 | 407 if authorization.is_none() => {
                    let value = answer_challenge(&resp, &trunk.credentials, "REGISTER", &request_uri)?;
                    authorization = Some((code == 407, value));
                }
                _ => {
                    let why = format!("the registrar answered {code} {}", resp.status_code);
                    // a 503 is this target's: another of the trunk's may take the registration
                    return Err(if code == 503 {
                        Miss::Unreachable(why)
                    } else {
                        Miss::Refused(why)
                    });
                }
            }
        }
        Err(Miss::Refused("the registrar refused the credentials".to_string()))
    }

    /// Sends a request outside a dialog and waits for its final answer.
    async fn send_request(&self, request: Request, target: &SipAddr) -> Result<Response, String> {
        let key = TransactionKey::from_request(&request, TransactionRole::Client).map_err(sip_err)?;
        let mut tx = Transaction::new_client(key, request, self.endpoint.clone(), None);
        tx.destination = Some(target.clone());
        tx.send().await.map_err(sip_err)?;
        let wait = async {
            while let Some(message) = tx.receive().await {
                if let SipMessage::Response(resp) = message {
                    if resp.status_code.kind() != StatusCodeKind::Provisional {
                        return Some(resp);
                    }
                }
            }
            None
        };
        match tokio::time::timeout(REGISTER_WAIT, wait).await {
            Ok(Some(resp)) => Ok(resp),
            Ok(None) => Err("no answer".to_string()),
            Err(_) => Err("no answer in time".to_string()),
        }
    }

    // ---------- outbound ----------

    async fn outbound(
        self: Arc<Self>,
        request: DialRequest,
        trunk: TrunkConfig,
        targets: Vec<SocketAddr>,
        mut gave_up: oneshot::Receiver<()>,
    ) {
        let leg = request.leg.clone();
        let reason = match self.place(&request, &trunk, &targets, &mut gave_up).await {
            Ok(None) => return,
            Ok(Some(reason)) => reason,
            Err(err) => {
                tracing::warn!("SIP leg {leg} to trunk {} failed: {err}", trunk.id);
                EndReason::Failed(None)
            }
        };
        lock(&self.legs).remove(&leg);
        self.emit(UaEvent::Ended { leg, reason });
    }

    /// Sends the INVITE (and once more with credentials) and follows it to its outcome: `None` once answered (the
    /// dialog's watcher reports its end), else how it ended. The trunk's targets are tried in order (RFC 3263 §4.3): the
    /// next one when a target takes no connection, or before anything rang and before the race has an outcome, when
    /// the INVITE could not be sent, had no answer at all, or was answered 503.
    async fn place(
        self: &Arc<Self>,
        request: &DialRequest,
        trunk: &TrunkConfig,
        targets: &[SocketAddr],
        gave_up: &mut oneshot::Receiver<()>,
    ) -> Result<Option<EndReason>, String> {
        let mut rest = targets.iter();
        let (_, mut target) = self.connect_next(trunk, &mut rest).await?;
        let domain = trunk.domain();
        // the Request-URI names the trunk's host and port: a CANCEL goes where it points
        let callee = Uri::try_from(trunk.uri(&request.to).as_str()).map_err(sip_err)?;
        let caller = Uri::try_from(format!("sip:{}@{domain}", request.from).as_str()).map_err(sip_err)?;
        let contact = Uri::try_from(self.contact(trunk.transport, &request.from).as_str()).map_err(sip_err)?;
        let call_id = format!("{}@moqui", random_text(20));
        let mut race = Race::new(Instant::now(), request.ring_timeout);
        let mut authorization: Option<Header> = None;
        // the host's hangup, once (a oneshot is not polled again after it fired)
        let mut given_up = false;
        // a display name or header value goes into the INVITE as it is: one with a line break or a quote could add
        // headers of its own (a P-Asserted-Identity the provider trusts), so they are cleaned first
        let display_name = request.from_name.as_deref().map(display_name).filter(|n| !n.is_empty());
        // one INVITE, and once more with credentials, for each target
        let mut attempts = 0;
        while attempts < 2 {
            attempts += 1;
            let mut headers: Vec<Header> = request
                .headers
                .iter()
                .filter(|(name, value)| header_safe(name, value))
                .map(|(name, value)| Header::Other(name.clone(), value.clone()))
                .collect();
            if let Some(auth) = &authorization {
                headers.push(auth.clone());
            }
            let option = InviteOption {
                caller_display_name: display_name.clone(),
                caller: caller.clone(),
                callee: callee.clone(),
                destination: Some(target.clone()),
                content_type: Some("application/sdp".to_string()),
                offer: Some(request.offer.clone().into_bytes()),
                contact: contact.clone(),
                headers: Some(headers),
                call_id: Some(call_id.clone()),
                ..Default::default()
            };
            let (states_tx, states) = self.dialogs.new_dialog_state_channel();
            let (dialog, handle) = self.dialogs.do_invite_async(option, states_tx).map_err(sip_err)?;
            if let Some(entry) = lock(&self.legs).get_mut(&request.leg) {
                entry.dialog = Some(dialog.clone());
            }
            let ringing = Arc::new(Mutex::new(false));
            let watcher = self.watch_early(request.leg.clone(), states, Arc::clone(&ringing));
            let mut handle = handle;
            let result = loop {
                let deadline = tokio::time::Instant::from_std(race.deadline());
                tokio::select! {
                    result = &mut handle => break result,
                    _ = &mut *gave_up, if !given_up => {
                        given_up = true;
                        if race.cancel() == race::Action::Cancel {
                            let _ = dialog.cancel().await;
                        }
                    }
                    // once the race has an outcome its deadline is past: waiting on it again would spin
                    _ = tokio::time::sleep_until(deadline), if race.outcome().is_none() => {
                        if race.poll(Instant::now()) == race::Action::Cancel {
                            let _ = dialog.cancel().await;
                        }
                    }
                }
            };
            let unreachable = race.outcome().is_none()
                && !*lock(&ringing)
                && match &result {
                    Ok(Ok((_, None))) | Ok(Err(_)) => true,
                    Ok(Ok((_, Some(resp)))) => u16::from(resp.status_code.clone()) == 503,
                    Err(_) => false,
                };
            if unreachable && !rest.as_slice().is_empty() {
                watcher.abort();
                let why = match &result {
                    Ok(Ok((_, Some(resp)))) => format!("answered {}", resp.status_code),
                    Ok(Err(err)) => err.to_string(),
                    _ => "no answer".to_string(),
                };
                tracing::warn!("SIP leg {} to trunk {}: {}: {why}", request.leg, trunk.id, target.addr);
                if let Ok((_, next)) = self.connect_next(trunk, &mut rest).await {
                    target = next;
                    authorization = None;
                    attempts = 0;
                    continue;
                }
            }
            let (_, response) = match result {
                Ok(Ok(done)) => done,
                Ok(Err(err)) => {
                    watcher.abort();
                    return Err(sip_err(err));
                }
                Err(err) => {
                    watcher.abort();
                    return Err(err.to_string());
                }
            };
            let Some(resp) = response else {
                watcher.abort();
                return Ok(Some(gave_up_reason(&race)));
            };
            let code = u16::from(resp.status_code.clone());
            match code {
                200..=299 => {
                    let tag = resp
                        .to_header()
                        .ok()
                        .and_then(|h| h.tag().ok().flatten())
                        .map(|t| t.value().to_string())
                        .unwrap_or_default();
                    if race.answered(&tag) != race::Action::Confirm {
                        // the 2xx crossed our CANCEL: rsipstack ACKs and ends it with BYE
                        watcher.abort();
                        return Ok(Some(gave_up_reason(&race)));
                    }
                    let answer = String::from_utf8_lossy(&resp.body).into_owned();
                    if let Some(entry) = lock(&self.legs).get_mut(&request.leg) {
                        entry.answered = true;
                        entry.give_up = None;
                    }
                    self.emit(UaEvent::Answered {
                        leg: request.leg.clone(),
                        answer,
                    });
                    // the watcher carries on, for the dialog's end and its re-INVITEs
                    return Ok(None);
                }
                401 | 407 if authorization.is_none() && race.outcome().is_none() => {
                    watcher.abort();
                    let value = answer_challenge(&resp, &trunk.credentials, "INVITE", &callee.to_string())?;
                    authorization = Some(if code == 407 {
                        Header::ProxyAuthorization(value.into())
                    } else {
                        Header::Authorization(value.into())
                    });
                }
                487 if race.outcome().is_some() => {
                    watcher.abort();
                    return Ok(Some(gave_up_reason(&race)));
                }
                _ => {
                    watcher.abort();
                    race.failed(code);
                    return Ok(Some(match code {
                        486 => EndReason::Busy,
                        600 | 603 => EndReason::Declined,
                        _ => EndReason::Failed(Some(code)),
                    }));
                }
            }
        }
        Ok(Some(EndReason::Failed(Some(407))))
    }

    /// Follows an outbound dialog's states: ringing, then once confirmed its re-INVITEs and its end.
    fn watch_early(
        self: &Arc<Self>,
        leg: String,
        states: DialogStateReceiver,
        ringing: Arc<Mutex<bool>>,
    ) -> tokio::task::JoinHandle<()> {
        let inner = Arc::clone(self);
        tokio::spawn(async move { inner.watch(leg, states, Some(ringing)).await })
    }

    async fn watch(self: Arc<Self>, leg: String, mut states: DialogStateReceiver, ringing: Option<Arc<Mutex<bool>>>) {
        while let Some(state) = states.recv().await {
            match state {
                DialogState::Early(_, resp) => {
                    if let Some(ringing) = &ringing {
                        let first = !std::mem::replace(&mut *lock(ringing), true);
                        if first && u16::from(resp.status_code.clone()) >= 180 {
                            self.emit(UaEvent::Ringing { leg: leg.clone() });
                        }
                    }
                }
                DialogState::Updated(_, req, handle) => {
                    if req.method == Method::Invite && !req.body.is_empty() {
                        let (reply, answer) = oneshot::channel();
                        self.emit(UaEvent::Reoffer {
                            leg: leg.clone(),
                            offer: String::from_utf8_lossy(&req.body).into_owned(),
                            reply,
                        });
                        match tokio::time::timeout(DECIDE_WAIT, answer).await {
                            Ok(Ok(Some(sdp))) => {
                                let _ = handle
                                    .respond(
                                        StatusCode::OK,
                                        Some(vec![Header::ContentType("application/sdp".into())]),
                                        Some(sdp.into_bytes()),
                                    )
                                    .await;
                            }
                            _ => {
                                let _ = handle.reply(StatusCode::NotAcceptableHere).await;
                            }
                        }
                    } else {
                        let _ = handle.reply(StatusCode::OK).await;
                    }
                }
                DialogState::Info(_, _, handle) | DialogState::Options(_, _, handle) => {
                    let _ = handle.reply(StatusCode::OK).await;
                }
                // a desk phone's blind transfer (RFC 3515): checked by the host first; one it took is accepted and the
                // phone told it went (its NOTIFY, so it hangs up), one it refused is answered 403 so the phone keeps the
                // call; an attended one (Replaces, RFC 3891) is not taken
                DialogState::Refer(id, req, handle) => {
                    let target = header_values(&req.headers, "Refer-To")
                        .into_iter()
                        .chain(header_values(&req.headers, "r"))
                        .next()
                        .and_then(|value| refer_target(&value));
                    match target {
                        Some(target) => {
                            let (reply, decision) = oneshot::channel();
                            self.emit(UaEvent::Refer {
                                leg: leg.clone(),
                                target,
                                reply,
                            });
                            let taken = matches!(tokio::time::timeout(DECIDE_WAIT, decision).await, Ok(Ok(true)));
                            if !taken {
                                let _ = handle.reply(StatusCode::Forbidden).await;
                                continue;
                            }
                            let _ = handle.reply(StatusCode::Accepted).await;
                            if let Some(Dialog::Invite(dialog)) = self.dialogs.get_dialog(&id) {
                                tokio::spawn(async move {
                                    let _ = dialog
                                        .notify_refer(StatusCode::OK, "terminated;reason=noresource")
                                        .await;
                                });
                            }
                        }
                        None => {
                            let _ = handle.reply(StatusCode::NotImplemented).await;
                        }
                    }
                }
                DialogState::Notify(_, _, handle)
                | DialogState::Message(_, _, handle)
                | DialogState::Publish(_, _, handle) => {
                    let _ = handle.reply(StatusCode::NotImplemented).await;
                }
                DialogState::Terminated(id, why) => {
                    self.dialogs.remove_dialog(&id);
                    {
                        let mut legs = lock(&self.legs);
                        // an outbound leg not yet answered reports its own outcome; a leg already gone was hung up here
                        match legs.get(&leg) {
                            None => break,
                            Some(entry) if matches!(entry.kind, LegKind::Outbound) && !entry.answered => break,
                            Some(_) => {
                                legs.remove(&leg);
                            }
                        }
                    }
                    let reason = match why {
                        TerminatedReason::UacBye | TerminatedReason::UasBye => EndReason::Bye,
                        TerminatedReason::UacCancel => EndReason::Canceled,
                        TerminatedReason::Timeout => EndReason::Failed(Some(408)),
                        TerminatedReason::UacBusy | TerminatedReason::UasBusy => EndReason::Busy,
                        TerminatedReason::UasDecline => EndReason::Declined,
                        TerminatedReason::UacOther(code) | TerminatedReason::UasOther(code) => {
                            EndReason::Failed(Some(u16::from(code)))
                        }
                        TerminatedReason::ProxyError(code) => EndReason::Failed(Some(u16::from(code))),
                        TerminatedReason::ProxyAuthRequired => EndReason::Failed(Some(407)),
                    };
                    self.emit(UaEvent::Ended {
                        leg: leg.clone(),
                        reason,
                    });
                    break;
                }
                _ => {}
            }
        }
    }

    // ---------- inbound ----------

    async fn incoming(self: Arc<Self>, mut tx: Transaction) {
        let method = tx.original.method;
        if let Some(Dialog::Invite(mut dialog)) = self.dialogs.match_dialog(&tx) {
            if let Err(err) = dialog.handle(&mut tx).await {
                tracing::debug!("SIP {method} in a dialog: {err}");
            }
            return;
        }
        let source = source_of(&tx);
        let trunk = source.and_then(|s| self.trunk_by_address(s.ip()));
        match method {
            Method::Invite | Method::Register => {}
            Method::Options if trunk.is_some() => {
                let _ = tx.reply(StatusCode::OK).await;
                return;
            }
            Method::Ack => return,
            _ if trunk.is_some() => {
                let _ = tx.reply(StatusCode::CallTransactionDoesNotExist).await;
                return;
            }
            // anything else from a stranger is not answered
            _ => return,
        }
        if method == Method::Register {
            if self.phones.is_some() {
                // the registrar (V4): a phone's REGISTER, from wherever it is
                self.register_phone(tx, source).await;
            } else {
                // a trunk does not register here, and with the registrar off no one can: refused without a
                // challenge, so a phone pointed here does not count failures against its source until it is blocked
                let _ = tx.reply(StatusCode::Forbidden).await;
            }
            return;
        }
        let caller = match trunk {
            Some(trunk) => Caller::Trunk(trunk),
            None => match self.authenticate(&mut tx, source, true).await {
                Some(caller) => caller,
                None => return,
            },
        };
        self.new_call(tx, caller, source).await;
    }

    fn trunk_by_address(&self, ip: IpAddr) -> Option<TrunkConfig> {
        lock(&self.trunks)
            .values()
            .find(|t| t.targets.iter().any(|a| a.ip() == ip) || t.config.inbound.iter().any(|e| matches_address(ip, e)))
            .map(|t| t.config.clone())
    }

    /// A request from an address that is not a trunk's: its digest answer must name a PBX trunk's username (`trunks`),
    /// or, with the registrar on, a phone's (its verifiers). A missing or stale answer is challenged; a failed one
    /// counts against the source.
    async fn authenticate(&self, tx: &mut Transaction, source: Option<SocketAddr>, trunks: bool) -> Option<Caller> {
        let proxy = tx.original.method == Method::Invite;
        let header_name = if proxy { "Proxy-Authorization" } else { "Authorization" };
        let given = header_values(&tx.original.headers, header_name)
            .into_iter()
            .find_map(|v| Authorization::parse(&v));
        let method = tx.original.method.to_string();
        // a device domain of its own (set_device_realms): its nonces and its directory's realm
        let realm = self.realm_of(tx, given.as_ref());
        let verdict = match given.as_ref() {
            None => None,
            Some(auth) => {
                // a PBX trunk's credentials only: a provider's are its account's, never a way in
                let trunk = lock(&self.trunks)
                    .values()
                    .find(|t| {
                        let c = &t.config.credentials;
                        trunks
                            && t.config.inbound_digest
                            && c.auth_username
                                .as_deref()
                                .filter(|u| !u.is_empty())
                                .unwrap_or(&c.username)
                                == auth.username
                            && !c.password.is_empty()
                    })
                    .map(|t| t.config.clone());
                match trunk {
                    Some(trunk) => {
                        let password = trunk.credentials.password.clone();
                        let verdict = lock(&self.nonces).verify(auth, &method, Some(&password), now_secs());
                        Some((verdict, Some(Caller::Trunk(trunk))))
                    }
                    None if self.phones.is_some() => {
                        let (verdict, device) = self.check_device(auth, &method, realm.as_deref()).await;
                        Some((verdict, device.map(|device| Caller::Device(device, realm.clone()))))
                    }
                    None => Some((lock(&self.nonces).verify(auth, &method, None, now_secs()), None)),
                }
            }
        };
        match verdict {
            Some((Verdict::Valid, Some(caller))) => {
                if let Some(source) = source {
                    lock(&self.guard).success(source.ip());
                }
                Some(caller)
            }
            None | Some((Verdict::Stale, _)) => {
                let stale = verdict.is_some();
                let algorithms = [Algorithm::Sha256, Algorithm::Md5];
                let challenges = match realm.as_deref() {
                    Some(realm) => lock(&self.device_realms)
                        .get(realm)
                        .map(|book| book.challenges(now_secs(), &algorithms, stale))
                        .unwrap_or_default(),
                    None => lock(&self.nonces).challenges(now_secs(), &algorithms, stale),
                };
                let headers = challenges
                    .into_iter()
                    .map(|c| {
                        if proxy {
                            Header::ProxyAuthenticate(c.into())
                        } else {
                            Header::WwwAuthenticate(c.into())
                        }
                    })
                    .collect();
                let code = if proxy {
                    StatusCode::ProxyAuthenticationRequired
                } else {
                    StatusCode::Unauthorized
                };
                let _ = tx.reply_with(code, headers, None).await;
                None
            }
            // whoever holds the verifiers could not be asked (an edge node's local node away): no failed attempt
            Some((Verdict::Unavailable, _)) => {
                let _ = tx.reply(StatusCode::ServiceUnavailable).await;
                None
            }
            Some(_) => {
                let username = given.map(|a| a.username).unwrap_or_default();
                let blocked = match source {
                    Some(source) => {
                        let failure = lock(&self.guard).failure(source.ip(), Instant::now());
                        self.emit(UaEvent::AuthFailure {
                            source: source.ip(),
                            username,
                            blocked: failure == Failure::Blocked,
                        });
                        matches!(failure, Failure::Blocked | Failure::StillBlocked)
                    }
                    None => false,
                };
                if !blocked {
                    let _ = tx.reply(StatusCode::Forbidden).await;
                }
                None
            }
        }
    }

    async fn new_call(self: Arc<Self>, mut tx: Transaction, caller: Caller, source: Option<SocketAddr>) {
        let request = tx.original.clone();
        // a trunk's call, or a registered phone's over the transport it came by
        let (trunk_id, device, transport) = match caller {
            Caller::Trunk(trunk) => (trunk.id, None, trunk.transport),
            Caller::Device(device, _) => (String::new(), Some(device.id), phones::transport_in(&tx)),
        };
        let to = user_of(&request.uri);
        let (from, from_name) = match request.from_header().ok().and_then(|h| h.typed().ok()) {
            Some(from) => (user_of(&from.uri), from.display_name.clone()),
            None => (String::new(), None),
        };
        let call_id = request
            .call_id_header()
            .map(|h| h.value().to_string())
            .unwrap_or_default();
        if request.body.is_empty() {
            // an INVITE without an offer (late offer) is not supported
            let _ = tx.reply(StatusCode::NotAcceptableHere).await;
            return;
        }
        let offer = String::from_utf8_lossy(&request.body).into_owned();
        let contact = Uri::try_from(self.contact(transport, &to).as_str()).ok();
        let (states_tx, states) = self.dialogs.new_dialog_state_channel();
        let dialog = match self.dialogs.get_or_create_server_invite(&tx, states_tx, None, contact) {
            Ok(dialog) => dialog,
            Err(err) => {
                tracing::debug!("SIP INVITE {call_id}: {err}");
                let _ = tx.reply(StatusCode::ServerInternalError).await;
                return;
            }
        };
        // the transaction's own handling: 100 Trying, the CANCEL, the ACK
        {
            let mut handler = dialog.clone();
            tokio::spawn(async move {
                let _ = handler.handle(&mut tx).await;
            });
        }
        let (reply, decision) = oneshot::channel();
        self.emit(UaEvent::Incoming(IncomingCall {
            trunk_id,
            to,
            from,
            from_name,
            call_id: call_id.clone(),
            offer,
            source: source.unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], 0))),
            device,
            reply,
        }));
        match tokio::time::timeout(DECIDE_WAIT, decision).await {
            Ok(Ok(Decision::Ring { leg, answer })) => {
                lock(&self.legs).insert(
                    leg.clone(),
                    Leg {
                        kind: LegKind::Inbound,
                        dialog: Some(dialog.clone()),
                        give_up: None,
                        answer: Some(answer),
                        answered: false,
                    },
                );
                if let Err(err) = dialog.ringing(None, None) {
                    tracing::debug!("SIP INVITE {call_id}: 180 not sent: {err}");
                }
                let early = lock(&self.early)
                    .remove(&leg)
                    .is_some_and(|at| at.elapsed() < EARLY_TTL);
                if early {
                    if let Err(err) = self.accept(&leg) {
                        tracing::debug!("SIP leg {leg}: not answered: {err}");
                    }
                }
                let inner = Arc::clone(&self);
                tokio::spawn(async move { inner.watch(leg, states, None).await });
            }
            Ok(Ok(Decision::Reject(code))) => {
                let _ = dialog.reject(Some(status_code(code)), None);
            }
            _ => {
                let _ = dialog.reject(Some(StatusCode::TemporarilyUnavailable), None);
            }
        }
    }
}

/// A display name for a From header: no control characters, quotes or backslashes, at most 60 characters.
fn display_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control() && *c != '"' && *c != '\\')
        .take(60)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Whether an extra header can go into a request as it is: a token name, and a value with no control characters.
fn header_safe(name: &str, value: &str) -> bool {
    let ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-.!%*_+`'~".contains(c))
        && !value.chars().any(char::is_control);
    if !ok {
        tracing::warn!("SIP: the header {name:?} is not sent, it is not a plain header");
    }
    ok
}

/// Why a request to one of a trunk's targets failed.
enum Miss {
    /// No connection, no answer in time, or a 503: the next target is tried (RFC 3263 §4.3).
    Unreachable(String),
    /// The trunk's own answer, which another of its targets would give too.
    Refused(String),
}

impl From<String> for Miss {
    fn from(err: String) -> Miss {
        Miss::Refused(err)
    }
}

fn gave_up_reason(race: &Race) -> EndReason {
    match race.outcome() {
        Some(race::Outcome::TimedOut) => EndReason::TimedOut,
        Some(race::Outcome::Failed(code)) => EndReason::Failed(Some(code)),
        _ => EndReason::Canceled,
    }
}

/// Where a request came from: its top Via with `received` and `rport` (RFC 3581), which the transport fills in.
fn source_of(tx: &Transaction) -> Option<SocketAddr> {
    if let Some(remote) = tx.connection.as_ref().and_then(|c| c.get_remote_addr()) {
        if let Ok(addr) = remote.get_socketaddr() {
            return Some(addr);
        }
    }
    let via = tx.original.via_header().ok()?;
    let (_, host_with_port) = SipConnection::parse_target_from_via(via).ok()?;
    SocketAddr::try_from(host_with_port).ok()
}

/// The expiry a 200 to a REGISTER granted (its Contact's `expires`, else its Expires header).
fn granted_expires(resp: &Response) -> Option<u32> {
    for contact in header_values(&resp.headers, "Contact") {
        if let Some(value) = contact.split(';').find_map(|p| p.trim().strip_prefix("expires=")) {
            if let Ok(n) = value.trim().parse() {
                return Some(n);
            }
        }
    }
    header_values(&resp.headers, "Expires")
        .first()
        .and_then(|v| v.trim().parse().ok())
}

/// The Authorization value for a 401 or 407, checked against the trunk's realm and algorithms.
fn answer_challenge(resp: &Response, credentials: &Credentials, method: &str, uri: &str) -> Result<String, String> {
    let name = if u16::from(resp.status_code.clone()) == 407 {
        "Proxy-Authenticate"
    } else {
        "WWW-Authenticate"
    };
    let challenges: Vec<Challenge> = header_values(&resp.headers, name)
        .iter()
        .filter_map(|v| Challenge::parse(v))
        .collect();
    let challenge = digest::choose(&challenges, credentials).map_err(|r| r.to_string())?;
    digest::answer(credentials, challenge, method, uri, &digest::cnonce()).map_err(|r| r.to_string())
}

#[cfg(test)]
#[path = "ua_tests.rs"]
mod tests;
