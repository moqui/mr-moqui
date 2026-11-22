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

//! The PASS authority over gRPC (plans/archive/Edge-Node.md §5.5): `moqui.pass.v1.PassAuthority`, generated from
//! `proto/pass.proto` ([`proto`]), what mr-pass calls in place of the authority's REST routes. It is served
//! - **direct**, on the host's gRPC listener (`crate::grpc_listen`) when `moqui_conf__pass_authority__grpc_enabled=true`: a call names
//!   a registered client in the `x-pass-client` metadata (or its `client_id`) with its secret in
//!   `x-pass-client-secret`, which the framework's client check (the REST servlet's) accepts before anything runs;
//! - **on the link**, on a local node's down connection (`edge_link::down`), for its edge node only and never for a
//!   forwarded request: the link's mutual TLS is the client's identity, its client id `edge/node@pass-client` (`edge`).
//!
//! Each call runs on the request pool through `moqui_core::pass_authority`, which runs the authority service
//! (`pass-authority@service`, Mantle's `handle#Call`) with the visitor's address as data only. [`client`] is the other
//! end, the sender behind scripts' `ec.service.passAuthority(endpoint)`.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use futures_util::Stream;
use moqui_core::context::ExecutionContextFactory;
use moqui_core::pass_authority::{self as core, Client, Refused, Reply, UploadFile};
use moqui_lib::sync::lock;
use tonic::{Request, Response, Status, Streaming};

use proto::pass_authority_server::{PassAuthority, PassAuthorityServer};
use proto::{download_part, upload_part, DownloadMeta, DownloadPart, PassCall, PassError, PassResult, UploadPart};

mod client;
pub(crate) mod proto;

/// `true` opens the host's gRPC listener for direct PASS clients.
pub(crate) const ENABLED_PROPERTY: &str = "pass-authority@grpc-enabled";
/// `true` serves direct clients on plain h2c at an address that is not loopback (their secrets in clear).
pub(crate) const INSECURE_PROPERTY: &str = "pass-authority@grpc-insecure";
/// The path prefix of the service's calls, which a local node's down connection takes from its edge node.
pub(crate) const PATH_PREFIX: &str = "/moqui.pass.v1.PassAuthority/";
/// The metadata that names a direct client, and its secret: the REST API's headers.
const CLIENT_METADATA: &str = "x-pass-client";
const SECRET_METADATA: &str = "x-pass-client-secret";
/// The largest message either way: a result, a chunk.
pub(crate) const MAX_MESSAGE: usize = 16 * 1024 * 1024;
/// The largest message a client sends the service: a call (its JSON input, at most a post of 100,000 characters and
/// its fields) or an upload's part (64 KiB chunks).
pub(crate) const MAX_REQUEST: usize = 2 * 1024 * 1024;
/// Wrong secrets from one source in [`FAILURE_WINDOW`] before its calls are checked one at a time ([`Authority::admit`])
/// until the window ends.
const FAILURE_LIMIT: u32 = 10;
const FAILURE_WINDOW: Duration = Duration::from_secs(600);
/// The sources whose wrong secrets are counted at once; while the table is full a new source is checked as one past
/// its limit.
const MAX_FAILURE_SOURCES: usize = 10_000;
/// How long a call from a source past its limit waits for its turn to be checked before it is refused.
const HELD_WAIT: Duration = Duration::from_secs(5);
/// A download's chunk.
const CHUNK: usize = 64 * 1024;

/// Whether the host's gRPC listener serves direct PASS clients (`pass-authority@grpc-enabled`).
pub(crate) fn direct_enabled() -> bool {
    moqui_core::context::conf_env::setting(ENABLED_PROPERTY).trim() == "true"
}

/// `routes` with the direct service added when it is enabled.
pub(crate) fn add_direct(
    routes: tonic::service::Routes,
    factory: &Arc<ExecutionContextFactory>,
) -> tonic::service::Routes {
    if !direct_enabled() {
        return routes;
    }
    routes.add_service(server(Arc::downgrade(factory), Mode::Direct))
}

/// The service a local node's down connection serves its edge node.
pub(crate) fn link_server(factory: Weak<ExecutionContextFactory>) -> PassAuthorityServer<Authority> {
    server(factory, Mode::Link)
}

fn server(factory: Weak<ExecutionContextFactory>, mode: Mode) -> PassAuthorityServer<Authority> {
    PassAuthorityServer::new(Authority {
        factory,
        mode,
        failures: Mutex::new(HashMap::new()),
        held_checks: tokio::sync::Semaphore::new(1),
    })
    .max_decoding_message_size(MAX_REQUEST)
    .max_encoding_message_size(MAX_MESSAGE)
}

