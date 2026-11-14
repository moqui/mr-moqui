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

use std::net::IpAddr;

use crate::guard::BanNotice;

fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

#[test]
fn a_notice_is_told_as_a_ban_or_a_lift_from_this_member() {
    let ban = BanNotice {
        address: ip("203.0.113.9"),
        seconds: 600,
        ban: "auth".to_string(),
        lift: false,
    };
    let body = super::body("m-1", &ban);
    assert_eq!(body["kind"], "ban");
    assert_eq!(body["address"], "203.0.113.9");
    assert_eq!(body["seconds"], 600);
    assert_eq!(body["from"], "m-1");
    let lift = BanNotice {
        lift: true,
        seconds: 0,
        ..ban
    };
    assert_eq!(super::body("m-1", &lift)["kind"], "lift");
}

/// Whose bans are taken (plans/Edge-Node2.md §4.3): another member's once it is a member, a paired node's only where
/// links are taken and then for its hosts only, never this member's own, nothing malformed.
#[test]
fn a_shared_ban_is_taken_by_whom_it_came_from() {
    use super::{decide, Decision};
    let member =
        serde_json::json!({"kind": "ban", "address": "203.0.113.9", "seconds": 600, "ban": "scan", "from": "m-2"});
    // another member's: once it is a member by its row
    let Decision::IfMember(told) = decide(&member, "m-1", false) else {
        panic!("a member's ban")
    };
    assert_eq!(told.node, None);
    assert_eq!(told.source, "member m-2: scan");
    assert!(!told.lift);
    assert!(matches!(decide(&member, "m-1", true), Decision::IfMember(_)));
    // this member's own, or one naming no member: not taken
    assert_eq!(decide(&member, "m-2", false), Decision::Ignore);
    let mut anonymous = member.clone();
    anonymous["from"] = serde_json::json!("");
    assert_eq!(decide(&anonymous, "m-1", false), Decision::Ignore);
    // a paired node's, stamped by the edge node's link: held for its hosts, on a node that takes links only
    let mut linked = member.clone();
    linked["linkNode"] = serde_json::json!("local-1");
    let Decision::Take(told) = decide(&linked, "edge-1", true) else {
        panic!("a pairing's ban on an edge node")
    };
    assert_eq!(told.node.as_deref(), Some("local-1"));
    assert_eq!(
        decide(&linked, "local-9", false),
        Decision::Ignore,
        "not on a node that takes no links"
    );
    // a pairing's lift lifts only its own
    let mut lift = linked.clone();
    lift["kind"] = serde_json::json!("lift");
    lift["seconds"] = serde_json::json!(0);
    let Decision::Take(told) = decide(&lift, "edge-1", true) else {
        panic!("a pairing's lift")
    };
    assert!(told.lift && told.node.as_deref() == Some("local-1"));
    // malformed: no address, another kind, a ban for no time
    for (field, value) in [("address", "not-an-address"), ("kind", "control")] {
        let mut bad = member.clone();
        bad[field] = serde_json::json!(value);
        assert_eq!(decide(&bad, "m-1", false), Decision::Ignore, "{field}");
    }
    let mut no_time = member.clone();
    no_time["seconds"] = serde_json::json!(0);
    assert_eq!(decide(&no_time, "m-1", false), Decision::Ignore);
}

#[test]
fn bans_go_up_to_the_edge_nodes_named_only() {
    assert_eq!(super::up_links("edge-1", "m-1"), ["edge-1"]);
    assert_eq!(super::up_links(" edge-1, edge-2 ,", "m-1"), ["edge-1", "edge-2"]);
    assert!(
        super::up_links("", "m-1").is_empty(),
        "a node not paired with an edge node tells no link"
    );
    assert!(super::up_links("m-1", "m-1").is_empty());
}
