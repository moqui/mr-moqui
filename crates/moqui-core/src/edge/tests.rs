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
fn a_node_role_is_read_in_any_case_and_an_unknown_one_is_a_normal_node() {
    assert_eq!(NodeRole::parse(""), NodeRole::Normal);
    assert_eq!(NodeRole::parse(" Edge "), NodeRole::Edge);
    assert_eq!(NodeRole::parse("front_edge"), NodeRole::FrontEdge);
    assert_eq!(NodeRole::parse("FRONT"), NodeRole::Front);
    assert_eq!(NodeRole::parse("all-in-one"), NodeRole::AllInOne);
    assert_eq!(NodeRole::parse("ALL-IN-ONE"), NodeRole::AllInOne);
    assert_eq!(NodeRole::parse("allinone"), NodeRole::Normal);
    assert_eq!(NodeRole::parse("front-edge"), NodeRole::Normal);
    assert_eq!(NodeRole::parse("local"), NodeRole::Normal);
    for role in ROLES {
        assert_eq!(NodeRole::parse(role.as_str()), role);
    }
}

const ROLES: [NodeRole; 5] = [
    NodeRole::Normal,
    NodeRole::Front,
    NodeRole::FrontEdge,
    NodeRole::Edge,
    NodeRole::AllInOne,
];

#[test]
fn every_role_takes_links_and_each_has_its_sets() {
    // plans/Edge-Node2.md §4.3: normal, front, front_edge, edge, all-in-one
    let of = |test: fn(NodeRole) -> bool| ROLES.map(test);
    assert_eq!(of(NodeRole::takes_links), [false, true, true, true, true]);
    assert_eq!(of(NodeRole::has_front), [false, true, true, false, true]);
    assert_eq!(of(NodeRole::has_edge), [false, false, true, true, true]);
    assert_eq!(of(NodeRole::edge_node), [false, true, true, true, false]);
    assert_eq!(of(NodeRole::all_in_one), [false, false, false, false, true]);
}