/// Install the sender of scripts' calls to an authority in this process, at server start so a direct endpoint works
/// with no listener; `edge:` is "local node offline" until this node's edge link runs. The guard clears it, and closes
/// the direct connections it kept, when the server stops.
pub(crate) fn install_sender() -> SenderGuard {
    let sender = Arc::new(client::Sender::default());
    moqui_core::pass_authority::install_pass_sender(Arc::clone(&sender) as Arc<dyn core::PassSender>);
    SenderGuard(sender)
}

/// The installed sender, until the server stops.
pub(crate) struct SenderGuard(Arc<client::Sender>);

impl Drop for SenderGuard {
    fn drop(&mut self) {
        moqui_core::pass_authority::clear_pass_sender();
        self.0.close();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// On the gRPC listener: the client and its secret in metadata.
    Direct,
    /// On a local node's down connection: the edge node.
    Link,
}

pub(crate) struct Authority {
    factory: Weak<ExecutionContextFactory>,
    mode: Mode,
    /// Wrong direct secrets by source ([`source_of`]): when the window began, and how many since.
    failures: Mutex<HashMap<Option<IpAddr>, (Instant, u32)>>,
    /// The one check at a time of the calls from sources past their wrong secrets.
    held_checks: tokio::sync::Semaphore,
}

/// The address of a call's connection: the listener's connections name it as their connect info.
fn remote_ip<T>(request: &Request<T>) -> Option<IpAddr> {
    request
        .remote_addr()
        .or_else(|| {
            request
                .extensions()
                .get::<crate::grpc_listen::Caller>()
                .map(|caller| caller.peer)
        })
        .or_else(|| request.extensions().get::<SocketAddr>().copied())
        .map(|address| address.ip())
}

/// The source a client's wrong secrets count under: an IPv4 address, or an IPv6 address's /64
/// (`moqui_lib::source_guard::source_key`), so one host cannot spread over its /64.
fn source_of(client: Option<IpAddr>) -> Option<IpAddr> {
    client.map(moqui_lib::source_guard::source_key)
}

/// Whether `source` has used up its wrong secrets in the window at `now`; a source not counted while the table is full
/// is taken as one that has.
fn throttled(failures: &HashMap<Option<IpAddr>, (Instant, u32)>, source: Option<IpAddr>, now: Instant) -> bool {
    match failures.get(&source) {
        Some((began, count)) => now.duration_since(*began) < FAILURE_WINDOW && *count >= FAILURE_LIMIT,
        None => failures.len() >= MAX_FAILURE_SOURCES,
    }
}

/// Count one wrong secret from `source` at `now`: the count in its window. A new source is not counted while the table
/// holds [`MAX_FAILURE_SOURCES`] in their windows.
fn record_failure(failures: &mut HashMap<Option<IpAddr>, (Instant, u32)>, source: Option<IpAddr>, now: Instant) -> u32 {
    failures.retain(|_, (began, _)| now.duration_since(*began) < FAILURE_WINDOW);
    if failures.len() >= MAX_FAILURE_SOURCES && !failures.contains_key(&source) {
        return FAILURE_LIMIT;
    }
    let entry = failures.entry(source).or_insert((now, 0));
    entry.1 += 1;
    entry.1
}

impl Authority {
    /// The client of a call: on the link the edge node; direct, the metadata's client and secret, refused before
    /// anything runs when either is missing.
    fn client(&self, metadata: &tonic::metadata::MetadataMap, call_client: &str) -> Result<Client, Status> {
        match self.mode {
            Mode::Link => Ok(Client::Link),
            Mode::Direct => {
                let text = |name: &str| {
                    metadata
                        .get(name)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .trim()
                        .to_string()
                };
                let id = match text(CLIENT_METADATA) {
                    id if id.is_empty() => call_client.trim().to_string(),
                    id => id,
                };
                let secret = text(SECRET_METADATA);
                if id.is_empty() || secret.is_empty() {
                    return Err(Status::unauthenticated(
                        "a registered PASS client and its secret are required",
                    ));
                }
                Ok(Client::Secret { id, secret })
            }
        }
    }

