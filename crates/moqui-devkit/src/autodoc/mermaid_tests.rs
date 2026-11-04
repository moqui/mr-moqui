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

//! Goldens of each diagram in `testdata/mermaid/` (`UPDATE_GOLDEN=1` writes them again; read the diff). The webroot's
//! markdown page test parses the same files with the vendored Mermaid, so a golden is also a syntax check.

use super::*;

fn golden(name: &str, text: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/autodoc/testdata/mermaid")
        .join(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("no golden {}", path.display()));
    assert_eq!(
        text, expected,
        "{name} differs from its golden; UPDATE_GOLDEN=1 writes it"
    );
}

fn names(prefix: &str, count: usize) -> Vec<String> {
    (1..=count).map(|at| format!("{prefix}{at}")).collect()
}

#[test]
fn labels_quote_what_would_end_or_open_them() {
    assert_eq!(label("place#Order"), "\"place#35;Order\"");
    assert_eq!(label("a \"b\" <c>\nd"), "\"a #quot;b#quot; #lt;c#gt; d\"");
    assert_eq!(safe_id("mantle.order.OrderHeader"), "mantle_order_OrderHeader");
    assert_eq!(safe_id("9lives"), "n9lives");
    let mut ids = Ids::default();
    assert_eq!(ids.id("a", "end"), "end_2", "a keyword is never an id");
    assert_eq!(ids.id("b", "x.y"), "x_y");
    assert_eq!(ids.id("c", "x_y"), "x_y_2");
    assert_eq!(ids.id("b", "other"), "x_y", "a key keeps its id");
}

#[test]
fn the_usage_flowchart_has_a_group_per_kind_capped_with_more() {
    let used_by = vec![
        UsageGroup {
            label: "Screens".into(),
            names: names("Screen", 10),
            observed_only: false,
        },
        UsageGroup {
            label: "Services".into(),
            names: vec!["mantle.order.OrderServices.place#Order".into()],
            observed_only: true,
        },
    ];
    let uses = vec![UsageGroup {
        label: "Entities".into(),
        names: vec!["OrderHeader".into(), "Order \"Item\"".into()],
        observed_only: false,
    }];
    let text = usage_flowchart("create#Order", &used_by, &uses);
    assert!(text.contains("used_by_1_more[\"+ 2 more\"]"));
    assert!(!text.contains("used_by_1_9["), "eight rows a group");
    assert!(text.contains("used_by_2 -.-> center"), "observed only is dashed");
    assert!(text.contains("center --> uses_1"));
    golden("usage.mmd", &text);
    golden("usage-empty.mmd", &usage_flowchart("lonely", &[], &[]));
}

#[test]
fn the_er_diagram_keys_its_fields_and_draws_each_cardinality() {
    let attribute = |field_type: &str, name: &str, primary: bool, foreign: bool| ErAttribute {
        field_type: field_type.into(),
        name: name.into(),
        primary,
        foreign,
    };
    let entities = vec![
        ErEntity {
            full_name: "mantle.order.OrderHeader".into(),
            short_name: "OrderHeader".into(),
            stub: false,
            attributes: vec![
                attribute("id", "orderId", true, false),
                attribute("id", "statusId", false, true),
                attribute("date-time", "placedDate", false, false),
            ],
        },
        ErEntity {
            full_name: "mantle.order.OrderItem".into(),
            short_name: "OrderItem".into(),
            stub: false,
            attributes: vec![
                attribute("id", "orderId", true, true),
                attribute("id", "orderItemSeqId", true, false),
            ],
        },
        ErEntity {
            full_name: "moqui.basic.StatusItem".into(),
            short_name: "StatusItem".into(),
            stub: true,
            attributes: Vec::new(),
        },
        ErEntity {
            full_name: "other.pkg.OrderItem".into(),
            short_name: "OrderItem".into(),
            stub: true,
            attributes: Vec::new(),
        },
    ];
    let links = vec![
        ErLink {
            from: "mantle.order.OrderItem".into(),
            to: "mantle.order.OrderHeader".into(),
            cardinality: Cardinality::One,
            title: String::new(),
        },
        ErLink {
            from: "mantle.order.OrderHeader".into(),
            to: "moqui.basic.StatusItem".into(),
            cardinality: Cardinality::OneNoFk,
            title: "Order \"Status\"".into(),
        },
        ErLink {
            from: "mantle.order.OrderHeader".into(),
            to: "other.pkg.OrderItem".into(),
            cardinality: Cardinality::Many,
            title: "Other".into(),
        },
        ErLink {
            from: "mantle.order.OrderHeader".into(),
            to: "not.listed.Entity".into(),
            cardinality: Cardinality::One,
            title: String::new(),
        },
    ];
    let text = er_diagram(&entities, &links);
    assert!(
        text.contains("mantle_order_OrderItem {"),
        "a shared short name takes the full name"
    );
    assert!(text.contains("id orderId PK, FK"));
    assert!(text.contains("date-time placedDate\n"));
    assert!(text.contains("}o..||"));
    assert!(
        !text.contains("not_listed"),
        "a link to an entity not listed is left out"
    );
    golden("er.mmd", &text);
}

