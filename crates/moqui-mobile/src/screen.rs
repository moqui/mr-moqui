// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

//! The native screen share's leg (plans/Comms-Whiteboard.md §3.6, plans/MM-Spike.md): a WebRTC client of the media
//! server on str0m, sending the VP8 frames MediaCodec encodes from the MediaProjection (`ScreenShare.kt`) as the
//! participant's screen leg. The page signals for it: it makes the leg (`add#ScreenLeg`) and relays the leg's rtc frames
//! on its own socket, so no credential of the account is here; this side only makes the offer, takes the answer and
//! the media server's later offers, and sends the media.
//!
//! One thread owns the [`Rtc`] (str0m is sans-IO): it takes datagrams from the socket's reader thread, frames from the
//! encoder (over JNI) and the page's descriptions, and asks the encoder for a keyframe when the media server asks
//! (PLI/FIR) and for a bitrate when the TWCC estimate changes.

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use str0m::bwe::{Bitrate, BweKind};
use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::format::Codec;
use str0m::media::{Direction, MediaKind, MediaTime, Mid};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};

/// The bitrate the encoder starts at and the most the estimate may ask for: a screen at 1280 px, mostly still.
pub(crate) const START_BPS: u64 = 1_200_000;
const MAX_BPS: u64 = 2_500_000;
const MIN_BPS: u64 = 150_000;
/// How often the encoder may be asked for a new bitrate, and by how much it must change.
const BITRATE_EVERY: Duration = Duration::from_millis(1000);

enum Msg {
    Offer(Sender<Result<String, String>>),
    Answer(String, Sender<Result<(), String>>),
    RemoteOffer(String, Sender<Result<String, String>>),
    Datagram(Vec<u8>, SocketAddr, SocketAddr),
    Frame(Vec<u8>, i64),
    Status(Sender<String>),
    Stop,
}

/// What the page and the native code reach: the leg's thread, while there is one.
static LEG: Mutex<Option<Sender<Msg>>> = Mutex::new(None);

