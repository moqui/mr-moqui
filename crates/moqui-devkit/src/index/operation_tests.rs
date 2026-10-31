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

//! Operations, origins and bindings of the references, and the derived entity-auto services.

use super::*;

fn read(kind: ArtifactKind, path: &str, location: &str, text: &str) -> FileIndex {
    let info = FileInfo {
        path,
        location,
        component: "example",
        kind,
    };
    extract::extract(&info, text).expect("parses")
}

/// Each reference as (to name, ref type, operation).
fn operations(index: &FileIndex) -> Vec<(&str, &str, &str)> {
    index
        .refs
        .iter()
        .map(|row| (row.to_name.as_str(), row.ref_type.as_str(), row.operation.as_str()))
        .collect()
}

const ENTITIES: &str = r#"<entities>
    <entity entity-name="Example" package="moqui.example">
        <field name="exampleId" type="id" is-pk="true"/>
        <field name="exampleName" type="text-medium"/>
        <field name="statusId" type="id"/>
        <field name="lastUpdatedStamp" type="date-time"/>
    </entity>
    <entity entity-name="ExampleItem" package="moqui.example">
        <field name="exampleId" type="id" is-pk="true"/>
        <field name="exampleItemSeqId" type="id" is-pk="true"/>
        <field name="amount" type="number-decimal"/>
    </entity>
    <entity entity-name="ExampleRate" package="moqui.example">
        <field name="exampleId" type="id" is-pk="true"/>
        <field name="fromDate" type="date-time" is-pk="true"/>
        <field name="rate" type="number-decimal"/>
    </entity>
</entities>"#;

fn entities() -> FileIndex {
    read(
        ArtifactKind::Entity,
        "/c/example/entity/ExampleEntities.xml",
        "component://example/entity/ExampleEntities.xml",
        ENTITIES,
    )
}

#[test]
fn xml_actions_name_their_operations() {
    let text = r#"<services>
    <service verb="run" noun="Thing">
        <actions>
            <entity-find entity-name="Example" list="list"><econdition field-name="statusId" value="X"/></entity-find>
            <entity-find-one entity-name="moqui.example.Example" value-field="found"/>
            <entity-update value-field="found"/>
            <entity-make-value entity-name="ExampleItem" value-field="item"/>
            <entity-create value-field="item"/>
            <entity-make-value entity-name="ExampleRate" value-field="rate"/>
            <entity-create value-field="rate" or-update="true"/>
            <entity-make-value entity-name="Example" value-field="gone"/>
            <entity-delete value-field="gone"/>
            <entity-delete-by-condition entity-name="ExampleItem"><econdition field-name="amount" value="0"/></entity-delete-by-condition>
            <entity-update value-field="unknownValue"/>
            <service-call name="example.Things.do#It"/>
            <service-call name="create#moqui.example.Example"/>
            <service-call name="update#Example"/>
            <service-call name="store#ExampleItem"/>
            <service-call name="delete#ExampleItem"/>
        </actions>
    </service>
</services>"#;
    let index = read(
        ArtifactKind::Service,
        "/c/example/service/example/Things.xml",
        "component://example/service/example/Things.xml",
        text,
    );
    assert_eq!(
        operations(&index),
        [
            ("Example", "entity-find", "find"),
            ("statusId", "econdition", "read"),
            ("moqui.example.Example", "entity-find-one", "find"),
            ("moqui.example.Example", "entity-update", "update"),
            ("ExampleItem", "entity-make-value", "create"),
            ("ExampleRate", "entity-make-value", "store"),
            ("Example", "entity-make-value", "delete"),
            ("ExampleItem", "entity-delete-by-condition", "delete"),
            ("example.Things.do#It", "service-call", "call"),
            ("create#moqui.example.Example", "service-call", "create"),
            ("update#Example", "service-call", "update"),
            ("store#ExampleItem", "service-call", "store"),
            ("delete#ExampleItem", "service-call", "delete"),
        ]
    );
    let hash = super::hash_of(text);
    assert!(index
        .refs
        .iter()
        .all(|row| row.origin == "xml" && row.origin_hash == hash));
}

