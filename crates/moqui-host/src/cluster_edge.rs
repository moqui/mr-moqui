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

//! Cluster HTTPS listener and the blocking peer client.
//!
//! Empty `cluster_https_port` is 8444 on `cluster_bind_address` (default `0.0.0.0`, so other hosts reach it; the
//! listener takes only peers that present the cluster certificate). `0` is ephemeral: the listener binds port 0
//! and [`ClusterEdge::address`] is the address the kernel chose. The public HTTP and HTTPS listeners do not ask
//! for a client certificate.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use moqui_lib::sync::lock;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use moqui_core::cluster::Sender;
use moqui_core::context::{ErrorKind, ExecutionContextFactory};
use moqui_core::notify::TopicMessage;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::DigitallySignedStruct;
use sha2::{Digest, Sha256};

const DEFAULT_PORT: u16 = 8444;
const BODY_LIMIT: usize = 1024 * 1024;
const HEADER_LIMIT: usize = 16 * 1024;
/// How often a connection with nothing to read checks whether the listener is stopping.
const READ_IDLE: Duration = Duration::from_millis(250);
const CLIENT_IO: Duration = Duration::from_secs(10);
/// How long a kept-alive connection may sit between exchanges. The listener closes it then, so the sender drops its
/// end too, which also lets go of a member that has left the cluster.
const CONNECTION_IDLE: Duration = Duration::from_secs(60);
/// Connections served at once; one more is closed, and the peer's post tries again.
const MAX_CONNECTIONS: usize = 64;
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

/// The HTTPS sender, which the SSH sender also uses for a peer whose base URL is `https://`.
pub(crate) fn https_sender() -> Result<Arc<dyn Sender>, HostError> {
    let material = load_material()?;
    let config = client_config(&material)?;
    Ok(Arc::new(PeerSender {
        config,
        live: Mutex::new(HashMap::new()),
    }))
}

/// The shared cluster key `cluster_tls_key`, as DER, for the SSH channel's derived login key.
#[cfg(feature = "ssh")]
pub(crate) fn cluster_key_der() -> Result<Vec<u8>, HostError> {
    let material = load_material()?;
    Ok(material.key)
}

/// Bind the cluster listener. Call after [`moqui_core::cluster::start`] so the member row exists.
pub(crate) fn listen(
    factory: Arc<ExecutionContextFactory>,
    posts: Arc<Mutex<Vec<String>>>,
) -> Result<ClusterEdge, HostError> {
    let material = load_material()?;
    let config = Arc::new(server_config(&material)?);
    let port = listen_port(&moqui_lib::env::get_prop_or_env("cluster_https_port"))?;
    let bind = SocketAddr::from((bind_address()?, port));
    let listener = TcpListener::bind(bind).map_err(|err| HostError::new(format!("could not bind {bind}: {err}")))?;
    let address = listener
        .local_addr()
        .map_err(|err| HostError::new(format!("cluster listener has no address: {err}")))?;
    let stop = Arc::new(AtomicBool::new(false));
    let joins: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));
    let accept_stop = Arc::clone(&stop);
    let accept_posts = Arc::clone(&posts);
    let accept_joins = Arc::clone(&joins);
    let accept = thread::Builder::new()
        .name("moqui-cluster".to_string())
        .spawn(move || accept_loop(listener, config, factory, accept_posts, accept_stop, accept_joins))
        .map_err(|err| HostError::new(format!("cluster listener did not start: {err}")))?;
    Ok(ClusterEdge {
        address,
        posts,
        stop,
        joins,
        accept: Some(accept),
    })
}

pub(crate) struct ClusterEdge {
    address: SocketAddr,
    posts: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    joins: Arc<Mutex<Vec<JoinHandle<()>>>>,
    accept: Option<JoinHandle<()>>,
}

impl ClusterEdge {
    pub(crate) fn address(&self) -> SocketAddr {
        self.address
    }

    pub(crate) fn posts(&self) -> Arc<Mutex<Vec<String>>> {
        Arc::clone(&self.posts)
    }
}

