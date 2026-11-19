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

//! The W1 media gate (plans/archive/Comms-Framework.md §6.2) on the in-memory network: two and five parties, simulcast layers
//! following TWCC, renegotiation both ways and glare, ICE restart, ICE-TCP, generations, media taken out and put in, and
//! keyframes asked of a room's publishers (the recorder, W5).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use str0m::change::SdpAnswer;
use str0m::media::MediaKind;
use str0m::net::Protocol;

use super::testnet::{Net, LAYER_BYTES, ROOM, SCREEN_BYTES};
use super::{Layer, LegKey, SfuConfig, SfuError, SfuEvent, SfuOutput, Source};
use crate::tap::{Kind, MediaTap, TapFrame};

fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

#[test]
fn two_parties_send_each_other_audio_and_video() {
    let mut net = Net::new();
    net.join("A", false);
    net.join("B", false);
    net.run(secs(2));
    assert!(net.client("A").connected && net.client("B").connected);
    for (at, from) in [("A", "B"), ("B", "A")] {
        assert!(net.received(at, from, MediaKind::Audio) > 20, "{at} hears {from}");
        assert!(net.received(at, from, MediaKind::Video) > 10, "{at} sees {from}");
    }
    assert!(net
        .events
        .iter()
        .any(|e| matches!(e, SfuEvent::Connected { key, .. } if &*key.leg == "B")));
}

#[test]
fn five_parties_each_get_the_other_four() {
    let mut net = Net::new();
    let legs = ["A", "B", "C", "D", "E"];
    for leg in legs {
        net.join(leg, false);
        net.run(Duration::from_millis(200));
    }
    net.run(secs(2));
    for at in legs {
        for from in legs.iter().filter(|f| **f != at) {
            assert!(net.received(at, from, MediaKind::Audio) > 20, "{at} hears {from}");
            assert!(net.received(at, from, MediaKind::Video) > 10, "{at} sees {from}");
        }
    }
    assert_eq!(net.sfu.legs(ROOM).len(), 5);
}

#[test]
fn simulcast_layer_follows_the_bandwidth_estimate() {
    let mut net = Net::new();
    net.join("A", true);
    net.join("B", false);
    net.run(secs(8));
    let layer = |net: &Net| net.client("B").last_video_len.get("A").copied();
    assert_eq!(layer(&net), Some(LAYER_BYTES[2]), "unconstrained: the high layer");

    net.client_mut("B").down.bps = Some(350_000);
    net.run(secs(8));
    assert_eq!(layer(&net), Some(LAYER_BYTES[0]), "constrained: the low layer");
    assert!(net
        .events
        .iter()
        .any(|e| matches!(e, SfuEvent::Layer { key, layer: Some(Layer::Low), .. } if &*key.leg == "B")));

    net.client_mut("B").down.bps = None;
    net.run(secs(10));
    assert!(layer(&net) > Some(LAYER_BYTES[0]), "freed again: a higher layer");
}

#[test]
fn a_subscriber_below_the_floor_gets_audio_only() {
    let mut net = Net::new();
    net.join("A", true);
    net.join("B", false);
    net.run(secs(2));
    net.client_mut("B").down.bps = Some(60_000);
    // the switch to audio only comes about 4 s in; what was queued before it is then paced out at the low estimate
    net.run(secs(10));
    let video = net.received("B", "A", MediaKind::Video);
    let audio = net.received("B", "A", MediaKind::Audio);
    net.run(secs(2));
    assert_eq!(
        net.received("B", "A", MediaKind::Video),
        video,
        "no video below the floor"
    );
    assert!(net.received("B", "A", MediaKind::Audio) > audio, "audio goes on");
    assert!(net
        .events
        .iter()
        .any(|e| matches!(e, SfuEvent::Layer { key, layer: None, .. } if &*key.leg == "B")));
}

#[test]
fn a_subscription_caps_the_layer() {
    let mut net = Net::new();
    net.join("A", true);
    net.join("B", false);
    net.run(secs(2));
    let key = LegKey::new(ROOM, "B");
    net.sfu
        .subscribe(net.now, &key, 1, "A", Some(Layer::Low))
        .expect("subscribed");
    net.run(secs(2));
    assert_eq!(net.client("B").last_video_len.get("A").copied(), Some(LAYER_BYTES[0]));
    net.sfu.subscribe(net.now, &key, 1, "A", None).expect("unsubscribed");
    net.run(Duration::from_millis(300));
    let video = net.received("B", "A", MediaKind::Video);
    net.run(secs(1));
    assert_eq!(
        net.received("B", "A", MediaKind::Video),
        video,
        "no video once unsubscribed"
    );
}

