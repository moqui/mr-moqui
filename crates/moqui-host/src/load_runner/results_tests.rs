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

fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("moqui-load-runs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    root
}

fn result(script: &str, status: &str, start: Value, requests: u64) -> Value {
    json!({
        "users": 2, "iterations": 2, "requests": requests, "errors": 0, "errorRate": 0.0, "rps": 4.0,
        "latency": { "min": 1.0, "p95": 3.5, "max": 4.0 },
        "names": {},
        "run": { "script": script, "label": "a label", "user": "john.doe", "status": status,
                 "target": "http://localhost:8080", "start": start, "end": null },
    })
}

#[test]
fn a_run_id_names_only_a_directory() {
    for id in [
        "20261110-142233-4242",
        "20261110-142233-1",
        "20261110-142233-4242-2",
        "20261110-142233-4242-999",
    ] {
        assert!(valid_run_id(id), "{id}");
    }
    for id in [
        "",
        "..",
        "20261110-142233",
        "20261110-142233-",
        "20261110-142233-4242/..",
        "../20261110-142233-4242",
        "20261110-142233-42a",
        "2026111-142233-4242",
        "20261110-14223-4242",
        "20261110-142233-4242-1000",
        "20261110-142233-4242-1-1",
        "20261110_142233-4242",
        "20261110-142233-12345678901",
    ] {
        assert!(!valid_run_id(id), "{id}");
    }
    let root = Path::new("/r");
    assert_eq!(run_dir(root, ".."), None);
    assert_eq!(run_dir(root, "20261110-142233-1"), Some(root.join("20261110-142233-1")));
}

#[test]
fn run_ids_follow_the_time_and_take_a_suffix_in_the_same_second() {
    let at: jiff::Timestamp = "2026-11-10T14:22:33Z".parse().unwrap();
    let base = run_id_base(at, 4242);
    assert_eq!(base, "20261110-142233-4242");
    assert_eq!(run_id_millis(&base), Some(at.as_millisecond()));

    let root = temp_root("create");
    let (first, dir) = create_run_dir(&root, &base).unwrap();
    assert_eq!(first, base);
    assert!(dir.is_dir());
    let (second, _) = create_run_dir(&root, &base).unwrap();
    assert_eq!(second, "20261110-142233-4242-2");
    assert!(create_run_dir(&root, "../x").is_err());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn runs_list_newest_first_and_filter_by_script_and_status() {
    let root = temp_root("list");
    let a = "component://tools/test/load/ServiceCall.load.js";
    let b = "component://mr-marble/test/load/OrderToCash.load.js";
    // run.start as RFC 3339, as epoch milliseconds, and missing (the runId's time)
    write_result(
        &root,
        "20261110-100000-1",
        &result(a, "completed", json!("2026-11-10T12:00:00Z"), 4),
    )
    .unwrap();
    write_result(
        &root,
        "20261110-110000-1",
        &result(b, "stopped", json!(1_762_772_400_000_i64), 9),
    )
    .unwrap();
    write_result(&root, "20261110-130000-1", &result(a, "failed", Value::Null, 1)).unwrap();
    // left out: a run still running (no result), a file that is not a result, a name that is not a runId
    std::fs::create_dir_all(root.join("20261110-140000-1")).unwrap();
    std::fs::create_dir_all(root.join("20261110-150000-1")).unwrap();
    std::fs::write(
        root.join("20261110-150000-1").join(RESULT_FILE),
        b"{\"not\": \"a result\"}",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(
        root.join("notes").join(RESULT_FILE),
        result(a, "completed", Value::Null, 1).to_string(),
    )
    .unwrap();

    let ids = |runs: &[Value]| -> Vec<String> {
        runs.iter()
            .map(|run| run["runId"].as_str().unwrap().to_string())
            .collect()
    };
    let all = list(&root, &Find::default());
    // 13:00 (runId), 12:00 (start text), 11:00 (start millis, 2025-11-10T11:00:00Z is earlier still)
    assert_eq!(
        ids(&all),
        ["20261110-130000-1", "20261110-100000-1", "20261110-110000-1"]
    );
    assert_eq!(all[1]["script"], json!(a));
    assert_eq!(all[1]["requests"], json!(4));
    assert_eq!(all[1]["p95"], json!(3.5));
    assert_eq!(all[1]["user"], json!("john.doe"));

    let of_a = list(
        &root,
        &Find {
            script: Some(a),
            status: None,
        },
    );
    assert_eq!(ids(&of_a), ["20261110-130000-1", "20261110-100000-1"]);
    let stopped = list(
        &root,
        &Find {
            script: None,
            status: Some("stopped"),
        },
    );
    assert_eq!(ids(&stopped), ["20261110-110000-1"]);

    // a rewritten file is read again, not answered from the cache
    write_result(
        &root,
        "20261110-100000-1",
        &result(a, "completed", json!("2026-11-10T12:00:00Z"), 40),
    )
    .unwrap();
    let again = list(
        &root,
        &Find {
            script: Some(a),
            status: Some("completed"),
        },
    );
    assert_eq!(again[0]["requests"], json!(40));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_run_reads_and_deletes_by_its_id_only() {
    let root = temp_root("delete");
    let id = "20261110-100000-7";
    let value = result(
        "component://tools/test/load/ServiceCall.load.js",
        "completed",
        Value::Null,
        4,
    );
    write_result(&root, id, &value).unwrap();
    assert_eq!(read_result(&root, id).unwrap(), Some(value));
    assert_eq!(read_result(&root, "20261110-100000-8").unwrap(), None);
    assert!(read_result(&root, "../etc").is_err());
    assert!(delete(&root, "..").is_err());
    assert!(root.is_dir());

    assert_eq!(delete(&root, id), Ok(true));
    assert!(!root.join(id).exists());
    assert_eq!(delete(&root, id), Ok(false));
    assert!(list(&root, &Find::default()).is_empty());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_result_is_told_from_other_json() {
    assert!(is_result(&result("s", "completed", Value::Null, 1)));
    let mut no_status = result("s", "completed", Value::Null, 1);
    no_status["run"].as_object_mut().unwrap().remove("status");
    assert!(is_result(&no_status));

    assert!(!is_result(&json!([])));
    assert!(!is_result(&json!({ "requests": 1 })));
    assert!(!is_result(&result("s", "running", Value::Null, 1)));
    for field in ["run", "requests", "errors", "latency", "names"] {
        let mut missing = result("s", "completed", Value::Null, 1);
        missing.as_object_mut().unwrap().remove(field);
        assert!(!is_result(&missing), "{field}");
    }
    let mut text_requests = result("s", "completed", Value::Null, 1);
    text_requests["requests"] = json!("4");
    assert!(!is_result(&text_requests));
}
