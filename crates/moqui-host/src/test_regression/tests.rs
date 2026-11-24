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

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::compare::{compare, Rules};
use super::db::{diff, row_key, RowChange, Rows, Snapshot};
use super::engine::{ready, test_conf, Clock};
use super::record::{CaseRecord, Recording};
use super::target::{Kind, Target};
use super::{parse_args, CLOCK_EPOCH_MS};

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_string()).collect()
}

#[test]
fn the_arguments_are_two_targets_then_components_and_options() {
    let args = parse_args(&strings(&[
        "rust:master", "java:.", "framework", "--spec", "Uom.spec.js, Enumeration", "--keep", "--java-repo", "/j",
        "--start-timeout", "60",
    ]))
    .unwrap();
    assert_eq!(args.baseline, "rust:master");
    assert_eq!(args.candidate, "java:.");
    assert_eq!(args.targets, ["framework"]);
    assert_eq!(args.specs, ["Uom", "Enumeration"]);
    assert!(args.keep);
    assert_eq!(args.java_repo, Some(PathBuf::from("/j")));
    assert!((args.start_timeout - 60.0).abs() < f64::EPSILON);
    assert!(parse_args(&strings(&["rust:."])).is_err());
    assert!(parse_args(&strings(&["rust:.", "rust:.", "--bogus"])).is_err());
    assert!(parse_args(&strings(&["rust:.", "rust:.", "--start-timeout", "0"])).is_err());
}

#[test]
fn a_target_is_an_engine_and_a_commit() {
    assert_eq!(Target::parse("rust:.", None).unwrap(), Target::RustHere);
    assert_eq!(Target::parse("rust:abc123", None).unwrap(), Target::RustRef("abc123".to_string()));
    let java = Target::parse("java:master", Some(Path::new("/j"))).unwrap();
    assert!(java.is_java());
    assert_eq!(java.name(), "java:master");
    assert!(Target::parse("java:.", None).is_err(), "java needs the checkout");
    assert!(Target::parse("rust:", None).is_err());
    assert!(Target::parse("master", None).is_err());
    assert!(Target::parse("groovy:.", None).is_err());
}

#[test]
fn a_row_key_is_its_primary_key_or_the_whole_row() {
    let row = json!({"enum_id": "A", "seq": 2, "note": null});
    assert_eq!(row_key(&row, &strings(&["enum_id", "seq"])), "A|2");
    assert_eq!(row_key(&row, &strings(&["note"])), "∅");
    assert_eq!(row_key(&row, &[]), row.to_string());
}

fn snapshot(tables: &[(&str, &[(&str, Value)])]) -> Snapshot {
    tables
        .iter()
        .map(|(table, rows)| {
            let rows: Rows = rows.iter().map(|(key, row)| ((*key).to_string(), row.clone())).collect();
            ((*table).to_string(), rows)
        })
        .collect()
}

#[test]
fn the_diff_has_each_insert_update_and_delete_by_table_and_key() {
    let before = snapshot(&[("enumeration", &[("A", json!({"id": "A", "d": "a"})), ("B", json!({"id": "B"}))])]);
    let after = snapshot(&[
        ("enumeration", &[("A", json!({"id": "A", "d": "changed"}))]),
        ("uom", &[("X", json!({"id": "X"}))]),
    ]);
    let kinds: Vec<(String, String, &str)> =
        diff(&before, &after).iter().map(|change| (change.table.clone(), change.key.clone(), change.kind())).collect();
    assert_eq!(
        kinds,
        [
            ("enumeration".to_string(), "A".to_string(), "update"),
            ("enumeration".to_string(), "B".to_string(), "delete"),
            ("uom".to_string(), "X".to_string(), "insert"),
        ]
    );
    assert!(diff(&before, &before).is_empty());
}

#[test]
fn an_engine_is_ready_when_its_data_is_loaded_and_it_listens_on_the_port() {
    let rust = Kind::Rust { binary: PathBuf::new() };
    let loaded = "INFO Table check and empty-db load finished in 3s\n";
    assert!(!ready(&rust, loaded, 8123));
    assert!(ready(&rust, &format!("{loaded}INFO Listening on http://127.0.0.1:8123\n"), 8123));
    assert!(!ready(&rust, &format!("{loaded}INFO Listening on http://127.0.0.1:81234\n"), 8123));
    let java = Kind::Java { war: PathBuf::new() };
    assert!(!ready(&java, "Moqui Framework initialized in 40s\n", 8123));
    assert!(ready(&java, "Moqui Framework initialized in 40s\nStarted oejs.Server@1a2b\n", 8123));
}

