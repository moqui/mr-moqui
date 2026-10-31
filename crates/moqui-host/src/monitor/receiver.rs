//! The OTLP/HTTP receiver (plans/Monitoring.md, Ingest), and the steps it shares with the gRPC receiver
//! (`grpc.rs`): the concurrency budget, ingest-key authentication with its rate limit, and the decode and enqueue
//! as the key's owner. A refusal is an HTTP status and a message; gRPC maps it to a `tonic::Status`.
use std::sync::Arc;
use std::time::Duration;

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::telemetry::ingest::{Decoded, Decoder, Signal};
use moqui_lib::error::Error;
use moqui_lib::object::Value;

/// A refused request: the HTTP status and the message the sender sees.
pub(crate) type Refusal = (u16, String);

pub(crate) fn routes(app: axum::Router<Arc<crate::http::HttpState>>) -> axum::Router<Arc<crate::http::HttpState>> {
    app.route("/v1/traces", axum::routing::post(ingest))
        .route("/v1/logs", axum::routing::post(ingest))
        .route("/v1/metrics", axum::routing::post(ingest))
        .route("/v1/annotations", axum::routing::post(ingest))
        .route("/v1/recovery", axum::routing::post(ingest))
}

/// An authenticated ingest request: it holds one of the receiver's concurrent-request permits until it is done.
pub(crate) struct Authorized {
    _permit: tokio::sync::OwnedSemaphorePermit,
    pub(crate) key: String,
    pub(crate) source: String,
    pub(crate) username: String,
}

pub(crate) fn receiver_enabled(factory: &ExecutionContextFactory) -> bool {
    factory.conf_setting("telemetry/receiver", "enabled") != "false"
}

pub(crate) fn body_limit(factory: &ExecutionContextFactory) -> usize {
    factory.conf_number("telemetry/receiver", "body-limit", 4 * 1024 * 1024_usize)
}

/// One of the `concurrent-requests` permits, shared by both transports. A refused request is the sender's to retry,
/// so it is not counted as dropped records: the decoded records that are rejected or do not fit the queue are, one
/// by one.
pub(crate) fn admit(factory: &ExecutionContextFactory) -> Result<tokio::sync::OwnedSemaphorePermit, Refusal> {
    let maximum = factory
        .conf_number::<usize>("telemetry/receiver", "concurrent-requests", 8)
        .clamp(1, 4096);
    let budget = factory.extension(|_| tokio::sync::Semaphore::new(maximum));
    budget
        .try_acquire_owned()
        .map_err(|_| (503, "telemetry receiver is busy".to_string()))
}

/// Admit, then authenticate `token` and count it against its key's rate, before the body is read.
pub(crate) async fn authorize(factory: &Arc<ExecutionContextFactory>, token: String) -> Result<Authorized, Refusal> {
    if token.is_empty() {
        return Err((401, "an ingest key is required".into()));
    }
    let permit = admit(factory)?;
    let factory = Arc::clone(factory);
    tokio::task::spawn_blocking(move || {
        let _suppressed = moqui_core::telemetry::suppress();
        let ec = factory.new_context();
        let _entered = ec.enter();
        let (key, source) = moqui_report::monitor::authenticate(&ec, &token).map_err(authentication_error)?;
        let max = factory.conf_number("telemetry/receiver", "requests-per-minute", 600_usize);
        if !factory.extension(|_| RateLimit::default()).allow(&key, max) {
            return Err((429, "ingest key rate limit exceeded".to_string()));
        }
        let username = moqui_core::user::current_username().ok().flatten().unwrap_or_default();
        Ok(Authorized {
            _permit: permit,
            key,
            source,
            username,
        })
    })
    .await
    .unwrap_or_else(|_| Err((503, "ingest authentication worker stopped".into())))
}

