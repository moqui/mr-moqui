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

//! The call media node (plans/Comms-Framework.md §6.2, §6.5): the UDP media socket and one driver thread that owns it
//! and the [`moqui_rtc::Sfu`]. Off while `rtc_media_port` is empty, like the other listeners. The socket binds
//! `rtc_media_host` (127.0.0.1 by default) and advertises `rtc_public_address` (the bound address by default; a node
//! behind 1:1 NAT names its public one) as its ICE candidate.
//!
//! The driver takes commands from a bounded channel: the signaling frames of the legs whose media is here (from
//! `notws/rtc.rs` on this node or another, by the `rtc-signal` cluster topic) and the control messages of the call
//! services (`send#MediaControl`). It runs no business logic: who may join was checked by `bind#Connection` on the
//! signaling node, and each frame carries the connection generation the SFU fences with. Replies go back to the socket
//! that sent the frame, on this node or by `rtc-signal` to its node. While the socket is up, the node registers itself
//! every 20 seconds (`register#MediaNode`); a call whose node stops registering ends (`expire#Calls`).
//!
//! On an edge node (`node_role=edge`, plans/Edge-Node.md §5.4, E7) the media node is its local node's: it registers
//! no row of its own, since it has no call state. Its media address and TURN URLs go in its link Hello
//! (`moqui_core::edge::media::set_advert`), and the local node keeps its `CallMediaNode` row from the link. Its frames,
//! replies and controls go over the link (`moqui_core::edge::signal`), its media events and captions' services are
//! delegated to the local node, and its recordings' segments are queued there (`recorder.rs`).
//!
//! An all-in-one node (`node_role=all-in-one`, plans/Edge-Node.md §10, E12) is both: it registers its own row for its
//! own calls and advertises its media node to its paired node. The driver keys a paired node's rooms by node and call
//! (`moqui_core::edge::media::room_key`, from the link's stamp on each message), so the two databases' call ids never
//! meet; the room's work (media events, captions, recordings) goes to the room's owner.
//!
//! Cascading media nodes (W7, `rtc_cascade_enabled`): a leg's media may be on another node than the call's (a leg node,
//! the node of its signaling socket). The leg node's SFU links the room to the call's node (its origin) with a relay
//! link on the two nodes' media sockets, negotiated by `rtc-signal` messages of kind `relay`; the room's notices and
//! relayed frames go between them as `roomFrame` (`driver.rs`). A leg node that stops ends only its legs
//! (`expire#Calls`).

pub(crate) mod captions;
mod driver;
pub(crate) mod media;
pub(crate) mod recorder;
mod tcp;
mod turn;
// the edge wizard's TURN check, which only the edge link calls
#[cfg(feature = "edge")]
pub(crate) mod turn_probe;

use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_lib::object::Value;
use moqui_lib::sync::lock;
use serde_json::Value as Json;

pub(crate) use driver::Route;

/// Commands waiting for the driver; past this a frame is refused with `overloaded`.
const COMMAND_QUEUE: usize = 4_096;
/// How often the node registers itself.
const REGISTER_EVERY: Duration = Duration::from_secs(20);
const REGISTER_SERVICE: &str = "org.moqui.impl.CallServices.register#MediaNode";

/// Something for the driver.
pub(crate) enum Command {
    /// A signaling frame of a leg, from the socket at `route`. `origin` is the call's media node: another node's when
    /// the leg's media is on this one as a leg node (W7).
    Frame {
        route: Route,
        room: String,
        leg: String,
        generation: u64,
        origin: String,
        frame: Json,
    },
    /// A message of the SFU of the node `from` about the room's relay link (W7).
    Relay {
        room: String,
        from: String,
        message: Box<moqui_rtc::RelayMessage>,
    },
    /// The room here relays to `origin`, the call's node (W7): a phone leg's media is on this node.
    Link {
        room: String,
        origin: String,
    },
    /// A frame for the room's legs from another node of the room (W7): a relayed `mute`, a `participant`, a notice
    /// (kept here as `notice` says) or the captions' lines. Not sent back to `from`.
    RoomFrame {
        room: String,
        from: String,
        except: Option<String>,
        frame: Json,
        notice: Option<(NoticeKind, bool)>,
    },
    /// The call services closed a leg or the call.
    CloseLeg {
        room: String,
        leg: String,
    },
    CloseRoom {
        room: String,
    },
    /// A keyframe of every video track published in the room, asked of the browsers: the recorder waits for one to
    /// start a file (`recorder.rs`).
    Keyframes {
        room: String,
    },
    /// A datagram from a browser's ICE-TCP connection.
    TcpIn {
        from: SocketAddr,
        data: Vec<u8>,
    },
    /// A consumer attaching or an injected leg (`media.rs`).
    Media(media::MediaCommand),
    /// A frame for every leg of a room, from the room's own work on this node (captions).
    ToRoom {
        room: String,
        frame: Json,
    },
    /// A notice (an `rtc` frame) for every leg of a room now. With `keep`, the room's notice of that kind: each leg
    /// that joins later gets it right after its `joined`, with the room's other kinds, until a notice of the same kind
    /// without `keep` (the recording stopped, captions off) takes it down.
    RoomNotice {
        room: String,
        kind: NoticeKind,
        frame: Json,
        keep: bool,
    },
    Stop,
}