impl Drop for ClusterEdge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_millis(200));
        if let Some(handle) = self.accept.take() {
            join_handle("cluster listener", handle);
        }
        let children = {
            let mut guard = lock(&self.joins);
            std::mem::take(&mut *guard)
        };
        for handle in children {
            join_handle("cluster connection", handle);
        }
    }
}

fn accept_loop(
    listener: TcpListener,
    config: Arc<rustls::ServerConfig>,
    factory: Arc<ExecutionContextFactory>,
    posts: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    joins: Arc<Mutex<Vec<JoinHandle<()>>>>,
) {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                spawn_conn(&config, &factory, &posts, &stop, &joins, stream);
            }
            Err(err) => {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                tracing::warn!(error = %err, "cluster accept failed");
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

fn spawn_conn(
    config: &Arc<rustls::ServerConfig>,
    factory: &Arc<ExecutionContextFactory>,
    posts: &Arc<Mutex<Vec<String>>>,
    stop: &Arc<AtomicBool>,
    joins: &Arc<Mutex<Vec<JoinHandle<()>>>>,
    stream: TcpStream,
) {
    {
        // Finished connections are joined as new ones come, so the list holds only the live ones.
        let mut live = lock(joins);
        let (done, running): (Vec<_>, Vec<_>) = std::mem::take(&mut *live)
            .into_iter()
            .partition(JoinHandle::is_finished);
        *live = running;
        drop(live);
        for handle in done {
            join_handle("cluster connection", handle);
        }
    }
    if lock(joins).len() >= MAX_CONNECTIONS {
        tracing::warn!("cluster listener is at {MAX_CONNECTIONS} connections; one was closed");
        return;
    }
    let config = Arc::clone(config);
    let factory = Arc::clone(factory);
    let posts = Arc::clone(posts);
    let stop = Arc::clone(stop);
    let spawned = thread::Builder::new()
        .name("moqui-cluster-conn".to_string())
        .spawn(move || {
            if let Err(err) = serve_conn(stream, config, factory, posts, stop) {
                tracing::debug!(error = %err, "cluster connection closed");
            }
        });
    match spawned {
        Ok(handle) => lock(joins).push(handle),
        Err(err) => tracing::warn!(error = %err, "cluster connection thread did not start"),
    }
}

fn serve_conn(
    stream: TcpStream,
    config: Arc<rustls::ServerConfig>,
    factory: Arc<ExecutionContextFactory>,
    posts: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    let _ = stream.set_read_timeout(Some(READ_IDLE));
    let _ = stream.set_write_timeout(Some(CLIENT_IO));
    let _ = stream.set_nodelay(true);
    let conn = rustls::ServerConnection::new(config).map_err(|err| err.to_string())?;
    let tls = rustls::StreamOwned::new(conn, stream);
    serve_exchanges(tls, ConnLimits::DEFAULT, &stop, |exchange| {
        handle_exchange(&factory, &posts, exchange)
    })
}

/// How long a cluster connection may wait on its peer.
#[derive(Clone, Copy)]
struct ConnLimits {
    /// From accept to the end of the first request, the TLS handshake included. A peer that never presents its
    /// certificate gives its slot back after this.
    first: Duration,
    /// From the end of one exchange to the start of the next. The sender reconnects to a peer that closed it.
    idle: Duration,
    /// From the first byte of a request to its last.
    partial: Duration,
}

impl ConnLimits {
    const DEFAULT: Self = Self {
        first: CLIENT_IO,
        idle: CONNECTION_IDLE,
        partial: CLIENT_IO,
    };
}

/// Answer the requests on one connection until it closes, a limit passes, or the listener stops. The stream's read
/// timeout sets how often the limits and `stop` are checked.
fn serve_exchanges<S: Read + Write>(
    mut stream: S,
    limits: ConnLimits,
    stop: &AtomicBool,
    mut handle: impl FnMut(&Exchange) -> u16,
) -> Result<(), String> {
    let mut buf = Vec::new();
    let accepted = std::time::Instant::now();
    // When the last exchange ended; none yet means the first request is still due.
    let mut last_exchange: Option<std::time::Instant> = None;
    // When a request began arriving: a pause inside one is waited out up to `partial`, not refused.
    let mut partial_since: Option<std::time::Instant> = None;
    while !stop.load(Ordering::SeqCst) {
        match read_exchange(&mut stream, &mut buf) {
            Ok(Some(exchange)) => {
                partial_since = None;
                let status = handle(&exchange);
                write_status(&mut stream, status).map_err(|err| err.to_string())?;
                last_exchange = Some(std::time::Instant::now());
            }
            Ok(None) => return Ok(()),
            Err(IoFail::Timeout) if buf.is_empty() => {
                partial_since = None;
                match last_exchange {
                    None if accepted.elapsed() > limits.first => {
                        return Err("cluster connection sent no request".to_string());
                    }
                    Some(at) if at.elapsed() > limits.idle => return Ok(()),
                    _ => {}
                }
            }
            Err(IoFail::Timeout) => {
                let since = *partial_since.get_or_insert_with(std::time::Instant::now);
                if since.elapsed() > limits.partial || (last_exchange.is_none() && accepted.elapsed() > limits.first) {
                    let _ = write_status(&mut stream, 400);
                    return Err("cluster request was incomplete".to_string());
                }
            }
            Err(IoFail::Closed) => return Ok(()),
            Err(IoFail::Bad(message)) => {
                let _ = write_status(&mut stream, 400);
                return Err(message);
            }
        }
    }
    Ok(())
}

fn handle_exchange(factory: &Arc<ExecutionContextFactory>, posts: &Mutex<Vec<String>>, exchange: &Exchange) -> u16 {
    let Some(topic) = cluster_topic(request_method(&exchange.start), request_path(&exchange.start)) else {
        return 404;
    };
    apply_topic(factory, posts, topic, &exchange.body)
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
    if ![
        moqui_core::notify::ENTITY_CACHE_INVALIDATE,
        moqui_core::notify::NOTIFICATION_MESSAGE,
        moqui_core::notify::COLLAB_UPDATE,
    ]
    .contains(&topic)
    {
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
    if [
        moqui_core::notify::ENTITY_CACHE_INVALIDATE,
        moqui_core::notify::NOTIFICATION_MESSAGE,
        moqui_core::notify::COLLAB_UPDATE,
    ]
    .contains(&topic)
    {
        Some(topic)
    } else {
        None
    }
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
    fn post(&self, base_url: &str, topic: &str, payload: &[u8]) -> Result<(), CoreError> {
        if !topic_segment(topic) {
            return Err(CoreError::new("cluster topic is not a path segment"));
        }
        if let Some(mut stream) = self.take_live(base_url) {
            if write_post(&mut stream, base_url, topic, payload).is_ok() {
                self.keep(base_url, stream);
                return Ok(());
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
) -> Result<(), CoreError> {
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
        Err(IoFail::Bad(message)) => {
            return Err(CoreError::new(format!("cluster post to {base_url} failed: {message}")))
        }
    };
    let status = status_code(&exchange.start);
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(CoreError::new(format!("cluster post to {base_url} returned {status}")))
    }
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

struct Exchange {
    start: String,
    body: Vec<u8>,
}

fn request_method(start: &str) -> &str {
    start.split_whitespace().next().unwrap_or("")
}

fn request_path(start: &str) -> &str {
    start.split_whitespace().nth(1).unwrap_or("")
}

enum IoFail {
    Timeout,
    Closed,
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
        return Err(IoFail::Bad("cluster body is too large".to_string()));
    }
    let total = split + 4 + length;
    while buf.len() < total {
        if read_some(stream, buf)? == 0 {
            return Err(IoFail::Closed);
        }
    }
    let body = buf[split + 4..total].to_vec();
    buf.drain(..total);
    Ok(Some(parse_exchange(&head, body)?))
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

fn parse_exchange(head: &str, body: Vec<u8>) -> Result<Exchange, IoFail> {
    let start = head.split("\r\n").next().unwrap_or("").to_string();
    if request_method(&start).is_empty() || request_path(&start).is_empty() {
        return Err(IoFail::Bad("cluster request line is empty".to_string()));
    }
    Ok(Exchange { start, body })
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
    Err(IoFail::Bad("cluster message has no content-length".to_string()))
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

fn write_status(stream: &mut impl Write, status: u16) -> std::io::Result<()> {
    let reason = match status {
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let bytes = format!("HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nConnection: keep-alive\r\n\r\n");
    stream.write_all(bytes.as_bytes())?;
    stream.flush()
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

fn fingerprint_verifier(material: &Material, provider: &rustls::crypto::CryptoProvider) -> Arc<FingerprintVerifier> {
    Arc::new(FingerprintVerifier {
        digest: material.digest,
        schemes: provider.signature_verification_algorithms,
        hints: Vec::new(),
    })
}

struct FingerprintVerifier {
    digest: [u8; 32],
    schemes: rustls::crypto::WebPkiSupportedAlgorithms,
    hints: Vec<rustls::DistinguishedName>,
}

impl std::fmt::Debug for FingerprintVerifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("FingerprintVerifier").finish_non_exhaustive()
    }
}

impl FingerprintVerifier {
    fn trusted(&self, end_entity: &CertificateDer<'_>) -> Result<(), rustls::Error> {
        let digest = Sha256::digest(end_entity.as_ref());
        if digest.as_slice() == self.digest {
            Ok(())
        } else {
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::UnknownIssuer,
            ))
        }
    }
}

impl ServerCertVerifier for FingerprintVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        self.trusted(end_entity)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.schemes)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.schemes)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.schemes.supported_schemes()
    }
}

