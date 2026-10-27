use super::*;

#[test]
fn w3c_context_validates_and_restores() {
    assert!(TraceContext::extract("00-00000000000000000000000000000000-0123456789abcdef-01", "").is_none());
    assert!(TraceContext::extract("00-0123456789abcdef0123456789abcdef-0000000000000000-01", "").is_none());
    let context = TraceContext::extract(
        "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01",
        "vendor=state",
    )
    .unwrap();
    let old = capture();
    {
        let _guard = attach(Some(context.clone()));
        assert_eq!(capture(), Some(context.clone()));
    }
    assert_eq!(capture(), old);
    assert_eq!(
        TraceContext::extract(&context.traceparent(), context.tracestate()),
        Some(context)
    );
}

#[test]
fn fingerprint_ignores_parameters_and_line_shifts() {
    assert_eq!(
        error_fingerprint("QueryError", "order 123 failed", "service(foo.js:12:2)"),
        error_fingerprint("QueryError", "order 988 failed", "service(foo.js:99:5)")
    );
    assert_ne!(
        error_fingerprint("QueryError", "order 123 failed", "service(foo.js:12:2)"),
        error_fingerprint("AccessError", "order 123 failed", "service(foo.js:12:2)")
    );
}

#[test]
fn error_groups_keep_source_and_service_boundaries_but_merge_instances() {
    let error = |source: &str, service: &str, instance: &str| {
        error_document(&Document::from_iter([
            ("source.name".into(), Value::string(source)),
            ("service.name".into(), Value::string(service)),
            ("service.instance.id".into(), Value::string(instance)),
            ("exception.type".into(), Value::string("QueryError")),
            ("exception.message".into(), Value::string("order 42 failed")),
        ]))
    };
    assert_eq!(
        text(&error("a", "service", "one"), "fingerprint"),
        text(&error("a", "service", "two"), "fingerprint")
    );
    assert_ne!(
        text(&error("a", "service", "one"), "fingerprint"),
        text(&error("b", "service", "one"), "fingerprint")
    );
    assert_ne!(
        text(&error("a", "service", "one"), "fingerprint"),
        text(&error("a", "other", "one"), "fingerprint")
    );
}

#[test]
fn sql_literals_are_redacted() {
    assert_eq!(
        redact_sql("SELECT * FROM x WHERE name='secret''value' AND amount=123.4"),
        "SELECT * FROM x WHERE name=? AND amount=?"
    );
    assert!(!redact_sql("SELECT $tag$private secret$tag$, $1 -- credentials\n /* hidden */ 1").contains("secret"));
    assert!(redact_sql("SELECT $tag$private secret$tag$, $1 -- credentials\n /* hidden */ 1").contains("$1"));
    assert!(!redact_sql("SELECT 1 -- private secret").contains("secret"));
    assert!(!redact_text("Authorization: Bearer password123 password='another secret'").contains("password123"));
    assert!(!redact_text("Authorization: Bearer password123 password='another secret'").contains("another secret"));
    assert!(!sensitive_key("gen_ai.usage.input_tokens"));
    assert!(sensitive_key("access_token"));
}

#[test]
fn legacy_configuration_and_slow_threshold() {
    let config = Config::from_conf(
        r#"<moqui-conf><telemetry sample-ratio="0.2"><artifact type="AT_SERVICE" spans="true" metrics="true"/></telemetry><server-stats><artifact-stats type="AT_SERVICE" persist-hit="false" persist-bin="true"/></server-stats></moqui-conf>"#,
    );
    assert_eq!(config.sample_ratio, 0.2);
    assert_eq!(config.artifact_types["AT_SERVICE"], (true, true));
    let mut stats = RunningStats::default();
    for _ in 0..50 {
        assert!(!stats.record(0.1));
    }
    assert!(stats.record(1.1));
}

#[test]
fn configuration_expands_telemetry_properties() {
    moqui_lib::env::set_property("monitoring_test_telemetry_disabled", "false");
    moqui_lib::env::set_property("monitoring_test_telemetry_ratio", "0.25");
    let config = Config::from_conf(
        r#"<moqui-conf><telemetry enabled="${monitoring_test_telemetry_disabled}" sample-ratio="${monitoring_test_telemetry_ratio}"/></moqui-conf>"#,
    );
    moqui_lib::env::clear_property("monitoring_test_telemetry_disabled");
    moqui_lib::env::clear_property("monitoring_test_telemetry_ratio");
    assert!(!config.enabled);
    assert_eq!(config.sample_ratio, 0.25);
}

#[test]
fn json_ingest_keeps_valid_records_and_redacts_attributes() {
    let input = serde_json::json!({"resourceSpans":[{"resource":{"attributes":[{"key":"service.name","value":{"stringValue":"demo"}},{"key":"authorization","value":{"stringValue":"secret"}}]},"scopeSpans":[{"spans":[
        {"traceId":"0123456789abcdef0123456789abcdef","spanId":"0123456789abcdef","name":"demo","kind":2,"startTimeUnixNano":"1000000000","endTimeUnixNano":"1100000000","attributes":[{"key":"password","value":{"stringValue":"hidden"}},{"key":"safe","value":{"stringValue":"yes"}}]},
        {"traceId":"invalid","spanId":"invalid"}
    ]}]}]});
    let result = ingest::Decoder::default()
        .decode(
            ingest::Signal::Traces,
            "application/json",
            &serde_json::to_vec(&input).unwrap(),
            "authenticated",
        )
        .unwrap();
    assert_eq!(result.documents.len(), 1);
    assert_eq!(result.rejected, 1);
    let record = &result.documents[0];
    assert_eq!(text(record, "service.name"), "demo");
    assert_eq!(text(record, "source.name"), "authenticated");
    let Value::Map(attributes) = &record["attributes"] else {
        panic!("expected attributes")
    };
    assert_eq!(text(attributes, "safe"), "yes");
    assert!(!record.contains_key("password"));
    assert!(!record.contains_key("authorization"));
    assert_eq!(number(record, "duration_nano"), 100_000_000.0);
}

