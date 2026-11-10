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
use crate::index::resolve::Resolver;

fn read(kind: ArtifactKind, location: &str, text: &str) -> FileIndex {
    let info = FileInfo {
        path: "/tmp/x.xml",
        location,
        component: location
            .trim_start_matches("component://")
            .split('/')
            .next()
            .unwrap_or(""),
        kind,
    };
    extract(&info, text).expect("parses")
}

fn ids(index: &FileIndex) -> Vec<&str> {
    index.artifacts.iter().map(|artifact| artifact.id.as_str()).collect()
}

fn refs_of(index: &FileIndex) -> Vec<(String, &str, &str, &str)> {
    index
        .refs
        .iter()
        .map(|row| {
            (
                row.from.clone(),
                row.to_kind.as_str(),
                row.to_name.as_str(),
                row.ref_type.as_str(),
            )
        })
        .collect()
}

const ENTITIES: &str = r#"<entities>
    <entity entity-name="Example" package="moqui.example">
        <description>An example. With more.</description>
        <field name="exampleId" type="id" is-pk="true"/>
        <field name="statusId" type="id"/>
        <relationship type="one" related="moqui.basic.StatusItem"/>
    </entity>
    <view-entity entity-name="ExampleAndStatus" package="moqui.example">
        <member-entity entity-alias="EX" entity-name="moqui.example.Example"/>
        <alias entity-alias="EX" name="exampleId"/>
    </view-entity>
    <extend-entity entity-name="StatusItem" package="moqui.basic">
        <field name="extra" type="text-short"/>
    </extend-entity>
</entities>"#;

#[test]
fn entities_fields_and_extensions() {
    let index = read(
        ArtifactKind::Entity,
        "component://example/entity/ExampleEntities.xml",
        ENTITIES,
    );
    assert_eq!(
        ids(&index),
        [
            "entity:moqui.example.Example",
            "field:moqui.example.Example.exampleId",
            "field:moqui.example.Example.statusId",
            "relationship:moqui.example.Example@moqui.basic.StatusItem",
            "entity:moqui.example.ExampleAndStatus",
            "view-member:moqui.example.ExampleAndStatus@EX",
            "field:moqui.example.ExampleAndStatus.exampleId",
            "extend-entity:component://example/entity/ExampleEntities.xml#moqui.basic.StatusItem",
            "field:moqui.basic.StatusItem.extra",
        ]
    );
    assert_eq!(index.artifacts[0].summary, "An example");
    assert_eq!(index.artifacts[0].line, 2);
    assert_eq!(
        refs_of(&index),
        [
            (
                "relationship:moqui.example.Example@moqui.basic.StatusItem".to_string(),
                "entity",
                "moqui.basic.StatusItem",
                "relationship"
            ),
            (
                "view-member:moqui.example.ExampleAndStatus@EX".to_string(),
                "entity",
                "moqui.example.Example",
                "member-entity"
            ),
            (
                "extend-entity:component://example/entity/ExampleEntities.xml#moqui.basic.StatusItem".to_string(),
                "entity",
                "moqui.basic.StatusItem",
                "extend"
            ),
        ]
    );
}

#[test]
fn services_calls_finds_and_scripts() {
    let text = r#"<services>
    <service verb="update" noun="Example" type="interface"/>
    <service verb="run" noun="Thing">
        <implements service="example.Things.update#Example"/>
        <actions>
            <entity-find entity-name="Example" list="list"><econdition field-name="statusId" value="X"/></entity-find>
            <service-call name="example.Things.update#Example"/>
            <script>ec.service.sync().name("other.Svc.do#It").call(); ec.entity.find('moqui.basic.Enumeration')</script>
        </actions>
    </service>
</services>"#;
    let index = read(
        ArtifactKind::Service,
        "component://example/service/example/Things.xml",
        text,
    );
    assert_eq!(
        ids(&index),
        [
            "service:example.Things.update#Example",
            "service:example.Things.run#Thing"
        ]
    );
    let from = "service:example.Things.run#Thing".to_string();
    assert_eq!(
        refs_of(&index),
        [
            (from.clone(), "service", "example.Things.update#Example", "implements"),
            (from.clone(), "entity", "Example", "entity-find"),
            (from.clone(), "field", "statusId", "econdition"),
            (from.clone(), "service", "example.Things.update#Example", "service-call"),
            (from.clone(), "service", "other.Svc.do#It", "script"),
            (from, "entity", "moqui.basic.Enumeration", "script"),
        ]
    );
    assert_eq!(index.refs[2].to_scope, "Example");
}

