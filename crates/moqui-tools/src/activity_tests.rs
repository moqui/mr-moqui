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

use std::collections::BTreeMap;

use moqui_lib::object::Value;

use super::{blocking_trees, duplicates, IndexRow};

fn pids(tree: &Value) -> String {
    let Value::Map(node) = tree else { panic!("a node") };
    let pid = moqui_lib::object::to_plain_string(&node["pid"]);
    let Value::List(children) = &node["blocks"] else {
        panic!("a list")
    };
    if children.is_empty() {
        pid
    } else {
        format!("{pid}({})", children.iter().map(pids).collect::<Vec<_>>().join(" "))
    }
}

/// 10 blocks 11, which blocks 12 and 13; 20 blocks 21; 30 waits on no one and blocks no one.
#[test]
fn blocking_chains_are_trees_from_the_sessions_that_wait_on_no_one() {
    let mut blocked = BTreeMap::new();
    blocked.insert(10, vec![]);
    blocked.insert(11, vec![10]);
    blocked.insert(12, vec![11]);
    blocked.insert(13, vec![11]);
    blocked.insert(21, vec![20]);
    blocked.insert(30, vec![]);
    let trees: Vec<String> = blocking_trees(&blocked).iter().map(pids).collect();
    assert_eq!(trees, ["10(11(12 13))", "20(21)"]);
    // a cycle is cut where it comes back
    let mut cycle = BTreeMap::new();
    cycle.insert(1, vec![2]);
    cycle.insert(2, vec![1]);
    let trees: Vec<String> = blocking_trees(&cycle).iter().map(pids).collect();
    assert_eq!(trees, ["1(2(1))"]);
}

fn index(table: &str, name: &str, columns: &[&str], unique: bool) -> IndexRow {
    IndexRow {
        table: table.to_string(),
        name: name.to_string(),
        columns: columns.iter().map(|column| (*column).to_string()).collect(),
        unique,
        primary: false,
        valid: true,
    }
}

/// An index that leads another of its table is a duplicate; a unique one, or one on another table, is not; of two
/// alike one is reported.
#[test]
fn an_index_another_leads_with_is_a_duplicate() {
    let indexes = [
        index("t", "t_a", &["a"], false),
        index("t", "t_a_b", &["a", "b"], false),
        index("t", "t_b", &["b"], false),
        index("t", "t_a_unique", &["a"], true),
        index("u", "u_a", &["a"], false),
        index("u", "u_a_again", &["a"], false),
    ];
    let found: Vec<String> = duplicates(&indexes)
        .iter()
        .map(|row| {
            let Value::Map(row) = row else { panic!("a row") };
            format!(
                "{} by {}",
                moqui_lib::object::to_plain_string(&row["index_name"]),
                moqui_lib::object::to_plain_string(&row["covered_by"])
            )
        })
        .collect();
    assert_eq!(found, ["t_a by t_a_b", "u_a_again by u_a"]);
}