#[test]
fn a_value_field_is_its_own_actions_blocks() {
    let text = r#"<services>
    <service verb="first" noun="Thing">
        <actions><entity-make-value entity-name="Example" value-field="value"/></actions>
    </service>
    <service verb="second" noun="Thing">
        <actions><entity-create value-field="value"/></actions>
    </service>
</services>"#;
    let index = read(
        ArtifactKind::Service,
        "/c/example/service/example/Things.xml",
        "component://example/service/example/Things.xml",
        text,
    );
    // the second service's value is not the first's: no operation, and no reference of its own
    assert_eq!(operations(&index), [("Example", "entity-make-value", "")]);
}

#[test]
fn rest_and_data_operations() {
    let rest = r#"<resource name="examples">
    <method type="get"><entity name="moqui.example.Example" operation="list"/></method>
    <method type="post"><entity name="moqui.example.Example" operation="create"/></method>
    <method type="delete"><service name="delete#moqui.example.Example"/></method>
</resource>"#;
    let index = read(
        ArtifactKind::Rest,
        "/c/example/service/examples.rest.xml",
        "component://example/service/examples.rest.xml",
        rest,
    );
    let ops: Vec<&str> = index.refs.iter().map(|row| row.operation.as_str()).collect();
    assert_eq!(ops, ["find", "create", "delete"]);

    let data = r#"<entity-facade-xml type="demo">
    <moqui.example.Example exampleId="ACME" exampleName="Acme"/>
</entity-facade-xml>"#;
    let index = read(
        ArtifactKind::Data,
        "/c/example/data/ExampleData.xml",
        "component://example/data/ExampleData.xml",
        data,
    );
    assert!(!index.refs.is_empty());
    assert!(index
        .refs
        .iter()
        .filter(|row| row.to_kind == ToKind::Entity)
        .all(|row| row.operation == "store"));
}

#[test]
fn script_operations_and_origins() {
    let groovy = r#"
ec.entity.makeValue("moqui.example.Example").setAll(context).create()
def item = ec.entity.makeValue('moqui.example.ExampleItem')
item.amount = 1
item.update()
ec.entity.find("moqui.example.ExampleRate").condition("exampleId", exampleId).deleteAll()
ec.entity.find("moqui.example.Example").one()
ec.service.sync().name("store#moqui.example.Example").parameters(context).call()
ec.service.sync().name("example.Things.do#It").call()
"#;
    let index = read(
        ArtifactKind::Script,
        "/c/example/script/Thing.groovy",
        "component://example/script/Thing.groovy",
        groovy,
    );
    assert_eq!(
        operations(&index),
        [
            ("moqui.example.Example", "script", "create"),
            ("moqui.example.ExampleItem", "script", "update"),
            ("moqui.example.ExampleRate", "script", "delete"),
            ("moqui.example.Example", "script", "find"),
            ("store#moqui.example.Example", "script", "store"),
            ("example.Things.do#It", "script", "call"),
        ]
    );
    assert!(index.refs.iter().all(|row| row.origin == "groovy"));

    let js = r#"ec.entity.find("moqui.example.Example").list();"#;
    let index = read(
        ArtifactKind::Script,
        "/c/example/script/thing.js",
        "component://example/script/thing.js",
        js,
    );
    assert_eq!(operations(&index), [("moqui.example.Example", "script", "find")]);
    assert_eq!(index.refs[0].origin, "js");
    assert_eq!(index.refs[0].origin_hash, super::hash_of(js));
}

#[test]
fn the_service_operation_of_a_name() {
    for (name, operation) in [
        ("create#moqui.example.Example", "create"),
        ("update#Example", "update"),
        ("delete#Example", "delete"),
        ("store#Example", "store"),
        ("createOrUpdate#Example", "store"),
        ("example.Things.create#Example", "call"),
        ("example.Things.run#Thing", "call"),
    ] {
        assert_eq!(extract::service_operation(name), operation, "{name}");
    }
}

const CALLS: &str = r#"<services>
    <service verb="run" noun="Thing">
        <in-parameters><parameter name="exampleId"/><parameter name="name"/></in-parameters>
        <actions>
            <service-call name="update#moqui.example.Example">
                <field-map field-name="exampleId"/>
                <field-map field-name="exampleName" from="name"/>
                <field-map field-name="statusId" value="ExDone"/>
                <field-map field-name="description" value="${name} done"/>
                <field-map field-name="comments" from="someValue"/>
                <field-map field-name="amount" from="total * 2"/>
            </service-call>
        </actions>
    </service>
</services>"#;

