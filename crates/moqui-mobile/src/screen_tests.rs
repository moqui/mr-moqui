// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use moqui_rtc::sfu::{LegKey, Proto, Sfu, SfuConfig, SfuEvent, SfuOutput, Source};
use moqui_rtc::tap::{Kind, MediaTap, TapFrame};
use sframe::frame::MonotonicCounter;
use sframe::CipherSuite;
use str0m::net::Protocol;

use super::*;

/// The tests that start a leg (there is one at a time, as on the phone) take turns.
static ONE_LEG: StdMutex<()> = StdMutex::new(());

#[test]
fn the_servers_candidates_are_taken_from_its_answer() {
    let sdp = "v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n\
        a=candidate:1 1 tcp 2130706431 10.1.0.202 10092 typ host tcptype passive\r\n\
        a=candidate:2 1 udp 2130706431 10.1.0.202 10091 typ host\r\n\
        a=candidate:3 1 udp 2130706431 2001:db8::5 10091 typ host\r\n\
        a=candidate:4 1 tcp 2130706431 10.1.0.202 9 typ host tcptype active\r\n\
        a=candidate:2 1 udp 2130706431 10.1.0.202 10091 typ host\r\n";
    let addr = |s: &str| s.parse::<SocketAddr>().expect("an address");
    assert_eq!(
        server_candidates(sdp),
        vec![
            (Protocol::Tcp, addr("10.1.0.202:10092")),
            (Protocol::Udp, addr("10.1.0.202:10091")),
            (Protocol::Udp, addr("[2001:db8::5]:10091")),
        ]
    );
    assert!(server_candidates("a=candidate:bad").is_empty());
}

#[test]
fn a_vp8_keyframe_is_told_by_its_frame_tag() {
    // RFC 6386 §9.1: bit 0 of the frame tag is 0 for a key frame
    assert!(is_vp8_keyframe(&[0x50, 0x2d, 0x01, 0x9d, 0x01, 0x2a]));
    assert!(!is_vp8_keyframe(&[0x31, 0x02, 0x00]));
    assert!(!is_vp8_keyframe(&[]));
}

#[test]
fn the_encoder_follows_the_estimate_within_its_bounds_and_not_on_every_change() {
    let second = Duration::from_secs(1);
    assert_eq!(encoder_bitrate(2_000_000, 1_200_000, second), Some(1_700_000));
    assert_eq!(encoder_bitrate(9_000_000, 1_200_000, second), Some(MAX_BPS));
    assert_eq!(encoder_bitrate(50_000, 1_200_000, second), Some(MIN_BPS));
    assert_eq!(
        encoder_bitrate(1_500_000, 1_200_000, second),
        None,
        "a change under 15 %"
    );
    assert_eq!(
        encoder_bitrate(2_000_000, 1_200_000, Duration::from_millis(400)),
        None,
        "too soon"
    );
}

#[test]
fn an_offer_is_one_send_only_vp8_video() {
    let _one = ONE_LEG.lock().unwrap_or_else(|e| e.into_inner());
    let sdp = offer(false).expect("an offer");
    stop();
    assert_eq!(sdp.matches("m=video").count(), 1);
    assert_eq!(sdp.matches("m=audio").count(), 0);
    assert!(sdp.contains("a=sendonly"));
    assert!(sdp.contains("VP8/90000"));
    assert!(!sdp.contains("a=candidate"));
}

