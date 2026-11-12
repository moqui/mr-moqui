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

fn row(id: &str, kind: &str, name: &str, component: &str, path: &str, line: u32) -> ArtifactRow {
    ArtifactRow {
        id: id.into(),
        kind: kind.into(),
        name: name.into(),
        short_name: name.rsplit(['.', '/']).next().unwrap_or(name).into(),
        component: component.into(),
        path: path.into(),
        line,
        column: 1,
        summary: format!("about {name}"),
        parent: None,
        origin: String::new(),
        marks: String::new(),
    }
}

fn components() -> Vec<(String, String)> {
    vec![
        ("mantle".into(), "/r/mantle".into()),
        ("marble".into(), "/r/marble".into()),
        ("my-site".into(), "/r/my-site".into()),
    ]
}

fn rows() -> Vec<ArtifactRow> {
    let mut derived = row(
        "service:create#mantle.order.OrderHeader",
        "service",
        "create#mantle.order.OrderHeader",
        "mantle",
        "derived:entity-auto",
        0,
    );
    derived.origin = "derived".into();
    let mut method = row(
        "rest:/mantle/orders:get",
        "rest",
        "/mantle/orders:get",
        "mantle",
        "/r/mantle/service/mantle.rest.xml",
        4,
    );
    method.parent = Some("rest:/mantle/orders".into());
    vec![
        row(
            "entity:mantle.order.OrderHeader",
            "entity",
            "mantle.order.OrderHeader",
            "mantle",
            "/r/mantle/entity/OrderEntities.xml",
            10,
        ),
        row(
            "entity:mantle.order.OrderItem",
            "entity",
            "mantle.order.OrderItem",
            "mantle",
            "/r/mantle/entity/OrderEntities.xml",
            40,
        ),
        row(
            "entity:mantle.order.OrderSummary",
            "view-entity",
            "mantle.order.OrderSummary",
            "mantle",
            "/r/mantle/entity/OrderViewEntities.xml",
            5,
        ),
        row(
            "service:mantle.order.OrderServices.place#Order",
            "service",
            "mantle.order.OrderServices.place#Order",
            "mantle",
            "/r/mantle/service/mantle/order/OrderServices.xml",
            30,
        ),
        row(
            "service:mantle.order.OrderServices.cancel#Order",
            "service",
            "mantle.order.OrderServices.cancel#Order",
            "mantle",
            "/r/mantle/service/mantle/order/OrderServices.xml",
            12,
        ),
        row(
            "service:mantle.order.OrderServices.store#mantle.order.OrderItem",
            "service",
            "mantle.order.OrderServices.store#mantle.order.OrderItem",
            "mantle",
            "/r/mantle/service/mantle/order/OrderServices.xml",
            50,
        ),
        derived,
        row(
            "screen:component://marble/screen/SimpleScreens/Order/OrderDetail.xml",
            "screen",
            "component://marble/screen/SimpleScreens/Order/OrderDetail.xml",
            "marble",
            "/r/marble/screen/SimpleScreens/Order/OrderDetail.xml",
            1,
        ),
        row(
            "screen:component://my-site/screen/SimpleScreens/Order/OrderDetail.xml",
            "screen",
            "component://my-site/screen/SimpleScreens/Order/OrderDetail.xml",
            "my-site",
            "/r/my-site/screen/SimpleScreens/Order/OrderDetail.xml",
            1,
        ),
        row(
            "screen:component://marble/screen/SimpleScreens.xml",
            "screen",
            "component://marble/screen/SimpleScreens.xml",
            "marble",
            "/r/marble/screen/SimpleScreens.xml",
            1,
        ),
        row(
            "seca:component://mantle/service/mantle/order/OrderServices.secas.xml#a",
            "seca",
            "seca a",
            "mantle",
            "/r/mantle/service/mantle/order/OrderServices.secas.xml",
            3,
        ),
        row(
            "seca:component://mantle/service/mantle/order/OrderServices.secas.xml#b",
            "seca",
            "seca b",
            "mantle",
            "/r/mantle/service/mantle/order/OrderServices.secas.xml",
            9,
        ),
        row(
            "rest:/mantle",
            "rest",
            "/mantle",
            "mantle",
            "/r/mantle/service/mantle.rest.xml",
            2,
        ),
        method,
    ]
}

