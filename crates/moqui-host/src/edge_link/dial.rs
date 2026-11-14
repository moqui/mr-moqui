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

//! The local node's dialer (plans/archive/Edge-Node.md §5.1): it opens the up connection (this node the HTTP/2 client, Hello
//! first) and then the down connection (this node the HTTP/2 server of [`LocalLink`] and its web app,
//! [`super::down`]), keeps them alive with HTTP/2 PINGs and a Ping that measures the round trip, and dials again with a
//! backoff when either drops. A panic in one attempt is caught and logged, and the loop goes on
//! (docs/dev/Practices-Rust.md §8).
//!
//! While the link is up the local node also serves the edge node's delegated calls and queued items (§5.2, as the user
//! `_EDGE_`, through `moqui_core::edge`), and sends its own outbox and replicated sets up ([`super::queue`],
//! [`super::replicate`]). These need the server's factory, which comes with [`LocalConfig::web`]. The Edge Node
//! wizard asks the edge node's offer and sends its Configure on the up connection ([`Dialer::offer`],
//! [`Dialer::configure`], plans/archive/Edge-Node.md §5.7).
//!
//! Every member of a local cluster dials with the pairing's key and names its own instance id (plans/Edge-Node2.md
//! §4.4): with its Hello, every Ping and when the edge node asks, it tells its state (starting, live, draining,
//! stopping) and its cluster's members ([`PoolMember`]), which the edge node admits a new member by. A member draining
//! or starting pings soon after its state changes, and one stopping says so before its links close.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::edge::{
    ClusterMemberInfo, Configured, DelegateCall, EdgeInfo, EdgeOffer, EdgeSetup, LinkState, LinkStatus, MemberState,
};
use moqui_core::telemetry::Telemetry;
use moqui_lib::sync::lock;
use tokio::io::AsyncWriteExt as _;
use tokio::sync::watch;
use tonic::{Request, Response, Status};

use super::pair::{client_config, dial_tls, parse_url, Identity, ServerTrust};
use super::proto::local_link_server::{LocalLink, LocalLinkServer};
use super::proto::{
    ContentChunk, DelegateReply, DelegateRequest, DeliverReply, DeliverRequest, DrainRequest, HelloRequest,
    LocalHelloReply, LocalHelloRequest, MemberStatus, MembersRequest, OfferRequest, PingReply, PingRequest,
    PutContentReply, SignalMessage, SignalReply, UnpairRequest,
};
use super::{
    check_versions, edge_info, hello, jitter, read_answer, Answer, LinkClient, LinkError, LocalConfig, LINK_VERSION,
    ROLE_DOWN, ROLE_UP,
};
use crate::tls_pin::parse_hex;

/// What a member of a local cluster tells its edge node of itself (plans/Edge-Node2.md §4.4): its state and its
/// cluster's members, with its Hello, every Ping and when the edge node asks; and the edge node's operator drains it
/// through it. Both are blocking: a server's reads its database.
pub trait PoolMember: Send + Sync {
    /// Its state and its cluster's members now.
    fn status(&self) -> (MemberState, Vec<ClusterMemberInfo>);
    /// Drain it, or bring it back; its state after.
    fn drain(&self, drain: bool) -> Result<MemberState, String>;
}

/// A member whose state and cluster are set by hand: a dialer with no server (a test's), alone in its cluster and live
/// until told otherwise.
pub struct FixedMember {
    state: Mutex<MemberState>,
    members: Mutex<Vec<ClusterMemberInfo>>,
}

impl FixedMember {
    /// Member `instance_id`, live and alone in its cluster.
    pub fn new(instance_id: &str) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(MemberState::Live),
            members: Mutex::new(vec![ClusterMemberInfo {
                instance_id: instance_id.to_string(),
                heartbeat_age_ms: 0,
                live: true,
                version: env!("CARGO_PKG_VERSION").to_string(),
            }]),
        })
    }

    /// The cluster it tells from now: each id with whether it is live.
    pub fn set_members(&self, members: &[(&str, bool)]) {
        *lock(&self.members) = members
            .iter()
            .map(|(id, live)| ClusterMemberInfo {
                instance_id: (*id).to_string(),
                heartbeat_age_ms: if *live { 1_000 } else { 3_600_000 },
                live: *live,
                version: env!("CARGO_PKG_VERSION").to_string(),
            })
            .collect();
    }

    pub fn set_state(&self, state: MemberState) {
        *lock(&self.state) = state;
    }
}

impl PoolMember for FixedMember {
    fn status(&self) -> (MemberState, Vec<ClusterMemberInfo>) {
        (*lock(&self.state), lock(&self.members).clone())
    }

    fn drain(&self, drain: bool) -> Result<MemberState, String> {
        let state = if drain {
            MemberState::Draining
        } else {
            MemberState::Live
        };
        *lock(&self.state) = state;
        Ok(state)
    }
}

/// A server's member: its state and its `moqui.server.ClusterMember` rows (`moqui_core::edge::pool`), and its own
/// `InstanceDrain` row for a drain.
pub(crate) struct ServerMember(pub(crate) Weak<ExecutionContextFactory>);

impl ServerMember {
    fn entered<T>(&self, job: impl FnOnce(&ExecutionContextFactory) -> T) -> Option<T> {
        let factory = self.0.upgrade()?;
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        Some(job(&factory))
    }
}

impl PoolMember for ServerMember {
    fn status(&self) -> (MemberState, Vec<ClusterMemberInfo>) {
        self.entered(|factory| {
            (
                moqui_core::edge::pool::member_state(factory),
                moqui_core::edge::pool::cluster_members(factory),
            )
        })
        .unwrap_or((MemberState::Stopping, Vec::new()))
    }

    fn drain(&self, drain: bool) -> Result<MemberState, String> {
        self.entered(|factory| moqui_core::edge::pool::set_drained(factory, drain).map_err(|err| err.to_string()))
            .unwrap_or_else(|| Err("the server is stopping".to_string()))
    }
}

/// The member `config` dials as.
fn pool_member(config: &LocalConfig) -> Arc<dyn PoolMember> {
    if let Some(member) = &config.member {
        return Arc::clone(member);
    }
    match config.web.as_ref().map(super::down::LocalWeb::factory) {
        Some(factory) => Arc::new(ServerMember(factory)),
        None => FixedMember::new(&config.instance_id),
    }
}