#[test]
fn leaving_stops_the_tracks_on_the_others() {
    let mut net = Net::new();
    net.join("A", false);
    net.join("B", false);
    net.run(secs(1));
    let offers = net
        .events
        .iter()
        .filter(|e| matches!(e, SfuEvent::Offer { key, .. } if &*key.leg == "A"))
        .count();
    net.sfu.leave(net.now, &LegKey::new(ROOM, "B"), 1).expect("left");
    net.run(Duration::from_millis(200));
    let after = net
        .events
        .iter()
        .filter(|e| matches!(e, SfuEvent::Offer { key, .. } if &*key.leg == "A"))
        .count();
    assert!(after > offers, "A is offered the stopped tracks");
    assert!(net
        .events
        .iter()
        .any(|e| matches!(e, SfuEvent::Track { key, open: false, .. } if &*key.leg == "B")));
    assert_eq!(net.sfu.legs(ROOM).len(), 1);
}

#[test]
fn the_sfu_gives_way_when_both_offer_at_once() {
    let mut net = Net::new();
    net.join("A", false);
    net.run(Duration::from_millis(300));
    net.hold_offers = Some("A".into());
    net.join("B", false);
    net.run(Duration::from_millis(300));
    let held = net.held.iter().find_map(|e| match e {
        SfuEvent::Offer { key, offer_id, .. } if &*key.leg == "A" => Some(*offer_id),
        _ => None,
    });
    let held = held.unwrap_or_else(|| panic!("A was offered B's tracks: {:?}", net.events));
    net.held.clear();
    net.hold_offers = None;

    let key = LegKey::new(ROOM, "A");
    let offer = net.client_offer("A");
    let answer = net
        .sfu
        .handle_offer(net.now, &key, 1, &offer)
        .expect("A's offer is answered");
    net.client_accept_answer("A", &answer);
    // An answer to the dropped offer is ignored.
    let stale = SdpAnswer::from_sdp_string(&answer).expect("answer").to_sdp_string();
    assert_eq!(net.sfu.handle_answer(net.now, &key, 1, held, &stale), Ok(()));
    net.pump();
    net.run(secs(2));
    assert!(
        net.received("A", "B", MediaKind::Video) > 10,
        "B's tracks are offered again and flow"
    );
}

#[test]
fn ice_restart_keeps_the_media() {
    let mut net = Net::new();
    net.join("A", false);
    net.join("B", false);
    net.run(secs(1));
    let c = net.client_mut("A");
    let mut change = c.rtc.sdp_api();
    change.ice_restart(false);
    let (offer, pending) = change.apply().expect("an offer");
    let answer = net
        .sfu
        .handle_offer(net.now, &LegKey::new(ROOM, "A"), 1, &offer.to_sdp_string())
        .expect("restart");
    let c = net.client_mut("A");
    c.rtc
        .sdp_api()
        .accept_answer(pending, SdpAnswer::from_sdp_string(&answer).expect("answer"))
        .expect("accepted");
    let before = net.received("B", "A", MediaKind::Audio);
    net.run(secs(2));
    assert!(net.received("B", "A", MediaKind::Audio) > before + 20);
}

#[test]
fn ice_tcp_reaches_a_client_without_udp() {
    let mut net = Net::new();
    net.join("A", false);
    net.join_with("T", false, Protocol::Tcp);
    net.run(secs(2));
    assert!(net.client("T").connected);
    assert!(net.received("T", "A", MediaKind::Audio) > 20);
    assert!(net.received("A", "T", MediaKind::Audio) > 20);
}

#[test]
fn an_older_generation_is_refused_and_a_newer_replaces() {
    let mut net = Net::new();
    net.join("A", false);
    net.join("B", false);
    net.run(secs(1));
    let key = LegKey::new(ROOM, "A");
    assert_eq!(
        net.sfu.join(net.now, &key, 1, "v=0").err(),
        Some(SfuError::StaleGeneration)
    );
    assert_eq!(
        net.sfu.subscribe(net.now, &key, 2, "B", None).err(),
        Some(SfuError::StaleGeneration)
    );
    assert_eq!(
        net.sfu.leave(net.now, &LegKey::new(ROOM, "Z"), 1).err(),
        Some(SfuError::UnknownLeg)
    );
    assert!(matches!(
        net.sfu.join(net.now, &key, 2, "not sdp"),
        Err(SfuError::BadSdp(_))
    ));
    assert_eq!(net.sfu.generation(&key), Some(1), "a bad offer leaves the peer");
}

