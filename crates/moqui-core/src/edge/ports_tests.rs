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
use crate::context::ExecutionContext;

fn open() -> (ExecutionContext, EntityFacade) {
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    (ctx, facade)
}

fn paired(facade: &EntityFacade, node: &str, status: &str) {
    let mut row = facade.make_value(CLIENT).unwrap();
    for (field, value) in [("pairedNodeId", node), ("keyFingerprint", "ab"), ("statusId", status)] {
        row.set(field, Value::string(value));
    }
    row.create(facade).unwrap();
}

#[test]
fn a_port_row_goes_to_one_active_pairing_and_the_link_is_told() {
    let (ctx, facade) = open();
    let factory = ctx.factory();
    paired(&facade, "local-a", "EdgeClientActive");
    paired(&facade, "local-b", "EdgeClientActive");
    paired(&facade, "local-gone", "EdgeClientRevoked");
    let before = generation(factory);
    let added = add(factory, &facade, "47022", "local-a", "SSH").unwrap();
    assert_eq!(
        added,
        EdgePort {
            port: 47022,
            paired_node_id: "local-a".to_string(),
            tunnel_kind: "ssh".to_string()
        }
    );
    assert!(generation(factory) > before, "the edge link is told");
    add(factory, &facade, "47023", "local-b", "xmpps").unwrap();
    assert_eq!(
        add(factory, &facade, "47022", "local-a", "ssh").unwrap(),
        added,
        "the same row again"
    );
    let refused = |port: &str, node: &str, kind: &str| add(factory, &facade, port, node, kind).unwrap_err().to_string();
    assert!(
        refused("47022", "local-b", "ssh").contains("remove it first"),
        "a port another pairing has"
    );
    assert!(
        refused("47024", "local-gone", "ssh").contains("not an active"),
        "a revoked pairing"
    );
    assert!(refused("47024", "local-a", "telnet").contains("tunnelKind"));
    assert!(refused("0", "local-a", "ssh").contains("not a port"));
    assert!(refused("70000", "local-a", "ssh").contains("not a port"));
    assert_eq!(list(&facade, Some("local-b")).unwrap().len(), 1);
    assert_eq!(
        list(&facade, None)
            .unwrap()
            .iter()
            .map(|port| port.port)
            .collect::<Vec<_>>(),
        vec![47022, 47023]
    );
    let at = generation(factory);
    assert!(remove(factory, &facade, "47022").unwrap());
    assert!(!remove(factory, &facade, "47022").unwrap());
    assert!(generation(factory) > at);
    assert_eq!(list(&facade, None).unwrap().len(), 1);
}

#[test]
fn a_port_of_this_nodes_own_listeners_is_no_rows() {
    let properties = |name: &str| match name {
        "edge_ssh_tunnel_port" => "2222".to_string(),
        "webapp_https_listen_port" => " 443 ".to_string(),
        _ => String::new(),
    };
    let reserved = |port: u16| reserved_in(port, properties);
    assert_eq!(reserved(2222).as_deref(), Some("edge_ssh_tunnel_port"));
    assert_eq!(
        reserved(443).as_deref(),
        Some("webapp_https_port"),
        "set or not, the usual port"
    );
    // the usual ports of services whose properties are unset, and the mail relay beside an edge node
    assert_eq!(reserved(25).as_deref(), Some("smtp_listen_port"));
    assert_eq!(reserved(5060).as_deref(), Some("sip_port"));
    assert_eq!(reserved(3478).as_deref(), Some("turn_port"));
    assert_eq!(reserved(10025).as_deref(), Some("edge_mail_relay_port"));
    assert_eq!(reserved(587).as_deref(), Some("mail submission"));
    // the media ranges
    assert!(reserved(20500).is_some_and(|what| what.starts_with("sip_rtp_ports")));
    assert!(reserved(50000).is_some_and(|what| what.starts_with("turn_relay_ports")));
    assert_eq!(reserved(47022), None);
}
