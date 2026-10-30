//! The OTLP/gRPC receiver on a real seeded PostgreSQL server: each signal through the generated collector clients,
//! the ingest key, the body limit and partial success (plans/Monitoring.md, Ingest).
#![cfg(all(unix, feature = "embedded-postgres", feature = "grpc"))]
use crate::common;
use moqui_lib::object::Value;
use opentelemetry_proto::tonic::collector::logs::v1::{
    logs_service_client::LogsServiceClient, ExportLogsServiceRequest,
};
use opentelemetry_proto::tonic::collector::metrics::v1::{
    metrics_service_client::MetricsServiceClient, ExportMetricsServiceRequest,
};
use opentelemetry_proto::tonic::collector::trace::v1::{
    trace_service_client::TraceServiceClient, ExportTraceServiceRequest,
};
use opentelemetry_proto::tonic::common::v1::{any_value, AnyValue, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::{
    metric, number_data_point, Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics,
};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use std::time::Duration;

fn resource() -> Option<Resource> {
    Some(Resource {
        attributes: vec![KeyValue {
            key: "service.name".into(),
            value: Some(AnyValue {
                value: Some(any_value::Value::StringValue("grpc-fixture".into())),
            }),
            ..Default::default()
        }],
        ..Default::default()
    })
}

fn traces(spans: Vec<Span>) -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: resource(),
            scope_spans: vec![ScopeSpans {
                spans,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

fn span(name: &str, now_nanos: u64) -> Span {
    Span {
        trace_id: vec![0x4d; 16],
        span_id: vec![0x2b; 8],
        name: name.into(),
        kind: 2,
        start_time_unix_nano: now_nanos - 10_000_000,
        end_time_unix_nano: now_nanos,
        ..Default::default()
    }
}

fn authorized<T>(message: T, token: &str) -> tonic::Request<T> {
    let mut request = tonic::Request::new(message);
    request
        .metadata_mut()
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    request
}

#[test]
fn each_signal_over_grpc_with_the_ingest_key_limits_and_partial_success() {
    if !common::own_process() {
        return;
    }
    let gate = common::start_with_conf_properties(
        "monitor-grpc",
        None,
        r#"<telemetry sample-ratio="0"><receiver body-limit="4096" grpc-enabled="true" grpc-port="0"/></telemetry>"#,
        &[],
    );
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let key = moqui_core::service::sync(
        "org.moqui.monitor.MonitorServices.create#IngestKey",
        &[("source".to_string(), Value::string("grpc-fixture"))]
            .into_iter()
            .collect(),
    )
    .unwrap();
    let token = key.get("accessToken").and_then(Value::as_str).unwrap().to_owned();
    // The gRPC listener binds after the HTTP one reports ready.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let address = loop {
        if let Some(address) = gate.server.grpc_address() {
            break address;
        }
        assert!(std::time::Instant::now() < deadline, "the gRPC receiver never bound");
        std::thread::sleep(Duration::from_millis(20));
    };
    let endpoint = format!("http://{address}");
    let now = u64::try_from(jiff::Timestamp::now().as_nanosecond()).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut client = TraceServiceClient::connect(endpoint.clone()).await.unwrap();
        let refused = client
            .export(tonic::Request::new(traces(vec![span("grpc-fixture-span", now)])))
            .await
            .unwrap_err();
        assert_eq!(refused.code(), tonic::Code::Unauthenticated, "{refused}");
        let refused = client
            .export(authorized(
                traces(vec![span("grpc-fixture-span", now)]),
                "mqi_not_a_key",
            ))
            .await
            .unwrap_err();
        assert_eq!(refused.code(), tonic::Code::Unauthenticated, "{refused}");
        let mut invalid = span("grpc-invalid", now);
        invalid.trace_id = vec![0; 16];
        let answer = client
            .export(authorized(
                traces(vec![span("grpc-fixture-span", now), invalid]),
                &token,
            ))
            .await
            .unwrap()
            .into_inner();
        let partial = answer.partial_success.expect("one span was rejected");
        assert_eq!(partial.rejected_spans, 1);
        assert!(!partial.error_message.is_empty());
        let refused = client
            .export(authorized(traces(vec![span(&"x".repeat(5000), now)]), &token))
            .await
            .unwrap_err();
        assert_eq!(refused.code(), tonic::Code::OutOfRange, "over body-limit: {refused}");

        let mut logs = LogsServiceClient::connect(endpoint.clone()).await.unwrap();
        let answer = logs
            .export(authorized(
                ExportLogsServiceRequest {
                    resource_logs: vec![ResourceLogs {
                        resource: resource(),
                        scope_logs: vec![ScopeLogs {
                            log_records: vec![LogRecord {
                                time_unix_nano: now,
                                severity_number: 9,
                                body: Some(AnyValue {
                                    value: Some(any_value::Value::StringValue("grpc-fixture-log".into())),
                                }),
                                trace_id: vec![0x4d; 16],
                                ..Default::default()
                            }],
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                },
                &token,
            ))
            .await
            .unwrap()
            .into_inner();
        assert!(answer.partial_success.is_none(), "{answer:?}");

        let mut metrics = MetricsServiceClient::connect(endpoint.clone()).await.unwrap();
        let answer = metrics
            .export(authorized(
                ExportMetricsServiceRequest {
                    resource_metrics: vec![ResourceMetrics {
                        resource: resource(),
                        scope_metrics: vec![ScopeMetrics {
                            metrics: vec![Metric {
                                name: "grpc.fixture.gauge".into(),
                                data: Some(metric::Data::Gauge(Gauge {
                                    data_points: vec![NumberDataPoint {
                                        time_unix_nano: now,
                                        value: Some(number_data_point::Value::AsDouble(1.5)),
                                        ..Default::default()
                                    }],
                                })),
                                ..Default::default()
                            }],
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                },
                &token,
            ))
            .await
            .unwrap()
            .into_inner();
        assert!(answer.partial_success.is_none(), "{answer:?}");
    });
    moqui_core::telemetry::for_factory(factory).flush();
    let search = moqui_core::search::installed(factory).unwrap();
    for (index, field, value) in [
        ("otel_spans", "name", "grpc-fixture-span"),
        ("otel_logs", "trace_id", "4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d"),
        ("otel_metrics", "name", "grpc.fixture.gauge"),
    ] {
        let found = search
            .search(
                &moqui_core::search::Target::Index(index.into()),
                &moqui_core::search::SearchRequest::new(moqui_core::search::Query::eq(field, Value::string(value))),
            )
            .unwrap();
        let hit = found.hits.first().unwrap_or_else(|| panic!("{index} has no {value}"));
        assert_eq!(
            hit.body.get("service.name").and_then(Value::as_str),
            Some("grpc-fixture"),
            "{index}"
        );
        assert_eq!(
            hit.body.get("source.name").and_then(Value::as_str),
            Some("grpc-fixture"),
            "{index}"
        );
    }
}