/// `member`'s status now, off the async threads; `stopping` overrides its state.
async fn member_status(member: &Arc<dyn PoolMember>, stopping: bool) -> (MemberState, MemberStatus) {
    let member = Arc::clone(member);
    let (state, members) = tokio::task::spawn_blocking(move || member.status())
        .await
        .unwrap_or((MemberState::Live, Vec::new()));
    let state = if stopping { MemberState::Stopping } else { state };
    (state, super::status_to_proto(state, &members))
}

/// An open up connection with its Hello done: the edge node's gRPC client, and the generation the edge node gave it.
/// Dropping it closes the connection.
pub struct UpLink {
    client: LinkClient,
    pub generation: u64,
    /// The pairing the edge node keys this node's rows by (plans/Edge-Node2.md §4.4): the instance id of the member
    /// that paired, which may not be this member's own; this node's own id from an edge node before E17.
    pub pairing_id: String,
    /// The token its down connection names (plans/Edge-Node2.md §4.11).
    pub down_token: Vec<u8>,
    /// The edge node, as its Hello said.
    pub edge: EdgeInfo,
    /// The edge node's TURN secret from its Hello answer (empty without TURN), which signs the credentials of its TURN
    /// URLs (`moqui_core::edge::media`); never logged.
    turn_secret: String,
    ended: watch::Receiver<bool>,
    driver: tokio::task::JoinHandle<()>,
    timeout: Duration,
}

impl Drop for UpLink {
    fn drop(&mut self) {
        self.driver.abort();
    }
}

impl UpLink {
    /// A Ping on the up connection: its round trip.
    pub async fn ping(&self) -> Result<Duration, String> {
        self.ping_with(None).await
    }

    /// A Ping that carries this member's state and cluster (plans/Edge-Node2.md §4.4): its round trip.
    pub(crate) async fn ping_with(&self, status: Option<MemberStatus>) -> Result<Duration, String> {
        let started = Instant::now();
        let mut client = self.client.clone();
        let request = PingRequest {
            generation: self.generation,
            status,
        };
        match tokio::time::timeout(self.timeout, client.ping(request)).await {
            Ok(Ok(_)) => Ok(started.elapsed()),
            Ok(Err(status)) => Err(format!("Ping: {}", status.message())),
            Err(_) => Err("the edge node did not answer Ping in time".to_string()),
        }
    }

    /// Ask the edge node to revoke this node's key.
    pub async fn unpair(&self) -> Result<(), String> {
        let mut client = self.client.clone();
        match tokio::time::timeout(self.timeout, client.unpair(UnpairRequest {})).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(status)) => Err(format!("Unpair: {}", status.message())),
            Err(_) => Err("the edge node did not answer Unpair in time".to_string()),
        }
    }

    /// Wait until the connection has closed.
    pub async fn closed(&self) {
        let mut ended = self.ended.clone();
        let _ = ended.wait_for(|ended| *ended).await;
    }
}

/// The parts of [`LocalConfig`] a connection needs, checked once.
struct Prepared {
    url: super::pair::EdgeUrl,
    tls: Arc<rustls::ClientConfig>,
}

fn prepare(config: &LocalConfig) -> Result<Prepared, String> {
    let url = parse_url(&config.url)?;
    let pin = parse_hex(&config.pin).ok_or_else(|| "edge_node_pin is not a SHA-256 fingerprint".to_string())?;
    let identity = Identity::from_key(&config.key, &config.instance_id)?;
    let tls = client_config(&ServerTrust::Pinned(pin), Some(&identity))?;
    Ok(Prepared { url, tls })
}

/// Dial the edge node with `role` (and its trailing bytes) and read the answer.
async fn dial_role(
    prepared: &Prepared,
    timeout: Duration,
    role: &[u8],
) -> Result<tokio_rustls::client::TlsStream<tokio::net::TcpStream>, LinkError> {
    let mut stream = dial_tls(&prepared.url, Arc::clone(&prepared.tls), timeout).await?;
    let roled = async {
        stream.write_all(role).await?;
        stream.flush().await?;
        read_answer(&mut stream).await
    };
    let (answer, message) = tokio::time::timeout(timeout, roled)
        .await
        .map_err(|_| LinkError::Failed("the edge node did not answer in time".to_string()))?
        .map_err(|err| LinkError::Failed(format!("the edge node closed the connection: {err}")))?;
    match answer {
        Answer::Ok => Ok(stream),
        Answer::Refused => Err(LinkError::Refused(message)),
        Answer::Stale | Answer::Failed => Err(LinkError::Failed(message)),
    }
}

/// Open an up connection and say Hello: the link's generation is in the answer.
pub async fn connect_up(config: &LocalConfig) -> Result<UpLink, LinkError> {
    let prepared = prepare(config).map_err(LinkError::Failed)?;
    connect_up_with(config, &prepared).await
}

/// Dial a down connection of `generation` and read the edge node's answer, then close it: for a test of the edge node's
/// generation check.
#[doc(hidden)]
pub async fn try_down(config: &LocalConfig, generation: u64, token: &[u8]) -> Result<(), LinkError> {
    let prepared = prepare(config).map_err(LinkError::Failed)?;
    let mut role = vec![ROLE_DOWN];
    role.extend_from_slice(&generation.to_be_bytes());
    role.extend_from_slice(token);
    dial_role(&prepared, config.connect_timeout, &role).await.map(drop)
}

