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

use super::*;

const KIND: &str = "test";

/// A local node's context whose edge conf has the test kind, two attempts and quick retries.
fn open() -> (ExecutionContext, EntityFacade) {
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    // the first read of the settings keeps them
    ctx.factory().extension(|_| settings());
    (ctx, facade)
}

fn settings() -> EdgeSettings {
    EdgeSettings {
        max_attempts: 2,
        retry_first: Duration::from_millis(100),
        kinds: BTreeMap::from([(KIND.to_string(), "test.Handlers.apply#Test".to_string())]),
        ..EdgeSettings::default()
    }
}

fn item(key: &str, idempotency: &str) -> NewItem {
    NewItem {
        kind: KIND.to_string(),
        conversation_key: key.to_string(),
        idempotency_key: idempotency.to_string(),
        payload: Some(format!("{key}:{idempotency}")),
        content_location: None,
    }
}

fn keys(heads: &Heads) -> Vec<&str> {
    heads.due.iter().map(|item| item.idempotency_key.as_str()).collect()
}

#[test]
fn an_idempotency_key_queues_once() {
    let (ctx, facade) = open();
    let first = enqueue(ctx.factory(), &facade, None, &item("room-1", "m-1")).unwrap();
    let again = enqueue(ctx.factory(), &facade, None, &item("room-1", "m-1")).unwrap();
    assert!(first.created);
    assert!(!again.created);
    assert_eq!(first.queue_item_id, again.queue_item_id);
    assert_eq!(
        first.queue_item_id,
        keyed_id(&local_node_id(ctx.factory()), TO_EDGE, "m-1"),
        "a key's item id is derived from it, so a racing enqueue finds the first's row"
    );
    assert!(first.queue_item_id.len() <= 40);
    let other = enqueue(ctx.factory(), &facade, None, &item("room-1", "")).unwrap();
    assert!(other.created, "an empty key is the item's own id");
    assert!(enqueue(ctx.factory(), &facade, None, &item("", "m-2")).is_err());
}

#[test]
fn a_kind_this_node_does_not_list_and_an_oversized_payload_are_refused() {
    let (ctx, facade) = open();
    let unknown = NewItem {
        kind: "nothing".to_string(),
        ..item("a", "1")
    };
    let err = enqueue(ctx.factory(), &facade, None, &unknown).unwrap_err();
    assert!(err.message().contains("not a queue kind"), "{err}");
    let big = NewItem {
        payload: Some("x".repeat(MAX_PAYLOAD_BYTES + 1)),
        ..item("a", "2")
    };
    let err = enqueue(ctx.factory(), &facade, None, &big).unwrap_err();
    assert!(err.message().contains("at most 1048576 bytes"), "{err}");
    let fits = NewItem {
        payload: Some("x".repeat(MAX_PAYLOAD_BYTES)),
        ..item("a", "3")
    };
    assert!(enqueue(ctx.factory(), &facade, None, &fits).unwrap().created);
}

#[test]
fn each_conversation_key_goes_one_at_a_time_and_a_refused_head_holds_its_key() {
    let (ctx, facade) = open();
    let node = local_node_id(ctx.factory());
    for (key, id) in [("a", "1"), ("b", "2"), ("a", "3"), ("b", "4")] {
        enqueue(ctx.factory(), &facade, None, &item(key, id)).unwrap();
    }
    let now = now_millis();
    let heads = due_heads(&facade, &node, TO_EDGE, now).unwrap();
    assert_eq!(keys(&heads), ["1", "2"], "the oldest item of each key");
    assert!(
        heads.due.iter().all(|head| head.payload.is_empty()),
        "no payloads in a round's heads"
    );

    let settings = settings();
    let [a, b] = [&heads.due[0], &heads.due[1]];
    let claimed = claim(&facade, a, "test", now).unwrap().unwrap();
    assert_eq!(claimed.payload, "a:1", "the claim reads the payload");
    assert!(claim(&facade, a, "other", now).unwrap().is_none(), "held already");
    let step = finish(&facade, &settings, a, &Outcome::Answered(Applied::Applied), now).unwrap();
    assert_eq!(step, Step::Delivered);
    assert!(claim(&facade, b, "test", now).unwrap().is_some());
    let refused = Outcome::Answered(Applied::Failed("no".to_string()));
    assert_eq!(finish(&facade, &settings, b, &refused, now).unwrap(), Step::Retry);

    let heads = due_heads(&facade, &node, TO_EDGE, now).unwrap();
    assert_eq!(keys(&heads), ["3"], "b's refused head holds b");
    assert_eq!(heads.next_due, Some(now + 100));

    // once due, the head goes again, and a second refusal fails it for good: then the next one of b goes
    let later = now + 150;
    let heads = due_heads(&facade, &node, TO_EDGE, later).unwrap();
    let retry = heads.due.iter().find(|item| item.idempotency_key == "2").unwrap();
    assert_eq!(retry.attempts, 1);
    assert!(claim(&facade, retry, "test", later).unwrap().is_some());
    assert_eq!(
        finish(&facade, &settings, retry, &refused, later).unwrap(),
        Step::Failed
    );
    let heads = due_heads(&facade, &node, TO_EDGE, later).unwrap();
    assert_eq!(keys(&heads), ["3", "4"]);
}

