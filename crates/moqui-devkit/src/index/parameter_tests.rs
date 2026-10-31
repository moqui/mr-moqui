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
use extract::{extract, parameter_id, parse_parameter_id, FileInfo, ToKind};

fn read(kind: ArtifactKind, location: &str, text: &str) -> extract::FileIndex {
    extract(
        &FileInfo {
            path: "/tmp/parameters.xml",
            location,
            component: "c",
            kind,
        },
        text,
    )
    .unwrap()
}

#[test]
fn service_parameters_have_direction_nesting_type_and_requiredness() {
    let file = read(
        ArtifactKind::Service,
        "component://c/service/a/B.xml",
        r#"<services>
      <service verb="verb" noun="Noun"><in-parameters>
        <parameter name="aId" required="true"/>
        <parameter name="order" type="Map"><parameter name="items" type="List"/></parameter>
      </in-parameters><out-parameters><parameter name="aId" type="Long"/></out-parameters></service>
    </services>"#,
    );
    let owner = "service:a.B.verb#Noun";
    let index = MemoryIndex {
        artifacts: file.artifacts,
        ..MemoryIndex::default()
    };
    let input = index.named(&parameter_id(owner, "in", "aId")).unwrap();
    assert_eq!(input.parent.as_deref(), Some(owner));
    assert!(input.summary.contains("String") && input.summary.contains("required"));
    let output = index.named(&parameter_id(owner, "out", "aId")).unwrap();
    assert!(output.summary.contains("Long"));
    let nested = index.named(&parameter_id(owner, "in", "order.items")).unwrap();
    assert_eq!(nested.parent, Some(parameter_id(owner, "in", "order")));
    assert_eq!(nested.short_name, "items");
    assert!(nested.summary.contains("List"));
}

#[test]
fn transitions_keep_overloads_and_unknown_method_references_reach_all() {
    let location = "component://c/screen/A.xml";
    let file = read(
        ArtifactKind::Screen,
        location,
        r#"<screen>
      <parameter name="aId" required="true"/>
      <transition name="save" method="GET"><parameter name="aId"/></transition>
      <transition name="save" method="post"><parameter name="aId"/><path-parameter name="pathId"/></transition>
      <transition name="view"/>
      <widgets><link url="save"/><link url="save" method="POST"/><link url="view" method="GET"/></widgets>
    </screen>"#,
    );
    let mut index = MemoryIndex {
        artifacts: file.artifacts,
        ..MemoryIndex::default()
    };
    for row in &index.artifacts {
        index.resolver.add(row);
    }
    for mut row in file.refs {
        index.resolver.resolve(&mut row, &|_| false);
        index.refs.push(("x".into(), row));
    }
    assert_eq!(index.find("save", &["transition".into()], 10).len(), 2);
    for method in ["get", "post"] {
        let owner = format!("transition:{location}#save:{method}");
        assert_eq!(
            index.named(&parameter_id(&owner, "in", "aId")).unwrap().parent,
            Some(owner)
        );
    }
    assert!(index.named(&format!("transition:{location}#view:any")).is_some());
    let path = index
        .named(&format!("parameter:transition:{location}#save:post@path.pathId"))
        .unwrap();
    assert!(path.summary.contains("String") && path.summary.contains("required"));
    assert!(index.named(&format!("parameter:screen:{location}@in.aId")).is_some());
    let targets: Vec<_> = index
        .refs
        .iter()
        .map(|(_, row)| row.to_id.as_deref().unwrap())
        .collect();
    assert_eq!(
        targets,
        [
            format!("transition:{location}#save:get"),
            format!("transition:{location}#save:post"),
            format!("transition:{location}#save:post"),
            format!("transition:{location}#view:any")
        ]
    );
}

