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

//! The session registry's user index (plans/High-Level-Review.md HL-009).

use super::*;

fn entry(registry: &Registry, user_id: &str, resource: &str, queue: usize) -> (Entry, mpsc::Receiver<Inbound>) {
    let (tx, rx) = mpsc::channel(queue);
    let (available, carbons) = flags();
    let entry = Entry {
        id: registry.next_id(),
        user_id: user_id.to_string(),
        username: user_id.to_string(),
        resource: resource.to_string(),
        tx,
        overflow: Arc::new(Notify::new()),
        notify: Arc::new(Mutex::new(HashSet::new())),
        available,
        carbons,
    };
    registry.register(entry.clone());
    (entry, rx)
}

#[test]
fn lookups_and_chat_pushes_reach_only_the_named_users_sessions() {
    let registry = Registry::default();
    let idle: Vec<_> = (0..500)
        .map(|n| entry(&registry, &format!("idle-{n}"), "phone", 4))
        .collect();
    let (ann_phone, mut ann_phone_rx) = entry(&registry, "ann", "phone", 4);
    let (ann_desk, mut ann_desk_rx) = entry(&registry, "ann", "desk", 4);
    let (_bob, mut bob_rx) = entry(&registry, "bob", "phone", 4);

    let mut of_ann: Vec<u64> = registry.sessions_of("ann").iter().map(|entry| entry.id).collect();
    of_ann.sort_unstable();
    assert_eq!(of_ann, [ann_phone.id, ann_desk.id]);
    assert_eq!(registry.session("ann", "desk").map(|entry| entry.id), Some(ann_desk.id));
    assert!(registry.session("ann", "tablet").is_none() && registry.session("carol", "phone").is_none());
    assert!(registry.has_user("bob") && !registry.has_user("carol"));

    registry.deliver_notice(
        &serde_json::json!({"topic": "chatRoom:r1", "userIds": ["ann", "bob", "ann"], "message": {"text": "hi"}}),
    );
    assert!(ann_phone_rx.try_recv().is_ok() && ann_desk_rx.try_recv().is_ok() && bob_rx.try_recv().is_ok());
    assert!(ann_phone_rx.try_recv().is_err(), "a user named twice gets it once");
    for (_, rx) in &idle {
        assert!(rx.is_empty());
    }

    // a session that goes leaves the index; the user's other session stays
    registry.unregister(ann_phone.id);
    registry.unregister(ann_phone.id);
    assert_eq!(registry.sessions_of("ann").len(), 1);
    assert!(registry.session("ann", "phone").is_none());
    registry.unregister(ann_desk.id);
    assert!(!registry.has_user("ann") && registry.sessions_of("ann").is_empty());
}

#[test]
fn a_session_whose_queue_is_full_is_told_to_close_and_the_others_still_get_the_push() {
    let registry = Registry::default();
    let (full, _full_rx) = entry(&registry, "dan", "phone", 1);
    full.tx
        .try_send(Inbound::Notice {
            topic: "userStatus".to_string(),
            message: Json::Null,
        })
        .unwrap();
    let (_other, mut other_rx) = entry(&registry, "dan", "desk", 4);
    registry.deliver_notice(&serde_json::json!({"topic": "userStatus", "userIds": ["dan"]}));
    assert!(other_rx.try_recv().is_ok());
    // the full one is closed by its own loop, which unregisters it; until then it stays in the index
    assert_eq!(registry.sessions_of("dan").len(), 2);
    registry.unregister(full.id);
    assert_eq!(registry.sessions_of("dan").len(), 1);
}
