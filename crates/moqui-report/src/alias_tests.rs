use super::*;
use serde_json::json;

fn never() -> bool {
    panic!("asked a holder the rule does not need")
}

#[test]
fn a_saved_telemetry_alias_opens_to_a_holder_of_the_index_or_by_its_owners_holding() {
    // the steward's alias: neither the reader nor the owner (the steward) holds otel_logs
    assert!(!opens(Source::Telemetry, false, || false, || false));
    // an administrator reads any alias; the owner is not asked
    assert!(opens(Source::Telemetry, false, || true, never));
    // an administrator's alias, shared: its reader does not hold the index
    assert!(opens(Source::Telemetry, false, || false, || true));
}

#[test]
fn a_shipped_telemetry_alias_is_its_components_and_asks_nobody() {
    assert!(opens(Source::Telemetry, true, never, never));
}

#[test]
fn an_alias_of_a_business_document_opens_only_to_a_holder_of_the_document() {
    assert!(opens(Source::Governed, false, || true, never));
    // neither a component's file nor an owner's holding opens a business document past its admitted dataset
    assert!(!opens(Source::Governed, true, || false, || true));
    assert!(!opens(Source::Governed, false, || false, || true));
}

#[test]
fn a_body_reads_its_index_else_its_document_else_the_document_of_its_id() {
    assert_eq!(
        target(
            "insights-logs",
            &json!({"indexName":"otel_logs","documentType":"Other"})
        ),
        Target::Index("otel_logs".into())
    );
    assert_eq!(
        target(
            "insights-logs",
            &json!({"documentType":"otel_logs","signal":"business"})
        ),
        Target::Documents(vec!["otel_logs".into()])
    );
    assert_eq!(
        target("MantleSalesOrderPart", &json!({})),
        Target::Documents(vec!["MantleSalesOrderPart".into()])
    );
    assert_eq!(artifact("otel_logs"), "org.moqui.report.dataset.otel_logs");
}