#[test]
fn rest_methods_own_service_refs_and_declared_path_parameters() {
    let file = read(
        ArtifactKind::Rest,
        "component://c/service/api.rest.xml",
        r#"<resource name="orders">
      <id name="orderId">
        <method type="GET"><service name="a.B.get#Order"/></method>
        <method type="POST">
          <service name="a.B.save#Order"/></method>
      </id>
    </resource>"#,
    );
    let find = |id: &str| file.artifacts.iter().find(|row| row.id == id).unwrap();
    for method in ["get", "post"] {
        assert_eq!(
            find(&format!("rest:/orders/{{orderId}}:{method}")).parent.as_deref(),
            Some("rest:/orders/{orderId}")
        );
    }
    for method in ["get", "post"] {
        assert_eq!(
            find(&format!("parameter:rest:/orders/{{orderId}}:{method}@path.orderId")).parent,
            Some(format!("rest:/orders/{{orderId}}:{method}"))
        );
    }
    assert!(!file.artifacts.iter().any(|row| row.id.contains("@in.")));
    let mut resolver = Resolver::default();
    for name in ["a.B.get#Order", "a.B.save#Order"] {
        resolver.add_parts(&format!("service:{name}"), "service", name, name);
    }
    for mut reference in file.refs.clone() {
        resolver.resolve(&mut reference, &|_| false);
        assert!(reference.resolved);
        assert_eq!(reference.to_id, Some(format!("service:{}", reference.to_name)));
    }
    assert!(file
        .refs
        .iter()
        .any(|row| row.from == "rest:/orders/{orderId}:get" && row.to_name == "a.B.get#Order"));
    assert!(file
        .refs
        .iter()
        .any(|row| row.from == "rest:/orders/{orderId}:post" && row.to_name == "a.B.save#Order"));
}

#[test]
fn field_maps_resolve_only_existing_input_parameters_and_can_unresolve_again() {
    let mut file = read(
        ArtifactKind::Service,
        "component://c/service/a/B.xml",
        r#"<services>
      <service verb="save" noun="Order"><in-parameters><parameter name="quantity"/></in-parameters>
        <out-parameters><parameter name="result"/></out-parameters></service>
      <service verb="call"><actions><service-call name="a.B.save#Order">
        <field-map field-name="quantity" from="1"/><field-map field-name="missing" from="2"/>
        <field-map field-name="result" from="3"/>
      </service-call></actions></service>
    </services>"#,
    );
    let mut index = MemoryIndex::default();
    for artifact in &file.artifacts {
        index.resolver.add(artifact);
    }
    for row in &mut file.refs {
        index.resolver.resolve(row, &|_| false);
    }
    let parameters: Vec<_> = file
        .refs
        .iter()
        .filter(|row| row.to_kind == ToKind::Parameter)
        .collect();
    assert_eq!(parameters.len(), 3);
    assert!(parameters[0].resolved);
    assert!(!parameters[1].resolved && !parameters[2].resolved);
    let id = "parameter:service:a.B.save#Order@in.quantity";
    assert_eq!(parameters[0].to_id.as_deref(), Some(id));
    index.refs = file.refs.into_iter().map(|row| ("x".into(), row)).collect();
    index.artifacts = file.artifacts;
    assert_eq!(index.refs(id, Direction::In).len(), 1);
    index.resolver.remove(&index.named(id).unwrap());
    for (_, row) in &mut index.refs {
        index.resolver.resolve(row, &|_| false);
    }
    assert!(index.refs(id, Direction::In).is_empty());
    assert!(index
        .refs
        .iter()
        .filter(|(_, row)| row.to_kind == ToKind::Parameter)
        .all(|(_, row)| !row.resolved && row.to_id.is_none()));
}

#[test]
fn parameter_ids_round_trip_method_owners_and_reject_invalid_directions() {
    for owner in [
        "service:a.B.verb#Noun",
        "screen:component://c/screen/A.xml",
        "transition:component://c/screen/A.xml#save:post",
        "rest:/orders/{orderId}:post",
    ] {
        for direction in ["in", "out", "path"] {
            let id = parameter_id(owner, direction, "order.items");
            assert_eq!(parse_parameter_id(&id), Some((owner, direction, "order.items")));
        }
    }
    for id in [
        "parameter:@in.a",
        "parameter:service:a@in.",
        "parameter:service:a@wrong.a",
        "service:a@in.a",
    ] {
        assert!(parse_parameter_id(id).is_none());
    }
}

#[test]
fn check_reports_unknown_field_map_parameter() {
    let ctx = ExecutionContextFactory::test_context_with_components(
        "",
        moqui_core::service::ServicePools::idle(),
        moqui_core::context::framework_dir(),
        Vec::new(),
    );
    let _entered = ctx.enter();
    let dir = std::env::temp_dir()
        .join(format!("parameter-check-{}", std::process::id()))
        .join("service/parametercheck");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Calls.xml");
    let text = r#"<services><service verb="save" noun="Order" type="interface">
        <in-parameters><parameter name="quantity"/></in-parameters></service>
        <service verb="call"><actions><service-call name="parametercheck.Calls.save#Order">
          <field-map field-name="quantity" from="1"/><field-map field-name="unknownQuantity" from="2"/>
        </service-call></actions></service></services>"#;
    std::fs::write(&path, text).unwrap();
    moqui_core::service::define::reload(std::slice::from_ref(&path)).unwrap();
    let problems = crate::check::check_text(ctx.factory(), &path, text);
    let missing: Vec<_> = problems
        .iter()
        .filter(|problem| problem.rule == "reference.parameter")
        .collect();
    assert_eq!(missing.len(), 1, "{problems:?}");
    assert!(missing[0].message.contains("unknownQuantity"));
    std::fs::remove_dir_all(dir.parent().unwrap().parent().unwrap()).unwrap();
}

