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

use moqui_core::notify::{TopicMessage, RTC_SIGNAL};

use super::{edge_takes, local_takes, member_route, route_name, with_origin};

fn message(body: serde_json::Value) -> TopicMessage {
    TopicMessage {
        origin_id: "test".to_string(),
        message_id: "m-1".to_string(),
        body,
    }
}

#[test]
fn an_edge_node_takes_from_a_local_node_only_its_frames_and_controls() {
    let take = edge_takes("local-1", "local-1", true);
    // a frame whose replies would go to another node
    let forged = message(serde_json::json!({"target": "edge-1", "kind": "frame", "origin": "local-2", "socketId": 7}));
    assert!(take(RTC_SIGNAL, &forged).is_err_and(|err| err.contains("another origin")));
    // what only a media node sends
    for kind in ["reply", "relay", "roomFrame", "other"] {
        let sent = message(serde_json::json!({"target": "edge-1", "kind": kind}));
        assert!(
            take(RTC_SIGNAL, &sent).is_err_and(|err| err.contains("of kind")),
            "{kind}"
        );
    }
}

#[test]
fn an_edge_node_refuses_a_message_that_already_names_a_node() {
    let take = edge_takes("local-1", "local-1", true);
    for kind in ["frame", "control", "sip"] {
        let forged = message(
            serde_json::json!({"target": "edge-1", "kind": kind, "origin": "local-1",
            "linkNode": "local-2"}),
        );
        assert!(
            take(RTC_SIGNAL, &forged).is_err_and(|err| err.contains("already names a node")),
            "{kind}"
        );
    }
    let not_an_object = message(serde_json::json!(["frame"]));
    assert!(take(RTC_SIGNAL, &not_an_object).is_err());
}

#[test]
fn an_edge_node_stamps_the_sending_node() {
    let body = serde_json::json!({"target": "edge-1", "kind": "control", "action": "closeRoom", "callSessionId": "1"});
    let stamped = super::stamped("local-1", &message(body)).unwrap();
    assert_eq!(moqui_core::edge::signal::link_node(&stamped.body), Some("local-1"));
    assert_eq!(stamped.body["callSessionId"], "1");
    assert_eq!(stamped.message_id, "m-1");
}

#[test]
fn a_local_node_takes_from_its_edge_node_only_replies() {
    let take = local_takes("edge-1");
    for kind in ["frame", "control", "relay", "roomFrame", "sip"] {
        let sent = message(serde_json::json!({"target": "local-1", "kind": kind}));
        assert!(
            take(RTC_SIGNAL, &sent).is_err_and(|err| err.contains("of kind")),
            "{kind}"
        );
    }
    let other_topic = message(serde_json::json!({"target": "local-1", "kind": "reply"}));
    assert!(take("entity-cache-invalidate", &other_topic).is_err());
}

