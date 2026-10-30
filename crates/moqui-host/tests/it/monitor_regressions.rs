//! Regressions and incidents on a real seeded PostgreSQL server, from synthetic minute metrics and release markers: a
//! version-wide error rise opens one regression and one incident that ranks the release first, a rise on one instance
//! is an instance fault, gaps degrade a verdict, a second pass adds nothing, a held lease skips the pass, and an
//! incident goes to Monitoring when its regression resolves and to Resolved after resolve-minutes
//! (plans/Monitoring.md, Later: Regressions and incidents).
#![cfg(all(unix, feature = "embedded-postgres", feature = "axum", feature = "telemetry"))]
use crate::common;
use moqui_core::entity::document::Document;
use moqui_lib::object::{to_plain_string, Value};
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;

const MINUTE: i64 = 60_000;
const BOUNDS: [f64; 14] = [
    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
];

fn document(value: Json) -> Document {
    match moqui_lib::json::from_json(&value) {
        Value::Map(map) => map.into_iter().collect(),
        _ => unreachable!(),
    }
}

/// A minute of requests of one instance: `ok` answered 200 and `failed` 500, all in the 25-50 ms bucket.
fn minute(service: &str, version: &str, instance: &str, at: i64, ok: u64, failed: u64) -> Vec<Document> {
    let at = at - at.rem_euclid(MINUTE);
    [(200, ok), (500, failed)]
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .map(|(status, count)| {
            let mut buckets = vec![0_u64; BOUNDS.len() + 1];
            buckets[3] = count;
            document(json!({
                "_id": format!("{service}-{version}-{instance}-{at}-{status}"),
                "name": "http.server.request.duration",
                "granularity": "minute",
                "aggregation": "histogram",
                "bucket_start": at,
                "bucket_end": at + MINUTE,
                "service.name": service,
                "service.version": version,
                "service.instance.id": instance,
                "http.response.status_code": status,
                "count": count,
                "sum": count as f64 * 0.03,
                "bounds": BOUNDS,
                "buckets": buckets,
            }))
        })
        .collect()
}

fn release(service: &str, version: &str, instance: &str, at: i64) -> Document {
    document(json!({
        "_id": format!("release-{service}-{version}-{instance}"),
        "time": at,
        "source": "release",
        "scope": "service",
        "service.name": service,
        "service.version": version,
        "service.instance.id": instance,
        "vcs.ref.head.revision": format!("rev-{version}"),
        "text": format!("{service} {version}"),
        "tags": ["release"],
    }))
}

fn call(service: &str, parameters: Json) -> Json {
    let Value::Map(map) = moqui_lib::json::from_json(&parameters) else {
        unreachable!()
    };
    let parameters: BTreeMap<String, Value> = map.into_iter().collect();
    let out = moqui_core::service::sync(service, &parameters).unwrap_or_else(|error| panic!("{service}: {error}"));
    let errors = moqui_core::context::ExecutionContext::with_current(|ctx| ctx.message().errors().to_vec());
    assert_eq!(errors, Some(Vec::new()), "{service}");
    moqui_lib::json::to_json(&Value::Map(out.into_iter().collect()))
}

