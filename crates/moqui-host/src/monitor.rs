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
pub(crate) fn routes(app: axum::Router<Arc<crate::http::HttpState>>) -> axum::Router<Arc<crate::http::HttpState>> {
    app.route("/v1/traces", axum::routing::post(ingest))
        .route("/v1/logs", axum::routing::post(ingest))
        .route("/v1/metrics", axum::routing::post(ingest))
        .route("/v1/annotations", axum::routing::post(ingest))
}

#[cfg(all(feature = "axum", feature = "telemetry"))]
async fn ingest(
    axum::extract::State(state): axum::extract::State<Arc<crate::http::HttpState>>,
    request: axum::extract::Request,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    use moqui_core::telemetry::ingest::{Decoder, Signal};
    if state.factory.conf_setting("telemetry/receiver", "enabled") == "false" {
        return (axum::http::StatusCode::NOT_FOUND, "not found").into_response();
    }
    let encoding = request
        .headers()
        .get("content-encoding")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("identity")
        .to_owned();
    if !matches!(encoding.as_str(), "identity" | "gzip") {
        return (
            axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported OTLP content encoding",
        )
            .into_response();
    }
    let is_annotation = request.uri().path() == "/v1/annotations";
    let signal = match request.uri().path() {
        "/v1/traces" => Signal::Traces,
        "/v1/logs" => Signal::Logs,
        _ => Signal::Metrics,
    };
    let content_type = request
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    if !matches!(
        content_type.split(';').next().unwrap_or("").trim(),
        "application/json" | "application/x-protobuf"
    ) {
        return (
            axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "OTLP requires application/json or application/x-protobuf",
        )
            .into_response();
    }
    let token = request
        .headers()
        .get("api_key")
        .and_then(|v| v.to_str().ok())
        .or_else(|| {
            request
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
        })
        .unwrap_or("")
        .to_owned();
    if token.is_empty() {
        return (axum::http::StatusCode::UNAUTHORIZED, "an ingest key is required").into_response();
    }
    let maximum = state
        .factory
        .conf_number::<usize>("telemetry/receiver", "concurrent-requests", 8)
        .clamp(1, 4096);
    let budget = state.factory.extension(|_| tokio::sync::Semaphore::new(maximum));
    let permit = match budget.try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            moqui_core::telemetry::for_factory(&state.factory).note_dropped(1);
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "telemetry receiver is busy",
            )
                .into_response();
        }
    };
    let auth_factory = Arc::clone(&state.factory);
    let auth_token = token.clone();
    let authenticated = tokio::task::spawn_blocking(move || {
        let _suppressed = moqui_core::telemetry::suppress();
        let ec = auth_factory.new_context();
        let _entered = ec.enter();
        let (key, _) = moqui_core::monitor::authenticate(&ec, &auth_token).map_err(authentication_error)?;
        let rate = auth_factory.extension(|_| RateLimit::default());
        let max = auth_factory.conf_number("telemetry/receiver", "requests-per-minute", 600_usize);
        if !rate.allow(&key, max) {
            moqui_core::telemetry::for_factory(&auth_factory).note_dropped(1);
            return Err((429, "ingest key rate limit exceeded".to_string()));
        }
        Ok(permit)
    })
    .await;
    let permit = match authenticated {
        Ok(Ok(permit)) => permit,
        Ok(Err((status, message))) => {
            return (
                axum::http::StatusCode::from_u16(status).unwrap_or(axum::http::StatusCode::UNAUTHORIZED),
                message,
            )
                .into_response();
        }
        Err(_) => {
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "ingest authentication worker stopped",
            )
                .into_response();
        }
    };
    let limit = state
        .factory
        .conf_number("telemetry/receiver", "body-limit", 4 * 1024 * 1024_usize);
    let body = match tokio::time::timeout(
        Duration::from_secs(30),
        axum::body::to_bytes(request.into_body(), limit),
    )
    .await
    {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => {
            moqui_core::telemetry::for_factory(&state.factory).note_dropped(1);
            return (
                axum::http::StatusCode::PAYLOAD_TOO_LARGE,
                "ingest body exceeds the configured limit",
            )
                .into_response();
        }
        Err(_) => {
            moqui_core::telemetry::for_factory(&state.factory).note_dropped(1);
            return (axum::http::StatusCode::REQUEST_TIMEOUT, "ingest body timed out").into_response();
        }
    };
    let factory = Arc::clone(&state.factory);
    let protobuf = content_type.starts_with("application/x-protobuf");
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let _suppressed = moqui_core::telemetry::suppress();
        let ec = factory.new_context();
        let _entered = ec.enter();
        let (key, source) = moqui_core::monitor::authenticate(&ec, &token).map_err(authentication_error)?;
        let body = if encoding == "gzip" {
            use std::io::Read;
            let mut bytes = Vec::new();
            flate2::read::MultiGzDecoder::new(body.as_ref())
                .take((limit as u64).saturating_add(1))
                .read_to_end(&mut bytes)
                .map_err(|_| {
                    moqui_core::telemetry::for_factory(&factory).note_dropped(1);
                    (400, "invalid gzip OTLP body".to_string())
                })?;
            if bytes.len() > limit {
                moqui_core::telemetry::for_factory(&factory).note_dropped(1);
                return Err((413, "decompressed ingest body exceeds the configured limit".into()));
            }
            bytes
        } else {
            body.to_vec()
        };
        if is_annotation {
            if !content_type.starts_with("application/json") {
                return Err((415, "annotations require JSON".into()));
            }
            let json: serde_json::Value =
                serde_json::from_slice(&body).map_err(|_| (400, "invalid annotation JSON".to_string()))?;
            let Value::Map(parameters) = moqui_lib::json::from_json(&json) else {
                return Err((400, "annotation requires an object".into()));
            };
            ec.shared_map()
                .insert("monitor.ingest.source".into(), Value::string(&source));
            let service = moqui_core::native::ServiceInfo {
                name: "org.moqui.report.ReportServices.create#Annotation",
                service_type: "rust",
                location: "moqui_core::report::create_annotation",
                method: "",
            };
            moqui_core::report::create_annotation(&ec, &service, &parameters.into_iter().collect())
                .map_err(|error| (400, error.to_string()))?;
            return Ok((0, String::new()));
        }
        let decoder = factory.extension(|_| Decoder::default());
        let decoded = decoder
            .decode_scoped(signal, &content_type, &body, &key, &source)
            .map_err(|e| {
                moqui_core::telemetry::for_factory(&factory).note_dropped(1);
                (400, e.to_string())
            })?;
        let telemetry = moqui_core::telemetry::for_factory(&factory);
        telemetry.note_dropped(decoded.rejected);
        telemetry
            .try_ingest(decoded.index, decoded.documents)
            .map_err(|_| (503, "telemetry queue is full".to_string()))?;
        if let Some(entity) = factory.entity() {
            let _authz = ec.disable_authz();
            if let Err(error) = entity.find("moqui.monitor.IngestKey").and_then(|find| {
                find.eq("keyId", Value::string(key))
                    .update_all([("lastSeen".into(), Value::I64(crate::logging::now_millis()))])
            }) {
                tracing::warn!(%error,"could not update ingest source last-seen time");
            }
        }
        Ok((decoded.rejected, decoded.message))
    })
    .await;
    match result {
        Ok(Ok((rejected, message))) => {
            if protobuf {
                (
                    [(axum::http::header::CONTENT_TYPE, "application/x-protobuf")],
                    partial_response(rejected, &message),
                )
                    .into_response()
            } else {
                let field = match signal {
                    Signal::Traces => "rejectedSpans",
                    Signal::Logs => "rejectedLogRecords",
                    Signal::Metrics => "rejectedDataPoints",
                };
                axum::Json(if rejected == 0 {
                    serde_json::json!({})
                } else {
                    serde_json::json!({"partialSuccess":{field:rejected.to_string(),"errorMessage":message}})
                })
                .into_response()
            }
        }
        Ok(Err((status, message))) => (
            axum::http::StatusCode::from_u16(status).unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
            message,
        )
            .into_response(),
        Err(_) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "ingest worker stopped").into_response(),
    }
}

