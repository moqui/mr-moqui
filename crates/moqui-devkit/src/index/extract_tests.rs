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
    for file in files.iter_mut() {
        file.bindings = resolver.resolve_bindings(std::mem::take(&mut file.bindings));
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

    // a plain name with no transition of its own screen is a path: here the directory subscreen App/Order.xml
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
    // `..` goes up to the directory's screen; an item of it, the directory subscreen of the item's directory, a
    // transition of a directory subscreen; expressions, schemes and `.` make no reference; the rest stay unresolved
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

    // a directory subscreen: a subscreens-item reference from its directory's screen, read from its own file
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
    let mut files = [read(
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
    )];
    // each field binds by the path the form posts to, its transition's name, which the resolver walks
    assert!(files[0].bindings.iter().all(|binding| binding
        .to_id
        .starts_with("parameter:screen-path:component://c/screen/Order.xml#")));
    resolve_together(&mut files);
    let file = &files[0];
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
            // a submit button is no field the user fills, and a service-call with an in-map of its own takes no request
            // parameters: NoteForm makes none
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
    assert!(ids(file).contains(&"form-field:component://c/screen/Order.xml#ShipForm@carrierPartyId"));
}

/// A screen item a conf mounts (`@conf`) or a `SubscreensItem` row mounts (`@row`), as [`super::super::rows`] makes it.
fn mount(screen: &str, name: &str, location: &str, mark: &str) -> ArtifactRow {
    ArtifactRow {
        id: format!("subscreen:{screen}#{name}{mark}"),
        kind: "subscreen".to_string(),
        name: name.to_string(),
        short_name: name.to_string(),
        component: String::new(),
        path: CONF_PATH.to_string(),
        line: 0,
        column: 0,
        summary: location.to_string(),
        parent: Some(format!("screen:{screen}")),
        origin: String::new(),
        marks: String::new(),
    }
}