#[test]
fn screens_transitions_forms_anchors_and_links() {
    let text = r#"<screen default-menu-title="Detail">
    <subscreens><subscreens-item name="Sub" location="component://example/screen/Sub.xml"/></subscreens>
    <transition name="updateExample"><service-call name="update#moqui.example.Example"/><default-response url="."/></transition>
    <widgets>
        <container-box name="ExampleBox">
            <form-single name="ExampleFacts" transition="updateExample"/>
            <link url="updateExample" text="Go"/>
        </container-box>
    </widgets>
</screen>"#;
    let location = "component://example/screen/Detail.xml";
    let index = read(ArtifactKind::Screen, location, text);
    assert_eq!(
        ids(&index),
        [
            "screen:component://example/screen/Detail.xml",
            "subscreen:component://example/screen/Detail.xml#Sub",
            "transition:component://example/screen/Detail.xml#updateExample:any",
            "anchor:component://example/screen/Detail.xml#ExampleBox",
            "form:component://example/screen/Detail.xml#ExampleFacts",
        ]
    );
    let kinds: Vec<(&str, &str, &str)> = index
        .refs
        .iter()
        .map(|row| (row.to_kind.as_str(), row.to_name.as_str(), row.ref_type.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            ("screen", "component://example/screen/Sub.xml", "subscreens-item"),
            ("service", "update#moqui.example.Example", "service-call"),
            ("transition", "updateExample:any", "form"),
            ("transition", "updateExample:any", "link"),
        ]
    );
}

#[test]
fn screen_extend_names_its_base_and_anchors() {
    let text = r#"<screen-extend>
    <form-single name="ExampleFacts"><field name="dueDate" after="exampleEndDate"/></form-single>
    <widgets-extend name="Gone" where="after"><label text="x"/></widgets-extend>
    <transition name="save"><default-response url="."/></transition>
</screen-extend>"#;
    let location = "component://my-site/screen-extend/example/screen/Detail.xml";
    let index = read(ArtifactKind::ScreenExtend, location, text);
    assert_eq!(index.artifacts[0].id, format!("screen-extend:{location}"));
    let base = "component://example/screen/Detail.xml";
    let targets: Vec<(&str, &str, &str, &str)> = index
        .refs
        .iter()
        .map(|row| {
            (
                row.to_kind.as_str(),
                row.to_name.as_str(),
                row.to_scope.as_str(),
                row.ref_type.as_str(),
            )
        })
        .collect();
    assert_eq!(
        targets,
        [
            ("screen", base, "", "extend"),
            ("anchor", "ExampleFacts", base, "anchor"),
            ("anchor", "Gone", base, "anchor"),
            ("transition-anchor", "save", base, "anchor"),
        ]
    );
}

#[test]
fn eca_rules_by_id_or_target() {
    let text = r#"<secas>
    <seca service="update#moqui.example.Example" when="post-service"><actions/></seca>
    <seca id="Named" service="example.Things.run#Thing" when="tx-commit"><actions/></seca>
</secas>"#;
    let location = "component://my-site/service/Site.secas.xml";
    let index = read(ArtifactKind::Seca, location, text);
    assert_eq!(
        ids(&index),
        [
            format!("seca:{location}#update#moqui.example.Example:post-service"),
            format!("seca:{location}#Named")
        ]
    );
    assert!(index
        .refs
        .iter()
        .all(|row| row.ref_type == "extend" && row.to_kind == ToKind::Service));
}

