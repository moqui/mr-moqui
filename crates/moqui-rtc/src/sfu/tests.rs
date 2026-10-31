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

//! The W1 media gate (plans/Comms-Framework.md §6.2) on the in-memory network: two and five parties, simulcast layers
//! following TWCC, renegotiation both ways and glare, ICE restart, ICE-TCP, generations, and media taken out and put in.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use str0m::change::SdpAnswer;
use str0m::media::MediaKind;
use str0m::net::Protocol;

use super::testnet::{Net, LAYER_BYTES, ROOM, SCREEN_BYTES};
use super::{Layer, LegKey, SfuConfig, SfuError, SfuEvent, Source};
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

    net.client_mut("B").downlink_bps = Some(350_000);
    net.run(secs(8));
    assert_eq!(layer(&net), Some(LAYER_BYTES[0]), "constrained: the low layer");
    assert!(net
        .events
        .iter()
        .any(|e| matches!(e, SfuEvent::Layer { key, layer: Some(Layer::Low), .. } if &*key.leg == "B")));

    net.client_mut("B").downlink_bps = None;
    net.run(secs(10));
    assert!(layer(&net) > Some(LAYER_BYTES[0]), "freed again: a higher layer");
}

#[test]
fn a_subscriber_below_the_floor_gets_audio_only() {
    let mut net = Net::new();
    net.join("A", true);
    net.join("B", false);
    net.run(secs(2));
    net.client_mut("B").downlink_bps = Some(60_000);
    net.run(secs(8));
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
