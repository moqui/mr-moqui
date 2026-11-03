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
use crate::edge::{EdgeSettings, NodeRole};
use moqui_lib::object::Value;

fn node_of(role: NodeRole) -> crate::context::ExecutionContext {
    let ctx = ExecutionContextFactory::test_context();
    ctx.factory().extension(|_| EdgeSettings {
        role,
        ..EdgeSettings::default()
    });
    ctx
}

#[test]
fn a_scope_holds_inside_and_the_one_before_is_back_after() {
    assert_eq!(scoped(), None);
    let inner = with(Owner::paired("local-a"), || {
        assert_eq!(scoped(), Some(Owner::paired("local-a")));
        with(Owner::Own, scoped)
    });
    assert_eq!(inner, Some(Owner::Own));
    assert_eq!(scoped(), None);
    // a panic inside does not leave the scope set
    let caught = std::panic::catch_unwind(|| with(Owner::paired("local-b"), || panic!("inside")));
    assert!(caught.is_err());
    assert_eq!(scoped(), None);
    assert_eq!(Owner::paired("local-a").node(), Some("local-a"));
    assert_eq!(Owner::Own.node(), None);
}

#[test]
fn a_normal_or_all_in_one_node_owns_an_item_unless_a_scope_names_a_paired_node() {
    for role in [NodeRole::Normal, NodeRole::AllInOne] {
        let ctx = node_of(role);
        assert_eq!(current(ctx.factory()).unwrap(), Owner::Own, "{role:?}");
        let paired = with(Owner::paired("local-a"), || current(ctx.factory()).unwrap());
        assert_eq!(paired, Owner::paired("local-a"), "{role:?}");
    }
}

#[test]
fn a_dedicated_edge_node_s_items_are_its_only_paired_node_s() {
    let ctx = node_of(NodeRole::Edge);
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    let err = current(ctx.factory()).unwrap_err();
    assert!(err.message().contains("no local node is paired"), "{}", err.message());
    let mut client = facade.make_value("moqui.edge.EdgeClient").unwrap();
    client.set("pairedNodeId", Value::string("local-e"));
    client.set("keyFingerprint", Value::string("ab"));
    client.set("statusId", Value::string("EdgeClientActive"));
    client.create(&facade).unwrap();
    assert_eq!(current(ctx.factory()).unwrap(), Owner::paired("local-e"));
    // a scope still holds
    assert_eq!(with(Owner::Own, || current(ctx.factory()).unwrap()), Owner::Own);
}