#[test]
fn the_clock_file_holds_the_time_and_moves_on() {
    let dir = std::env::temp_dir().join(format!("moqui-test-regression-clock-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let clock = Clock::new(&dir.join("clock"), CLOCK_EPOCH_MS).unwrap();
    assert_eq!(std::fs::read_to_string(clock.path()).unwrap().trim(), CLOCK_EPOCH_MS.to_string());
    assert_eq!(clock.advance(1_000).unwrap(), CLOCK_EPOCH_MS + 1_000);
    assert_eq!(clock.now(), CLOCK_EPOCH_MS + 1_000);
    assert_eq!(std::fs::read_to_string(clock.path()).unwrap().trim(), (CLOCK_EPOCH_MS + 1_000).to_string());
    let _ = std::fs::remove_dir_all(&dir);
}

fn recording(target: &str, boot: Snapshot, answer: Value, changes: Vec<RowChange>) -> Recording {
    Recording {
        label: "x".to_string(),
        target: target.to_string(),
        boot,
        cases: vec![CaseRecord {
            spec: "Uom".to_string(),
            case: "convert".to_string(),
            outcome: "passed".to_string(),
            calls: vec![json!({"call": "service", "name": "convert#Uom", "answer": answer})],
            changes,
            clock_ms: CLOCK_EPOCH_MS,
        }],
        problems: Vec::new(),
    }
}

fn insert(table: &str, key: &str, row: Value) -> RowChange {
    RowChange { table: table.to_string(), key: key.to_string(), before: None, after: Some(row) }
}

#[test]
fn equal_recordings_pass_and_a_changed_answer_fails() {
    let boot = snapshot(&[("uom", &[("USD", json!({"uom_id": "USD"}))])]);
    let a = recording("rust:.", boot.clone(), json!({"status": 200, "body": {"convertedAmount": "200.0"}}), vec![]);
    let report = compare(&a, &a.clone(), &Rules::default());
    assert!(report.passed(), "{:?}", report.lines());
    let b = recording("rust:.", boot, json!({"status": 200, "body": {"convertedAmount": "2.0"}}), vec![]);
    let report = compare(&a, &b, &Rules::default());
    assert!(!report.passed());
    let lines = report.lines().join("\n");
    assert!(lines.contains("answer.body.convertedAmount: baseline \"200.0\", candidate \"2.0\""), "{lines}");
    assert!(lines.contains("RESULT: FAIL"), "{lines}");
}

#[test]
fn volatile_columns_and_tables_are_left_out() {
    let mut rules = Rules::default();
    rules.add_volatile(&json!({
        "columns": ["last_updated_stamp"], "tables": ["public.artifact_hit"], "tableColumns": {"visit": ["visit_id"]},
    }));
    let a = recording(
        "rust:.",
        snapshot(&[("uom", &[("USD", json!({"uom_id": "USD", "last_updated_stamp": 1}))])]),
        json!({}),
        vec![insert("artifact_hit", "1", json!({"id": 1})), insert("visit", "v", json!({"visit_id": "a", "user": "j"}))],
    );
    let b = recording(
        "rust:.",
        snapshot(&[("uom", &[("USD", json!({"uom_id": "USD", "last_updated_stamp": 2}))])]),
        json!({}),
        vec![insert("visit", "v", json!({"visit_id": "b", "user": "j"}))],
    );
    let report = compare(&a, &b, &rules);
    assert!(report.passed(), "{:?}", report.lines());
    // a column that is not volatile still counts
    let c = recording("rust:.", a.boot.clone(), json!({}), vec![insert("visit", "v", json!({"visit_id": "a", "user": "k"}))]);
    assert!(!compare(&a, &c, &rules).passed());
}

#[test]
fn the_approved_differences_apply_to_their_engine_only() {
    let mut rules = Rules::default();
    rules.add_approved(&json!({
        "rust_only_tables": ["public.auth_flow"],
        "rust_only_columns": [{"table": "public.user_account", "name": "passkey", "type": "text"}],
        "java_seed_additions": [{"reason": "r", "table": "public.enumeration", "entity": "moqui.basic.Enumeration",
            "key": ["enum_id"], "values": [["JavaOnly"]]}],
    }));
    let rust = snapshot(&[
        ("auth_flow", &[("1", json!({"id": 1}))]),
        ("user_account", &[("j", json!({"user_id": "j", "passkey": "x"}))]),
        ("enumeration", &[("A", json!({"enum_id": "A"}))]),
    ]);
    let java = snapshot(&[
        ("user_account", &[("j", json!({"user_id": "j"}))]),
        ("enumeration", &[("A", json!({"enum_id": "A"})), ("JavaOnly", json!({"enum_id": "JavaOnly"}))]),
    ]);
    let a = recording("java:.", java, json!({}), vec![]);
    let b = recording("rust:.", rust.clone(), json!({}), vec![]);
    let report = compare(&a, &b, &rules);
    assert!(report.passed(), "{:?}", report.lines());
    // with both targets Rust, a Rust-only table is no excuse: the baseline lacks it
    let c = recording("rust:.", snapshot(&[("user_account", &[("j", json!({"user_id": "j", "passkey": "x"}))])]), json!({}), vec![]);
    let report = compare(&c, &b, &rules);
    assert!(!report.passed(), "{:?}", report.lines());
}

#[test]
fn a_case_only_one_target_ran_and_a_failed_spec_both_fail_the_run() {
    let a = recording("rust:.", Snapshot::new(), json!({}), vec![]);
    let mut b = a.clone();
    b.cases[0].case = "other".to_string();
    assert!(!compare(&a, &b, &Rules::default()).passed());
    let mut failed = a.clone();
    failed.cases[0].outcome = "failed: no".to_string();
    let report = compare(&failed, &failed.clone(), &Rules::default());
    assert!(!report.passed());
    assert!(report.lines().join("\n").contains("same, but the spec failed"));
}

#[test]
fn the_test_conf_serves_plain_http_and_no_grpc() {
    let dev = r#"<webapp name="webroot" http-listen-port="off" grpc-listen-port="off" https-enabled="true"/>"#;
    assert_eq!(test_conf(dev), r#"<webapp name="webroot" grpc-listen-port="off" https-enabled="false"/>"#);
    let bare = r#"<webapp name="webroot" https-enabled="true" https-port="8443"/>"#;
    assert_eq!(test_conf(bare), r#"<webapp name="webroot" grpc-listen-port="off" https-enabled="false" https-port="8443"/>"#);
}
