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

//! The parameter owners: one row form for a service's, a screen's, a transition's and a REST method's parameters, and a
//! transition's parameters from the service its own `service-call` calls.

use std::collections::HashSet;

use super::*;
use crate::index::extract::{extract, FileInfo, ORIGIN_SERVICE_CALL};
use crate::index::MemoryIndex;
use crate::ArtifactKind;

const SERVICES: &str = r#"<services>
    <service verb="save" noun="Order">
        <in-parameters><parameter name="orderId" required="true"/><parameter name="quantity" type="BigDecimal"/>
            <parameter name="note" default-value="none"/></in-parameters>
        <out-parameters><parameter name="orderId"/><parameter name="total" type="BigDecimal"/></out-parameters>
    </service>
    <service verb="cancel" noun="Order">
        <implements service="c.OrderServices.save#Order"/>
        <in-parameters><parameter name="quantity" required="disabled"/></in-parameters>
    </service>
</services>"#;

const SCREEN: &str = r#"<screen>
    <transition name="save" method="post"><parameter name="quantity" required="true"/>
        <path-parameter name="storeId"/>
        <service-call name="c.OrderServices.save#Order"/><default-response url="."/></transition>
    <transition name="own"><service-call name="c.OrderServices.save#Order" in-map="[orderId: orderId]"/>
        <default-response url="."/></transition>
    <transition name="cancel"><service-call name="c.OrderServices.cancel#Order"/><default-response url="."/></transition>
    <transition name="script"><actions><set field="a" value="1"/></actions><default-response url="."/></transition>
</screen>"#;

const SCREEN_LOCATION: &str = "component://c/screen/Orders.xml";

fn index_of(files: &[(ArtifactKind, &str, &str)]) -> MemoryIndex {
    let mut index = MemoryIndex::default();
    for (kind, location, text) in files {
        let path = format!("/tmp/owners/{}", location.trim_start_matches("component://"));
        let read = extract(
            &FileInfo {
                path: &path,
                location,
                component: "c",
                kind: *kind,
            },
            text,
        )
        .unwrap();
        index.take_rows(path, read);
    }
    index.resolve_all(&|_| false);
    index
}

fn orders() -> MemoryIndex {
    index_of(&[
        (
            ArtifactKind::Service,
            "component://c/service/c/OrderServices.xml",
            SERVICES,
        ),
        (ArtifactKind::Screen, SCREEN_LOCATION, SCREEN),
    ])
}

/// The parameters of `owner`, each as `<direction>.<name> <origin>`, sorted.
fn params_of(index: &MemoryIndex, owner: &str) -> Vec<String> {
    let mut found: Vec<String> = index
        .artifacts
        .iter()
        .filter(|row| row.kind == "parameter")
        .filter_map(|row| {
            let (of, direction, name) = parse_parameter_id(&row.id)?;
            (of == owner).then(|| format!("{direction}.{name} {}", row.origin))
        })
        .collect();
    found.sort();
    found
}

#[test]
fn one_row_form_for_every_owner_with_nested_parents() {
    let place = ParamPlace {
        component: "c",
        path: "/a.xml",
        line: 3,
        column: 5,
        origin: "",
    };
    let nested = ParamSpec::new(ParamDirection::In, "order.items", "List", "true").with_default(Some("[]".into()));
    let row = parameter_row("service:a.B.save#Order", &nested, &place);
    assert_eq!(row.id, "parameter:service:a.B.save#Order@in.order.items");
    assert_eq!(row.name, "a.B.save#Order@in.order.items");
    assert_eq!(row.short_name, "items");
    assert_eq!(row.summary, "in List, required=true, default []");
    assert_eq!(row.parent.as_deref(), Some("parameter:service:a.B.save#Order@in.order"));
    assert_eq!((row.kind.as_str(), row.line, row.column), ("parameter", 3, 5));
    let specs = [
        ParamSpec::new(ParamDirection::Out, "total", "BigDecimal", "false"),
        ParamSpec::new(ParamDirection::Path, "orderId", "String", "true"),
    ];
    for owner in [
        "transition:component://c/screen/A.xml#save:post",
        "rest:/orders/{orderId}:get",
        "screen:component://c/screen/A.xml",
    ] {
        let rows = parameter_rows(owner, &specs, &place);
        let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                format!("parameter:{owner}@out.total"),
                format!("parameter:{owner}@path.orderId")
            ]
        );
        assert!(rows.iter().all(|row| row.parent.as_deref() == Some(owner)));
        assert_eq!(rows[1].summary, "path String, required=true");
        for row in &rows {
            let (of, direction, _) = parse_parameter_id(&row.id).unwrap();
            assert_eq!(of, owner);
            assert!(ParamDirection::parse(direction).is_some());
        }
    }
}

