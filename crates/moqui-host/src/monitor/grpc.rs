//! The OTLP/gRPC receiver (plans/Monitoring.md, Ingest): the trace, logs and metrics collector services on tonic,
//! on `<telemetry><receiver grpc-port>` (4317 by default), opened only when `grpc-enabled="true"`. It shares the
//! HTTP receiver's ingest keys, rate limit, concurrency budget, decoder and queue (`receiver.rs`); a refusal is the
//! matching gRPC status. The services are routes of the host's gRPC listener (`crate::grpc_listen`), which serves TLS
//! with the HTTPS listener's certificates (ALPN h2), else plain h2c, and admits each connection by the edge guard as
//! for HTTP.
use std::net::SocketAddr;
use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::telemetry::ingest::Signal;
use opentelemetry_proto::tonic::collector::logs::v1::logs_service_server::{LogsService, LogsServiceServer};
use opentelemetry_proto::tonic::collector::logs::v1::{
    ExportLogsPartialSuccess, ExportLogsServiceRequest, ExportLogsServiceResponse,
};
use opentelemetry_proto::tonic::collector::metrics::v1::metrics_service_server::{
    MetricsService, MetricsServiceServer,
};
use opentelemetry_proto::tonic::collector::metrics::v1::{
    ExportMetricsPartialSuccess, ExportMetricsServiceRequest, ExportMetricsServiceResponse,
};
use opentelemetry_proto::tonic::collector::trace::v1::trace_service_server::{TraceService, TraceServiceServer};
use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTracePartialSuccess, ExportTraceServiceRequest, ExportTraceServiceResponse,
};
use tonic::codec::CompressionEncoding;
use tonic::{Request, Response, Status};

use super::receiver::{self, Refusal};

/// Whether the OTLP/gRPC receiver is on: `<telemetry><receiver grpc-enabled="true">` with the receiver enabled.
pub(crate) fn enabled(factory: &ExecutionContextFactory) -> bool {
    factory.conf_setting("telemetry/receiver", "grpc-enabled") == "true" && receiver::receiver_enabled(factory)
}

/// Where the receiver listens, when it is on: `grpc-bind` (the HTTP listener's address when empty) and `grpc-port`.
pub(crate) fn bind_address(factory: &ExecutionContextFactory, http: SocketAddr) -> Option<SocketAddr> {
    if !enabled(factory) {
        return None;
    }
    let port = factory.conf_number("telemetry/receiver", "grpc-port", 4317_u16);
    let ip = match factory.conf_setting("telemetry/receiver", "grpc-bind").trim() {
        "" => http.ip(),
        text => text.parse().unwrap_or_else(|_| {
            tracing::error!(
                "telemetry receiver grpc-bind {text} is not an IP address; using {}",
                http.ip()
            );
            http.ip()
        }),
    };
    Some(SocketAddr::new(ip, port))
}

/// `routes` with the collector services, held to the receiver's `body-limit`.
pub(crate) fn add_routes(
    routes: tonic::service::Routes,
    factory: &Arc<ExecutionContextFactory>,
) -> tonic::service::Routes {
    let limit = receiver::body_limit(factory);
    let otlp = Arc::new(Otlp {
        factory: Arc::clone(factory),
    });
    routes
        .add_service(
            TraceServiceServer::from_arc(Arc::clone(&otlp))
                .accept_compressed(CompressionEncoding::Gzip)
                .max_decoding_message_size(limit),
        )
        .add_service(
            LogsServiceServer::from_arc(Arc::clone(&otlp))
                .accept_compressed(CompressionEncoding::Gzip)
                .max_decoding_message_size(limit),
        )
        .add_service(
            MetricsServiceServer::from_arc(otlp)
                .accept_compressed(CompressionEncoding::Gzip)
                .max_decoding_message_size(limit),
        )
}

struct Otlp {
    factory: Arc<ExecutionContextFactory>,
}

