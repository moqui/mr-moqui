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

use super::{edge_takes, local_takes};

fn message(body: serde_json::Value) -> TopicMessage {
    TopicMessage {
        origin_id: "test".to_string(),
        message_id: "m-1".to_string(),
        body,
    }
}

#[test]
fn an_edge_node_takes_from_a_local_node_only_its_frames_and_controls() {
    let take = edge_takes("local-1", || 1);
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
fn an_edge_node_takes_no_call_messages_while_two_local_nodes_are_linked() {
    let take = edge_takes("local-1", || 2);
    let frame = message(serde_json::json!({"target": "edge-1", "kind": "frame", "origin": "local-1"}));
    assert!(take(RTC_SIGNAL, &frame).is_err_and(|err| err.contains("2 local nodes")));
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