#[test]
fn a_service_has_its_declared_in_and_out_parameters() {
    let index = orders();
    let owner = "service:c.OrderServices.save#Order";
    assert_eq!(
        params_of(&index, owner),
        ["in.note ", "in.orderId ", "in.quantity ", "out.orderId ", "out.total "]
    );
    let note = index.named(&parameter_id(owner, "in", "note")).unwrap();
    assert_eq!(note.summary, "in String, required=false, default none");
    let total = index.named(&parameter_id(owner, "out", "total")).unwrap();
    assert_eq!(total.summary, "out BigDecimal, required=false");
}

#[test]
fn a_transition_takes_the_in_and_out_parameters_of_its_service_call() {
    let index = orders();
    let save = format!("transition:{SCREEN_LOCATION}#save:post");
    let call = ORIGIN_SERVICE_CALL;
    // its own quantity and path parameter win; the service's other in-parameters and every out-parameter are added
    assert_eq!(
        params_of(&index, &save),
        [
            format!("in.note {call}"),
            format!("in.orderId {call}"),
            "in.quantity ".to_string(),
            format!("out.orderId {call}"),
            format!("out.total {call}"),
            "path.storeId ".to_string(),
        ]
    );
    let total = index.named(&parameter_id(&save, "out", "total")).unwrap();
    assert_eq!(total.summary, "out BigDecimal, required=false");
    assert_eq!(total.parent.as_deref(), Some(save.as_str()));
    assert_eq!(total.path, "/tmp/owners/c/screen/Orders.xml");
    let order = index.named(&parameter_id(&save, "in", "orderId")).unwrap();
    assert_eq!(order.summary, "in String, required=true");
    let store = index.named(&parameter_id(&save, "path", "storeId")).unwrap();
    assert_eq!(store.summary, "path String, required=true");
}

#[test]
fn a_call_with_its_own_in_map_gives_only_out_parameters_and_an_actions_transition_none() {
    let index = orders();
    let call = ORIGIN_SERVICE_CALL;
    assert_eq!(
        params_of(&index, &format!("transition:{SCREEN_LOCATION}#own:any")),
        [format!("out.orderId {call}"), format!("out.total {call}")]
    );
    assert!(params_of(&index, &format!("transition:{SCREEN_LOCATION}#script:any")).is_empty());
}

#[test]
fn a_transition_takes_what_its_service_implements_without_a_disabled_parameter() {
    let index = orders();
    let call = ORIGIN_SERVICE_CALL;
    assert_eq!(
        params_of(&index, &format!("transition:{SCREEN_LOCATION}#cancel:any")),
        [
            format!("in.note {call}"),
            format!("in.orderId {call}"),
            format!("out.orderId {call}"),
            format!("out.total {call}"),
        ]
    );
}

#[test]
fn a_changed_service_expands_its_transitions_again() {
    let index = orders();
    let changed = HashSet::from(["service:c.OrderServices.save#Order".to_string()]);
    let dependents = index.resolver.dependents(&changed);
    for name in ["save:post", "own:any", "cancel:any"] {
        assert!(
            dependents.contains(&format!("transition:{SCREEN_LOCATION}#{name}")),
            "{name}: {dependents:?}"
        );
    }
    // the service that implements it reads it too
    assert!(dependents.contains("service:c.OrderServices.cancel#Order"));
    assert!(!dependents.contains(&format!("transition:{SCREEN_LOCATION}#script:any")));
}

#[test]
fn an_entity_auto_service_call_gives_the_derived_service_parameters() {
    let entities = r#"<entities><entity entity-name="Order" package="c">
        <field name="orderId" type="id" is-pk="true"/><field name="statusId" type="id"/></entity></entities>"#;
    let screen = r#"<screen><transition name="make"><service-call name="create#c.Order"/>
        <default-response url="."/></transition></screen>"#;
    let index = index_of(&[
        (ArtifactKind::Entity, "component://c/entity/Orders.xml", entities),
        (ArtifactKind::Screen, SCREEN_LOCATION, screen),
    ]);
    let call = ORIGIN_SERVICE_CALL;
    assert_eq!(
        params_of(&index, &format!("transition:{SCREEN_LOCATION}#make:any")),
        [
            format!("in.orderId {call}"),
            format!("in.statusId {call}"),
            format!("out.orderId {call}"),
        ]
    );
}
