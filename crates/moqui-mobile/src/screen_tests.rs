// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use moqui_rtc::sfu::{LegKey, Proto, Sfu, SfuConfig, SfuEvent, SfuOutput, Source};
use moqui_rtc::tap::{Kind, MediaTap, TapFrame};
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
    let sdp = offer().expect("an offer");
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
        .join_as(Instant::now(), &key, 1, &offer().expect("an offer"), Some("P"))
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

/// The frames the media server forwards (a tap sees them past the stage's gate).
struct Seen(Arc<StdMutex<Vec<(String, Source)>>>);

impl MediaTap for Seen {
    fn on_frame(&mut self, f: &TapFrame<'_>) {
        if f.kind == Kind::Video {
            self.0.lock().expect("seen").push((f.leg.to_string(), f.source));
        }
    }
}

/// The leg against the repository's media server (moqui-rtc's `Sfu`) on loopback UDP, as the page joins it for its
/// page leg: it connects, sends the encoder's frames as the screen once the stage names it, and the server's keyframe
/// request reaches the encoder's side.
#[test]
fn the_leg_connects_to_the_media_server_and_its_frames_are_the_screen() {
    let _one = ONE_LEG.lock().unwrap_or_else(|e| e.into_inner());
    let socket = UdpSocket::bind("127.0.0.1:0").expect("a media socket");
    socket
        .set_read_timeout(Some(Duration::from_millis(2)))
        .expect("a timeout");
    let addr = socket.local_addr().expect("its address");
    let mut sfu = Sfu::new(SfuConfig {
        udp_candidates: vec![addr],
        ..SfuConfig::default()
    });
    let key = LegKey::new("R", "S");
    let seen = Arc::new(StdMutex::new(Vec::new()));
    sfu.add_tap("R", Box::new(Seen(Arc::clone(&seen)))).expect("a tap");
    let offered = offer().expect("an offer");
    let answer_sdp = sfu
        .join_as(Instant::now(), &key, 1, &offered, Some("P"))
        .expect("joined");
    sfu.set_screen_stage("R", Some("S"));
    answer(answer_sdp).expect("the answer taken");

    let mut connected = false;
    let mut screen_track = false;
    let mut buf = vec![0u8; 2000];
    let until = Instant::now() + Duration::from_secs(10);
    let mut next_frame = Instant::now();
    let mut n = 0u32;
    while Instant::now() < until && !(connected && screen_track && !seen.lock().expect("seen").is_empty()) {
        if let Ok((len, from)) = socket.recv_from(&mut buf) {
            sfu.handle_receive(Instant::now(), Proto::Udp, from, addr, &buf[..len]);
        }
        sfu.handle_timeout(Instant::now());
        loop {
            match sfu.poll_output() {
                SfuOutput::Transmit(t) => {
                    let _ = socket.send_to(&t.contents, t.destination);
                }
                SfuOutput::Event(SfuEvent::Connected { .. }) => connected = true,
                SfuOutput::Event(SfuEvent::Track { key, source, .. }) => {
                    screen_track |= &*key.leg == "S" && source == Source::Screen;
                }
                SfuOutput::Event(_) => {}
                SfuOutput::Timeout(_) => break,
            }
        }
        // the encoder's frames: a keyframe each 30th, deltas between
        if Instant::now() >= next_frame {
            let mut data = vec![0u8; 1200];
            data[0] = if n.is_multiple_of(30) { 0x50 } else { 0x51 };
            frame(data, i64::from(n) * 66_666);
            n += 1;
            next_frame = Instant::now() + Duration::from_millis(66);
        }
    }
    let status: serde_json::Value = serde_json::from_str(&status()).expect("a status");
    stop();
    assert!(connected, "the leg connected: {status}");
    assert!(screen_track, "its video is the screen from the start");
    let seen = seen.lock().expect("seen");
    assert!(
        seen.iter().all(|(leg, source)| leg == "S" && *source == Source::Screen) && !seen.is_empty(),
        "the media server forwards the leg's frames as the screen: {seen:?}"
    );
    assert_eq!(status["state"], "connected");
    assert!(status["framesSent"].as_u64().unwrap_or(0) > 0);
    // the leg asks the encoder for a keyframe when it connects and sends nothing before one
    assert!(status["bytesSent"].as_u64().unwrap_or(0) >= 1200);
}
