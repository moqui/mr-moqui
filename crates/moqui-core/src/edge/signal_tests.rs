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

use std::sync::{Arc, Mutex};

use super::*;

fn message(body: serde_json::Value) -> TopicMessage {
    TopicMessage {
        origin_id: "test".to_string(),
        message_id: "m-1".to_string(),
        body,
    }
}

/// A route that keeps what it was handed.
fn keeping(kept: &Arc<Mutex<Vec<String>>>, answer: Result<(), String>) -> Sender {
    let kept = Arc::clone(kept);
    Arc::new(move |topic: &str, message: &TopicMessage| {
        lock(&kept).push(format!("{topic} {}", message.body));
        answer.clone()
    })
}

#[test]
fn a_route_takes_its_node_s_messages_until_its_guard_drops() {
    let kept = Arc::new(Mutex::new(Vec::new()));
    assert!(send("signal-test-a", RTC_SIGNAL, &message(serde_json::json!({}))).is_none());
    let guard = add_route("signal-test-a", keeping(&kept, Ok(())));
    let sent = send(
        "signal-test-a",
        RTC_SIGNAL,
        &message(serde_json::json!({"target": "signal-test-a"})),
    );
    assert!(matches!(sent, Some(Ok(()))));
    assert_eq!(*lock(&kept), [r#"rtc-signal {"target":"signal-test-a"}"#]);
    // another node's message is not this route's
    assert!(send("signal-test-other", RTC_SIGNAL, &message(serde_json::json!({}))).is_none());
    drop(guard);
    assert!(send("signal-test-a", RTC_SIGNAL, &message(serde_json::json!({}))).is_none());
}

#[test]
fn a_newer_route_is_kept_when_the_older_guard_drops() {
    let old = Arc::new(Mutex::new(Vec::new()));
    let new = Arc::new(Mutex::new(Vec::new()));
    let first = add_route("signal-test-b", keeping(&old, Ok(())));
    let second = add_route("signal-test-b", keeping(&new, Err("overloaded".to_string())));
    drop(first);
    let sent = send("signal-test-b", RTC_SIGNAL, &message(serde_json::json!({"n": 1})));
    assert_eq!(
        sent.map(|sent| sent.map_err(|err| err.message().to_string())),
        Some(Err("overloaded".to_string()))
    );
    assert!(lock(&old).is_empty());
    assert_eq!(lock(&new).len(), 1);
    drop(second);
    assert!(route("signal-test-b").is_none());
}

#[test]
fn only_the_link_s_topics_are_received() {
    let refused = receive("entity-cache-invalidate", &message(serde_json::json!({})));
    assert!(refused.is_err_and(|err| err.message().contains("no messages on the topic")));
}