/// Run `work` on a blocking thread in a context logged in as `username`, with telemetry of its own suppressed.
pub(crate) async fn run_as<T: Send + 'static>(
    factory: &Arc<ExecutionContextFactory>,
    username: String,
    work: impl FnOnce(&Arc<ExecutionContextFactory>, &ExecutionContext) -> Result<T, Refusal> + Send + 'static,
) -> Result<T, Refusal> {
    let factory = Arc::clone(factory);
    tokio::task::spawn_blocking(move || {
        let _suppressed = moqui_core::telemetry::suppress();
        let ec = factory.new_context();
        let _entered = ec.enter();
        // The owner, whom authentication logged in and checked, writes what the request carries.
        moqui_core::user::internal_login_user(&username).map_err(|error| {
            tracing::warn!(%error, "ingest owner could not be logged in");
            (503, "ingest authorization is unavailable".to_string())
        })?;
        work(&factory, &ec)
    })
    .await
    .unwrap_or_else(|_| Err((500, "ingest worker stopped".into())))
}

/// Decode with `decode` and hand the documents to the telemetry queue. Answers the rejected count and its message
/// for the partial-success response.
pub(crate) fn enqueue(
    factory: &ExecutionContextFactory,
    decode: impl FnOnce(&Decoder) -> Result<Decoded, Error>,
) -> Result<(u64, String), Refusal> {
    let telemetry = moqui_core::telemetry::for_factory(factory);
    if !telemetry.accepting() {
        return Err((503, "telemetry is disabled".into()));
    }
    let decoded = decode(&factory.extension(|_| Decoder::default())).map_err(|e| (400, e.to_string()))?;
    telemetry.note_dropped(decoded.rejected);
    // The queue counts each record it cannot take.
    telemetry
        .try_ingest(decoded.index, decoded.documents)
        .map_err(|_| (503, "telemetry queue is full".to_string()))?;
    Ok((decoded.rejected, decoded.message))
}

/// Record that the key sent something, for the Sources screen. A failure is only logged.
pub(crate) fn touch_key(factory: &ExecutionContextFactory, ec: &ExecutionContext, key: &str) {
    if let Some(entity) = factory.entity() {
        let _authz = ec.disable_authz();
        if let Err(error) = entity.find("moqui.monitor.IngestKey").and_then(|find| {
            find.eq("keyId", Value::string(key))
                .update_all([("lastSeen".into(), Value::I64(crate::logging::now_millis()))])
        }) {
            tracing::warn!(%error, "could not update ingest source last-seen time");
        }
    }
}

/// Undo `gzip`, holding the result to `limit`.
pub(crate) fn gunzip(body: &[u8], limit: usize) -> Result<Vec<u8>, Refusal> {
    use std::io::Read;
    let mut bytes = Vec::new();
    flate2::read::MultiGzDecoder::new(body)
        .take((limit as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| (400, "invalid gzip OTLP body".to_string()))?;
    if bytes.len() > limit {
        return Err((413, "decompressed ingest body exceeds the configured limit".into()));
    }
    Ok(bytes)
}

fn refused(status: u16, message: impl Into<String>) -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        axum::http::StatusCode::from_u16(status).unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
        message.into(),
    )
        .into_response()
}

