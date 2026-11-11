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
    report.record(Section::Listener, "SSH", State::off("ssh_port is not set"));
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
            ["SSH", "off", "ssh_port is not set"],
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
