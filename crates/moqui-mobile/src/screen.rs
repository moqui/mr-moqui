// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

//! The native screen share's leg (plans/Comms-Whiteboard.md §3.6, plans/MM-Spike.md): a WebRTC client of the media
//! server on str0m, sending the VP8 frames MediaCodec encodes from the MediaProjection (`ScreenShare.kt`) as the
//! participant's screen leg. The page signals for it: it makes the leg (`add#ScreenLeg`) and relays the leg's rtc frames
//! on its own socket, so no credential of the account is here; this side only makes the offer, takes the answer and
//! the media server's later offers, and sends the media.
//!
//! One thread owns the [`Rtc`] (str0m is sans-IO): it takes datagrams from the sockets' reader threads, frames from the
//! encoder (over JNI) and the page's descriptions, and asks the encoder for a keyframe when the media server asks
//! (PLI/FIR) and for a bitrate when the TWCC estimate changes. The server's candidates come in its answer (it is
//! ICE-lite): a UDP socket for each local address that reaches one of them (IPv4 and IPv6), and an active ICE-TCP
//! candidate for its TCP ones (RFC 4571 framing), which ICE takes when UDP does not get through. The leg ends when the
//! page stops it, when its connection is lost for [`LOST_AFTER`] or never made in [`CONNECT_WITHIN`], and the page is
//! told (`screen`).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use str0m::bwe::{Bitrate, BweKind};
use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::format::Codec;
use str0m::media::{Direction, MediaKind, MediaTime, Mid};
use str0m::net::{Protocol, Receive, TcpType};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};

/// The bitrate the encoder starts at and the most the estimate may ask for: a screen at 1280 px, mostly still.
pub(crate) const START_BPS: u64 = 1_200_000;
const MAX_BPS: u64 = 2_500_000;
const MIN_BPS: u64 = 150_000;
/// How often the encoder may be asked for a new bitrate (and only on a change of 15 % or more).
const BITRATE_EVERY: Duration = Duration::from_millis(1000);
/// A connection lost this long ends the leg (str0m's ICE consent is checked each few seconds).
pub(crate) const LOST_AFTER: Duration = Duration::from_secs(10);
/// A leg whose connection is not made this long after the answer ends.
pub(crate) const CONNECT_WITHIN: Duration = Duration::from_secs(20);
/// An ICE-TCP connection's connect.
const TCP_CONNECT: Duration = Duration::from_secs(3);

enum Msg {
    Offer(Sender<Result<String, String>>),
    Answer(String, Sender<Result<(), String>>),
    RemoteOffer(String, Sender<Result<String, String>>),
    Datagram(Vec<u8>, Protocol, SocketAddr, SocketAddr),
    TcpConnected(SocketAddr, TcpStream),
    TcpFailed(SocketAddr),
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    Frame(Vec<u8>, i64),
    Status(Sender<String>),
    Stop,
}

/// What the page and the native code reach: the leg's thread, while there is one, by its number.
static LEG: Mutex<Option<(u64, Sender<Msg>)>> = Mutex::new(None);
static NEXT_LEG: AtomicU64 = AtomicU64::new(1);

fn sender() -> Option<Sender<Msg>> {
    moqui_lib::sync::lock(&LEG).as_ref().map(|(_, tx)| tx.clone())
}

fn ask<T>(make: impl FnOnce(Sender<T>) -> Msg) -> Result<T, String> {
    let tx = sender().ok_or("No screen share")?;
    let (reply, answer) = mpsc::channel();
    tx.send(make(reply)).map_err(|_| "The screen share ended")?;
    answer
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| "The screen share did not answer".to_string())
}

/// A new leg's offer: one send-only VP8 video, with no candidates (the media server is ICE-lite; this side's host
/// candidates are added once the answer names the server's addresses). An earlier leg ends first.
pub(crate) fn offer() -> Result<String, String> {
    stop();
    let (tx, rx) = mpsc::channel();
    let id = NEXT_LEG.fetch_add(1, Ordering::Relaxed);
    *moqui_lib::sync::lock(&LEG) = Some((id, tx.clone()));
    std::thread::Builder::new()
        .name("screen-leg".into())
        .spawn(move || run(id, rx, tx))
        .map_err(|err| err.to_string())?;
    ask(Msg::Offer)?
}

