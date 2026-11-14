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

use moqui_core::edge::{EdgeInfo, NodeRole};

use super::{pairing_properties, pairing_slot, Pairing};

/// Pairing with a front node keeps it as the front pairing; anything else is the edge pairing, which a node that takes
/// links does not have; a front node pairs with nothing (plans/Edge-Node2.md §4.7).
#[test]
fn a_pairing_goes_where_the_other_nodes_role_says() {
    assert_eq!(pairing_slot(NodeRole::Normal, Some("front")), Ok(Pairing::Front));
    assert_eq!(pairing_slot(NodeRole::Normal, Some("front_edge")), Ok(Pairing::Edge));
    assert_eq!(pairing_slot(NodeRole::Normal, Some("")), Ok(Pairing::Edge));
    for role in [NodeRole::FrontEdge, NodeRole::Edge, NodeRole::AllInOne] {
        assert_eq!(pairing_slot(role, None), Ok(Pairing::Edge), "{role:?} may pair");
        assert_eq!(pairing_slot(role, Some("front")), Ok(Pairing::Front));
        let refused = pairing_slot(role, Some("front_edge")).unwrap_err();
        assert!(refused.contains("only with a front node"), "{refused}");
    }
    assert!(pairing_slot(NodeRole::Front, None).is_err());
    assert!(pairing_slot(NodeRole::Front, Some("front")).is_err());
}

/// The front pairing writes `front_node_*` and no pairing id; the edge pairing `edge_node_*` with the pairing id.
#[test]
fn each_pairing_writes_its_own_properties() {
    let paired = crate::edge_link::pair::Paired {
        edge: EdgeInfo {
            instance_id: "front-1".to_string(),
            ..EdgeInfo::default()
        },
        pin: "ab".repeat(32),
        key: "key".to_string(),
        key_fingerprint: "cd".repeat(32),
    };
    let front = pairing_properties(Pairing::Front, " https://front.example.com ", &paired, "local-1");
    let names: Vec<&str> = front.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        ["front_node_url", "front_node_key", "front_node_pin", "front_node_id"]
    );
    assert_eq!(front[0].1, "https://front.example.com");
    assert_eq!(front[3].1, "front-1");
    let edge = pairing_properties(Pairing::Edge, "https://edge.example.com", &paired, "local-1");
    let names: Vec<&str> = edge.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "edge_node_url",
            "edge_node_key",
            "edge_node_pin",
            "edge_node_id",
            "edge_node_pairing"
        ]
    );
    assert_eq!(edge[4].1, "local-1");
}
