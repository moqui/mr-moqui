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

use std::collections::HashMap;

use super::*;

fn healths(count: usize) -> Vec<Health> {
    (0..count).map(|_| Health::default()).collect()
}

fn candidates<'a>(members: &[&'a str], healths: &'a [Health]) -> Vec<Candidate<'a>> {
    members
        .iter()
        .zip(healths)
        .map(|(member, health)| Candidate { member, health })
        .collect()
}

/// The member `affinity` chooses among `members`, all healthy.
fn chosen<'a>(members: &[&'a str], affinity: Affinity<'_>) -> &'a str {
    let healths = healths(members.len());
    let index = choose(&candidates(members, &healths), affinity, 0, Instant::now()).unwrap();
    members[index]
}

fn rules(failures: u32, aside_for: Duration) -> Rules {
    Rules {
        sticky_cookie: Arc::from("moqui.session"),
        failures,
        aside_for,
    }
}

#[test]
fn a_cookie_stays_on_one_member_whatever_the_order_of_the_list() {
    let members = ["member-a", "member-b", "member-c"];
    for cookie in ["abc.123", "def.456", "a-session-of-its-own"] {
        let first = chosen(&members, Affinity::Key(cookie));
        assert_eq!(
            chosen(&members, Affinity::Key(cookie)),
            first,
            "the same cookie, the same member"
        );
        for order in [
            ["member-c", "member-a", "member-b"],
            ["member-b", "member-c", "member-a"],
        ] {
            assert_eq!(
                chosen(&order, Affinity::Key(cookie)),
                first,
                "another edge member's list order"
            );
        }
    }
    // the weight is SHA-256's, the same in every build and on every edge node
    assert_eq!(weight("member-a", "abc"), weight("member-a", "abc"));
    assert_ne!(weight("member-a", "abc"), weight("member-b", "abc"));
    assert_ne!(
        weight("ab", "c"),
        weight("a", "bc"),
        "the id and the key are kept apart"
    );
}

#[test]
fn a_member_that_goes_moves_only_its_own_cookies_and_they_spread_evenly() {
    let all = ["member-a", "member-b", "member-c"];
    let cookies: Vec<String> = (0..3000).map(|n| format!("session-{n}")).collect();
    let before: Vec<&str> = cookies
        .iter()
        .map(|cookie| chosen(&all, Affinity::Key(cookie)))
        .collect();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for member in &before {
        *counts.entry(member).or_default() += 1;
    }
    for member in all {
        let count = counts[member];
        assert!((800..1200).contains(&count), "{member} has {count} of 3000");
    }
    let left = ["member-a", "member-c"];
    for (cookie, was) in cookies.iter().zip(&before) {
        let now = chosen(&left, Affinity::Key(cookie));
        if *was == "member-b" {
            assert_ne!(now, "member-b");
        } else {
            assert_eq!(now, *was, "{cookie} moved though its member stayed");
        }
    }
}

#[test]
fn with_no_cookie_the_members_take_turns() {
    let members = ["member-a", "member-b"];
    let healths = healths(2);
    let list = candidates(&members, &healths);
    let now = Instant::now();
    let turns: Vec<usize> = (0..4)
        .map(|turn| choose(&list, Affinity::Any, turn, now).unwrap())
        .collect();
    assert_eq!(turns, [0, 1, 0, 1]);
    assert_eq!(choose(&[], Affinity::Any, 0, now), None, "no member: none");
}

#[test]
fn a_passthrough_client_is_sticky_by_its_address_and_an_ipv6_one_by_its_64() {
    let members = ["member-a", "member-b", "member-c", "member-d"];
    let ip = |text: &str| text.parse::<IpAddr>().unwrap();
    let one = chosen(&members, Affinity::Address(ip("2001:db8:1:2:aaaa::1")));
    for same in ["2001:db8:1:2::9", "2001:db8:1:2:ffff:ffff:ffff:ffff"] {
        assert_eq!(
            chosen(&members, Affinity::Address(ip(same))),
            one,
            "{same} is in the same /64"
        );
    }
    assert_eq!(address_key(ip("2001:db8:1:2:aaaa::1")), "2001:db8:1:2::");
    assert_eq!(address_key(ip("::ffff:198.51.100.7")), "198.51.100.7");
    assert_eq!(
        chosen(&members, Affinity::Address(ip("198.51.100.7"))),
        chosen(&members, Affinity::Address(ip("::ffff:198.51.100.7")))
    );
    // other /64s and other IPv4 clients spread over the members
    let spread: std::collections::HashSet<&str> = (0..64)
        .map(|n| chosen(&members, Affinity::Address(ip(&format!("2001:db8:1:{n:x}::1")))))
        .collect();
    assert!(spread.len() > 1, "{spread:?}");
}

