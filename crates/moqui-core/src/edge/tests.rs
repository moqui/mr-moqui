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
    assert_eq!(NodeRole::parse("all-in-one"), NodeRole::AllInOne);
    assert_eq!(NodeRole::parse("ALL-IN-ONE"), NodeRole::AllInOne);
    assert_eq!(NodeRole::parse("allinone"), NodeRole::Normal);
    assert_eq!(NodeRole::parse("local"), NodeRole::Normal);
    for role in [NodeRole::Normal, NodeRole::Edge, NodeRole::AllInOne] {
        assert_eq!(NodeRole::parse(role.as_str()), role);
    }
}

#[test]
fn an_edge_and_an_all_in_one_node_take_links_and_only_the_edge_node_is_dedicated() {
    let roles = [NodeRole::Normal, NodeRole::Edge, NodeRole::AllInOne];
    let of = |test: fn(NodeRole) -> bool| roles.map(test);
    assert_eq!(of(NodeRole::edge_node), [false, true, false]);
    assert_eq!(of(NodeRole::all_in_one), [false, false, true]);
    assert_eq!(of(NodeRole::takes_links), [false, true, true]);
}