#[test]
fn the_class_diagram_has_a_class_per_file_and_each_link_once() {
    let classes = vec![
        ClassBox {
            key: "service/mantle/order/OrderServices.xml".into(),
            name: "OrderServices".into(),
            members: vec!["place#Order".into(), "cancel#Order".into(), "get#Order<T>".into()],
            interface: false,
        },
        ClassBox {
            key: "service/mantle/order/OrderInfoServices.xml".into(),
            name: "OrderInfoServices".into(),
            members: vec!["get#OrderDisplayInfo".into()],
            interface: false,
        },
        ClassBox {
            key: "mantle.order.OrderServices.define#OrderParams".into(),
            name: "define#OrderParams".into(),
            members: Vec::new(),
            interface: true,
        },
    ];
    let links = vec![
        ClassLink {
            from: "service/mantle/order/OrderServices.xml".into(),
            to: "service/mantle/order/OrderInfoServices.xml".into(),
            kind: ClassLinkKind::Depends,
        },
        ClassLink {
            from: "service/mantle/order/OrderServices.xml".into(),
            to: "service/mantle/order/OrderInfoServices.xml".into(),
            kind: ClassLinkKind::Depends,
        },
        ClassLink {
            from: "service/mantle/order/OrderServices.xml".into(),
            to: "mantle.order.OrderServices.define#OrderParams".into(),
            kind: ClassLinkKind::Realizes,
        },
        ClassLink {
            from: "service/mantle/order/OrderServices.xml".into(),
            to: "service/mantle/order/OrderServices.xml".into(),
            kind: ClassLinkKind::Depends,
        },
    ];
    let text = class_diagram(&classes, &links);
    assert_eq!(text.matches("..>").count(), 1, "each link once, none to itself");
    assert!(text.contains("+place_Order()"));
    assert!(text.contains("+get_OrderT()"), "no generic opened by a name");
    assert!(text.contains("<<interface>>"));
    golden("class.mmd", &text);
}

#[test]
fn the_flowchart_draws_each_shape_and_each_edge_once() {
    let node = |key: &str, text: &str, shape: Shape| FlowNode {
        key: key.into(),
        label: text.into(),
        shape,
    };
    let edge = |from: &str, to: &str, text: &str, dashed: bool| FlowEdge {
        from: from.into(),
        to: to.into(),
        label: text.into(),
        dashed,
    };
    let nodes = vec![
        node("screen:a", "Order", Shape::Subject),
        node("screen:b", "OrderDetail", Shape::Box),
        node("seca:x", "seca post place#Order", Shape::Round),
        node("screen:c", "Elsewhere", Shape::Stub),
        node("screen:b", "duplicate", Shape::Box),
    ];
    let edges = vec![
        edge("screen:a", "screen:b", "Detail", false),
        edge("screen:a", "screen:b", "Detail", false),
        edge("screen:b", "screen:c", "", true),
        edge("screen:a", "screen:missing", "", false),
        edge("seca:x", "screen:a", "", false),
    ];
    let text = flowchart("TD", &nodes, &edges);
    assert_eq!(text.matches("-->|").count(), 1);
    assert!(text.contains("-.->"));
    assert!(!text.contains("missing"));
    assert!(text.contains("seca_post_place_Order(\"seca post place#35;Order\")"));
    golden("flow.mmd", &text);
}