/// What a room's notice is about. A room keeps one notice of each kind, and a joining leg gets them in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum NoticeKind {
    /// The `recording` frame (`recorder.rs`, W5).
    Recording,
    /// The `captions` frame (`captions.rs`, W6).
    Captions,
}

impl NoticeKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            NoticeKind::Recording => "recording",
            NoticeKind::Captions => "captions",
        }
    }

    pub(crate) fn parse(text: &str) -> Option<Self> {
        match text {
            "recording" => Some(NoticeKind::Recording),
            "captions" => Some(NoticeKind::Captions),
            _ => None,
        }
    }
}

fn media() -> &'static Mutex<Option<SyncSender<Command>>> {
    static MEDIA: OnceLock<Mutex<Option<SyncSender<Command>>>> = OnceLock::new();
    MEDIA.get_or_init(|| Mutex::new(None))
}

/// Hands a command to this node's driver. Err `noMedia` when this node holds no media, `overloaded` when the driver
/// is behind.
pub(crate) fn command(command: Command) -> Result<(), &'static str> {
    let sender = lock(media()).clone().ok_or("noMedia")?;
    match sender.try_send(command) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => Err("overloaded"),
        Err(TrySendError::Disconnected(_)) => Err("noMedia"),
    }
}

/// A command that must not be dropped (a recording's notice to its legs): when the queue is full it waits for room, up to
/// `wait`, instead of answering `overloaded` at once.
pub(crate) fn command_waiting(mut command: Command, wait: Duration) -> Result<(), &'static str> {
    let sender = lock(media()).clone().ok_or("noMedia")?;
    let until = std::time::Instant::now() + wait;
    loop {
        match sender.try_send(command) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Disconnected(_)) => return Err("noMedia"),
            Err(TrySendError::Full(back)) => {
                if std::time::Instant::now() >= until {
                    return Err("overloaded");
                }
                command = back;
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

/// This node's id for calls: its cluster instance id.
pub(crate) fn own_node(factory: &ExecutionContextFactory) -> String {
    moqui_core::cluster::instance_id(factory).unwrap_or_else(|| "local".to_string())
}

/// The media socket's bound address once it started, for `Server::rtc_address`.
#[derive(Default)]
pub struct RtcAddress(pub Mutex<Option<SocketAddr>>);

/// The media socket and its threads, stopped when dropped.
pub(crate) struct RtcNode {
    stop: Arc<AtomicBool>,
    driver: Option<JoinHandle<()>>,
    register: Option<JoinHandle<()>>,
    tcp: Option<JoinHandle<()>>,
    turn: Option<Arc<turn::TurnNode>>,
    /// On an edge node, the factory whose link Hello advertises this media node, until it stops.
    advertised: Option<std::sync::Weak<ExecutionContextFactory>>,
}

impl Drop for RtcNode {
    fn drop(&mut self) {
        if let Some(factory) = self.advertised.take().and_then(|factory| factory.upgrade()) {
            moqui_core::edge::media::set_advert(&factory, None);
        }
        self.stop.store(true, Ordering::Release);
        let _ = command(Command::Stop);
        if let Some(t) = self.register.take() {
            let _ = t.join();
        }
        if let Some(t) = self.driver.take() {
            let _ = t.join();
        }
        if let Some(t) = self.tcp.take() {
            let _ = t.join();
        }
        self.turn.take();
        *lock(media()) = None;
    }
}

/// The media settings: where to bind, what to advertise.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MediaConfig {
    pub bind: SocketAddr,
    pub public: SocketAddr,
    /// The ICE-TCP port (`rtc_media_tcp_port`), on the same addresses.
    pub tcp_port: Option<u16>,
}

/// The media settings from the properties, `None` while `rtc_media_port` is empty.
pub(crate) fn media_config() -> Result<Option<MediaConfig>, String> {
    media_config_from(&moqui_lib::env::get_prop_or_env)
}

