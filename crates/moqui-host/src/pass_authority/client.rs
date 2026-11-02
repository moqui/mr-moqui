/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! A PASS frontend's calls to its authority over gRPC (plans/Edge-Node.md §5.5): the `moqui_core::pass_authority`
//! sender behind scripts' `ec.service.passAuthority(endpoint)`, on the host's runtime.
//! - `grpc://host:port` and `grpcs://host:port`: the authority's gRPC listener, plain HTTP/2 or TLS (WebPKI roots,
//!   ALPN h2), one connection per endpoint kept and dialed again when it closes; the client is `pass_client_id`, its
//!   secret `pass_client_secret` in the metadata.
//! - `edge:`: on an edge node, its local node down the link (`EdgeServer::only_link`); "local node offline" at once
//!   while the link is down.

use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use moqui_core::pass_authority::{Call, Download, DownloadMeta, PassError, PassSender, Reply, UploadFile};
use moqui_lib::error::Error;
use moqui_lib::sync::lock;
use tokio::net::TcpStream;

use super::proto::pass_authority_client::PassAuthorityClient;
use super::proto::{download_part, upload_part, UploadPart};
use crate::edge_link::H2Channel;

/// A call, an upload, and each part of a download wait at most this long, as the REST adapter's calls.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Dialing a direct endpoint, TCP and TLS.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// An upload's chunk.
const CHUNK: usize = 64 * 1024;

type H2Send = hyper::client::conn::http2::SendRequest<tonic::body::Body>;
type AuthorityClient = PassAuthorityClient<H2Channel>;

/// Where a call goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Endpoint {
    Direct { host: String, port: u16, tls: bool },
    Edge,
}

/// `grpc://host:port`, `grpcs://host:port` (port 4317 when left out) or `edge:`.
pub(crate) fn parse_endpoint(endpoint: &str) -> Result<Endpoint, Error> {
    let text = endpoint.trim();
    if text == "edge:" {
        return Ok(Endpoint::Edge);
    }
    let (rest, tls) = match (text.strip_prefix("grpcs://"), text.strip_prefix("grpc://")) {
        (Some(rest), _) => (rest, true),
        (None, Some(rest)) => (rest, false),
        _ => {
            return Err(Error::new(format!(
                "{text} is not a PASS authority endpoint (grpc://host:port, grpcs://host:port or edge:)"
            )))
        }
    };
    let authority: http::uri::Authority = rest
        .trim_end_matches('/')
        .parse()
        .map_err(|_| Error::new(format!("{text} has no host")))?;
    let host = authority
        .host()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    if host.is_empty() {
        return Err(Error::new(format!("{text} has no host")));
    }
    Ok(Endpoint::Direct {
        host,
        port: authority.port_u16().unwrap_or(4317),
        tls,
    })
}

/// The sender, with its direct connections by endpoint.
#[derive(Default)]
pub(crate) struct Sender {
    direct: Arc<Mutex<HashMap<String, H2Send>>>,
}

/// A request with the direct client's metadata.
fn request<T>(message: T, endpoint: &Endpoint) -> Result<tonic::Request<T>, Error> {
    let mut request = tonic::Request::new(message);
    request.set_timeout(CALL_TIMEOUT);
    if matches!(endpoint, Endpoint::Direct { .. }) {
        let (id, secret) = direct_client();
        if id.is_empty() || secret.is_empty() {
            return Err(Error::new(
                "pass_client_id and pass_client_secret must be set to call a gRPC authority",
            ));
        }
        let metadata = request.metadata_mut();
        for (name, value) in [("x-pass-client", id), ("x-pass-client-secret", secret)] {
            let value = value
                .parse()
                .map_err(|_| Error::new(format!("{name} is not a valid metadata value")))?;
            metadata.insert(name, value);
        }
    }
    Ok(request)
}

/// `pass_client_id` and `pass_client_secret`.
fn direct_client() -> (String, String) {
    let get = |name: &str| moqui_lib::env::get_prop_or_env(name).trim().to_string();
    (get("pass_client_id"), get("pass_client_secret"))
}

/// A transport failure, with the gRPC status's message.
fn failed(status: &tonic::Status) -> Error {
    Error::new(format!(
        "the PASS authority call failed: {} ({:?})",
        status.message(),
        status.code()
    ))
}