fn calls() -> FileIndex {
    read(
        ArtifactKind::Service,
        "/c/example/service/example/Things.xml",
        "component://example/service/example/Things.xml",
        CALLS,
    )
}

#[test]
fn field_maps_bind_into_the_callees_parameters() {
    let index = calls();
    let from = "service:example.Things.run#Thing";
    let to = |name: &str| format!("parameter:service:update#moqui.example.Example@in.{name}");
    let bindings: Vec<(String, &str, &str, Option<&str>)> = index
        .bindings
        .iter()
        .map(|binding| {
            assert_eq!(binding.from, from);
            assert_eq!(binding.direction, "in");
            assert_eq!(binding.origin, "xml");
            assert_eq!(binding.origin_hash, super::hash_of(CALLS));
            (
                binding.to_id.clone(),
                binding.source_kind.as_str(),
                binding.source_text.as_str(),
                binding.source_id.as_deref(),
            )
        })
        .collect();
    let caller = |name: &str| format!("parameter:{from}@in.{name}");
    assert_eq!(
        bindings,
        [
            (
                to("exampleId"),
                "parameter",
                "exampleId",
                Some(caller("exampleId").as_str())
            ),
            (to("exampleName"), "parameter", "name", Some(caller("name").as_str())),
            (to("statusId"), "literal", "ExDone", None),
            (to("description"), "expression", "${name} done", None),
            (to("comments"), "context", "someValue", None),
            (to("amount"), "expression", "total * 2", None),
        ]
    );
    assert!(index.bindings.iter().all(|binding| binding.line > 0));
}

fn memory(files: Vec<(&str, FileIndex)>) -> MemoryIndex {
    let mut index = MemoryIndex::default();
    for (path, read) in files {
        index.take_rows(path.to_string(), read);
    }
    index.resolve_all(&|_| false);
    index
}

/// A derived service's parameters as (id tail after the service, summary).
fn parameters(index: &MemoryIndex, service: &str) -> Vec<(String, String)> {
    let prefix = format!("parameter:{service}@");
    index
        .artifacts
        .iter()
        .filter(|artifact| artifact.parent.as_deref() == Some(service))
        .map(|artifact| {
            assert_eq!(artifact.origin, "derived");
            assert_eq!(artifact.path, extract::DERIVED_PATH);
            (
                artifact.id.strip_prefix(&prefix).unwrap_or(&artifact.id).to_string(),
                artifact.summary.clone(),
            )
        })
        .collect()
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter().map(|(a, b)| ((*a).to_string(), (*b).to_string())).collect()
}

