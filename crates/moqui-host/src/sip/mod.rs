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

//! Phone calls over SIP trunks (plans/archive/Comms-Framework.md §5.1, §5.2, §6.5; docs/interfaces/SIP.md): the SIP engine of
//! this node. Off while `sip_port` and `sip_tls_port` are empty, like the other listeners, and only beside this node's
//! call media (`rtc_media_port`), since a phone leg's audio joins the call's SFU room here.
//!
//! - **Listeners.** `sip_port` (UDP and TCP) on `sip_host`, `sip_tls_port` (TLS, the certificate from `runtime/cert`
//!   as TURN's) on `sip_tls_host`; each leg's RTP on a port of `sip_rtp_ports` on `sip_rtp_host`. `sip_public_address`
//!   is the address in Contact headers and SDP when the bound one is not what the trunk reaches.
//! - **The user agent** is [`moqui_sip::ua::Ua`] on a small Tokio runtime of its own: signaling only, no business logic.
//!   What a call may do is the voice services' (`org.moqui.call.VoiceServices`), called on a request worker: the engine
//!   reads its trunks and policy with `get#EngineConfig`, hands an incoming call to `receive#PhoneCall`, reports each
//!   leg with `note#PhoneLeg`, a trunk's registration with `note#TrunkStatus` and a source its failed digests blocked
//!   with `record#AuthFailure`, and registers itself every 20 seconds (`register#SipNode`).
//! - **Control.** The services tell it to dial, answer, hang up, hold, resume, send digits or reload with
//!   `send#SipControl`, on the `rtc-signal` topic with kind `sip`, to this node only.
//! - **Fraud, again.** Before a dial goes out the engine checks the number's class against the deployment's policy
//!   (international and premium off unless enabled), sends an emergency number only through an emergency-capable
//!   trunk, and holds the deployment's concurrency limit, whatever asked for the call (§5.2).
//! - **Media.** Each leg is a [`leg::LegMedia`]: its own thread between its RTP socket and the room (`leg.rs`).
//!
//! - **Notices.** The control `prompt` plays a call's notice to a phone leg (`prompt.rs`): the recording's (W5, D-V5)
//!   and the captions' (W6), which browsers get as `rtc` frames; a tone pattern of each, then the deployment's audio.
//! - **Voicemail** (V2, `voicemail.rs`, `audio.rs`): an inbound call unanswered after its box's rings (this engine's
//!   timer), whose user is busy, that its user declined (the control `voicemail`), or to the access number, is answered
//!   here with no one else in it: the greeting, a beep, the caller's message as Ogg Opus; or the PIN and the new messages.
//! - **IVR, queues, transfer and hold** (V5, `ivr.rs`): a number routed to a menu or a queue is answered here; the menu's
//!   keys, a queue's music and ticks, ringback during a ring step or a blind transfer, hold music, a transfer's timer,
//!   a merge's move to another room (the controls `route`, `ringback`, `connect`, `transferTimer`, `move`), and a desk
//!   phone's REFER.
//! - **On an edge node** (plans/archive/Edge-Node.md §5.4 "Voice (SIP)", E8, `edge.rs`, `fallback.rs`): the voice services it
//!   calls run on the local node (delegated), its trunks come from the replicated copy, it registers no `SipNode` (the
//!   local node keeps its row from the link's Hello), its messages and the greetings it plays cross the link's queue,
//!   and a call whose routing finds the local node away is answered from the number's fallback: its outside number,
//!   then a message, or 480.
//! - **On an all-in-one node** (plans/archive/Edge-Node.md §10, E12, `owners.rs`): both. Its own trunks, numbers and phones
//!   as a normal node's, and its paired node's as an edge node's, each item by its owner: the trunks of both sides,
//!   an inbound call by the number called, a phone by the domain it registers to, a call's services and its fallback
//!   by the call's owner. It registers its own `SipNode` and advertises the paired node's domain over the link.
//! - **Several pairings** (plans/Edge-Node2.md §4.1, E16): every pairing's trunks load, each keyed by its node; an
//!   inbound call goes by the number called to the pairing whose copies have it, and a phone by its pairing's domain;
//!   a number or a domain two pairings claim is refused. A message recorded is queued to its call's pairing.
//! - **An edge cluster** (plans/Edge-Node2.md §4.6, E18): every member runs an engine and loads every trunk, but only
//!   the holder of a lock row registers them (`trunk_lock.rs`). A phone's binding names the member it registered with
//!   (its `SipRegistration.instanceId`, this engine's own id), so a call to the phone goes to that member. The
//!   registrar's nonces and their counts, and the fallback's forward counts, are each member's own.

mod audio;
mod edge;
mod fallback;
mod ivr;
mod leg;
mod owners;
mod phones;
mod prompt;
mod trunk_lock;
mod voicemail;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::edge::owner::Owner;
use moqui_core::notify::{TopicMessage, RTC_SIGNAL};
use moqui_lib::object::Value;
use moqui_lib::sync::lock;
use moqui_sip::dial::{self, DestinationClass, DialPolicy};
use moqui_sip::digest::{Algorithm, Credentials};
use moqui_sip::guard::{GuardPolicy, RequestLimits};
use moqui_sip::sdp::{self, Direction, LocalMedia};
use moqui_sip::srtp::MasterKey;
use moqui_sip::ua::{
    Decision, DialRequest, EndReason, IncomingCall, Transport, TrunkConfig, TrunkState, Ua, UaConfig, UaEvent,
};
use serde_json::Value as Json;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use leg::{LegCommand, LegMedia, LegNote, PortRange, Prompt};

const CONFIG_SERVICE: &str = "org.moqui.call.VoiceServices.get#EngineConfig";
const RECEIVE_SERVICE: &str = "org.moqui.call.VoiceServices.receive#PhoneCall";
const NOTE_LEG_SERVICE: &str = "org.moqui.call.VoiceServices.note#PhoneLeg";
const NOTE_TRUNK_SERVICE: &str = "org.moqui.call.VoiceServices.note#TrunkStatus";
const AUTH_FAILURE_SERVICE: &str = "org.moqui.call.VoiceServices.record#AuthFailure";
const REGISTER_SERVICE: &str = "org.moqui.call.VoiceServices.register#SipNode";
/// How often the node registers itself.
const REGISTER_EVERY: Duration = Duration::from_secs(20);
/// How long a call given up (decided too late) takes no dial.
const ABANDONED_FOR: Duration = Duration::from_secs(300);
/// How often (in seconds) a node that takes links reads its pairings' SIP domains again (E12, E16).
const PAIRED_EVERY: u32 = 5;
/// How long an outbound call rings before it is canceled.
const RING_TIMEOUT: Duration = Duration::from_secs(60);
/// The usual CA bundle, for TLS to trunks when `sip_tls_ca_file` is empty.
const SYSTEM_CA: &str = "/etc/ssl/certs/ca-certificates.crt";

/// The SIP settings from the properties.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SipSettings {
    /// UDP and TCP.
    pub listen: Option<SocketAddr>,
    pub tls: Option<SocketAddr>,
    pub rtp_host: IpAddr,
    pub rtp_ports: String,
    pub public: Option<IpAddr>,
    pub ca_file: Option<PathBuf>,
    /// The flood caps: `sip_request_rate`, `sip_request_burst`, `sip_max_transactions`, `sip_max_connections`,
    /// `sip_max_connections_per_source`.
    pub limits: RequestLimits,
    /// The realm of the digest challenges and the domain phones register to: `sip_domain`, else the public address,
    /// else `moqui`.
    pub domain: String,
    /// The registrar for desk and soft phones (V4), `None` unless `sip_registrar_enabled`.
    pub registrar: Option<moqui_sip::registrar::RegistrarLimits>,
}

/// The flood caps from the properties; one that is not a number is logged and keeps its default.
fn limits_from(property: &dyn Fn(&str) -> String) -> RequestLimits {
    fn read<T: std::str::FromStr>(property: &dyn Fn(&str) -> String, name: &str, default: T) -> T {
        let value = property(name);
        let value = value.trim();
        if value.is_empty() {
            return default;
        }
        value.parse().unwrap_or_else(|_| {
            tracing::error!("SIP: {name} {value} is not a number, its default is used");
            default
        })
    }
    let defaults = RequestLimits::default();
    RequestLimits {
        rate: read(property, "sip_request_rate", defaults.rate).max(0.0),
        burst: read(property, "sip_request_burst", defaults.burst),
        max_transactions: read(property, "sip_max_transactions", defaults.max_transactions),
        max_connections: read(property, "sip_max_connections", defaults.max_connections),
        max_connections_per_source: read(
            property,
            "sip_max_connections_per_source",
            defaults.max_connections_per_source,
        ),
    }
}

fn bind_address(host: &str, port: u16, name: &str) -> Result<SocketAddr, String> {
    (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("{name} {host}: {e}"))?
        .next()
        .ok_or_else(|| format!("{name} {host} has no address"))
}