#[test]
fn each_tree_follows_its_package_structure() {
    let plan = DocPlan::build(&rows(), &components());
    let path = |id: &str| plan.of_id(id).map(|entry| entry.path.clone()).unwrap_or_default();
    assert_eq!(
        path("entity:mantle.order.OrderHeader"),
        "entity/mantle/order/OrderHeader.md"
    );
    assert_eq!(
        path("entity:mantle.order.OrderSummary"),
        "entity/mantle/order/OrderSummary.md"
    );
    assert_eq!(
        path("service:mantle.order.OrderServices.place#Order"),
        "service/mantle/order/OrderServices/place-Order.md"
    );
    assert_eq!(
        path("service:mantle.order.OrderServices.store#mantle.order.OrderItem"),
        "service/mantle/order/OrderServices/store-mantle.order.OrderItem.md",
        "a noun with dots keeps them"
    );
    assert_eq!(
        path("service:create#mantle.order.OrderHeader"),
        "",
        "a derived service has no doc"
    );
    assert_eq!(
        plan.of_file("/r/mantle/service/mantle/order/OrderServices.xml")
            .map(|entry| entry.path.as_str()),
        Some("service/mantle/order/OrderServices.md")
    );
    assert_eq!(
        path("screen:component://marble/screen/SimpleScreens.xml"),
        "screen/SimpleScreens.md"
    );
    assert_eq!(
        path("seca:component://mantle/service/mantle/order/OrderServices.secas.xml#b"),
        "eca/service/mantle/order/OrderServices.secas.md",
        "a rule's doc is its file's"
    );
    assert_eq!(
        plan.of_file("/r/mantle/service/mantle.rest.xml")
            .map(|entry| entry.path.as_str()),
        Some("rest/mantle.md")
    );
    let services: Vec<&str> = plan
        .services_of("/r/mantle/service/mantle/order/OrderServices.xml")
        .iter()
        .map(|entry| entry.title.as_str())
        .collect();
    assert_eq!(
        services,
        ["cancel#Order", "place#Order", "store#mantle.order.OrderItem"],
        "in file order"
    );
}

#[test]
fn two_units_at_one_path_take_their_components_names() {
    let plan = DocPlan::build(&rows(), &components());
    let path = |id: &str| plan.of_id(id).map(|entry| entry.path.clone()).unwrap_or_default();
    assert_eq!(
        path("screen:component://marble/screen/SimpleScreens/Order/OrderDetail.xml"),
        "screen/SimpleScreens/Order/OrderDetail.marble.md"
    );
    assert_eq!(
        path("screen:component://my-site/screen/SimpleScreens/Order/OrderDetail.xml"),
        "screen/SimpleScreens/Order/OrderDetail.my-site.md"
    );
}