fn media_config_from(property: &dyn Fn(&str) -> String) -> Result<Option<MediaConfig>, String> {
    let port = property("rtc_media_port");
    let port = port.trim();
    if port.is_empty() {
        return Ok(None);
    }
    let port: u16 = port
        .parse()
        .map_err(|_| format!("rtc_media_port {port} is not a port"))?;
    let host = property("rtc_media_host");
    let host = if host.trim().is_empty() {
        "127.0.0.1".to_string()
    } else {
        host.trim().to_string()
    };
    let bind = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| format!("rtc_media_host {host}: {e}"))?
        .next()
        .ok_or_else(|| format!("rtc_media_host {host} has no address"))?;
    let public = property("rtc_public_address");
    let public = public.trim();
    let public = if public.is_empty() {
        None
    } else if let Ok(addr) = public.parse::<SocketAddr>() {
        Some(addr)
    } else {
        let ip: std::net::IpAddr = public
            .parse()
            .map_err(|_| format!("rtc_public_address {public} is not an address"))?;
        Some(SocketAddr::new(ip, port))
    };
    let tcp = property("rtc_media_tcp_port");
    let tcp = tcp.trim();
    let tcp_port = if tcp.is_empty() {
        None
    } else {
        Some(
            tcp.parse::<u16>()
                .map_err(|_| format!("rtc_media_tcp_port {tcp} is not a port"))?,
        )
    };
    Ok(Some(MediaConfig {
        bind,
        public: public.unwrap_or(bind),
        tcp_port,
    }))
}

/// Starts the media node when `rtc_media_port` is set. A node that cannot start is logged; the server starts
/// without calls.
pub(crate) fn start(factory: &Arc<ExecutionContextFactory>) -> Option<RtcNode> {
    let config = match media_config() {
        Ok(Some(config)) => config,
        Ok(None) => {
            tracing::info!("Not starting call media, rtc_media_port is not set");
            return None;
        }
        Err(err) => {
            tracing::error!("Call media did not start, the server starts without it: {err}");
            return None;
        }
    };
    if config.public.ip().is_unspecified() {
        tracing::error!(
            "Call media did not start: rtc_media_host {} binds every address; set rtc_public_address to the one browsers reach",
            config.bind
        );
        return None;
    }
    let socket = match UdpSocket::bind(config.bind) {
        Ok(socket) => socket,
        Err(err) => {
            tracing::error!(
                "Call media did not start, the media port {} did not bind: {err}",
                config.bind
            );
            return None;
        }
    };
    let bound = socket.local_addr().unwrap_or(config.bind);
    let public = if config.public.port() == 0 {
        SocketAddr::new(config.public.ip(), bound.port())
    } else {
        config.public
    };
    let (tx, rx) = sync_channel(COMMAND_QUEUE);
    *lock(media()) = Some(tx.clone());
    recorder::sweep(factory.runtime_dir());
    let stop = Arc::new(AtomicBool::new(false));
    let own = own_node(factory);
    // ICE-TCP, when its port is set: a listener that does not bind is logged, and the UDP media goes on
    let streams: tcp::Streams = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let mut tcp_public = None;
    let mut tcp_thread = None;
    if let Some(port) = config.tcp_port {
        match std::net::TcpListener::bind((config.bind.ip(), port)) {
            Ok(listener) => {
                let local = listener.local_addr().map(|a| a.port()).unwrap_or(port);
                match tcp::start(listener, Arc::clone(&streams), tx.clone(), Arc::clone(&stop)) {
                    Ok(handle) => {
                        tcp_public = Some(SocketAddr::new(public.ip(), local));
                        tcp_thread = Some(handle);
                    }
                    Err(err) => tracing::error!("ICE-TCP did not start: {err}"),
                }
            }
            Err(err) => tracing::error!("ICE-TCP did not start, rtc_media_tcp_port {port} did not bind: {err}"),
        }
    }
    // the embedded TURN server, when its ports are set: one that does not start is logged, and calls go on without it
    let turn_node = match turn::settings(&moqui_lib::env::get_prop_or_env, bound, public, &[]) {
        Ok(Some(settings)) => match turn::start(factory.runtime_dir(), &settings) {
            Ok(node) => {
                tracing::info!("TURN started: {}", turn::urls(&settings, &node.bound).join(", "));
                Some((Arc::new(node), settings))
            }
            Err(err) => {
                tracing::error!("TURN did not start, calls go on without it: {err}");
                None
            }
        },
        Ok(None) => None,
        Err(err) => {
            tracing::error!("TURN did not start, calls go on without it: {err}");
            None
        }
    };
    let turn_urls = turn_node
        .as_ref()
        .map(|(node, settings)| turn::urls(settings, &node.bound))
        .unwrap_or_default()
        .join(",");
    let turn_arc = turn_node.map(|(node, _)| node);
    let revoke: Option<driver::Revoke> = turn_arc.clone().map(|node| {
        let revoke: driver::Revoke = Box::new(move |leg: &str| node.revoke(leg));
        revoke
    });
    let driver = {
        let factory = Arc::clone(factory);
        let stop = Arc::clone(&stop);
        let own = own.clone();
        let tcp = tcp_public.map(|addr| (addr, Arc::clone(&streams)));
        std::thread::Builder::new()
            .name("moqui-rtc-media".to_string())
            .spawn(move || driver::run(&factory, socket, public, own, rx, tcp, revoke, &stop))
    };
    let driver = match driver {
        Ok(handle) => handle,
        Err(err) => {
            tracing::error!("Call media did not start, its thread did not start: {err}");
            *lock(media()) = None;
            return None;
        }
    };
    // an edge node registers no row: its local node keeps it from the link, which advertises it (E7). An all-in-one
    // node (E12) does both: its own calls' row, and the advert for its paired node's
    let settings = moqui_core::edge::settings(factory);
    let advertised = settings.takes_links();
    if advertised {
        let advert = moqui_core::edge::media::MediaAdvert {
            media_address: public.to_string(),
            turn_urls: turn_urls
                .split(',')
                .filter(|url| !url.is_empty())
                .map(str::to_string)
                .collect(),
            started_ms: moqui_core::user::now_millis(),
        };
        moqui_core::edge::media::set_advert(factory, Some(advert));
    }
    let register = if settings.edge_node() {
        None
    } else {
        let factory = Arc::clone(factory);
        let stop = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("moqui-rtc-register".to_string())
            .spawn(move || register_loop(&factory, &own, public, &turn_urls, &stop))
            .ok()
    };
    *lock(&factory.extension(|_| RtcAddress::default()).0) = Some(bound);
    tracing::info!("Call media started on {bound}, advertised as {public}");
    Some(RtcNode {
        stop,
        driver: Some(driver),
        register,
        tcp: tcp_thread,
        turn: turn_arc,
        advertised: advertised.then(|| Arc::downgrade(factory)),
    })
}