async fn connect_up_with(config: &LocalConfig, prepared: &Prepared) -> Result<UpLink, LinkError> {
    let stream = dial_role(prepared, config.connect_timeout, &[ROLE_UP]).await?;
    // the member's own key signs this connection's exporter value (plans/Edge-Node2.md §4.11)
    let key = config
        .member_key
        .clone()
        .ok_or_else(|| LinkError::Failed("this member has no key of its own".to_string()))?;
    let binding = super::member_key::binding(stream.get_ref().1)
        .ok_or_else(|| LinkError::Failed("the link's TLS gave no exporter value".to_string()))?;
    let (send, connection) = hyper::client::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new())
        .timer(hyper_util::rt::TokioTimer::new())
        .keep_alive_interval(config.keepalive)
        .keep_alive_timeout(config.keepalive)
        .keep_alive_while_idle(true)
        .handshake(hyper_util::rt::TokioIo::new(stream))
        .await
        .map_err(|err| LinkError::Failed(format!("HTTP/2 with the edge node failed: {err}")))?;
    let (ended_tx, ended) = watch::channel(false);
    let driver = tokio::spawn(async move {
        if let Err(err) = connection.await {
            tracing::debug!("edge link up connection ended: {err}");
        }
        let _ = ended_tx.send(true);
    });
    let mut client = super::link_client(send);
    let (_, status) = member_status(&pool_member(config), false).await;
    let request = HelloRequest {
        hello: Some(hello(&config.instance_id, &config.node_name, &[], &[])),
        status: Some(status),
        member_key: key.public(),
        member_proof: key.prove(&binding),
    };
    let reply = match tokio::time::timeout(config.connect_timeout, client.hello(request)).await {
        Ok(Ok(reply)) => reply.into_inner(),
        Ok(Err(status)) => {
            driver.abort();
            return Err(match status.code() {
                tonic::Code::FailedPrecondition | tonic::Code::PermissionDenied => {
                    LinkError::Refused(status.message().to_string())
                }
                _ => LinkError::Failed(format!("Hello: {}", status.message())),
            });
        }
        Err(_) => {
            driver.abort();
            return Err(LinkError::Failed(
                "the edge node did not answer Hello in time".to_string(),
            ));
        }
    };
    let edge_hello = reply.hello.unwrap_or_default();
    if let Err(message) = check_versions(LINK_VERSION, edge_hello.link_version, "edge node") {
        driver.abort();
        return Err(LinkError::Refused(message));
    }
    let pairing_id = if reply.pairing_id.is_empty() {
        config.instance_id.clone()
    } else {
        reply.pairing_id.clone()
    };
    Ok(UpLink {
        client,
        generation: reply.generation,
        pairing_id,
        down_token: reply.down_token.clone(),
        edge: edge_info(&edge_hello, &config.pin),
        turn_secret: reply.turn_secret,
        ended,
        driver,
        timeout: config.connect_timeout,
    })
}

/// The local node's side of the down connection: the edge node's calls. A call of a generation other than the current
/// up connection's is refused.
struct LocalService {
    current: Arc<AtomicU64>,
    hello: super::proto::Hello,
    /// The server the queue and delegated calls run on; none without [`LocalConfig::web`].
    factory: Option<Weak<ExecutionContextFactory>>,
    /// The edge node's instance id, from its Hello: the only media node whose replies this link delivers (E7).
    edge_node: String,
    /// What this member tells of itself, and drains through (plans/Edge-Node2.md §4.4).
    member: Arc<dyn PoolMember>,
    /// The dialer's: this node is stopping.
    stopping: Arc<AtomicBool>,
}

#[tonic::async_trait]
impl LocalLink for LocalService {
    async fn hello(&self, request: Request<LocalHelloRequest>) -> Result<Response<LocalHelloReply>, Status> {
        let request = request.into_inner();
        self.fenced(request.generation)?;
        let edge = request.hello.unwrap_or_default();
        check_versions(LINK_VERSION, edge.link_version, "edge node").map_err(Status::failed_precondition)?;
        Ok(Response::new(LocalHelloReply {
            hello: Some(self.hello.clone()),
        }))
    }

    async fn ping(&self, request: Request<PingRequest>) -> Result<Response<PingReply>, Status> {
        let generation = request.into_inner().generation;
        self.fenced(generation)?;
        Ok(Response::new(PingReply { generation }))
    }

    async fn deliver(&self, request: Request<DeliverRequest>) -> Result<Response<DeliverReply>, Status> {
        let request = request.into_inner();
        self.fenced(request.generation)?;
        let factory = self.factory()?;
        let item = super::queue::from_proto(request.item.unwrap_or_default());
        let node = factory
            .upgrade()
            .map(|factory| moqui_core::edge::queue::local_node_id(&factory))
            .ok_or_else(|| Status::unavailable("the server is stopping"))?;
        let applied = super::queue::receive(&factory, &node, item).await;
        Ok(Response::new(super::queue::reply(&applied)))
    }

    async fn put_content(
        &self,
        request: Request<tonic::Streaming<ContentChunk>>,
    ) -> Result<Response<PutContentReply>, Status> {
        let factory = self.factory()?;
        let node = factory
            .upgrade()
            .map(|factory| moqui_core::edge::queue::local_node_id(&factory))
            .ok_or_else(|| Status::unavailable("the server is stopping"))?;
        let reply = super::queue::receive_content(&factory, &node, request.into_inner(), |generation| {
            self.fenced(generation)
        })
        .await?;
        Ok(Response::new(reply))
    }

    async fn signal(&self, request: Request<tonic::Streaming<SignalMessage>>) -> Result<Response<SignalReply>, Status> {
        self.factory()?;
        let take = super::signal::local_takes(&self.edge_node);
        let reply = super::signal::receive(request.into_inner(), |generation| self.fenced(generation), take).await?;
        Ok(Response::new(reply))
    }

    async fn members(&self, request: Request<MembersRequest>) -> Result<Response<MemberStatus>, Status> {
        self.fenced(request.into_inner().generation)?;
        let (_, status) = member_status(&self.member, self.stopping.load(Ordering::Acquire)).await;
        Ok(Response::new(status))
    }

    async fn drain(&self, request: Request<DrainRequest>) -> Result<Response<MemberStatus>, Status> {
        let request = request.into_inner();
        self.fenced(request.generation)?;
        let member = Arc::clone(&self.member);
        let drain = request.drain;
        tokio::task::spawn_blocking(move || member.drain(drain))
            .await
            .map_err(|_| Status::internal("the drain did not run"))?
            .map_err(Status::internal)?;
        tracing::warn!(
            "Edge link: the edge node {} this member",
            if drain { "drains" } else { "resumes" }
        );
        let (_, status) = member_status(&self.member, self.stopping.load(Ordering::Acquire)).await;
        Ok(Response::new(status))
    }