/// A media server with only an ICE-TCP candidate (UDP blocked): the leg connects over TCP, RFC 4571 framed.
#[test]
fn the_leg_takes_ice_tcp_when_the_media_server_offers_only_that() {
    use std::io::{Read, Write};
    let _one = ONE_LEG.lock().unwrap_or_else(|e| e.into_inner());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a TCP listener");
    let addr = listener.local_addr().expect("its address");
    let mut sfu = Sfu::new(SfuConfig {
        udp_candidates: Vec::new(),
        tcp_candidates: vec![addr],
        ..SfuConfig::default()
    });
    let key = LegKey::new("T", "S");
    let answer_sdp = sfu
        .join_as(Instant::now(), &key, 1, &offer(false).expect("an offer"), Some("P"))
        .expect("joined");
    assert!(server_candidates(&answer_sdp).iter().all(|c| c.0 == Protocol::Tcp));
    answer(answer_sdp).expect("the answer taken");
    let (mut stream, from) = listener.accept().expect("the leg connects");
    stream
        .set_read_timeout(Some(Duration::from_millis(5)))
        .expect("a timeout");
    let mut connected = false;
    let mut pending = Vec::new();
    let mut buf = [0u8; 4096];
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until && !connected {
        if let Ok(n) = stream.read(&mut buf) {
            pending.extend_from_slice(&buf[..n]);
        }
        while pending.len() >= 2 {
            let len = u16::from_be_bytes([pending[0], pending[1]]) as usize;
            if pending.len() < 2 + len {
                break;
            }
            let framed: Vec<u8> = pending.drain(..2 + len).skip(2).collect();
            sfu.handle_receive(Instant::now(), Proto::Tcp, from, addr, &framed);
        }
        sfu.handle_timeout(Instant::now());
        loop {
            match sfu.poll_output() {
                SfuOutput::Transmit(t) => {
                    let len = u16::try_from(t.contents.len()).expect("a frame");
                    let _ = stream.write_all(&len.to_be_bytes());
                    let _ = stream.write_all(&t.contents);
                }
                SfuOutput::Event(SfuEvent::Connected { .. }) => connected = true,
                SfuOutput::Event(_) => {}
                SfuOutput::Timeout(_) => break,
            }
        }
    }
    let status: serde_json::Value = serde_json::from_str(&status()).expect("a status");
    stop();
    assert!(connected, "the leg connected over ICE-TCP: {status}");
}

/// A frame the media server forwards (a tap sees them past the stage's gate): its leg, source, keyframe and bytes.
type Forwarded = (String, Source, bool, Vec<u8>);

struct Seen(Arc<StdMutex<Vec<Forwarded>>>);

impl MediaTap for Seen {
    fn on_frame(&mut self, f: &TapFrame<'_>) {
        if f.kind == Kind::Video {
            self.0
                .lock()
                .expect("seen")
                .push((f.leg.to_string(), f.source, f.keyframe, f.data.to_vec()));
        }
    }
}

/// The encoder's `n`th frame: a VP8 keyframe each 30th (its tag's bit 0 clear), deltas between, its bytes numbered.
fn encoded(n: u32) -> Vec<u8> {
    let mut data: Vec<u8> = (0..1200u32).map(|i| (i ^ n) as u8).collect();
    data[0] = if n.is_multiple_of(30) { 0x50 } else { 0x51 };
    data
}

/// The repository's media server (moqui-rtc's `Sfu`) on a loopback UDP socket, with the leg joined as the page joins it
/// for its page leg `P` and on the stage; `encrypted` as in an end-to-end encrypted call.
struct Server {
    sfu: Sfu,
    socket: UdpSocket,
    addr: SocketAddr,
    seen: Arc<StdMutex<Vec<Forwarded>>>,
    connected: bool,
    screen_track: bool,
    sent: u32,
    next_frame: Instant,
}

impl Server {
    fn join(encrypted: bool) -> Server {
        let socket = UdpSocket::bind("127.0.0.1:0").expect("a media socket");
        socket
            .set_read_timeout(Some(Duration::from_millis(2)))
            .expect("a timeout");
        let addr = socket.local_addr().expect("its address");
        let mut sfu = Sfu::new(SfuConfig {
            udp_candidates: vec![addr],
            ..SfuConfig::default()
        });
        let seen = Arc::new(StdMutex::new(Vec::new()));
        sfu.add_tap("R", Box::new(Seen(Arc::clone(&seen)))).expect("a tap");
        let offered = offer(encrypted).expect("an offer");
        let answer_sdp = sfu
            .join_as(Instant::now(), &LegKey::new("R", "S"), 1, &offered, Some("P"))
            .expect("joined");
        sfu.set_screen_stage("R", Some("S"));
        answer(answer_sdp).expect("the answer taken");
        Server {
            sfu,
            socket,
            addr,
            seen,
            connected: false,
            screen_track: false,
            sent: 0,
            next_frame: Instant::now(),
        }
    }