impl ClientCertVerifier for FingerprintVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &self.hints
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        self.trusted(end_entity)?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.schemes)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.schemes)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.schemes.supported_schemes()
    }
}

fn join_handle(name: &str, handle: JoinHandle<()>) {
    if handle.thread().id() == thread::current().id() {
        std::mem::forget(handle);
        return;
    }
    if handle.join().is_err() {
        tracing::error!("{name} thread panicked");
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};

    use super::{listen_port, serve_exchanges, ConnLimits, READ_IDLE};

    const SHORT: ConnLimits = ConnLimits {
        first: Duration::from_millis(300),
        idle: Duration::from_millis(600),
        partial: Duration::from_millis(300),
    };

    /// The server end of a fresh connection, with the listener's read timeout, and the client end.
    fn pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        server
            .set_read_timeout(Some(READ_IDLE.min(Duration::from_millis(50))))
            .unwrap();
        (server, client)
    }

    #[test]
    fn a_connection_that_sends_nothing_gives_its_slot_back() {
        let (server, _client) = pair();
        let started = Instant::now();
        let ended = serve_exchanges(server, SHORT, &AtomicBool::new(false), |_| 204);
        assert!(ended.is_err());
        let elapsed = started.elapsed();
        assert!(
            elapsed >= SHORT.first && elapsed < Duration::from_secs(3),
            "{elapsed:?}"
        );
    }

    #[test]
    fn a_kept_alive_connection_closes_once_idle() {
        let (server, mut client) = pair();
        let serving = std::thread::spawn(move || serve_exchanges(server, SHORT, &AtomicBool::new(false), |_| 204));
        client
            .write_all(b"POST /cluster/x HTTP/1.1\r\nContent-Length: 2\r\n\r\n{}")
            .unwrap();
        let mut answer = [0_u8; 12];
        client.read_exact(&mut answer).unwrap();
        assert_eq!(&answer, b"HTTP/1.1 204");
        let answered = Instant::now();
        assert!(serving.join().unwrap().is_ok());
        let elapsed = answered.elapsed();
        assert!(
            elapsed >= Duration::from_millis(500) && elapsed < Duration::from_secs(3),
            "{elapsed:?}"
        );
    }

    #[test]
    fn empty_cluster_https_port_listens_on_8444_and_zero_is_ephemeral() {
        assert_eq!(listen_port("").unwrap(), 8444);
        assert_eq!(listen_port("0").unwrap(), 0);
        assert!(listen_port("nope").is_err());
    }
}
