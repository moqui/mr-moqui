/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
//! Alert state, validation and contact rules, kept in `alert::tests`.
use super::*;
#[test]
fn timers_and_unknown_health() {
    let mut i = Instance::default();
    assert!(i.evaluate(Observation::Breaching, 100, 20, 30));
    assert_eq!(i.state, State::Pending);
    i.evaluate(Observation::Breaching, 119, 20, 30);
    assert_eq!(i.state, State::Pending);
    i.evaluate(Observation::Breaching, 120, 20, 30);
    assert_eq!(i.state, State::Firing);
    i.evaluate(Observation::NoData, 130, 20, 30);
    assert_eq!(i.state, State::NoData);
    i.evaluate(Observation::Healthy, 140, 20, 30);
    assert_eq!(i.state, State::Recovering);
    i.evaluate(Observation::Healthy, 169, 20, 30);
    assert_eq!(i.state, State::Recovering);
    i.evaluate(Observation::Healthy, 170, 20, 30);
    assert_eq!(i.state, State::Normal);
}
#[test]
fn error_is_not_normal_and_stale_samples_do_not_change_state() {
    let mut i = Instance::default();
    i.evaluate(Observation::Error, 100, 0, 0);
    i.evaluate(Observation::Healthy, 99, 0, 0);
    assert_eq!(i.state, State::Error);
    assert!(matches!(
        Condition::Above(1.0).observe(Some(f64::NAN)),
        Observation::NoData
    ));
}
#[test]
fn alert_instances_keep_only_approved_record_dimensions() {
    let result = json!({"frames":[{"fields":[{"name":"total","type":"number","values":[9]},{"name":"productId","type":"string","values":["P100"]},{"name":"body","type":"string","values":["private invoice text"]}]}]});
    let instances = series(
        &result,
        &json!({"field":"total"}),
        &BTreeSet::from(["productId".into()]),
    )
    .unwrap();
    let (labels, value) = instances.values().next().unwrap();
    assert_eq!(labels["productId"], "P100");
    assert!(labels.get("body").is_none());
    assert!(!labels.to_string().contains("private invoice text"));
    assert_eq!(*value, Some(9.0));
}
#[test]
fn conditions_need_a_known_type_and_ordered_bounds() {
    for good in [
        json!({}),
        json!({"condition":{"type":"above","threshold":10,"recoveryThreshold":7}}),
        json!({"condition":{"type":"below"}}),
        json!({"condition":{"type":"within","min":1,"max":1}}),
        json!({"condition":{"type":"outside","min":-5,"max":5},"pruneAfterMissing":3}),
    ] {
        assert!(validate_condition(&good).is_ok(), "{good}");
    }
    for bad in [
        json!({"condition":{"type":"abvoe","threshold":10}}),
        json!({"condition":{"type":"above","threshold":"ten"}}),
        json!({"condition":{"type":"within","min":5,"max":1}}),
        json!({"condition":{"type":"outside","min":1}}),
        json!({"condition":5}),
        json!({"condition":{"type":"above"},"pruneAfterMissing":0}),
    ] {
        assert!(validate_condition(&bad).is_err(), "{bad}");
    }
}
#[test]
fn document_queries_cannot_be_alert_sources() {
    let ec = moqui_core::context::ExecutionContextFactory::test_context();
    assert!(approved_labels(
        &ec,
        &json!({"query":{"queries":[{"mode":"documents","datasetId":"otel_logs"}]}})
    )
    .is_err());
}
#[test]
fn shared_contacts_use_property_names_and_never_literal_credentials() {
    let mut legacy = json!({"contactPoints":[{"type":"webhook","url":"https://example.test/alerts","headers":{"Authorization":"Bearer private-fixture-token"}}]});
    assert!(validate_contacts(&legacy).is_err());
    redact_contact_credentials(&mut legacy);
    assert!(!legacy.to_string().contains("private-fixture-token"));
    assert!(validate_contacts(&json!({"contactPoints":[{"type":"webhook","urlProperty":"MONITOR_WEBHOOK_ALERT_URL","headerProperties":{"Authorization":"MONITOR_WEBHOOK_ALERT_AUTH"}}]})).is_ok());
    assert!(validate_contacts(
        &json!({"contactPoints":[{"type":"webhook","url":"https://user:password@example.test/alerts"}]})
    )
    .is_err());
}
#[test]
fn shared_items_validate_like_rule_items() {
    for (kind, good) in [
        ("contactPoint", json!({"type":"notification","topic":"ReportAlert"})),
        (
            "contactPoint",
            json!({"type":"webhook","urlProperty":"MONITOR_WEBHOOK_OPS_URL"}),
        ),
        (
            "silence",
            json!({"from":"2026-10-26T00:00:00Z","thru":"2026-10-27T00:00:00Z","matchers":{"team":"ops"}}),
        ),
        ("silence", json!({"from":"now","thru":"now+2h"})),
        ("muteTiming", json!({"weekdays":[5,6],"start":"18:00","end":"24:00"})),
        ("muteTiming", json!({"start":"22:00","end":"06:00"})),
    ] {
        assert!(validate_shared(kind, &good).is_ok(), "{kind} {good}");
    }
    for (kind, bad) in [
        ("contactPoint", json!({"type":"pager"})),
        (
            "contactPoint",
            json!({"type":"webhook","url":"https://example.test/a","headers":{"Authorization":"x"}}),
        ),
        ("contactPoint", json!("ops")),
        (
            "silence",
            json!({"from":"2026-10-27T00:00:00Z","thru":"2026-10-26T00:00:00Z"}),
        ),
        ("silence", json!({"from":"2026-10-26T00:00:00Z"})),
        ("silence", json!({"from":"now","thru":"now+1h","matchers":["team"]})),
        ("muteTiming", json!({"weekdays":[]})),
        ("muteTiming", json!({"weekdays":[7]})),
        ("muteTiming", json!({"start":"24:00"})),
        ("muteTiming", json!({"end":"24:01"})),
        ("muteTiming", json!({"start":"9:00"})),
    ] {
        assert!(validate_shared(kind, &bad).is_err(), "{kind} {bad}");
    }
}
#[test]
fn shared_contact_points_hide_credentials_from_other_viewers() {
    let mut body = json!({"type":"webhook","url":"https://example.test/a","headers":{"Authorization":"Bearer fixture"},"headerProperties":{"Authorization":"MONITOR_WEBHOOK_A"}});
    redact_definition_credentials(&json!("contactPoint"), &mut body);
    assert!(!body.to_string().contains("Bearer fixture"));
    assert_eq!(body["credentialHeadersOmitted"], true);
    assert_eq!(body["headerProperties"]["Authorization"], "MONITOR_WEBHOOK_A");
}
#[test]
fn shared_items_join_the_rule_body_for_muting() {
    let monday_noon = "2026-10-26T12:00:00Z"
        .parse::<jiff::Timestamp>()
        .unwrap()
        .as_millisecond();
    let labels = json!({"team":"ops"});
    let mut body = json!({});
    assert!(!muted(&body, &labels, monday_noon));
    body["silences"] = json!([{"from":"2026-10-26T11:00:00Z","thru":"2026-10-26T13:00:00Z","matchers":{"team":"ops"}}]);
    assert!(muted(&body, &labels, monday_noon));
    assert!(!muted(&body, &json!({"team":"sales"}), monday_noon));
}
