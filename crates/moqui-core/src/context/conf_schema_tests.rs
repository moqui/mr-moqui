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

/// The web listeners' switches, ports and addresses apply while the server runs: the listener registry's hook binds
/// them again (plans/Runtime-Config.md §3.3, RC2); the ports of the URLs the server writes still wait for a restart.
#[test]
fn the_web_listener_settings_apply_through_the_listeners_hook() {
    let webapp = unit(&["webapp-list", "webapp"]).expect("the webapp unit");
    assert_eq!(super::Group::Listeners.name(), "listeners");
    for setting in [
        "http-listen-enabled",
        "http-listen-port",
        "https-listen-enabled",
        "https-listen-port",
        "http-bind",
        "grpc-listen-enabled",
        "grpc-listen-port",
        "grpc-listen-bind",
    ] {
        assert_eq!(webapp.attr_class(setting), super::LISTENERS, "{setting}");
    }
    assert_eq!(webapp.attr_class("http-port"), super::LISTENER);
    assert_eq!(webapp.attr_class("http-host"), super::Class::Live);
}

/// `attribute_class` walks the catalog as the reload's diff does: a keyed unit's own class, an attribute's class in
/// its unit, the nearest unit's class for an element no unit names, and a component's class at the top.
#[test]
fn attribute_class_walks_the_catalog_as_the_diff_does() {
    use super::{attribute_class, Class, Group, COMPONENT, LISTENER, LISTENERS, UNKNOWN};
    let webapp = ["webapp-list", "webapp"];
    assert_eq!(attribute_class(&webapp, "https-listen-port"), LISTENERS);
    assert_eq!(attribute_class(&webapp, "http-port"), LISTENER);
    assert_eq!(attribute_class(&webapp, "http-host"), Class::Live);
    // a keyed unit below it
    let rate_limit = ["webapp-list", "webapp", "web-protection", "rate-limit"];
    assert_eq!(
        attribute_class(&rate_limit, "limit"),
        Class::Apply(Group::WebProtection)
    );
    // an element no unit names is its parent unit's content: that unit's class, not the class of an attribute of the
    // same name on the unit's own element
    assert_eq!(
        attribute_class(
            &["webapp-list", "webapp", "web-protection", "rate-limit", "note"],
            "text"
        ),
        Class::Apply(Group::WebProtection)
    );
    assert_eq!(
        attribute_class(&["webapp-list", "webapp", "no-unit", "deeper"], "https-listen-port"),
        Class::Live
    );
    assert_eq!(attribute_class(&["ssh-server"], "port"), LISTENER);
    // a top-level element no unit names is a component's settings
    assert_eq!(attribute_class(&["pass"], "backend-url"), COMPONENT);
    assert_eq!(attribute_class(&["pass", "inner"], "backend-url"), COMPONENT);
    assert_eq!(attribute_class(&[], "anything"), UNKNOWN);
}