#[test]
fn a_long_backlog_of_one_key_does_not_hide_another_keys_head() {
    let (ctx, facade) = open();
    let node = local_node_id(ctx.factory());
    for n in 0..600 {
        enqueue(ctx.factory(), &facade, None, &item("busy", &format!("busy-{n}"))).unwrap();
    }
    enqueue(ctx.factory(), &facade, None, &item("quiet", "quiet-1")).unwrap();
    let heads = due_heads(&facade, &node, TO_EDGE, now_millis()).unwrap();
    assert_eq!(keys(&heads), ["busy-0", "quiet-1"]);
}

#[test]
fn an_attempt_with_no_answer_or_a_receiver_that_cannot_take_it_is_not_counted() {
    let (ctx, facade) = open();
    let node = local_node_id(ctx.factory());
    enqueue(ctx.factory(), &facade, None, &item("a", "1")).unwrap();
    let settings = settings();
    let mut at = now_millis();
    let outcomes = [
        Outcome::Lost("the link dropped".to_string()),
        Outcome::Answered(Applied::Paused),
        Outcome::Answered(Applied::Unavailable("the pool is full".to_string())),
    ];
    for outcome in &outcomes {
        let head = due_heads(&facade, &node, TO_EDGE, at).unwrap().due.remove(0);
        assert_eq!(head.attempts, 0, "{outcome:?}");
        assert!(claim(&facade, &head, "test", at).unwrap().is_some());
        assert_eq!(finish(&facade, &settings, &head, outcome, at).unwrap(), Step::Retry);
        at += 150;
    }
    // no handler for the kind: not counted, and tried again only after ten minutes
    let head = due_heads(&facade, &node, TO_EDGE, at).unwrap().due.remove(0);
    assert!(claim(&facade, &head, "test", at).unwrap().is_some());
    let skew = Outcome::Answered(Applied::NotHandled("no handler".to_string()));
    assert_eq!(finish(&facade, &settings, &head, &skew, at).unwrap(), Step::Retry);
    let heads = due_heads(&facade, &node, TO_EDGE, at + 1000).unwrap();
    assert!(heads.due.is_empty());
    assert_eq!(heads.next_due, Some(at + duration_millis(MAX_RETRY)));
    let head = due_heads(&facade, &node, TO_EDGE, at + duration_millis(MAX_RETRY))
        .unwrap()
        .due
        .remove(0);
    assert_eq!(head.attempts, 0);
}

#[test]
fn the_stats_count_by_kind_and_the_clean_pass_keeps_what_failed_for_the_retention_period() {
    let (ctx, facade) = open();
    enqueue(ctx.factory(), &facade, None, &item("a", "1")).unwrap();
    enqueue(ctx.factory(), &facade, None, &item("a", "2")).unwrap();
    let stats = stats(&facade, None).unwrap();
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].kind, KIND);
    assert_eq!(stats[0].direction, TO_EDGE);
    assert_eq!(stats[0].pending, 2);
    assert!(stats[0].oldest_pending.is_some());

    // nothing is old yet, but a cap of one item's bytes fails the oldest
    let capped = EdgeSettings {
        max_bytes: 4,
        ..settings()
    };
    let now = now_millis();
    let cleaned = clean(&facade, &capped, now).unwrap();
    assert_eq!(cleaned.expired, 1);
    // fifteen days on, the first (failed fifteen days ago) goes; the other has waited past the retention period and
    // fails now, and stays for inspection the full period from its failure, though it was queued long before
    let later = now + 15 * 86_400_000;
    let cleaned = clean(&facade, &settings(), later).unwrap();
    assert_eq!((cleaned.removed, cleaned.expired), (1, 1));
    let cleaned = clean(&facade, &settings(), later + 86_400_000).unwrap();
    assert_eq!(cleaned.removed, 0, "kept from its failure date");
    let stats = super::stats(&facade, None).unwrap();
    assert_eq!((stats[0].pending, stats[0].failed), (0, 1));
    // fifteen days after its failure, it goes too
    let cleaned = clean(&facade, &settings(), later + 15 * 86_400_000).unwrap();
    assert_eq!(cleaned.removed, 1);
}

#[test]
fn a_pause_is_kept_per_facade() {
    let (_ctx, facade) = open();
    assert!(!paused(&facade));
    set_paused(&facade, true);
    assert!(paused(&facade));
    assert!(!paused(&crate::testing::open()));
    set_paused(&facade, false);
    assert!(!paused(&facade));
}

#[test]
fn an_unpaired_outbox_fails_with_its_reason_and_stays_for_inspection() {
    let (ctx, facade) = open();
    enqueue(ctx.factory(), &facade, None, &item("a", "1")).unwrap();
    enqueue(ctx.factory(), &facade, None, &item("b", "2")).unwrap();
    assert_eq!(fail_pending(&facade, TO_LOCAL, "unpaired", now_millis()).unwrap(), 0);
    assert_eq!(fail_pending(&facade, TO_EDGE, "unpaired", now_millis()).unwrap(), 2);
    let stats = stats(&facade, None).unwrap();
    assert_eq!((stats[0].pending, stats[0].failed), (0, 2));
    let rows = facade.find(QUEUE_ITEM).unwrap().disable_authz().list().unwrap();
    assert!(rows.values().iter().all(|row| text(row, "lastErrorText") == "unpaired"));
    assert!(due_heads(
        &facade,
        &local_node_id(ctx.factory()),
        TO_EDGE,
        now_millis() + MAX_RETRY.as_millis() as i64
    )
    .unwrap()
    .due
    .is_empty());
}
