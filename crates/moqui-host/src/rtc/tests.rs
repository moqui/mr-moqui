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

use std::collections::HashMap;

use super::media_config_from;

fn config(pairs: &[(&str, &str)]) -> Result<Option<super::MediaConfig>, String> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    media_config_from(&|name: &str| map.get(name).cloned().unwrap_or_default())
}

#[test]
fn media_is_off_without_a_port() {
    assert_eq!(config(&[]), Ok(None));
    assert_eq!(config(&[("rtc_media_port", "  ")]), Ok(None));
}

#[test]
fn media_binds_loopback_by_default_and_advertises_its_bind() {
    let c = config(&[("rtc_media_port", "10000")]).expect("valid").expect("on");
    assert_eq!(c.bind.to_string(), "127.0.0.1:10000");
    assert_eq!(c.public, c.bind);
}

#[test]
fn a_public_address_is_an_ip_or_an_ip_and_port() {
    let c = config(&[
        ("rtc_media_port", "10000"),
        ("rtc_media_host", "0.0.0.0"),
        ("rtc_public_address", "203.0.113.5"),
    ])
    .expect("valid")
    .expect("on");
    assert_eq!(c.bind.to_string(), "0.0.0.0:10000");
    assert_eq!(c.public.to_string(), "203.0.113.5:10000");
    let c = config(&[("rtc_media_port", "10000"), ("rtc_public_address", "203.0.113.5:443")])
        .expect("valid")
        .expect("on");
    assert_eq!(c.public.to_string(), "203.0.113.5:443");
}

#[test]
fn a_bad_port_or_address_is_an_error() {
    assert!(config(&[("rtc_media_port", "media")]).is_err());
    assert!(config(&[("rtc_media_port", "10000"), ("rtc_public_address", "not an address")]).is_err());
}

#[test]
fn a_room_keeps_one_notice_of_each_kind_for_its_joiners() {
    use super::driver::RoomNotices;
    use super::NoticeKind;
    use serde_json::json;

    let mut notices = RoomNotices::default();
    assert!(notices.of("call-1").is_empty());
    // captions on first, then a recording: a joiner gets both, the recording's first
    let captions = json!({"rtc": "captions", "state": "on", "provider": "whisper"});
    notices.set("call-1", NoticeKind::Captions, Some(captions.clone()));
    let recording = json!({"rtc": "recording", "state": "recording", "callRecordingId": "R1"});
    notices.set("call-1", NoticeKind::Recording, Some(recording));
    // a newer notice of a kind takes the place of the older
    let paused = json!({"rtc": "recording", "state": "paused", "callRecordingId": "R1"});
    notices.set("call-1", NoticeKind::Recording, Some(paused.clone()));
    assert_eq!(notices.of("call-1"), vec![paused.clone(), captions]);
    assert!(notices.of("call-2").is_empty(), "another room has none");
    // captions off: the recording's stays
    notices.set("call-1", NoticeKind::Captions, None);
    assert_eq!(notices.of("call-1"), vec![paused]);
    notices.set(
        "call-1",
        NoticeKind::Captions,
        Some(json!({"rtc": "captions", "state": "on"})),
    );
    notices.room_closed("call-1");
    assert!(notices.of("call-1").is_empty());
}

#[test]
fn a_cascaded_room_goes_between_media_nodes_by_its_whole_key() {
    use super::driver::{relay_body, room_frame_body};
    use super::NoticeKind;
    use serde_json::json;

    // a paired node's room (E12): the bare id would be the server's own call 100 on the other media node
    let room = moqui_core::edge::media::room_key(Some("local-1"), "100");
    let relay = relay_body("media-a", "media-b", &room, json!({"offer": 1}));
    assert_eq!(relay["callSessionId"], room.as_str());
    assert_eq!(relay["kind"], "relay");
    assert_eq!(relay["target"], "media-b");
    assert_eq!(relay["from"], "media-a");
    let frame = room_frame_body(
        "media-a",
        "media-b",
        &room,
        Some("L1"),
        &json!({"rtc": "mute"}),
        Some((NoticeKind::Captions, true)),
    );
    assert_eq!(frame["callSessionId"], room.as_str());
    assert_eq!(frame["kind"], "roomFrame");
    assert_eq!(frame["except"], "L1");
    assert_eq!(frame["keep"], true);
    // the server's own room is its call id, as before E12
    assert_eq!(
        relay_body("media-a", "media-b", "100", json!({}))["callSessionId"],
        "100"
    );
}