#[test]
fn cumulative_ingest_handles_resets_and_out_of_order_points() {
    let decoder = ingest::Decoder::default();
    let request = |start: &str, time: &str, value: i64| {
        serde_json::to_vec(&serde_json::json!({"resourceMetrics":[{"scopeMetrics":[{"metrics":[{"name":"jobs","sum":{"aggregationTemporality":2,"dataPoints":[{"startTimeUnixNano":start,"timeUnixNano":time,"asInt":value.to_string()}]}}]}]}]})).unwrap()
    };
    let first = decoder
        .decode(
            ingest::Signal::Metrics,
            "application/json",
            &request("1", "1000000000", 10),
            "source",
        )
        .unwrap();
    let second = decoder
        .decode(
            ingest::Signal::Metrics,
            "application/json",
            &request("1", "2000000000", 15),
            "source",
        )
        .unwrap();
    assert_eq!(number(&first.documents[0], "sum"), 10.0);
    assert_eq!(number(&second.documents[0], "sum"), 5.0);
    let reset = decoder
        .decode(
            ingest::Signal::Metrics,
            "application/json",
            &request("2", "3000000000", 2),
            "source",
        )
        .unwrap();
    assert_eq!(number(&reset.documents[0], "sum"), 2.0);
    let late = decoder
        .decode(
            ingest::Signal::Metrics,
            "application/json",
            &request("2", "2000000000", 3),
            "source",
        )
        .unwrap();
    assert!(late.documents.is_empty());
    assert_eq!(late.rejected, 1);
}

#[test]
fn old_cumulative_first_points_establish_a_visible_baseline() {
    let decoder = ingest::Decoder::default();
    let request = |time: &str, value: i64| {
        serde_json::to_vec(&serde_json::json!({"resourceMetrics":[{"scopeMetrics":[{"metrics":[{"name":"old-counter","sum":{"aggregationTemporality":2,"dataPoints":[{"startTimeUnixNano":"1","timeUnixNano":time,"asInt":value.to_string()}]}}]}]}]})).unwrap()
    };
    let first = decoder
        .decode(
            ingest::Signal::Metrics,
            "application/json",
            &request("120000000000", 1000),
            "source",
        )
        .unwrap();
    assert!(first.documents.is_empty());
    assert_eq!(first.rejected, 1);
    assert!(first.message.contains("baseline"));
    let next = decoder
        .decode(
            ingest::Signal::Metrics,
            "application/json",
            &request("121000000000", 1005),
            "source",
        )
        .unwrap();
    assert_eq!(next.documents.len(), 1);
    assert_eq!(number(&next.documents[0], "sum"), 5.0);
}

#[test]
fn cumulative_histogram_extremes_are_only_kept_for_fresh_intervals() {
    let decoder = ingest::Decoder::default();
    let request = |start: &str, time: &str, count: u64, sum: f64, min: f64, max: f64| {
        serde_json::to_vec(&serde_json::json!({"resourceMetrics":[{"scopeMetrics":[{"metrics":[{"name":"hist","histogram":{"aggregationTemporality":2,"dataPoints":[{"startTimeUnixNano":start,"timeUnixNano":time,"count":count.to_string(),"sum":sum,"min":min,"max":max,"explicitBounds":[10],"bucketCounts":[count.to_string(),"0"]}]}}]}]}]})).unwrap()
    };
    let first = decoder
        .decode(
            ingest::Signal::Metrics,
            "application/json",
            &request("1", "1000000000", 2, 4.0, 1.0, 3.0),
            "source",
        )
        .unwrap();
    assert_eq!(number(&first.documents[0], "min"), 1.0);
    let next = decoder
        .decode(
            ingest::Signal::Metrics,
            "application/json",
            &request("1", "2000000000", 3, 8.0, 1.0, 4.0),
            "source",
        )
        .unwrap();
    assert!(!next.documents[0].contains_key("min"));
    assert!(!next.documents[0].contains_key("max"));
    let reset = decoder
        .decode(
            ingest::Signal::Metrics,
            "application/json",
            &request("2", "3000000000", 1, 5.0, 5.0, 5.0),
            "source",
        )
        .unwrap();
    assert_eq!(number(&reset.documents[0], "min"), 5.0);
}