fn register_loop(
    factory: &Arc<ExecutionContextFactory>,
    own: &str,
    public: SocketAddr,
    turn_urls: &str,
    stop: &AtomicBool,
) {
    let mut next = std::time::Instant::now();
    while !stop.load(Ordering::Acquire) {
        if std::time::Instant::now() >= next {
            register(factory, own, public, turn_urls, false);
            next = std::time::Instant::now() + REGISTER_EVERY;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    register(factory, own, public, turn_urls, true);
}

fn register(factory: &Arc<ExecutionContextFactory>, own: &str, public: SocketAddr, turn_urls: &str, stopping: bool) {
    let parameters = std::collections::BTreeMap::from([
        ("instanceId".to_string(), Value::string(own)),
        ("mediaAddress".to_string(), Value::string(public.to_string())),
        ("turnUrls".to_string(), Value::string(turn_urls)),
        ("stopping".to_string(), Value::Bool(stopping)),
    ]);
    let ran = factory.service_pools().call(move || {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        moqui_core::service::sync(REGISTER_SERVICE, &parameters).map(|_| ())
    });
    if let Err(err) = ran {
        tracing::warn!("call media node not registered: {}", err.message());
    }
}

/// Runs a call service as internal work on a request worker, without waiting for it: the media events the driver
/// reports (a leg's media connected). A paired node's room (E12) runs it as that node's, so the service goes down its
/// link (`moqui_core::edge::owner`); the server's own as on a normal node.
pub(crate) fn note(
    factory: &Arc<ExecutionContextFactory>,
    room: &str,
    service: &'static str,
    parameters: Vec<(&'static str, Value)>,
) {
    let owner = Arc::clone(factory);
    let room_owner = moqui_core::edge::media::room_owner(room);
    let parameters: std::collections::BTreeMap<String, Value> =
        parameters.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    let submitted = factory.request_pool().try_submit(owner, move || {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        moqui_core::edge::owner::with(room_owner, || {
            moqui_core::service::sync(service, &parameters).map(|_| ())
        })
    });
    if let Err(err) = submitted {
        tracing::debug!("call media event not noted: {}", err.message());
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
