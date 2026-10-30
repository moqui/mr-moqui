//! The process edge of telemetry. The exporter never owns the factory or a business transaction.
use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::document::Document;
use moqui_core::telemetry::Exporter;
use moqui_lib::error::Error;
use moqui_lib::object::Value;

struct SearchExporter(Arc<moqui_core::search::SearchFacade>);

impl Exporter for SearchExporter {
    fn export(&self, index: &str, documents: Vec<Document>) -> Result<usize, Error> {
        self.0.index_entries_deferred_refresh(index, documents)
    }

    fn upsert_error(&self, document: Document) -> Result<(), Error> {
        let fingerprint = document
            .get("fingerprint")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::new("error document has no fingerprint"))?;
        self.0.upsert_counter("otel_errors", fingerprint, &document, "count")
    }
}

pub(crate) fn install(factory: &Arc<ExecutionContextFactory>) {
    if let Some(search) = moqui_core::search::installed(factory) {
        moqui_core::telemetry::for_factory(factory).set_exporter(moqui_core::telemetry::otlp::configured(
            factory,
            Arc::new(SearchExporter(search)),
        ));
        crate::logging::attach_telemetry(factory);
        moqui_core::telemetry::register_runtime_gauges(factory);
    }
}

#[cfg(all(feature = "axum", feature = "telemetry"))]
mod receiver;
#[cfg(all(feature = "axum", feature = "telemetry"))]
pub(crate) use receiver::routes;
#[cfg(feature = "grpc")]
pub(crate) mod grpc;

#[cfg(feature = "axum")]
/// The guard of the listeners now serving, which the guard gauges observe; a later serve replaces it.
#[derive(Default)]
struct ServingGuard {
    guard: std::sync::Mutex<std::sync::Weak<crate::guard::Guard>>,
    registered: std::sync::atomic::AtomicBool,
}

#[cfg(feature = "axum")]
/// The guard's `/metrics` values as observable gauges, `edge_denied` as `moqui.edge.denied` and `request_queue_full` as
/// `moqui.request.queue_full`, with the runtime gauges, so a scrape and the minute metric documents have them whether
/// or not search is installed.
pub(crate) fn register_guard_gauges(factory: &Arc<ExecutionContextFactory>, guard: &Arc<crate::guard::Guard>) {
    let slot = factory.extension(|_| ServingGuard::default());
    *moqui_lib::sync::lock(&slot.guard) = Arc::downgrade(guard);
    moqui_core::telemetry::register_runtime_gauges(factory);
    if !slot.registered.swap(true, std::sync::atomic::Ordering::AcqRel) {
        observe_guard(&moqui_core::telemetry::for_factory(factory), &slot, guard);
    }
}

#[cfg(feature = "axum")]
fn observe_guard(telemetry: &moqui_core::telemetry::Telemetry, slot: &Arc<ServingGuard>, guard: &crate::guard::Guard) {
    for (index, (name, _)) in guard.values().into_iter().enumerate() {
        let weak = Arc::downgrade(slot);
        telemetry.observe_gauge(&guard_gauge_name(name), move || {
            let guard = moqui_lib::sync::lock(&weak.upgrade()?.guard).upgrade()?;
            Some(guard.values()[index].1 as f64)
        });
    }
}

#[cfg(feature = "axum")]
/// `edge_denied` is `moqui.edge.denied`.
fn guard_gauge_name(name: &str) -> String {
    format!("moqui.{}", name.replacen('_', ".", 1))
}

/// A Prometheus scrape: `format=prometheus` in the query, or an Accept header that asks for the exposition format
/// (`text/plain;version=0.0.4` or OpenMetrics, as Prometheus sends). `format=text`, or any other Accept, is the
/// plain `name value` lines.
pub(crate) fn wants_prometheus(query: Option<&str>, accept: Option<&str>) -> bool {
    for pair in query.unwrap_or("").split('&') {
        match pair {
            "format=prometheus" => return true,
            "format=text" => return false,
            _ => {}
        }
    }
    accept.is_some_and(|accept| {
        accept.split(',').any(|item| {
            let item = item.trim().to_ascii_lowercase();
            item.starts_with("application/openmetrics-text")
                || (item.starts_with("text/plain") && item.contains("version="))
        })
    })
}

