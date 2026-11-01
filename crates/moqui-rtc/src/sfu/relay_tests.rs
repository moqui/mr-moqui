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

//! The W7 gate (plans/Comms-Framework.md §6.2): one room on two media nodes, an origin and a leg node, on the in-memory
//! network. Media both ways over the relay link; each track crossing once, in the layers up to the highest any
//! subscriber behind the link wants, a layer changed at the leg node; the leg node's tracks reaching the origin's taps;
//! the caps refusing a link; a lost link reported and replaced; the link closed when its last leg leaves or the room
//! ends; an audio-only tap bringing no video across; late links of an ended room fenced; a link that never connects
//! dropped.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use str0m::media::MediaKind;

use super::testnet::{Net, LAYER_BYTES, LEG_NODE, ORIGIN, ROOM};
use super::{LegKey, RelayMessage, RelayRole, SfuConfig, SfuError, SfuEvent, Source};
use crate::tap::{Kind, Kinds, MediaTap, QueueTap, TapFrame};

fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

fn leg_node_net() -> Net {
    Net::with_leg_node(SfuConfig::default())
}

fn down(events: &[SfuEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            SfuEvent::RelayDown { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_room_on_two_nodes_carries_media_both_ways_over_one_link() {
    let mut net = leg_node_net();
    net.join("A", false);
    net.join_leg_node("B", false);
    net.join_leg_node("C", false);
    // B is loud: the origin, which sees B only through the link, makes B the speaker
    net.client_mut("B").audio_level = -5;
    net.client_mut("A").audio_level = -70;
    net.client_mut("C").audio_level = -70;
    net.run(secs(3));
    for (at, from) in [("A", "B"), ("A", "C"), ("B", "A"), ("C", "A"), ("B", "C"), ("C", "B")] {
        assert!(net.received(at, from, MediaKind::Audio) > 30, "{at} hears {from}");
        assert!(net.received(at, from, MediaKind::Video) > 15, "{at} sees {from}");
    }
    // one link: the origin has A, the leg node B and C; each side's legs are its own
    assert_eq!(net.sfu.legs(ROOM), vec![Arc::<str>::from("A")]);
    assert_eq!(net.leg_node().sfu.legs(ROOM).len(), 2);
    assert_eq!(
        net.sfu.relays(ROOM),
        vec![(Arc::<str>::from(LEG_NODE), RelayRole::Origin, true)]
    );
    assert_eq!(
        net.leg_node().sfu.relays(ROOM),
        vec![(Arc::<str>::from(ORIGIN), RelayRole::LegNode, true)]
    );
    // the leg node's legs' tracks are named for them at the origin
    assert!(net.events.iter().any(|e| matches!(e,
        SfuEvent::Track { key, kind: Kind::Video, open: true, .. } if &*key.leg == "B")));
    assert!(net
        .events
        .iter()
        .any(|e| matches!(e, SfuEvent::ActiveSpeaker { leg, .. } if &**leg == "B")));
    assert!(net
        .leg_node()
        .events
        .iter()
        .any(|e| matches!(e, SfuEvent::ActiveSpeaker { leg, .. } if &**leg == "B")));
    // A's audio crossed once for its two subscribers there: about one frame in 20 ms
    let sent = net.sfu.relay_sent(ROOM, LEG_NODE).expect("a link");
    assert!(
        (100..=170).contains(&sent[0]),
        "A's audio frames over the link: {}",
        sent[0]
    );
}

#[test]
fn a_track_crosses_in_the_layers_up_to_the_highest_wanted_behind_the_link() {
    let mut net = leg_node_net();
    net.join("A", true);
    net.join_leg_node("B", false);
    net.join_leg_node("C", false);
    net.run(secs(4));
    let layer = |net: &Net, at: &str| net.client(at).last_video_len.get("A").copied();
    assert_eq!(layer(&net, "B"), Some(LAYER_BYTES[2]), "B gets the high layer");
    let before = net.sfu.relay_sent(ROOM, LEG_NODE).expect("a link");
    assert!(before[4] > 30, "the high layer crossed: {before:?}");

    // a layer change at the leg node: B wants the low layer, C still the high one
    let now = net.now;
    net.leg_node_sfu()
        .subscribe(now, &LegKey::new(ROOM, "B"), 1, "A", Some(super::Layer::Low))
        .expect("subscribed");
    net.run(secs(2));
    assert_eq!(layer(&net, "B"), Some(LAYER_BYTES[0]), "B switched to the low layer");
    assert_eq!(layer(&net, "C"), Some(LAYER_BYTES[2]), "C keeps the high layer");
    assert!(net.leg_node().events.iter().any(|e| matches!(e,
        SfuEvent::Layer { key, layer: Some(super::Layer::Low), .. } if &*key.leg == "B")));

    // C too: only the low layer is wanted behind the link now, and the others stop crossing
    let now = net.now;
    net.leg_node_sfu()
        .subscribe(now, &LegKey::new(ROOM, "C"), 1, "A", Some(super::Layer::Low))
        .expect("subscribed");
    net.run(secs(1));
    let capped = net.sfu.relay_sent(ROOM, LEG_NODE).expect("a link");
    net.run(secs(2));
    let after = net.sfu.relay_sent(ROOM, LEG_NODE).expect("a link");
    assert_eq!(after[3], capped[3], "the mid layer stopped crossing");
    assert_eq!(after[4], capped[4], "the high layer stopped crossing");
    assert!(
        after[2] > capped[2] + 40,
        "the low layer still crosses: {capped:?} {after:?}"
    );
    assert_eq!(layer(&net, "C"), Some(LAYER_BYTES[0]));
    assert!(net.relayed.iter().any(|(from, m)| *from == LEG_NODE
        && matches!(m, RelayMessage::Want { wants, .. }
            if wants.iter().any(|w| w.leg == "A" && w.layer.as_deref() == Some("low")))));

    // nobody wants A's video behind the link: no video crosses, audio does
    for leg in ["B", "C"] {
        let now = net.now;
        net.leg_node_sfu()
            .subscribe(now, &LegKey::new(ROOM, leg), 1, "A", None)
            .expect("subscribed");
    }
    net.run(secs(1));
    let off = net.sfu.relay_sent(ROOM, LEG_NODE).expect("a link");
    net.run(secs(1));
    let still = net.sfu.relay_sent(ROOM, LEG_NODE).expect("a link");
    assert_eq!(still[2..], off[2..], "no video crosses");
    assert!(still[0] > off[0] + 40, "audio crosses");
}

type Seen = Arc<Mutex<Vec<(String, Kind, Option<String>, Source)>>>;

struct Collect(Seen);

impl MediaTap for Collect {
    fn on_frame(&mut self, f: &TapFrame<'_>) {
        if let Ok(mut v) = self.0.lock() {
            v.push((f.leg.to_string(), f.kind, f.rid.map(str::to_string), f.source));
        }
    }
}

#[test]
fn tracks_of_a_leg_at_the_leg_node_reach_the_origin_s_taps_in_every_layer() {
    let mut net = leg_node_net();
    net.join("A", false);
    net.join_leg_node("B", true);
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    net.sfu.add_tap(ROOM, Box::new(Collect(seen.clone()))).expect("tapped");
    // B shares its camera as a screen: the origin learns the new source over the link
    let video_mid = net.client("B").video_mid.expect("a video mid").to_string();
    net.leg_node_sfu()
        .set_source(&LegKey::new(ROOM, "B"), 1, &video_mid, Source::Screen)
        .expect("declared");
    net.run(secs(3));
    let seen = seen.lock().expect("frames");
    assert!(
        seen.iter().any(|(leg, kind, _, _)| leg == "B" && *kind == Kind::Audio),
        "B's audio"
    );
    for rid in ["l", "m", "h"] {
        assert!(
            seen.iter().any(|(leg, kind, r, source)| leg == "B"
                && *kind == Kind::Video
                && r.as_deref() == Some(rid)
                && *source == Source::Screen),
            "B's {rid} layer, as a screen"
        );
    }
    assert!(net.relayed.iter().any(|(from, m)| *from == ORIGIN
        && matches!(m, RelayMessage::Want { wants, .. }
            if wants.iter().any(|w| w.leg == "B" && w.layer.as_deref() == Some("high")))));
}

#[test]
fn links_past_the_caps_are_refused_explicitly() {
    let mut net = Net::with_leg_node(SfuConfig {
        max_relays_per_room: 0,
        ..SfuConfig::default()
    });
    net.join("A", false);
    net.join_leg_node("B", false);
    net.run(secs(1));
    assert!(
        net.relayed
            .iter()
            .any(|(from, m)| *from == ORIGIN
                && matches!(m, RelayMessage::Refused { code, .. } if code == "tooManyRelays"))
    );
    assert_eq!(down(&net.leg_node().events), vec!["refused:tooManyRelays".to_string()]);
    assert!(net.sfu.relays(ROOM).is_empty());
    assert_eq!(net.received("B", "A", MediaKind::Audio), 0);

    // the leg node's own cap
    let mut net = Net::with_leg_node(SfuConfig {
        max_relays: 0,
        ..SfuConfig::default()
    });
    let now = net.now;
    assert_eq!(net.leg_node_sfu().link(now, ROOM, ORIGIN), Err(SfuError::TooManyRelays));
}

#[test]
fn a_lost_link_is_reported_and_a_new_link_replaces_it() {
    let mut net = leg_node_net();
    net.join("A", false);
    net.join_leg_node("B", false);
    net.run(secs(2));
    assert!(net.received("B", "A", MediaKind::Audio) > 20);
    let old_link = net
        .relayed
        .iter()
        .find_map(|(from, m)| (*from == LEG_NODE).then(|| m.link()))
        .expect("the leg node's first offer");

    net.link_up = false;
    let mut waited = 0;
    while !down(&net.leg_node().events).contains(&"lost".to_string()) && waited < 60 {
        net.run(secs(1));
        waited += 1;
    }
    assert!(
        down(&net.leg_node().events).contains(&"lost".to_string()),
        "the leg node saw its link lost"
    );
    assert!(net.leg_node().sfu.relays(ROOM).is_empty());

    // the reconnect path: the leg node links again, with a new link that replaces the origin's old one
    net.link_up = true;
    let now = net.now;
    net.leg_node_sfu().link(now, ROOM, ORIGIN).expect("linked again");
    let heard = net.received("B", "A", MediaKind::Audio);
    net.run(secs(3));
    assert!(net.received("B", "A", MediaKind::Audio) > heard + 30, "media again");
    assert_eq!(net.sfu.relays(ROOM).len(), 1, "one link at the origin");
    // a message of the old link is dropped
    let now = net.now;
    net.sfu
        .handle_relay(
            now,
            ROOM,
            LEG_NODE,
            RelayMessage::Want {
                link: old_link,
                wants: vec![super::RelayWant {
                    leg: "A".to_string(),
                    layer: None,
                }],
            },
        )
        .expect("dropped");
    let video = net.received("B", "A", MediaKind::Video);
    net.run(secs(1));
    assert!(
        net.received("B", "A", MediaKind::Video) > video + 10,
        "the old link's want did nothing"
    );
}

#[test]
fn the_link_closes_when_the_leg_node_s_last_leg_leaves_or_the_room_ends() {
    let mut net = leg_node_net();
    net.join("A", false);
    net.join_leg_node("B", false);
    net.run(secs(2));
    let now = net.now;
    net.leg_node_sfu().leave(now, &LegKey::new(ROOM, "B"), 1).expect("left");
    net.run(Duration::from_millis(200));
    assert!(net
        .relayed
        .iter()
        .any(|(from, m)| *from == LEG_NODE && matches!(m, RelayMessage::Close { reason, .. } if reason == "empty")));
    assert_eq!(down(&net.events), vec!["empty".to_string()]);
    assert!(net.sfu.relays(ROOM).is_empty());
    assert_eq!(net.leg_node().sfu.room_count(), 0, "the leg node let the room go");
    assert_eq!(net.sfu.legs(ROOM), vec![Arc::<str>::from("A")]);

    // a new leg on the leg node links again; the origin ending the room ends the link
    net.join_leg_node("C", false);
    net.run(secs(2));
    assert!(net.received("C", "A", MediaKind::Audio) > 20);
    let now = net.now;
    net.sfu.close_room(now, ROOM);
    net.run(Duration::from_millis(200));
    assert!(down(&net.leg_node().events).contains(&"ended".to_string()));
    assert!(net.leg_node().sfu.relays(ROOM).is_empty());
}

#[test]
fn an_audio_only_tap_brings_no_video_across_and_a_video_tap_the_high_layer() {
    let mut net = leg_node_net();
    net.join("A", false);
    net.join_leg_node("B", true);
    // A wants none of B's video; the origin's mixer (an audio tap) wants none either
    let now = net.now;
    net.sfu
        .subscribe(now, &LegKey::new(ROOM, "A"), 1, "B", None)
        .expect("subscribed");
    let (mixer, _mixed) = QueueTap::channel(10_000, Kinds::Audio);
    net.sfu.add_tap(ROOM, Box::new(mixer)).expect("tapped");
    net.run(secs(2));
    let off = net.leg_node().sfu.relay_sent(ROOM, ORIGIN).expect("a link");
    net.run(secs(1));
    let still = net.leg_node().sfu.relay_sent(ROOM, ORIGIN).expect("a link");
    assert_eq!(still[1..], off[1..], "no video crosses for an audio tap");
    assert!(still[0] > off[0] + 30, "audio crosses: {off:?} {still:?}");

    // the recorder (a tap of every kind) wants the high layer
    let (recorder, _recorded) = QueueTap::channel(10_000, Kinds::All);
    net.sfu.add_tap(ROOM, Box::new(recorder)).expect("tapped");
    net.run(secs(1));
    let on = net.leg_node().sfu.relay_sent(ROOM, ORIGIN).expect("a link");
    net.run(secs(1));
    let after = net.leg_node().sfu.relay_sent(ROOM, ORIGIN).expect("a link");
    assert!(after[4] > on[4] + 10, "the high layer crosses: {on:?} {after:?}");
}

#[test]
fn a_room_that_ended_takes_no_late_link_and_a_leg_node_none_from_its_origin() {
    let mut net = leg_node_net();
    net.join("A", false);
    net.join_leg_node("B", false);
    net.run(secs(2));
    let now = net.now;
    net.sfu.close_room(now, ROOM);
    net.run(Duration::from_millis(200));
    assert_eq!(down(&net.leg_node().events), vec!["ended".to_string()]);

    // a late link of the leg node's (its leg not yet gone): the origin does not open the room again, and ends the link
    let now = net.now;
    net.leg_node_sfu().link(now, ROOM, ORIGIN).expect("linked");
    net.run(Duration::from_millis(200));
    assert_eq!(net.sfu.room_count(), 0, "the origin did not open the room again");
    assert!(net.sfu.relays(ROOM).is_empty());
    assert_eq!(
        down(&net.leg_node().events),
        vec!["ended".to_string(), "ended".to_string()]
    );
    assert!(net.leg_node().sfu.relays(ROOM).is_empty());

    // the room ended at the leg node too: a late leg or control does not link it again
    let now = net.now;
    net.leg_node_sfu().close_room(now, ROOM);
    assert_eq!(net.leg_node_sfu().link(now, ROOM, ORIGIN), Err(SfuError::UnknownRoom));
}

#[test]
fn a_leg_node_takes_no_new_link_from_its_origin() {
    let mut net = leg_node_net();
    net.join("A", false);
    net.join_leg_node("B", false);
    net.run(secs(2));
    let now = net.now;
    net.leg_node_sfu().leave(now, &LegKey::new(ROOM, "B"), 1).expect("left");
    net.run(Duration::from_millis(200));
    assert_eq!(net.leg_node().sfu.room_count(), 0);

    // a late offer of the origin's, of a link the leg node closed: dropped, nothing opened
    let now = net.now;
    let late = RelayMessage::Offer {
        link: 7,
        offer_id: 3,
        sdp: "v=0".to_string(),
        tracks: Vec::new(),
    };
    assert_eq!(net.leg_node_sfu().handle_relay(now, ROOM, ORIGIN, late), Ok(()));
    assert_eq!(net.leg_node().sfu.room_count(), 0, "the leg node opened nothing");
    assert!(net.leg_node().sfu.relays(ROOM).is_empty());
}

#[test]
fn a_link_that_never_connects_is_dropped_as_unreachable() {
    let mut net = leg_node_net();
    net.join("A", false);
    // the leg node's offer never reaches the origin
    net.link_up = false;
    net.join_leg_node("B", false);
    net.run(secs(8));
    assert!(down(&net.leg_node().events).is_empty(), "still trying");
    net.run(secs(3));
    assert_eq!(down(&net.leg_node().events), vec!["unreachable".to_string()]);
    assert!(net.leg_node().sfu.relays(ROOM).is_empty());
    assert!(net.sfu.relays(ROOM).is_empty());
}