#[test]
fn a_service_prefix_and_an_extended_location_are_the_path_after_their_directory() {
    assert_eq!(
        service_prefix("component://mantle/service/mantle/order/OrderServices.xml"),
        "mantle.order.OrderServices"
    );
    assert_eq!(service_prefix("component://mantle/data/Seed.xml"), "");
    assert_eq!(
        extended_location("component://site/screen-extend/SimpleScreens/Order/FindOrder.xml"),
        "component://SimpleScreens/Order/FindOrder.xml"
    );
    assert_eq!(extended_location("component://site/screen/Home.xml"), "");
}

#[test]
fn resolving_names_fields_entity_auto_and_anchors() {
    let entities = read(
        ArtifactKind::Entity,
        "component://example/entity/ExampleEntities.xml",
        ENTITIES,
    );
    let screen = read(
        ArtifactKind::Screen,
        "component://example/screen/Detail.xml",
        r#"<screen><widgets><form-single name="ExampleFacts"/></widgets></screen>"#,
    );
    let mut resolver = Resolver::default();
    for artifact in entities.artifacts.iter().chain(&screen.artifacts) {
        resolver.add(artifact);
    }
    let check = |kind: ToKind, name: &str, scope: &str| {
        let mut row = RefRow {
            from: String::new(),
            to_kind: kind,
            to_name: name.into(),
            to_scope: scope.into(),
            ref_type: String::new(),
            line: 0,
            column: 0,
            to_id: None,
            resolved: false,
            operation: String::new(),
            origin: String::new(),
            origin_hash: String::new(),
        };
        resolver.resolve(&mut row, &|_| false);
        row.to_id
    };
    assert_eq!(
        check(ToKind::Entity, "Example", "").as_deref(),
        Some("entity:moqui.example.Example")
    );
    assert_eq!(
        check(ToKind::Field, "statusId", "Example").as_deref(),
        Some("field:moqui.example.Example.statusId")
    );
    assert_eq!(check(ToKind::Field, "nope", "Example"), None);
    assert_eq!(
        check(ToKind::Service, "update#moqui.example.Example", "").as_deref(),
        Some("entity:moqui.example.Example")
    );
    assert_eq!(
        check(ToKind::Anchor, "ExampleFacts", "component://example/screen/Detail.xml").as_deref(),
        Some("form:component://example/screen/Detail.xml#ExampleFacts")
    );
    // a renamed form: the extension's anchor is broken
    assert_eq!(
        check(ToKind::Anchor, "Renamed", "component://example/screen/Detail.xml"),
        None
    );
    // by the base's path under any component's screen directory
    assert_eq!(
        check(ToKind::Screen, "component://Detail.xml", "").as_deref(),
        Some("screen:component://example/screen/Detail.xml")
    );
}

#[test]
fn search_keys_and_matching() {
    assert_eq!(search_key("mantle.order.OrderHeader"), "mantle order order header");
    assert_eq!(search_key("place#Order"), "place order");
    assert_eq!(search_key(".Lead"), "lead");
    let query = query_segments("OrdHdr");
    assert_eq!(query, ["ord", "hdr"]);
    assert!(match_score(&search_key("OrderHeader"), &query).is_some());
    assert!(match_score(&search_key("OrderItem"), &query).is_none());
    let exact = match_score(&search_key("OrderHeader"), &query_segments("OrderHeader")).unwrap();
    let abbreviated = match_score(&search_key("OrderHeader"), &query).unwrap();
    assert!(exact < abbreviated);
    assert_eq!(
        short_of("service", "mantle.order.OrderServices.place#Order"),
        "place#Order"
    );
}

/// Each file's references resolved against the artifacts of all of them, as a build resolves them.
fn resolve_together(files: &mut [FileIndex]) -> Resolver {
    let mut resolver = Resolver::default();
    for artifact in files.iter().flat_map(|file| &file.artifacts) {
        resolver.add(artifact);
    }
    for row in files.iter_mut().flat_map(|file| &mut file.refs) {
        resolver.resolve(row, &|_| false);
    }
    resolver
}

/// The resolved target of each reference of `file` of `kind`, by the name it is written with.
fn targets(file: &FileIndex, kind: ToKind) -> Vec<(&str, Option<&str>)> {
    file.refs
        .iter()
        .filter(|row| row.to_kind == kind)
        .map(|row| (row.to_name.as_str(), row.to_id.as_deref()))
        .collect()
}

