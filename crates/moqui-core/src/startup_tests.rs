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

#[test]
fn a_second_record_replaces_the_first_and_order_is_kept() {
    let report = StartupReport::default();
    report.record(Section::Listener, "SSH", State::On("tcp 127.0.0.1:2222".into()));
    report.record(Section::Integration, "Search", State::on("postgres"));
    report.record(Section::Listener, "SSH", State::failed("bind"));
    report.record_default(Section::Integration, "Search", State::NotBuilt);
    let entries = report.entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].state, State::failed("bind"));
    assert_eq!(entries[1].state, State::on("postgres"));
}

/// The registry sets the cause after it records a state (plans/Runtime-Config.md §3.1); the next record, by the
/// registry or a subsystem, stamps a new time and starts with no cause.
#[test]
fn a_record_stamps_its_time_and_set_cause_names_why() {
    let report = StartupReport::default();
    let before = jiff::Timestamp::now();
    report.record(Section::Listener, "SSH", State::On("tcp 127.0.0.1:2222".into()));
    let first = report.entries()[0].clone();
    assert!(first.since >= before, "{first:?}");
    assert_eq!(first.cause, None);
    assert!(report.set_cause(Section::Listener, "SSH", "conf reload"));
    assert_eq!(report.entries()[0].cause.as_deref(), Some("conf reload"));
    // by section and name: an integration of the same name, or a name not recorded, is not this row
    assert!(!report.set_cause(Section::Integration, "SSH", "boot"));
    assert!(!report.set_cause(Section::Listener, "SMTP", "boot"));
    report.record(Section::Listener, "SSH", State::off("ssh-server@enabled is false"));
    let second = report.entries()[0].clone();
    assert!(second.since >= first.since, "{second:?}");
    assert_eq!(second.cause, None);
    assert_eq!(report.entries().len(), 1);
}

fn identity() -> Identity {
    Identity {
        version: "1.2.3".into(),
        package: "maximal".into(),
        purpose: "production".into(),
        role: String::new(),
    }
}

/// `[name, state, detail]` of each row of `key`.
fn rows(value: &ValueMap, key: &str) -> Vec<[String; 3]> {
    let Some(Value::List(rows)) = value.get(key) else {
        panic!("{key} is a list");
    };
    rows.iter()
        .map(|row| {
            let Value::Map(row) = row else { panic!("a row is a map") };
            ["name", "state", "detail"].map(|field| row[field].as_str().unwrap_or("").to_string())
        })
        .collect()
}

fn texts(value: &ValueMap, key: &str) -> Vec<String> {
    let Some(Value::List(items)) = value.get(key) else {
        panic!("{key} is a list");
    };
    items
        .iter()
        .map(|item| item.as_str().unwrap_or("").to_string())
        .collect()
}

#[test]
fn the_script_value_has_each_state_as_a_word_in_the_logs_order() {
    let report = StartupReport::default();
    report.record(Section::Listener, "SSH", State::off("ssh-server@port is not set"));
    report.record(
        Section::Listener,
        "Edge tunnels",
        State::Skipped("port 7000 is in use".into()),
    );
    report.record(Section::Listener, "HTTP", State::On("tcp 0.0.0.0:8080".into()));
    report.record(Section::Listener, "Other", State::On("tcp 0.0.0.0:9".into()));
    report.record(Section::Listener, "gRPC", State::NotBuilt);
    report.record(Section::Integration, "search", State::on("postgres"));
    report.record(Section::Integration, "PDF", State::failed("no Chromium found"));
    report.record(Section::Integration, "Dev reload", State::NotBuilt);
    let value = report.to_value(&identity(), &["Csv".to_string(), "Mail".to_string()]);
    assert_eq!(
        rows(&value, "listeners"),
        [
            ["HTTP", "on", "tcp 0.0.0.0:8080"],
            ["SSH", "off", "ssh-server@port is not set"],
            ["Edge tunnels", "skipped", "port 7000 is in use"],
            ["Other", "on", "tcp 0.0.0.0:9"],
        ]
        .map(|row| row.map(str::to_string))
    );
    // by name, whatever its case
    assert_eq!(
        rows(&value, "integrations"),
        [["PDF", "failed", "no Chromium found"], ["search", "on", "postgres"]].map(|row| row.map(str::to_string))
    );
    assert_eq!(texts(&value, "notBuilt"), ["Dev reload", "gRPC"]);
    assert_eq!(texts(&value, "tools"), ["Csv", "Mail"]);
    assert_eq!(value["version"].as_str(), Some("1.2.3"));
    assert_eq!(value["package"].as_str(), Some("maximal"));
    assert_eq!(value["purpose"].as_str(), Some("production"));
    assert_eq!(value["role"].as_str(), Some(""));
    // the host has not logged yet: no ready time (null, not zero) and no routes
    assert!(value["readySeconds"].is_null());
    assert!(texts(&value, "routes").is_empty());
}