/// The media server's answer to the offer (the `joined` frame's `sdp`).
pub(crate) fn answer(sdp: String) -> Result<(), String> {
    ask(|reply| Msg::Answer(sdp, reply))?
}

/// An offer of the media server's (an ICE restart, a renegotiation): the answer for the page to send.
pub(crate) fn remote_offer(sdp: String) -> Result<String, String> {
    ask(|reply| Msg::RemoteOffer(sdp, reply))?
}

/// The leg's state as JSON, for the page.
pub(crate) fn status() -> String {
    ask(Msg::Status).unwrap_or_else(|_| r#"{"state":"none"}"#.to_string())
}

/// An encoded VP8 frame from the encoder, its presentation time in microseconds.
// over JNI from the encoder, so only on Android (and in the tests)
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn frame(data: Vec<u8>, pts_us: i64) {
    if let Some(tx) = sender() {
        let _ = tx.send(Msg::Frame(data, pts_us));
    }
}

/// The share ends: the leg's thread closes its connection and goes.
pub(crate) fn stop() {
    if let Some((_, tx)) = moqui_lib::sync::lock(&LEG).take() {
        let _ = tx.send(Msg::Stop);
    }
}

fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// An ICE-TCP connection to one of the server's TCP candidates: frames waiting for its connect, then the stream.
enum Tcp {
    Connecting(Vec<Vec<u8>>),
    Open(TcpStream),
    Failed,
}

struct Leg {
    rtc: Rtc,
    mid: Option<Mid>,
    pending: Option<SdpPendingOffer>,
    udp: Vec<(SocketAddr, UdpSocket)>,
    tcp: HashMap<SocketAddr, Tcp>,
    tcp_local: Vec<SocketAddr>,
    answered: Option<Instant>,
    lost_since: Option<Instant>,
    ended: Option<&'static str>,
    connected_ms: Option<u64>,
    frames_in: u64,
    frames_sent: u64,
    bytes_sent: u64,
    first_sent_ms: Option<u64>,
    keyframe_requests: u64,
    last_keyframe_request_ms: Option<u64>,
    estimate_bps: Option<u64>,
    asked_bps: u64,
    asked_at: Instant,
    waiting_key: bool,
}

fn run(id: u64, rx: Receiver<Msg>, tx: Sender<Msg>) {
    let now = Instant::now();
    let rtc = Rtc::builder()
        .clear_codecs()
        .enable_vp8(true)
        .enable_bwe(Some(Bitrate::bps(START_BPS)))
        .build(now);
    let mut leg = Leg {
        rtc,
        mid: None,
        pending: None,
        udp: Vec::new(),
        tcp: HashMap::new(),
        tcp_local: Vec::new(),
        answered: None,
        lost_since: None,
        ended: None,
        connected_ms: None,
        frames_in: 0,
        frames_sent: 0,
        bytes_sent: 0,
        first_sent_ms: None,
        keyframe_requests: 0,
        last_keyframe_request_ms: None,
        estimate_bps: None,
        asked_bps: START_BPS,
        asked_at: now,
        waiting_key: true,
    };
    log::info!("Screen leg: started");
    let mut wait = Duration::from_millis(50);
    let reason = loop {
        let msg = match rx.recv_timeout(wait) {
            Ok(msg) => Some(msg),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => break "stopped",
        };
        let now = Instant::now();
        match msg {
            Some(Msg::Stop) => break "stopped",
            Some(Msg::Offer(reply)) => {
                let _ = reply.send(leg.make_offer());
            }
            Some(Msg::Answer(sdp, reply)) => {
                let _ = reply.send(leg.take_answer(now, &sdp, &tx));
            }
            Some(Msg::RemoteOffer(sdp, reply)) => {
                let _ = reply.send(leg.take_offer(&sdp));
            }
            Some(Msg::Status(reply)) => {
                let _ = reply.send(leg.status());
            }
            Some(Msg::Datagram(data, proto, source, destination)) => {
                if let Ok(contents) = data.as_slice().try_into() {
                    let input = Input::Receive(
                        now,
                        Receive {
                            proto,
                            source,
                            destination,
                            contents,
                        },
                    );
                    if let Err(err) = leg.rtc.handle_input(input) {
                        log::warn!("Screen leg: a datagram failed: {err}");
                    }
                }
            }
            Some(Msg::TcpConnected(remote, stream)) => leg.tcp_open(remote, stream, &tx),
            Some(Msg::TcpFailed(remote)) => {
                leg.tcp.insert(remote, Tcp::Failed);
            }
            Some(Msg::Frame(data, pts_us)) => leg.send_frame(now, &data, pts_us),
            None => {}
        }
        if let Err(err) = leg.rtc.handle_input(Input::Timeout(now)) {
            log::warn!("Screen leg: {err}");
        }
        match leg.drain(now, &tx) {
            Ok(next) => wait = next,
            Err(err) => {
                log::warn!("Screen leg: closed: {err}");
                break "failed";
            }
        }
        if let Some(reason) = leg.ended.or_else(|| leg.gone(now)) {
            break reason;
        }
    };
    leg.rtc.disconnect();
    for (_, t) in leg.tcp.drain() {
        if let Tcp::Open(stream) = t {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
    log::info!("Screen leg: ended ({reason}) after {} frames sent", leg.frames_sent);
    // this leg goes, not a newer one the page started since
    {
        let mut current = moqui_lib::sync::lock(&LEG);
        if current.as_ref().is_some_and(|(leg, _)| *leg == id) {
            *current = None;
        }
    }
    #[cfg(target_os = "android")]
    crate::android::screen_leg_ended();
    crate::native::tell_page(
        "screen",
        &serde_json::json!({"state": "ended", "reason": reason}).to_string(),
    );
}

impl Leg {
    fn make_offer(&mut self) -> Result<String, String> {
        let mut change = self.rtc.sdp_api();
        let mid = change.add_media(MediaKind::Video, Direction::SendOnly, Some("screen".into()), None, None);
        let (offer, pending) = change.apply().ok_or("No offer")?;
        self.mid = Some(mid);
        self.pending = Some(pending);
        Ok(offer.to_sdp_string())
    }

    fn take_answer(&mut self, now: Instant, sdp: &str, tx: &Sender<Msg>) -> Result<(), String> {
        let pending = self.pending.take().ok_or("No offer is out")?;
        let answer = SdpAnswer::from_sdp_string(sdp).map_err(|err| err.to_string())?;
        let servers = server_candidates(sdp);
        if servers.is_empty() {
            return Err("The answer names no address of the media server".to_string());
        }
        self.rtc
            .sdp_api()
            .accept_answer(pending, answer)
            .map_err(|err| err.to_string())?;
        // a socket for each local address that reaches a server's UDP candidate (the interface a socket connected to it
        // names): str0m pairs each with the server's candidates of its family
        for server in servers.iter().filter(|c| c.0 == Protocol::Udp).map(|c| c.1) {
            let Some(ip) = local_ip_for(server) else { continue };
            if self.udp.iter().any(|(local, _)| local.ip() == ip) {
                continue;
            }
            match self.open_udp(ip, tx) {
                Ok(local) => log::info!("Screen leg: UDP from {local}"),
                Err(err) => log::warn!("Screen leg: no UDP socket on {ip}: {err}"),
            }
        }
        // ICE-TCP to the server's passive TCP candidates: an active candidate of each local address (port 9, RFC 6544)
        for server in servers.iter().filter(|c| c.0 == Protocol::Tcp).map(|c| c.1) {
            let Some(ip) = local_ip_for(server) else { continue };
            let local = SocketAddr::new(ip, 9);
            if self.tcp_local.contains(&local) {
                continue;
            }
            match Candidate::builder().tcp().host(local).tcptype(TcpType::Active).build() {
                Ok(candidate) => {
                    self.rtc.add_local_candidate(candidate);
                    self.tcp_local.push(local);
                }
                Err(err) => log::warn!("Screen leg: no TCP candidate on {ip}: {err}"),
            }
        }
        if self.udp.is_empty() && self.tcp_local.is_empty() {
            return Err("No address of the media server can be reached from here".to_string());
        }
        self.answered = Some(now);
        Ok(())
    }

    fn open_udp(&mut self, ip: IpAddr, tx: &Sender<Msg>) -> Result<SocketAddr, String> {
        let socket = UdpSocket::bind(SocketAddr::new(ip, 0)).map_err(|err| err.to_string())?;
        let local = socket.local_addr().map_err(|err| err.to_string())?;
        let candidate = Candidate::host(local, "udp").map_err(|err| err.to_string())?;
        self.rtc.add_local_candidate(candidate);
        let reader = socket.try_clone().map_err(|err| err.to_string())?;
        let tx = tx.clone();
        std::thread::Builder::new()
            .name("screen-leg-udp".into())
            .spawn(move || {
                let mut buf = vec![0u8; 2000];
                while let Ok((n, from)) = reader.recv_from(&mut buf) {
                    if tx
                        .send(Msg::Datagram(buf[..n].to_vec(), Protocol::Udp, from, local))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|err| err.to_string())?;
        self.udp.push((local, socket));
        Ok(local)
    }

    fn take_offer(&mut self, sdp: &str) -> Result<String, String> {
        let offer = SdpOffer::from_sdp_string(sdp).map_err(|err| err.to_string())?;
        let answer = self.rtc.sdp_api().accept_offer(offer).map_err(|err| err.to_string())?;
        Ok(answer.to_sdp_string())
    }

    fn send_frame(&mut self, now: Instant, data: &[u8], pts_us: i64) {
        self.frames_in += 1;
        let Some(mid) = self.mid else { return };
        if self.connected_ms.is_none() {
            return;
        }
        // a subscriber decodes nothing before a keyframe: skip to the first one after connecting
        let key = is_vp8_keyframe(data);
        if self.waiting_key && !key {
            return;
        }
        self.waiting_key = false;
        let Some(writer) = self.rtc.writer(mid) else { return };
        let Some(pt) = writer
            .payload_params()
            .find(|p| p.spec().codec == Codec::Vp8)
            .map(|p| p.pt())
        else {
            return;
        };
        let time = MediaTime::new(pts_us.max(0) as u64 * 9 / 100, str0m::media::Frequency::NINETY_KHZ);
        match writer.write(pt, now, time, data.to_vec()) {
            Ok(()) => {
                self.frames_sent += 1;
                self.bytes_sent += data.len() as u64;
                if self.first_sent_ms.is_none() {
                    self.first_sent_ms = Some(wall_ms());
                    log::info!("Screen leg: first frame sent ({} bytes, key {key})", data.len());
                }
            }
            Err(err) => log::warn!("Screen leg: a frame was not sent: {err}"),
        }
    }

    /// Everything str0m has to send or say; the wait until its next timeout.
    fn drain(&mut self, now: Instant, tx: &Sender<Msg>) -> Result<Duration, String> {
        loop {
            match self.rtc.poll_output().map_err(|err| err.to_string())? {
                Output::Timeout(at) => {
                    return Ok(at
                        .saturating_duration_since(now)
                        .clamp(Duration::from_millis(1), Duration::from_millis(100)));
                }
                Output::Transmit(t) => match t.proto {
                    Protocol::Udp => {
                        if let Some((_, socket)) = self.udp.iter().find(|(local, _)| *local == t.source) {
                            let _ = socket.send_to(&t.contents, t.destination);
                        }
                    }
                    _ => self.tcp_send(t.destination, &t.contents, tx),
                },
                Output::Event(e) => self.event(now, e),
            }
        }
    }

    /// A frame to the server's TCP candidate `remote`, framed by its length (RFC 4571); the connection is made in the
    /// background on the first, and what comes before it is open waits for it.
    fn tcp_send(&mut self, remote: SocketAddr, contents: &[u8], tx: &Sender<Msg>) {
        let Ok(len) = u16::try_from(contents.len()) else { return };
        let mut framed = Vec::with_capacity(contents.len() + 2);
        framed.extend_from_slice(&len.to_be_bytes());
        framed.extend_from_slice(contents);
        match self.tcp.get_mut(&remote) {
            Some(Tcp::Open(stream)) => {
                if stream.write_all(&framed).is_err() {
                    self.tcp.insert(remote, Tcp::Failed);
                }
            }
            Some(Tcp::Connecting(waiting)) => {
                if waiting.len() < 64 {
                    waiting.push(framed);
                }
            }
            Some(Tcp::Failed) => {}
            None => {
                self.tcp.insert(remote, Tcp::Connecting(vec![framed]));
                let tx = tx.clone();
                let _ = std::thread::Builder::new()
                    .name("screen-leg-tcp".into())
                    .spawn(move || {
                        let msg = match TcpStream::connect_timeout(&remote, TCP_CONNECT) {
                            Ok(stream) => Msg::TcpConnected(remote, stream),
                            Err(_) => Msg::TcpFailed(remote),
                        };
                        let _ = tx.send(msg);
                    });
            }
        }
    }

    fn tcp_open(&mut self, remote: SocketAddr, mut stream: TcpStream, tx: &Sender<Msg>) {
        let _ = stream.set_nodelay(true);
        let Some(Tcp::Connecting(waiting)) = self.tcp.remove(&remote) else {
            return;
        };
        for framed in waiting {
            if stream.write_all(&framed).is_err() {
                self.tcp.insert(remote, Tcp::Failed);
                return;
            }
        }
        // str0m takes what comes on it as received at the active candidate of the interface it left by
        let local = stream
            .local_addr()
            .ok()
            .and_then(|a| self.tcp_local.iter().copied().find(|l| l.ip() == a.ip()))
            .or_else(|| self.tcp_local.first().copied());
        if let (Some(local), Ok(mut reader)) = (local, stream.try_clone()) {
            let tx = tx.clone();
            let _ = std::thread::Builder::new()
                .name("screen-leg-tcp".into())
                .spawn(move || {
                    let mut len = [0u8; 2];
                    let mut buf = vec![0u8; 65_536];
                    while reader.read_exact(&mut len).is_ok() {
                        let n = u16::from_be_bytes(len) as usize;
                        if reader.read_exact(&mut buf[..n]).is_err() {
                            break;
                        }
                        if tx
                            .send(Msg::Datagram(buf[..n].to_vec(), Protocol::Tcp, remote, local))
                            .is_err()
                        {
                            break;
                        }
                    }
                });
        }
        self.tcp.insert(remote, Tcp::Open(stream));
    }

    /// Why the leg ends now, if it does: its connection lost too long, or never made.
    fn gone(&self, now: Instant) -> Option<&'static str> {
        if self.lost_since.is_some_and(|at| now.duration_since(at) >= LOST_AFTER) {
            return Some("lost");
        }
        if self.connected_ms.is_none() && self.answered.is_some_and(|at| now.duration_since(at) >= CONNECT_WITHIN) {
            return Some("unreachable");
        }
        if !self.rtc.is_alive() {
            return Some("closed");
        }
        None
    }

    fn event(&mut self, now: Instant, e: Event) {
        match e {
            Event::Connected => {
                self.connected_ms = Some(wall_ms());
                self.waiting_key = true;
                log::info!("Screen leg: connected");
                // the estimate may probe up to what the share can use
                self.rtc.bwe().set_bitrate(Bitrate::bps(MIN_BPS), Bitrate::bps(MAX_BPS));
                keyframe();
            }
            Event::IceConnectionStateChange(state) => {
                log::info!("Screen leg: ICE {state:?}");
                match state {
                    IceConnectionState::Disconnected => {
                        self.waiting_key = true;
                        self.lost_since.get_or_insert(now);
                    }
                    // back after a loss: subscribers start again from a keyframe
                    IceConnectionState::Connected | IceConnectionState::Completed
                        if self.lost_since.take().is_some() =>
                    {
                        keyframe();
                    }
                    _ => {}
                }
            }
            Event::KeyframeRequest(req) => {
                self.keyframe_requests += 1;
                self.last_keyframe_request_ms = Some(wall_ms());
                log::debug!("Screen leg: keyframe asked ({:?})", req.kind);
                keyframe();
            }
            Event::EgressBitrateEstimate(BweKind::Twcc { estimate, .. } | BweKind::Remb { estimate, .. }) => {
                let bps = estimate.as_u64();
                self.estimate_bps = Some(bps);
                if let Some(want) = encoder_bitrate(bps, self.asked_bps, now.duration_since(self.asked_at)) {
                    log::info!("Screen leg: estimate {bps} bps, the encoder to {want}");
                    self.asked_bps = want;
                    self.asked_at = now;
                    bitrate(want);
                }
            }
            _ => {}
        }
    }

    fn status(&self) -> String {
        serde_json::json!({
            "state": if self.connected_ms.is_some() {
                if self.lost_since.is_some() { "reconnecting" } else { "connected" }
            } else if self.answered.is_some() {
                "connecting"
            } else {
                "offered"
            },
            "local": self.udp.iter().map(|(a, _)| a.to_string()).chain(self.tcp_local.iter().map(|a| format!("tcp {a}"))).collect::<Vec<_>>(),
            "connectedMs": self.connected_ms,
            "firstSentMs": self.first_sent_ms,
            "framesIn": self.frames_in,
            "framesSent": self.frames_sent,
            "bytesSent": self.bytes_sent,
            "keyframeRequests": self.keyframe_requests,
            "lastKeyframeRequestMs": self.last_keyframe_request_ms,
            "estimateBps": self.estimate_bps,
            "encoderBps": self.asked_bps,
        })
        .to_string()
    }
}

/// The encoder's new bitrate for an estimate of `estimate` bps, if it is to change: 85 % of the estimate within
/// [`MIN_BPS`] and [`MAX_BPS`], at most once in [`BITRATE_EVERY`] and on a change of 15 % or more of `current`.
pub(crate) fn encoder_bitrate(estimate: u64, current: u64, since: Duration) -> Option<u64> {
    let want = (estimate * 85 / 100).clamp(MIN_BPS, MAX_BPS);
    let moved = want.abs_diff(current) * 100 / current.max(1) >= 15;
    (moved && since >= BITRATE_EVERY).then_some(want)
}

/// The media server's candidates in its answer (ICE-lite: its host candidates are in the description): UDP ones, and
/// TCP passive ones (ICE-TCP), IPv4 and IPv6.
pub(crate) fn server_candidates(sdp: &str) -> Vec<(Protocol, SocketAddr)> {
    sdp.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("a=candidate:")?;
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() < 8 || parts[6] != "typ" {
                return None;
            }
            let ip: IpAddr = parts[4].parse().ok()?;
            let port: u16 = parts[5].parse().ok()?;
            let proto = if parts[2].eq_ignore_ascii_case("udp") {
                Protocol::Udp
            } else if parts[2].eq_ignore_ascii_case("tcp")
                && parts.windows(2).any(|w| w[0] == "tcptype" && w[1] == "passive")
            {
                Protocol::Tcp
            } else {
                return None;
            };
            Some((proto, SocketAddr::new(ip, port)))
        })
        .fold(Vec::new(), |mut all, c| {
            if !all.contains(&c) {
                all.push(c);
            }
            all
        })
}

/// The local address this phone reaches `server` from: a UDP socket connected to it names its interface (nothing is
/// sent). None when no route reaches it (an IPv6 server from a phone without IPv6).
fn local_ip_for(server: SocketAddr) -> Option<IpAddr> {
    let probe = UdpSocket::bind(if server.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" }).ok()?;
    probe.connect(server).ok()?;
    probe.local_addr().ok().map(|a| a.ip())
}

/// A VP8 frame's keyframe bit (RFC 6386 §9.1: the frame tag's first bit is 0 for a key frame).
pub(crate) fn is_vp8_keyframe(data: &[u8]) -> bool {
    data.first().is_some_and(|b| b & 1 == 0)
}

fn keyframe() {
    #[cfg(target_os = "android")]
    crate::android::screen_keyframe();
}

#[cfg_attr(not(target_os = "android"), allow(unused_variables))]
fn bitrate(bps: u64) {
    #[cfg(target_os = "android")]
    crate::android::screen_bitrate(bps);
}

#[cfg(test)]
#[path = "screen_tests.rs"]
mod tests;