fn port_of(property: &dyn Fn(&str) -> String, name: &str) -> Result<Option<u16>, String> {
    let text = property(name);
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    text.parse()
        .map(Some)
        .map_err(|_| format!("{name} {text} is not a port"))
}

/// `None` while both `sip_port` and `sip_tls_port` are empty.
pub(crate) fn settings_from(property: &dyn Fn(&str) -> String) -> Result<Option<SipSettings>, String> {
    let port = port_of(property, "sip_port")?;
    let tls_port = port_of(property, "sip_tls_port")?;
    if port.is_none() && tls_port.is_none() {
        return Ok(None);
    }
    let or = |name: &str, fallback: &str| {
        let value = property(name);
        if value.trim().is_empty() {
            fallback.to_string()
        } else {
            value.trim().to_string()
        }
    };
    let host = or("sip_host", "127.0.0.1");
    let listen = port.map(|p| bind_address(&host, p, "sip_host")).transpose()?;
    let tls = tls_port
        .map(|p| bind_address(&or("sip_tls_host", &host), p, "sip_tls_host"))
        .transpose()?;
    let rtp_host = bind_address(&or("sip_rtp_host", &host), 0, "sip_rtp_host")?.ip();
    let public = property("sip_public_address");
    let public = public.trim();
    let public = if public.is_empty() {
        None
    } else {
        Some(
            public
                .parse::<IpAddr>()
                .map_err(|_| format!("sip_public_address {public} is not an address"))?,
        )
    };
    let ca = property("sip_tls_ca_file");
    let ca_file = if ca.trim().is_empty() {
        Some(PathBuf::from(SYSTEM_CA)).filter(|p| p.exists())
    } else {
        Some(PathBuf::from(ca.trim()))
    };
    let domain = or(
        "sip_domain",
        &public.map(|p| p.to_string()).unwrap_or_else(|| "moqui".to_string()),
    );
    Ok(Some(SipSettings {
        listen,
        tls,
        rtp_host,
        rtp_ports: or("sip_rtp_ports", "20000-20999"),
        public,
        ca_file,
        limits: limits_from(property),
        domain,
        registrar: phones::registrar_from(property),
    }))
}

/// Makes aws-lc-rs the process's default rustls provider, first thing in the process: rsipstack's TLS turns on rustls's
/// ring provider beside aws-lc-rs, and with two providers built in, a library that builds its rustls config from the
/// process default (the MySQL driver, hyper-rustls) would panic without one. aws-lc-rs is what the host uses
/// everywhere, so nothing changes for the code that names its provider.
pub(crate) fn install_crypto() {
    moqui_sip::install_crypto();
}

/// The SIP listeners' bound addresses once started, for `Server::sip_address`.
#[derive(Default)]
pub struct SipAddress(pub Mutex<Option<SocketAddr>>);

/// What the engine task is given.
enum Command {
    /// A `send#SipControl` message.
    Control(Json),
    Note(LegNote),
    Prompt {
        leg: String,
        prompt: Prompt,
    },
    /// A re-INVITE this side sent was answered.
    Reanswered {
        leg: String,
        answer: Option<String>,
    },
    /// Voicemail (V2): the services' answer to `begin#Voicemail`.
    VmBegun {
        leg: String,
        reason: String,
        result: Result<Json, String>,
    },
    /// Voicemail: the services' answer to a PIN.
    VmPin {
        leg: String,
        result: Result<Json, String>,
    },
    /// Voicemail: a prompt's audio is ready.
    VmPlay {
        leg: String,
        tag: u32,
        prompt: Prompt,
    },
    /// Voicemail: a wait for a key ended.
    VmIdle {
        leg: String,
        wait: u32,
    },
    /// An inbound call rang its box's rings, or its handling plan's step (V3); `generation` is the timer's, so a timer
    /// a later step replaced does nothing.
    RingTimeout {
        leg: String,
        generation: u64,
    },
    /// V5 (`ivr.rs`): a menu's prompt is ready.
    IvrPlay {
        leg: String,
        tag: u32,
        prompt: Prompt,
    },
    /// V5: a menu's wait for a key ended.
    IvrIdle {
        leg: String,
        wait: u32,
    },
    /// V5: the services' answer of where a call goes (route#IvrCall, tick#QueueCall, expire#CallTransfer).
    Routed {
        leg: String,
        result: Result<Json, String>,
    },
    /// V5: a queue call's next tick.
    QueueTick {
        leg: String,
        generation: u64,
    },
    /// V5: a loop's audio (ringback, music) is ready.
    LoopReady {
        leg: String,
        token: u64,
        audio: Prompt,
    },
    /// The trunks did not load: try again (the `attempt`th time).
    ReloadRetry {
        attempt: u32,
    },
    /// E8: a fallback's forward rang its time unanswered.
    FallbackTimeout {
        leg: String,
    },
    /// E12, E16: each active pairing and its SIP device domain, read again.
    Paired(Vec<(String, String)>),
    /// E16: a pairing's setup changed (its Configure): read the pairings again now.
    ReadPairings,
}

fn engine() -> &'static Mutex<Option<UnboundedSender<Command>>> {
    static ENGINE: OnceLock<Mutex<Option<UnboundedSender<Command>>>> = OnceLock::new();
    ENGINE.get_or_init(|| Mutex::new(None))
}

/// The running engine, stopped when dropped (before the factory: its last act deregisters the node).
pub(crate) struct SipNode {
    runtime: Option<tokio::runtime::Runtime>,
    ua: Ua,
    stop: Arc<AtomicBool>,
    register: Option<JoinHandle<()>>,
    _signal: Option<moqui_core::notify::Subscription>,
    /// On a node that takes links (an edge node, E8, or an all-in-one node, E12), the factory whose Hello tells its
    /// addresses, cleared when the engine stops.
    advertised: Option<std::sync::Weak<ExecutionContextFactory>>,
    /// On a node that takes links, the id this member claims the trunk registration lock as, let go when it stops.
    trunk_member: Option<String>,
}

impl Drop for SipNode {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(factory) = self.advertised.take().and_then(|factory| factory.upgrade()) {
            if let Some(member) = self.trunk_member.take() {
                trunk_lock::release(&factory, &member);
            }
            moqui_core::edge::media::set_sip_advert(&factory, None);
            moqui_core::edge::media::set_sip_trunks(&factory, Vec::new());
            moqui_core::edge::media::set_sip_realms(&factory, None);
        }
        *lock(engine()) = None;
        self.ua.shutdown();
        if let Some(t) = self.register.take() {
            let _ = t.join();
        }
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(Duration::from_secs(2));
        }
    }
}

/// The TLS listener's certificate and key, PEM, from `runtime/cert`: the public address's pair, else `default`.
fn tls_pair(runtime_dir: &Path, name: Option<&str>) -> Option<(Vec<u8>, Vec<u8>)> {
    let dir = runtime_dir.join("cert");
    let names = name.into_iter().map(str::to_string).chain(["default".to_string()]);
    for name in names {
        if let Some((cert, key)) = crate::tls::find_files(&dir, &name) {
            if let (Ok(cert), Ok(key)) = (std::fs::read(&cert), std::fs::read(&key)) {
                return Some((cert, key));
            }
        }
    }
    None
}

/// The startup report's name for the SIP listeners.
pub(crate) const LISTENER: &str = "SIP";

/// The ports SIP will bind, for the port check before the framework is built: `sip_port` over UDP and TCP, and
/// `sip_tls_port`. None while both are empty or a setting does not parse ([`start`] reports that).
pub(crate) fn listen_targets() -> Vec<crate::port_check::Target> {
    use crate::port_check::{Proto, Target};
    let Ok(Some(settings)) = settings_from(&moqui_lib::env::get_prop_or_env) else {
        return Vec::new();
    };
    let mut targets = Vec::new();
    if let Some(listen) = settings.listen {
        targets.push(Target::optional(
            LISTENER,
            "sip_port",
            listen.ip(),
            listen.port(),
            Proto::Udp,
        ));
        targets.push(Target::optional(
            LISTENER,
            "sip_port",
            listen.ip(),
            listen.port(),
            Proto::Tcp,
        ));
    }
    if let Some(tls) = settings.tls {
        targets.push(Target::optional(
            LISTENER,
            "sip_tls_port",
            tls.ip(),
            tls.port(),
            Proto::Tcp,
        ));
    }
    targets
}

/// Starts the SIP engine when `sip_port` or `sip_tls_port` is set, and records it in the startup report. One that
/// cannot start is logged; the server starts without phone calls.
pub(crate) fn start(factory: &Arc<ExecutionContextFactory>) -> Option<SipNode> {
    let node = start_node(factory);
    // a start that recorded nothing returned early on an error it logged
    moqui_core::startup::report(factory).record_default(
        moqui_core::startup::Section::Listener,
        LISTENER,
        moqui_core::startup::State::failed("see the SIP error above"),
    );
    node
}