#[test]
fn a_second_record_replaces_the_first_in_the_script_value() {
    let report = StartupReport::default();
    report.record(Section::Listener, "SSH", State::On("tcp 127.0.0.1:2222".into()));
    report.record(
        Section::Listener,
        "SSH",
        State::failed("the host key could not be read"),
    );
    report.record_ready(Ready {
        seconds: 9.0,
        routes: String::new(),
        listeners: true,
    });
    report.record_ready(Ready {
        seconds: 1.23456,
        routes: "/health, /metrics, the webapp's servlets".into(),
        listeners: true,
    });
    let value = report.to_value(&identity(), &[]);
    assert_eq!(
        rows(&value, "listeners"),
        [["SSH", "failed", "the host key could not be read"].map(str::to_string)]
    );
    assert_eq!(value["readySeconds"], Value::F64(1.235));
    assert_eq!(
        texts(&value, "routes"),
        ["/health", "/metrics", "the webapp's servlets"]
    );
}

#[test]
fn a_boot_with_no_listeners_lists_none() {
    let report = StartupReport::default();
    report.record(Section::Listener, "HTTP", State::off("no listeners"));
    report.record(Section::Listener, "SSH", State::NotBuilt);
    report.record(Section::Integration, "Search", State::on("postgres"));
    report.record_ready(Ready {
        seconds: 0.5,
        routes: String::new(),
        listeners: false,
    });
    let value = report.to_value(&identity(), &[]);
    assert!(rows(&value, "listeners").is_empty());
    assert!(texts(&value, "notBuilt").is_empty());
    assert_eq!(rows(&value, "integrations").len(), 1);
}

/// A row's `since` and `cause` follow `name`, `state` and `detail`, which System Home, the attention check and the
/// protocol facts read; `cause` is null until the registry sets one.
#[test]
fn the_script_value_carries_since_and_cause_after_the_first_three() {
    let report = StartupReport::default();
    report.record(Section::Listener, "SSH", State::On("tcp 127.0.0.1:2222".into()));
    report.record(Section::Listener, "HTTP", State::On("tcp 0.0.0.0:8080".into()));
    report.set_cause(Section::Listener, "SSH", "set ssh-server@port");
    let value = report.to_value(&identity(), &[]);
    let Some(Value::List(listeners)) = value.get("listeners") else {
        panic!("listeners is a list");
    };
    let fields = |row: &Value| -> Vec<String> {
        let Value::Map(row) = row else { panic!("a row is a map") };
        row.keys().cloned().collect()
    };
    assert_eq!(fields(&listeners[0]), ["name", "state", "detail", "since", "cause"]);
    let Value::Map(http) = &listeners[0] else {
        panic!("a row is a map")
    };
    let Value::Map(ssh) = &listeners[1] else {
        panic!("a row is a map")
    };
    assert_eq!(http["name"].as_str(), Some("HTTP"));
    assert!(http["cause"].is_null());
    assert!(matches!(http["since"], Value::Timestamp(_)), "{http:?}");
    assert_eq!(ssh["cause"].as_str(), Some("set ssh-server@port"));
}