    /// The client of a call for counting its wrong secrets: the connection's peer, or the client a proxy in front
    /// names when the webapp's `client-ip-header` is believed for that peer (`moqui_core::web::address`), so the PASS
    /// servers behind one proxy or front node are not one source.
    fn client_ip<T>(&self, request: &Request<T>) -> Option<IpAddr> {
        let peer = remote_ip(request)?;
        let Some(factory) = self.factory.upgrade() else {
            return Some(peer);
        };
        let addressing = factory.extension(moqui_core::web::address::ClientAddressing::of_factory);
        let named = Some(addressing.header())
            .filter(|header| !header.is_empty())
            .and_then(|header| request.metadata().get(header))
            .and_then(|value| value.to_str().ok());
        Some(addressing.resolve(peer, named))
    }

    /// A direct client checked on the request pool before anything else runs, as `Client::Checked`; refused, and
    /// counted against its source, when the secret is wrong. The link's client passes through.
    async fn checked(&self, client: Client, remote: Option<IpAddr>) -> Result<Client, Status> {
        let Client::Secret { id, secret } = client else {
            return Ok(client);
        };
        let factory = Weak::clone(&self.factory);
        let check = move || async move {
            crate::worker::on_pool(&factory, move |_| Ok(core::check_client(&id, &secret)))
                .await
                .map_err(|err| worker_failed(&err))
        };
        self.admit(remote, HELD_WAIT, check)
            .await
            .map(|id| Client::Checked { id })
    }

    /// Run `check` of a call from `client`: the checked client id, or the refusal. A wrong secret counts against the
    /// client's source. A source past its wrong secrets is not refused unchecked, which would lock out every PASS
    /// server behind its address with it, but its calls are checked one at a time across all such sources, waiting at
    /// most `wait` ([`HELD_WAIT`]), so a guesser's cost stays bounded while a right secret still gets in (SEC-CLU-05,
    /// SEC-PSA-04).
    async fn admit<F, Checked>(&self, client: Option<IpAddr>, wait: Duration, check: F) -> Result<String, Status>
    where
        F: FnOnce() -> Checked,
        Checked: std::future::Future<Output = Result<Result<String, Refused>, Status>>,
    {
        let source = source_of(client);
        let _turn = if throttled(&lock(&self.failures), source, Instant::now()) {
            match tokio::time::timeout(wait, self.held_checks.acquire()).await {
                Ok(Ok(permit)) => Some(permit),
                _ => return Err(Status::resource_exhausted("too many wrong PASS client secrets; wait")),
            }
        } else {
            None
        };
        match check().await? {
            Ok(id) => Ok(id),
            Err(Refused(reason)) => {
                let count = record_failure(&mut lock(&self.failures), source, Instant::now());
                // the first of a window and the one that reaches the limit, not each
                if count == 1 || count == FAILURE_LIMIT {
                    let from = client.map_or_else(|| "an unknown address".to_string(), |ip| ip.to_string());
                    tracing::warn!(
                        "PASS authority: refused a direct call from {from} ({count} in 10 minutes): {reason}"
                    );
                }
                Err(Status::unauthenticated("not a registered PASS client"))
            }
        }
    }
}

/// A call from its message: the input must be a JSON object.
pub(crate) fn call_from(message: PassCall) -> Result<core::Call, Status> {
    let input = if message.input_json.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_slice(&message.input_json)
            .map_err(|err| Status::invalid_argument(format!("the input is not JSON: {err}")))?
    };
    if !input.is_object() {
        return Err(Status::invalid_argument("the input is not a JSON object"));
    }
    if message.operation.is_empty() {
        return Err(Status::invalid_argument("no operation"));
    }
    Ok(core::Call {
        operation: message.operation,
        site_id: message.site_id,
        input,
        delegation: message.delegation,
        visitor_address: message.visitor_address,
        operation_id: message.operation_id,
        session_handle: message.session_handle,
        client_id: message.client_id,
    })
}

/// A call's message.
pub(crate) fn call_message(call: &core::Call) -> PassCall {
    PassCall {
        operation: call.operation.clone(),
        site_id: call.site_id.clone(),
        input_json: call.input.to_string().into_bytes(),
        delegation: call.delegation.clone(),
        visitor_address: call.visitor_address.clone(),
        operation_id: call.operation_id.clone(),
        session_handle: call.session_handle.clone(),
        client_id: call.client_id.clone(),
    }
}

pub(crate) fn error_message(error: &core::PassError) -> PassError {
    PassError {
        code: error.code.clone(),
        message: error.message.clone(),
        field_errors: error
            .field_errors
            .iter()
            .map(|field| proto::FieldError {
                field: field.field.clone(),
                message: field.message.clone(),
            })
            .collect(),
        correlation_id: error.correlation_id.clone(),
        status: u32::from(error.status),
    }
}

