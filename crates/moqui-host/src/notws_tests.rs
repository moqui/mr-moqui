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

//! The notification socket registry's user index (plans/archive/High-Level-Review.md HL-009). The registry is the process's,
//! so each test names its own users and removes its own sockets.

use super::*;

/// A registered socket of `user_id` following `topics`, with a queue of `queue`, and its receiving end.
fn socket(user_id: &str, topics: &[&str], queue: usize) -> (Arc<Slot>, tokio::sync::mpsc::Receiver<Push>) {
    let (tx, rx) = tokio::sync::mpsc::channel(queue);
    let slot = Arc::new(Slot {
        id: NEXT_SOCKET.fetch_add(1, Ordering::Relaxed),
        user_id: user_id.to_string(),
        session_id: format!("session-{user_id}"),
        topics: Mutex::new(topics.iter().map(|topic| (*topic).to_string()).collect()),
        tx,
        overflow: tokio::sync::Notify::new(),
    });
    register(&slot);
    (slot, rx)
}

fn notice(topic: &str, users: &[&str]) -> TopicMessage {
    TopicMessage {
        origin_id: "test".to_string(),
        message_id: "m".to_string(),
        body: serde_json::json!({"topic": topic, "notificationMessageId": "", "userIds": users, "title": "hi"}),
    }
}

#[test]
fn a_notice_for_a_few_users_visits_only_their_sockets_among_many_idle_ones() {
    let idle: Vec<_> = (0..2000)
        .map(|n| socket(&format!("fan-idle-{n}"), &["ALL"], 4))
        .collect();
    let (ann_tab, mut ann_tab_rx) = socket("fan-ann", &["Chat"], 4);
    let (ann_other, mut ann_other_rx) = socket("fan-ann", &["Other"], 4);
    let (bob, mut bob_rx) = socket("fan-bob", &["ALL"], 4);

    // the two recipients' three sockets, the same user named twice counted once
    assert_eq!(fan_out_notice(&notice("Chat", &["fan-ann", "fan-bob", "fan-ann"])), 3);
    assert!(ann_tab_rx.try_recv().is_ok(), "ann's Chat tab got it");
    assert!(
        ann_other_rx.try_recv().is_err(),
        "ann's other tab follows another topic"
    );
    let pushed = bob_rx.try_recv().unwrap();
    assert!(
        !pushed.text.contains("userIds"),
        "the recipients are for routing only: {}",
        pushed.text
    );
    for (_, rx) in &idle {
        assert!(rx.is_empty());
    }
    // no recipients, or none with a socket here: nothing is visited
    assert_eq!(fan_out_notice(&notice("Chat", &[])), 0);
    assert_eq!(fan_out_notice(&notice("Chat", &["fan-nobody"])), 0);

    for (slot, _) in &idle {
        unregister(slot.id);
    }
    for slot in [&ann_tab, &ann_other, &bob] {
        unregister(slot.id);
    }
    assert!(!has_socket("fan-ann") && !has_socket("fan-idle-7"));
}

#[test]
fn a_socket_whose_queue_overflows_during_a_fan_out_is_removed_and_the_others_still_get_it() {
    // carol's first tab is full; her second tab and dave are not
    let (full, _full_rx) = socket("fan-carol", &["ALL"], 1);
    full.tx
        .try_send(Push {
            text: "waiting".to_string(),
            message_id: String::new(),
        })
        .unwrap();
    let (second, mut second_rx) = socket("fan-carol", &["ALL"], 4);
    let (dave, mut dave_rx) = socket("fan-dave", &["ALL"], 4);

    assert_eq!(fan_out_notice(&notice("Chat", &["fan-carol", "fan-dave"])), 3);
    assert!(second_rx.try_recv().is_ok() && dave_rx.try_recv().is_ok());
    {
        let registry = lock(sockets());
        assert!(!registry.by_id.contains_key(&full.id), "the full socket is removed");
        assert_eq!(registry.by_user.get("fan-carol"), Some(&vec![second.id]));
    }
    assert!(has_socket("fan-carol"), "carol still has a socket");
    // the next notice visits only what is left, and a second removal of the full one changes nothing
    unregister(full.id);
    assert_eq!(fan_out_notice(&notice("Chat", &["fan-carol"])), 1);
    assert!(second_rx.try_recv().is_ok());
    // direct signaling still finds a socket by its id
    assert!(push_to_socket(dave.id, "frame".to_string()));
    assert!(!push_to_socket(full.id, "frame".to_string()));

    unregister(second.id);
    unregister(dave.id);
    assert!(!has_socket("fan-carol") && !has_socket("fan-dave"));
}
