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

//! A2A follow and resubscribe (plans/archive/High-Level-Review.md HL-014): resume after a stored sequence, a gap the hub
//! missed (a run that moved between nodes), a paused run, a client that left, and a stop or a deadline that ends a
//! stream or a blocking wait while nothing comes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use super::ops::{self, Follow};
use crate::run;

struct Bed {
    ctx: ExecutionContext,
    factory: Arc<ExecutionContextFactory>,
    facade: EntityFacade,
}

fn bed() -> Bed {
    let ctx = ExecutionContextFactory::test_context_with_conf("<moqui-conf><llm-facade/></moqui-conf>");
    let facade = moqui_core::testing::open();
    ctx.factory().set_entity(facade.clone());
    Bed {
        factory: Arc::clone(ctx.factory()),
        ctx,
        facade,
    }
}

/// A task of `user` in `state`, with no events yet.
fn task(bed: &Bed, run_id: &str, status_id: &str) {
    let mut row = bed.facade.make_value("moqui.agent.AgentRun").unwrap();
    for (field, text) in [
        ("runId", run_id),
        ("conversationId", "conv-follow"),
        ("userId", "follower"),
        ("surfaceEnumId", "AgsfA2a"),
        ("statusId", status_id),
    ] {
        row.set(field, Value::string(text));
    }
    row.create(&bed.facade).unwrap();
}

fn stored(bed: &Bed, run_id: &str, seq: i64, event_type: &str, payload: &Json) {
    let mut event = bed.facade.make_value(run::events::RUN_EVENT).unwrap();
    event.set("runId", Value::string(run_id));
    event.set("eventSeq", Value::I64(seq));
    event.set("eventType", Value::string(event_type));
    event.set("payloadJson", Value::string(payload.to_string()));
    event.create(&bed.facade).unwrap();
}

fn chunk(text: &str) -> Json {
    json!({"messageId": "m", "part": 0, "offset": 0, "text": text})
}

fn follow_of(run_id: &str, after: i64, close_on_pause: bool) -> Follow {
    Follow {
        run_id: run_id.to_string(),
        context_id: "conv-follow".to_string(),
        base_url: "https://moqui.example.com".to_string(),
        after,
        close_on_pause,
        text: true,
    }
}

/// The texts of the artifact updates `follow` sent, and what it returned.
fn texts(responses: &[Json]) -> Vec<String> {
    responses
        .iter()
        .filter_map(|response| response["artifactUpdate"]["artifact"]["parts"][0]["text"].as_str())
        .map(str::to_string)
        .collect()
}

#[test]
fn a_resubscribe_after_a_stored_sequence_sends_only_what_came_after_it() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    task(&bed, "follow-after", "AgrWorking");
    for (seq, text) in [(1, "one"), (2, "two"), (3, "three"), (4, "four")] {
        stored(&bed, "follow-after", seq, "text.chunk", &chunk(text));
    }
    stored(&bed, "follow-after", 5, "run.done", &json!({}));
    let mut sent = Vec::new();
    let last = ops::follow(
        &bed.factory,
        &follow_of("follow-after", 2, true),
        None,
        &|| false,
        &mut |response| {
            sent.extend(response);
            true
        },
    )
    .unwrap();
    assert_eq!(texts(&sent), ["three", "four"]);
    assert_eq!(last, 5);
}

#[test]
fn a_gap_the_hub_missed_is_filled_from_the_database_in_order() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    task(&bed, "follow-gap", "AgrWorking");
    stored(&bed, "follow-gap", 1, "text.chunk", &chunk("one"));
    // the run moved here from another node: its events 2 and 3 are stored, the hub gets only 4
    let writer = {
        let (factory, facade) = (Arc::clone(&bed.factory), bed.facade.clone());
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            let context = factory.new_context();
            let _entered = context.enter();
            for (seq, event_type, payload) in [
                (2, "text.chunk", chunk("two")),
                (3, "text.chunk", chunk("three")),
                (4, "run.done", json!({})),
            ] {
                let mut event = facade.make_value(run::events::RUN_EVENT).unwrap();
                event.set("runId", Value::string("follow-gap"));
                event.set("eventSeq", Value::I64(seq));
                event.set("eventType", Value::string(event_type));
                event.set("payloadJson", Value::string(payload.to_string()));
                event.create(&facade).unwrap();
            }
            run::engine(&factory)
                .hub()
                .push_stored("follow-gap", vec![(4, "run.done".to_string(), json!({}))]);
        })
    };
    let mut sent = Vec::new();
    let last = ops::follow(
        &bed.factory,
        &follow_of("follow-gap", 0, true),
        Some(Instant::now() + Duration::from_secs(10)),
        &|| false,
        &mut |response| {
            sent.extend(response);
            true
        },
    )
    .unwrap();
    writer.join().unwrap();
    assert_eq!(texts(&sent), ["one", "two", "three"]);
    assert_eq!(last, 4);
}

