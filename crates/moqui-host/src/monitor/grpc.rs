//! The OTLP/gRPC receiver (plans/Monitoring.md, Ingest): the trace, logs and metrics collector services on tonic,
//! on `<telemetry><receiver grpc-port>` (4317 by default), opened only when `grpc-enabled="true"`. It shares the
//! HTTP receiver's ingest keys, rate limit, concurrency budget, decoder and queue (`receiver.rs`); a refusal is the
//! matching gRPC status. With the HTTPS listener's certificates it serves TLS (ALPN h2), else plain h2c. The edge
//! guard admits each connection as it does for HTTP.
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

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
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpListener;
use tonic::codec::CompressionEncoding;
use tonic::{Request, Response, Status};

use super::receiver::{self, Refusal};

/// The address the gRPC listener bound, for `Server::grpc_address`.
#[derive(Default)]
pub struct GrpcAddress(pub std::sync::Mutex<Option<SocketAddr>>);

/// Where to listen, when the receiver and its gRPC side are on: `grpc-bind` (the HTTP listener's address when
/// empty) and `grpc-port`.
pub(crate) fn bind_address(factory: &ExecutionContextFactory, http: SocketAddr) -> Option<SocketAddr> {
    if factory.conf_setting("telemetry/receiver", "grpc-enabled") != "true" || !receiver::receiver_enabled(factory) {
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

/// Bind `bind` and serve until `stop`. A bind failure is logged and the HTTP listeners go on without gRPC.
pub(crate) async fn serve(
    factory: Arc<ExecutionContextFactory>,
    bind: SocketAddr,
    tls: Option<Arc<rustls::ServerConfig>>,
    guard: Arc<crate::guard::Guard>,
    stop: impl std::future::Future<Output = ()>,
) {
    let listener = match TcpListener::bind(bind).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!("could not bind the OTLP/gRPC receiver on {bind}: {error}");
            return;
        }
    };
    let local = listener.local_addr().unwrap_or(bind);
    *moqui_lib::sync::lock(&factory.extension(|_| GrpcAddress::default()).0) = Some(local);
    tracing::info!(
        "OTLP/gRPC receiver on {local}{}",
        if tls.is_some() { " (TLS)" } else { "" }
    );
    let acceptor = tls.map(|config| {
        // gRPC is HTTP/2 only; the HTTPS listener's config offers http/1.1.
        let mut config = (*config).clone();
        config.alpn_protocols = vec![b"h2".to_vec()];
        tokio_rustls::TlsAcceptor::from(Arc::new(config))
    });
    let limit = receiver::body_limit(&factory);
    let otlp = Arc::new(Otlp {
        factory: Arc::clone(&factory),
    });
    let routes = tonic::service::Routes::new(
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
    );
    let (connections, incoming) = tokio::sync::mpsc::channel::<std::io::Result<Connection>>(64);
    let accepting = tokio::spawn(accept(listener, acceptor, guard, connections));
    let incoming = futures_util::stream::unfold(incoming, |mut incoming| async move {
        incoming.recv().await.map(|connection| (connection, incoming))
    });
    if let Err(error) = tonic::transport::Server::builder()
        .add_routes(routes)
        .serve_with_incoming_shutdown(incoming, stop)
        .await
    {
        tracing::error!("the OTLP/gRPC receiver stopped: {error}");
    }
    accepting.abort();
}

/// Accept connections, each TLS handshake in a task of its own so a slow client does not hold up the others.
async fn accept(
    listener: TcpListener,
    acceptor: Option<tokio_rustls::TlsAcceptor>,
    guard: Arc<crate::guard::Guard>,
    connections: tokio::sync::mpsc::Sender<std::io::Result<Connection>>,
) {
    loop {
        let (stream, remote) = match listener.accept().await {
            Ok(pair) => pair,
            Err(error) => {
                tracing::debug!("OTLP/gRPC accept failed: {error}");
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                continue;
            }
        };
        let Some(slot) = guard.admit(remote.ip()) else {
            continue;
        };
        let _ = stream.set_nodelay(true);
        let connections = connections.clone();
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            let io: Box<dyn Io> = match acceptor {
                None => Box::new(stream),
                Some(acceptor) => {
                    match tokio::time::timeout(std::time::Duration::from_secs(10), acceptor.accept(stream)).await {
                        Ok(Ok(stream)) => Box::new(stream),
                        Ok(Err(error)) => {
                            tracing::debug!("OTLP/gRPC TLS handshake with {remote} failed: {error}");
                            return;
                        }
                        Err(_) => return,
                    }
                }
            };
            let _ = connections
                .send(Ok(Connection {
                    io,
                    remote,
                    _slot: slot,
                }))
                .await;
        });
    }
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// A plain or TLS connection, holding its edge-guard slot while it is open.
struct Connection {
    io: Box<dyn Io>,
    remote: SocketAddr,
    _slot: crate::guard::ConnectionSlot,
}

impl tonic::transport::server::Connected for Connection {
    type ConnectInfo = SocketAddr;

    fn connect_info(&self) -> SocketAddr {
        self.remote
    }
}

impl AsyncRead for Connection {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.io).poll_read(cx, buf)
    }
}

impl AsyncWrite for Connection {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut *self.io).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.io).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut *self.io).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }
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
