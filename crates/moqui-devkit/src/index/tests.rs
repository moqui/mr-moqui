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

fn artifact(id: &str, kind: &str, name: &str) -> ArtifactRow {
    ArtifactRow {
        id: id.into(),
        kind: kind.into(),
        name: name.into(),
        short_name: extract::short_of(kind, name),
        component: "c".into(),
        path: String::new(),
        line: 1,
        column: 1,
        summary: String::new(),
        parent: None,
    }
}

#[test]
fn ranking_puts_exact_and_short_matches_first() {
    let candidates = vec![
        artifact("entity:mantle.order.OrderItem", "entity", "mantle.order.OrderItem"),
        artifact("entity:mantle.order.OrderHeader", "entity", "mantle.order.OrderHeader"),
        artifact(
            "field:mantle.order.OrderHeader.orderId",
            "field",
            "mantle.order.OrderHeader.orderId",
        ),
        artifact(
            "service:mantle.order.OrderServices.place#Order",
            "service",
            "mantle.order.OrderServices.place#Order",
        ),
    ];
    let found = rank(candidates.clone(), "OrdHdr", 10);
    assert_eq!(found[0].id, "entity:mantle.order.OrderHeader");
    assert!(found
        .iter()
        .all(|artifact| artifact.id != "entity:mantle.order.OrderItem"));
    let found = rank(candidates.clone(), "place#Order", 10);
    assert_eq!(found[0].id, "service:mantle.order.OrderServices.place#Order");
    let found = rank(candidates, "entity:mantle.order.OrderItem", 1);
    assert_eq!(found[0].id, "entity:mantle.order.OrderItem");
}