pub(crate) fn error_from(message: PassError) -> core::PassError {
    core::PassError {
        status: u16::try_from(message.status)
            .ok()
            .filter(|status| (400..600).contains(status))
            .unwrap_or_else(|| core::status_of(&message.code)),
        code: message.code,
        message: message.message,
        field_errors: message
            .field_errors
            .into_iter()
            .map(|field| core::FieldError {
                field: field.field,
                message: field.message,
            })
            .collect(),
        correlation_id: message.correlation_id,
    }
}

pub(crate) fn result_message(reply: Reply) -> PassResult {
    match reply.result {
        Ok(result) => PassResult {
            result_json: result.to_string().into_bytes(),
            error: None,
            session_handle: reply.session_handle,
        },
        Err(error) => PassResult {
            result_json: Vec::new(),
            error: Some(error_message(&error)),
            session_handle: reply.session_handle,
        },
    }
}

/// A reply from its message: a result that is not JSON is the authority being unavailable.
pub(crate) fn reply_from(message: PassResult) -> Reply {
    let result = match message.error {
        Some(error) => Err(error_from(error)),
        None if message.result_json.is_empty() => Ok(serde_json::json!({})),
        None => serde_json::from_slice(&message.result_json)
            .map_err(|_| core::PassError::unavailable("the authority's answer is not JSON")),
    };
    Reply {
        result,
        session_handle: message.session_handle,
    }
}

/// A refusal after the check: not reached, as a direct client is `Client::Checked` by then.
fn refused(Refused(reason): Refused) -> Status {
    tracing::warn!("PASS authority: refused a direct call: {reason}");
    Status::unauthenticated("not a registered PASS client")
}

fn worker_failed(err: &moqui_core::context::Error) -> Status {
    Status::unavailable(err.message().to_string())
}

/// A download's metadata as its first part.
fn meta_message(meta: &core::DownloadMeta) -> DownloadMeta {
    let mut message = DownloadMeta {
        content_type: meta.content_type.clone(),
        filename: meta.filename.clone(),
        length: meta.length.unwrap_or_default(),
        disposition: meta.disposition.clone(),
        cache_control: meta.cache_control.clone().unwrap_or_default(),
        ..DownloadMeta::default()
    };
    if let Some(ranged) = meta.ranged {
        message.ranged = true;
        message.status = u32::from(ranged.status());
        match ranged {
            core::Ranged::Whole => {}
            core::Ranged::Part { start, end, complete } => {
                (message.range_start, message.range_end, message.complete_length) = (start, end, complete);
            }
            core::Ranged::Unsatisfiable { complete } => message.complete_length = complete,
        }
    }
    message
}

/// A download of `operation`'s metadata as it arrived: a range only as a status and bounds that agree, a `Cache-Control`
/// only as an exact value the operation may carry ([`core::download_cache_control`]).
pub(crate) fn meta_from(meta: DownloadMeta, operation: &str) -> core::DownloadMeta {
    let ranged = meta.ranged.then_some(match meta.status {
        206 if meta.range_start <= meta.range_end && meta.range_end < meta.complete_length => core::Ranged::Part {
            start: meta.range_start,
            end: meta.range_end,
            complete: meta.complete_length,
        },
        206 | 416 => core::Ranged::Unsatisfiable {
            complete: meta.complete_length,
        },
        _ => core::Ranged::Whole,
    });
    core::DownloadMeta {
        content_type: meta.content_type,
        filename: meta.filename,
        length: match ranged {
            Some(core::Ranged::Unsatisfiable { .. }) => Some(0),
            _ => (meta.length > 0).then_some(meta.length),
        },
        disposition: meta.disposition,
        ranged,
        cache_control: core::download_cache_control(operation, &meta.cache_control).map(str::to_string),
    }
}

type DownloadStream = Pin<Box<dyn Stream<Item = Result<DownloadPart, Status>> + Send>>;

/// The parts of a download as a stream, from a channel a reader fills.
fn stream_of(receiver: tokio::sync::mpsc::Receiver<Result<DownloadPart, Status>>) -> DownloadStream {
    Box::pin(futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|part| (part, receiver))
    }))
}

#[tonic::async_trait]
impl PassAuthority for Authority {
    async fn call(&self, request: Request<PassCall>) -> Result<Response<PassResult>, Status> {
        let message = request.get_ref();
        let client = self.client(request.metadata(), &message.client_id)?;
        let remote = self.client_ip(&request);
        let call = call_from(request.into_inner())?;
        let client = self.checked(client, remote).await?;
        let reply = crate::worker::on_pool(&self.factory, move |_| Ok(core::serve(&call, &client, None)))
            .await
            .map_err(|err| worker_failed(&err))?
            .map_err(refused)?;
        Ok(Response::new(result_message(reply)))
    }

