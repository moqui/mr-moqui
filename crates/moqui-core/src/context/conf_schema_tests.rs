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

use moqui_lib::node::MNode;

use super::{keying, unit, Key};

#[test]
fn keying_follows_the_units_and_takes_an_unknown_element_as_single() {
    assert_eq!(keying(&["webapp-list", "webapp"]), Key::By(&["name"]));
    assert_eq!(
        keying(&["webapp-list", "webapp", "response-header"]),
        Key::By(&["type", "name"])
    );
    assert_eq!(keying(&["entity-facade", "load-data"]), Key::Sequence);
    assert_eq!(keying(&["ssh-server"]), Key::Single);
    assert_eq!(keying(&["pass"]), Key::Single);
    assert_eq!(keying(&["webapp-list", "webapp", "no-such-child"]), Key::Single);
    assert!(unit(&["llm-facade", "endpoint", "header"]).is_some());
    assert!(unit(&["nothing"]).is_none());
}

/// Every element the default conf repeats under one parent is keyed or a sequence in the schema, so an environment
/// name never takes the first of several by accident (plans/archive/Conf-Paths.md §1).
#[test]
fn every_repeated_element_of_the_default_conf_is_keyed_or_a_sequence() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/components/base/framework/conf/MoquiDefaultConf.xml"
    );
    let conf = MNode::parse_file(path).unwrap();
    let mut missing = Vec::new();
    walk(&conf, &mut Vec::new(), &mut missing);
    assert!(
        missing.is_empty(),
        "repeated with no key in the conf schema: {missing:?}"
    );
}

fn walk(node: &MNode, path: &mut Vec<String>, missing: &mut Vec<String>) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for child in node.children() {
        *counts.entry(child.name()).or_default() += 1;
    }
    for child in node.children() {
        path.push(child.name());
        let names: Vec<&str> = path.iter().map(String::as_str).collect();
        if counts[&child.name()] > 1 && keying(&names) == Key::Single {
            let joined = names.join("/");
            if !missing.contains(&joined) {
                missing.push(joined);
            }
        }
        walk(&child, path, missing);
        path.pop();
    }
}

/// The settings the review's fixes read on each use are live, though their elements restart (SEC-TLS-101, SEC-DAT-84).
#[test]
fn settings_read_on_each_use_are_live() {
    let live = |path: &[&str], attr: &str| unit(path).expect("a unit").attr_class(attr) == super::Class::Live;
    assert!(live(&["tools"], "external-database-private-network"));
    assert!(live(&["tools"], "database-audit-days"));
    assert!(live(&["search-facade"], "query-timeout-seconds"));
    assert!(live(&["search-facade"], "query-max-clauses"));
    assert!(!live(&["search-facade"], "backend"));
}
