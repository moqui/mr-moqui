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

use super::bins::{Bins, Series, Tally};
use super::pace::{Pause, Rng};
use super::script::Plan;
use super::stats::{grouped, result, Parts};
use super::*;

fn one_scenario(users: u64) -> Arc<Load> {
    let settings = ScenarioSettings::from_json("load", &json!({ "users": users, "iterations": 1 }), "load").unwrap();
    Load::new(&[settings])
}

fn ok(ms: u64) -> Observed<'static> {
    Observed {
        status: Some(200),
        text: "",
        elapsed: Duration::from_millis(ms),
        expect: None,
    }
}

fn result_of(load: &Load, thresholds: &Value) -> (Value, Vec<String>) {
    let parts = Parts {
        users: load.users(),
        wall: Duration::from_secs(1),
        thresholds,
        generator: "same-machine",
        cpu: None,
        run: json!({}),
    };
    let shared = lock(&load.shared);
    result(&shared, &parts)
}

#[test]
fn a_load_user_counts_its_requests_errors_and_throws_into_the_stats() {
    let load = one_scenario(2);
    let user = {
        let load = Arc::clone(&load);
        std::thread::spawn(move || {
            begin(Some(load), 0, 0, None);
            // setup is not timed
            assert!(observe(|| "GET /setup".to_string(), Kind::Http, &ok(5)).is_ok());
            assert!(!timed(true));
            assert!(observe(|| default_name("get", "/rest/s1/a?x=1"), Kind::Http, &ok(2)).is_ok());
            assert!(observe(|| "b".to_string(), Kind::Http, &ok(4)).is_ok());
            let refused = Observed {
                status: Some(429),
                text: "too many",
                elapsed: Duration::from_millis(6),
                expect: None,
            };
            let err = observe(|| "b".to_string(), Kind::Http, &refused).unwrap_err();
            assert_eq!(err, "load: b: HTTP 429: too many");
            // the throw the counted error causes is not counted again; a throw of the scenario's own is
            assert!(!scenario_error(&err, 0));
            assert!(!iterated(1));
            assert!(!scenario_error("Error: boom", 1));
            let expected = Observed {
                status: Some(401),
                text: "",
                elapsed: Duration::from_millis(8),
                expect: Some(vec![401]),
            };
            assert!(observe(|| "b".to_string(), Kind::Http, &expected).is_ok());
            let transport = Observed {
                status: None,
                text: "connection refused",
                elapsed: Duration::from_millis(1),
                expect: None,
            };
            assert!(observe(|| "c".to_string(), Kind::Raw, &transport).is_err());
            // a step: its own sample, with the requests inside under their names
            step_begin();
            assert!(observe(|| "c".to_string(), Kind::Raw, &ok(3)).is_ok());
            step_end("a step", None);
            assert!(!iterated(2));
            timed(false);
            assert_eq!(end(), Vec::<u64>::new());
        })
    };
    user.join().unwrap();
    lock(&load.shared).user_error(0, 1, "Error: login of test.load failed: HTTP 401");
    let limits = json!({ "errorRate": 0.4, "p95": 100, "names": { "b": { "p95": 100 }, "missing": { "p95": 1 } } });
    let (stats, order) = result_of(&load, &limits);
    load.unregister();
    assert_eq!(order, vec!["GET /rest/s1/a", "b", "c", "a step"]);
    assert_eq!(stats["users"], json!(2));
    assert_eq!(stats["iterations"], json!(2));
    // the step is not a request
    assert_eq!(stats["requests"], json!(6));
    // the 429, the throw, the transport error and the user whose setup failed
    assert_eq!(stats["errors"], json!(4));
    // per attempt: 6 requests and the 2 errors that were not requests'
    assert_eq!(stats["errorRate"], json!(0.5));
    assert_eq!(stats["latency"]["max"], json!(8.0));
    let b = &stats["names"]["b"];
    assert_eq!(b["kind"], json!("http"));
    assert_eq!(b["count"], json!(3));
    assert_eq!(b["errors"], json!(1));
    assert_eq!(b["statuses"], json!({ "200": 1, "401": 1, "429": 1 }));
    assert_eq!(b["outcomes"], json!({ "200": 1, "401": 1, "429": 1 }));
    // the histogram's value, within 1/64 of 6 ms
    let p50 = b["p50"].as_f64().unwrap();
    assert!((p50 - 6.0).abs() <= 6.0 / 64.0, "{p50}");
    let c = &stats["names"]["c"];
    assert_eq!(c["kind"], json!("raw"));
    assert_eq!(c["statuses"], json!({ "0": 1, "200": 1 }));
    assert_eq!(c["outcomes"], json!({ "200": 1, "error": 1 }));
    let step = &stats["names"]["a step"];
    assert_eq!(step["kind"], json!("step"));
    assert_eq!(step["outcomes"], json!({ "ok": 1 }));
    assert!(step.get("statuses").is_none());
    assert_eq!(
        stats["scenarios"]["load"],
        json!({ "users": 2, "iterations": 2, "errors": 4 })
    );
    let samples = stats["errorSamples"].as_array().unwrap();
    assert_eq!(samples.len(), 4);
    assert_eq!(samples[0]["name"], json!("b"));
    assert_eq!(samples[0]["status"], json!(429));
    assert_eq!(samples[1]["message"], json!("Error: boom"));
    assert_eq!(samples[3]["user"], json!(1));
    // the bins of the time series, and a name's
    assert_eq!(stats["binSeconds"], json!(1));
    assert!(stats["bins"].is_array());
    assert_eq!(b["binSeconds"], json!(10));
    assert_eq!(b["bins"][0]["count"], json!(3));
    // the thresholds: the error rate and the name with no samples are broken
    assert_eq!(stats["passed"], json!(false));
    let checked = stats["thresholds"].as_array().unwrap();
    assert_eq!(checked.len(), 4);
    assert_eq!(
        checked[0],
        json!({ "name": null, "metric": "errorRate", "limit": 0.4, "value": 0.5, "passed": false })
    );
    assert_eq!(checked[1]["passed"], json!(true));
    assert_eq!(checked[2]["name"], json!("b"));
    assert_eq!(checked[2]["passed"], json!(true));
    assert_eq!(checked[3]["value"], Value::Null);
    assert_eq!(stats["generator"]["where"], json!("same-machine"));
    let text = table(&stats, &order);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("2 users, "), "{text}");
    assert!(lines[0].contains(": 6 requests, "), "{text}");
    assert!(lines[0].ends_with(", 4 errors"), "{text}");
    assert_eq!(lines[1], "name              count  errors    p50    p95    p99    max");
    assert_eq!(lines[3], "b                     3       1  6.0ms  8.0ms  8.0ms  8.0ms");
    assert_eq!(lines[6], "error: b: HTTP 429: too many");
    assert!(
        lines[9].starts_with("threshold broken: errorRate 0.5 over 0.4"),
        "{text}"
    );
    assert!(
        lines[10].starts_with("threshold broken: missing p95 none over 1"),
        "{text}"
    );
    assert_eq!(lines.len(), 11, "{text}");
}