#[test]
fn entity_auto_services_are_derived_by_the_runners_rules() {
    let text = r#"<services><service verb="run" noun="Thing"><actions>
        <service-call name="create#moqui.example.Example"/>
        <service-call name="update#Example"/>
        <service-call name="store#Example"/>
        <service-call name="delete#Example"/>
        <service-call name="create#ExampleItem"/>
        <service-call name="store#ExampleItem"/>
        <service-call name="create#ExampleRate"/>
        <service-call name="update#NoSuchEntity"/>
    </actions></service></services>"#;
    let index = memory(vec![
        ("/c/example/entity/ExampleEntities.xml", entities()),
        (
            "/c/example/service/example/Things.xml",
            read(
                ArtifactKind::Service,
                "/c/example/service/example/Things.xml",
                "component://example/service/example/Things.xml",
                text,
            ),
        ),
    ]);
    let service = index
        .artifacts
        .iter()
        .find(|artifact| artifact.id == "service:create#moqui.example.Example")
        .expect("derived");
    assert_eq!(
        (
            service.kind.as_str(),
            service.origin.as_str(),
            service.parent.as_deref()
        ),
        ("service", "derived", Some("entity:moqui.example.Example"))
    );
    assert_eq!(
        parameters(&index, "service:create#moqui.example.Example"),
        pairs(&[
            ("in.exampleId", "in id, required=false"),
            ("in.exampleName", "in text-medium, required=false"),
            ("in.statusId", "in id, required=false"),
            ("out.exampleId", "out id, required=false"),
        ])
    );
    assert_eq!(
        parameters(&index, "service:update#moqui.example.Example"),
        pairs(&[
            ("in.exampleId", "in id, required=true"),
            ("in.exampleName", "in text-medium, required=false"),
            ("in.statusId", "in id, required=false"),
            ("out.exampleId", "out id, required=false"),
            ("out.oldStatusId", "out id, required=false"),
            ("out.statusChanged", "out Boolean, required=false"),
        ])
    );
    assert_eq!(
        parameters(&index, "service:store#moqui.example.Example").len(),
        6,
        "store as create, with the status outs"
    );
    assert_eq!(
        parameters(&index, "service:delete#moqui.example.Example"),
        pairs(&[("in.exampleId", "in id, required=true")])
    );
    // composite key: the last is sequenced when left out
    assert_eq!(
        parameters(&index, "service:create#moqui.example.ExampleItem"),
        pairs(&[
            ("in.exampleId", "in id, required=true"),
            ("in.exampleItemSeqId", "in id, required=false"),
            ("in.amount", "in number-decimal, required=false"),
            ("out.exampleId", "out id, required=false"),
            ("out.exampleItemSeqId", "out id, required=false"),
        ])
    );
    assert_eq!(
        parameters(&index, "service:store#moqui.example.ExampleItem")[..2],
        pairs(&[
            ("in.exampleId", "in id, required=true"),
            ("in.exampleItemSeqId", "in id, required=false"),
        ])
    );
    // a fromDate key defaults to now
    assert_eq!(
        parameters(&index, "service:create#moqui.example.ExampleRate")[..2],
        pairs(&[
            ("in.exampleId", "in id, required=true"),
            ("in.fromDate", "in date-time, required=false"),
        ])
    );
    assert!(!index
        .artifacts
        .iter()
        .any(|artifact| artifact.name.contains("NoSuchEntity")));
    // the calls resolve to the derived services, short entity names too
    let resolved: Vec<Option<&str>> = index
        .refs
        .iter()
        .filter(|(_, row)| row.ref_type == "service-call")
        .map(|(_, row)| row.to_id.as_deref())
        .collect();
    assert_eq!(resolved[0], Some("service:create#moqui.example.Example"));
    assert_eq!(resolved[1], Some("service:update#moqui.example.Example"));
    assert_eq!(resolved[7], None, "no entity, no service");
}

#[test]
fn bindings_resolve_to_derived_parameters_and_are_replaced_per_file() {
    let short = CALLS.replace("update#moqui.example.Example", "update#Example");
    let path = "/c/example/service/example/Things.xml";
    let mut index = memory(vec![
        ("/c/example/entity/ExampleEntities.xml", entities()),
        (
            path,
            read(
                ArtifactKind::Service,
                path,
                "component://example/service/example/Things.xml",
                &short,
            ),
        ),
    ]);
    let to = "parameter:service:update#moqui.example.Example@in.exampleName";
    let found = index.bindings_of("", to);
    assert_eq!(found.len(), 1, "the short entity name binds to the derived parameter");
    assert_eq!(found[0].path, path);
    assert_eq!(found[0].binding.source_text, "name");
    assert!(index.artifacts.iter().any(|artifact| artifact.id == to));
    // the field-map's parameter reference resolves to the derived parameter too
    assert!(index
        .refs
        .iter()
        .any(|(_, row)| row.to_kind == ToKind::Parameter && row.to_id.as_deref() == Some(to)));
    assert_eq!(index.bindings_of("service:example.Things.run#Thing", "").len(), 6);

    // the file read again without its field-maps: its bindings go
    let plain = r#"<services><service verb="run" noun="Thing"><actions>
        <service-call name="update#Example"/>
    </actions></service></services>"#;
    let (artifacts, refs, bindings) = (
        std::mem::take(&mut index.artifacts),
        std::mem::take(&mut index.refs),
        std::mem::take(&mut index.bindings),
    );
    index.artifacts = artifacts.into_iter().filter(|artifact| artifact.path != path).collect();
    index.refs = refs.into_iter().filter(|(file, _)| file != path).collect();
    index.bindings = bindings.into_iter().filter(|(file, _)| file != path).collect();
    index.take_rows(
        path.to_string(),
        read(
            ArtifactKind::Service,
            path,
            "component://example/service/example/Things.xml",
            plain,
        ),
    );
    index.resolve_all(&|_| false);
    assert!(index.bindings.is_empty());
    assert_eq!(
        index
            .artifacts
            .iter()
            .filter(|artifact| artifact.kind == "service" && artifact.origin == "derived")
            .count(),
        1
    );
}