    /// The media server and the encoder's frames (one each 66 ms) until `done` or `within`.
    fn run(&mut self, within: Duration, done: impl Fn(&Server) -> bool) {
        let mut buf = vec![0u8; 2000];
        let until = Instant::now() + within;
        while Instant::now() < until && !done(self) {
            if let Ok((len, from)) = self.socket.recv_from(&mut buf) {
                self.sfu
                    .handle_receive(Instant::now(), Proto::Udp, from, self.addr, &buf[..len]);
            }
            self.sfu.handle_timeout(Instant::now());
            loop {
                match self.sfu.poll_output() {
                    SfuOutput::Transmit(t) => {
                        let _ = self.socket.send_to(&t.contents, t.destination);
                    }
                    SfuOutput::Event(SfuEvent::Connected { .. }) => self.connected = true,
                    SfuOutput::Event(SfuEvent::Track { key, source, .. }) => {
                        self.screen_track |= &*key.leg == "S" && source == Source::Screen;
                    }
                    SfuOutput::Event(_) => {}
                    SfuOutput::Timeout(_) => break,
                }
            }
            if Instant::now() >= self.next_frame {
                frame(encoded(self.sent), i64::from(self.sent) * 66_666);
                self.sent += 1;
                self.next_frame = Instant::now() + Duration::from_millis(66);
            }
        }
    }

    fn forwarded(&self) -> Vec<Forwarded> {
        self.seen.lock().expect("seen").clone()
    }
}

fn leg_status() -> serde_json::Value {
    serde_json::from_str(&status()).expect("a status")
}

/// The leg against the repository's media server on loopback UDP: it connects, sends the encoder's frames as the screen
/// once the stage names it, and asks the encoder for a keyframe when it connects.
#[test]
fn the_leg_connects_to_the_media_server_and_its_frames_are_the_screen() {
    let _one = ONE_LEG.lock().unwrap_or_else(|e| e.into_inner());
    let mut server = Server::join(false);
    server.run(Duration::from_secs(10), |s| {
        s.connected && s.screen_track && !s.forwarded().is_empty()
    });
    let status = leg_status();
    stop();
    assert!(server.connected, "the leg connected: {status}");
    assert!(server.screen_track, "its video is the screen from the start");
    let seen = server.forwarded();
    assert!(
        !seen.is_empty()
            && seen
                .iter()
                .all(|(leg, source, _, _)| leg == "S" && *source == Source::Screen),
        "the media server forwards the leg's frames as the screen"
    );
    // nothing before a keyframe, and the frames as the encoder made them
    assert!(seen[0].2, "the first frame forwarded is a keyframe");
    assert!(
        (0..300).step_by(30).any(|n| seen[0].3 == encoded(n)),
        "an encoder's keyframe as it made it"
    );
    assert_eq!(status["state"], "connected");
    assert_eq!(status["encrypted"], false);
}