    async fn delegate(&self, request: Request<DelegateRequest>) -> Result<Response<DelegateReply>, Status> {
        let request = request.into_inner();
        self.fenced(request.generation)?;
        let factory = self.factory()?;
        let json = |text: &str| serde_json::from_str(text).unwrap_or(serde_json::Value::Null);
        let call = DelegateCall {
            parameters: json(&request.parameters_json),
            caller: json(&request.caller_json),
            service: request.service,
            operation_id: request.operation_id,
            timeout: None,
            node: None,
        };
        // the edge node's own wait (E8), counted from now: the time on the pool's queue is part of it
        let received = std::time::Instant::now();
        let budget = (request.timeout_ms > 0).then(|| Duration::from_millis(request.timeout_ms));
        let answer = crate::worker::on_pool(&factory, move |factory| {
            let mut call = call;
            call.timeout = budget.map(|budget| budget.saturating_sub(received.elapsed()));
            Ok(moqui_core::edge::delegate::run(factory, &call))
        })
        .await
        .map_err(|err| Status::internal(err.message().to_string()))?;
        Ok(Response::new(DelegateReply {
            result_json: if answer.result.is_null() {
                String::new()
            } else {
                answer.result.to_string()
            },
            errors: answer.errors,
        }))
    }
}

impl LocalService {
    fn factory(&self) -> Result<Weak<ExecutionContextFactory>, Status> {
        self.factory
            .clone()
            .ok_or_else(|| Status::unavailable("this node serves no queue or delegated calls on its edge link"))
    }

    fn fenced(&self, generation: u64) -> Result<(), Status> {
        if generation == self.current.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(Status::failed_precondition("stale link generation"))
        }
    }
}

/// An open down connection, served until it closes or is dropped.
struct DownLink {
    ended: watch::Receiver<bool>,
    driver: tokio::task::JoinHandle<()>,
}

impl Drop for DownLink {
    fn drop(&mut self) {
        self.driver.abort();
    }
}

async fn connect_down(
    config: &LocalConfig,
    prepared: &Prepared,
    generation: u64,
    edge_node: &str,
    current: &Arc<AtomicU64>,
    stopping: &Arc<AtomicBool>,
    token: &[u8],
) -> Result<DownLink, LinkError> {
    let mut role = vec![ROLE_DOWN];
    role.extend_from_slice(&generation.to_be_bytes());
    role.extend_from_slice(token);
    let stream = dial_role(prepared, config.connect_timeout, &role).await?;
    let factory = config.web.as_ref().map(super::down::LocalWeb::factory);
    let link = LocalLinkServer::new(LocalService {
        current: Arc::clone(current),
        hello: hello(&config.instance_id, &config.node_name, &[], &[]),
        factory: factory.clone(),
        edge_node: edge_node.to_string(),
        member: pool_member(config),
        stopping: Arc::clone(stopping),
    })
    .max_decoding_message_size(super::MAX_MESSAGE)
    .max_encoding_message_size(super::MAX_MESSAGE);
    // the PASS authority for the edge node's mr-pass, the link its client (plans/archive/Edge-Node.md §5.5); it runs on the
    // server's factory, so only with a web app
    let mut grpc = tonic::service::Routes::new(link);
    if let Some(factory) = factory {
        grpc = grpc.add_service(crate::pass_authority::link_server(factory));
    }
    // the link's calls, and this node's web app and tunnels (plans/archive/Edge-Node.md §5.3)
    let builder = super::down::builder(config.keepalive, config.web.as_ref());
    let service = super::down::service(grpc, config.web.clone());
    let (ended_tx, ended) = watch::channel(false);
    let driver = tokio::spawn(async move {
        let served = builder
            .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
            .await;
        if let Err(err) = served {
            tracing::debug!("edge link down connection ended: {err}");
        }
        let _ = ended_tx.send(true);
    });
    Ok(DownLink { ended, driver })
}

/// What the dialer shares with its handle: the status, and the up connection's client for Unpair.
struct Shared {
    status: Mutex<LinkStatus>,
    /// The current up connection's generation, which the down connection's calls must name.
    current: Arc<AtomicU64>,
    up: Mutex<Option<LinkClient>>,
    /// The edge node's pinned instance id (empty until a first link pins it, for a node paired before E7).
    pinned: Mutex<String>,
    /// The registrar's realm and whether it is on, as the last Configure's offer said since this link's Hello: an
    /// all-in-one node registers the phones of the domain a Configure names (E12), which its Hello may predate.
    sip_realm: Mutex<Option<(String, bool)>>,
    /// Wakes the keeper of the edge node's rows (`keep_media_node`) when `sip_realm` changes.
    sip_changed: tokio::sync::Notify,
    /// Wakes the link to Ping now: this member's state changed (plans/Edge-Node2.md §4.4).
    ping_now: tokio::sync::Notify,
    /// This node is stopping: its links say so.
    stopping: Arc<AtomicBool>,
}

impl Shared {
    fn update(&self, change: impl FnOnce(&mut LinkStatus)) {
        change(&mut lock(&self.status));
    }
}

/// The running dialer of a paired local node. Dropping it, or [`Dialer::stop`], closes the link.
pub struct Dialer {
    shared: Arc<Shared>,
    stop: watch::Sender<bool>,
    timeout: Duration,
}

impl Drop for Dialer {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}

impl Dialer {
    /// Start dialing on `runtime`, until [`Dialer::stop`], a drop, or `shutdown` turns true. `telemetry` gets the link's
    /// round trips and reconnects (`moqui.edgelink.*`).
    pub fn start(
        config: LocalConfig,
        runtime: &tokio::runtime::Handle,
        shutdown: watch::Receiver<bool>,
        telemetry: Option<Arc<Telemetry>>,
    ) -> Result<Self, String> {
        let prepared = Arc::new(prepare(&config)?);
        let shared = Arc::new(Shared {
            status: Mutex::new(LinkStatus {
                state: LinkState::Connecting,
                url: config.url.clone(),
                peer: EdgeInfo {
                    fingerprint: config.pin.clone(),
                    ..EdgeInfo::default()
                },
                ..LinkStatus::default()
            }),
            current: Arc::new(AtomicU64::new(0)),
            up: Mutex::new(None),
            pinned: Mutex::new(config.edge_id.clone()),
            sip_realm: Mutex::new(None),
            sip_changed: tokio::sync::Notify::new(),
            ping_now: tokio::sync::Notify::new(),
            stopping: Arc::new(AtomicBool::new(false)),
        });
        let (stop, stop_rx) = watch::channel(false);
        let timeout = config.connect_timeout;
        // replication notes writes from now on, and keeps its notes across links and dialers (plans/archive/Edge-Node.md §5.2)
        let replica = config
            .web
            .as_ref()
            .and_then(|web| web.factory().upgrade())
            .and_then(|factory| {
                let settings = moqui_core::edge::settings(&factory);
                let facade = factory.entity()?;
                moqui_core::edge::replica::enable(&facade, &settings.sets)
            });
        let links = Links {
            prepared,
            shared: Arc::clone(&shared),
            telemetry,
            replica,
        };
        runtime.spawn(run(config, links, stop_rx, shutdown));
        Ok(Self { shared, stop, timeout })
    }

