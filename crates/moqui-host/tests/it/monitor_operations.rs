//! Business operations on a real seeded PostgreSQL server: a service call that owns its transaction counts as
//! moqui.operation.runs by name and outcome, a joined call counts nothing and a force-new one counts its own run, a
//! scheduled job's failure is a retry and an unscheduled one's a rollback, and an async call is pending and then runs;
//! the service's span carries the name, outcome and correlation reference (plans/Monitoring.md, Later: Business
//! operations).
#![cfg(all(unix, feature = "embedded-postgres", feature = "axum", feature = "telemetry"))]
use crate::common;
use moqui_core::search::{Query, SearchRequest, Target};
use moqui_lib::object::{to_plain_string, Value};
use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

const SERVICES: &str = r#"<services xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
        xsi:noNamespaceSchemaLocation="http://moqui.org/xsd/service-definition-3.xsd">
    <service verb="place" noun="Order" authenticate="anonymous-all" operation="order.place" operation-ref="orderId">
        <in-parameters><parameter name="orderId" required="true"/></in-parameters>
        <actions><set field="placed" value="Y"/></actions>
    </service>
    <service verb="refuse" noun="Order" authenticate="anonymous-all">
        <in-parameters><parameter name="orderId"/></in-parameters>
        <actions><return error="true" message="Order refused"/></actions>
    </service>
    <service verb="outer" noun="Order" authenticate="anonymous-all">
        <in-parameters><parameter name="orderId" required="true"/></in-parameters>
        <actions>
            <service-call name="ops.fixture.OrderServices.place#Order" in-map="{orderId}"/>
            <service-call name="ops.fixture.OrderServices.audit#Order" in-map="{orderId}" transaction="force-new"/>
        </actions>
    </service>
    <service verb="audit" noun="Order" authenticate="anonymous-all">
        <in-parameters><parameter name="orderId"/></in-parameters>
        <actions><set field="audited" value="Y"/></actions>
    </service>
    <service verb="sweep" noun="Orders" authenticate="anonymous-all">
        <actions><return error="true" message="Sweep failed"/></actions>
    </service>
    <service verb="peek" noun="Order" authenticate="anonymous-all" transaction="ignore" operation="order.peek">
        <actions><set field="seen" value="Y"/></actions>
    </service>
</services>"#;

const DATA: &str = r#"<entity-facade-xml type="seed">
    <moqui.service.job.ServiceJob jobName="ops_sweep_scheduled" serviceName="ops.fixture.OrderServices.sweep#Orders"
            cronExpression="0 0 0 1 1 ? 2099" paused="Y"/>
    <moqui.service.job.ServiceJob jobName="ops_sweep_once" serviceName="ops.fixture.OrderServices.sweep#Orders"
            paused="Y"/>
</entity-facade-xml>"#;

/// The runs counted so far, by operation name and outcome.
fn runs(factory: &moqui_core::context::ExecutionContextFactory) -> HashMap<(String, String), i64> {
    moqui_core::telemetry::for_factory(factory).flush();
    let search = moqui_core::search::installed(factory).unwrap();
    let mut request = SearchRequest::new(Query::eq("name", Value::string("moqui.operation.runs")));
    request.limit = 500;
    let page = search.search(&Target::Index("otel_metrics".into()), &request).unwrap();
    let mut out = HashMap::new();
    for hit in &page.hits {
        let field = |name: &str| hit.body.get(name).map(to_plain_string).unwrap_or_default();
        let count: i64 = field("sum").parse::<f64>().map(|sum| sum as i64).unwrap_or(0);
        *out.entry((field("moqui.operation.name"), field("outcome")))
            .or_insert(0) += count;
    }
    out
}

fn count(runs: &HashMap<(String, String), i64>, name: &str, outcome: &str) -> i64 {
    runs.get(&(name.to_string(), outcome.to_string())).copied().unwrap_or(0)
}