fn start_node(factory: &Arc<ExecutionContextFactory>) -> Option<SipNode> {
    use moqui_core::startup::{listener, State};
    let settings = match settings_from(&moqui_lib::env::get_prop_or_env) {
        Ok(Some(settings)) => settings,
        Ok(None) => {
            listener(factory, LISTENER, State::off("sip_port and sip_tls_port are not set"));
            return None;
        }
        Err(err) => {
            tracing::error!("SIP did not start, the server starts without phone calls: {err}");
            return None;
        }
    };
    if let Some(busy) = crate::port_check::busy(LISTENER) {
        listener(factory, LISTENER, State::Skipped(busy));
        return None;
    }
    if lock(&factory.extension(|_| crate::rtc::RtcAddress::default()).0).is_none() {
        tracing::error!("SIP did not start: phone calls need this server's call media (rtc_media_port)");
        return None;
    }
    let bind_any = |a: Option<SocketAddr>| a.is_some_and(|a| a.ip().is_unspecified());
    if settings.public.is_none()
        && (bind_any(settings.listen) || bind_any(settings.tls) || settings.rtp_host.is_unspecified())
    {
        tracing::error!("SIP did not start: it binds every address; set sip_public_address to the one trunks reach");
        return None;
    }
    let ports = match PortRange::parse(&settings.rtp_ports, settings.rtp_host) {
        Ok(ports) => Arc::new(ports),
        Err(err) => {
            tracing::error!("SIP did not start: {err}");
            return None;
        }
    };
    let tls = settings.tls.and_then(|addr| {
        let name = settings.public.map(|p| p.to_string());
        match tls_pair(factory.runtime_dir(), name.as_deref()) {
            Some((cert, key)) => Some((addr, cert, key)),
            None => {
                tracing::error!(
                    "SIP over TLS did not start: no certificate in runtime/cert (default or the public address)"
                );
                None
            }
        }
    });
    let ca_pem = settings.ca_file.as_ref().and_then(|path| match std::fs::read(path) {
        Ok(pem) => Some(pem),
        Err(err) => {
            tracing::warn!(
                "SIP: the CA file {} did not read, TLS to trunks cannot verify them: {err}",
                path.display()
            );
            None
        }
    });
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("moqui-sip")
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            tracing::error!("SIP did not start, its runtime did not start: {err}");
            return None;
        }
    };
    let (events_tx, events) = unbounded_channel();
    // E8, E12: a node that takes links has a paired node's trunks, calls and phones; a dedicated edge node has nothing
    // of its own
    let edge_settings = moqui_core::edge::settings(factory);
    let (links, dedicated) = (edge_settings.takes_links(), edge_settings.edge_node());
    let paired: phones::Paired = Arc::new(Mutex::new(None));
    let config = UaConfig {
        udp: settings.listen,
        tcp: settings.listen,
        tls,
        public_host: settings.public.map(|p| p.to_string()),
        ca_pem,
        realm: settings.domain.clone(),
        guard: GuardPolicy::default(),
        limits: settings.limits,
        user_agent: "Moqui".to_string(),
        registrar: settings
            .registrar
            .map(|limits| phones::registrar_config(factory, limits, &settings.domain, dedicated, Arc::clone(&paired))),
        trunk_ping: None,
        tls_handshake: None,
        resolver: None,
    };
    let ua = match runtime.block_on(Ua::start(config, events_tx)) {
        Ok(ua) => ua,
        Err(err) => {
            tracing::error!("SIP did not start, the server starts without phone calls: {err}");
            return None;
        }
    };
    let bound = ua.bound();
    let (commands_tx, commands) = unbounded_channel();
    *lock(engine()) = Some(commands_tx.clone());
    let signal = {
        let factory = Arc::downgrade(factory);
        let commands = commands_tx.clone();
        let deliver = move |message: &TopicMessage| {
            let Some(factory) = factory.upgrade() else { return };
            if message.body.get("kind").and_then(Json::as_str) != Some("sip")
                || !moqui_core::cluster::for_this_member(&factory, message)
            {
                return;
            }
            // E12: a paired node's control (over its link) names its ids, which the engine keys by that node
            let mut body = message.body.clone();
            if let Some(node) = moqui_core::edge::signal::link_node(&message.body) {
                owners::key_ids(node, &mut body);
            }
            let _ = commands.send(Command::Control(body));
        };
        match moqui_core::notify::subscribe(RTC_SIGNAL, Arc::new(deliver)) {
            Ok(subscription) => Some(subscription),
            Err(err) => {
                tracing::error!(
                    "SIP: the rtc-signal topic was not subscribed, calls cannot be placed: {}",
                    err.message()
                );
                None
            }
        }
    };
    let own = crate::rtc::own_node(factory);
    let stop = Arc::new(AtomicBool::new(false));
    let sip_address = bound.udp.or(bound.tcp).or(bound.tls);
    let addresses = [
        (
            "sipAddress",
            bound.udp.or(bound.tcp).map(|a| public_of(a, settings.public)),
        ),
        ("tlsAddress", bound.tls.map(|a| public_of(a, settings.public))),
    ];
    let addresses: Vec<(&'static str, String)> = addresses
        .into_iter()
        .filter_map(|(k, v)| v.map(|v| (k, v.to_string())))
        .collect();
    // a node that takes links tells its addresses in its Hello, and the local node keeps the row (E8). A dedicated edge
    // node's registrar takes a pairing's phones in its own realm; an all-in-one node's own realm is its own. Each
    // pairing's Hello and offer name the domain its Configure named instead (E12, E16,
    // `moqui_core::edge::sip::pairing_realm`)
    let advert = links.then(|| {
        let address = |key: &str| {
            addresses
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        moqui_core::edge::media::SipAdvert {
            sip_address: address("sipAddress"),
            tls_address: address("tlsAddress"),
            domain: if dedicated {
                settings.domain.clone()
            } else {
                String::new()
            },
            registrar: settings.registrar.is_some(),
        }
    });
    if let Some(advert) = &advert {
        moqui_core::edge::media::set_sip_advert(factory, Some(advert.clone()));
    }
    let engine = Engine {
        factory: Arc::clone(factory),
        ua: ua.clone(),
        own: own.clone(),
        links,
        dedicated,
        paired,
        registrar: settings.registrar.is_some(),
        ports,
        public: settings.public,
        legs: HashMap::new(),
        trunks: HashMap::new(),
        policy: DialPolicy::default(),
        max_concurrent: 0,
        commands: commands_tx,
        voicemail: HashMap::new(),
        voicemail_starting: HashSet::new(),
        ring_timers: RingTimers::default(),
        ivr: HashMap::new(),
        queued: HashMap::new(),
        next_tick: 0,
        loops: HashMap::new(),
        next_loop: 0,
        fallbacks: HashMap::new(),
        forwards: HashMap::new(),
        abandoned: HashMap::new(),
        forward_counts: HashMap::new(),
        trunk_lock: Arc::new(AtomicBool::new(false)),
    };
    runtime.spawn(engine.run(events, commands));
    // a dedicated edge node registers no SipNode (E8); an all-in-one node registers its own (E12)
    let register = if dedicated {
        None
    } else {
        let factory = Arc::clone(factory);
        let stop = Arc::clone(&stop);
        let rtp_ports = settings.rtp_ports.clone();
        std::thread::Builder::new()
            .name("moqui-sip-register".to_string())
            .spawn(move || register_loop(&factory, &own, &addresses, &rtp_ports, &stop))
            .ok()
    };
    *lock(&factory.extension(|_| SipAddress::default()).0) = sip_address;
    listener(
        factory,
        LISTENER,
        State::On(
            [("udp/tcp", bound.udp), ("tls", bound.tls)]
                .iter()
                .filter_map(|(k, a)| a.map(|a| format!("{k} {a}")))
                .collect::<Vec<_>>()
                .join(", "),
        ),
    );
    Some(SipNode {
        runtime: Some(runtime),
        ua,
        stop,
        register,
        _signal: signal,
        advertised: links.then(|| Arc::downgrade(factory)),
        trunk_member: links.then(|| trunk_lock::member_id(factory)),
    })
}

fn public_of(bound: SocketAddr, public: Option<IpAddr>) -> SocketAddr {
    SocketAddr::new(public.unwrap_or(bound.ip()), bound.port())
}

fn register_loop(
    factory: &Arc<ExecutionContextFactory>,
    own: &str,
    addresses: &[(&'static str, String)],
    rtp_ports: &str,
    stop: &AtomicBool,
) {
    let mut next = Instant::now();
    while !stop.load(Ordering::Acquire) {
        if Instant::now() >= next {
            register(factory, own, addresses, rtp_ports, false);
            next = Instant::now() + REGISTER_EVERY;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    register(factory, own, addresses, rtp_ports, true);
}

fn register(
    factory: &Arc<ExecutionContextFactory>,
    own: &str,
    addresses: &[(&'static str, String)],
    rtp_ports: &str,
    stopping: bool,
) {
    let mut parameters = BTreeMap::from([
        ("instanceId".to_string(), Value::string(own)),
        ("rtpPorts".to_string(), Value::string(rtp_ports)),
        ("stopping".to_string(), Value::Bool(stopping)),
    ]);
    for (key, value) in addresses {
        parameters.insert((*key).to_string(), Value::string(value));
    }
    let ran = factory.service_pools().call(move || {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        moqui_core::service::sync(REGISTER_SERVICE, &parameters).map(|_| ())
    });
    if let Err(err) = ran {
        tracing::warn!("SIP node not registered: {}", err.message());
    }
}

/// Runs a voice service on a request worker, as internal work, for `owner`'s item; its answer as JSON, or its errors.
/// A paired node's (E12, `owners.rs`) runs as that node's, so a delegated service goes down its link, with the ids
/// bare, and its answer's ids keyed again.
async fn call_service(
    factory: &Arc<ExecutionContextFactory>,
    owner: Owner,
    name: &'static str,
    mut parameters: Json,
) -> Result<Json, String> {
    let node = owner.node().map(str::to_string);
    if node.is_some() {
        owners::bare_ids(&mut parameters);
    }
    let factory = Arc::clone(factory);
    let answered = tokio::task::spawn_blocking(move || {
        let parameters: BTreeMap<String, Value> = match moqui_lib::json::from_json(&parameters) {
            Value::Map(map) => map.into_iter().collect(),
            _ => BTreeMap::new(),
        };
        let job = move || {
            let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
            let called = moqui_core::edge::owner::with(owner, || moqui_core::service::sync(name, &parameters));
            let errors = ExecutionContext::with_current(|ec| {
                let errors = ec.message().errors().join("; ");
                ec.message().clear_errors();
                errors
            })
            .unwrap_or_default();
            Ok(match called {
                Ok(out) => {
                    let map: serde_json::Map<String, Json> = out
                        .iter()
                        .map(|(k, v)| (k.clone(), moqui_lib::json::to_json(v)))
                        .collect();
                    (Json::Object(map), errors)
                }
                Err(err) => (Json::Null, err.message().to_string()),
            })
        };
        let rx = factory
            .request_pool()
            .try_submit(Arc::clone(&factory), job)
            .map_err(|e| e.message().to_string())?;
        match rx.recv() {
            Ok(Ok((out, errors))) if errors.is_empty() => Ok(out),
            Ok(Ok((out, errors))) => {
                let code = out.get("reasonCode").and_then(Json::as_str).unwrap_or("refused");
                Err(format!("{code}: {errors}"))
            }
            Ok(Err(err)) => Err(err.message().to_string()),
            Err(_) => Err("request worker stopped".to_string()),
        }
    })
    .await
    .map_err(|_| "request worker panicked".to_string())?;
    answered.map(|mut out| {
        if let Some(node) = &node {
            owners::key_ids(node, &mut out);
        }
        out
    })
}

/// The recorded audio of a notice, 48 kHz mono, when its property names a WAV file; one that does not read is logged and
/// the notice plays its tones only.
async fn notice_audio(factory: &Arc<ExecutionContextFactory>, notice: prompt::Notice) -> Option<Vec<i16>> {
    let property = notice.audio_property();
    let location = moqui_lib::env::get_prop_or_env(property).trim().to_string();
    if location.is_empty() {
        return None;
    }
    let factory = Arc::clone(factory);
    let read = tokio::task::spawn_blocking(move || {
        let job = {
            let (factory, location) = (Arc::clone(&factory), location.clone());
            move || factory.resource().location_bytes(&location)
        };
        let rx = factory
            .request_pool()
            .try_submit(Arc::clone(&factory), job)
            .map_err(|e| e.message().to_string())?;
        match rx.recv() {
            Ok(Ok(Some(bytes))) => prompt::wav_pcm48(&bytes).map_err(|e| format!("{location}: {e}")),
            Ok(Ok(None)) => Err(format!("{location}: no file there")),
            Ok(Err(err)) => Err(format!("{location}: {}", err.message())),
            Err(_) => Err("request worker stopped".to_string()),
        }
    })
    .await;
    match read {
        Ok(Ok(audio)) => Some(audio),
        Ok(Err(err)) => {
            tracing::error!("SIP: the prompt audio of {property} did not load, the notice plays its tones: {err}");
            None
        }
        Err(_) => None,
    }
}

/// A trunk as the engine knows it, besides what the user agent has.
#[derive(Debug, Clone)]
struct TrunkInfo {
    emergency: bool,
    srtp: bool,
}

/// A leg here: its room, its media and its SDP; an inbound one's caller, and whether its user answered.
struct Leg {
    room: String,
    media: LegMedia,
    local: LocalMedia,
    caller: String,
    /// An inbound leg's number called (E.164 when it is one): the number a fallback or a queued message is for (E8).
    dialed: String,
    answered: bool,
    /// A desk or soft phone's leg (V4): its device (`sipDeviceId`), whose legs `dropDevice` hangs up.
    device: Option<String>,
    /// A device leg's SDP with SRTP keys, for a binding over TLS, until a binding answers.
    srtp_local: Option<LocalMedia>,
    /// V5: the engine answered the trunk's INVITE (for the IVR, a queue, voicemail or a person), so it is not answered again.
    sip_answered: bool,
    /// V5: on hold (hold music, its audio kept from the others); an answer elsewhere does not take it off hold.
    held: bool,
}

struct Engine {
    factory: Arc<ExecutionContextFactory>,
    ua: Ua,
    own: String,
    /// A node that takes links (a dedicated edge node, E8, or an all-in-one node, E12): the paired node's trunks from
    /// its copy, its calls' services over its link, and its numbers' fallback while it is away (`owners.rs`).
    links: bool,
    /// A dedicated edge node: no trunks, numbers or phones of its own; all are the paired node's.
    dedicated: bool,
    /// Each pairing and its SIP device domain, shared with the registrar's directory (E12, E16).
    paired: phones::Paired,
    /// The registrar is on (`sip_registrar_enabled`).
    registrar: bool,
    ports: Arc<PortRange>,
    public: Option<IpAddr>,
    legs: HashMap<String, Leg>,
    trunks: HashMap<String, TrunkInfo>,
    policy: DialPolicy,
    max_concurrent: usize,
    commands: UnboundedSender<Command>,
    /// The legs in voicemail (V2), and those whose `begin#Voicemail` is on its way.
    voicemail: HashMap<String, voicemail::VmCall>,
    voicemail_starting: HashSet<String>,
    /// Each inbound leg's current ring timer (V3: a handling plan's steps replace it).
    ring_timers: RingTimers,
    /// V5 (`ivr.rs`): the legs in a menu, the legs waiting in a queue (with their tick's generation), and each leg's
    /// current loop (ringback, music), whose audio is read off the engine task.
    ivr: HashMap<String, ivr::IvrCall>,
    queued: HashMap<String, ivr::QueueCall>,
    next_tick: u64,
    loops: HashMap<String, u64>,
    next_loop: u64,
    /// E8 (`fallback.rs`): the inbound legs answered from their number's fallback while the local node is away, and
    /// each forward's outbound leg with its inbound one.
    fallbacks: HashMap<String, fallback::FallbackCall>,
    forwards: HashMap<String, String>,
    /// E8: the rooms of calls whose phone the user agent refused because the services decided too late, each with
    /// when: a dial into one is refused, and what was dialed into it is hung up, so no number is rung for a caller who
    /// is gone.
    abandoned: HashMap<String, Instant>,
    /// On a node that takes links: this member holds the trunk registration lock, so its trunks register
    /// (`trunk_lock.rs`, E18).
    trunk_lock: Arc<AtomicBool>,
    /// E8: the forwards the fallback made, by number: those going on, and when each of the last day began.
    forward_counts: HashMap<String, fallback::ForwardCount>,
}

/// The ring timers of the inbound legs (V3): a leg has at most one that counts, the one armed last, so a timer a later step
/// of the call's handling plan replaced does nothing when it ends. Each carries its step's token (`CallRoute.stepSeq`),
/// which `begin#Voicemail` checks, so a timer or control of a step the plan already left cannot skip the step it is on.
#[derive(Default)]
pub(crate) struct RingTimers(HashMap<String, (u64, Option<String>)>);

impl RingTimers {
    /// A new timer for the leg and its step's token, replacing its last: the generation its end reports.
    pub(crate) fn arm(&mut self, leg: &str, step: Option<String>) -> u64 {
        let generation = self.0.get(leg).map_or(1, |(g, _)| g + 1);
        self.0.insert(leg.to_string(), (generation, step));
        generation
    }

    /// A timer ended: when it is the leg's current one, its step's token (and it is forgotten); else None.
    pub(crate) fn fire(&mut self, leg: &str, generation: u64) -> Option<Option<String>> {
        if self.0.get(leg).map(|(g, _)| *g) != Some(generation) {
            return None;
        }
        self.0.remove(leg).map(|(_, step)| step)
    }

    /// The leg's timer no longer counts (the leg ended, or its step has no timer).
    pub(crate) fn clear(&mut self, leg: &str) {
        self.0.remove(leg);
    }
}

/// A step token from a services' answer or a control (`stepToken`), when it has one.
/// A pairing's setup changed on this node (its Configure, E16): the SIP engine reads the pairings' domains again now,
/// so its registrar takes a new domain's phones at once. Nothing while no engine runs.
pub(crate) fn pairings_changed() {
    if let Some(engine) = lock(engine()).as_ref() {
        let _ = engine.send(Command::ReadPairings);
    }
}

pub(crate) fn step_token(body: &Json) -> Option<String> {
    match body.get("stepToken") {
        Some(Json::String(token)) if !token.is_empty() => Some(token.clone()),
        Some(Json::Number(token)) => Some(token.to_string()),
        _ => None,
    }
}

/// What `note#PhoneLeg` hears of a leg that ended, and its SIP status: busy (486) is a phone on another call, whose
/// user's other devices ring on; declined (603, or 600) is the person's refusal, on every device (V3).
pub(crate) fn ended_event(reason: EndReason) -> (&'static str, Option<u16>) {
    match reason {
        EndReason::Bye | EndReason::Hangup => ("ended", None),
        EndReason::Canceled | EndReason::TimedOut => ("canceled", None),
        EndReason::Busy => ("busy", Some(486)),
        EndReason::Declined => ("declined", Some(603)),
        EndReason::Failed(code) => ("failed", code),
    }
}

/// The SIP status an incoming call is refused with, for `receive#PhoneCall`'s reasonCode.
pub(crate) fn refusal_status(reason_code: &str) -> u16 {
    match reason_code {
        "notFound" => 404,
        "busy" => 486,
        // V3: the user's call handling rules refused it: an anonymous caller (RFC 5079), or a caller list
        "anonymous" => 433,
        "declined" => 603,
        _ => 480,
    }
}

fn text(body: &Json, key: &str) -> String {
    match body.get(key) {
        Some(Json::String(s)) => s.clone(),
        Some(Json::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn texts(body: &Json, key: &str) -> Vec<String> {
    match body.get(key) {
        Some(Json::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .filter(|s| !s.trim().is_empty())
            .collect(),
        Some(Json::String(s)) => s
            .split(',')
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn flag(body: &Json, key: &str) -> bool {
    match body.get(key) {
        Some(Json::Bool(b)) => *b,
        Some(Json::String(s)) => s == "Y" || s == "true",
        _ => false,
    }
}

fn number(body: &Json, key: &str) -> Option<u64> {
    match body.get(key) {
        Some(Json::Number(n)) => n.as_u64(),
        Some(Json::String(s)) => s.trim().parse().ok(),
        _ => None,
    }
}

impl Engine {
    async fn run(mut self, mut events: UnboundedReceiver<UaEvent>, mut commands: UnboundedReceiver<Command>) {
        if self.links {
            let paired = owners::pairing_domains(&self.factory).await.unwrap_or_default();
            self.paired_changed(paired);
            // the trunks register only on the member that holds the lock (E18): asked before the first load
            let member = trunk_lock::member_id(&self.factory);
            let holds = trunk_lock::claim(&self.factory, &member).await.unwrap_or(false);
            self.trunk_lock.store(holds, Ordering::Release);
            tokio::spawn(trunk_lock::keep(
                Arc::downgrade(&self.factory),
                member,
                Arc::clone(&self.trunk_lock),
                self.commands.clone(),
            ));
        }
        if !self.reload().await {
            self.retry_reload(1);
        }
        if self.links {
            self.watch_trunks();
            // E8: messages that did not queue before the process stopped
            edge::requeue_spool(&self.factory);
        }
        loop {
            tokio::select! {
                event = events.recv() => match event {
                    Some(event) => self.event(event).await,
                    None => break,
                },
                command = commands.recv() => match command {
                    Some(command) => self.command(command).await,
                    None => break,
                },
            }
        }
    }

    /// Fire and forget: a note for the services, as the owner of what it names (E12). A call the fallback answered (E8)
    /// is none of theirs.
    fn note(&self, name: &'static str, parameters: Json) {
        if parameters
            .get("callSessionId")
            .and_then(Json::as_str)
            .is_some_and(edge::is_fallback)
        {
            return;
        }
        let owner = owners::owner_in(&parameters);
        let factory = Arc::clone(&self.factory);
        tokio::spawn(async move {
            if let Err(err) = call_service(&factory, owner, name, parameters).await {
                tracing::warn!("SIP: {name} failed: {err}");
            }
        });
    }

    fn note_leg(&self, room: &str, leg: &str, event: &str, code: Option<u16>, reason: &str) {
        self.note(
            NOTE_LEG_SERVICE,
            serde_json::json!({"callSessionId": room, "callLegId": leg, "event": event, "sipCode": code,
                "reasonText": reason}),
        );
    }

    /// The trunks did not load at start: tried again after a wait that doubles from 2 s to a minute, until they do.
    fn retry_reload(&self, attempt: u32) {
        let wait = Duration::from_secs((1_u64 << attempt.min(6)).min(60));
        let commands = self.commands.clone();
        tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            let _ = commands.send(Command::ReloadRetry { attempt });
        });
    }

    /// On a node that takes links (E8): load the trunks again when a batch of their replicated set is applied, which is
    /// how a trunk or the policy changed on the local node reaches this engine, with the link up or after it comes
    /// back. Every few seconds, the pairings and their SIP device domains are read again too (E12, E16): a pairing
    /// that comes or goes loads the trunks again.
    fn watch_trunks(&self) {
        let (factory, commands) = (Arc::downgrade(&self.factory), self.commands.clone());
        let mut paired = lock(&self.paired).clone().unwrap_or_default();
        tokio::spawn(async move {
            // the count is of this process: the start's load read the copies before any batch came
            let mut seen = 0;
            let mut turn = 0_u32;
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let Some(factory) = factory.upgrade() else { return };
                turn = turn.wrapping_add(1);
                if turn.is_multiple_of(PAIRED_EVERY) {
                    if let Some(now) = owners::pairing_domains(&factory).await.filter(|now| *now != paired) {
                        let nodes_changed = now.iter().map(|(node, _)| node).ne(paired.iter().map(|(node, _)| node));
                        paired.clone_from(&now);
                        if commands.send(Command::Paired(now)).is_err() {
                            return;
                        }
                        if nodes_changed
                            && commands
                                .send(Command::Control(serde_json::json!({"action": "reload"})))
                                .is_err()
                        {
                            return;
                        }
                    }
                }
                let Some(count) = edge::trunks_applied(&factory).await else {
                    continue;
                };
                if count != seen
                    && commands
                        .send(Command::Control(serde_json::json!({"action": "reload"})))
                        .is_err()
                {
                    return;
                }
                seen = count;
            }
        });
    }

    /// Loads the trunks and the policy: the server's own (`get#EngineConfig`) but on a dedicated edge node, and each
    /// pairing's from its replicated copy on a node that takes links (E8, E16), keyed by that node (E12,
    /// `owners::merge_config`); false when they did not load. On an all-in-one node the pairings' copies that do not
    /// read are logged, and its own trunks load.
    async fn reload(&mut self) -> bool {
        let own = if self.dedicated {
            None
        } else {
            match call_service(&self.factory, Owner::Own, CONFIG_SERVICE, serde_json::json!({})).await {
                Ok(config) => Some(config),
                Err(err) => {
                    tracing::error!("SIP: the trunks did not load, calls wait for a reload: {err}");
                    return false;
                }
            }
        };
        let paired = if self.links {
            match edge::engine_config(&self.factory).await {
                Ok(paired) => paired,
                Err(err) if self.dedicated => {
                    tracing::error!("SIP: the trunks did not load, calls wait for a reload: {err}");
                    return false;
                }
                Err(err) => {
                    tracing::error!("SIP: the paired nodes' trunks did not load, their calls wait for a reload: {err}");
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        let config = owners::merge_config(own, &paired);
        let policy = config.get("policy").cloned().unwrap_or(Json::Null);
        let defaults = DialPolicy::default();
        self.policy = DialPolicy {
            home_country: Some(text(&policy, "homeCountryCode"))
                .filter(|c| !c.is_empty())
                .unwrap_or(defaults.home_country),
            emergency_numbers: Some(texts(&policy, "emergencyNumbers"))
                .filter(|n| !n.is_empty())
                .unwrap_or(defaults.emergency_numbers),
            premium_prefixes: Some(texts(&policy, "premiumPrefixes"))
                .filter(|n| !n.is_empty())
                .unwrap_or(defaults.premium_prefixes),
            international_enabled: flag(&policy, "internationalEnabled"),
            premium_enabled: flag(&policy, "premiumEnabled"),
        };
        self.max_concurrent = number(&policy, "maxConcurrentCalls").unwrap_or(0) as usize;
        let guard = GuardPolicy::default();
        self.ua.set_guard_policy(GuardPolicy {
            limit: number(&policy, "authFailureLimit")
                .map(|n| n as u32)
                .unwrap_or(guard.limit),
            window: number(&policy, "authFailureWindowSeconds")
                .map(Duration::from_secs)
                .unwrap_or(guard.window),
            block: number(&policy, "authBlockSeconds")
                .map(Duration::from_secs)
                .unwrap_or(guard.block),
        });
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if let Some(Json::Array(blocks)) = config.get("blockedSourceList") {
            for block in blocks {
                let (Ok(ip), Some(until)) = (
                    text(block, "sourceAddress").parse::<IpAddr>(),
                    number(block, "blockedUntil"),
                ) else {
                    continue;
                };
                if until > now_ms {
                    self.ua
                        .set_block(ip, Some(Instant::now() + Duration::from_millis(until - now_ms)));
                }
            }
        }
        let mut trunks = Vec::new();
        // on an edge node, the trunks loaded for its offer (E10 phones), each loaded until it answers
        let mut loaded = Vec::new();
        self.trunks.clear();
        if let Some(Json::Array(list)) = config.get("trunkList") {
            for trunk in list {
                // one trunk that does not make sense is logged and the others go on
                let id = text(trunk, "voiceTrunkId");
                let Some(transport) = Transport::parse(&text(trunk, "transport")) else {
                    tracing::error!("SIP trunk {id}: unknown transport {}", text(trunk, "transport"));
                    continue;
                };
                let srtp = text(trunk, "mediaEncryption") != "plain";
                // the offer tells each pairing of its own trunks only, by their ids (E12, E16)
                if let Some(node) = owners::owner_of(&id).node() {
                    loaded.push(moqui_core::edge::media::SipTrunkState {
                        paired_node_id: node.to_string(),
                        voice_trunk_id: owners::bare(&id).to_string(),
                        name: text(trunk, "name"),
                        state: "loaded".to_string(),
                        error: String::new(),
                    });
                }
                self.trunks.insert(
                    id.clone(),
                    TrunkInfo {
                        emergency: flag(trunk, "emergencyCapable"),
                        srtp,
                    },
                );
                trunks.push(TrunkConfig {
                    id,
                    host: text(trunk, "host"),
                    port: number(trunk, "port").and_then(|p| u16::try_from(p).ok()),
                    transport,
                    credentials: Credentials {
                        username: text(trunk, "username"),
                        auth_username: Some(text(trunk, "authUsername")).filter(|u| !u.is_empty()),
                        password: text(trunk, "password"),
                        realm: Some(text(trunk, "realm")).filter(|r| !r.is_empty()),
                        algorithms: texts(trunk, "algorithms")
                            .iter()
                            .filter_map(|a| Algorithm::parse(a))
                            .collect(),
                    },
                    domain: Some(text(trunk, "fromDomain")).filter(|d| !d.is_empty()),
                    register: trunk_lock::registers(
                        flag(trunk, "register"),
                        self.links,
                        self.trunk_lock.load(Ordering::Acquire),
                    ),
                    inbound: texts(trunk, "inboundAddresses"),
                    inbound_digest: flag(trunk, "inboundDigest"),
                    srtp,
                });
            }
        }
        tracing::info!("SIP: {} trunks", trunks.len());
        if self.links {
            moqui_core::edge::media::set_sip_trunks(&self.factory, loaded);
        }
        self.ua.set_trunks(trunks).await;
        true
    }

    async fn event(&mut self, event: UaEvent) {
        match event {
            UaEvent::Incoming(call) if call.device.is_some() => self.incoming_device(call).await,
            UaEvent::Incoming(call) => self.incoming(call).await,
            UaEvent::Binding { change, binding } => self.binding_changed(change, &binding),
            UaEvent::DeviceAnswered { leg, answer, source } => self.device_answered(&leg, &answer, source),
            UaEvent::Ringing { .. } => {}
            // V5: a desk phone's blind transfer
            UaEvent::Refer { leg, target, reply } => self.refer(&leg, &target, reply),
            UaEvent::Answered { leg, answer } => {
                let Some(entry) = self.legs.get(&leg) else {
                    // the leg was hung up here as the answer came: end the answered call too
                    self.ua.hangup(&leg);
                    return;
                };
                let room = entry.room.clone();
                match sdp::parse(&answer).and_then(|a| sdp::accept(&entry.local, &a)) {
                    Ok(settled) => {
                        entry.media.send(LegCommand::Settled(Box::new(settled)));
                        self.note_leg(&room, &leg, "answered", Some(200), "");
                        // E8: a fallback's forward answered: the caller is answered too
                        self.forward_answered(&leg);
                    }
                    Err(err) => {
                        tracing::warn!("SIP leg {leg}: the answer did not settle: {err}");
                        self.ua.hangup(&leg);
                        self.legs.remove(&leg);
                        self.note_leg(&room, &leg, "failed", Some(488), &err.to_string());
                    }
                }
            }
            UaEvent::Ended { leg, reason } => {
                self.voicemail.remove(&leg);
                self.forget_ivr(&leg);
                self.ring_timers.clear(&leg);
                let removed = self.legs.remove(&leg);
                // E8: one leg of a fallback's forward ended: the other hangs up, or the caller goes on to a message
                self.fallback_leg_ended(&leg);
                let Some(entry) = removed else { return };
                let (event, code) = ended_event(reason);
                self.note_leg(&entry.room, &leg, event, code, &format!("{reason:?}"));
            }
            UaEvent::Reoffer { leg, offer, reply } => {
                let answer = self.legs.get(&leg).and_then(|entry| {
                    let (answer, settled) = sdp::parse(&offer).and_then(|o| sdp::answer(&o, &entry.local)).ok()?;
                    let held = settled.direction != Direction::SendRecv;
                    entry.media.send(LegCommand::Settled(Box::new(settled)));
                    Some((answer, held, entry.room.clone()))
                });
                if let Some((_, held, room)) = &answer {
                    self.note_leg(room, &leg, if *held { "held" } else { "resumed" }, None, "");
                }
                let _ = reply.send(answer.map(|(a, _, _)| a));
            }
            UaEvent::TrunkStatus { trunk, state } => {
                let (status, error) = match state {
                    TrunkState::Registered => ("registered", String::new()),
                    TrunkState::Reachable => ("ok", String::new()),
                    TrunkState::Failed(err) => ("failed", err),
                    TrunkState::Unregistered => ("unregistered", String::new()),
                };
                // a pairing's trunks only, for its offer (E10, E12, E16)
                if let Some(node) = owners::owner_of(&trunk).node().filter(|_| self.links) {
                    moqui_core::edge::media::note_sip_trunk(&self.factory, node, owners::bare(&trunk), status, &error);
                }
                self.note(
                    NOTE_TRUNK_SERVICE,
                    serde_json::json!({"voiceTrunkId": trunk, "status": status, "errorText": error}),
                );
            }
            UaEvent::AuthFailure {
                source,
                username,
                blocked,
            } => {
                // the block is kept by the services; each failure before it is only counted here, so a flood of failures
                // from many addresses is not a service call and a database write each
                if !blocked {
                    tracing::debug!("SIP: a failed digest from {source}");
                    return;
                }
                tracing::warn!("SIP: {source} is blocked after failed digests (last for {username:?})");
                self.note(
                    AUTH_FAILURE_SERVICE,
                    serde_json::json!({"sourceAddress": source.to_string(), "username": username, "blocked": true}),
                );
            }
        }
    }

    /// The owner of an inbound call to `to` that came on the trunk `trunk` (the engine's key of it), on a node that
    /// takes links: the server's when it has the number, else the pairing whose copies have it, else the trunk's owner
    /// (E12, E16, `owners::number_route`); elsewhere the trunk's.
    async fn number_owner(&self, to: &str, trunk: &str) -> Owner {
        let by_trunk = owners::owner_of(trunk);
        if !self.links {
            return by_trunk;
        }
        owners::number_route(&self.factory, to, by_trunk, self.dedicated).await
    }

    /// The pairings and their SIP device domains, read again (E12, E16): the registrar takes each domain's phones
    /// (their answers checked by that pairing's node), and each pairing's Hello names its own.
    fn paired_changed(&mut self, paired: Vec<(String, String)>) {
        let before = lock(&self.paired).replace(paired.clone()).unwrap_or_default();
        let mut realms: Vec<String> = paired
            .iter()
            .map(|(_, domain)| domain.clone())
            .filter(|domain| self.registrar && !domain.is_empty())
            .collect();
        realms.sort();
        realms.dedup();
        for (node, domain) in &paired {
            if !before.contains(&(node.clone(), domain.clone())) {
                tracing::info!(
                    "SIP: local node {node}'s phones register to {}",
                    if domain.is_empty() {
                        "the registrar's own realm"
                    } else {
                        domain.as_str()
                    }
                );
            }
        }
        self.ua.set_device_realms(&realms);
        moqui_core::edge::media::set_sip_realms(&self.factory, Some(paired));
    }

    /// A leg's RTP socket and its SDP for this side.
    fn local_media(&self, srtp: bool) -> Result<(std::net::UdpSocket, LocalMedia), String> {
        let socket = self.ports.take().ok_or("no free port in sip_rtp_ports")?;
        let addr = leg::sdp_address(self.public, &socket).ok_or("the RTP socket has no address")?;
        let local = LocalMedia::new(addr.ip(), addr.port(), srtp.then(MasterKey::random));
        Ok((socket, local))
    }

    async fn incoming(&mut self, call: IncomingCall) {
        let srtp_required = self.trunks.get(&call.trunk_id).is_none_or(|t| t.srtp);
        let to = dial::normalize(&call.to, &self.policy).unwrap_or_else(|| call.to.clone());
        let from = dial::normalize(&call.from, &self.policy).unwrap_or_else(|| call.from.clone());
        // E12: whose call it is, by the number called on an all-in-one node, else by the trunk
        let owner = self.number_owner(&to, &call.trunk_id).await;
        let mut parameters = serde_json::json!({"toNumber": to, "fromNumber": from, "fromName": call.from_name,
            "instanceId": self.own, "sipCallId": call.call_id});
        // the number's trunk check applies only when the trunk is its owner's: a call routed by number on the other
        // side's trunk carries none, as that side's trunk id means nothing in the owner's database
        if let Some(trunk) = owners::trunk_of(&owner, &call.trunk_id) {
            parameters["voiceTrunkId"] = Json::String(trunk.to_string());
        }
        // V2: voicemail busy or access sends it to voicemail at once; voicemailSeconds is how long it rings before it does
        let mut voicemail_now = None;
        let mut ring_seconds = None;
        let mut step = None;
        let mut ivr_answer = None;
        let routed = call_service(&self.factory, owner.clone(), RECEIVE_SERVICE, parameters).await;
        // E8: a paired node's number whose node is away (no link, or no answer in time) is answered from its fallback
        if let Err(err) = &routed {
            if owner != Owner::Own && edge::is_offline(err) {
                tracing::warn!(
                    "SIP call from {} to {to}: the local node is away ({err}); the number's fallback answers",
                    call.source
                );
                self.answer_from_fallback(call, &owner, to, from, srtp_required).await;
                return;
            }
        }
        let decided = match routed {
            Ok(out) => {
                let room = text(&out, "callSessionId");
                let leg = text(&out, "callLegId");
                voicemail_now = Some(text(&out, "voicemail")).filter(|v| !v.is_empty());
                ring_seconds = number(&out, "voicemailSeconds").filter(|s| *s > 0);
                step = step_token(&out);
                ivr_answer = out.get("ivr").filter(|a| a.is_object()).cloned();
                if room.is_empty() || leg.is_empty() {
                    Err(404)
                } else {
                    Ok((room, leg))
                }
            }
            Err(err) => {
                tracing::info!("SIP call from {} to {to} not taken: {err}", call.source);
                Err(refusal_status(err.split(':').next().unwrap_or_default()))
            }
        };
        let (room, leg) = match decided {
            Ok(ids) => ids,
            Err(code) => {
                let _ = call.reply.send(Decision::Reject(code));
                return;
            }
        };
        if !self.ring_inbound(call, &room, &leg, &from, &to, srtp_required) {
            return;
        }
        // V5: an IVR menu or a queue answers the call itself
        if let Some(answer) = ivr_answer {
            self.start_ivr(&leg, answer);
            return;
        }
        match (voicemail_now, ring_seconds) {
            (Some(reason), _) => self.start_voicemail(&leg, &reason, step),
            (None, Some(seconds)) => self.ring_timer(&leg, seconds, step),
            (None, None) => {}
        }
    }

    /// Rings an inbound call as leg `leg` of room `room`: its offer answered, its media started, the user agent told to
    /// ring it. False when it was refused instead (488, 503, or decided too late), noted to the services.
    fn ring_inbound(
        &mut self,
        call: IncomingCall,
        room: &str,
        leg: &str,
        from: &str,
        to: &str,
        srtp_required: bool,
    ) -> bool {
        let offer = match sdp::parse(&call.offer) {
            Ok(offer) => offer,
            Err(err) => {
                let _ = call.reply.send(Decision::Reject(488));
                self.note_leg(room, leg, "failed", Some(488), &err.to_string());
                return false;
            }
        };
        if srtp_required && !offer.secure {
            let _ = call.reply.send(Decision::Reject(488));
            self.note_leg(
                room,
                leg,
                "failed",
                Some(488),
                "plain RTP offered to a trunk whose profile requires SRTP",
            );
            return false;
        }
        let started = self.local_media(offer.secure).and_then(|(socket, local)| {
            let (answer, settled) = sdp::answer(&offer, &local).map_err(|e| e.to_string())?;
            let notes = self.notes_sender();
            let media = LegMedia::start(room, leg, socket, Some(settled), notes)?;
            Ok((answer, media, local))
        });
        match started {
            Ok((answer, media, local)) => {
                let ring = Decision::Ring {
                    leg: leg.to_string(),
                    answer,
                };
                if call.reply.send(ring).is_err() {
                    // the user agent stopped waiting (the services took longer than it waits) and refused the call:
                    // the leg's media goes, and the call ends
                    tracing::warn!("SIP call {leg}: decided too late, the call was refused");
                    drop(media);
                    self.note_leg(room, leg, "failed", Some(480), "decided too late");
                    self.abandon_room(room);
                    return false;
                }
                self.legs.insert(
                    leg.to_string(),
                    Leg {
                        room: room.to_string(),
                        media,
                        local,
                        caller: from.to_string(),
                        dialed: to.to_string(),
                        answered: false,
                        device: None,
                        srtp_local: None,
                        sip_answered: false,
                        held: false,
                    },
                );
                true
            }
            Err(err) => {
                tracing::warn!("SIP call {leg}: its media did not start: {err}");
                let _ = call.reply.send(Decision::Reject(503));
                self.note_leg(room, leg, "failed", Some(503), &err);
                false
            }
        }
    }

    /// A sender for the leg threads' notes, which come back as commands.
    fn notes_sender(&self) -> UnboundedSender<LegNote> {
        let (tx, mut rx) = unbounded_channel::<LegNote>();
        let commands = self.commands.clone();
        tokio::spawn(async move {
            while let Some(note) = rx.recv().await {
                if commands.send(Command::Note(note)).is_err() {
                    break;
                }
            }
        });
        tx
    }

    async fn command(&mut self, command: Command) {
        match command {
            Command::Control(body) => self.control(&body).await,
            Command::Note(LegNote::Gone { leg }) => {
                self.voicemail.remove(&leg);
                self.forget_ivr(&leg);
                if self.legs.remove(&leg).is_some() {
                    self.ua.hangup(&leg);
                }
            }
            Command::Note(LegNote::Digit { leg, digit }) => {
                if self.voicemail_event(&leg, voicemail::Event::Digit(digit))
                    || self.ivr_event(&leg, ivr::Event::Digit(digit))
                {
                    return;
                }
                if let Some(entry) = self.legs.get(&leg) {
                    tracing::debug!("SIP leg {leg}: digit {digit}");
                    self.note(
                        NOTE_LEG_SERVICE,
                        serde_json::json!({"callSessionId": entry.room, "callLegId": leg, "event": "dtmf",
                            "digits": digit.to_string()}),
                    );
                }
            }
            Command::Prompt { leg, prompt } => match self.legs.get(&leg) {
                Some(entry) => entry.media.send(LegCommand::Prompt(prompt)),
                None => tracing::debug!("SIP: no leg {leg} here for a prompt"),
            },
            Command::Reanswered { leg, answer } => {
                let Some(entry) = self.legs.get(&leg) else { return };
                match answer.as_deref().map(sdp::parse) {
                    Some(Ok(answer)) => match sdp::accept(&entry.local, &answer) {
                        Ok(settled) => entry.media.send(LegCommand::Settled(Box::new(settled))),
                        Err(err) => tracing::warn!("SIP leg {leg}: the re-INVITE's answer did not settle: {err}"),
                    },
                    _ => tracing::warn!("SIP leg {leg}: the re-INVITE was not answered"),
                }
            }
            Command::Note(LegNote::Played { leg, tag }) => {
                if !self.voicemail_event(&leg, voicemail::Event::Played(tag)) {
                    self.ivr_event(&leg, ivr::Event::Played(tag));
                }
            }
            Command::Note(LegNote::Recorded { leg, recorded }) => self.voicemail_recorded(&leg, *recorded),
            Command::VmBegun { leg, reason, result } => self.voicemail_begun(leg, &reason, result),
            Command::VmPin { leg, result } => self.voicemail_pin(&leg, result),
            Command::VmPlay { leg, tag, prompt } => self.voicemail_play(&leg, tag, prompt),
            Command::VmIdle { leg, wait } => {
                self.voicemail_event(&leg, voicemail::Event::Idle(wait));
            }
            Command::IvrPlay { leg, tag, prompt } => self.ivr_play(&leg, tag, prompt),
            Command::IvrIdle { leg, wait } => {
                self.ivr_event(&leg, ivr::Event::Idle(wait));
            }
            Command::Routed { leg, result } => self.routed(&leg, result),
            Command::QueueTick { leg, generation } => self.queue_tick(&leg, generation),
            Command::LoopReady { leg, token, audio } => self.loop_ready(&leg, token, audio),
            Command::ReloadRetry { attempt } => {
                if !self.reload().await {
                    self.retry_reload(attempt + 1);
                }
            }
            Command::FallbackTimeout { leg } => self.forward_timed_out(&leg),
            Command::Paired(paired) => self.paired_changed(paired),
            Command::ReadPairings if self.links => {
                let (factory, commands) = (Arc::clone(&self.factory), self.commands.clone());
                tokio::spawn(async move {
                    if let Some(now) = owners::pairing_domains(&factory).await {
                        let _ = commands.send(Command::Paired(now));
                    }
                });
            }
            Command::ReadPairings => {}
            Command::RingTimeout { leg, generation } => {
                let Some(step) = self.ring_timers.fire(&leg, generation) else {
                    return;
                };
                if self.legs.get(&leg).is_some_and(|entry| !entry.answered) {
                    self.start_voicemail(&leg, "noAnswer", step);
                }
            }
        }
    }

    async fn control(&mut self, body: &Json) {
        let leg = text(body, "callLegId");
        match body.get("action").and_then(Json::as_str).unwrap_or_default() {
            "dial" => self.dial(body),
            "answer" => {
                let Some(entry) = self.legs.get_mut(&leg) else {
                    tracing::debug!("SIP: no leg {leg} here to answer");
                    return;
                };
                entry.answered = true;
                let held = entry.held;
                if let Err(err) = self.answer_leg(&leg) {
                    tracing::warn!("SIP leg {leg}: not answered: {err}");
                }
                // V5: someone here answered: the IVR, the queue and the ringback are over (hold music stays on hold)
                self.ivr.remove(&leg);
                self.queued.remove(&leg);
                if !held {
                    self.set_loop(&leg, None);
                }
            }
            // V5: a transfer's target answered: the far end hears the call again
            "connect" => {
                self.queued.remove(&leg);
                if self.legs.get(&leg).is_some_and(|e| !e.held) {
                    self.set_loop(&leg, None);
                }
            }
            // V5: the far end waits while a blind transfer's target rings
            "ringback" => self.set_loop(&leg, Some(ivr::LoopKind::Ringback)),
            // V5: where the far end goes now (a transfer to a queue, a declined target's voicemail)
            "route" => {
                let answer = body.get("answer").cloned().unwrap_or(Json::Null);
                self.route_answer(&leg, &answer);
            }
            "transferTimer" => {
                let seconds = number(body, "seconds").unwrap_or(30).clamp(1, 600);
                self.transfer_timer(&leg, seconds, text(body, "callTransferId"));
            }
            // V5: a merge moved the leg into another call
            "move" => self.move_leg(&leg, &text(body, "callSessionId")),
            "hangup" => {
                self.legs.remove(&leg);
                self.ua.hangup(&leg);
            }
            "dtmf" => {
                if let Some(entry) = self.legs.get(&leg) {
                    entry.media.send(LegCommand::Digits(text(body, "digits")));
                }
            }
            action @ ("hold" | "resume") => {
                let Some(entry) = self.legs.get_mut(&leg) else { return };
                // V5: hold music in place of the call, and the phone kept from the others; or back
                entry.held = action == "hold";
                entry.media.send(LegCommand::Mute(action == "hold"));
                let music = (action == "hold").then_some(ivr::LoopKind::Music(None));
                self.set_loop(&leg, music);
                let Some(entry) = self.legs.get_mut(&leg) else { return };
                entry.local.direction = if action == "hold" {
                    Direction::SendOnly
                } else {
                    Direction::SendRecv
                };
                entry.local.version += 1;
                let offer = sdp::offer(&entry.local);
                let ua = self.ua.clone();
                let commands = self.commands.clone();
                tokio::spawn(async move {
                    let answer = ua.reinvite(&leg, offer).await.unwrap_or_else(|err| {
                        tracing::warn!("SIP leg {leg}: the re-INVITE failed: {err}");
                        None
                    });
                    let _ = commands.send(Command::Reanswered { leg, answer });
                });
            }
            "prompt" => {
                if !self.legs.contains_key(&leg) {
                    tracing::debug!("SIP: no leg {leg} here for a prompt");
                    return;
                }
                // the deployment's audio is read on a request worker (a dbresource:// location needs one)
                let notice = prompt::Notice::from_code(&text(body, "notice"));
                let factory = Arc::clone(&self.factory);
                let commands = self.commands.clone();
                tokio::spawn(async move {
                    let audio = notice_audio(&factory, notice).await;
                    let prompt = prompt::notice_prompt(notice, audio.as_deref());
                    let _ = commands.send(Command::Prompt { leg, prompt });
                });
            }
            "reload" => {
                self.reload().await;
            }
            // V2: its user declined it (CallServices decline#Call); V3: of the plan's step stepToken, so one sent for a
            // step the plan has left does nothing
            "voicemail" => {
                let reason = Some(text(body, "reason")).filter(|r| !r.is_empty());
                self.start_voicemail(&leg, reason.as_deref().unwrap_or("declined"), step_token(body));
            }
            "ringDevice" => self.ring_device(body),
            "dropDevice" => self.drop_device(body),
            other => tracing::debug!("SIP: unknown control {other}"),
        }
    }

    /// A call whose phone was refused as decided too late (E8): its other legs here hang up, and no dial goes into it
    /// for a while.
    fn abandon_room(&mut self, room: &str) {
        let now = Instant::now();
        self.abandoned.retain(|_, at| now.duration_since(*at) < ABANDONED_FOR);
        self.abandoned.insert(room.to_string(), now);
        let legs: Vec<String> = self
            .legs
            .iter()
            .filter(|(_, entry)| entry.room == room)
            .map(|(leg, _)| leg.clone())
            .collect();
        for leg in legs {
            tracing::warn!("SIP leg {leg}: its call was given up, hung up");
            self.end_leg(&leg, room);
        }
    }

    /// An outbound call the services placed, checked again here before it goes to a trunk.
    fn dial(&mut self, body: &Json) {
        let room = text(body, "callSessionId");
        let leg = text(body, "callLegId");
        if self.abandoned.get(&room).is_some_and(|at| at.elapsed() < ABANDONED_FOR) {
            tracing::warn!("SIP call {leg} refused: its call was given up");
            self.note_leg(&room, &leg, "failed", Some(480), "the call was given up");
            return;
        }
        let trunk_id = text(body, "voiceTrunkId");
        let emergency = flag(body, "emergency");
        let refuse = |engine: &Engine, why: &str| {
            tracing::warn!("SIP call {leg} refused: {why}");
            engine.note_leg(&room, &leg, "failed", Some(403), why);
        };
        let Some(trunk) = self.trunks.get(&trunk_id).cloned() else {
            return refuse(self, "the trunk is not known to this SIP engine");
        };
        let Some((to, class)) = dial::classify(&text(body, "toNumber"), &self.policy) else {
            return refuse(self, "not a phone number");
        };
        if emergency != (class == DestinationClass::Emergency) {
            return refuse(self, "an emergency number goes only through the emergency path");
        }
        if emergency && !trunk.emergency {
            return refuse(self, "the trunk does not take emergency calls");
        }
        if let Err(code) = dial::deployment_allows(class, &self.policy) {
            return refuse(self, code);
        }
        if !emergency && self.max_concurrent > 0 && self.legs.len() >= self.max_concurrent {
            return refuse(self, "tooManyCalls");
        }
        let from = text(body, "fromNumber");
        let started = self.local_media(trunk.srtp).and_then(|(socket, local)| {
            let media = LegMedia::start(&room, &leg, socket, None, self.notes_sender())?;
            Ok((media, local))
        });
        let (media, local) = match started {
            Ok(started) => started,
            Err(err) => {
                tracing::warn!("SIP call {leg}: its media did not start: {err}");
                self.note_leg(&room, &leg, "failed", Some(503), &err);
                return;
            }
        };
        let offer = sdp::offer(&local);
        let mut headers = Vec::new();
        if emergency {
            let callback = text(body, "callbackNumber");
            if !callback.is_empty() {
                headers.push(("P-Asserted-Identity".to_string(), format!("<tel:{callback}>")));
            }
        } else if flag(body, "privacy") {
            // caller-ID blocking (V3): the trunk still sees the number it authorizes, and withholds it (RFC 3323, 3325)
            headers.push(("Privacy".to_string(), "id".to_string()));
        }
        self.legs.insert(
            leg.clone(),
            Leg {
                room: room.clone(),
                media,
                local,
                caller: String::new(),
                dialed: to.clone(),
                answered: true,
                device: None,
                srtp_local: None,
                sip_answered: false,
                held: false,
            },
        );
        let request = DialRequest {
            leg: leg.clone(),
            trunk: trunk_id,
            to,
            from,
            from_name: Some(text(body, "fromName")).filter(|n| !n.is_empty()),
            offer,
            ring_timeout: RING_TIMEOUT,
            headers,
        };
        if let Err(err) = self.ua.dial(request) {
            self.legs.remove(&leg);
            self.note_leg(&room, &leg, "failed", Some(503), &err);
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
