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

//! The phone's side of live location (plans/Location-Sharing.md §3.4): the page's ask checked, and `report_location`
//! with the account's key through a stand-in poster. Each test has accounts of its own: the reporters are the process's.

use std::cell::RefCell;

use moqui_client::account::Account;
use serde_json::{json, Value};

use super::*;

/// A post made: its path and its form.
type Post = (String, Vec<(String, String)>);

/// The posts made, and the answer each gets.
struct Posts {
    made: RefCell<Vec<Post>>,
    answer: RefCell<Vec<(u16, Value)>>,
}

impl Posts {
    fn answering(answers: Vec<(u16, Value)>) -> Self {
        Self {
            made: RefCell::new(Vec::new()),
            answer: RefCell::new(answers),
        }
    }

    fn field(&self, index: usize, name: &str) -> Option<String> {
        self.made.borrow()[index]
            .1
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.clone())
    }
}

impl Poster for Posts {
    fn post(&self, _account: &Account, key: &str, path: &str, form: &[(&str, &str)]) -> Result<(u16, Value), String> {
        assert_eq!(key, "device-key", "the account's device key signs it");
        self.made.borrow_mut().push((
            path.to_string(),
            form.iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
        ));
        let mut answers = self.answer.borrow_mut();
        Ok(if answers.is_empty() {
            (200, json!({ "statusCode": 200 }))
        } else {
            answers.remove(0)
        })
    }
}

fn account(id: &str) -> Account {
    Account {
        id: id.to_string(),
        server: "https://moqui.example.com".to_string(),
        username: "ann".to_string(),
        user_id: "ANN".to_string(),
        display: "Ann".to_string(),
        server_name: String::new(),
        device_mode: None,
    }
}

fn fix(latitude: f64, time: i64) -> String {
    json!({ "latitude": latitude, "longitude": -111.9, "accuracy": 9.0, "time": time }).to_string()
}

fn state(outcome: &str) -> Value {
    serde_json::from_str(outcome).unwrap()
}

#[test]
fn the_pages_ask_is_checked() {
    let now = 1_700_000_000_000;
    let ask = ShareAsk {
        share_id: " 100123 ".to_string(),
        thru_date: now + 3_600_000,
        approximate: false,
        interval_seconds: 5,
        audience: "Sam\nand 2 others".to_string(),
    };
    let checked = ask.clone().checked(now).unwrap();
    assert_eq!(checked.share_id, "100123");
    assert_eq!(checked.audience, "Sam and 2 others");
    let json: Value = serde_json::from_str(&checked.to_json("acct")).unwrap();
    assert_eq!(json["account"], "acct");
    assert_eq!(json["intervalSeconds"], 5);
    for bad in [
        ShareAsk {
            share_id: "1;drop".to_string(),
            ..ask.clone()
        },
        ShareAsk {
            share_id: String::new(),
            ..ask.clone()
        },
        ShareAsk {
            thru_date: now - 1,
            ..ask.clone()
        },
        ShareAsk {
            thru_date: now + 30 * 24 * 3_600_000,
            ..ask.clone()
        },
        ShareAsk {
            interval_seconds: 0,
            ..ask.clone()
        },
    ] {
        assert!(bad.checked(now).is_err());
    }
}

#[test]
fn a_report_sends_the_newest_point_with_the_device_key_and_a_410_stops_it() {
    let ann = account("location-test-newest");
    let posts = Posts::answering(vec![
        (200, json!({ "statusCode": 200 })),
        (200, json!({ "statusCode": 410, "reasonCode": "SHARE_ENDED" })),
    ]);
    begin(&ann.id, "200", 5_000);
    assert_eq!(sending(&ann.id), vec!["200".to_string()]);
    let signed_in = Some((&ann, "device-key"));
    let sent = report_location(&ann.id, "200", Some(&fix(40.1, 1_000)), signed_in, &posts, 1_000);
    assert_eq!(state(&sent)["state"], "sent");
    assert_eq!(posts.made.borrow()[0].0, moqui_client::location::REPORT_PATH);
    // two fixes inside the interval: only the newer goes, at the interval's end
    let waiting = report_location(&ann.id, "200", Some(&fix(40.2, 2_000)), signed_in, &posts, 2_000);
    assert_eq!(state(&waiting)["state"], "waiting");
    report_location(&ann.id, "200", Some(&fix(40.3, 3_000)), signed_in, &posts, 3_000);
    assert_eq!(posts.made.borrow().len(), 1);
    let ended = report_location(&ann.id, "200", None, signed_in, &posts, 6_000);
    assert_eq!(posts.field(1, "latitude").as_deref(), Some("40.3"));
    assert_eq!(state(&ended), json!({ "state": "stopped", "reason": "ended" }));
    // gone: nothing more is sent
    assert!(sending(&ann.id).is_empty());
    let after = report_location(&ann.id, "200", Some(&fix(40.4, 9_000)), signed_in, &posts, 9_000);
    assert_eq!(state(&after)["state"], "stopped");
    assert_eq!(posts.made.borrow().len(), 2);
}

#[test]
fn an_account_not_signed_in_or_signed_out_stops_its_shares() {
    let ann = account("location-test-signed-out");
    let posts = Posts::answering(Vec::new());
    begin(&ann.id, "300", 5_000);
    let stopped = report_location(&ann.id, "300", Some(&fix(40.1, 1_000)), None, &posts, 1_000);
    assert_eq!(state(&stopped), json!({ "state": "stopped", "reason": "signedOut" }));
    assert!(posts.made.borrow().is_empty());
    begin(&ann.id, "301", 5_000);
    begin(&ann.id, "302", 5_000);
    let mut gone = forget_account(&ann.id);
    gone.sort();
    assert_eq!(gone, vec!["301".to_string(), "302".to_string()]);
    let after = report_location(
        &ann.id,
        "301",
        Some(&fix(40.1, 2_000)),
        Some((&ann, "device-key")),
        &posts,
        2_000,
    );
    assert_eq!(state(&after)["state"], "stopped");
    assert!(posts.made.borrow().is_empty());
}

#[test]
fn the_notifications_stop_stops_the_share_on_the_server() {
    let ann = account("location-test-stop");
    let posts = Posts::answering(vec![(200, json!({ "stopped": true }))]);
    begin(&ann.id, "400", 5_000);
    assert!(stop_share(&ann.id, "400", true, Some((&ann, "device-key")), &posts));
    assert_eq!(posts.made.borrow()[0].0, moqui_client::location::STOP_PATH);
    assert_eq!(posts.field(0, "shareId").as_deref(), Some("400"));
    assert!(sending(&ann.id).is_empty());
    // the page's own stop tells the server nothing; signed out, nothing can be told
    begin(&ann.id, "401", 5_000);
    assert!(stop_share(&ann.id, "401", false, Some((&ann, "device-key")), &posts));
    assert!(!stop_share(&ann.id, "402", true, None, &posts));
    assert_eq!(posts.made.borrow().len(), 1);
}