/// Wait for a worker's runs to be counted.
fn runs_until(
    factory: &moqui_core::context::ExecutionContextFactory,
    done: impl Fn(&HashMap<(String, String), i64>) -> bool,
) -> HashMap<(String, String), i64> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let seen = runs(factory);
        if done(&seen) || Instant::now() > deadline {
            return seen;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn call(service: &str, order: &str) {
    let mut parameters = BTreeMap::new();
    parameters.insert("orderId".to_string(), Value::string(order));
    let _ = moqui_core::service::sync(service, &parameters);
    moqui_core::context::ExecutionContext::with_current(|ctx| ctx.message().clear_errors());
}

#[test]
fn operations_count_their_outcomes_from_the_transaction() {
    if !common::own_process() {
        return;
    }
    let gate = common::start_with_files(
        "monitor-operations",
        Some(DATA),
        &[],
        &[
            (
                "component/ops-fixture/component.xml",
                r#"<component name="ops-fixture"/>"#,
            ),
            (
                "component/ops-fixture/MoquiConf.xml",
                r#"<moqui-conf><telemetry sample-ratio="1"/></moqui-conf>"#,
            ),
            ("component/ops-fixture/service/ops/fixture/OrderServices.xml", SERVICES),
        ],
    );
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();

    call("ops.fixture.OrderServices.place#Order", "OPS-1");
    call("ops.fixture.OrderServices.refuse#Order", "OPS-2");
    // The outer call owns the transaction place#Order joins; audit#Order begins its own.
    call("ops.fixture.OrderServices.outer#Order", "OPS-3");
    // A service that ignores transactions owns none, so it is no operation even when declared.
    call("ops.fixture.OrderServices.peek#Order", "OPS-4");
    let seen = runs(factory);
    assert_eq!(count(&seen, "order.place", "commit"), 1, "{seen:?}");
    assert_eq!(
        count(&seen, "ops.fixture.OrderServices.refuse#Order", "rollback"),
        1,
        "{seen:?}"
    );
    assert_eq!(
        count(&seen, "ops.fixture.OrderServices.outer#Order", "commit"),
        1,
        "{seen:?}"
    );
    assert_eq!(
        count(&seen, "ops.fixture.OrderServices.audit#Order", "commit"),
        1,
        "{seen:?}"
    );
    assert!(!seen.keys().any(|(name, _)| name == "order.peek"), "{seen:?}");
    // The framework's own services are left out by the default conf.
    assert!(!seen.keys().any(|(name, _)| name.starts_with("org.moqui.")), "{seen:?}");

    // A scheduled job that fails will run again: retry. One with no schedule will not: rollback.
    {
        let _authz = context.disable_authz();
        moqui_core::service::run_job_now("ops_sweep_scheduled", &BTreeMap::new()).unwrap();
        moqui_core::service::run_job_now("ops_sweep_once", &BTreeMap::new()).unwrap();
    }
    let sweep = "ops.fixture.OrderServices.sweep#Orders";
    let seen = runs_until(factory, |seen| {
        count(seen, sweep, "retry") + count(seen, sweep, "rollback") >= 2
    });
    assert_eq!(count(&seen, sweep, "retry"), 1, "{seen:?}");
    assert_eq!(count(&seen, sweep, "rollback"), 1, "{seen:?}");

    // A queued call is pending, then its worker's run commits.
    let mut parameters = BTreeMap::new();
    parameters.insert("orderId".to_string(), Value::string("OPS-5"));
    moqui_core::service::call::async_call("ops.fixture.OrderServices.place#Order", &parameters).unwrap();
    let seen = runs_until(factory, |seen| count(seen, "order.place", "commit") >= 2);
    assert_eq!(count(&seen, "order.place", "pending"), 1, "{seen:?}");
    assert_eq!(count(&seen, "order.place", "commit"), 2, "{seen:?}");

    // The shipped dashboard reads the same counts.
    let definition: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../runtime/base-component/tools/report/monitor-operations.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let frames =
        moqui_report::dashboard_frames(&context, &definition, &serde_json::json!({}), "now-1h", "now+1h").unwrap();
    for (panel, frame) in frames["panels"].as_object().unwrap() {
        assert_eq!(frame["notices"], serde_json::json!([]), "{panel}: {frame}");
    }
    let summary = frames["panels"]["summary"].to_string();
    assert!(
        summary.contains("order.place") && summary.contains("rollbacks"),
        "{summary}"
    );

    // The span of each run says which operation it was, how it ended, and what it was about.
    let search = moqui_core::search::installed(factory).unwrap();
    let mut request = SearchRequest::new(Query::eq("moqui.operation.name", Value::string("order.place")));
    request.limit = 10;
    let spans = search.search(&Target::Index("otel_spans".into()), &request).unwrap();
    let references: Vec<String> = spans
        .hits
        .iter()
        .filter(|hit| hit.body.get("moqui.operation.outcome").and_then(Value::as_str) == Some("commit"))
        .filter_map(|hit| hit.body.get("moqui.operation.ref").map(to_plain_string))
        .collect();
    assert!(references.contains(&"orderId:OPS-1".to_string()), "{references:?}");
    let mut request = SearchRequest::new(Query::eq(
        "moqui.operation.name",
        Value::string("ops.fixture.OrderServices.refuse#Order"),
    ));
    request.limit = 10;
    let refused = search.search(&Target::Index("otel_spans".into()), &request).unwrap();
    assert_eq!(
        refused
            .hits
            .first()
            .and_then(|hit| hit.body.get("moqui.operation.outcome"))
            .and_then(Value::as_str),
        Some("rollback")
    );
}