impl Sender {
    /// A client for `endpoint`: the kept connection, dialed again when it has closed; or the edge node's down
    /// connection.
    async fn client(
        direct: Arc<Mutex<HashMap<String, H2Send>>>,
        endpoint: &Endpoint,
    ) -> Result<AuthorityClient, Error> {
        let (send, origin) = match endpoint {
            Endpoint::Edge => {
                let server = crate::edge_link::hook::edge_server().ok_or_else(|| {
                    Error::new(format!(
                        "{}: this is not a running edge node, so edge: names no local node",
                        moqui_core::edge::OFFLINE
                    ))
                })?;
                let (_, send, _) = server.only_link()?;
                (send, crate::edge_link::origin())
            }
            Endpoint::Direct { host, port, tls } => {
                let key = format!("{}://{host}:{port}", if *tls { "https" } else { "http" });
                let kept = lock(&direct).get(&key).filter(|send| !send.is_closed()).cloned();
                let send = match kept {
                    Some(send) => send,
                    None => {
                        let send = dial(host, *port, *tls).await?;
                        lock(&direct).insert(key.clone(), send.clone());
                        send
                    }
                };
                let origin: http::Uri = key.parse().map_err(|_| Error::new(format!("{key} is not a URI")))?;
                (send, origin)
            }
        };
        Ok(PassAuthorityClient::with_origin(H2Channel(send), origin)
            .max_decoding_message_size(super::MAX_MESSAGE)
            .max_encoding_message_size(super::MAX_MESSAGE))
    }
}

/// An HTTP/2 connection to a direct endpoint, its driver on the runtime.
async fn dial(host: &str, port: u16, tls: bool) -> Result<H2Send, Error> {
    let place = format!("{host}:{port}");
    let connect = async {
        let tcp = TcpStream::connect((host, port))
            .await
            .map_err(|err| Error::new(format!("could not connect to the PASS authority at {place}: {err}")))?;
        let _ = tcp.set_nodelay(true);
        let io: Box<dyn Io> = if tls {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
            let mut config = rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|err| Error::new(format!("PASS authority TLS: {err}")))?
                .with_root_certificates(roots)
                .with_no_client_auth();
            config.alpn_protocols = vec![b"h2".to_vec()];
            let name = rustls::pki_types::ServerName::try_from(host.to_string())
                .map_err(|_| Error::new(format!("{host} is not a host name")))?;
            let stream = tokio_rustls::TlsConnector::from(Arc::new(config))
                .connect(name, tcp)
                .await
                .map_err(|err| Error::new(format!("TLS with the PASS authority at {place} failed: {err}")))?;
            Box::new(stream)
        } else {
            Box::new(tcp)
        };
        Ok::<_, Error>(io)
    };
    let io = tokio::time::timeout(CONNECT_TIMEOUT, connect)
        .await
        .map_err(|_| Error::new(format!("the PASS authority at {place} did not answer in time")))??;
    let (send, connection) = hyper::client::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new())
        .timer(hyper_util::rt::TokioTimer::new())
        .keep_alive_interval(Duration::from_secs(30))
        .keep_alive_timeout(Duration::from_secs(20))
        .handshake(hyper_util::rt::TokioIo::new(io))
        .await
        .map_err(|err| Error::new(format!("HTTP/2 with the PASS authority at {place} failed: {err}")))?;
    let place = place.clone();
    tokio::spawn(async move {
        if let Err(err) = connection.await {
            tracing::debug!("the connection to the PASS authority at {place} ended: {err}");
        }
    });
    Ok(send)
}

trait Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Io for T {}

impl PassSender for Sender {
    fn call(&self, endpoint: &str, mut call: Call) -> Result<Reply, Error> {
        let endpoint = parse_endpoint(endpoint)?;
        let direct = Arc::clone(&self.direct);
        crate::edge::block_on(async move {
            call.client_id = direct_client_id(&endpoint);
            let mut client = Sender::client(direct, &endpoint).await?;
            let request = request(super::call_message(&call), &endpoint)?;
            let answer = tokio::time::timeout(CALL_TIMEOUT, client.call(request))
                .await
                .map_err(|_| Error::new("the PASS authority did not answer in time"))?
                .map_err(|status| failed(&status))?;
            Ok(super::reply_from(answer.into_inner()))
        })?
    }

