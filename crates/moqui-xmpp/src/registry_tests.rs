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

//! The session registry's user index (plans/archive/High-Level-Review.md HL-009).

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
        blocked: Arc::new(Mutex::new(None)),
        detached: Arc::new(AtomicBool::new(false)),
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

fn stanza(n: usize) -> Element {
    Element::builder("message", crate::ns::CLIENT)
        .append(n.to_string().as_str())
        .build()
}

/// SEC-COM-19: a flooding sender cannot close another user's session: past its allowance, or past the target's queue,
/// its stanzas are refused to it and the target's session is never told to close; another sender still gets through,
/// and the target's own sessions are not held to an allowance.
#[test]
fn a_flooding_sender_does_not_close_the_target() {
    let registry = Registry::with_limits(0.0, 5.0);
    let (bob, mut bob_rx) = entry(&registry, "bob", "phone", 3);
    let routed: Vec<Routed> = (0..8)
        .map(|n| registry.route("eve", "eve@example.com", &bob, stanza(n)))
        .collect();
    assert_eq!(&routed[..3], &[Routed::Sent; 3]);
    assert_eq!(
        &routed[3..],
        &[Routed::Limited; 5],
        "the queue is full, then the allowance is spent"
    );
    assert!(overflow_not_notified(&bob), "the target's session is not told to close");
    while bob_rx.try_recv().is_ok() {}
    assert_eq!(
        registry.route("eve", "eve@example.com", &bob, stanza(9)),
        Routed::Limited,
        "eve's allowance is spent"
    );
    assert_eq!(
        registry.route("ann", "ann@example.com", &bob, stanza(10)),
        Routed::Sent,
        "another sender's is its own"
    );
    for n in 0..2 {
        assert_eq!(
            registry.route("bob", "bob@example.com", &bob, stanza(20 + n)),
            Routed::Sent,
            "bob's own"
        );
    }
}

/// Whether `entry`'s overflow was not notified: a notification waiting would complete a `notified` at once.
fn overflow_not_notified(entry: &Entry) -> bool {
    let notified = entry.overflow.notified();
    tokio::pin!(notified);
    let waker = std::task::Waker::noop();
    let mut context = std::task::Context::from_waker(waker);
    std::future::Future::poll(notified.as_mut(), &mut context).is_pending()
}

/// SEC-COM-19: a stanza from a user the target blocks is dropped before it is queued, once the target read its list.
#[test]
fn a_blocked_senders_stanza_never_reaches_the_queue() {
    let registry = Registry::default();
    let (bob, mut bob_rx) = entry(&registry, "bob", "phone", 4);
    *lock(&bob.blocked) = Some(HashSet::from(["eve@example.com".to_string()]));
    for n in 0..10 {
        assert_eq!(
            registry.route("eve", "eve@example.com", &bob, stanza(n)),
            Routed::Blocked
        );
    }
    assert!(bob_rx.try_recv().is_err(), "nothing queued");
    assert_eq!(registry.route("ann", "ann@example.com", &bob, stanza(11)), Routed::Sent);
}