#[cfg(all(feature = "axum", feature = "telemetry"))]
use std::time::Duration;
#[cfg(all(feature = "axum", feature = "telemetry"))]
fn authentication_error(error: Error) -> (u16, String) {
    if moqui_core::context::ErrorKind::of(&error) == moqui_core::context::ErrorKind::Unavailable {
        tracing::warn!(%error,"ingest authorization is unavailable");
        (503, "ingest authorization is unavailable".into())
    } else {
        (401, "ingest key is invalid, expired, revoked or unauthorized".into())
    }
}
#[cfg(all(feature = "axum", feature = "telemetry"))]
#[derive(Default)]
struct RateLimit(std::sync::Mutex<std::collections::HashMap<String, (i64, usize)>>);
#[cfg(all(feature = "axum", feature = "telemetry"))]
impl RateLimit {
    fn allow(&self, key: &str, max: usize) -> bool {
        let minute = crate::logging::now_millis() / 60000;
        let mut entries = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.retain(|_, (at, _)| *at == minute);
        if entries.len() >= 4096 && !entries.contains_key(key) {
            return false;
        }
        let entry = entries.entry(key.into()).or_insert((minute, 0));
        entry.1 = entry.1.saturating_add(1);
        entry.1 <= max
    }
}
/// OTLP Export*ServiceResponse has one nested partial_success message: count=1, error_message=2.
#[cfg(all(feature = "axum", feature = "telemetry"))]
fn partial_response(rejected: u64, message: &str) -> Vec<u8> {
    fn varint(mut value: u64, out: &mut Vec<u8>) {
        while value >= 128 {
            out.push((value as u8) | 128);
            value >>= 7;
        }
        out.push(value as u8);
    }
    if rejected == 0 {
        return Vec::new();
    }
    let mut partial = vec![8];
    varint(rejected, &mut partial);
    partial.push(18);
    varint(message.len() as u64, &mut partial);
    partial.extend_from_slice(message.as_bytes());
    let mut response = vec![10];
    varint(partial.len() as u64, &mut response);
    response.extend(partial);
    response
}