#[test]
fn screen_paths_walk_as_moqui_walks_a_url() {
    let webroot = "component://webroot/screen/webroot.xml";
    let app = "component://c/screen/App.xml";
    let mut files = [
        read(
            ArtifactKind::Screen,
            webroot,
            r#"<screen><subscreens><subscreens-item name="apps" location="component://c/screen/App.xml"/></subscreens></screen>"#,
        ),
        read(
            ArtifactKind::Screen,
            app,
            r##"<screen>
    <subscreens default-item="Order">
        <conditional-default condition="ec.user.userId" item="Mounted"/>
        <subscreens-item name="Wrap" location="component://c/screen/Wrap.xml" no-sub-path="true"/>
        <subscreens-item name="ship" location="component://c/screen/Ship.xml"/>
    </subscreens>
    <transition name="ship"><default-response url="."/></transition>
    <widgets>
        <link url="Order/Detail/~orderId=1/save" text="Inline parameter"/>
        <link url="Order/Detail/save/extra/path" text="Extra path"/>
        <link url="Inner" text="Through a no-sub-path item"/>
        <link url="Order/Detail.pdf" text="Render mode"/>
        <link url="Order/Detail/save.json" text="Transition in a render mode"/>
        <link url="ship" text="A subscreen before a transition"/>
        <link url="#top" text="Fragment"/>
        <link url="?x=1" text="Query"/>
        <link url="//Detail" text="Sparse"/>
        <link url="//Order/Nowhere" text="Sparse, not found"/>
        <link url="/apps/Order/Detail" text="Absolute"/>
        <link url="Mounted" text="A conf mount"/>
        <link url="Extra" text="A screen-extend's item"/>
        <link url="Docs/guide/intro" text="Past a screen with allow-extra-path"/>
        <link url="Order/nothing/here" text="Past a screen without it"/>
    </widgets>
</screen>"##,
        ),
        read(
            ArtifactKind::Screen,
            "component://c/screen/App/Order.xml",
            r#"<screen/>"#,
        ),
        read(
            ArtifactKind::Screen,
            "component://c/screen/App/Order/Detail.xml",
            r#"<screen><transition name="save" method="post"><default-response url="."/></transition></screen>"#,
        ),
        read(ArtifactKind::Screen, "component://c/screen/Wrap.xml", r#"<screen/>"#),
        read(
            ArtifactKind::Screen,
            "component://c/screen/Wrap/Inner.xml",
            r#"<screen/>"#,
        ),
        read(ArtifactKind::Screen, "component://c/screen/Ship.xml", r#"<screen/>"#),
        read(
            ArtifactKind::Screen,
            "component://c/screen/Elsewhere.xml",
            r#"<screen/>"#,
        ),
        read(ArtifactKind::Screen, "component://c/screen/Extra.xml", r#"<screen/>"#),
        read(
            ArtifactKind::Screen,
            "component://c/screen/App/Docs.xml",
            r#"<screen allow-extra-path="true"/>"#,
        ),
        read(
            ArtifactKind::ScreenExtend,
            "component://site/screen-extend/c/screen/App.xml",
            r#"<screen-extend><subscreens-item name="Extra" location="component://c/screen/Extra.xml"/></screen-extend>"#,
        ),
        FileIndex {
            artifacts: vec![mount(app, "Mounted", "component://c/screen/Elsewhere.xml", CONF_ITEM)],
            ..FileIndex::default()
        },
    ];
    resolve_together(&mut files);
    let detail = "screen:component://c/screen/App/Order/Detail.xml";
    let save = "transition:component://c/screen/App/Order/Detail.xml#save:post";
    let paths: Vec<(&str, Option<&str>)> = targets(&files[1], ToKind::ScreenPath)
        .into_iter()
        .filter(|(_, target)| *target != Some("screen:component://c/screen/App/Order.xml"))
        .collect();
    assert_eq!(
        paths,
        [
            // the default item and the conditional one: edges from the screen
            ("Mounted", Some("screen:component://c/screen/Elsewhere.xml")),
            ("Order/Detail/~orderId=1/save", Some(save)),
            // a transition mid-path is the end: the rest is extra path
            ("Order/Detail/save/extra/path", Some(save)),
            ("Inner", Some("screen:component://c/screen/Wrap/Inner.xml")),
            ("Order/Detail.pdf", Some(detail)),
            ("Order/Detail/save.json", Some(save)),
            // a subscreens item comes before a transition of its name, as Moqui walks it
            ("ship", Some("screen:component://c/screen/Ship.xml")),
            // `#top` and `?x=1` make none; a sparse path is searched for, and one not found stays, unresolved
            ("//Detail", Some(detail)),
            ("//Order/Nowhere", None),
            ("/apps/Order/Detail", Some(detail)),
            ("Mounted", Some("screen:component://c/screen/Elsewhere.xml")),
            ("Extra", Some("screen:component://c/screen/Extra.xml")),
            // nothing of the name under a screen with allow-extra-path: the path ends at it; without it, none
            ("Docs/guide/intro", Some("screen:component://c/screen/App/Docs.xml")),
            ("Order/nothing/here", None),
        ]
    );
    let edges: Vec<(&str, &str, Option<&str>)> = files[1]
        .refs
        .iter()
        .filter(|row| matches!(row.ref_type.as_str(), "default-item" | "conditional-default"))
        .map(|row| (row.ref_type.as_str(), row.from.as_str(), row.to_id.as_deref()))
        .collect();
    assert_eq!(
        edges,
        [
            (
                "default-item",
                "screen:component://c/screen/App.xml",
                Some("screen:component://c/screen/App/Order.xml")
            ),
            (
                "conditional-default",
                "screen:component://c/screen/App.xml",
                Some("screen:component://c/screen/Elsewhere.xml")
            ),
        ]
    );
}

#[test]
fn mounts_override_items_and_directories_and_up_needs_one_parent() {
    let app = "component://c/screen/App.xml";
    let shared = "component://c/screen/Shared.xml";
    let mut resolver = Resolver::default();
    for (location, text) in [
        (
            app,
            r#"<screen><subscreens><subscreens-item name="Order" location="component://c/screen/Other.xml"/>
                <subscreens-item name="Shared" location="component://c/screen/Shared.xml"/></subscreens></screen>"#,
        ),
        (
            "component://c/screen/Two.xml",
            r#"<screen><subscreens><subscreens-item name="Shared" location="component://c/screen/Shared.xml"/></subscreens></screen>"#,
        ),
        ("component://c/screen/App/Order.xml", "<screen/>"),
        ("component://c/screen/App/Common.xml", "<screen/>"),
        ("component://c/screen/Two/Sib.xml", "<screen/>"),
        ("component://c/screen/App/Sib.xml", "<screen/>"),
        ("component://c/screen/Other.xml", "<screen/>"),
        ("component://c/screen/Row.xml", "<screen/>"),
        (shared, "<screen/>"),
    ] {
        for artifact in read(ArtifactKind::Screen, location, text).artifacts {
            resolver.add(&artifact);
        }
    }
    // an explicit item wins over the directory subscreen of its name, a conf mount over the item, a row over both
    assert_eq!(
        resolver.subscreen(app, "Order").as_deref(),
        Some("component://c/screen/Other.xml")
    );
    let conf = mount(app, "Order", "component://c/screen/App/Order.xml", CONF_ITEM);
    resolver.add(&conf);
    assert_eq!(
        resolver.subscreen(app, "Order").as_deref(),
        Some("component://c/screen/App/Order.xml")
    );
    let row = mount(app, "Order", "component://c/screen/Row.xml", ROW_ITEM);
    resolver.add(&row);
    assert_eq!(
        resolver.subscreen(app, "Order").as_deref(),
        Some("component://c/screen/Row.xml")
    );
    resolver.remove(&row);
    resolver.remove(&conf);
    assert_eq!(
        resolver.subscreen(app, "Order").as_deref(),
        Some("component://c/screen/Other.xml")
    );

    // Shared is under App and Two: `..` resolves only where the walk succeeds from one of them, or both reach the same
    assert_eq!(
        resolver.screen_path(shared, "../Common").as_deref(),
        Some("screen:component://c/screen/App/Common.xml")
    );
    assert_eq!(resolver.screen_path(shared, "../Sib"), None);
    assert_eq!(
        resolver.screen_path(shared, "../Shared").as_deref(),
        Some("screen:component://c/screen/Shared.xml")
    );

    // a directory subscreen's parent must be the screen of its directory itself, not one of another component
    let mut lone = RefRow {
        from: "screen:component://c/screen/App.xml".to_string(),
        to_kind: ToKind::Screen,
        to_name: "component://d/screen/App/Order.xml".to_string(),
        to_scope: "component://d/screen/App.xml".to_string(),
        ref_type: "subscreens-item".to_string(),
        line: 0,
        column: 0,
        to_id: None,
        resolved: false,
        operation: String::new(),
        origin: ORIGIN_DIRECTORY.to_string(),
        origin_hash: String::new(),
    };
    resolver.resolve(&mut lone, &|_| false);
    assert!(!lone.resolved, "{lone:?}");
}

#[test]
fn form_fields_flow_through_paths_actions_and_field_maps() {
    let order = "component://c/screen/App/Order.xml";
    let mut files = [
        read(
            ArtifactKind::Screen,
            order,
            r#"<screen>
    <transition name="update">
        <service-call name="a.OrderServices.update#Order" in-map="[orderId:orderId]"><field-map field-name="partyId" from="customerPartyId"/></service-call>
        <actions><if condition="true"><service-call name="a.OrderServices.note#Order" in-map="context"/></if>
            <service-call name="a.OrderServices.skip#Order"/></actions>
        <default-response url="."/></transition>
    <widgets>
        <form-single name="Edit" transition="update">
            <field name="customerPartyId"><default-field><drop-down/></default-field></field>
            <field name="note"><default-field><text-area/></default-field></field>
            <field name="orderId"><header-field><text-find/></header-field></field>
        </form-single>
        <form-list name="Find" list="orders" transition="../Tasks/save">
            <field name="taskName"><header-field><text-find/></header-field><default-field><text-line/></default-field></field>
            <field name="go"><default-field><submit/></default-field></field>
        </form-list>
    </widgets>
</screen>"#,
        ),
        read(
            ArtifactKind::Screen,
            "component://c/screen/App/Tasks.xml",
            r#"<screen><transition name="save"><service-call name="a.TaskServices.save#Task"/><default-response url="."/></transition></screen>"#,
        ),
        read(ArtifactKind::Screen, "component://c/screen/App.xml", "<screen/>"),
    ];
    resolve_together(&mut files);
    let flows: Vec<(&str, &str)> = files[0]
        .bindings
        .iter()
        .filter(|binding| binding.source_kind == "form-field")
        .map(|binding| (binding.source_text.as_str(), binding.to_id.as_str()))
        .collect();
    assert_eq!(
        flows,
        [
            // renamed by the call's field-map, though the call has an in-map of its own; the context in-map of a call in
            // the actions takes every field; a field with only a header-field is none
            (
                "customerPartyId",
                "parameter:service:a.OrderServices.update#Order@in.partyId"
            ),
            (
                "customerPartyId",
                "parameter:service:a.OrderServices.note#Order@in.customerPartyId"
            ),
            ("note", "parameter:service:a.OrderServices.note#Order@in.note"),
            // a transition by its path, named by the resolver; a submit button is none
            ("taskName", "parameter:service:a.TaskServices.save#Task@in.taskName"),
        ]
    );
    let update = files[0]
        .artifacts
        .iter()
        .find(|artifact| artifact.kind == "transition")
        .unwrap();
    assert_eq!(
        update.summary,
        "calls a.OrderServices.update#Order, own in-map, request to a.OrderServices.note#Order"
    );
    assert_eq!(
        crate::index::resolve::request_services(&update.summary),
        ["a.OrderServices.note#Order"]
    );
    // the path's binding keeps the id it was written with and its slot, and names it again while the path reaches no
    // transition
    let mut path_binding = files[0]
        .bindings
        .iter()
        .find(|binding| binding.source_text == "taskName")
        .unwrap()
        .clone();
    let written = "parameter:screen-path:component://c/screen/App/Order.xml#../Tasks/save@in.taskName";
    assert_eq!(path_binding.to_path, Some(format!("{written}#0")));
    let mut resolver = resolve_together(&mut files);
    for artifact in &files[1].artifacts {
        resolver.remove(artifact);
    }
    resolver.resolve_binding(&mut path_binding);
    assert_eq!(path_binding.to_id, written);
}

#[test]
fn expressions_name_no_service_and_rows_of_a_group_lose_to_rows_of_all() {
    let file = read(
        ArtifactKind::Screen,
        "component://c/screen/A.xml",
        r#"<screen><transition name="run"><service-call name="${svcName}"/><default-response url="."/></transition></screen>"#,
    );
    assert_eq!(file.artifacts[1].summary, "");
    assert!(crate::index::resolve::request_services("calls ${svcName}, request to ${other}").is_empty());

    let app = "component://c/screen/App.xml";
    let mut resolver = Resolver::default();
    for location in [app, "component://c/screen/All.xml", "component://c/screen/Group.xml"] {
        resolver.add_parts(&format!("screen:{location}"), "screen", location, "");
    }
    let group = mount(app, "X", "component://c/screen/Group.xml", "@row:ADMIN");
    resolver.add(&group);
    assert_eq!(
        resolver.subscreen(app, "X").as_deref(),
        Some("component://c/screen/Group.xml")
    );
    resolver.add(&mount(app, "X", "component://c/screen/All.xml", ROW_ITEM));
    assert_eq!(
        resolver.subscreen(app, "X").as_deref(),
        Some("component://c/screen/All.xml")
    );
}

#[test]
fn a_sparse_search_ends_in_a_cycle_of_items() {
    // A mounts B and B mounts A: a name under neither is searched for once under each; `//` starts at the webroot
    let mut resolver = Resolver::default();
    for (location, other) in [
        ("component://webroot/screen/webroot.xml", "component://c/screen/A.xml"),
        ("component://c/screen/A.xml", "component://c/screen/B.xml"),
        ("component://c/screen/B.xml", "component://c/screen/A.xml"),
    ] {
        let text =
            format!(r#"<screen><subscreens><subscreens-item name="Next" location="{other}"/></subscreens></screen>"#);
        for artifact in read(ArtifactKind::Screen, location, &text).artifacts {
            resolver.add(&artifact);
        }
    }
    let started = std::time::Instant::now();
    assert_eq!(
        resolver.screen_path("component://c/screen/A.xml", "//Nope/Nowhere/At/All"),
        None
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    assert_eq!(
        resolver
            .screen_path("component://c/screen/A.xml", "//Next/Next")
            .as_deref(),
        Some("screen:component://c/screen/B.xml")
    );
}