#[test]
fn rest_literal_route_variables_and_explicit_path_declarations_belong_to_method() {
    let file = read(
        ArtifactKind::Rest,
        "component://c/service/api.rest.xml",
        r#"<resource name="orders/{orderId}"><path-parameter name="shared"/>
        <method type="patch"><path-parameter name="extra"/><id name="explicit"/><service name="a.B.save#Order"/></method>
    </resource>"#,
    );
    for name in ["orderId", "extra", "explicit", "shared"] {
        let id = format!("parameter:rest:/orders/{{orderId}}:patch@path.{name}");
        let row = file.artifacts.iter().find(|row| row.id == id).unwrap();
        assert_eq!(row.parent.as_deref(), Some("rest:/orders/{orderId}:patch"));
    }
}

#[test]
fn transition_extension_anchor_still_resolves_with_method_ids() {
    let location = "component://c/screen/A.xml";
    let screen = read(
        ArtifactKind::Screen,
        location,
        r#"<screen><transition name="save"/></screen>"#,
    );
    let mut resolver = Resolver::default();
    for row in &screen.artifacts {
        resolver.add(row);
    }
    let mut reference = extract::RefRow {
        from: "extension".into(),
        to_kind: ToKind::Anchor,
        to_name: "save".into(),
        to_scope: location.into(),
        ref_type: "anchor".into(),
        line: 1,
        column: 1,
        to_id: None,
        resolved: false,
    };
    resolver.resolve(&mut reference, &|_| false);
    assert_eq!(
        reference.to_id.as_deref(),
        Some("transition:component://c/screen/A.xml#save:any")
    );
}

/// A parameter is named for its owner, so a find of a bare name gives an entity's field first, and the refs of a
/// service list each caller once, not once more for each of its field-maps.
#[test]
fn a_field_comes_before_parameters_of_its_name_and_field_maps_stay_out_of_service_refs() {
    let entities = read(
        ArtifactKind::Entity,
        "component://c/entity/A.xml",
        r#"<entities><entity entity-name="Order" package="a"><field name="quantity" type="number-decimal"/></entity></entities>"#,
    );
    let mut services = read(
        ArtifactKind::Service,
        "component://c/service/a/B.xml",
        r#"<services>
      <service verb="save" noun="Order"><in-parameters><parameter name="quantity"/></in-parameters></service>
      <service verb="first"><actions><service-call name="a.B.save#Order"><field-map field-name="quantity" from="1"/>
      </service-call></actions></service>
      <service verb="second"><actions><service-call name="a.B.save#Order"><field-map field-name="quantity" from="2"/>
      </service-call></actions></service>
    </services>"#,
    );
    let mut index = MemoryIndex::default();
    for artifact in entities.artifacts.iter().chain(&services.artifacts) {
        index.resolver.add(artifact);
    }
    for row in &mut services.refs {
        index.resolver.resolve(row, &|_| false);
    }
    let parameter = "parameter:service:a.B.save#Order@in.quantity";
    index.artifacts = entities.artifacts.into_iter().chain(services.artifacts).collect();
    index.refs = services.refs.into_iter().map(|row| ("x".into(), row)).collect();

    let named = index.describe(parameter).unwrap();
    assert_eq!(named.name, "a.B.save#Order@in.quantity");
    assert_eq!(named.short_name, "quantity");
    assert_eq!(index.named("quantity").unwrap().id, "field:a.Order.quantity");
    let found: Vec<String> = index.find("quantity", &[], 5).into_iter().map(|row| row.kind).collect();
    assert_eq!(found, ["field", "parameter"]);

    let callers = index.refs("service:a.B.save#Order", Direction::In);
    assert_eq!(callers.len(), 2, "{callers:?}");
    assert!(
        callers.iter().all(|entry| entry.ref_type == "service-call"),
        "{callers:?}"
    );
    let used = index.refs("service:a.B.first", Direction::Out);
    assert_eq!(used.len(), 1, "{used:?}");
    // the parameter's own refs are its field-maps
    assert_eq!(index.refs(parameter, Direction::In).len(), 2);
}
