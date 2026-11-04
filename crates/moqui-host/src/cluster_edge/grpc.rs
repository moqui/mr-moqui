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

//! The cluster over gRPC (plans/archive/Cluster.md): the `moqui.cluster.v1.Cluster` service the listener serves to a
//! peer whose TLS chose h2 ([`Server`]), and the sender that posts to peers that way ([`GrpcSender`]).
//!
//! A `Post` carries the messages the sender had waiting for one peer, in order, and is answered with a status for
//! each: the HTTP status `POST /cluster/{topic}` would give. The peer applies them one after another and stops at the
//! first it could not apply for now (5xx), answering 503 for those after it unapplied, so the sender sends them again
//! after it and in order ([`moqui_core::cluster::Sender::post_batch`]).
//!
//! The sender keeps one HTTP/2 connection to each peer, dialed again when it closes. A peer that does not choose h2 (a
//! member built before gRPC, or one with `cluster_protocol=rest`) is posted to over HTTP/1.1 instead, and dialed for
//! gRPC again after [`REST_RECHECK`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_core::cluster::{BatchOutcome, PostOutcome, Sender, MAX_CLUSTER_BODY};
use moqui_lib::sync::lock;
use tower::ServiceExt as _;

use super::listen::Apply;
use super::proto::cluster_client::ClusterClient;
use super::proto::cluster_server::{Cluster, ClusterServer};
use super::proto::{ClusterMessage, PostReply, PostRequest};
use super::{CoreError, CLIENT_IO};
use crate::grpc_client::{Dial, DialError, H2Channel, H2Send};

/// The path prefix of the service's calls.
const PATH_PREFIX: &str = "/moqui.cluster.v1.Cluster/";
/// The largest request either end takes: a batch is split so its payloads stay under [`MAX_CLUSTER_BODY`], and this
/// leaves room for the protobuf framing around them.
const MAX_MESSAGE: usize = MAX_CLUSTER_BODY + 1024 * 1024;
/// How long a peer that did not choose h2 is posted to over HTTP/1.1 before gRPC is tried again.
pub(super) const REST_RECHECK: Duration = Duration::from_secs(300);
/// HTTP/2 PINGs on a kept connection, so one to a peer that went away is closed and dialed again.
const PING_EVERY: Duration = Duration::from_secs(15);
const PING_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether `request` is a call of the `Cluster` service.
pub(super) fn is_call<B>(request: &http::Request<B>) -> bool {
    let grpc = request
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/grpc"));
    grpc && request.uri().path().starts_with(PATH_PREFIX)
}

/// The listener's `Cluster` service.
pub(super) struct Server(ClusterServer<Service>);

impl Server {
    pub(super) fn new(apply: Apply) -> Self {
        Self(
            ClusterServer::new(Service { apply })
                .max_decoding_message_size(MAX_MESSAGE)
                .max_encoding_message_size(MAX_MESSAGE),
        )
    }

    pub(super) async fn call(&self, request: http::Request<hyper::body::Incoming>) -> http::Response<axum::body::Body> {
        match self.0.clone().oneshot(request).await {
            Ok(response) => response.map(axum::body::Body::new),
            Err(never) => match never {},
        }
    }
}

struct Service {
    apply: Apply,
}

#[tonic::async_trait]
impl Cluster for Service {
    async fn post(&self, request: tonic::Request<PostRequest>) -> Result<tonic::Response<PostReply>, tonic::Status> {
        let messages = request.into_inner().messages;
        let apply = Arc::clone(&self.apply);
        let status = tokio::task::spawn_blocking(move || apply_in_order(&apply, &messages))
            .await
            .map_err(|_| tonic::Status::internal("applying the cluster messages did not finish"))?;
        Ok(tonic::Response::new(PostReply { status }))
    }
}

