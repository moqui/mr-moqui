use super::*;
use std::collections::BTreeMap;

fn parse(text: &str) -> (Conf, Vec<String>) {
    let node = MNode::parse_shared("operations", text).expect("test conf parses");
    let mut warnings = Vec::new();
    (Conf::from_node(&node, &mut warnings), warnings)
}

#[test]
fn a_service_is_its_own_operation_unless_declared_or_named_by_the_conf() {
    let (conf, warnings) = parse(
        r#"<operations>
            <operation service="org\.moqui\..*" record="false"/>
            <operation service="mantle\.order\.OrderServices\.place#Order" name="order.place" ref="orderId"/>
        </operations>"#,
    );
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        conf.resolve("mantle.party.PartyServices.create#Person", None, None),
        Some(Operation {
            name: "mantle.party.PartyServices.create#Person".into(),
            reference: None
        })
    );
    assert_eq!(
        conf.resolve(
            "mantle.party.PartyServices.create#Person",
            Some("party.create"),
            Some("partyId")
        ),
        Some(Operation {
            name: "party.create".into(),
            reference: Some("partyId".into())
        })
    );
    // The conf's name wins over the declaration, as a site may rename an operation it did not write.
    assert_eq!(
        conf.resolve("mantle.order.OrderServices.place#Order", Some("placeOrder"), None),
        Some(Operation {
            name: "order.place".into(),
            reference: Some("orderId".into())
        })
    );
    assert_eq!(
        conf.resolve("org.moqui.impl.ServerServices.ping#Server", Some("ping"), None),
        None
    );
}

#[test]
fn declared_mode_records_only_declared_or_named_services() {
    let (conf, _) =
        parse(r#"<operations record="declared"><operation service="mantle\.order\..*" ref="orderId"/></operations>"#);
    assert_eq!(
        conf.resolve("mantle.party.PartyServices.create#Person", None, None),
        None
    );
    assert_eq!(
        conf.resolve("mantle.party.PartyServices.create#Person", Some("party.create"), None)
            .map(|operation| operation.name),
        Some("party.create".into())
    );
    assert_eq!(
        conf.resolve("mantle.order.OrderServices.approve#Order", None, None),
        Some(Operation {
            name: "mantle.order.OrderServices.approve#Order".into(),
            reference: Some("orderId".into())
        })
    );
    let (none, _) = parse(r#"<operations record="none"/>"#);
    assert_eq!(none.resolve("a.B.c#D", Some("declared"), None), None);
}

#[test]
fn bad_settings_are_warned_about_and_the_defaults_kept() {
    let (conf, warnings) = parse(
        r#"<operations record="sometimes" max-names="0"><operation service="(" /><operation name="x"/></operations>"#,
    );
    assert_eq!(conf.record, Record::Default);
    assert_eq!(conf.max_names, 500);
    assert_eq!(warnings.len(), 4, "{warnings:?}");
}

#[test]
fn names_past_the_limit_are_other_and_each_service_resolves_once() {
    let (conf, _) = parse(r#"<operations max-names="2"/>"#);
    let registry = Registry::default();
    let names: Vec<String> = ["a.S.one#A", "a.S.two#A", "a.S.three#A", "a.S.one#A"]
        .iter()
        .map(|service| {
            registry
                .operation(&conf, service, None, None)
                .expect("an operation")
                .name
                .clone()
        })
        .collect();
    assert_eq!(names, ["a.S.one#A", "a.S.two#A", OTHER, "a.S.one#A"]);
}

#[test]
fn outcomes_and_references() {
    assert_eq!(Outcome::ended(true, true), Outcome::Commit);
    assert_eq!(Outcome::ended(false, true), Outcome::Retry);
    assert_eq!(Outcome::ended(false, false), Outcome::Rollback);
    let operation = Operation {
        name: "order.place".into(),
        reference: Some("orderId".into()),
    };
    let mut parameters = BTreeMap::new();
    assert_eq!(operation.reference_of(&parameters), None);
    parameters.insert("orderId".to_string(), Value::string("100010"));
    assert_eq!(operation.reference_of(&parameters).as_deref(), Some("orderId:100010"));
    parameters.insert("orderId".to_string(), Value::string("x".repeat(500)));
    assert_eq!(
        operation.reference_of(&parameters).map(|text| text.len()),
        Some("orderId:".len() + 128)
    );
}
