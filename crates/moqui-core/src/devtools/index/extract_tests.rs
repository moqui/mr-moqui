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
use crate::devtools::index::resolve::Resolver;

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
            "entity:moqui.example.ExampleAndStatus",
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
                "entity:moqui.example.Example".to_string(),
                "entity",
                "moqui.basic.StatusItem",
                "relationship"
            ),
            (
                "entity:moqui.example.ExampleAndStatus".to_string(),
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
            "transition:component://example/screen/Detail.xml#updateExample",
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
            ("transition", "updateExample", "form"),
            ("transition", "updateExample", "link"),
        ]
    );
}

#[test]
fn screen_extend_names_its_base_and_anchors() {
    let text = r#"<screen-extend>
    <form-single name="ExampleFacts"><field name="dueDate" after="exampleEndDate"/></form-single>
    <widgets-extend name="Gone" where="after"><label text="x"/></widgets-extend>
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
        ]
    );
}

#[test]
fn eca_rules_by_id_or_position() {
    let text = r#"<secas>
    <seca service="update#moqui.example.Example" when="post-service"><actions/></seca>
    <seca id="Named" service="example.Things.run#Thing" when="tx-commit"><actions/></seca>
</secas>"#;
    let location = "component://my-site/service/Site.secas.xml";
    let index = read(ArtifactKind::Seca, location, text);
    assert_eq!(
        ids(&index),
        [format!("seca:{location}#1"), format!("seca:{location}#Named")]
    );
    assert!(index
        .refs
        .iter()
        .all(|row| row.ref_type == "extend" && row.to_kind == ToKind::Service));
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
