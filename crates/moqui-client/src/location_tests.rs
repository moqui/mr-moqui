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

//! The reports of a share sent from this device (plans/Location-Sharing.md §3.4, LL3): the newest point only, 429
//! waiting, a stale point dropped, a device clock off sent with the server's time, and a 410, 404, another device or a
//! sign-out stopping it.

use std::cell::RefCell;

use serde_json::{json, Value};

use super::*;

const INTERVAL: i64 = 5_000;

fn fix(latitude: f64, observed_ms: i64) -> Fix {
    Fix {
        latitude,
        longitude: -111.9,
        accuracy: Some(8.0),
        heading: None,
        speed: Some(1.5),
        observed_ms,
    }
}

/// What the server answers a post: its HTTP status and JSON, or no answer.
type Answer = Result<(u16, Value), String>;

/// The forms posted, and the answer each gets.
struct Server {
    posted: RefCell<Vec<Vec<(String, String)>>>,
}

impl Server {
    fn new() -> Self {
        Self {
            posted: RefCell::new(Vec::new()),
        }
    }

    fn answer(&self, status: u16, json: Value) -> impl FnOnce(&[(String, String)]) -> Answer + '_ {
        move |form| {
            self.posted.borrow_mut().push(form.to_vec());
            Ok((status, json))
        }
    }

    fn field(&self, index: usize, name: &str) -> Option<String> {
        self.posted.borrow()[index]
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.clone())
    }

    fn count(&self) -> usize {
        self.posted.borrow().len()
    }
}

fn ok() -> Value {
    json!({ "statusCode": 200 })
}

#[test]
fn only_the_newest_point_waiting_is_sent() {
    let server = Server::new();
    let mut reporter = Reporter::new("100", INTERVAL);
    assert_eq!(
        reporter.report(Some(fix(40.1, 1_000)), 1_000, server.answer(200, ok())),
        Outcome::Sent
    );
    // two fixes inside the interval: neither goes, the newer takes the older's place
    assert_eq!(
        reporter.report(Some(fix(40.2, 2_000)), 2_000, server.answer(200, ok())),
        Outcome::Waiting(4_000)
    );
    assert_eq!(
        reporter.report(Some(fix(40.3, 3_000)), 3_000, server.answer(200, ok())),
        Outcome::Waiting(3_000)
    );
    // an older fix than the one waiting is not kept
    reporter.offer(fix(40.25, 2_500));
    assert_eq!(server.count(), 1);
    // the interval over: the newest point alone goes, with its device time
    assert_eq!(reporter.report(None, 6_000, server.answer(200, ok())), Outcome::Sent);
    assert_eq!(server.count(), 2);
    assert_eq!(server.field(1, "latitude").as_deref(), Some("40.3"));
    assert_eq!(server.field(1, "observedDate").as_deref(), Some("3000"));
    assert_eq!(server.field(1, "shareId").as_deref(), Some("100"));
    assert_eq!(server.field(1, "heading"), None);
    // nothing left
    assert_eq!(reporter.report(None, 20_000, server.answer(200, ok())), Outcome::Idle);
    // a fix older than the last one sent is never sent
    assert_eq!(
        reporter.report(Some(fix(40.4, 2_900)), 21_000, server.answer(200, ok())),
        Outcome::Idle
    );
    assert_eq!(server.count(), 2);
}

#[test]
fn a_429_waits_its_retry_after_and_sends_the_newest_then() {
    let server = Server::new();
    let mut reporter = Reporter::new("100", INTERVAL);
    let too_soon = json!({ "statusCode": 429, "reasonCode": "RATE_LIMITED", "retryAfterSeconds": 3 });
    assert_eq!(
        reporter.report(Some(fix(40.1, 1_000)), 1_000, server.answer(200, too_soon)),
        Outcome::Waiting(3_000)
    );
    // still waiting: nothing posted, and a newer fix takes the place of the refused one
    assert_eq!(
        reporter.report(Some(fix(40.2, 2_000)), 2_000, server.answer(200, ok())),
        Outcome::Waiting(2_000)
    );
    assert_eq!(server.count(), 1);
    assert_eq!(reporter.report(None, 4_000, server.answer(200, ok())), Outcome::Sent);
    assert_eq!(server.field(1, "latitude").as_deref(), Some("40.2"));
}

#[test]
fn a_stale_point_is_dropped_and_not_sent_again() {
    let server = Server::new();
    let mut reporter = Reporter::new("100", INTERVAL);
    let stale = json!({ "statusCode": 409, "reasonCode": "STALE" });
    assert_eq!(
        reporter.report(Some(fix(40.1, 1_000)), 1_000, server.answer(200, stale)),
        Outcome::Dropped
    );
    assert_eq!(reporter.report(None, 10_000, server.answer(200, ok())), Outcome::Idle);
    // and so is a value the server refuses
    let refused = json!({ "statusCode": 400, "reasonCode": "BAD_ACCURACY" });
    assert_eq!(
        reporter.report(Some(fix(40.2, 11_000)), 11_000, server.answer(200, refused)),
        Outcome::Dropped
    );
    assert_eq!(server.count(), 2);
}