    pub fn status(&self) -> LinkStatus {
        lock(&self.shared.status).clone()
    }

    /// The up connection's client while the link is up, for a call of the wizard's (hook.rs).
    pub(crate) fn up_client(&self) -> Option<LinkClient> {
        if lock(&self.shared.status).state != LinkState::Linked {
            return None;
        }
        lock(&self.shared.up).clone()
    }

    /// What the edge node offers this node now (the link's GetOffer).
    pub async fn offer(&self) -> Result<EdgeOffer, String> {
        offer_on(self.up_client(), self.timeout).await
    }

    /// Send the edge node this node's part of the setup (the link's Configure).
    pub async fn configure(&self, setup: &EdgeSetup) -> Result<Configured, String> {
        let configured = configure_on(self.up_client(), setup, self.timeout).await?;
        self.note_configured(&configured);
        Ok(configured)
    }

    /// After a Configure: the registrar's realm and whether it is on, as its offer says, go to the edge node's
    /// `SipNode` row now (the realm `create#SipDevice` gives a phone), not at the next link's Hello. An all-in-one
    /// node's registrar takes the domain the Configure named (E12); a dedicated edge node's offer says what its Hello
    /// did.
    pub(crate) fn note_configured(&self, configured: &Configured) {
        let sip = &configured.offer.sip;
        if !sip.available {
            return;
        }
        let realm = (sip.domain.clone(), sip.registrar);
        let changed = lock(&self.shared.sip_realm).replace(realm.clone()) != Some(realm.clone());
        self.shared.update(|status| {
            status.peer.sip_domain.clone_from(&realm.0);
            status.peer.sip_registrar = realm.1;
        });
        if changed {
            self.shared.sip_changed.notify_one();
        }
    }

    /// This member's state changed (drained or resumed on this node): its link tells the edge node now.
    pub fn state_changed(&self) {
        self.shared.ping_now.notify_one();
    }

    /// Stop dialing and close the link.
    pub fn stop(&self) {
        let _ = self.stop.send(true);
    }

    /// Ask the edge node to revoke this node's key on the up connection, then stop. The dialer stops also when the edge
    /// node cannot be reached; the error says so.
    pub async fn unpair(&self) -> Result<(), String> {
        let client = lock(&self.shared.up).clone();
        let outcome = match client {
            Some(mut client) => match tokio::time::timeout(self.timeout, client.unpair(UnpairRequest {})).await {
                Ok(Ok(_)) => Ok(()),
                Ok(Err(status)) => Err(format!("the edge node did not unpair: {}", status.message())),
                Err(_) => Err("the edge node did not answer Unpair in time".to_string()),
            },
            None => Err("the link is not up, so the edge node was not told; revoke this node there".to_string()),
        };
        self.stop();
        outcome
    }
}

const NOT_UP: &str = "the link to the edge node is not up";

/// The edge node's offer, asked on `client`'s up connection.
pub(crate) async fn offer_on(client: Option<LinkClient>, timeout: Duration) -> Result<EdgeOffer, String> {
    let mut client = client.ok_or_else(|| NOT_UP.to_string())?;
    match tokio::time::timeout(timeout, client.get_offer(OfferRequest {})).await {
        Ok(Ok(reply)) => Ok(super::offer::offer_of(reply.into_inner())),
        Ok(Err(status)) => Err(format!(
            "the edge node did not answer with its offer: {}",
            status.message()
        )),
        Err(_) => Err(format!(
            "the edge node is not answering: no offer in {} s",
            timeout.as_secs()
        )),
    }
}

/// Send `setup` on `client`'s up connection.
pub(crate) async fn configure_on(
    client: Option<LinkClient>,
    setup: &EdgeSetup,
    timeout: Duration,
) -> Result<Configured, String> {
    let mut client = client.ok_or_else(|| NOT_UP.to_string())?;
    let request = super::offer::request_of(setup);
    match tokio::time::timeout(timeout, client.configure(request)).await {
        Ok(Ok(reply)) => Ok(super::offer::configured_of(reply.into_inner())),
        Ok(Err(status)) => Err(format!("the edge node refused the setup: {}", status.message())),
        Err(_) => Err("the edge node did not answer the setup in time".to_string()),
    }
}

/// The edge node's instance id, from its Hello, before the link carries anything: the one pinned (pinned now when none
/// was), not this node's own, and no live member of this node's cluster (plans/archive/Edge-Node.md §5.4). An edge node that
/// took another media node's id would get that node's call signaling and media row; it is refused, as a changed key is.
async fn check_peer(config: &LocalConfig, shared: &Shared, seen: &str) -> Result<(), String> {
    let pinned = lock(&shared.pinned).clone();
    super::check_edge_id(&pinned, seen, &config.instance_id)?;
    if let Some(factory) = config.web.as_ref().map(super::down::LocalWeb::factory) {
        let id = seen.to_string();
        let member = crate::worker::on_pool_without_authz(&factory, move |factory| {
            if moqui_core::cluster::instance_id(factory).as_deref() == Some(id.as_str()) {
                return Ok(true);
            }
            match factory.entity() {
                Some(facade) => moqui_core::cluster::is_live_member(factory, &facade, &id),
                None => Ok(false),
            }
        })
        .await
        .map_err(|err| format!("the edge node's instance id was not checked: {}", err.message()))?;
        if member {
            return Err(format!(
                "the edge node names {seen}, which is this node or a live member of its cluster"
            ));
        }
    }
    if pinned.is_empty() {
        *lock(&shared.pinned) = seen.to_string();
        if let Some(on_pin) = &config.on_pin {
            on_pin(seen);
        }
        tracing::info!("Edge link: the edge node's instance id {seen} is pinned");
    }
    Ok(())
}