#[test]
fn a_room_holds_its_limit() {
    let mut net = Net::new();
    net.sfu = super::Sfu::new(SfuConfig {
        max_legs_per_room: 1,
        udp_candidates: vec![net.sfu_addr],
        ..Default::default()
    });
    net.join("A", false);
    let offer = "v=0\r\no=- 1 1 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\n";
    let r = net.sfu.join(net.now, &LegKey::new(ROOM, "B"), 1, offer);
    assert_eq!(r.err(), Some(SfuError::RoomFull));
}

type Seen = Arc<Mutex<Vec<(String, Kind, &'static str, Source)>>>;

struct Collect(Seen);

impl MediaTap for Collect {
    fn on_frame(&mut self, f: &TapFrame<'_>) {
        let codec = if f.codec == "opus" { "opus" } else { "vp8" };
        if let Ok(mut v) = self.0.lock() {
            v.push((f.leg.to_string(), f.kind, codec, f.source));
        }
    }
}

#[test]
fn media_is_taken_out_and_put_in() {
    let mut net = Net::new();
    net.join("A", false);
    net.join("B", false);
    net.run(secs(1));
    let seen = Arc::new(Mutex::new(Vec::new()));
    net.sfu.add_tap(ROOM, Box::new(Collect(seen.clone()))).expect("tapped");
    let key = LegKey::new(ROOM, "A");
    let video_mid = net.client("A").video_mid.expect("a video mid").to_string();
    net.sfu
        .set_source(&key, 1, &video_mid, Source::Screen)
        .expect("declared");
    net.run(secs(1));
    {
        let seen = seen.lock().expect("frames");
        assert!(seen
            .iter()
            .any(|(leg, kind, codec, _)| leg == "A" && *kind == Kind::Audio && *codec == "opus"));
        assert!(seen
            .iter()
            .any(|(leg, kind, _, source)| leg == "A" && *kind == Kind::Video && *source == Source::Screen));
        assert!(seen.iter().any(|(leg, _, _, _)| leg == "B"));
    }

    net.sfu.add_injected_source(net.now, ROOM, "PH1").expect("a phone leg");
    net.run(Duration::from_millis(500));
    for n in 0..50u64 {
        let now = net.now;
        net.sfu
            .inject(now, ROOM, "PH1", n * 960, b"phone-audio")
            .expect("injected");
        net.run(Duration::from_millis(20));
    }
    for at in ["A", "B"] {
        assert!(net.received(at, "PH1", MediaKind::Audio) > 30, "{at} hears the phone");
        assert_eq!(
            net.client(at).last_audio.get("PH1").map(|v| v.as_slice()),
            Some(&b"phone-audio"[..])
        );
    }
}

#[test]
fn the_loudest_leg_becomes_the_active_speaker() {
    let mut net = Net::new();
    net.join("A", false);
    net.join("B", false);
    net.join("C", false);
    net.client_mut("A").audio_level = -10;
    net.client_mut("B").audio_level = -90;
    net.client_mut("C").audio_level = -100;
    net.run(secs(2));
    let speakers = |net: &Net| -> Vec<String> {
        net.events
            .iter()
            .filter_map(|e| match e {
                SfuEvent::ActiveSpeaker { leg, .. } => Some(leg.to_string()),
                _ => None,
            })
            .collect()
    };
    assert_eq!(speakers(&net).last().map(String::as_str), Some("A"));
    // B speaks up, A goes quiet
    net.client_mut("A").audio_level = -90;
    net.client_mut("B").audio_level = -5;
    net.run(secs(2));
    assert_eq!(speakers(&net).last().map(String::as_str), Some("B"));
    // nobody above the floor: the speaker stays
    net.client_mut("B").audio_level = -100;
    let before = speakers(&net).len();
    net.run(secs(1));
    assert_eq!(speakers(&net).len(), before);
}

#[test]
fn a_screen_share_opens_as_its_own_track_and_ends_on_the_others() {
    let mut net = Net::new();
    net.join("A", false);
    net.join("B", false);
    net.run(secs(1));
    let mid = net.share_screen("A");
    net.run(secs(2));
    assert!(net.events.iter().any(
        |e| matches!(e, SfuEvent::Track { key, source: Source::Screen, open: true, mid: m, .. }
        if &*key.leg == "A" && *m == mid.to_string())
    ));
    assert_eq!(
        net.client("B")
            .last_video_len
            .get("A")
            .copied()
            .map(|n| n == SCREEN_BYTES || n > 0),
        Some(true)
    );
    let offers = net
        .events
        .iter()
        .filter(|e| matches!(e, SfuEvent::Offer { key, .. } if &*key.leg == "B"))
        .count();
    net.stop_screen("A");
    net.run(secs(1));
    assert!(net
        .events
        .iter()
        .any(|e| matches!(e, SfuEvent::Track { key, open: false, mid: m, .. }
        if &*key.leg == "A" && *m == mid.to_string())));
    let after = net
        .events
        .iter()
        .filter(|e| matches!(e, SfuEvent::Offer { key, .. } if &*key.leg == "B"))
        .count();
    assert!(after > offers, "B is offered the stopped share");
    // the camera goes on
    let video = net.received("B", "A", MediaKind::Video);
    net.run(secs(1));
    assert!(net.received("B", "A", MediaKind::Video) > video);
}

#[test]
fn a_room_s_publishers_are_asked_for_keyframes() {
    let mut net = Net::new();
    // one without simulcast, one with: each layer is asked
    net.join("A", false);
    net.join("B", true);
    net.run(secs(2));
    let before = |net: &Net| [net.client("A").keyframe_requests, net.client("B").keyframe_requests];
    let start = before(&net);
    net.run(secs(1));
    assert_eq!(before(&net), start, "nobody asked in a call that goes well");
    let now = net.now;
    net.sfu.request_keyframes(now, ROOM);
    net.sfu.request_keyframes(now, "another room");
    net.run(Duration::from_millis(300));
    let after = before(&net);
    assert!(after[0] > start[0] && after[1] > start[1], "{start:?} then {after:?}");
}

/// HL-008 measurement, opt-in: independent two-party rooms on one node at the same traffic per room (audio every
/// 20 ms and video every 34 ms each way), with the work per received datagram: peers asked whether they accept it,
/// peers polled by the all-peer drive, and the SFU's time per datagram. Linear growth in the per-datagram counts with
/// the number of rooms is the node-wide work the finding describes. Run it with
/// `cargo test -p moqui-rtc --lib sfu_work_per_packet_by_rooms -- --ignored --nocapture` (times are
/// of whatever profile it is built in; the counts do not depend on it).
#[test]
#[ignore = "measurement: prints SFU work per packet as independent rooms grow"]
fn sfu_work_per_packet_by_rooms() {
    for rooms in [1, 4, 16, 48] {
        let mut net = Net::new();
        for room in 0..rooms {
            net.join_room(&format!("R{room}"), &format!("A{room}"));
            net.join_room(&format!("R{room}"), &format!("B{room}"));
        }
        net.run(secs(2));
        let connected = net.clients.iter().filter(|c| c.connected).count();
        let frames = |net: &Net| -> usize { net.clients.iter().map(|c| c.received.values().sum::<usize>()).sum() };
        let (stats, frames_before) = (net.sfu.stats(), frames(&net));
        let (receive_time, receive_polls, timeout_time) = (net.receive_time, net.receive_polls, net.timeout_time);
        net.run(secs(3));
        let after = net.sfu.stats();
        let received = (after.received - stats.received).max(1);
        let accepted = after.accepted - stats.accepted;
        let checks = after.accept_checks - stats.accept_checks;
        let polls = after.peer_polls - stats.peer_polls;
        let drives = after.drives - stats.drives;
        let receive_micros = (net.receive_time - receive_time).as_secs_f64() * 1e6;
        let timeout_micros = (net.timeout_time - timeout_time).as_secs_f64() * 1e6;
        let receive_polls = net.receive_polls - receive_polls;
        let frames_per_room = (frames(&net) - frames_before) as f64 / rooms as f64;
        println!(
            "{rooms:>3} rooms ({connected}/{} connected): {received} datagrams in ({accepted} accepted); per datagram \
             {:.1} accept checks, {:.1} peer polls in its receive ({:.1} with timeouts), {:.2} drives, \
             {:.1} us in receive, {:.1} us in timeouts; {frames_per_room:.0} frames delivered per room in 3 s",
            rooms * 2,
            checks as f64 / received as f64,
            receive_polls as f64 / received as f64,
            polls as f64 / received as f64,
            drives as f64 / received as f64,
            receive_micros / received as f64,
            timeout_micros / received as f64,
        );
    }
}

/// HL-008: a datagram drives the peers it reaches, not every peer of the node: with one busy room and ten idle ones,
/// the polls per datagram received are those of the busy room.
#[test]
fn an_idle_rooms_peers_are_not_polled_by_another_rooms_traffic() {
    let mut net = Net::new();
    for room in 0..11 {
        net.join_room(&format!("R{room}"), &format!("A{room}"));
        net.join_room(&format!("R{room}"), &format!("B{room}"));
    }
    net.run(secs(2));
    assert!(net.clients.iter().all(|c| c.connected));
    // the other rooms stop sending media
    for c in net.clients.iter_mut().filter(|c| &*c.key.room != "R0") {
        c.audio_mid = None;
        c.video_mid = None;
    }
    net.run(secs(1));
    let (received, polls) = (net.sfu.stats().received, net.receive_polls);
    net.run(secs(2));
    let received = net.sfu.stats().received - received;
    let polls = net.receive_polls - polls;
    let per_datagram = polls as f64 / received as f64;
    // two peers in the busy room, a few passes; every peer of the node would be 22 a pass
    assert!(per_datagram < 6.0, "{per_datagram:.1} peer polls per datagram");
    assert!(
        net.received("B0", "A0", MediaKind::Video) > 50,
        "the busy room's media flows"
    );
}

/// HL-008: a peer's own timer fires with no datagram: its deadline is kept, and a timeout before it polls no one.
#[test]
fn a_peers_timer_fires_with_no_traffic() {
    let mut net = Net::new();
    net.join("A", false);
    net.run(secs(1));
    let deadline = loop {
        if let SfuOutput::Timeout(t) = net.sfu.poll_output() {
            break t;
        }
    };
    let polls = net.sfu.stats().peer_polls;
    let received = net.sfu.stats().received;
    if deadline > net.now + Duration::from_millis(1) {
        net.sfu.handle_timeout(net.now);
        assert_eq!(net.sfu.stats().peer_polls, polls, "nothing is due yet");
    }
    net.sfu.handle_timeout(deadline);
    assert!(net.sfu.stats().peer_polls > polls, "the due peer was polled");
    assert_eq!(net.sfu.stats().received, received, "with no datagram");
    assert!(matches!(
        net.sfu.poll_output(),
        SfuOutput::Transmit(_) | SfuOutput::Event(_) | SfuOutput::Timeout(_)
    ));
}

/// HL-008: the address index is a first guess. When the peer it names does not take a datagram (the leg moved, an
/// ICE restart), every peer is asked, the one that takes it is found, and the index follows it; a peer that leaves is
/// dropped from it.
#[test]
fn the_address_index_follows_a_moved_peer_and_forgets_one_that_left() {
    let mut net = Net::new();
    net.join("A", false);
    net.join("B", false);
    net.run(secs(2));
    let id = |net: &Net, leg: &str| net.sfu.by_leg[&LegKey::new(ROOM, leg)];
    let (a, b) = (id(&net, "A"), id(&net, "B"));
    assert!(net.sfu.by_addr.values().any(|peer| *peer == a));
    // the index points A's addresses at B, as after A moved to addresses B had
    for peer in net.sfu.by_addr.values_mut() {
        if *peer == a {
            *peer = b;
        }
    }
    let video = net.received("B", "A", MediaKind::Video);
    net.run(secs(1));
    assert!(
        net.received("B", "A", MediaKind::Video) > video + 10,
        "A's media still goes through"
    );
    assert!(
        net.sfu.by_addr.values().any(|peer| *peer == a),
        "the index found A again"
    );
    let now = net.now;
    net.sfu.leave(now, &LegKey::new(ROOM, "A"), 1).unwrap();
    assert!(
        net.sfu.by_addr.values().all(|peer| *peer != a),
        "A is gone from the index"
    );
    assert!(!net.sfu.deadline_of.contains_key(&a) && !net.sfu.pending.contains(&a));
}

/// The security proof of OWASP ASVS 5.0.0 v5.0.0-17.2.3 (docs/security/proofs/V17-WebRTC.md), in-process: the media
/// server checks SRTP authentication, so an RTP packet that was not sealed with a peer's keys is not forwarded. On the
/// in-memory network with two browsers in a call, a packet of A's audio held back on its way to the SFU and sent on with
/// one bit of its payload flipped is dropped there (str0m's `SrtpContext::unprotect_rtp` fails its tag), and so is one
/// with that packet's header and a payload and tag of the attacker's: B gets no frame for that packet. Another held
/// packet sent on with a bit flipped and then unchanged is forwarded once, and B goes on hearing A after both. A real
/// packet of A's from an address no peer's ICE session has is not taken (`handle_receive` false). The SIP legs' own SRTP
/// (moqui-sip `srtp.rs`) refuses a changed packet and a replay:
/// `packets_round_trip_across_a_rollover_and_tampering_and_replays_are_refused`.
#[test]
fn rtp_that_fails_its_srtp_authentication_is_dropped_at_the_sfu() {
    use super::Proto;

    let mut net = Net::new();
    net.join("A", false);
    net.join("B", false);
    net.client_mut("B").keep_audio = true;
    net.run(secs(1));
    assert!(net.client("A").connected && net.client("B").connected);
    let pt = net.opus_pt("A");
    let header_len = |packet: &[u8]| {
        let mut len = 12 + 4 * usize::from(packet[0] & 0x0f);
        if packet[0] & 0x10 != 0 {
            len += 4 + 4 * usize::from(u16::from_be_bytes([packet[len + 2], packet[len + 3]]));
        }
        len
    };
    let hold = |net: &mut Net| loop {
        if let Some(held) = net.take_rtp_to_sfu("A", pt) {
            break held;
        }
        net.run(Duration::from_millis(2));
    };
    let send = |net: &mut Net, source, packet: &[u8]| {
        let (now, to) = (net.now, net.sfu_addr);
        net.sfu.handle_receive(now, Proto::Udp, source, to, packet)
    };

    // one packet changed and one forged in its place; the packet itself never arrives
    let (source, dropped) = hold(&mut net);
    let mut changed = dropped.clone();
    let last_payload_byte = changed.len() - 17;
    changed[last_payload_byte] ^= 0x01;
    assert!(send(&mut net, source, &changed), "A's ICE session takes it");
    let mut forged = dropped[..header_len(&dropped)].to_vec();
    forged.extend_from_slice(b"attacker audio");
    forged.extend_from_slice(&[0x5a; 16]);
    assert!(send(&mut net, source, &forged));
    net.run(Duration::from_millis(100));

    // another changed, then sent on as it was
    let (source, kept) = hold(&mut net);
    let mut changed = kept.clone();
    let last_payload_byte = changed.len() - 17;
    changed[last_payload_byte] ^= 0x01;
    assert!(send(&mut net, source, &changed));
    assert!(send(&mut net, source, &kept));
    net.run(Duration::from_millis(100));

    // a real packet from another address, then from A's
    let (source, real) = hold(&mut net);
    let stranger: std::net::SocketAddr = "10.66.0.9:40000".parse().unwrap();
    assert!(
        !send(&mut net, stranger, &real),
        "no peer's ICE session has that address"
    );
    assert!(send(&mut net, source, &real));
    net.run(Duration::from_millis(500));

    // B's frames from A, by RTP time: one frame step (960) apart but for the one packet that never arrived
    let mut times: Vec<u64> = net.client("B").audio_frames["A"]
        .iter()
        .map(|(time, _)| *time)
        .collect();
    times.sort_unstable();
    let steps: Vec<u64> = times.windows(2).map(|pair| pair[1] - pair[0]).collect();
    assert_eq!(
        steps.iter().filter(|step| **step == 1920).count(),
        1,
        "only the dropped packet's frame is missing: {steps:?}"
    );
    assert!(steps.iter().all(|step| *step == 960 || *step == 1920), "{steps:?}");
    assert!(
        net.client("B").audio_frames["A"]
            .iter()
            .all(|(_, payload)| payload.as_slice() == b"audio:A"),
        "no frame of the attacker's"
    );
    let after = net.client("B").audio_in["A"].last().map(|(at, _)| *at).unwrap();
    assert!(net.now - after < Duration::from_millis(100), "B still hears A");
}

/// The extensions of a DTLS 1.2 ClientHello or ServerHello `body` from `at` (the extensions' length), by type.
fn hello_extensions(body: &[u8], mut at: usize) -> Vec<(u16, Vec<u8>)> {
    let read16 = |at: usize| usize::from(u16::from_be_bytes([body[at], body[at + 1]]));
    let end = at + 2 + read16(at);
    at += 2;
    let mut found = Vec::new();
    while at + 4 <= end {
        let (kind, len) = (read16(at) as u16, read16(at + 2));
        found.push((kind, body[at + 4..at + 4 + len].to_vec()));
        at += 4 + len;
    }
    found
}

/// A DTLS 1.2 ClientHello record offering `suites` and, in `use_srtp`, `profiles`, with the extensions WebRTC needs
/// (an EC group, EC point formats, ECDSA and RSA signatures, the extended master secret).
fn client_hello(suites: &[u16], profiles: &[u16]) -> Vec<u8> {
    let mut extensions = Vec::new();
    let mut extension = |kind: u16, data: &[u8]| {
        extensions.extend_from_slice(&kind.to_be_bytes());
        extensions.extend_from_slice(&(data.len() as u16).to_be_bytes());
        extensions.extend_from_slice(data);
    };
    extension(0x000a, &[0x00, 0x04, 0x00, 0x1d, 0x00, 0x17]);
    extension(0x000b, &[0x01, 0x00]);
    extension(0x000d, &[0x00, 0x06, 0x04, 0x03, 0x08, 0x04, 0x04, 0x01]);
    let mut srtp = ((profiles.len() * 2) as u16).to_be_bytes().to_vec();
    for profile in profiles {
        srtp.extend_from_slice(&profile.to_be_bytes());
    }
    srtp.push(0);
    extension(0x000e, &srtp);
    extension(0x0017, &[]);
    let mut body = vec![0xfe, 0xfd];
    body.extend_from_slice(&[0x42; 32]);
    body.extend_from_slice(&[0, 0]); // no session id, no cookie
    body.extend_from_slice(&((suites.len() * 2) as u16).to_be_bytes());
    for suite in suites {
        body.extend_from_slice(&suite.to_be_bytes());
    }
    body.extend_from_slice(&[1, 0]);
    body.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
    body.extend_from_slice(&extensions);
    let length = (body.len() as u32).to_be_bytes();
    let mut handshake = vec![1, length[1], length[2], length[3], 0, 0, 0, 0, 0];
    handshake.extend_from_slice(&length[1..]);
    handshake.extend_from_slice(&body);
    let mut record = vec![22, 0xfe, 0xff, 0, 0, 0, 0, 0, 0, 0, 0];
    record.extend_from_slice(&(handshake.len() as u16).to_be_bytes());
    record.extend_from_slice(&handshake);
    record
}

/// The security proof of OWASP ASVS 5.0.0 v5.0.0-17.2.2 (docs/security/proofs/V17-WebRTC.md), in-process: the media
/// server's DTLS and SRTP take only strong suites. The SFU builds each peer with str0m's defaults (no crypto provider
/// or DTLS version of its own, `sfu/mod.rs`), so DTLS 1.2 on str0m's aws-lc-rs provider (dimpl). In the client role its
/// ClientHello offers only ECDHE-ECDSA with AES-GCM or ChaCha20-Poly1305 (forward secret, AEAD), asks for the extended
/// master secret, and offers the SRTP profiles AEAD_AES_256_GCM, AEAD_AES_128_GCM and AES128_CM_HMAC_SHA1_80 only. In
/// the server role it answers no ClientHello that offers only weak suites (RSA key exchange, CBC, a null cipher, the
/// PSK suite with its short tag), while one that also offers a strong suite gets a ServerHello choosing it; the SRTP
/// profile it picks is the strongest offered, AEAD_AES_256_GCM, and an offer of only the weak profiles (the 32-bit tag
/// and the null ciphers) gets none. Among the strong DTLS suites the client's order decides (dimpl picks by it).
#[test]
fn the_media_servers_dtls_and_srtp_take_only_strong_suites() {
    use str0m::crypto::dtls::{DtlsInstance, DtlsOutput};

    const STRONG_SUITES: [u16; 3] = [0xc02b, 0xc02c, 0xcca9];
    const STRONG_PROFILES: [u16; 3] = [0x0008, 0x0007, 0x0001];
    let provider = str0m::crypto::from_feature_flags();
    let cert = provider
        .dtls_provider
        .generate_certificate()
        .expect("a DTLS certificate");
    let version = str0m::Rtc::builder().dtls_version();
    let now = std::time::Instant::now();
    let instance = |active: bool| {
        let mut dtls = provider
            .dtls_provider
            .new_dtls(&cert, now, version, None)
            .expect("a DTLS instance");
        dtls.set_active(active);
        dtls.handle_timeout(now).expect("its first timeout");
        dtls
    };
    let output = |dtls: &mut Box<dyn DtlsInstance>| {
        let mut packets = Vec::new();
        let mut buffer = vec![0_u8; 4096];
        loop {
            match dtls.poll_output(&mut buffer) {
                DtlsOutput::Packet(packet) => packets.push(packet.to_vec()),
                DtlsOutput::Timeout(_) => break,
                _ => {}
            }
        }
        packets
    };
    let read16 = |bytes: &[u8], at: usize| u16::from_be_bytes([bytes[at], bytes[at + 1]]);

    // the client role: what its ClientHello offers
    let mut client = instance(true);
    let hello = output(&mut client).into_iter().next().expect("a ClientHello");
    assert_eq!(
        (hello[0], hello[13]),
        (22, 1),
        "a handshake record holding a ClientHello"
    );
    let body = &hello[25..];
    let mut at = 2 + 32;
    at += 1 + usize::from(body[at]); // session id
    at += 1 + usize::from(body[at]); // cookie
    let suites: Vec<u16> = (0..usize::from(read16(body, at)) / 2)
        .map(|n| read16(body, at + 2 + 2 * n))
        .collect();
    at += 2 + usize::from(read16(body, at));
    at += 1 + usize::from(body[at]); // compression methods
    let extensions = hello_extensions(body, at);
    assert!(
        !suites.is_empty() && suites.iter().all(|suite| STRONG_SUITES.contains(suite)),
        "{suites:04x?}"
    );
    let srtp = &extensions.iter().find(|(kind, _)| *kind == 0x000e).expect("use_srtp").1;
    let profiles: Vec<u16> = (0..usize::from(read16(srtp, 0)) / 2)
        .map(|n| read16(srtp, 2 + 2 * n))
        .collect();
    assert_eq!(profiles, STRONG_PROFILES, "{profiles:04x?}");
    assert!(
        extensions.iter().any(|(kind, _)| *kind == 0x0017),
        "the extended master secret"
    );

    // the server role: what it answers
    let answer = |suites: &[u16], profiles: &[u16]| {
        let mut server = instance(false);
        let taken = server.handle_packet(&client_hello(suites, profiles)).is_ok();
        let _ = server.handle_timeout(now);
        let packets = if taken { output(&mut server) } else { Vec::new() };
        packets
            .into_iter()
            .find(|packet| packet.len() > 25 && packet[0] == 22 && packet[13] == 2)
            .map(|hello| {
                let body = hello[25..].to_vec();
                let at = 2 + 32;
                let at = at + 1 + usize::from(body[at]);
                let suite = read16(&body, at);
                let profile = hello_extensions(&body, at + 3)
                    .into_iter()
                    .find(|(kind, _)| *kind == 0x000e)
                    .map(|(_, data)| read16(&data, 2));
                (suite, profile)
            })
    };
    let weak_suites = [0x002f, 0x009c, 0xc009, 0xc006, 0xc0a8];
    assert_eq!(
        answer(&weak_suites, &STRONG_PROFILES),
        None,
        "weak suites only: no ServerHello"
    );
    let mut offered = weak_suites.to_vec();
    offered.push(0xc02b);
    assert_eq!(
        answer(&offered, &[0x0001, 0x0007, 0x0008]),
        Some((0xc02b, Some(0x0008))),
        "the strong suite, and the strongest SRTP profile offered"
    );
    let chosen = answer(&[0xc02b], &[0x0002, 0x0005, 0x0006]);
    assert!(
        matches!(chosen, None | Some((_, None))),
        "weak SRTP profiles only: none is chosen ({chosen:04x?})"
    );
}
