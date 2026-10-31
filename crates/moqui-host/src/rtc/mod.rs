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

mod driver;

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
    /// A signaling frame of a leg, from the socket at `route`.
    Frame {
        route: Route,
        room: String,
        leg: String,
        generation: u64,
        frame: Json,
    },
    /// The call services closed a leg or the call.
    CloseLeg {
        room: String,
        leg: String,
    },
    CloseRoom {
        room: String,
    },
    Stop,
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
}

impl Drop for RtcNode {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = command(Command::Stop);
        if let Some(t) = self.register.take() {
            let _ = t.join();
        }
        if let Some(t) = self.driver.take() {
            let _ = t.join();
        }
        *lock(media()) = None;
    }
}

/// The media settings: where to bind, what to advertise.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MediaConfig {
    pub bind: SocketAddr,
    pub public: SocketAddr,
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
    Ok(Some(MediaConfig {
        bind,
        public: public.unwrap_or(bind),
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
    *lock(media()) = Some(tx);
    let stop = Arc::new(AtomicBool::new(false));
    let own = own_node(factory);
    let driver = {
        let factory = Arc::clone(factory);
        let stop = Arc::clone(&stop);
        let own = own.clone();
        std::thread::Builder::new()
            .name("moqui-rtc-media".to_string())
            .spawn(move || driver::run(&factory, socket, public, own, rx, &stop))
    };
    let driver = match driver {
        Ok(handle) => handle,
        Err(err) => {
            tracing::error!("Call media did not start, its thread did not start: {err}");
            *lock(media()) = None;
            return None;
        }
    };
    let register = {
        let factory = Arc::clone(factory);
        let stop = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("moqui-rtc-register".to_string())
            .spawn(move || register_loop(&factory, &own, public, &stop))
            .ok()
    };
    *lock(&factory.extension(|_| RtcAddress::default()).0) = Some(bound);
    tracing::info!("Call media started on {bound}, advertised as {public}");
    Some(RtcNode {
        stop,
        driver: Some(driver),
        register,
    })
}

fn register_loop(factory: &Arc<ExecutionContextFactory>, own: &str, public: SocketAddr, stop: &AtomicBool) {
    let mut next = std::time::Instant::now();
    while !stop.load(Ordering::Acquire) {
        if std::time::Instant::now() >= next {
            register(factory, own, public, false);
            next = std::time::Instant::now() + REGISTER_EVERY;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    register(factory, own, public, true);
}

fn register(factory: &Arc<ExecutionContextFactory>, own: &str, public: SocketAddr, stopping: bool) {
    let parameters = std::collections::BTreeMap::from([
        ("instanceId".to_string(), Value::string(own)),
        ("mediaAddress".to_string(), Value::string(public.to_string())),
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
/// reports (a leg's media connected).
pub(crate) fn note(
    factory: &Arc<ExecutionContextFactory>,
    service: &'static str,
    parameters: Vec<(&'static str, Value)>,
) {
    let owner = Arc::clone(factory);
    let parameters: std::collections::BTreeMap<String, Value> =
        parameters.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    let submitted = factory.request_pool().try_submit(owner, move || {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        moqui_core::service::sync(service, &parameters).map(|_| ())
    });
    if let Err(err) = submitted {
        tracing::debug!("call media event not noted: {}", err.message());
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