#[test]
fn the_two_stops_and_the_snapshot() {
    let load = one_scenario(1);
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let user = {
        let load = Arc::clone(&load);
        std::thread::spawn(move || {
            begin(Some(load), 0, 0, None);
            timed(true);
            assert!(observe(|| "a".to_string(), Kind::Http, &ok(2)).is_ok());
            // a merge before the snapshot, as a second's wait would
            with_recorder(Recorder::merge);
            ready_tx.send(()).unwrap();
            go_rx.recv().unwrap();
            // stop after the iteration: the next request still runs, the iteration then ends the user
            assert!(before_request().is_ok());
            assert!(iterated(1));
            ready_tx.send(()).unwrap();
            go_rx.recv().unwrap();
            // stop now: a request is refused and its throw is not an error
            let refused = check_stopped().unwrap_err();
            assert_eq!(refused, "the load was stopped");
            assert!(scenario_error(&format!("Error: {refused}"), 1));
            // a pause ends at once
            let started = Instant::now();
            pause(&Pause::Fixed(5_000.0), None);
            assert!(started.elapsed() < Duration::from_secs(1));
            timed(false);
            end();
        })
    };
    ready_rx.recv().unwrap();
    let snapshot = load.snapshot();
    assert_eq!(snapshot["requests"], json!(1));
    assert_eq!(snapshot["activeUsers"], json!(1));
    assert_eq!(snapshot["stopping"], Value::Null);
    assert!(snapshot["bins"].is_array());
    assert!(snapshot["names"]["a"]["count"] == json!(1));
    load.stop_after_iteration();
    assert_eq!(load.stopping(), Some("after-iteration"));
    go_tx.send(()).unwrap();
    ready_rx.recv().unwrap();
    load.stop_now();
    go_tx.send(()).unwrap();
    user.join().unwrap();
    assert_eq!(load.active_users(), 0);
    assert_eq!(load.stopping(), Some("now"));
    assert_eq!(lock(&load.shared).errors, 0);
    load.unregister();
}