/// How long a member that stops waits for its last Ping, which says so.
const STOPPING_TOLD: Duration = Duration::from_secs(1);
/// How often a member that is starting pings, so its link says it is live soon after it is.
const STARTING_PING: Duration = Duration::from_millis(500);

/// The pairing the edge node keys this node by, as its Hello answer said (plans/Edge-Node2.md §4.4): this server's
/// queue rows and receipts are kept by it from now (`moqui_core::edge::queue::set_pairing_id`). When `edge_node_pairing`
/// says another, or nothing, that is logged as an error and the runtime's `.env` is written again with this one.
fn note_pairing(config: &LocalConfig, pairing: &str) {
    let Some(factory) = config.web.as_ref().and_then(|web| web.factory().upgrade()) else {
        return;
    };
    if pairing.is_empty() {
        return;
    }
    moqui_core::edge::queue::set_pairing_id(&factory, pairing);
    let written = moqui_lib::env::get_prop_or_env(moqui_core::edge::NODE_PAIRING);
    if written.trim() == pairing {
        return;
    }
    tracing::error!(
        "Edge link: the edge node keys this node as pairing {pairing}, and edge_node_pairing says {:?}; runtime/.env \
         is written with {pairing}, which this server uses from now",
        written.trim()
    );
    let env = factory.runtime_dir().join(".env");
    let set = [(moqui_core::edge::NODE_PAIRING.to_string(), pairing.to_string())];
    if let Err(err) = moqui_lib::env::update_dotenv_file(&env, &set, &[]) {
        tracing::error!(
            "Edge link: edge_node_pairing was not written to {}: {err}",
            env.display()
        );
    }
}

/// How one attempt ended.
enum Ended {
    /// Stopped on purpose.
    Stopped,
    /// Lost after it was up (or failed before), with why.
    Lost { linked: bool, error: String },
    /// Refused by the edge node.
    Refused(String),
}

/// What each link of a dialer shares.
#[derive(Clone)]
struct Links {
    prepared: Arc<Prepared>,
    shared: Arc<Shared>,
    telemetry: Option<Arc<Telemetry>>,
    replica: Option<Arc<moqui_core::edge::replica::Replica>>,
}

async fn run(config: LocalConfig, links: Links, mut stop: watch::Receiver<bool>, mut shutdown: watch::Receiver<bool>) {
    let Links {
        ref shared,
        ref telemetry,
        ..
    } = links;
    let mut attempt: u32 = 0;
    let mut refusals: u32 = 0;
    loop {
        if *stop.borrow() || *shutdown.borrow() {
            break;
        }
        // a refusal stays shown while the dialer tries again
        shared.update(|status| {
            if status.state != LinkState::Refused {
                status.state = LinkState::Connecting;
            }
        });
        let once = tokio::spawn(link_once(config.clone(), links.clone(), stop.clone(), shutdown.clone()));
        let ended = match once.await {
            Ok(ended) => ended,
            Err(err) if err.is_panic() => {
                tracing::error!("the edge link to {} panicked; dialing again", config.url);
                Ended::Lost {
                    linked: false,
                    error: "the link task panicked".to_string(),
                }
            }
            Err(_) => Ended::Stopped,
        };
        *lock(&shared.up) = None;
        shared.current.store(0, Ordering::Release);
        let refused = matches!(ended, Ended::Refused(_));
        match ended {
            Ended::Stopped => break,
            Ended::Lost { linked, error } => {
                refusals = 0;
                if linked {
                    attempt = 0;
                    if let Some(telemetry) = &telemetry {
                        telemetry.add_counter("moqui.edgelink.reconnects", 1, Default::default());
                    }
                    shared.update(|status| status.reconnects += 1);
                    tracing::warn!("The edge link to {} was lost: {error}", config.url);
                } else {
                    tracing::debug!("The edge link to {} did not come up: {error}", config.url);
                }
                shared.update(|status| {
                    status.state = LinkState::Retrying;
                    status.last_error = error;
                    status.rtt_ms = None;
                });
            }
            Ended::Refused(error) => {
                tracing::warn!("The edge node at {} refused the link: {error}", config.url);
                shared.update(|status| {
                    status.state = LinkState::Refused;
                    status.last_error = error;
                    status.rtt_ms = None;
                });
            }
        }
        // a refused node waits much longer: dialing again helps only once its operator acts
        let delay = if refused {
            refusals = refusals.saturating_add(1);
            config.refused_backoff.delay(refusals - 1, jitter())
        } else {
            attempt = attempt.saturating_add(1);
            config.backoff.delay(attempt - 1, jitter())
        };
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            _ = stop.wait_for(|stopped| *stopped) => break,
            _ = shutdown.wait_for(|stopped| *stopped) => break,
        }
    }
    shared.update(|status| {
        if status.state == LinkState::Linked || status.state == LinkState::Connecting {
            status.state = LinkState::Retrying;
            status.last_error = "stopped".to_string();
        }
    });
}