#[test]
fn a_relayed_frame_keeps_only_its_own_short_fields_and_names_its_sender() {
    use super::driver::relayed_frame;
    use serde_json::json;
    let sent = json!({"rtc": "mute", "id": 4, "callSessionId": "100", "callLegId": "L1", "audioMuted": true,
        "cameraOff": "yes", "from": "L9", "admin": true, "padding": "x".repeat(100_000)});
    let relayed = relayed_frame("mute", "L1", &sent).unwrap();
    assert_eq!(
        relayed,
        json!({"rtc": "mute", "from": "L1", "callLegId": "L1", "audioMuted": true, "cameraOff": false})
    );
    let reaction = relayed_frame("reaction", "L1", &json!({"emoji": "👍", "html": "<img>"})).unwrap();
    assert_eq!(reaction["emoji"], "👍");
    assert!(reaction.get("html").is_none());
    let long = relayed_frame("reaction", "L1", &json!({"emoji": "x".repeat(65)})).unwrap();
    assert_eq!(long["emoji"], "", "a long reaction is not relayed");
    assert_eq!(
        relayed_frame("hand", "L1", &json!({"raised": true})).unwrap()["raised"],
        true
    );
    assert!(relayed_frame("offer", "L1", &json!({})).is_none());
    assert!(relayed.to_string().len() < 200);
}

#[test]
fn a_leg_relays_a_burst_and_then_a_few_frames_a_second() {
    use super::driver::RelayBucket;
    let start = std::time::Instant::now();
    let mut bucket = RelayBucket::new(start);
    let taken = (0..100).filter(|_| bucket.take(start)).count();
    assert_eq!(taken, 20, "the burst");
    assert!(!bucket.take(start));
    let later = start + std::time::Duration::from_secs(1);
    let refilled = (0..100).filter(|_| bucket.take(later)).count();
    assert_eq!(refilled, 5, "five a second after it");
    // a direct leg's peer frames: a call's offer, answer and candidates at once
    let mut peer = RelayBucket::for_peer(start);
    assert_eq!((0..100).filter(|_| peer.take(start)).count(), 60);
}

/// The direct path (plans/Comms-Messaging.md §8.3 item 15): a page's `peer` frame reaches the other page with only its
/// kind and what that kind carries, named by its sender; an unknown kind, or an offer without its SDP or with one too
/// long, goes nowhere.
#[test]
fn a_peer_frame_keeps_only_what_its_kind_carries() {
    use super::driver::peer_frame;
    use serde_json::json;
    let offer = peer_frame(
        "L1",
        &json!({"rtc": "peer", "kind": "offer", "to": "L2", "sdp": "v=0", "from": "L9", "callLegId": "L1", "admin": 1}),
    )
    .unwrap();
    assert_eq!(
        offer,
        json!({"rtc": "peer", "kind": "offer", "from": "L1", "sdp": "v=0"})
    );
    let candidate = peer_frame(
        "L1",
        &json!({"kind": "candidate", "candidate": "candidate:1 1 udp 1 10.0.0.2 5000 typ host", "sdpMid": "0",
            "sdpMLineIndex": 0}),
    )
    .unwrap();
    assert_eq!(candidate["sdpMid"], "0");
    assert_eq!(candidate["sdpMLineIndex"], 0);
    assert_eq!(
        peer_frame("L1", &json!({"kind": "hello", "sdp": "v=0"})).unwrap(),
        json!({"rtc": "peer", "kind": "hello", "from": "L1"})
    );
    assert!(
        peer_frame("L1", &json!({"kind": "offer"})).is_none(),
        "an offer without SDP"
    );
    assert!(peer_frame("L1", &json!({"kind": "offer", "sdp": "x".repeat(70_000)})).is_none());
    assert!(peer_frame("L1", &json!({"kind": "candidate", "candidate": "x".repeat(2_000)})).is_none());
    assert!(peer_frame("L1", &json!({"kind": "mute"})).is_none());
}
