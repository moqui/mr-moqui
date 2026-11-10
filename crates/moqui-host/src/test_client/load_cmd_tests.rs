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

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|arg| (*arg).to_string()).collect()
}

#[test]
fn options_parse_and_refuse() {
    let parsed = parse_load_args(&args(&[
        "mr-marble",
        "--script",
        "OrderToCash",
        "--users",
        "50",
        "--duration",
        "300",
        "--ramp-up",
        "30",
        "--param",
        "pdf=false",
        "--label",
        "Java 3.1",
        "--scenario",
        "sales",
        "--iterations",
        "2",
    ]))
    .unwrap();
    assert_eq!(parsed.target.as_deref(), Some("mr-marble"));
    assert_eq!(parsed.users, Some(50));
    assert_eq!(parsed.params, vec![("pdf".to_string(), "false".to_string())]);
    assert_eq!(
        overrides(&parsed),
        json!({ "scenarios": { "sales": { "users": 50, "rampUp": 30.0, "duration": 300.0, "iterations": 2 } } })
    );
    let compare = parse_load_args(&args(&["--compare", "a.json", "b.json"])).unwrap();
    assert_eq!(compare.compare.len(), 2);
    let refused = |list: &[&str]| parse_load_args(&args(list)).unwrap_err().to_string();
    assert!(refused(&[]).starts_with("moqui test-load needs a component"));
    assert!(refused(&["x", "--users", "0"]).starts_with("--users is a whole number from 1"));
    assert!(refused(&["x", "--duration", "0"]).starts_with("--duration is more than 0"));
    assert!(refused(&["x", "--param", "novalue"]).starts_with("--param needs name=value"));
    assert!(refused(&["x", "--bogus"]).starts_with("unknown option --bogus"));
    assert!(refused(&["x", "--compare", "a.json"]).starts_with("--compare needs two"));
    assert!(refused(&["x", "y"]).starts_with("one component"));
    assert!(refused(&["x", "--script"]).starts_with("--script needs a value"));
}

#[test]
fn the_progress_line_and_the_verdict() {
    let snapshot = json!({ "elapsed": 35.2, "activeUsers": 10, "errors": 0, "latency": { "p95": 38.2 } });
    assert_eq!(
        progress_line(&snapshot, 241.24),
        "35s  10 users  241.2/s  0 errors  p95 38ms"
    );
    let ok = json!({ "passed": true, "names": { "a": { "kind": "http", "count": 2, "errors": 1 } } });
    assert!(verdict(&ok).is_ok());
    let broken = json!({ "passed": false, "names": { "a": { "kind": "http", "count": 2, "errors": 0 } } });
    assert_eq!(verdict(&broken).unwrap_err().to_string(), "a threshold was broken");
    let none = json!({ "passed": true, "names": { "a": { "kind": "http", "count": 2, "errors": 2 },
        "s": { "kind": "step", "count": 1, "errors": 0 } } });
    assert_eq!(verdict(&none).unwrap_err().to_string(), "no request succeeded");
}

#[test]
fn a_script_file_belongs_to_the_component_above_it() {
    let root = std::env::temp_dir().join(format!("moqui-load-cmd-{}", std::process::id()));
    let dir = root.join("comp/test/load");
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(component_of(&dir.join("A.load.js")), root.join("comp"));
    std::fs::write(root.join("component.xml"), "<component name=\"outer\"/>").unwrap();
    assert_eq!(component_of(&dir.join("A.load.js")), root);
    let _ = std::fs::remove_dir_all(root);
}
