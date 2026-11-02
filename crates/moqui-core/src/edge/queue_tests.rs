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
        content: None,
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
fn unavailable_answers_in_a_row_wait_longer_each_time_and_are_not_counted() {
    let (ctx, facade) = open();
    let node = local_node_id(ctx.factory());
    enqueue(ctx.factory(), &facade, None, &item("a", "1")).unwrap();
    let settings = settings();
    let mut at = now_millis();
    let unavailable = Outcome::Answered(Applied::Unavailable("the relay is away".to_string()));
    for waits in 1..=4_u32 {
        let head = due_heads(&facade, &node, TO_EDGE, at).unwrap().due.remove(0);
        assert_eq!((head.attempts, head.waits), (0, waits - 1));
        assert!(claim(&facade, &head, "test", at).unwrap().is_some());
        assert_eq!(
            finish(&facade, &settings, &head, &unavailable, at).unwrap(),
            Step::Retry
        );
        // 100 ms, 200, 400, 800: doubled each time, as counted refusals are
        let wait = duration_millis(settings.retry_after(waits));
        assert_eq!(wait, 100 * (1 << (waits - 1)));
        let heads = due_heads(&facade, &node, TO_EDGE, at).unwrap();
        assert_eq!(heads.next_due, Some(at + wait), "wait {waits}");
        at += wait;
    }
    // any other answer ends the run: the next unavailable one waits the first wait again
    let head = due_heads(&facade, &node, TO_EDGE, at).unwrap().due.remove(0);
    assert!(claim(&facade, &head, "test", at).unwrap().is_some());
    let lost = Outcome::Lost("the link dropped".to_string());
    assert_eq!(finish(&facade, &settings, &head, &lost, at).unwrap(), Step::Retry);
    at += 100;
    let head = due_heads(&facade, &node, TO_EDGE, at).unwrap().due.remove(0);
    assert_eq!(head.waits, 0);
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
    assert_eq!(
        fail_pending(&facade, &settings(), TO_LOCAL, "unpaired", now_millis()).unwrap(),
        0
    );
    assert_eq!(
        fail_pending(&facade, &settings(), TO_EDGE, "unpaired", now_millis()).unwrap(),
        2
    );
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

#[test]
fn content_is_kept_with_its_item_counted_in_the_cap_and_removed_with_it() {
    let (ctx, facade) = open();
    // the content's files are DbResource rows; the in-memory database has every table
    facade.assume_db_tables();
    let body = vec![b'x'; MAX_PAYLOAD_BYTES + 10];
    let with_content = NewItem {
        content: Some(body.clone()),
        ..item("mail", "m-1")
    };
    let queued = enqueue(ctx.factory(), &facade, None, &with_content).unwrap();
    let row = facade
        .find(QUEUE_ITEM)
        .unwrap()
        .eq("queueItemId", Value::string(&queued.queue_item_id))
        .disable_authz()
        .one()
        .unwrap()
        .unwrap();
    let location = text(&row, "contentLocation");
    assert_eq!(location, format!("{CONTENT_OUT}{}", queued.queue_item_id));
    assert_eq!(
        number(&row, "payloadSize") as usize,
        body.len() + "mail:m-1".len(),
        "the content counts toward the cap"
    );
    let claimed = claim(
        &facade,
        &due_heads(&facade, &local_node_id(ctx.factory()), TO_EDGE, now_millis())
            .unwrap()
            .due[0],
        "test",
        now_millis(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(read_content(&facade, &claimed).unwrap(), Some(body.clone()));
    // only this node's own copy is read, and too much content is refused at enqueue
    let elsewhere = QueueItem {
        content_location: "dbresource://moqui/rich-text/x.png".to_string(),
        ..claimed.clone()
    };
    assert!(read_content(&facade, &elsewhere).is_err());
    let too_big = NewItem {
        content: Some(vec![0; MAX_CONTENT_BYTES + 1]),
        ..item("mail", "m-2")
    };
    assert!(enqueue(ctx.factory(), &facade, None, &too_big).is_err());

    // the receiver's copy: by node and item id, which must be plain names
    let kept = store_received(&facade, "edge-1", &queued.queue_item_id, &body).unwrap();
    assert!(kept.starts_with(&format!("{CONTENT_IN}edge-1/{}.", queued.queue_item_id)));
    assert!(is_received("edge-1", &queued.queue_item_id, &kept));
    let again = store_received(&facade, "edge-1", &queued.queue_item_id, &body).unwrap();
    assert_ne!(
        again, kept,
        "each upload is its own, so a late apply removes only its own"
    );
    assert!(store_received(&facade, "edge-1", "../out/x", b"x").is_err());
    assert!(received_location("..", "x").is_err());
    // a delivery naming content this node did not keep for that item is refused, before any handler runs
    for wrong in [
        location.clone(),
        format!("{CONTENT_IN}edge-2/{}.x", queued.queue_item_id),
        format!("{CONTENT_IN}edge-1/other.x"),
        format!("{CONTENT_IN}edge-1/{}./x", queued.queue_item_id),
    ] {
        let refused = apply(
            ctx.factory(),
            "edge-1",
            &QueueItem {
                content_location: wrong.clone(),
                ..claimed.clone()
            },
        );
        assert!(
            matches!(&refused, Applied::Failed(error) if error.contains("not where this node keeps")),
            "{wrong}: {refused:?}"
        );
    }
    assert!(remove_content(&facade, &again, "the test"));

    // delivered, then past the retention period: the item goes with its content, and the receiver's copy that was
    // never answered goes too
    finish(
        &facade,
        &settings(),
        &claimed,
        &Outcome::Answered(Applied::Applied),
        now_millis(),
    )
    .unwrap();
    let later = now_millis() + 15 * 86_400_000;
    let cleaned = clean(&facade, &settings(), later).unwrap();
    assert_eq!(cleaned.removed, 1);
    assert_eq!(facade.read_db_resource_bytes(&location).unwrap(), None);
    assert_eq!(facade.read_db_resource_bytes(&kept).unwrap(), None);
}

/// An item that fails for good tells its kind's failed-service on the sender, so the failure is not silent.
#[test]
fn a_kind_with_a_failed_service_is_told_when_its_item_fails_for_good() {
    let (ctx, facade) = open();
    let path = std::env::temp_dir().join(format!("moqui-edge-failed-{}.xml", std::process::id()));
    std::fs::write(
        &path,
        r#"<services><service verb="fail" noun="EdgeQueueTest">
            <in-parameters><parameter name="queueItemId"/><parameter name="kind"/><parameter name="conversationKey"/>
                <parameter name="payload"/><parameter name="error"/></in-parameters>
            <actions><script>ec.message.addMessage('told ' + kind + ' ' + payload + ': ' + error)</script></actions>
        </service></services>"#,
    )
    .unwrap();
    let _entered = ctx.enter();
    crate::service::lint(std::slice::from_ref(&path)).unwrap();
    let told = EdgeSettings {
        failed: BTreeMap::from([(KIND.to_string(), "fail#EdgeQueueTest".to_string())]),
        ..settings()
    };
    enqueue(ctx.factory(), &facade, None, &item("a", "1")).unwrap();
    assert_eq!(
        fail_pending(&facade, &told, TO_EDGE, "unpaired", now_millis()).unwrap(),
        1
    );
    let messages = ctx.message().messages().to_vec();
    assert!(
        messages.iter().any(|line| line == "told test a:1: unpaired"),
        "{messages:?}"
    );
}