#[test]
fn bins_close_each_second_and_merge_past_the_cap() {
    let mut bins = Bins::new(4);
    for second in 0..10u64 {
        for _ in 0..=second {
            bins.add(second, 1_000 * (second + 1), false);
        }
        bins.note_users(second, second + 1);
    }
    bins.close_before(10);
    // 10 seconds past a cap of 4: twice merged, 4-second bins
    assert_eq!(bins.seconds(), 4);
    let closed = bins.closed();
    assert_eq!(closed.len(), 3);
    assert_eq!(
        closed.iter().map(|bin| bin.requests).collect::<Vec<_>>(),
        vec![10, 26, 19]
    );
    assert_eq!(closed[0].max, 4_000);
    assert_eq!(closed[1].users, 8);
    // a late sample adds to its bin's counts
    bins.add(1, 50_000, true);
    assert_eq!(bins.closed()[0].requests, 11);
    assert_eq!(bins.closed()[0].errors, 1);
    assert_eq!(bins.closed()[0].max, 50_000);
    let (thinned, seconds) = bins.to_json(Some(2));
    assert_eq!(thinned.len(), 2);
    assert_eq!(seconds, 8);
    assert_eq!(thinned[1]["t"], json!(8));
    assert_eq!(thinned[0]["requests"], json!(37));
    // one second's percentiles are of its own samples
    let mut one = Bins::new(10);
    for micros in 1..=100u64 {
        one.add(0, micros * 1_000, micros > 98);
    }
    one.close_before(3);
    let (out, _) = one.to_json(None);
    assert_eq!(out.len(), 3);
    assert_eq!(out[0]["p50"], json!(50.0));
    assert_eq!(out[0]["p95"], json!(95.0));
    assert_eq!(out[0]["p99"], json!(99.0));
    assert_eq!(out[0]["errors"], json!(2));
    assert_eq!(out[1]["requests"], json!(0));
    // a name's series of 10 seconds doubles past its cap
    let mut series = Series::new(10, 3);
    for second in [0, 15, 25, 35] {
        series.add(second, 2_000, second == 35);
    }
    assert_eq!(series.seconds(), 20);
    assert_eq!(
        series.bins(),
        &[
            Tally {
                count: 2,
                errors: 0,
                sum: 4_000
            },
            Tally {
                count: 2,
                errors: 1,
                sum: 4_000
            }
        ]
    );
}

#[test]
fn pauses_parse_and_a_gaussian_one_is_never_negative() {
    let mut rng = Rng::with_seed(7);
    let gaussian = Pause::parse(&json!({ "gaussian": [200, 50] }), "pause")
        .unwrap()
        .unwrap();
    let draws: Vec<f64> = (0..20_000).map(|_| gaussian.sample_ms(&mut rng)).collect();
    let mean = draws.iter().sum::<f64>() / draws.len() as f64;
    assert!((195.0..205.0).contains(&mean), "{mean}");
    let deviation = (draws.iter().map(|ms| (ms - mean).powi(2)).sum::<f64>() / draws.len() as f64).sqrt();
    assert!((45.0..55.0).contains(&deviation), "{deviation}");
    assert!(draws.iter().all(|ms| *ms >= 0.0));
    let wide = Pause::Gaussian {
        mean: 10.0,
        deviation: 50.0,
    };
    let draws: Vec<f64> = (0..5_000).map(|_| wide.sample_ms(&mut rng)).collect();
    assert!(draws.iter().all(|ms| *ms >= 0.0));
    assert!(draws.contains(&0.0), "clamped at zero");
    let uniform = Pause::parse(&json!([300, 100]), "think").unwrap().unwrap();
    assert_eq!(uniform, Pause::Uniform(100.0, 300.0));
    assert!((0..1_000).all(|_| (100.0..=300.0).contains(&uniform.sample_ms(&mut rng))));
    assert_eq!(Pause::parse(&json!(25), "think").unwrap(), Some(Pause::Fixed(25.0)));
    assert_eq!(Pause::parse(&json!([25]), "think").unwrap(), Some(Pause::Fixed(25.0)));
    assert_eq!(Pause::parse(&Value::Null, "think").unwrap(), None);
    assert!(Pause::parse(&json!(-1), "think")
        .unwrap_err()
        .starts_with("think is milliseconds"));
    assert!(Pause::parse(&json!("soon"), "pause").is_err());
    assert!(Pause::parse(&json!({ "gaussian": [1] }), "pause").is_err());
    assert_eq!(gaussian.to_json(), json!({ "gaussian": [200.0, 50.0] }));
}