impl Otlp {
    /// Authenticate the request's key, then decode its message through its serde form, as the HTTP receiver does a
    /// protobuf body. tonic has decoded the message already, held to `body-limit` by `max_decoding_message_size`.
    async fn receive<T: Send + 'static>(
        &self,
        signal: Signal,
        request: Request<T>,
        to_json: fn(&T) -> serde_json::Result<serde_json::Value>,
    ) -> Result<(u64, String), Status> {
        if !receiver::receiver_enabled(&self.factory) {
            return Err(Status::unimplemented("the telemetry receiver is disabled"));
        }
        let token = token(request.metadata());
        let authorized = receiver::authorize(&self.factory, token).await.map_err(status)?;
        let message = request.into_inner();
        let username = authorized.username.clone();
        receiver::run_as(&self.factory, username, move |factory, ec| {
            let json = to_json(&message).map_err(|error| (400, format!("OTLP normalization failed: {error}")))?;
            let answer = receiver::enqueue(factory, |decoder| {
                Ok(decoder.decode_value(signal, &json, &authorized.key, &authorized.source))
            })?;
            receiver::touch_key(factory, ec, &authorized.key);
            Ok(answer)
        })
        .await
        .map_err(status)
    }
}

/// The key, from `authorization: Bearer` or `api_key`, as the HTTP receiver takes it.
fn token(metadata: &tonic::metadata::MetadataMap) -> String {
    let text = |name: &str| metadata.get(name).and_then(|value| value.to_str().ok());
    text("api_key")
        .or_else(|| text("authorization").and_then(|value| value.strip_prefix("Bearer ")))
        .unwrap_or("")
        .to_owned()
}

/// The gRPC status for an HTTP receiver refusal.
pub(crate) fn status((code, message): Refusal) -> Status {
    match code {
        401 => Status::unauthenticated(message),
        413 | 429 => Status::resource_exhausted(message),
        503 => Status::unavailable(message),
        400 | 415 => Status::invalid_argument(message),
        404 => Status::unimplemented(message),
        _ => Status::internal(message),
    }
}

fn rejected(count: u64) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

#[tonic::async_trait]
impl TraceService for Otlp {
    async fn export(
        &self,
        request: Request<ExportTraceServiceRequest>,
    ) -> Result<Response<ExportTraceServiceResponse>, Status> {
        let (count, error_message) = self
            .receive(Signal::Traces, request, |message| serde_json::to_value(message))
            .await?;
        Ok(Response::new(ExportTraceServiceResponse {
            partial_success: (count > 0).then(|| ExportTracePartialSuccess {
                rejected_spans: rejected(count),
                error_message,
            }),
        }))
    }
}

#[tonic::async_trait]
impl LogsService for Otlp {
    async fn export(
        &self,
        request: Request<ExportLogsServiceRequest>,
    ) -> Result<Response<ExportLogsServiceResponse>, Status> {
        let (count, error_message) = self
            .receive(Signal::Logs, request, |message| serde_json::to_value(message))
            .await?;
        Ok(Response::new(ExportLogsServiceResponse {
            partial_success: (count > 0).then(|| ExportLogsPartialSuccess {
                rejected_log_records: rejected(count),
                error_message,
            }),
        }))
    }
}

#[tonic::async_trait]
impl MetricsService for Otlp {
    async fn export(
        &self,
        request: Request<ExportMetricsServiceRequest>,
    ) -> Result<Response<ExportMetricsServiceResponse>, Status> {
        let (count, error_message) = self
            .receive(Signal::Metrics, request, |message| serde_json::to_value(message))
            .await?;
        Ok(Response::new(ExportMetricsServiceResponse {
            partial_success: (count > 0).then(|| ExportMetricsPartialSuccess {
                rejected_data_points: rejected(count),
                error_message,
            }),
        }))
    }
}

#[cfg(test)]
#[path = "grpc_tests.rs"]
mod tests;