    fn upload(&self, endpoint: &str, mut call: Call, file: UploadFile) -> Result<Reply, Error> {
        let endpoint = parse_endpoint(endpoint)?;
        let direct = Arc::clone(&self.direct);
        crate::edge::block_on(async move {
            call.client_id = direct_client_id(&endpoint);
            let mut client = Sender::client(direct, &endpoint).await?;
            let length = file.bytes.len() as u64;
            let mut parts = vec![
                UploadPart {
                    part: Some(upload_part::Part::Call(super::call_message(&call))),
                },
                UploadPart {
                    part: Some(upload_part::Part::File(super::proto::UploadFile {
                        filename: file.filename,
                        content_type: file.content_type,
                        length,
                    })),
                },
            ];
            for chunk in file.bytes.chunks(CHUNK) {
                parts.push(UploadPart {
                    part: Some(upload_part::Part::Chunk(chunk.to_vec())),
                });
            }
            let request = request(futures_util::stream::iter(parts), &endpoint)?;
            let answer = tokio::time::timeout(CALL_TIMEOUT, client.upload(request))
                .await
                .map_err(|_| Error::new("the PASS authority did not answer in time"))?
                .map_err(|status| failed(&status))?;
            Ok(super::reply_from(answer.into_inner()))
        })?
    }

    fn download(&self, endpoint: &str, mut call: Call) -> Result<Result<Download, PassError>, Error> {
        let endpoint = parse_endpoint(endpoint)?;
        let direct = Arc::clone(&self.direct);
        let opened = crate::edge::block_on(async move {
            call.client_id = direct_client_id(&endpoint);
            let mut client = Sender::client(direct, &endpoint).await?;
            let request = request(super::call_message(&call), &endpoint)?;
            let mut stream = tokio::time::timeout(CALL_TIMEOUT, client.download(request))
                .await
                .map_err(|_| Error::new("the PASS authority did not answer in time"))?
                .map_err(|status| failed(&status))?
                .into_inner();
            let first = tokio::time::timeout(CALL_TIMEOUT, stream.message())
                .await
                .map_err(|_| Error::new("the PASS authority did not answer in time"))?
                .map_err(|status| failed(&status))?
                .and_then(|part| part.part);
            Ok::<_, Error>((first, stream))
        })??;
        let meta = match opened.0 {
            Some(download_part::Part::Meta(meta)) => meta,
            Some(download_part::Part::Error(error)) => return Ok(Err(super::error_from(error))),
            _ => return Err(Error::new("the PASS authority's download began with no metadata")),
        };
        let (sender, receiver) = tokio::sync::mpsc::channel(8);
        let mut stream = opened.1;
        crate::edge::handle().spawn(async move {
            loop {
                let next = match tokio::time::timeout(CALL_TIMEOUT, stream.message()).await {
                    Ok(Ok(Some(part))) => match part.part {
                        Some(download_part::Part::Chunk(chunk)) => Ok(Bytes::from(chunk)),
                        _ => Err("the PASS authority sent a download part out of place".to_string()),
                    },
                    Ok(Ok(None)) => return,
                    Ok(Err(status)) => Err(failed(&status).message().to_string()),
                    Err(_) => Err("the PASS authority's download stalled".to_string()),
                };
                let stop = next.is_err();
                if sender.send(next).await.is_err() || stop {
                    return;
                }
            }
        });
        Ok(Ok(Download {
            meta: DownloadMeta {
                content_type: meta.content_type,
                filename: meta.filename,
                length: (meta.length > 0).then_some(meta.length),
                disposition: meta.disposition,
            },
            reader: Box::new(ChannelReader {
                receiver,
                current: Bytes::new(),
            }),
        }))
    }
}

/// The client id a direct call names in its message too; none on the link.
fn direct_client_id(endpoint: &Endpoint) -> String {
    match endpoint {
        Endpoint::Direct { .. } => direct_client().0,
        Endpoint::Edge => String::new(),
    }
}

/// A download's bytes as they arrive, for a reader on a blocking thread (a streamed response's, or a script's
/// `bytes()`).
struct ChannelReader {
    receiver: tokio::sync::mpsc::Receiver<Result<Bytes, String>>,
    current: Bytes,
}

impl Read for ChannelReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while self.current.is_empty() {
            match self.receiver.blocking_recv() {
                Some(Ok(chunk)) => self.current = chunk,
                Some(Err(message)) => return Err(std::io::Error::other(message)),
                None => return Ok(0),
            }
        }
        let count = buf.len().min(self.current.len());
        buf[..count].copy_from_slice(&self.current.split_to(count));
        Ok(count)
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