#[test]
fn scenario_settings_take_the_runs_overrides() {
    let plan = Plan {
        scenarios: vec![
            (
                "browse".to_string(),
                json!({ "users": 3, "duration": 60, "pause": [10, 20] }),
            ),
            ("buy".to_string(), json!({ "users": 1, "iterations": 5 })),
        ],
        ..Plan::default()
    };
    let all = script::settings(&plan, &json!({ "iterations": 2 })).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].users, 3);
    assert_eq!(all[0].iterations, Some(2));
    // iterations alone replace the script's duration, and a duration alone its iterations
    assert_eq!(all[0].duration, None);
    let timed = script::settings(&plan, &json!({ "users": 50, "duration": 300 })).unwrap();
    assert_eq!(
        (timed[1].duration, timed[1].iterations, timed[1].users),
        (Some(300.0), None, 50)
    );
    let both = script::settings(&plan, &json!({ "duration": 30, "iterations": 1 })).unwrap();
    assert_eq!((both[0].duration, both[0].iterations), (Some(30.0), Some(1)));
    let own = script::settings(&plan, &json!({ "scenarios": { "buy": { "duration": 10 } } })).unwrap();
    assert_eq!((own[0].duration, own[0].iterations), (Some(10.0), None));
    // null in a scenario's own settings unsets the script's default
    let unset = script::settings(
        &plan,
        &json!({ "scenarios": { "browse": { "pause": null, "iterations": 1 } } }),
    )
    .unwrap();
    assert_eq!(
        (unset[0].pause.clone(), unset[0].duration, unset[0].iterations),
        (None, None, Some(1))
    );
    assert_eq!(all[0].pause, Some(Pause::Uniform(10.0, 20.0)));
    let one = script::settings(&plan, &json!({ "users": 9, "scenarios": { "buy": { "users": 2 } } })).unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].name, "buy");
    assert_eq!(one[0].users, 2);
    assert_eq!(one[0].iterations, Some(5));
    let err = script::settings(&plan, &json!({ "scenarios": { "sell": {} } })).unwrap_err();
    assert_eq!(err, "load: the load script has no scenario sell (it has browse, buy)");
    let err = script::settings(&plan, &json!({ "users": 0 })).unwrap_err();
    assert!(
        err.starts_with("load: scenario browse: users is a whole number"),
        "{err}"
    );
    let none = ScenarioSettings::from_json("load", &json!({ "users": 1 }), "load").unwrap_err();
    assert_eq!(none, "load needs a duration (seconds) or iterations (per user)");
    assert_eq!(all[1].timeout(), 600.0);
    assert_eq!(timed[1].timeout(), 420.0);
}

#[test]
fn counts_names_and_file_parts() {
    assert_eq!(grouped(2412), "2,412");
    assert_eq!(grouped(999), "999");
    assert_eq!(grouped(1_000_000), "1,000,000");
    assert_eq!(default_name("post", "/apps/setPreference"), "POST /apps/setPreference");
    assert_eq!(default_name("GET", "http://localhost:8080/rest/x?y=1"), "GET /rest/x");
    assert_eq!(file_part("mixed reads and writes"), "mixed-reads-and-writes");
    assert_eq!(file_part("a/b: \"c\""), "a_b_-_c_");
}

#[test]
fn a_caught_request_error_does_not_hide_a_later_throw() {
    let load = one_scenario(1);
    let user = {
        let load = Arc::clone(&load);
        std::thread::spawn(move || {
            begin(Some(load), 0, 0, None);
            timed(true);
            let refused = Observed {
                status: Some(500),
                text: "boom",
                elapsed: Duration::from_millis(1),
                expect: None,
            };
            // the script catches the request's error, then fails on its own
            assert!(observe(|| "a".to_string(), Kind::Http, &refused).is_err());
            scenario_error("Error: not found in the page", 0);
            // a request error left over from one iteration does not hide the next one's throw
            assert!(observe(|| "a".to_string(), Kind::Http, &refused).is_err());
            iterated(1);
            scenario_error("Error: load: a: HTTP 500: boom", 1);
            timed(false);
            end();
        })
    };
    user.join().unwrap();
    // two requests and two throws
    assert_eq!(lock(&load.shared).errors, 4);
    load.unregister();
}