fn sender() -> Option<Sender<Msg>> {
    moqui_lib::sync::lock(&LEG).clone()
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
/// candidate is added once the answer names the server's address). An earlier leg ends first.
pub(crate) fn offer() -> Result<String, String> {
    stop();
    let (tx, rx) = mpsc::channel();
    *moqui_lib::sync::lock(&LEG) = Some(tx.clone());
    std::thread::Builder::new()
        .name("screen-leg".into())
        .spawn(move || run(rx, tx))
        .map_err(|err| err.to_string())?;
    ask(Msg::Offer)?
}

/// The media server's answer to the offer (the `joined` frame's `sdp`).
pub(crate) fn answer(sdp: String) -> Result<(), String> {
    ask(|reply| Msg::Answer(sdp, reply))?
}

/// An offer of the media server's (other legs' tracks, an ICE restart): the answer for the page to send.
pub(crate) fn remote_offer(sdp: String) -> Result<String, String> {
    ask(|reply| Msg::RemoteOffer(sdp, reply))?
}

/// The leg's state as JSON, for the page and the spike's measurements.
pub(crate) fn status() -> String {
    ask(Msg::Status).unwrap_or_else(|_| r#"{"state":"none"}"#.to_string())
}

/// An encoded VP8 frame from the encoder, its presentation time in microseconds.
pub(crate) fn frame(data: Vec<u8>, pts_us: i64) {
    if let Some(tx) = sender() {
        let _ = tx.send(Msg::Frame(data, pts_us));
    }
}

/// The share ends: the leg's thread closes its connection and goes.
pub(crate) fn stop() {
    if let Some(tx) = moqui_lib::sync::lock(&LEG).take() {
        let _ = tx.send(Msg::Stop);
    }
}

fn wall_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

struct Leg {
    rtc: Rtc,
    mid: Option<Mid>,
    pending: Option<SdpPendingOffer>,
    socket: Option<UdpSocket>,
    local: Option<SocketAddr>,
    started: Instant,
    connected_ms: Option<u128>,
    frames_in: u64,
    frames_sent: u64,
    bytes_sent: u64,
    first_sent_ms: Option<u128>,
    keyframe_requests: u64,
    last_keyframe_request_ms: Option<u128>,
    estimate_bps: Option<u64>,
    asked_bps: u64,
    asked_at: Instant,
    waiting_key: bool,
}

fn run(rx: Receiver<Msg>, tx: Sender<Msg>) {
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
        socket: None,
        local: None,
        started: now,
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
    loop {
        let msg = match rx.recv_timeout(wait) {
            Ok(msg) => Some(msg),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let now = Instant::now();
        match msg {
            Some(Msg::Stop) => break,
            Some(Msg::Offer(reply)) => {
                let _ = reply.send(leg.make_offer());
            }
            Some(Msg::Answer(sdp, reply)) => {
                let _ = reply.send(leg.take_answer(&sdp, &tx));
            }
            Some(Msg::RemoteOffer(sdp, reply)) => {
                let _ = reply.send(leg.take_offer(&sdp));
            }
            Some(Msg::Status(reply)) => {
                let _ = reply.send(leg.status());
            }
            Some(Msg::Datagram(data, source, destination)) => {
                if let Ok(contents) = data.as_slice().try_into() {
                    let input = Input::Receive(
                        now,
                        Receive {
                            proto: Protocol::Udp,
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
            Some(Msg::Frame(data, pts_us)) => leg.send_frame(now, &data, pts_us),
            None => {}
        }
        if let Err(err) = leg.rtc.handle_input(Input::Timeout(now)) {
            log::warn!("Screen leg: {err}");
        }
        match leg.drain(now) {
            Ok(next) => wait = next,
            Err(err) => {
                log::warn!("Screen leg: closed: {err}");
                break;
            }
        }
        if !leg.rtc.is_alive() {
            log::info!("Screen leg: the connection is closed");
            break;
        }
    }
    leg.rtc.disconnect();
    log::info!("Screen leg: ended after {} frames sent", leg.frames_sent);
    #[cfg(target_os = "android")]
    crate::android::screen_leg_ended();
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

    fn take_answer(&mut self, sdp: &str, tx: &Sender<Msg>) -> Result<(), String> {
        let pending = self.pending.take().ok_or("No offer is out")?;
        let answer = SdpAnswer::from_sdp_string(sdp).map_err(|err| err.to_string())?;
        let server = server_address(sdp).ok_or("The answer names no UDP address of the media server")?;
        self.rtc
            .sdp_api()
            .accept_answer(pending, answer)
            .map_err(|err| err.to_string())?;
        // the address the phone reaches the server from: a socket connected to it names its own interface
        let socket = UdpSocket::bind(if server.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" })
            .and_then(|s| s.connect(server).map(|_| s))
            .map_err(|err| err.to_string())?;
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
                    if tx.send(Msg::Datagram(buf[..n].to_vec(), from, local)).is_err() {
                        break;
                    }
                }
            })
            .map_err(|err| err.to_string())?;
        log::info!("Screen leg: answered; {local} to {server}");
        self.socket = Some(socket);
        self.local = Some(local);
        Ok(())
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
                    log::info!(
                        "Screen leg: first frame sent at {} ({} bytes, key {key})",
                        wall_ms(),
                        data.len()
                    );
                }
            }
            Err(err) => log::warn!("Screen leg: a frame was not sent: {err}"),
        }
    }

    /// Everything str0m has to send or say; the wait until its next timeout.
    fn drain(&mut self, now: Instant) -> Result<Duration, String> {
        loop {
            match self.rtc.poll_output().map_err(|err| err.to_string())? {
                Output::Timeout(at) => {
                    return Ok(at
                        .saturating_duration_since(now)
                        .clamp(Duration::from_millis(1), Duration::from_millis(100)));
                }
                Output::Transmit(t) => {
                    if let Some(socket) = &self.socket {
                        let _ = socket.send(&t.contents);
                    }
                }
                Output::Event(e) => self.event(now, e),
            }
        }
    }

    fn event(&mut self, now: Instant, e: Event) {
        match e {
            Event::Connected => {
                self.connected_ms = Some(wall_ms());
                self.waiting_key = true;
                log::info!(
                    "Screen leg: connected at {} ({} ms after the offer)",
                    wall_ms(),
                    now.duration_since(self.started).as_millis()
                );
                // the estimate may probe up to what the share can use
                self.rtc.bwe().set_bitrate(Bitrate::bps(MIN_BPS), Bitrate::bps(MAX_BPS));
                keyframe();
            }
            Event::IceConnectionStateChange(state) => {
                log::info!("Screen leg: ICE {state:?}");
                if state == IceConnectionState::Disconnected {
                    self.waiting_key = true;
                }
            }
            Event::KeyframeRequest(req) => {
                self.keyframe_requests += 1;
                self.last_keyframe_request_ms = Some(wall_ms());
                log::info!("Screen leg: keyframe asked ({:?}) at {}", req.kind, wall_ms());
                keyframe();
            }
            Event::EgressBitrateEstimate(BweKind::Twcc { estimate, .. } | BweKind::Remb { estimate, .. }) => {
                let bps = estimate.as_u64();
                self.estimate_bps = Some(bps);
                let want = (bps * 85 / 100).clamp(MIN_BPS, MAX_BPS);
                let moved = want.abs_diff(self.asked_bps) * 100 / self.asked_bps.max(1) >= 15;
                if moved && now.duration_since(self.asked_at) >= BITRATE_EVERY {
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
            "state": if self.connected_ms.is_some() { "connected" } else if self.socket.is_some() { "connecting" } else { "offered" },
            "local": self.local.map(|a| a.to_string()),
            "connectedMs": self.connected_ms.map(|v| v as u64),
            "firstSentMs": self.first_sent_ms.map(|v| v as u64),
            "framesIn": self.frames_in,
            "framesSent": self.frames_sent,
            "bytesSent": self.bytes_sent,
            "keyframeRequests": self.keyframe_requests,
            "lastKeyframeRequestMs": self.last_keyframe_request_ms.map(|v| v as u64),
            "estimateBps": self.estimate_bps,
            "encoderBps": self.asked_bps,
        })
        .to_string()
    }
}

/// The media server's first UDP candidate in its answer (ICE-lite: its host candidates are in the description).
pub(crate) fn server_address(sdp: &str) -> Option<SocketAddr> {
    sdp.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("a=candidate:")?;
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() < 6 || !parts[2].eq_ignore_ascii_case("udp") {
            return None;
        }
        let ip: IpAddr = parts[4].parse().ok()?;
        let port: u16 = parts[5].parse().ok()?;
        Some(SocketAddr::new(ip, port))
    })
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