#[test]
fn histogram_rollups_preserve_totals_and_ids_on_retry() {
    let point = |start: i64, sum: f64, buckets: Vec<i64>| {
        Document::from_iter([
            ("series_fingerprint".into(), Value::string("series")),
            ("bucket_start".into(), Value::I64(start)),
            ("count".into(), Value::I64(2)),
            ("sum".into(), Value::F64(sum)),
            ("min".into(), Value::F64(sum / 4.0)),
            ("max".into(), Value::F64(sum / 2.0)),
            ("bounds".into(), Value::List(vec![Value::F64(0.1), Value::F64(1.0)])),
            (
                "buckets".into(),
                Value::List(buckets.into_iter().map(Value::I64).collect()),
            ),
        ])
    };
    let input = vec![point(60_000, 0.5, vec![1, 1, 0]), point(120_000, 1.0, vec![0, 1, 1])];
    let result = maintenance::rollup_documents(input.clone(), "hour", 3_600_000).unwrap();
    assert_eq!(
        result,
        maintenance::rollup_documents(input.clone(), "hour", 3_600_000).unwrap()
    );
    assert_eq!(result.len(), 1);
    assert_eq!(text(&result[0], "_id"), "series:hour:0");
    assert_eq!(number(&result[0], "count"), 4.0);
    assert_eq!(number(&result[0], "sum"), 1.5);
    let (bounds, counts) = maintenance::merge_histogram_buckets(&input).unwrap();
    assert_eq!(counts, vec![1, 2, 1]);
    assert_eq!(maintenance::histogram_percentile(&bounds, &counts, 0.5), Some(0.55));
    let mut incompatible = input.clone();
    incompatible[1].insert("bounds".into(), Value::List(vec![Value::F64(0.2), Value::F64(1.0)]));
    assert!(maintenance::merge_histogram_buckets(&incompatible).is_err());
    assert!(maintenance::rollup_documents(incompatible, "hour", 3_600_000).is_err());
}

#[cfg(feature = "telemetry")]
mod enabled {
    use super::*;

    #[derive(Default)]
    struct Memory(Mutex<Vec<(String, Document)>>);
    impl Exporter for Memory {
        fn export(&self, index: &str, documents: Vec<Document>) -> Result<usize, Error> {
            let count = documents.len();
            lock(&self.0).extend(documents.into_iter().map(|document| (index.to_string(), document)));
            Ok(count)
        }
    }

    fn telemetry(sample_ratio: f64) -> (Telemetry, Arc<Memory>) {
        let telemetry = Telemetry::from_conf(
            &format!("<moqui-conf><telemetry sample-ratio=\"{sample_ratio}\"/></moqui-conf>"),
            Resource::default(),
        );
        let memory = Arc::new(Memory::default());
        telemetry.set_exporter(memory.clone());
        (telemetry, memory)
    }

    #[test]
    fn sdk_tree_log_correlation_and_delta_metrics() {
        let (telemetry, memory) = telemetry(1.0);
        let root = telemetry.start_span("request", SpanKind::Server, None);
        let context = root.context().unwrap();
        {
            let _guard = attach(Some(context.clone()));
            let child = telemetry.start_span("service", SpanKind::Internal, None);
            assert_eq!(child.context().unwrap().trace_id, context.trace_id);
            {
                let _guard = attach(child.context());
                telemetry.record_log(LogRecord::new(17, "ERROR", "correlated"));
            }
            child.finish();
        }
        root.finish();
        telemetry.record_duration("http.server.request.duration", 0.1, Document::new());
        telemetry.record_duration("http.server.request.duration", 0.2, Document::new());
        telemetry.flush();
        let first = lock(&memory.0).clone();
        let spans: Vec<_> = first.iter().filter(|(index, _)| index == "otel_spans").collect();
        assert_eq!(spans.len(), 2);
        let child = spans
            .iter()
            .find(|(_, document)| text(document, "name") == "service")
            .unwrap();
        assert_eq!(text(&child.1, "parent_span_id"), context.span_id);
        assert!(
            first
                .iter()
                .any(|(index, document)| index == "otel_logs" && text(document, "trace_id") == context.trace_id)
        );
        let metrics: Vec<_> = first.iter().filter(|(index, _)| index == "otel_metrics").collect();
        assert_eq!(metrics.len(), 1, "{first:?}");
        assert_eq!(number(&metrics[0].1, "count"), 2.0);
        assert!((number(&metrics[0].1, "sum") - 0.3).abs() < 0.000001);
        telemetry.flush();
        assert_eq!(
            lock(&memory.0)
                .iter()
                .filter(|(index, _)| index == "otel_metrics")
                .count(),
            1
        );
    }

    #[test]
    fn unsampled_request_is_retained_and_error_promotes_children() {
        let (telemetry, memory) = telemetry(0.0);
        let root = telemetry.start_span("normal", SpanKind::Server, None);
        let child = telemetry.start_span("discarded", SpanKind::Internal, root.context());
        child.finish();
        root.finish();
        let root = telemetry.start_span("failed", SpanKind::Server, None);
        let mut child = telemetry.start_span("error", SpanKind::Internal, root.context());
        child.set_error("failure 42");
        child.finish();
        root.finish();
        telemetry.flush();
        let entries = lock(&memory.0);
        let spans: Vec<_> = entries
            .iter()
            .filter(|(index, _)| index == "otel_spans")
            .map(|(_, document)| text(document, "name"))
            .collect();
        assert_eq!(spans.len(), 3);
        assert!(!spans.contains(&"discarded".to_string()));
        assert!(spans.contains(&"error".to_string()));
        assert_eq!(entries.iter().filter(|(index, _)| index == "otel_errors").count(), 1);
    }

    #[test]
    fn failed_export_does_not_fail_recording() {
        struct Failing;
        impl Exporter for Failing {
            fn export(&self, _: &str, _: Vec<Document>) -> Result<usize, Error> {
                Err(Error::new("offline"))
            }
        }
        let telemetry = Telemetry::from_conf("<moqui-conf/>", Resource::default());
        telemetry.set_exporter(Arc::new(Failing));
        telemetry
            .start_span("business succeeded", SpanKind::Server, None)
            .finish();
        telemetry.flush();
        assert!(telemetry.counters().failed > 0);
        assert!(telemetry.counters().dropped > 0);
    }