#[test]
fn packages_have_pages_and_every_doc_a_way_up() {
    let plan = DocPlan::build(&rows(), &components());
    for path in [
        "README.md",
        "entity/README.md",
        "entity/mantle/order/README.md",
        "service/mantle/order/README.md",
        "screen/SimpleScreens/Order/README.md",
        "eca/README.md",
        "rest/README.md",
    ] {
        assert!(plan.at_path(path).is_some(), "{path}");
    }
    assert!(plan.at_path("screen/README.md").is_some());
    assert!(
        plan.at_path("eca/service/mantle/order/README.md").is_none(),
        "ECA files are listed on the tree's index"
    );
    let up = |path: &str| {
        let doc = plan.at_path(path).unwrap();
        plan.parent_of(doc).map(|entry| entry.path.clone()).unwrap_or_default()
    };
    assert_eq!(
        up("entity/mantle/order/OrderHeader.md"),
        "entity/mantle/order/README.md"
    );
    assert_eq!(
        up("service/mantle/order/OrderServices/place-Order.md"),
        "service/mantle/order/OrderServices.md"
    );
    assert_eq!(
        up("service/mantle/order/OrderServices.md"),
        "service/mantle/order/README.md"
    );
    assert_eq!(
        up("screen/SimpleScreens.md"),
        "screen/README.md",
        "a doc at a tree's top goes to its index"
    );
    assert_eq!(up("eca/service/mantle/order/OrderServices.secas.md"), "eca/README.md");
    assert_eq!(up("entity/mantle/order/README.md"), "entity/README.md");
    assert_eq!(up("entity/README.md"), "README.md");
    let members: Vec<&str> = plan
        .members(DocKind::Entity, "mantle.order")
        .iter()
        .map(|entry| entry.title.as_str())
        .collect();
    assert_eq!(members, ["OrderHeader", "OrderItem", "OrderSummary"]);
    assert_eq!(
        plan.packages_of(DocKind::Screen),
        [(String::new(), 1), ("SimpleScreens/Order".to_string(), 2)]
    );
}

#[test]
fn an_artifacts_doc_is_its_owners() {
    let plan = DocPlan::build(&rows(), &components());
    let doc = |id: &str, kind: &str, path: &str| {
        let row = row(id, kind, id, "x", path, 1);
        plan.of_artifact(&row)
            .map(|entry| entry.path.clone())
            .unwrap_or_default()
    };
    assert_eq!(
        doc(
            "transition:component://marble/screen/SimpleScreens.xml#find:any",
            "transition",
            ""
        ),
        "screen/SimpleScreens.md"
    );
    assert_eq!(
        doc("field:mantle.order.OrderHeader.statusId", "field", ""),
        "entity/mantle/order/OrderHeader.md"
    );
    assert_eq!(
        doc(
            "relationship:mantle.order.OrderItem@mantle.order.OrderHeader",
            "relationship",
            ""
        ),
        "entity/mantle/order/OrderItem.md"
    );
    assert_eq!(
        doc(
            "parameter:service:mantle.order.OrderServices.place#Order@in.orderId",
            "parameter",
            ""
        ),
        "service/mantle/order/OrderServices/place-Order.md"
    );
    assert_eq!(
        doc(
            "parameter:transition:component://marble/screen/SimpleScreens.xml#find:any@in.q",
            "parameter",
            ""
        ),
        "screen/SimpleScreens.md"
    );
    assert_eq!(
        doc("rest:/mantle/orders:get", "rest", "/r/mantle/service/mantle.rest.xml"),
        "rest/mantle.md"
    );
    assert_eq!(
        doc("data:component://mantle/data/Seed.xml", "data", ""),
        "",
        "a data file has no doc"
    );
}

#[test]
fn links_are_relative_between_tree_paths() {
    assert_eq!(
        relative_link("entity/mantle/order/OrderHeader.md", "entity/mantle/order/OrderItem.md"),
        "OrderItem.md"
    );
    assert_eq!(
        relative_link("entity/mantle/order/OrderHeader.md", "entity/moqui/basic/StatusItem.md"),
        "../../moqui/basic/StatusItem.md"
    );
    assert_eq!(relative_link("README.md", "entity/README.md"), "entity/README.md");
    assert_eq!(relative_link("service/a/B/c-D.md", "README.md"), "../../../README.md");
    assert_eq!(
        relative_link("screen/A.md", "screen/My Dir/B (2).md"),
        "My%20Dir/B%20%282%29.md"
    );
    assert_eq!(DocKind::parse(" Screen "), Some(DocKind::Screen));
    assert_eq!(DocKind::parse("data"), None);
}