#[test]
fn a_pause_ends_a_stream_that_closes_on_pause_and_a_subscriber_goes_on_to_its_deadline() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    task(&bed, "follow-pause", "AgrInputRequired");
    stored(&bed, "follow-pause", 1, "text.chunk", &chunk("asked"));
    stored(
        &bed,
        "follow-pause",
        2,
        "run.state",
        &json!({"state": "TASK_STATE_INPUT_REQUIRED"}),
    );
    let mut sent = Vec::new();
    let started = Instant::now();
    let last = ops::follow(
        &bed.factory,
        &follow_of("follow-pause", 0, true),
        None,
        &|| false,
        &mut |response| {
            sent.extend(response);
            true
        },
    )
    .unwrap();
    assert_eq!(last, 2);
    assert_eq!(texts(&sent), ["asked"]);
    assert!(
        sent.iter().any(|response| response["statusUpdate"].is_object()),
        "{sent:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(1));

    // a subscriber of a task waiting for its owner goes on through the pause, to its deadline here
    let started = Instant::now();
    let last = ops::follow(
        &bed.factory,
        &follow_of("follow-pause", 0, false),
        Some(Instant::now() + Duration::from_millis(300)),
        &|| false,
        &mut |_| true,
    )
    .unwrap();
    assert_eq!(last, 2);
    let took = started.elapsed();
    assert!(
        took >= Duration::from_millis(300) && took < Duration::from_secs(2),
        "{took:?}"
    );
}

#[test]
fn a_client_that_left_or_a_stop_ends_the_follow_within_a_second() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    task(&bed, "follow-gone", "AgrWorking");
    stored(&bed, "follow-gone", 1, "text.chunk", &chunk("one"));
    stored(&bed, "follow-gone", 2, "text.chunk", &chunk("two"));
    // the client is gone at the first write
    let mut calls = 0;
    let last = ops::follow(
        &bed.factory,
        &follow_of("follow-gone", 0, true),
        None,
        &|| false,
        &mut |_| {
            calls += 1;
            false
        },
    )
    .unwrap();
    assert_eq!((calls, last), (1, 1));

    // nothing comes; the server stops (or the client closes) and the follow notices within its one-second check
    let stopped = Arc::new(AtomicBool::new(false));
    let stopper = {
        let stopped = Arc::clone(&stopped);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            stopped.store(true, Ordering::SeqCst);
        })
    };
    let started = Instant::now();
    let last = ops::follow(
        &bed.factory,
        &follow_of("follow-gone", 2, true),
        None,
        &|| stopped.load(Ordering::SeqCst),
        &mut |_| true,
    )
    .unwrap();
    stopper.join().unwrap();
    assert_eq!(last, 2);
    let took = started.elapsed();
    assert!(took < Duration::from_millis(1600), "{took:?}");
}

/// The blocking `message:send` wait follows the run emitting nothing; the host's stop (the client closed the request,
/// the server stops) ends it within a second rather than at `blocking-wait`.
#[test]
fn a_blocking_wait_ends_at_the_hosts_stop_not_its_whole_wait() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    task(&bed, "follow-wait", "AgrWorking");
    let task_row = ops::load_task(&bed.facade, "follow-wait").unwrap().unwrap();
    let stopped = Arc::new(AtomicBool::new(false));
    let stopper = {
        let stopped = Arc::clone(&stopped);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            stopped.store(true, Ordering::SeqCst);
        })
    };
    let started = Instant::now();
    let found = ops::wait_settled(
        &bed.factory,
        &task_row,
        "https://moqui.example.com",
        Duration::from_secs(60),
        &|| stopped.load(Ordering::SeqCst),
    )
    .unwrap();
    stopper.join().unwrap();
    assert_eq!(found.map(|task| task.run.state), Some(run::RunState::Working));
    // the follow stops within its one-second check, and a stopped wait does not wait for a step to finish its writes
    let took = started.elapsed();
    assert!(took < Duration::from_millis(1600), "{took:?}");
}