    #[test]
    fn suppressed_artifact_and_descendants_emit_nothing_but_request_survives() {
        let (telemetry, memory) = telemetry(1.0);
        let request = telemetry.start_span("request", SpanKind::Server, None);
        let mut artifact = telemetry.start_span("private", SpanKind::Internal, request.context());
        artifact.suppress();
        {
            let _suppressed = suppress();
            telemetry
                .start_span("child", SpanKind::Internal, artifact.context())
                .finish();
            telemetry.record_duration("moqui.artifact.duration", 0.1, Document::new());
            telemetry.record_log(LogRecord::new(9, "INFO", "suppressed"));
        }
        artifact.finish();
        request.finish();
        telemetry.flush();
        let documents = lock(&memory.0);
        assert_eq!(documents.len(), 1, "{documents:?}");
        assert_eq!(text(&documents[0].1, "name"), "request");
    }

    #[test]
    fn disabled_configuration_starts_no_workers_or_providers() {
        let telemetry = Telemetry::from_conf(
            "<moqui-conf><telemetry enabled=\"false\"/></moqui-conf>",
            Resource::default(),
        );
        assert!(telemetry.sdk.is_none());
        assert!(lock(&telemetry.worker).is_none());
        assert!(
            telemetry
                .start_span("disabled", SpanKind::Server, None)
                .context()
                .is_none()
        );
        telemetry.flush();
        assert_eq!(telemetry.counters().queued, 0);
    }

    #[test]
    fn binding_the_cluster_identity_updates_all_signal_resources() {
        let (telemetry, memory) = telemetry(1.0);
        telemetry.set_instance_id("cluster-instance".to_string());
        telemetry.start_span("identified", SpanKind::Server, None).finish();
        telemetry.record_log(LogRecord::new(9, "INFO", "identified"));
        telemetry.record_duration("http.server.request.duration", 0.1, Document::new());
        telemetry.flush();
        assert_eq!(telemetry.resource().instance_id, "cluster-instance");
        for (_, document) in lock(&memory.0).iter() {
            assert_eq!(text(document, "service.instance.id"), "cluster-instance");
            assert_eq!(document.get("moqui.identity.uncertain"), Some(&Value::Bool(false)));
        }
    }