/// An end-to-end encrypted call's leg: nothing leaves before its key; then each frame is SFrame in the pages' layout,
/// which the media server forwards as the screen (it reads the keyframe bit left in the clear) and which decrypts
/// with the leg's key to the encoder's frame; a later key is used once its time comes.
#[test]
fn an_encrypted_legs_frames_wait_for_its_key_and_go_as_the_pages_sframe() {
    use sframe::frame::EncryptedFrameView;
    use sframe::key::DecryptionKey;
    let _one = ONE_LEG.lock().unwrap_or_else(|e| e.into_inner());
    let mut server = Server::join(true);
    server.run(Duration::from_secs(10), |s| s.connected && s.sent > 8);
    let before = leg_status();
    assert!(server.connected, "the leg connected: {before}");
    assert!(server.forwarded().is_empty(), "nothing is sent before the key");
    assert_eq!(before["framesSent"], 0);
    assert!(before["unkeyed"].as_u64().unwrap_or(0) > 0);
    assert!(
        set_key(65537, vec![7u8; 15], Duration::ZERO).is_err(),
        "a base key is 16 bytes"
    );
    let base = [0x11u8; 16];
    set_key(65537, base.to_vec(), Duration::ZERO).expect("the key");
    server.run(Duration::from_secs(10), |s| s.forwarded().len() >= 3);
    let seen = server.forwarded();
    assert!(seen.len() >= 3, "frames are sent once the key came");
    let decrypt = |kid: u64, base: &[u8], data: &[u8]| -> Option<Vec<u8>> {
        let key = DecryptionKey::derive_from(CipherSuite::AesGcm128Sha256, kid, base).ok()?;
        let clear = clear_bytes(data);
        let view = EncryptedFrameView::try_with_meta_data(&data[clear..], &data[..clear]).ok()?;
        if view.header().key_id() != kid {
            return None;
        }
        let mut buffer = Vec::new();
        let out = view.decrypt_into(&key, &mut buffer).ok()?;
        Some([&data[..clear], out.payload()].concat())
    };
    let (_, source, keyframe, data) = &seen[0];
    assert_eq!(*source, Source::Screen);
    assert!(
        *keyframe,
        "the media server reads the keyframe bit the SFrame leaves in the clear"
    );
    let plain = decrypt(65537, &base, data).expect("the page's key decrypts it");
    assert!(
        (0..300).step_by(30).any(|n| plain == encoded(n)),
        "the encoder's keyframe"
    );
    assert!(decrypt(65537, &[0x22u8; 16], data).is_none(), "another key does not");
    // a rotation: the next key is used after its wait, and its frames decrypt with it
    let next = [0x33u8; 16];
    set_key(65538, next.to_vec(), Duration::from_millis(300)).expect("the next key");
    let had = seen.len();
    server.run(Duration::from_secs(10), |s| {
        s.forwarded()
            .iter()
            .skip(had)
            .any(|(_, _, _, d)| decrypt(65538, &next, d).is_some())
    });
    let after = leg_status();
    stop();
    assert_eq!(after["kid"], 65538, "{after}");
    assert!(server
        .forwarded()
        .iter()
        .skip(had)
        .all(|(_, _, _, d)| { decrypt(65537, &base, d).is_some() || decrypt(65538, &next, d).is_some() }));
}

/// The pages' call frames (crates/moqui-rtc/testdata/sframe-call-frames.json, which the webroot spec WebrootSFrame checks
/// the pages' worker against): the leg seals a video frame to the same bytes.
#[test]
fn the_leg_seals_a_frame_as_the_pages_worker_does() {
    const CALL_FRAMES: &str = include_str!("../../moqui-rtc/testdata/sframe-call-frames.json");
    let unhex = |s: &str| -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
            .collect()
    };
    let vectors: serde_json::Value = serde_json::from_str(CALL_FRAMES).expect("the vectors");
    let mut videos = 0;
    for f in vectors["frames"].as_array().expect("frames") {
        if f["kind"] != "video" {
            continue;
        }
        videos += 1;
        let (kid, ctr) = (f["kid"].as_u64().expect("kid"), f["ctr"].as_u64().expect("ctr"));
        let key = derive_key(kid, &unhex(f["base_key"].as_str().expect("key"))).expect("a key");
        let frame = unhex(f["frame"].as_str().expect("frame"));
        assert_eq!(clear_bytes(&frame) as u64, f["clear_bytes"].as_u64().expect("clear"));
        let mut counter = MonotonicCounter::with_start_value(ctr, u64::MAX);
        let sealed = seal(&key, &mut counter, &frame).expect("sealed");
        assert_eq!(
            sealed,
            unhex(f["encrypted"].as_str().expect("encrypted")),
            "kid {kid} ctr {ctr}"
        );
    }
    assert!(videos >= 2, "a keyframe and a delta");
}