/// Apply each message in order, stopping at the first that was not applied for now (5xx): it and those after it are
/// answered with its status and 503, so the sender sends them again in order.
pub(super) fn apply_in_order(apply: &Apply, messages: &[ClusterMessage]) -> Vec<u32> {
    let mut status = Vec::with_capacity(messages.len());
    for message in messages {
        let code = apply(&message.topic, &message.payload);
        status.push(u32::from(code));
        if code >= 500 {
            break;
        }
    }
    status.resize(messages.len(), 503);
    status
}

/// What the sender knows of a peer.
enum Peer {
    /// A kept HTTP/2 connection.
    H2(H2Send),
    /// The peer did not choose h2: HTTP/1.1 until then.
    Rest(Instant),
}

/// The cluster's sender over gRPC, with `rest` for a peer that does not choose h2.
pub(super) struct GrpcSender {
    /// The cluster's client settings, with the ALPN `h2, http/1.1`.
    config: Arc<rustls::ClientConfig>,
    rest: Arc<dyn Sender>,
    peers: Arc<Mutex<HashMap<String, Peer>>>,
}

impl GrpcSender {
    pub(super) fn new(config: Arc<rustls::ClientConfig>, rest: Arc<dyn Sender>) -> Self {
        Self {
            config,
            rest,
            peers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// True while `base_url` is posted to over HTTP/1.1.
    fn over_rest(&self, base_url: &str) -> bool {
        let mut peers = lock(&self.peers);
        match peers.get(base_url) {
            Some(Peer::Rest(until)) if Instant::now() < *until => true,
            Some(Peer::Rest(_)) => {
                peers.remove(base_url);
                false
            }
            _ => false,
        }
    }
}

impl Sender for GrpcSender {
    fn post(&self, base_url: &str, topic: &str, payload: &[u8]) -> Result<PostOutcome, CoreError> {
        let BatchOutcome { mut outcomes, failed } = self.post_batch(base_url, &[(topic, payload)]);
        match (outcomes.pop(), failed) {
            (Some(outcome), _) => Ok(outcome),
            (None, Some(err)) => Err(err),
            (None, None) => Err(CoreError::new(format!("cluster peer {base_url} gave no answer"))),
        }
    }

    fn post_batch(&self, base_url: &str, messages: &[(&str, &[u8])]) -> BatchOutcome {
        if messages.is_empty() {
            return BatchOutcome {
                outcomes: Vec::new(),
                failed: None,
            };
        }
        if self.over_rest(base_url) {
            return self.rest.post_batch(base_url, messages);
        }
        let requests = split(messages);
        let call = post_all(
            Arc::clone(&self.peers),
            Arc::clone(&self.config),
            base_url.to_string(),
            requests,
        );
        match crate::edge::block_on(call) {
            Ok(Sent::Batch(outcome)) => outcome,
            Ok(Sent::NotH2) => {
                tracing::info!("cluster peer {base_url} does not take gRPC; posting to it over HTTP/1.1");
                lock(&self.peers).insert(base_url.to_string(), Peer::Rest(Instant::now() + REST_RECHECK));
                self.rest.post_batch(base_url, messages)
            }
            Err(err) => BatchOutcome {
                outcomes: Vec::new(),
                failed: Some(CoreError::new(format!("cluster post to {base_url} did not run: {err}"))),
            },
        }
    }
}

/// `messages` as requests whose payloads together stay under [`MAX_CLUSTER_BODY`], in order.
fn split(messages: &[(&str, &[u8])]) -> Vec<PostRequest> {
    let mut requests = Vec::new();
    let mut current = PostRequest::default();
    let mut size = 0;
    for (topic, payload) in messages {
        if !current.messages.is_empty() && size + payload.len() > MAX_CLUSTER_BODY {
            requests.push(std::mem::take(&mut current));
            size = 0;
        }
        size += payload.len();
        current.messages.push(ClusterMessage {
            topic: (*topic).to_string(),
            payload: payload.to_vec(),
        });
    }
    if !current.messages.is_empty() {
        requests.push(current);
    }
    requests
}

enum Sent {
    Batch(BatchOutcome),
    /// TLS did not choose h2: post over HTTP/1.1.
    NotH2,
}

/// Send each of `requests` to `base_url` in order, stopping at the first message that was not applied.
async fn post_all(
    peers: Arc<Mutex<HashMap<String, Peer>>>,
    config: Arc<rustls::ClientConfig>,
    base_url: String,
    requests: Vec<PostRequest>,
) -> Sent {
    let failed = |message: String| {
        Sent::Batch(BatchOutcome {
            outcomes: Vec::new(),
            failed: Some(CoreError::new(message)),
        })
    };
    let (host, port) = match super::split_base(&base_url) {
        Ok(place) => place,
        Err(err) => return failed(err.to_string()),
    };
    let send = match connection(&peers, config, &base_url, &host, port).await {
        Ok(send) => send,
        Err(DialError::NotH2) => return Sent::NotH2,
        Err(DialError::Failed(message)) => return failed(message),
    };
    let Ok(origin) = format!("https://{host}:{port}").parse::<http::Uri>() else {
        return failed(format!("cluster base url {base_url} is not a URI"));
    };
    let mut client = ClusterClient::with_origin(H2Channel(send), origin)
        .max_decoding_message_size(MAX_MESSAGE)
        .max_encoding_message_size(MAX_MESSAGE);
    let mut outcomes = Vec::new();
    for request in requests {
        let count = request.messages.len();
        let answer = match tokio::time::timeout(CLIENT_IO, client.post(request)).await {
            Ok(Ok(answer)) => answer.into_inner(),
            Ok(Err(status)) => {
                forget(&peers, &base_url);
                return Sent::Batch(BatchOutcome {
                    outcomes,
                    failed: Some(CoreError::new(format!(
                        "cluster post to {base_url} failed: {}",
                        status.message()
                    ))),
                });
            }
            Err(_) => {
                forget(&peers, &base_url);
                return Sent::Batch(BatchOutcome {
                    outcomes,
                    failed: Some(CoreError::new(format!("cluster post to {base_url} timed out"))),
                });
            }
        };
        for code in answer.status.iter().take(count) {
            match PostOutcome::from_status(u16::try_from(*code).unwrap_or(500)) {
                Ok(outcome) => outcomes.push(outcome),
                Err(err) => {
                    return Sent::Batch(BatchOutcome {
                        outcomes,
                        failed: Some(err),
                    })
                }
            }
        }
        if answer.status.len() < count {
            return Sent::Batch(BatchOutcome {
                outcomes,
                failed: Some(CoreError::new(format!(
                    "cluster peer {base_url} answered for {} of {count} messages",
                    answer.status.len()
                ))),
            });
        }
    }
    Sent::Batch(BatchOutcome { outcomes, failed: None })
}

/// The kept connection to `base_url`, or a new one, which is kept.
async fn connection(
    peers: &Mutex<HashMap<String, Peer>>,
    config: Arc<rustls::ClientConfig>,
    base_url: &str,
    host: &str,
    port: u16,
) -> Result<H2Send, DialError> {
    let kept = match lock(peers).get(base_url) {
        Some(Peer::H2(send)) if !send.is_closed() => Some(send.clone()),
        _ => None,
    };
    if let Some(send) = kept {
        return Ok(send);
    }
    let name = super::server_name(host).map_err(|err| DialError::Failed(err.to_string()))?;
    let send = crate::grpc_client::dial(Dial {
        what: "cluster peer",
        host,
        port,
        tls: Some((config, name)),
        timeout: CLIENT_IO,
        keep_alive: PING_EVERY,
        keep_alive_timeout: PING_TIMEOUT,
    })
    .await?;
    lock(peers).insert(base_url.to_string(), Peer::H2(send.clone()));
    Ok(send)
}

/// Drop the kept connection to `base_url` after a failed call, so the next post dials again.
fn forget(peers: &Mutex<HashMap<String, Peer>>, base_url: &str) {
    let mut peers = lock(peers);
    if matches!(peers.get(base_url), Some(Peer::H2(_))) {
        peers.remove(base_url);
    }
}
