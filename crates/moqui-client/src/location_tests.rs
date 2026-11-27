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

//! The reports of a share sent from this device (plans/archive/Location-Sharing.md §3.4, LL3): the newest point only, 429
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

#[test]
fn a_408_or_a_429_that_is_not_the_transitions_waits_and_tries_again() {
    let server = Server::new();
    let mut reporter = Reporter::new("100", INTERVAL);
    // a proxy's 429 naming its wait (Retry-After, as post_form gives it)
    let busy = json!({ RETRY_AFTER: 7 });
    assert_eq!(
        reporter.report(Some(fix(40.1, 1_000)), 1_000, server.answer(429, busy)),
        Outcome::Waiting(7_000)
    );
    // with no wait named: half a minute
    assert_eq!(
        reporter.report(None, 8_000, server.answer(429, Value::Null)),
        Outcome::Waiting(30_000)
    );
    // a 408: after the interval
    assert_eq!(
        reporter.report(None, 38_000, server.answer(408, Value::Null)),
        Outcome::Waiting(INTERVAL)
    );
    assert_eq!(reporter.report(None, 43_000, server.answer(200, ok())), Outcome::Sent);
    assert_eq!(server.count(), 4);
    assert_eq!(server.field(3, "latitude").as_deref(), Some("40.1"));
    assert_eq!(reporter.stopped(), None);
}

#[test]
fn another_device_across_a_key_renewal_is_tried_again_once() {
    let mut reporter = Reporter::new("100", INTERVAL);
    let other = || Ok((200, json!({ "statusCode": 409, "reasonCode": "OTHER_DEVICE" })));
    reporter.offer(fix(40.1, 1_000));
    let Next::Send(sent, _) = reporter.next(1_000) else {
        panic!("nothing to send")
    };
    // the key changed while it was posted: once more at once, with the new key
    assert_eq!(reporter.answered_after(sent, other(), 1_000, true), Outcome::Waiting(0));
    let Next::Send(again, form) = reporter.next(1_000) else {
        panic!("not sent again")
    };
    assert_eq!(again, sent);
    assert!(form.iter().any(|(name, value)| name == "latitude" && value == "40.1"));
    // refused again: another device sends it
    assert_eq!(
        reporter.answered_after(again, other(), 1_000, true),
        Outcome::Stopped(Stopped::OtherDevice)
    );
    // with the key unchanged it stops at the first
    let mut plain = Reporter::new("101", INTERVAL);
    plain.offer(fix(40.1, 1_000));
    let Next::Send(sent, _) = plain.next(1_000) else {
        panic!("nothing to send")
    };
    assert_eq!(
        plain.answered_after(sent, other(), 1_000, false),
        Outcome::Stopped(Stopped::OtherDevice)
    );
}

/// A server on loopback that reads a request (up to its form) and answers `reply`: its base URL, and what it read.
fn answering(reply: &'static [u8]) -> (String, std::thread::JoinHandle<String>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut seen = Vec::new();
        let mut buffer = [0_u8; 4096];
        while !String::from_utf8_lossy(&seen).contains("latitude=") {
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            seen.extend_from_slice(&buffer[..read]);
        }
        stream.write_all(reply).unwrap();
        String::from_utf8_lossy(&seen).to_string()
    });
    (base, server)
}

/// A post that gets no answer gives up within its timeout, not the proxy's ten minutes; one answered gives its JSON,
/// and the `Retry-After` of one that is not JSON.
#[test]
fn a_report_gives_up_at_its_timeout_and_reads_the_answer() {
    let proxy = crate::proxy::Proxy::new().unwrap();
    // a server that takes the connection and never answers (half-open)
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", silent.local_addr().unwrap());
    let held = std::thread::spawn(move || {
        let (stream, _) = silent.accept().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1_500));
        drop(stream);
    });
    let started = std::time::Instant::now();
    let quick = std::time::Duration::from_millis(500);
    let answer = post_form(&proxy, &base, "k", REPORT_PATH, &[("shareId", "100")], quick);
    assert!(answer.is_err(), "{answer:?}");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(1_200),
        "{:?}",
        started.elapsed()
    );
    held.join().unwrap();
    // an answer: its JSON, and the form as posted with the key
    let (base, server) = answering(
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 18\r\nConnection: close\r\n\r\n{\"statusCode\":200}",
    );
    let answer = post_form(
        &proxy,
        &base,
        "k",
        REPORT_PATH,
        &[("shareId", "100"), ("latitude", "40.5")],
        POST_TIMEOUT,
    );
    let seen = server.join().unwrap();
    assert_eq!(answer.unwrap(), (200, json!({ "statusCode": 200 })));
    assert!(seen.starts_with("POST /qapps/reportClientLocation"), "{seen}");
    assert!(seen.to_ascii_lowercase().contains("login_key: k"), "{seen}");
    let (base, server) = answering(
        b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 12\r\nContent-Length: 4\r\nConnection: close\r\n\r\nbusy",
    );
    let answer = post_form(&proxy, &base, "k", REPORT_PATH, &[("latitude", "40.5")], POST_TIMEOUT);
    server.join().unwrap();
    assert_eq!(answer.unwrap(), (429, json!({ RETRY_AFTER: 12 })));
}

/// An answer whose body stalls after its headers gives up at the timeout too: the timeout covers the body's read.
#[test]
fn a_report_whose_answer_stalls_after_its_headers_gives_up_at_its_timeout() {
    use std::io::{Read, Write};
    let proxy = crate::proxy::Proxy::new().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut seen = Vec::new();
        let mut buffer = [0_u8; 4096];
        while !String::from_utf8_lossy(&seen).contains("latitude=") {
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            seen.extend_from_slice(&buffer[..read]);
        }
        // the headers and the body's start, then nothing
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{\"stat")
            .unwrap();
        stream.flush().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1_500));
        drop(stream);
    });
    let started = std::time::Instant::now();
    let quick = std::time::Duration::from_millis(500);
    let answer = post_form(&proxy, &base, "k", REPORT_PATH, &[("latitude", "40.5")], quick);
    assert!(answer.is_err(), "{answer:?}");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(1_200),
        "{:?}",
        started.elapsed()
    );
    server.join().unwrap();
}

#[test]
fn each_report_carries_the_confirmed_audience_version_and_a_changed_audience_stops_it() {
    let server = Server::new();
    let mut reporter = Reporter::new("100", INTERVAL);
    reporter.set_audience_version(4);
    assert_eq!(
        reporter.report(Some(fix(40.1, 1_000)), 1_000, server.answer(200, ok())),
        Outcome::Sent
    );
    assert_eq!(server.field(0, "audienceVersion").as_deref(), Some("4"));
    let changed = json!({ "statusCode": 409, "reasonCode": "AUDIENCE_CHANGED" });
    assert_eq!(
        reporter.report(Some(fix(40.2, 7_000)), 7_000, server.answer(200, changed)),
        Outcome::Stopped(Stopped::AudienceChanged)
    );
    assert_eq!(
        Outcome::Stopped(Stopped::AudienceChanged).to_json()["reason"],
        "audienceChanged"
    );
    // with no version set none is sent (a browser's share, never the app's)
    let plain = Server::new();
    let mut browser = Reporter::new("101", INTERVAL);
    browser.report(Some(fix(40.1, 1_000)), 1_000, plain.answer(200, ok()));
    assert_eq!(plain.field(0, "audienceVersion"), None);
}