async fn link_once(
    config: LocalConfig,
    links: Links,
    mut stop: watch::Receiver<bool>,
    mut shutdown: watch::Receiver<bool>,
) -> Ended {
    let Links {
        prepared,
        shared,
        telemetry,
        replica,
    } = links;
    let failed = |error: LinkError| match error {
        LinkError::Refused(message) => Ended::Refused(message),
        LinkError::Failed(error) => Ended::Lost { linked: false, error },
    };
    // the connect phase gives way to a stop or a shutdown at once (no await in a branch: a watch's borrow is not Send)
    let connecting = async {
        let up = connect_up_with(&config, &prepared).await?;
        check_peer(&config, &shared, &up.edge.instance_id)
            .await
            .map_err(LinkError::Refused)?;
        shared.current.store(up.generation, Ordering::Release);
        note_pairing(&config, &up.pairing_id);
        let down = connect_down(
            &config,
            &prepared,
            up.generation,
            &up.edge.instance_id,
            &shared.current,
            &shared.stopping,
            &up.down_token,
        )
        .await?;
        Ok::<_, LinkError>((up, down))
    };
    let connected = tokio::select! {
        connected = connecting => Some(connected),
        _ = stop.wait_for(|stopped| *stopped) => None,
        _ = shutdown.wait_for(|stopped| *stopped) => None,
    };
    let (up, down) = match connected {
        Some(Ok(pair)) => pair,
        Some(Err(error)) => return failed(error),
        None => return Ended::Stopped,
    };
    *lock(&shared.up) = Some(up.client.clone());
    // this link's Hello tells the registrar's realm until a Configure on it says otherwise
    *lock(&shared.sip_realm) = None;
    // the outbox and the replicated sets go up while this link lasts
    let _work = Work::start(&config, &up, &shared, replica, telemetry.clone());
    let connected = moqui_core::user::now_millis();
    shared.update(|status| {
        status.state = LinkState::Linked;
        status.generation = up.generation;
        status.last_connect = Some(connected);
        status.last_error.clear();
        status.peer = up.edge.clone();
    });
    tracing::info!(
        "The edge link to {} is up (generation {}, edge node {})",
        config.url,
        up.generation,
        up.edge.name
    );
    let mut down_ended = down.ended.clone();
    let mut next_ping = tokio::time::Instant::now();
    let member = pool_member(&config);
    loop {
        // no await in a branch: a watch's borrow is not Send
        let event = tokio::select! {
            () = up.closed() => Event::UpClosed,
            _ = down_ended.wait_for(|ended| *ended) => Event::DownClosed,
            () = tokio::time::sleep_until(next_ping) => Event::Ping,
            () = shared.ping_now.notified() => Event::Ping,
            _ = stop.wait_for(|stopped| *stopped) => Event::Stop,
            _ = shutdown.wait_for(|stopped| *stopped) => Event::Stop,
        };
        match event {
            Event::UpClosed => {
                return Ended::Lost {
                    linked: true,
                    error: "the up connection closed".to_string(),
                }
            }
            Event::DownClosed => {
                return Ended::Lost {
                    linked: true,
                    error: "the down connection closed".to_string(),
                }
            }
            Event::Stop => {
                // a member stopping says so before its links close, so it gets no new work (plans/Edge-Node2.md §4.4)
                shared.stopping.store(true, Ordering::Release);
                // the state alone, read from nothing: the server may be stopping under it
                let status = super::status_to_proto(MemberState::Stopping, &[]);
                let _ = tokio::time::timeout(STOPPING_TOLD, up.ping_with(Some(status))).await;
                return Ended::Stopped;
            }
            Event::Ping => {
                let (state, status) = member_status(&member, shared.stopping.load(Ordering::Acquire)).await;
                // a member starting is live within a moment: told then, not a Ping later
                let every = if state == MemberState::Starting {
                    config.ping_every.min(STARTING_PING)
                } else {
                    config.ping_every
                };
                next_ping = tokio::time::Instant::now() + every;
                let rtt = match up.ping_with(Some(status)).await {
                    Ok(rtt) => rtt,
                    Err(error) => return Ended::Lost { linked: true, error },
                };
                shared.update(|status| status.rtt_ms = Some(rtt.as_secs_f64() * 1000.0));
                if let Some(telemetry) = &telemetry {
                    telemetry.record_duration("moqui.edgelink.rtt", rtt.as_secs_f64(), Default::default());
                }
            }
        }
    }
}

/// The tasks that send this node's work up one link: its outbox, its replicated sets, its messages for the edge node
/// on cluster topics, and the heartbeat of the edge node's `CallMediaNode` row. Dropped with the link, which also
/// takes the route of those messages away.
struct Work {
    tasks: Vec<tokio::task::JoinHandle<()>>,
    _route: Option<moqui_core::edge::signal::RouteGuard>,
}