    #[test]
    fn oversized_ingest_batches_are_rejected_without_entering_the_queue() {
        let (telemetry, _) = telemetry(1.0);
        assert!(
            telemetry
                .try_ingest("otel_logs", vec![Document::new(); MAX_QUEUE + 1])
                .is_err()
        );
        assert_eq!(telemetry.counters().queued, 0);
        assert_eq!(telemetry.counters().dropped, (MAX_QUEUE + 1) as u64);
        assert_eq!(telemetry.shared.queued_records.load(Ordering::Relaxed), 0);
        assert_eq!(telemetry.shared.queued_bytes.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn shutdown_exports_final_sdk_batches_without_an_explicit_flush() {
        let (telemetry, memory) = telemetry(1.0);
        telemetry.start_span("last request", SpanKind::Server, None).finish();
        telemetry.record_log(LogRecord::new(9, "INFO", "last log"));
        telemetry.record_duration("http.server.request.duration", 0.25, Document::new());
        telemetry.shutdown();
        let documents = lock(&memory.0);
        for index in ["otel_spans", "otel_logs", "otel_metrics"] {
            assert!(
                documents.iter().any(|(stored, _)| stored == index),
                "missing {index}: {documents:?}"
            );
        }
        assert!(lock(&telemetry.worker).is_none());
    }

    #[test]
    fn simultaneous_factory_access_shares_one_provider() {
        let context = ExecutionContextFactory::test_context();
        let factory = Arc::clone(context.factory());
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let factory = Arc::clone(&factory);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    for_factory(&factory)
                })
            })
            .collect();
        let providers: Vec<_> = threads.into_iter().map(|thread| thread.join().unwrap()).collect();
        assert!(providers.iter().all(|provider| Arc::ptr_eq(&providers[0], provider)));
    }

    #[cfg(feature = "llm")]
    #[test]
    fn gen_ai_spans_capture_usage_without_prompt_or_response_contents() {
        use crate::llm::{
            conf::{EndpointConf, ModelConf, ProtocolId, ReasoningLevel},
            provider::{self, CallOptions, Message, ModelRequest, fake},
            registry::Target,
            usage::{CallSource, Usage},
        };
        let context = ExecutionContextFactory::test_context_with_conf(
            "<moqui-conf><telemetry><artifact type=\"AT_LLM\" spans=\"true\" metrics=\"true\"/></telemetry></moqui-conf>",
        );
        let _entered = context.enter();
        let telemetry = for_factory(context.factory());
        let memory = Arc::new(Memory::default());
        telemetry.set_exporter(memory.clone());
        let id = "monitor-genai-metadata";
        fake::script(
            id,
            vec![fake::Reply::steps(vec![
                fake::Step::Text("private response text".into()),
                fake::Step::Usage(Usage {
                    input: 10,
                    output: 3,
                    ..Usage::default()
                }),
                fake::Step::Finish("stop".into()),
            ])],
        );
        let target = Target {
            endpoint: EndpointConf::new(id, ProtocolId::Fake, ""),
            model: ModelConf::new("monitor-model", id, "model-1"),
            reasoning: ReasoningLevel::None,
            feature: None,
            fell_back_from: None,
        };
        let request = ModelRequest {
            history: vec![Message::user("private prompt text")],
            ..ModelRequest::default()
        };
        let reply = provider::call(&target, &request, &mut CallOptions::new(CallSource::Test));
        assert_eq!(reply.usage.input, 10);
        telemetry.flush();
        let documents = lock(&memory.0);
        let span = documents
            .iter()
            .find(|(index, document)| index == "otel_spans" && text(document, "moqui.artifact.type") == "AT_LLM")
            .unwrap();
        assert_eq!(text(&span.1, "gen_ai.provider.name"), id);
        assert_eq!(number(&span.1, "gen_ai.usage.input_tokens"), 10.0);
        assert_eq!(number(&span.1, "gen_ai.usage.output_tokens"), 3.0);
        let contents = format!("{documents:?}");
        assert!(!contents.contains("private prompt text"));
        assert!(!contents.contains("private response text"));
        fake::clear(id);
    }

    #[test]
    fn suppressed_sdk_spans_do_not_exhaust_raw_queue_reservations() {
        let (telemetry, memory) = telemetry(0.0);
        for _ in 0..MAX_QUEUE + 32 {
            let mut span = telemetry.start_span("suppressed", SpanKind::Internal, None);
            span.suppress();
            span.finish();
        }
        telemetry
            .start_span("root after suppression", SpanKind::Server, None)
            .finish();
        telemetry.flush();
        assert_eq!(telemetry.shared.queued_records.load(Ordering::Relaxed), 0);
        assert_eq!(telemetry.shared.queued_bytes.load(Ordering::Relaxed), 0);
        assert_eq!(telemetry.counters().dropped, 0);
        let spans: Vec<_> = lock(&memory.0)
            .iter()
            .filter(|(index, _)| index == "otel_spans")
            .cloned()
            .collect();
        assert_eq!(spans.len(), 1);
        assert_eq!(text(&spans[0].1, "name"), "root after suppression");
    }

    #[test]
    fn moving_owned_attributes_preserves_credential_redaction() {
        let (telemetry, memory) = telemetry(1.0);
        let mut root = telemetry.start_span("privacy", SpanKind::Server, None);
        root.set_attribute("safe", Value::string("password=private-span-value"));
        root.finish();
        telemetry.record_duration(
            "privacy.duration",
            0.1,
            Document::from_iter([(
                "moqui.artifact.name".into(),
                Value::string("password=private-label-value"),
            )]),
        );
        telemetry.record_log(LogRecord::new(9, "INFO", "password=private-log-value"));
        telemetry.flush();
        let rendered = format!("{:?}", lock(&memory.0));
        for secret in ["private-span-value", "private-label-value", "private-log-value"] {
            assert!(!rendered.contains(secret), "{secret}");
        }
        assert!(rendered.contains("redacted"));
    }

    #[test]
    fn protobuf_and_json_have_the_same_span_shape() {
        use opentelemetry_proto::tonic::{
            collector::trace::v1::ExportTraceServiceRequest,
            trace::v1::{ResourceSpans, ScopeSpans, Span as ProtoSpan},
        };
        use prost::Message;
        let request = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                scope_spans: vec![ScopeSpans {
                    spans: vec![ProtoSpan {
                        trace_id: vec![1; 16],
                        span_id: vec![2; 8],
                        name: "protobuf".into(),
                        kind: 2,
                        start_time_unix_nano: 1_000_000_000,
                        end_time_unix_nano: 1_100_000_000,
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        let decoder = ingest::Decoder::default();
        let protobuf = decoder
            .decode(
                ingest::Signal::Traces,
                "application/x-protobuf",
                &request.encode_to_vec(),
                "source",
            )
            .unwrap();
        let json = decoder
            .decode(
                ingest::Signal::Traces,
                "application/json",
                &serde_json::to_vec(&request).unwrap(),
                "source",
            )
            .unwrap();
        assert_eq!(protobuf.documents, json.documents);
        assert_eq!(protobuf.documents.len(), 1);
    }
}

#[test]
fn malformed_metrics_are_partial_rejections_and_missing_statistics_stay_unknown() {
    let point = serde_json::json!({"timeUnixNano":"1000000000","asDouble":3.0});
    let mut bad_number = point.clone();
    bad_number["asDouble"] = serde_json::json!("NaN");
    let mut missing = point.clone();
    missing.as_object_mut().unwrap().remove("asDouble");
    let mut overflow = point.clone();
    overflow["timeUnixNano"] = serde_json::json!("18446744073709551615");
    let histogram =
        serde_json::json!({"timeUnixNano":"1000000000","count":"2","bucketCounts":["1","1"],"explicitBounds":[1]});
    let mut bad_count = histogram.clone();
    bad_count["count"] = serde_json::json!("-1");
    let mut mismatch = histogram.clone();
    mismatch["count"] = serde_json::json!("3");
    let mut bad_bound = histogram.clone();
    bad_bound["explicitBounds"] = serde_json::json!(["Infinity"]);
    let input = serde_json::json!({"resourceMetrics":[{"scopeMetrics":[{"metrics":[
        {"name":"valid-and-invalid","gauge":{"dataPoints":[point,bad_number,missing,overflow]}},
        {"name":"missing-histogram-sum","histogram":{"aggregationTemporality":1,"dataPoints":[histogram,bad_count,mismatch,bad_bound]}}
    ]}]}]});
    let result = ingest::Decoder::default()
        .decode(
            ingest::Signal::Metrics,
            "application/json",
            &serde_json::to_vec(&input).unwrap(),
            "source",
        )
        .unwrap();
    assert_eq!(result.documents.len(), 2);
    assert_eq!(result.rejected, 6);
    assert_eq!(number(&result.documents[0], "sum"), 3.0);
    assert!(!result.documents[1].contains_key("sum"));
    assert!(!result.documents[1].contains_key("min"));
    assert!(!result.documents[1].contains_key("max"));
}

#[test]
fn authenticated_source_ids_override_claims_and_isolate_counter_baselines() {
    let input = |time, value| {
        serde_json::to_vec(&serde_json::json!({"resourceMetrics":[{"resource":{"attributes":[{"key":"source.id","value":{"stringValue":"forged"}}]},"scopeMetrics":[{"metrics":[{"name":"scope-counter","sum":{"aggregationTemporality":2,"dataPoints":[{"startTimeUnixNano":"1","timeUnixNano":time,"asInt":value,"attributes":[{"key":"source.id","value":{"stringValue":"forged-again"}}]}]}}]}]}]})).unwrap()
    };
    let decoder = ingest::Decoder::default();
    let first = decoder
        .decode_scoped(
            ingest::Signal::Metrics,
            "application/json",
            &input("1000000000", 10),
            "key-a",
            "same-label",
        )
        .unwrap();
    let other = decoder
        .decode_scoped(
            ingest::Signal::Metrics,
            "application/json",
            &input("2000000000", 20),
            "key-b",
            "same-label",
        )
        .unwrap();
    assert_eq!(text(&first.documents[0], "source.id"), "key-a");
    assert_eq!(text(&other.documents[0], "source.id"), "key-b");
    assert_eq!(text(&other.documents[0], "source.name"), "same-label");
    assert_eq!(number(&other.documents[0], "sum"), 20.0);
    assert_ne!(
        text(&first.documents[0], "series_fingerprint"),
        text(&other.documents[0], "series_fingerprint")
    );
    for key in ["resource", "attributes"] {
        let Value::Map(values) = &other.documents[0][key] else {
            panic!("expected attribute map")
        };
        assert!(!values.contains_key("source.id"));
    }
}

#[test]
fn rollups_do_not_manufacture_missing_sum_or_interval_extremes() {
    let point = |bucket, extra: bool| {
        let mut document = Document::from_iter([
            ("series_fingerprint".into(), Value::string("missing-stat")),
            ("bucket_start".into(), Value::I64(bucket)),
            ("count".into(), Value::I64(1)),
            ("bounds".into(), Value::List(vec![Value::F64(1.0)])),
            ("buckets".into(), Value::List(vec![Value::I64(1), Value::I64(0)])),
        ]);
        if extra {
            for field in ["sum", "min", "max"] {
                document.insert(field.into(), Value::F64(1.0));
            }
        }
        document
    };
    let merged = maintenance::rollup_documents(vec![point(0, true), point(60_000, false)], "hour", 3_600_000).unwrap();
    assert_eq!(number(&merged[0], "count"), 2.0);
    for key in ["sum", "min", "max"] {
        assert!(!merged[0].contains_key(key));
    }
}

#[test]
fn arbitrary_attributes_remain_in_payload_without_colliding_with_typed_resource_columns() {
    let input = serde_json::json!({"resourceLogs":[{"resource":{"attributes":[{"key":"service.name","value":{"stringValue":"resource-service"}},{"key":"service.name.conflict","value":{"stringValue":"opaque"}}]},"scopeLogs":[{"logRecords":[{"timeUnixNano":"1000000000","body":{"stringValue":"fixture"},"attributes":[{"key":"service","value":{"stringValue":"logged-service"}},{"key":"safe","value":{"stringValue":"opaque"}}]}]}]}]});
    let decoded = ingest::Decoder::default()
        .decode(
            ingest::Signal::Logs,
            "application/json",
            &serde_json::to_vec(&input).unwrap(),
            "source",
        )
        .unwrap();
    let document = &decoded.documents[0];
    assert_eq!(text(document, "service.name"), "resource-service");
    assert!(!document.contains_key("service"));
    assert!(!document.contains_key("service.name.conflict"));
    let Value::Map(attributes) = &document["attributes"] else {
        panic!("attributes")
    };
    assert_eq!(text(attributes, "service"), "logged-service");
}

#[cfg(feature = "telemetry")]
#[test]
fn sdk_processors_expose_every_record_to_the_core_queue_and_account_for_overload() {
    struct Blocking {
        entered: std::sync::mpsc::SyncSender<()>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
        first: AtomicBool,
        records: AtomicU64,
    }
    impl Exporter for Blocking {
        fn export(&self, _: &str, documents: Vec<Document>) -> Result<usize, Error> {
            if !self.first.swap(true, Ordering::Relaxed) {
                self.entered.send(()).unwrap();
                lock(&self.release).recv_timeout(Duration::from_secs(20)).unwrap();
            }
            self.records.fetch_add(documents.len() as u64, Ordering::Relaxed);
            Ok(documents.len())
        }
    }
    let telemetry = Telemetry::from_conf(
        "<moqui-conf><telemetry sample-ratio=\"0\"/></moqui-conf>",
        Resource::default(),
    );
    let (entered, waiting) = std::sync::mpsc::sync_channel(1);
    let (release, blocked) = std::sync::mpsc::channel();
    let exporter = Arc::new(Blocking {
        entered,
        release: Mutex::new(blocked),
        first: AtomicBool::new(false),
        records: AtomicU64::new(0),
    });
    telemetry.set_exporter(exporter.clone());
    telemetry.start_span("first root", SpanKind::Server, None).finish();
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(telemetry.counters().queued, 1, "SDK completion enqueues synchronously");
    let started = Instant::now();
    for at in 0..MAX_QUEUE + 32 {
        if at % 2 == 0 {
            telemetry.start_span("burst root", SpanKind::Server, None).finish();
        } else {
            telemetry.record_log(LogRecord::new(9, "INFO", "burst log"));
        }
    }
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "transport must not block span/log producers"
    );
    let counters = telemetry.counters();
    assert!(counters.queued <= (MAX_QUEUE + 1) as u64);
    assert!(counters.dropped >= 32);
    assert_eq!(counters.queued + counters.dropped, (MAX_QUEUE + 33) as u64,
        "reserved root capacity can reject ordinary logs earlier; every SDK record is accounted for");
    release.send(()).unwrap();
    telemetry.shutdown();
    assert_eq!(exporter.records.load(Ordering::Relaxed), counters.queued);
    assert_eq!(telemetry.counters().dropped, counters.dropped);
}

fn admission_shared() -> (Shared, mpsc::Receiver<Command>) {
    let (sender, receiver) = mpsc::sync_channel(MAX_QUEUE);
    (
        Shared {
            sender,
            counters: Arc::default(),
            queued_records: AtomicUsize::new(0),
            queued_bytes: AtomicUsize::new(0),
            config: Config::from_conf("<moqui-conf><telemetry/></moqui-conf>"),
            resource: Mutex::new(Resource::default()),
            stopped: AtomicBool::new(false),
            drained: AtomicBool::new(false),
            warned_legacy: AtomicBool::new(false),
            gauges_registered: AtomicBool::new(false),
            slow: Mutex::default(),
        },
        receiver,
    )
}

fn admission_root(job: bool) -> Document {
    Document::from_iter(if job {
        vec![("moqui.job.root".into(), Value::Bool(true))]
    } else {
        vec![("kind".into(), Value::string("SERVER"))]
    })
}

#[test]
fn queue_reserves_capacity_for_local_roots_and_bounds_total_records_and_bytes() {
    let (shared, _receiver) = admission_shared();
    shared
        .try_enqueue(
            "otel_logs",
            vec![Document::new(); MAX_QUEUE - ROOT_RESERVED_RECORDS],
            true,
        )
        .unwrap();
    assert!(shared.try_enqueue("otel_logs", vec![Document::new()], true).is_err());
    assert!(
        shared
            .try_enqueue("otel_spans", vec![admission_root(false)], false)
            .is_err(),
        "received roots use ordinary capacity"
    );
    shared
        .try_enqueue("otel_spans", vec![admission_root(true); ROOT_RESERVED_RECORDS], true)
        .unwrap();
    assert_eq!(shared.queued_records.load(Ordering::Relaxed), MAX_QUEUE);
    assert!(shared
        .try_enqueue("otel_spans", vec![admission_root(false)], true)
        .is_err());
    assert_eq!(shared.counters.dropped.load(Ordering::Relaxed), 3);
    assert!(shared.queued_bytes.load(Ordering::Relaxed) <= MAX_BUFFER_BYTES);

    let (shared, _receiver) = admission_shared();
    shared
        .queued_bytes
        .store(MAX_BUFFER_BYTES - ROOT_RESERVED_BYTES, Ordering::Relaxed);
    assert!(shared.try_enqueue("otel_logs", vec![Document::new()], true).is_err());
    assert_eq!(
        shared.queued_records.load(Ordering::Relaxed),
        0,
        "failed byte reservation releases record capacity"
    );
    shared
        .try_enqueue("otel_spans", vec![admission_root(false)], true)
        .unwrap();
    shared.queued_bytes.store(MAX_BUFFER_BYTES, Ordering::Relaxed);
    assert!(shared
        .try_enqueue("otel_spans", vec![admission_root(false)], true)
        .is_err());
    assert_eq!(shared.queued_records.load(Ordering::Relaxed), 1);
}

#[test]
fn mixed_normalized_spans_preserve_completion_order_and_promoted_children_cannot_drop_root() {
    let (shared, receiver) = admission_shared();
    let child = Document::from_iter([("name".into(), Value::string("error child"))]);
    shared
        .try_enqueue(
            "otel_spans",
            vec![
                child.clone(),
                admission_root(false),
                child.clone(),
                admission_root(true),
            ],
            true,
        )
        .unwrap();
    for expected in [false, true, false, true] {
        let Command::Documents(_, documents, _, _, _) = receiver.try_recv().unwrap() else {
            panic!("documents")
        };
        assert_eq!(documents.len(), 1);
        assert_eq!(
            mandatory_root(&documents[0]),
            expected,
            "child evidence must precede root tail decision"
        );
    }
    let mut ready = vec![child.clone(); MAX_QUEUE];
    ready.push(admission_root(false));
    let (mut count, mut bytes) = (0, 0);
    let dropped = admit_ready(&mut ready, true, &mut count, &mut bytes);
    assert!(dropped > 0);
    assert_eq!(ready.iter().filter(|document| mandatory_root(document)).count(), 1);
    assert_eq!(ready.len(), count);
    assert!(count <= MAX_QUEUE && bytes <= MAX_BUFFER_BYTES);
    assert_eq!(dropped + ready.len(), MAX_QUEUE + 1);
    let mut ready = vec![child, admission_root(true)];
    let (mut count, mut bytes) = (0, MAX_BUFFER_BYTES - ROOT_RESERVED_BYTES);
    assert_eq!(admit_ready(&mut ready, true, &mut count, &mut bytes), 1);
    assert_eq!(ready.len(), 1);
    assert!(
        mandatory_root(&ready[0]),
        "byte-limited promotion retains root rather than rejecting the whole batch"
    );
}

#[cfg(feature = "telemetry")]
#[test]
fn raw_sdk_server_and_job_roots_use_reserve_without_reordering_error_evidence() {
    use opentelemetry::{
        trace::{SpanContext, SpanId, TraceFlags, TraceId, TraceState},
        KeyValue,
    };
    use opentelemetry_sdk::trace::SpanData;
    let span = |kind, job, name: &'static str| SpanData {
        span_context: SpanContext::new(
            TraceId::from_bytes([1; 16]),
            SpanId::from_bytes([2; 8]),
            TraceFlags::SAMPLED,
            false,
            TraceState::default(),
        ),
        parent_span_id: SpanId::INVALID,
        parent_span_is_remote: false,
        span_kind: kind,
        name: name.into(),
        start_time: SystemTime::UNIX_EPOCH,
        end_time: SystemTime::UNIX_EPOCH,
        attributes: if job {
            vec![KeyValue::new("moqui.job.root", true)]
        } else {
            Vec::new()
        },
        dropped_attributes_count: 0,
        events: Default::default(),
        links: Default::default(),
        status: opentelemetry::trace::Status::Unset,
        instrumentation_scope: Default::default(),
    };
    use opentelemetry::trace::SpanKind::{Internal, Server};
    assert_eq!(
        sdk::ExportBatch::Spans(vec![span(Server, false, "server")]).uniform_root_priority(),
        Some(true)
    );
    assert_eq!(
        sdk::ExportBatch::Spans(vec![span(Internal, true, "job")]).uniform_root_priority(),
        Some(true)
    );
    assert_eq!(
        sdk::ExportBatch::Spans(vec![span(Internal, false, "child")]).uniform_root_priority(),
        Some(false)
    );
    let mut string_job = span(Internal, false, "string job");
    string_job.attributes.push(KeyValue::new("moqui.job.root", "true"));
    assert_eq!(
        sdk::ExportBatch::Spans(vec![string_job]).uniform_root_priority(),
        Some(true)
    );
    let (shared, receiver) = admission_shared();
    shared.enqueue_sdk(sdk::ExportBatch::Spans(vec![
        span(Internal, false, "error-child"),
        span(Server, false, "server"),
        span(Internal, false, "later-child"),
        span(Internal, true, "job"),
    ]));
    for expected in ["error-child", "server", "later-child", "job"] {
        let Command::Sdk(sdk::ExportBatch::Spans(spans), _) = receiver.try_recv().unwrap() else {
            panic!("SDK spans")
        };
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, expected);
    }
    let (shared, _receiver) = admission_shared();
    shared
        .try_enqueue(
            "otel_logs",
            vec![Document::new(); MAX_QUEUE - ROOT_RESERVED_RECORDS],
            true,
        )
        .unwrap();
    shared.enqueue_sdk(sdk::ExportBatch::Spans(vec![
        span(Internal, false, "rejected child"),
        span(Server, false, "server"),
        span(Internal, true, "job"),
    ]));
    assert_eq!(shared.counters.dropped.load(Ordering::Relaxed), 1);
    assert_eq!(
        shared.queued_records.load(Ordering::Relaxed),
        MAX_QUEUE - ROOT_RESERVED_RECORDS + 2
    );
}