#[test]
fn regressions_open_incidents_with_ranked_causes_and_settle() {
    if !common::own_process() {
        return;
    }
    let gate = common::start_with_files(
        "monitor-regressions",
        None,
        &[],
        &[
            (
                "component/regression-fixture/component.xml",
                r#"<component name="regression-fixture"/>"#,
            ),
            (
                "component/regression-fixture/MoquiConf.xml",
                r#"<moqui-conf><telemetry><regressions lookback-hours="1" window-minutes="30" resolve-minutes="20"/></telemetry></moqui-conf>"#,
            ),
        ],
    );
    let factory = gate.server.factory();
    let search = moqui_core::search::installed(factory).unwrap();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let now = jiff::Timestamp::now().as_millisecond();
    let released = now - 40 * MINUTE;

    // shop: 1.0 for the hour before 2.0, on two instances; 2.0 fails ten times as often on both.
    let mut docs = vec![
        release("shop", "1.0", "shop-a", released - 5 * 60 * MINUTE),
        release("shop", "2.0", "shop-a", released),
        release("shop", "2.0", "shop-b", released + MINUTE),
        release("cart", "1.0", "cart-c", now - 10 * 60 * MINUTE),
    ];
    let mut metrics = Vec::new();
    for step in 1..=60 {
        for instance in ["shop-a", "shop-b"] {
            metrics.extend(minute("shop", "1.0", instance, released - step * MINUTE, 199, 1));
        }
    }
    for step in 0..30 {
        let at = now - (step + 1) * MINUTE;
        for instance in ["shop-a", "shop-b"] {
            metrics.extend(minute("shop", "2.0", instance, at, 180, 20));
        }
        // cart: one version a day on; only cart-c fails, cart-d is healthy.
        metrics.extend(minute("cart", "1.0", "cart-c", at, 160, 40));
        metrics.extend(minute("cart", "1.0", "cart-d", at, 200, 0));
        // billing: a rise with only a third of the minutes reported.
        if step % 3 == 0 {
            metrics.extend(minute("billing", "1.0", "bill-e", at, 300, 60));
        }
    }
    for step in 0..30 {
        let day_before = now - 24 * 60 * MINUTE - (step + 1) * MINUTE;
        metrics.extend(minute("cart", "1.0", "cart-c", day_before, 199, 1));
        metrics.extend(minute("cart", "1.0", "cart-d", day_before, 199, 1));
        metrics.extend(minute("billing", "1.0", "bill-e", day_before, 300, 1));
    }
    search.index_entries("otel_metrics", metrics).unwrap();
    docs.push(document(
        json!({"_id":"marker-config","time":released - 30 * MINUTE,"source":"config","text":"pool size 4",
        "service.name":"shop"}),
    ));
    search.index_entries("monitor_annotations", docs).unwrap();
    search
        .index_entries(
            "otel_errors",
            vec![document(
                json!({"_id":"shop-error","fingerprint":"fp-shop","service.name":"shop",
                "exception.type":"OrderTotalMismatch","first_seen":released + 2 * MINUTE,"last_seen":now,"count":900}),
            )],
        )
        .unwrap();

    // A held lease skips the pass.
    let entity = factory.entity().unwrap();
    let lease = moqui_report::lock::claim(&context, &entity, "regressions", 60_000)
        .unwrap()
        .unwrap();
    assert_eq!(
        call("org.moqui.monitor.MonitorServices.detect#Regressions", json!({}))["skipped"],
        "another server is detecting"
    );
    moqui_report::lock::release(&context, &entity, "regressions", &lease);

    let pass = call("org.moqui.monitor.MonitorServices.detect#Regressions", json!({}));
    assert_eq!(pass["failed"], json!([]), "{pass}");
    assert_eq!(pass["regressions"], 3, "{pass}");
    assert_eq!(pass["incidents"], 3, "{pass}");

    let regressions = call(
        "org.moqui.monitor.MonitorServices.list#Regressions",
        json!({"statusId":"Open"}),
    )["regressions"]
        .clone();
    let of = |service: &str| {
        regressions
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["serviceName"] == service && row["metric"] == "error_rate")
            .unwrap_or_else(|| panic!("no {service} regression in {regressions}"))
            .clone()
    };
    let shop = of("shop");
    assert_eq!(
        (shop["baselineVersion"].as_str(), shop["locality"].as_str()),
        (Some("1.0"), Some("version")),
        "{shop}"
    );
    assert_eq!(shop["affectedInstances"], "shop-a,shop-b");
    assert_eq!(shop["dataQuality"], "good");
    let cart = of("cart");
    assert_eq!(
        (cart["locality"].as_str(), cart["affectedInstances"].as_str()),
        (Some("instance"), Some("cart-c")),
        "{cart}"
    );
    assert_eq!(cart["healthyInstances"], "cart-d");
    let billing = of("billing");
    assert_eq!(billing["dataQuality"], "degraded", "{billing}");

    // The shop incident: its timeline and the release ranked first, the config change and the new error after it.
    let incident = call(
        "org.moqui.monitor.MonitorServices.get#Incident",
        json!({"incidentId": shop["incidentId"]}),
    );
    assert_eq!(incident["incident"]["statusId"], "Open");
    assert_eq!(incident["incident"]["causeStatus"], "hypothesis");
    let causes = incident["causes"].as_array().unwrap();
    assert_eq!(causes[0]["kind"], "release", "{causes:?}");
    assert!(
        causes
            .iter()
            .any(|cause| cause["kind"] == "error" && cause["refId"] == "fp-shop"),
        "{causes:?}"
    );
    assert!(causes.iter().any(|cause| cause["kind"] == "change"), "{causes:?}");
    let kinds: Vec<&str> = incident["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| row["kind"].as_str())
        .collect();
    for kind in ["regression", "release", "config", "error"] {
        assert!(kinds.contains(&kind), "{kind} in {kinds:?}");
    }
    let cart_incident = call(
        "org.moqui.monitor.MonitorServices.get#Incident",
        json!({"incidentId": cart["incidentId"]}),
    );
    assert_eq!(cart_incident["causes"][0]["kind"], "instance", "{cart_incident}");
    let billing_incident = call(
        "org.moqui.monitor.MonitorServices.get#Incident",
        json!({"incidentId": billing["incidentId"]}),
    );
    assert!(
        billing_incident["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["kind"] == "missing"),
        "{billing_incident}"
    );

    // A second pass finds the same regressions: nothing new.
    let again = call("org.moqui.monitor.MonitorServices.detect#Regressions", json!({}));
    assert_eq!(
        (again["regressions"].as_i64(), again["incidents"].as_i64()),
        (Some(0), Some(0)),
        "{again}"
    );

    // The releases list shows 2.0 with its open regression.
    let releases = call(
        "org.moqui.monitor.MonitorServices.list#Releases",
        json!({"serviceName":"shop"}),
    )["releases"]
        .clone();
    assert_eq!(releases[0]["version"], "2.0", "{releases}");
    assert_eq!(releases[0]["previousVersion"], "1.0");
    assert_eq!(releases[0]["openRegressions"], 1);
    let detail = call(
        "org.moqui.monitor.MonitorServices.get#Release",
        json!({"serviceName":"shop","version":"2.0"}),
    );
    assert_eq!(detail["newErrors"][0]["fingerprint"], "fp-shop", "{detail}");

    // cart's failing instance recovers: half an hour on, its regression resolves and the incident is watched, then
    // resolved after resolve-minutes.
    let later = now + 31 * MINUTE;
    let mut healthy = Vec::new();
    for step in 0..31 {
        let at = later - (step + 1) * MINUTE;
        healthy.extend(minute("cart", "1.0", "cart-c", at, 200, 0));
        healthy.extend(minute("cart", "1.0", "cart-d", at, 200, 0));
        let day_before = at - 24 * 60 * MINUTE;
        healthy.extend(minute("cart", "1.0", "cart-c", day_before, 199, 1));
        healthy.extend(minute("cart", "1.0", "cart-d", day_before, 199, 1));
    }
    search.index_entries("otel_metrics", healthy).unwrap();
    context.set_effective_time(Some(later));
    let settled = call("org.moqui.monitor.MonitorServices.detect#Regressions", json!({}));
    assert!(settled["resolved"].as_i64().unwrap() >= 1, "{settled}");
    let status = |id: &Json| {
        call(
            "org.moqui.monitor.MonitorServices.get#Incident",
            json!({"incidentId": id}),
        )["incident"]["statusId"]
            .clone()
    };
    assert_eq!(status(&cart["incidentId"]), "Monitoring");
    context.set_effective_time(Some(later + 21 * MINUTE));
    call("org.moqui.monitor.MonitorServices.detect#Regressions", json!({}));
    assert_eq!(status(&cart["incidentId"]), "Resolved");

    // An operator confirms the shop release as the cause.
    let release_cause = causes[0]["causeSeqId"].clone();
    let updated = call(
        "org.moqui.monitor.MonitorServices.update#Incident",
        json!({"incidentId": shop["incidentId"], "confirmCauseSeqId": release_cause, "statusId": "Mitigating"}),
    );
    assert_eq!(updated["incident"]["causeStatus"], "confirmed");
    assert!(updated["incident"]["causeConfirmedDate"].is_string(), "{updated}");
    let _ = to_plain_string(&Value::Null);
}