/// In a pool each member has its own route on the edge node (plans/Edge-Node2.md §4.4): a frame from member b names b
/// as its origin (another origin, even the pairing's, is refused); the edge node takes its origin as b's route, so the
/// replies for the leg go down b's down connection with b as their target, and a's route stays when b's goes.
#[test]
fn call_signaling_reaches_the_member_whose_socket_bound_the_leg() {
    let take = edge_takes("pool-1", "member-b", true);
    for origin in ["member-a", "pool-1", "member-b@pool-1"] {
        let forged = message(serde_json::json!({"target": "edge-1", "kind": "frame", "origin": origin}));
        assert!(
            take(RTC_SIGNAL, &forged).is_err_and(|err| err.contains("another origin")),
            "{origin}"
        );
    }
    let frame = message(serde_json::json!({"target": "edge-1", "kind": "frame", "origin": "member-b", "socketId": 7}));
    let routed = with_origin(&frame, &route_name("pool-1", "member-b"));
    assert_eq!(routed.body["origin"], "member-b@pool-1");
    assert_eq!(routed.body["socketId"], 7);

    let (route_a, mut queue_a) = member_route("pool-1", "member-a", 3);
    let (route_b, mut queue_b) = member_route("pool-1", "member-b", 4);
    let reply = |to: &str| message(serde_json::json!({"target": to, "kind": "reply", "socketId": 7, "frame": {}}));
    moqui_core::edge::signal::send("member-b@pool-1", RTC_SIGNAL, &reply("member-b@pool-1"))
        .unwrap()
        .unwrap();
    let sent = queue_b.try_recv().unwrap();
    assert_eq!(sent.generation, 4);
    let body: serde_json::Value = serde_json::from_str(&sent.body_json).unwrap();
    assert_eq!(body["target"], "member-b", "the member's own id, as it checks");
    assert!(queue_a.try_recv().is_err(), "nothing for a");
    drop(route_b);
    assert!(
        moqui_core::edge::signal::send("member-b@pool-1", RTC_SIGNAL, &reply("member-b@pool-1"))
            .is_some_and(|sent| sent.is_err()),
        "b's route went with its down connection: refused, not sent to a"
    );
    moqui_core::edge::signal::send("member-a@pool-1", RTC_SIGNAL, &reply("member-a@pool-1"))
        .unwrap()
        .unwrap();
    assert_eq!(queue_a.try_recv().unwrap().generation, 3, "a's route stays");
    drop(route_a);
}

/// A web protection ban goes up the link only (plans/Edge-Node2.md §4.3): from a local node stamped with it, so the edge
/// node holds it for that node's hosts only. Only a ban or a lift; a local node takes none from its edge node.
#[test]
fn web_bans_cross_the_link_marked_with_where_they_came_from() {
    let ban = message(serde_json::json!({"kind": "ban", "address": "203.0.113.9", "seconds": 60, "from": "m-1"}));
    assert!(edge_takes("local-1", "local-1", true)(moqui_core::notify::WEB_BAN, &ban).is_ok());
    assert!(local_takes("edge-1")(moqui_core::notify::WEB_BAN, &ban).is_err());
    let other = message(serde_json::json!({"kind": "control", "address": "203.0.113.9"}));
    assert!(
        edge_takes("local-1", "local-1", true)(moqui_core::notify::WEB_BAN, &other)
            .is_err_and(|err| err.contains("of kind"))
    );
    assert!(local_takes("edge-1")(moqui_core::notify::WEB_BAN, &other).is_err());
    // a ban that names a node already is refused, as any message
    let forged = message(serde_json::json!({"kind": "ban", "address": "203.0.113.9", "linkNode": "local-2"}));
    assert!(
        edge_takes("local-1", "local-1", true)(moqui_core::notify::WEB_BAN, &forged)
            .is_err_and(|err| err.contains("already names"))
    );
    // and a local node's message on another topic is not taken
    let frame = message(serde_json::json!({"kind": "frame", "origin": "local-1"}));
    assert!(edge_takes("local-1", "local-1", true)("session-end", &frame).is_err());
}

/// A front node has no calls (plans/Edge-Node2.md §4.7): a ban from a node behind it is taken, its call signaling not.
#[test]
fn a_front_node_takes_a_ban_and_no_call_signaling() {
    let ban = message(serde_json::json!({"kind": "ban", "address": "203.0.113.9", "seconds": 60, "from": "m-1"}));
    assert!(edge_takes("edge-pool", "edge-1", false)(moqui_core::notify::WEB_BAN, &ban).is_ok());
    // the same control is looked at by an edge node (and refused there for naming a node), and not at all by a front
    let control = message(serde_json::json!({"kind": "control", "linkNode": "other"}));
    assert!(edge_takes("edge-pool", "edge-1", false)(RTC_SIGNAL, &control).is_err_and(|err| err.contains("on rtc")));
    assert!(
        edge_takes("edge-pool", "edge-1", true)(RTC_SIGNAL, &control).is_err_and(|err| err.contains("already names"))
    );
}
