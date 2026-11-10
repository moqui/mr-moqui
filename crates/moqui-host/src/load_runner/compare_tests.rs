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

fn result(requests: u64, p95: f64, names: Value, order: &[&str]) -> Value {
    json!({ "requests": requests, "rps": 10.0, "errors": 0, "errorRate": 0.0,
        "latency": { "p50": 1.0, "p95": p95, "p99": 9.0 }, "names": names, "nameOrder": order, "run": {} })
}

#[test]
fn values_and_changes_from_the_first() {
    let a = result(
        100,
        4.0,
        json!({ "b": { "count": 50, "p95": 2.0 }, "a": { "count": 50, "p95": 4.0 } }),
        &["b", "a"],
    );
    let b = result(
        75,
        5.0,
        json!({ "a": { "count": 75, "p95": 3.0 }, "c": { "count": 1 } }),
        &["a", "c"],
    );
    let rows = rows(&[a, b]);
    let requests = &rows[0];
    assert_eq!((requests.section, requests.metric), ("summary", "requests"));
    assert_eq!(requests.values, [Some(100.0), Some(75.0)]);
    assert_eq!(requests.changes, [None, Some(-25.0)]);
    let p95 = rows
        .iter()
        .find(|row| row.section == "summary" && row.metric == "p95")
        .unwrap();
    assert_eq!(p95.changes, [None, Some(25.0)]);
    // errors 0 in the first: no change
    let errors = rows
        .iter()
        .find(|row| row.section == "summary" && row.metric == "errors")
        .unwrap();
    assert_eq!(errors.changes, [None, None]);
    // names in the first's nameOrder, then those only the second has
    let names: Vec<&str> = rows
        .iter()
        .filter(|row| row.section == "name" && row.metric == "count")
        .map(|row| row.name.as_deref().unwrap())
        .collect();
    assert_eq!(names, ["b", "a", "c"]);
    let b_count = rows
        .iter()
        .find(|row| row.name.as_deref() == Some("b") && row.metric == "count")
        .unwrap();
    assert_eq!(b_count.values, [Some(50.0), None]);
    assert_eq!(b_count.changes, [None, None]);
    let a_p95 = rows
        .iter()
        .find(|row| row.name.as_deref() == Some("a") && row.metric == "p95")
        .unwrap();
    assert_eq!(a_p95.changes, [None, Some(-25.0)]);
    assert_eq!(a_p95.to_json()["section"], json!("name"));
}