#[test]
fn screen_paths_walk_subscreens_items_and_directories() {
    let app = "component://c/screen/App.xml";
    let order = "component://c/screen/App/Order.xml";
    let detail = "component://c/screen/App/Order/Detail.xml";
    let tasks = "component://c/screen/Tasks.xml";
    let edit = "component://c/screen/Tasks/EditTask.xml";
    let lone = "component://c/screen/Lone/X.xml";
    let mut files = [
        read(
            ArtifactKind::Screen,
            app,
            r#"<screen><subscreens><subscreens-item name="Tasks" location="component://c/screen/Tasks.xml"/></subscreens>
    <widgets><link url="Order" text="Orders"/></widgets></screen>"#,
        ),
        read(
            ArtifactKind::Screen,
            order,
            r#"<screen>
    <transition name="ship"><service-call name="a.OrderServices.ship#Order"/><default-response url="../Tasks/EditTask"/></transition>
    <widgets>
        <link url="../Tasks" text="Tasks"/>
        <link url="Detail/save?x=1" text="Save"/>
        <link url="${somewhere}" text="Expression"/>
        <link url="https://example.com/a" text="Away"/>
        <link url="mailto:a@example.com" text="Mail"/>
        <link url="." text="Here"/>
        <link url="/apps/x" text="Absolute"/>
        <link url="Nope/x" text="Nowhere"/>
        <link url="ship" text="Ship"/>
    </widgets></screen>"#,
        ),
        read(
            ArtifactKind::Screen,
            detail,
            r#"<screen><transition name="save" method="post"><default-response url="."/></transition></screen>"#,
        ),
        read(
            ArtifactKind::Screen,
            tasks,
            r#"<screen><widgets><link url="../Order/ship" text="Ship"/><link url="EditTask" text="Edit"/></widgets></screen>"#,
        ),
        read(ArtifactKind::Screen, edit, r#"<screen/>"#),
        read(ArtifactKind::Screen, lone, r#"<screen/>"#),
    ];
    resolve_together(&mut files);
    let [app_file, order_file, detail_file, tasks_file, edit_file, lone_file] = &files;

    // a plain name with no transition of its own screen is a path: here the default subscreen App/Order.xml
    assert_eq!(
        targets(app_file, ToKind::ScreenPath),
        [("Order", Some("screen:component://c/screen/App/Order.xml"))]
    );
    // a plain name of a transition of the screen stays a transition reference, one per method
    assert_eq!(
        targets(order_file, ToKind::Transition),
        [(
            "ship:any",
            Some("transition:component://c/screen/App/Order.xml#ship:any")
        )]
    );
    // `..` goes up to the directory's screen; an item of it, the default subscreen of the item's directory, a
    // transition of a default subscreen; expressions, schemes and `.` make no reference; the rest stay unresolved
    assert_eq!(
        targets(order_file, ToKind::ScreenPath),
        [
            (
                "../Tasks/EditTask",
                Some("screen:component://c/screen/Tasks/EditTask.xml")
            ),
            ("../Tasks", Some("screen:component://c/screen/Tasks.xml")),
            (
                "Detail/save?x=1",
                Some("transition:component://c/screen/App/Order/Detail.xml#save:post")
            ),
            ("/apps/x", None),
            ("Nope/x", None),
        ]
    );
    let response = order_file
        .refs
        .iter()
        .find(|row| row.to_name == "../Tasks/EditTask")
        .unwrap();
    assert_eq!(
        (response.from.as_str(), response.ref_type.as_str()),
        ("transition:component://c/screen/App/Order.xml#ship:any", "link")
    );
    // a screen named by an item has no directory parent: `..` goes up to the item's screen
    assert_eq!(
        targets(tasks_file, ToKind::ScreenPath),
        [
            (
                "../Order/ship",
                Some("transition:component://c/screen/App/Order.xml#ship:any")
            ),
            ("EditTask", Some("screen:component://c/screen/Tasks/EditTask.xml")),
        ]
    );

    // a default subscreen: a subscreens-item reference from its directory's screen, read from its own file
    let edge = |file: &FileIndex| {
        file.refs
            .iter()
            .filter(|row| row.origin == ORIGIN_DIRECTORY)
            .map(|row| (row.from.clone(), row.to_kind, row.ref_type.clone(), row.to_id.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        edge(edit_file),
        [(
            "screen:component://c/screen/Tasks.xml".to_string(),
            ToKind::Screen,
            "subscreens-item".to_string(),
            Some("screen:component://c/screen/Tasks/EditTask.xml".to_string())
        )]
    );
    assert_eq!(
        edge(detail_file)[0].3.as_deref(),
        Some("screen:component://c/screen/App/Order/Detail.xml")
    );
    // none for a screen directly in its component's screen directory; unresolved while its directory has no screen
    assert!(edge(app_file).is_empty() && edge(tasks_file).is_empty());
    assert_eq!(edge(lone_file)[0].3, None);
    assert!(!lone_file.refs[0].resolved);
}

#[test]
fn a_forms_fields_flow_into_its_transitions_service() {
    let location = "component://c/screen/Order.xml";
    let file = read(
        ArtifactKind::Screen,
        location,
        r#"<screen>
    <transition name="ship"><parameter name="orderId"/><service-call name="a.OrderServices.ship#Order"/>
        <default-response url="."/></transition>
    <transition name="note"><service-call name="a.OrderServices.note#Order" in-map="[orderId:orderId]"/>
        <default-response url="."/></transition>
    <widgets>
        <form-single name="ShipForm" transition="ship">
            <field name="orderId"><default-field><hidden/></default-field></field>
            <field name="carrierPartyId"><default-field><drop-down/></default-field></field>
            <field name="submitButton"><default-field><submit/></default-field></field>
        </form-single>
        <form-single name="NoteForm" transition="note"><field name="note"><default-field><text-area/></default-field></field></form-single>
        <form-list name="Lines" list="lines" transition="ship"><field name="orderItemSeqId"><default-field><display/></default-field></field></form-list>
    </widgets>
</screen>"#,
    );
    let flows: Vec<(&str, &str, &str, &str, Option<&str>)> = file
        .bindings
        .iter()
        .filter(|binding| binding.source_kind == "form-field")
        .map(|binding| {
            (
                binding.from.as_str(),
                binding.to_id.as_str(),
                binding.direction.as_str(),
                binding.source_text.as_str(),
                binding.source_id.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        flows,
        [
            (
                "form:component://c/screen/Order.xml#ShipForm",
                "parameter:service:a.OrderServices.ship#Order@in.orderId",
                "in",
                "orderId",
                Some("form-field:component://c/screen/Order.xml#ShipForm@orderId"),
            ),
            (
                "form:component://c/screen/Order.xml#ShipForm",
                "parameter:service:a.OrderServices.ship#Order@in.carrierPartyId",
                "in",
                "carrierPartyId",
                Some("form-field:component://c/screen/Order.xml#ShipForm@carrierPartyId"),
            ),
            (
                "form:component://c/screen/Order.xml#ShipForm",
                "parameter:service:a.OrderServices.ship#Order@in.submitButton",
                "in",
                "submitButton",
                Some("form-field:component://c/screen/Order.xml#ShipForm@submitButton"),
            ),
            // a service-call with an in-map of its own takes no request parameters: NoteForm makes none
            (
                "form:component://c/screen/Order.xml#Lines",
                "parameter:service:a.OrderServices.ship#Order@in.orderItemSeqId",
                "in",
                "orderItemSeqId",
                Some("form-field:component://c/screen/Order.xml#Lines@orderItemSeqId"),
            ),
        ]
    );
    // the form still submits to its transition, and the field ids the bindings name are artifacts
    assert!(file
        .refs
        .iter()
        .any(|row| row.from == "form:component://c/screen/Order.xml#ShipForm"
            && row.to_kind == ToKind::Transition
            && row.to_name == "ship:any"));
    assert!(ids(&file).contains(&"form-field:component://c/screen/Order.xml#ShipForm@carrierPartyId"));
}