/// The scrape body: the SDK's instruments, then the host's totals that no instrument carries. Without the SDK (telemetry
/// off), `observed` are the gauges it would have had, under the same names.
pub(crate) fn prometheus_body(
    sdk: Option<String>,
    observed: &[(&str, f64)],
    totals: &[(&str, f64)],
    gauges: &[(&str, f64)],
) -> String {
    use std::fmt::Write as _;
    let mut text = sdk.unwrap_or_default();
    let observed = if text.is_empty() { observed } else { &[] };
    for (kind, values) in [("gauge", observed), ("counter", totals), ("gauge", gauges)] {
        for (name, value) in values {
            let _ = writeln!(text, "# TYPE {name} {kind}\n{name} {value}");
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scrape_is_asked_for_by_format_or_by_the_prometheus_accept_header() {
        for (query, accept, prometheus) in [
            (None, None, false),
            (None, Some("*/*"), false),
            (None, Some("text/plain"), false),
            (Some("format=prometheus"), None, true),
            (Some("a=1&format=text"), Some("text/plain;version=0.0.4"), false),
            (None, Some("text/plain; version=0.0.4; charset=utf-8"), true),
            (
                None,
                Some("application/openmetrics-text;version=1.0.0;q=0.5,text/plain;version=0.0.4;q=0.3,*/*;q=0.2"),
                true,
            ),
        ] {
            assert_eq!(wants_prometheus(query, accept), prometheus, "{query:?} {accept:?}");
        }
    }

    #[cfg(all(feature = "telemetry", feature = "axum"))]
    #[test]
    fn guard_values_are_gauges_and_the_scrape_adds_the_totals_no_instrument_has() {
        assert_eq!(guard_gauge_name("edge_denied"), "moqui.edge.denied");
        assert_eq!(guard_gauge_name("request_queue_full"), "moqui.request.queue_full");
        let telemetry = moqui_core::telemetry::Telemetry::from_conf(
            "<moqui-conf><telemetry/></moqui-conf>",
            moqui_core::telemetry::Resource::default(),
        );
        let guard = Arc::new(crate::guard::Guard::disabled());
        guard.counts.denied.fetch_add(3, std::sync::atomic::Ordering::Relaxed);
        let slot = Arc::new(ServingGuard::default());
        *moqui_lib::sync::lock(&slot.guard) = Arc::downgrade(&guard);
        observe_guard(&telemetry, &slot, &guard);
        let sdk = telemetry.prometheus_text();
        let text = prometheus_body(
            sdk,
            &[("moqui_edge_denied", 99.0)],
            &[("moqui_db_statements_total", 7.0)],
            &[("moqui_search_replica_lag_seconds", 0.5)],
        );
        for line in [
            "# TYPE moqui_edge_denied gauge",
            "moqui_edge_denied 3",
            "moqui_request_abandoned 0",
            "# TYPE moqui_db_statements_total counter",
            "moqui_db_statements_total 7",
            "moqui_search_replica_lag_seconds 0.5",
        ] {
            assert!(text.lines().any(|candidate| candidate == line), "{line}\n{text}");
        }
        assert!(
            !text.contains("moqui_edge_denied 99"),
            "with the SDK, observed values come from it"
        );
        // A guard that is gone is not observed; the totals stay.
        drop(guard);
        let text = prometheus_body(telemetry.prometheus_text(), &[], &[], &[]);
        assert!(!text.contains("moqui_edge_denied "), "{text}");
        telemetry.shutdown();
        let text = prometheus_body(None, &[("moqui_edge_denied", 1.0)], &[], &[]);
        assert_eq!(text, "# TYPE moqui_edge_denied gauge\nmoqui_edge_denied 1\n");
    }
}