#[test]
fn histogram_percentiles_respect_nonpositive_first_bucket_bounds() {
    assert_eq!(
        maintenance::histogram_percentile(&[-2.0, 2.0], &[10, 0, 0], 0.5),
        Some(-2.0)
    );
    assert_eq!(
        maintenance::histogram_percentile(&[0.0, 2.0], &[10, 0, 0], 0.5),
        Some(0.0)
    );
    assert_eq!(maintenance::histogram_percentile(&[2.0], &[10, 0], 0.5), Some(1.0));
}

#[test]
fn histogram_merging_keeps_exact_large_counts_and_rejects_overflow() {
    let point = |buckets: Vec<Value>| {
        Document::from_iter([
            ("bounds".into(), Value::List(vec![Value::F64(1.0)])),
            ("buckets".into(), Value::List(buckets)),
        ])
    };
    let precise = 9_007_199_254_740_993_i64;
    let documents = vec![
        point(vec![Value::I64(precise), Value::I64(0)]),
        point(vec![Value::I64(1), Value::I64(0)]),
    ];
    assert_eq!(
        maintenance::merge_histogram_buckets(&documents).unwrap().1,
        vec![precise as u64 + 1, 0]
    );
    let overflow = vec![point(vec![Value::String(u64::MAX.to_string()), Value::I64(1)])];
    assert!(
        maintenance::merge_histogram_buckets(&overflow)
            .unwrap_err()
            .message()
            .contains("overflows")
    );
    let across = vec![point(vec![Value::I64(i64::MAX), Value::I64(0)]); 3];
    assert!(maintenance::merge_histogram_buckets(&across).is_err());
    for invalid in [Value::I64(-1), Value::F64(1.5), Value::String("not-a-count".into())] {
        assert!(maintenance::merge_histogram_buckets(&[point(vec![invalid, Value::I64(0)])]).is_err());
    }
    assert_eq!(maintenance::histogram_percentile(&[1.0], &[u64::MAX, 1], 0.5), None);
    assert_eq!(
        maintenance::histogram_percentile(&[-1e308, 1e308], &[0, 1, 0], 0.5),
        Some(0.0)
    );
}

#[test]
fn flush_barrier_waits_for_full_queue_capacity_and_bounds_its_deadline() {
    let (queue, receiver) = std::sync::mpsc::sync_channel(1);
    queue.send(super::Command::Stop).unwrap();
    assert!(!super::flush_barrier(&queue, std::time::Duration::from_millis(2)));
    let worker = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(matches!(receiver.recv().unwrap(), super::Command::Stop));
        let super::Command::Flush(reply) = receiver.recv().unwrap() else {
            panic!("expected flush");
        };
        reply.send(()).unwrap();
    });
    assert!(super::flush_barrier(&queue, std::time::Duration::from_secs(1)));
    worker.join().unwrap();
    assert!(!super::flush_barrier(&queue, std::time::Duration::from_millis(2)));
}