    type DownloadStream = DownloadStream;

    async fn download(&self, request: Request<PassCall>) -> Result<Response<Self::DownloadStream>, Status> {
        let message = request.get_ref();
        let client = self.client(request.metadata(), &message.client_id)?;
        let remote = self.client_ip(&request);
        let call = call_from(request.into_inner())?;
        let client = self.checked(client, remote).await?;
        let served = crate::worker::on_pool(&self.factory, move |factory| {
            Ok(core::serve_download(factory, &call, &client))
        })
        .await
        .map_err(|err| worker_failed(&err))?
        .map_err(refused)?;
        let (sender, receiver) = tokio::sync::mpsc::channel(4);
        match served {
            Err(error) => {
                let _ = sender.try_send(Ok(DownloadPart {
                    part: Some(download_part::Part::Error(error_message(&error))),
                }));
            }
            Ok(served) => {
                let meta = meta_message(&served.meta);
                // the source is read a window at a time on a blocking thread, as a streamed response is
                tokio::task::spawn_blocking(move || {
                    let first = DownloadPart {
                        part: Some(download_part::Part::Meta(meta)),
                    };
                    if sender.blocking_send(Ok(first)).is_err() {
                        return;
                    }
                    // only the bytes sent: the whole file, or a range of it (Shop F88)
                    let end = served
                        .start
                        .saturating_add(served.meta.length.unwrap_or_default())
                        .min(served.source.len());
                    let mut offset = served.start;
                    while offset < end {
                        let window = usize::try_from(end - offset).map_or(CHUNK, |left| left.min(CHUNK));
                        match served.source.read_at(offset, window) {
                            Ok(bytes) if bytes.is_empty() => break,
                            Ok(bytes) => {
                                offset += bytes.len() as u64;
                                let part = DownloadPart {
                                    part: Some(download_part::Part::Chunk(bytes)),
                                };
                                if sender.blocking_send(Ok(part)).is_err() {
                                    return;
                                }
                            }
                            Err(err) => {
                                tracing::warn!("PASS authority: a download could not be read at {offset}: {err}");
                                let _ = sender.blocking_send(Err(Status::internal("the file could not be read")));
                                return;
                            }
                        }
                    }
                });
            }
        }
        Ok(Response::new(stream_of(receiver)))
    }

    async fn upload(&self, request: Request<Streaming<UploadPart>>) -> Result<Response<PassResult>, Status> {
        let metadata = request.metadata().clone();
        let remote = self.client_ip(&request);
        let mut parts = request.into_inner();
        let Some(upload_part::Part::Call(message)) = parts.message().await?.and_then(|part| part.part) else {
            return Err(Status::invalid_argument("an upload starts with its call"));
        };
        let client = self.client(&metadata, &message.client_id)?;
        let call = call_from(message)?;
        // the client before its file is read
        let client = self.checked(client, remote).await?;
        let Some(upload_part::Part::File(file)) = parts.message().await?.and_then(|part| part.part) else {
            return Err(Status::invalid_argument("an upload's file follows its call"));
        };
        let too_large = || {
            Ok(Response::new(result_message(Reply::error(core::PassError {
                code: "Unprocessable".to_string(),
                message: "Upload exceeds the size limit".to_string(),
                status: 422,
                ..core::PassError::default()
            }))))
        };
        if file.length > core::MAX_UPLOAD_BYTES {
            return too_large();
        }
        let mut bytes = Vec::with_capacity(usize::try_from(file.length).unwrap_or_default());
        while let Some(part) = parts.message().await? {
            let Some(upload_part::Part::Chunk(chunk)) = part.part else {
                return Err(Status::invalid_argument(
                    "an upload's file is followed only by its bytes",
                ));
            };
            if (bytes.len() + chunk.len()) as u64 > core::MAX_UPLOAD_BYTES {
                return too_large();
            }
            bytes.extend_from_slice(&chunk);
        }
        let upload = UploadFile {
            filename: file.filename,
            content_type: file.content_type,
            bytes,
        };
        let reply = crate::worker::on_pool(&self.factory, move |_| Ok(core::serve(&call, &client, Some(upload))))
            .await
            .map_err(|err| worker_failed(&err))?
            .map_err(refused)?;
        Ok(Response::new(result_message(reply)))
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