impl Drop for Work {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// The service that writes each phone number's fallback for while this node is away (`moqui.call.PhoneNumberFallback`,
/// the replicated set `voiceFallbacks`, E8), and queues their greetings to the edge node.
const REFRESH_FALLBACKS: &str = "org.moqui.call.VoiceServices.refresh#PhoneFallbacks";
/// How often the fallbacks are written again while the link lasts, besides the services that change their sources.
const REFRESH_EVERY: Duration = Duration::from_secs(60);

/// Run `service` as internal work in the current context; its errors are taken out of the context.
fn run_internal(
    service: &str,
    parameters: &std::collections::BTreeMap<String, moqui_lib::object::Value>,
) -> Result<(), String> {
    let _authz =
        moqui_core::context::ExecutionContext::with_current(moqui_core::context::ExecutionContext::disable_authz);
    // its errors apart from the caller's, which stay
    moqui_core::context::ExecutionContext::with_current(|ec| ec.message().push_errors());
    let ran = moqui_core::service::sync(service, parameters);
    let errors = moqui_core::context::ExecutionContext::with_current(|ec| {
        let mut messages = ec.message();
        let errors = messages.errors().to_vec();
        messages.clear_errors();
        messages.pop_errors();
        errors
    })
    .unwrap_or_default();
    match ran {
        Err(err) => Err(err.message().to_string()),
        Ok(_) if !errors.is_empty() => Err(errors.join("; ")),
        Ok(_) => Ok(()),
    }
}

/// The edge node's `CallMediaNode` and `SipNode` rows while this link lasts (plans/archive/Edge-Node.md §5.4), on the member of a
/// clustered local node that is the replication sender (`role`, plans/Edge-Node2.md §4.4) and on no other: written when
/// it becomes the sender and again every `every`, from the link's liveness, so they go stale (and their calls end, `expire#Calls`) only when
/// the link has been down past the nodes' grace. An edge node with no media, or no SIP engine, has that row removed.
/// With this node's `edge_calls_enabled` off the media row is draining, read again each time, so the wizard's choice
/// holds (E10 calls). The phone numbers' fallbacks are written at the start and every [`REFRESH_EVERY`] (E8), while
/// this node's `edge_sip_enabled` is on (E10 phones). The SIP row's realm is the Hello's until a Configure's offer
/// says another ([`Dialer::note_configured`]), which writes the row at once.
async fn keep_media_node(
    factory: Weak<ExecutionContextFactory>,
    edge: EdgeInfo,
    every: Duration,
    shared: Arc<Shared>,
    mut role: tokio::sync::watch::Receiver<Option<u64>>,
) {
    let media_stopping = edge.media_address.is_empty();
    let sip_stopping = media_stopping || (edge.sip_address.is_empty() && edge.sip_tls_address.is_empty());
    let mut refreshed: Option<std::time::Instant> = None;
    loop {
        if role.borrow_and_update().is_none() {
            // another member is the sender, and keeps these rows; the fallbacks are written again when this one is
            refreshed = None;
            if role.changed().await.is_err() {
                return;
            }
            continue;
        }
        let mut row = edge.clone();
        if let Some((domain, registrar)) = lock(&shared.sip_realm).clone() {
            row.sip_domain = domain;
            row.sip_registrar = registrar;
        }
        // the fallbacks (and their greetings) only while phones go through the edge node (E10 phones)
        let refresh = refreshed.is_none_or(|at| at.elapsed() >= REFRESH_EVERY) && moqui_core::edge::sip::sip_enabled();
        let kept = crate::worker::on_pool(&factory, move |_| {
            let media = moqui_core::edge::calls::register_row(&row, media_stopping);
            let sip = moqui_core::edge::calls::register_sip_row(&row, sip_stopping);
            let fallbacks = if refresh {
                run_internal(REFRESH_FALLBACKS, &std::collections::BTreeMap::new())
            } else {
                Ok(())
            };
            Ok((media, sip, fallbacks))
        })
        .await;
        match kept {
            Ok((media, sip, fallbacks)) => {
                if let Err(err) = media {
                    tracing::warn!("the edge node's call media row was not kept: {err}");
                }
                if let Err(err) = sip {
                    tracing::warn!("the edge node's SIP node row was not kept: {err}");
                }
                match fallbacks {
                    Ok(()) if refresh => refreshed = Some(std::time::Instant::now()),
                    Ok(()) => {}
                    Err(err) => tracing::warn!("the phone numbers' fallbacks were not written: {err}"),
                }
            }
            Err(err) => tracing::warn!("the edge node's call rows were not kept: {}", err.message()),
        }
        if media_stopping && sip_stopping {
            return;
        }
        tokio::select! {
            () = tokio::time::sleep(every) => {}
            () = shared.sip_changed.notified() => {}
            _ = role.changed() => {}
        }
    }
}

impl Work {
    fn start(
        config: &LocalConfig,
        up: &UpLink,
        shared: &Arc<Shared>,
        replica: Option<Arc<moqui_core::edge::replica::Replica>>,
        telemetry: Option<Arc<Telemetry>>,
    ) -> Self {
        let none = || Self {
            tasks: Vec::new(),
            _route: None,
        };
        let Some(factory) = config.web.as_ref().map(super::down::LocalWeb::factory) else {
            return none();
        };
        let Some(node) = factory.upgrade().map(|factory| {
            // the edge node's TURN checks this pairing's secret: the credentials of its TURN URLs are signed with it and
            // name the pairing (E16)
            moqui_core::edge::media::set_peer_turn(&factory, &up.edge.instance_id, &up.turn_secret, &up.pairing_id);
            // its media node row may be kept until unpairing (moqui_core::edge::calls::forget_rows)
            moqui_core::edge::calls::allow_row(&factory, &up.edge.instance_id);
            moqui_core::edge::queue::local_node_id(&factory)
        }) else {
            return none();
        };
        let mut tasks = Vec::new();
        // the messages of calls whose media is on the edge node go over this link (moqui_core::edge::signal)
        let route = (!up.edge.instance_id.is_empty()).then(|| {
            let (route, queue) = super::signal::route(&up.edge.instance_id, up.generation);
            let client = up.client.clone();
            tasks.push(tokio::spawn(super::signal::forward(queue, move |messages| {
                let mut client = client.clone();
                let call: super::signal::Call = Box::pin(async move { client.signal(messages).await.map(drop) });
                call
            })));
            route
        });
        // one member of a clustered local node sends replication and keeps the link's own rows (plans/Edge-Node2.md §4.4)
        let member = factory
            .upgrade()
            .and_then(|factory| moqui_core::cluster::instance_id(&factory))
            .unwrap_or_else(|| config.instance_id.clone());
        tasks.push(tokio::spawn(super::replicate::keep_sender_lock(
            factory.clone(),
            member,
        )));
        let role = factory.upgrade().map(|factory| super::replicate::sender_role(&factory));
        if let (false, Some(role)) = (up.edge.instance_id.is_empty(), role.clone()) {
            tasks.push(tokio::spawn(keep_media_node(
                factory.clone(),
                up.edge.clone(),
                config.ping_every,
                Arc::clone(shared),
                role,
            )));
        }
        let client = up.client.clone();
        let generation = up.generation;
        let owner = format!("{}/{generation}", config.instance_id);
        let send = move |mut item: moqui_core::edge::queue::QueueItem, content: Option<Vec<u8>>| {
            let mut client = client.clone();
            async move {
                item.content_location = match content {
                    Some(content) => {
                        let chunks = super::queue::content_chunks(generation, &item.queue_item_id, &content);
                        match client.put_content(chunks).await {
                            Ok(reply) => reply.into_inner().content_location,
                            Err(status) => return super::queue::delivery_error(&status),
                        }
                    }
                    None => String::new(),
                };
                let request = DeliverRequest {
                    generation,
                    item: Some(super::queue::to_proto(&item)),
                };
                match client.deliver(request).await {
                    Ok(reply) => Ok(super::queue::applied(&reply.into_inner())),
                    Err(status) => super::queue::delivery_error(&status),
                }
            }
        };
        tasks.push(tokio::spawn(super::queue::deliver(
            factory.clone(),
            node,
            moqui_core::edge::queue::TO_EDGE,
            owner,
            telemetry,
            send,
        )));
        if let (Some(replica), Some(role)) = (replica, role) {
            let sender = super::replicate::Sender {
                factory,
                replica,
                client: up.client.clone(),
                timeout: super::queue::ANSWER_TIMEOUT,
                role,
            };
            tasks.push(tokio::spawn(sender.run()));
        }
        Self { tasks, _route: route }
    }
}

enum Event {
    UpClosed,
    DownClosed,
    Ping,
    Stop,
}

/// Whether the dialer's link is up, for the `moqui.edgelink.up` gauge.
pub(crate) fn is_linked(dialer: &Dialer) -> bool {
    lock(&dialer.shared.status).state == LinkState::Linked
}