#[test]
fn a_stop_ends_the_ramp_and_a_late_user_is_refused() {
    let load = one_scenario(1);
    let user = {
        let load = Arc::clone(&load);
        std::thread::spawn(move || {
            begin(Some(load), 0, 0, None);
            let started = Instant::now();
            // a start a day away, past a pause's hour
            let stopped = ramp(Instant::now() + Duration::from_secs(86_400));
            end();
            (stopped, started.elapsed())
        })
    };
    std::thread::sleep(Duration::from_millis(50));
    load.stop_after_iteration();
    let (stopped, took) = user.join().unwrap();
    assert!(stopped);
    assert!(took < Duration::from_secs(2), "{took:?}");
    let id = load.id();
    load.unregister();
    let late = host_call("thread.begin", &json!({ "load": id, "user": 1 })).unwrap();
    assert_eq!(late.unwrap_err(), "the load this user belongs to has ended");
    end();
}

#[test]
fn a_held_second_stays_open_and_a_step_leaves_out_its_pauses() {
    let load = one_scenario(1);
    let (merged_tx, merged_rx) = std::sync::mpsc::channel();
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let user = {
        let load = Arc::clone(&load);
        std::thread::spawn(move || {
            begin(Some(load), 0, 0, None);
            timed(true);
            step_begin();
            pause(&Pause::Fixed(300.0), None);
            assert!(observe(|| "a".to_string(), Kind::Http, &ok(2)).is_ok());
            step_end("s", None);
            merged_tx.send(()).unwrap();
            go_rx.recv().unwrap();
            with_recorder(Recorder::merge);
            merged_tx.send(()).unwrap();
            go_rx.recv().unwrap();
            timed(false);
            end();
        })
    };
    merged_rx.recv().unwrap();
    // the user holds second 0 (its samples are not merged): ten seconds on, it is still open
    load.tick_at(10);
    assert!(lock(&load.shared).bins.closed().is_empty());
    go_tx.send(()).unwrap();
    merged_rx.recv().unwrap();
    load.tick_at(10);
    assert_eq!(lock(&load.shared).bins.closed().len(), 8);
    assert_eq!(lock(&load.shared).bins.closed()[0].requests, 1);
    go_tx.send(()).unwrap();
    user.join().unwrap();
    // the step's 300 ms pause is not its time
    let step = lock(&load.shared).names["s"].histogram.max();
    assert!(step < 200_000, "{step}");
    load.unregister();
}

#[test]
fn huge_times_are_cut_and_thresholds_are_checked_before_a_run() {
    assert_eq!(seconds(1e300), Duration::from_secs_f64(pace::MAX_SECONDS));
    assert_eq!(seconds(f64::NAN), Duration::ZERO);
    assert_eq!(seconds(-1.0), Duration::ZERO);
    let huge = ScenarioSettings::from_json("load", &json!({ "duration": 1e300, "rampUp": 1e300 }), "load").unwrap();
    assert_eq!(huge.duration, Some(pace::MAX_SECONDS));
    let _ = seconds(huge.timeout());
    assert!(stats::check_thresholds(&json!({ "p95": 100, "names": { "a": { "p99": 1 } } })).is_ok());
    assert!(stats::check_thresholds(&Value::Null).is_ok());
    let err = stats::check_thresholds(&json!({ "p96": 1 })).unwrap_err();
    assert!(
        err.starts_with("load: thresholds: p96 is not a metric (errorRate, "),
        "{err}"
    );
    let err = stats::check_thresholds(&json!({ "names": { "a": { "p95": "fast" } } })).unwrap_err();
    assert_eq!(err, "load: thresholds.names.a.p95 is a number, not \"fast\"");
    assert!(stats::check_thresholds(&json!({ "names": { "a": { "names": {} } } })).is_err());
    // with no request, the run's own metrics have no value and fail
    let load = one_scenario(1);
    let (stats, _) = result_of(&load, &json!({ "errorRate": 1, "p95": 1000 }));
    load.unregister();
    assert_eq!(stats["passed"], json!(false));
    assert_eq!(stats["thresholds"][0]["value"], Value::Null);
    assert_eq!(stats["thresholds"][1]["value"], Value::Null);
}

#[test]
fn scenarios_pick_by_name_list() {
    let plan = Plan {
        scenarios: vec![
            ("browse".to_string(), json!({ "users": 3, "iterations": 1 })),
            ("buy".to_string(), json!({ "users": 1, "iterations": 5 })),
        ],
        ..Plan::default()
    };
    let one = script::settings(&plan, &json!({ "scenarios": ["buy"] })).unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].iterations, Some(5));
    let err = script::settings(&plan, &json!({ "scenarios": ["sell"] })).unwrap_err();
    assert_eq!(err, "load: the load script has no scenario sell (it has browse, buy)");
    assert!(script::settings(&plan, &json!({ "scenarios": "buy" })).is_err());
}