#[test]
fn the_fewest_in_flight_take_the_next_call() {
    let members = ["member-a", "member-b", "member-c"];
    let healths: Vec<Arc<Health>> = (0..3).map(|_| Arc::new(Health::default())).collect();
    let rules = Rules::default();
    let _a = [
        Flight::start(Arc::clone(&healths[0]), rules.clone()),
        Flight::start(Arc::clone(&healths[0]), rules.clone()),
    ];
    let _c = Flight::start(Arc::clone(&healths[2]), rules.clone());
    let list: Vec<Candidate<'_>> = members
        .iter()
        .zip(&healths)
        .map(|(member, health)| Candidate { member, health })
        .collect();
    let now = Instant::now();
    assert_eq!(choose(&list, Affinity::Fewest, 0, now), Some(1));
    drop(_c);
    // b and c have none: in turn
    assert_eq!(choose(&list, Affinity::Fewest, 1, now), Some(1));
    assert_eq!(choose(&list, Affinity::Fewest, 2, now), Some(2));
    assert_eq!(healths[0].now(now).in_flight, 2);
    assert_eq!(healths[0].now(now).requests, 2);
}

#[test]
fn a_member_failing_in_a_row_is_set_aside_then_tried_again() {
    let rules = rules(3, Duration::from_millis(200));
    let members = ["member-a", "member-b"];
    let healths: Vec<Arc<Health>> = (0..2).map(|_| Arc::new(Health::default())).collect();
    let list = || -> Vec<Candidate<'_>> {
        members
            .iter()
            .zip(&healths)
            .map(|(member, health)| Candidate { member, health })
            .collect()
    };
    let flight = Flight::start(Arc::clone(&healths[0]), rules.clone());
    // two failures, a success: not in a row
    assert!(!flight.failed());
    assert!(!flight.failed());
    flight.succeeded();
    assert!(!flight.failed());
    assert!(!flight.failed());
    assert!(flight.failed(), "the third in a row sets it aside");
    let now = Instant::now();
    assert!(healths[0].aside_at(now));
    assert!(!flight.failed(), "set aside once while it is aside");
    assert_eq!(healths[0].now(now).set_asides, 1);
    // its cookies go to the other member while it is aside, and with every member aside they are all tried
    let cookie = (0..100)
        .map(|n| format!("cookie-{n}"))
        .find(|cookie| chosen(&members, Affinity::Key(cookie)) == "member-a")
        .unwrap();
    assert_eq!(choose(&list(), Affinity::Key(&cookie), 0, now), Some(1));
    assert_eq!(choose(&list(), Affinity::Any, 0, now), Some(1));
    assert_eq!(choose(&list(), Affinity::Any, 1, now), Some(1));
    let other = Flight::start(Arc::clone(&healths[1]), rules.clone());
    for _ in 0..3 {
        other.failed();
    }
    assert_eq!(
        choose(&list(), Affinity::Key(&cookie), 0, Instant::now()),
        Some(0),
        "all aside: all tried"
    );
    other.succeeded();
    drop(other);
    *lock(&healths[1].aside_until) = None;
    // after its time it is tried again; a failure then sets it aside again at once, a success keeps it
    let later = now + Duration::from_millis(250);
    assert!(!healths[0].aside_at(later));
    assert_eq!(choose(&list(), Affinity::Key(&cookie), 0, later), Some(0));
    assert!(healths[0].failed(&rules, later), "tried again and failed: aside again");
    assert_eq!(healths[0].now(later).set_asides, 2);
    let after = later + Duration::from_millis(250);
    flight.succeeded();
    assert!(!healths[0].failed(&rules, after), "a success cleared its count");
    drop(flight);
    assert_eq!(healths[0].in_flight(), 0);
}

#[test]
fn a_sticky_key_longer_than_any_session_id_goes_in_turn() {
    let members = ["member-a", "member-b"];
    let healths = healths(2);
    let list = candidates(&members, &healths);
    let long = "x".repeat(MAX_STICKY_KEY + 1);
    let now = Instant::now();
    let turns: Vec<usize> = (0..4)
        .map(|turn| choose(&list, Affinity::Key(&long), turn, now).unwrap())
        .collect();
    assert_eq!(turns, [0, 1, 0, 1]);
    let fits = "x".repeat(MAX_STICKY_KEY);
    let first = choose(&list, Affinity::Key(&fits), 0, now);
    assert_eq!(
        choose(&list, Affinity::Key(&fits), 1, now),
        first,
        "one that fits stays"
    );
}