#[test]
fn a_device_clock_off_sends_with_the_servers_time_from_then_on() {
    for reason in [(409, "CLOCK_AHEAD"), (400, "BEFORE_SHARE")] {
        let server = Server::new();
        let mut reporter = Reporter::new("100", INTERVAL);
        let off = json!({ "statusCode": reason.0, "reasonCode": reason.1 });
        assert_eq!(
            reporter.report(Some(fix(40.1, 1_000)), 1_000, server.answer(200, off)),
            Outcome::Waiting(0),
            "{reason:?}"
        );
        assert_eq!(server.field(0, "observedDate").as_deref(), Some("1000"));
        // the same point again at once, without the device's time
        assert_eq!(reporter.report(None, 1_000, server.answer(200, ok())), Outcome::Sent);
        assert_eq!(server.field(1, "latitude").as_deref(), Some("40.1"));
        assert_eq!(server.field(1, "observedDate"), None, "{reason:?}");
        assert_eq!(
            reporter.report(Some(fix(40.2, 7_000)), 7_000, server.answer(200, ok())),
            Outcome::Sent
        );
        assert_eq!(server.field(2, "observedDate"), None, "{reason:?}");
    }
}

#[test]
fn a_410_an_unknown_share_another_device_or_a_sign_out_stops_it() {
    let cases = [
        (
            200,
            json!({ "statusCode": 410, "reasonCode": "SHARE_ENDED" }),
            Stopped::Ended,
        ),
        (
            200,
            json!({ "statusCode": 404, "reasonCode": "SHARE_NOT_FOUND" }),
            Stopped::NotFound,
        ),
        (
            200,
            json!({ "statusCode": 409, "reasonCode": "OTHER_DEVICE" }),
            Stopped::OtherDevice,
        ),
        (401, json!({ "errors": "Authentication required" }), Stopped::SignedOut),
        (403, Value::Null, Stopped::SignedOut),
        // a sign-in page, not the transition's JSON
        (200, Value::Null, Stopped::SignedOut),
    ];
    for (http, answer, why) in cases {
        let server = Server::new();
        let mut reporter = Reporter::new("100", INTERVAL);
        assert_eq!(
            reporter.report(Some(fix(40.1, 1_000)), 1_000, server.answer(http, answer)),
            Outcome::Stopped(why)
        );
        assert_eq!(reporter.stopped(), Some(why));
        // nothing more is sent, a new fix included
        assert_eq!(
            reporter.report(Some(fix(40.2, 9_000)), 9_000, server.answer(200, ok())),
            Outcome::Stopped(why)
        );
        assert_eq!(server.count(), 1, "{why:?}");
    }
}

#[test]
fn no_answer_keeps_the_point_for_the_next_try() {
    let mut reporter = Reporter::new("100", INTERVAL);
    let outcome = reporter.report(Some(fix(40.1, 1_000)), 1_000, |_| Err("no network".to_string()));
    assert_eq!(outcome, Outcome::Waiting(INTERVAL));
    let server = Server::new();
    assert_eq!(
        reporter.report(None, 3_000, server.answer(200, ok())),
        Outcome::Waiting(3_000)
    );
    assert_eq!(reporter.report(None, 6_000, server.answer(200, ok())), Outcome::Sent);
    assert_eq!(server.field(0, "latitude").as_deref(), Some("40.1"));
}

#[test]
fn a_fix_is_read_from_the_native_codes_json_and_a_bad_one_is_none() {
    let read = Fix::from_json(
        &json!({ "latitude": 40.5, "longitude": -111.8, "accuracy": 12.0, "heading": 90.0,
        "speed": 2.0, "time": 1_700_000_000_000_i64 }),
    )
    .unwrap();
    assert_eq!(read.accuracy, Some(12.0));
    assert_eq!(read.observed_ms, 1_700_000_000_000);
    assert!(Fix::from_json(&json!({ "latitude": 91.0, "longitude": 0.0, "time": 1 })).is_none());
    assert!(Fix::from_json(&json!({ "latitude": 1.0, "longitude": 0.0 })).is_none());
    // a heading the device does not know is left out
    let no_heading = Fix::from_json(&json!({ "latitude": 1.0, "longitude": 2.0, "heading": -1.0, "time": 5 })).unwrap();
    assert_eq!(no_heading.heading, None);
    assert_eq!(Outcome::Stopped(Stopped::Ended).to_json()["reason"], "ended");
    assert_eq!(Outcome::Waiting(3_000).to_json()["waitMs"], 3_000);
}