async fn ingest(
    axum::extract::State(state): axum::extract::State<Arc<crate::http::HttpState>>,
    request: axum::extract::Request,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if !receiver_enabled(&state.factory) {
        return refused(404, "not found");
    }
    // An owned copy: a borrow of the request (whose body is not Sync) must not live across an await.
    let headers = request.headers().clone();
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let encoding = header("content-encoding").unwrap_or_else(|| "identity".into());
    if !matches!(encoding.as_str(), "identity" | "gzip") {
        return refused(415, "unsupported OTLP content encoding");
    }
    let is_annotation = request.uri().path() == "/v1/annotations";
    // A deployment controller's report on a recovery action (plans/Monitoring.md, Later: Recovery): the ingest key and
    // the action's callback token both have to match.
    let is_recovery = request.uri().path() == "/v1/recovery";
    let signal = match request.uri().path() {
        "/v1/traces" => Signal::Traces,
        "/v1/logs" => Signal::Logs,
        _ => Signal::Metrics,
    };
    let content_type = header("content-type").unwrap_or_default();
    if !matches!(
        content_type.split(';').next().unwrap_or("").trim(),
        "application/json" | "application/x-protobuf"
    ) {
        return refused(415, "OTLP requires application/json or application/x-protobuf");
    }
    let token = header("api_key")
        .or_else(|| header("authorization").and_then(|v| v.strip_prefix("Bearer ").map(str::to_owned)))
        .unwrap_or_default();
    let authorized = match authorize(&state.factory, token).await {
        Ok(authorized) => authorized,
        Err((status, message)) => return refused(status, message),
    };
    let limit = body_limit(&state.factory);
    let body = match tokio::time::timeout(
        Duration::from_secs(30),
        axum::body::to_bytes(request.into_body(), limit),
    )
    .await
    {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => return refused(413, "ingest body exceeds the configured limit"),
        Err(_) => return refused(408, "ingest body timed out"),
    };
    let protobuf = content_type.starts_with("application/x-protobuf");
    let username = authorized.username.clone();
    let result = run_as(&state.factory, username, move |factory, ec| {
        let authorized = authorized;
        let body = if encoding == "gzip" {
            gunzip(&body, limit)?
        } else {
            body.to_vec()
        };
        if is_recovery {
            if !content_type.starts_with("application/json") {
                return Err((415, "recovery reports require JSON".into()));
            }
            let json: serde_json::Value =
                serde_json::from_slice(&body).map_err(|_| (400, "invalid recovery report JSON".to_string()))?;
            moqui_report::recovery::callback(ec, &json).map_err(|error| (400, error.to_string()))?;
            return Ok((0, String::new()));
        }
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
                .insert("monitor.ingest.source".into(), Value::string(&authorized.source));
            let service = moqui_core::native::ServiceInfo {
                name: "org.moqui.report.ReportServices.create#Annotation",
                service_type: "rust",
                location: "moqui_core::report::create_annotation",
                method: "",
            };
            moqui_report::create_annotation(ec, &service, &parameters.into_iter().collect())
                .map_err(|error| (400, error.to_string()))?;
            return Ok((0, String::new()));
        }
        let answer = enqueue(factory, |decoder| {
            decoder.decode_scoped(signal, &content_type, &body, &authorized.key, &authorized.source)
        })?;
        touch_key(factory, ec, &authorized.key);
        Ok(answer)
    })
    .await;
    match result {
        Ok((rejected, message)) => {
            if protobuf {
                (
                    [(axum::http::header::CONTENT_TYPE, "application/x-protobuf")],
                    partial_response(rejected, &message),
                )
                    .into_response()
            } else {
                axum::Json(partial_json(signal, rejected, &message)).into_response()
            }
        }
        Err((status, message)) => refused(status, message),
    }
}

/// The OTLP/JSON partial-success body; empty when nothing was rejected.
pub(crate) fn partial_json(signal: Signal, rejected: u64, message: &str) -> serde_json::Value {
    let field = match signal {
        Signal::Traces => "rejectedSpans",
        Signal::Logs => "rejectedLogRecords",
        Signal::Metrics => "rejectedDataPoints",
    };
    if rejected == 0 {
        serde_json::json!({})
    } else {
        serde_json::json!({"partialSuccess":{field:rejected.to_string(),"errorMessage":message}})
    }
}

fn authentication_error(error: Error) -> Refusal {
    if moqui_core::context::ErrorKind::of(&error) == moqui_core::context::ErrorKind::Unavailable {
        tracing::warn!(%error, "ingest authorization is unavailable");
        (503, "ingest authorization is unavailable".into())
    } else {
        (401, "ingest key is invalid, expired, revoked or unauthorized".into())
    }
}

/// Requests per minute per key (or per session, for RUM), for at most 4096 keys a minute.
#[derive(Default)]
pub(crate) struct RateLimit(std::sync::Mutex<std::collections::HashMap<String, (i64, usize)>>);

impl RateLimit {
    pub(crate) fn allow(&self, key: &str, max: usize) -> bool {
        let minute = crate::logging::now_millis() / 60000;
        let mut entries = moqui_lib::sync::lock(&self.0);
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
