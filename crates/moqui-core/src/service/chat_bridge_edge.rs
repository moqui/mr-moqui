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

//! Chat bridges and SMS through an edge node (plans/archive/Edge-Node.md §5.4, E6): the link's queue carries a network's
//! webhooks to the local node, and the files of the messages the local node sends by address to the edge node.
//!
//! - **Webhooks** (kind [`KIND_WEBHOOK`], edge node to local node). On the edge node [`webhook`] reads the bridge from
//!   a local node's replicated set [`REPLICA_SET`]: by the bridge id within the request's host name
//!   (plans/Edge-Node2.md §4.1), since ids come from separate databases and two pairings may have the same one. A host
//!   name of a pairing (its `EdgeHost`, where the edge node answers the bridges that pairing routes through it) picks
//!   that pairing's copy only; at the edge node's own names the id must be one pairing's. The copy holds the bridge's
//!   network, adapter, status and the webhook's public key, never its `configJson`, whose provider keys stay on the
//!   local node. The edge node checks the network's signature with the adapter's
//!   [`verify_webhook`](super::ChatBridgeAdapter::verify_webhook), and queues the raw request with the path and query it
//!   came to, keyed by the network's event id ([`webhook_event_id`](super::ChatBridgeAdapter::webhook_event_id)) so a
//!   repeat is dropped, and ordered by their number. It answers 200 once the item is committed; a bad signature, an
//!   unknown bridge and a disabled one answer as on any node, and the network's answers are in its shape
//!   ([`edge_answer`](super::ChatBridgeAdapter::edge_answer)). The local node's `apply#EdgeWebhook` rebuilds the request
//!   (checked at the time it came: `receivedMillis`) and checks its signature again with the bridge's whole
//!   configuration, whichever node checked it before (an adapter changed since the edge node queued it is never applied
//!   unchecked), then hands it to the adapter's [`apply_webhook`](super::ChatBridgeAdapter::apply_webhook) with the
//!   transport-verified mark set.
//! - **Verified on the local node only.** A network that signs with a secret (Twilio's auth token, Sinch's webhook
//!   secret) cannot be checked on the edge node, which never holds one ([`WebhookVerification::Core`]). For its adapter
//!   the edge node checks only the method, the size (`channels/edge-limits@unverified-max-bytes`, 32 KiB), that the signature
//!   header is there, a rate a client address (`channels/edge-limits@webhooks-per-client-minute`, 120) and a bridge
//!   (`channels/edge-limits@webhooks-per-minute`, 600; token buckets on each edge node), and how many of the bridge's items
//!   wait (`channels/edge-limits@unverified-waiting`, 500, so at most 16 MiB), then reads the event key from the unverified
//!   body and queues the request, its idempotency key including a hash of the signature so a forged item cannot take a
//!   real event's key ([`UnverifiedLimits`]). On the local node a request this node cannot check (no secret) waits
//!   (`retryLater`); one whose signature is not the network's is refused and kept for 30 days in
//!   [`REFUSED`] (`replay#RefusedWebhooks` after a configuration fix), counted ([`refused_webhooks`], `find#Bridges`)
//!   and logged once per bridge every ten minutes, the item answered as done so it holds nothing behind it.
//! - **Files** (kind [`KIND_MEDIA`], local node to edge node). A paired local node whose conf lists `chatMedia` sends
//!   every file of a message that goes by address (an MMS picture) through its edge node ([`media_urls`]): it queues
//!   the file as the item's content, with a token it makes (an HMAC under `channels@media-key`, so a later attempt of
//!   the same send names the same file), and does not send until the item is delivered, so the network never fetches
//!   an address the edge node does not have yet: while the link is up it waits up to [`MEDIA_WAIT`], else the send
//!   waits [`MEDIA_RETRY_MILLIS`] and is tried again, its attempt not counted (`DeliveryOutcome::Wait`). The address is
//!   under `channels@public-url`, which on a paired node names the edge node. The edge node's `store#EdgeMedia` keeps
//!   the file as a `moqui.edge.EdgeMedia` row by the token's hash, within its own limits (`channels@media-minutes`,
//!   `channels/edge-limits@media-max-bytes`, `channels/edge-limits@media-node-megabytes`), and [`media`] serves it until it runs
//!   out; [`sweep_edge_media`] (the link's retention pass, and each store) removes the files that ran out.
//! - **Keys of older bridges.** [`fill_webhook_keys`], at the delivery worker's start, copies a bridge's `configJson`
//!   `publicKey` and `webhookToleranceSeconds` to the replicated fields of a bridge saved before E6.
//!
//! Each handler checks the node's role: the local node does not keep files for networks, and the edge node does not
//! apply webhooks.

use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::time::{Duration, Instant};

use base64::Engine;
use hmac::{KeyInit, Mac};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use sha2::Digest;

use super::web::{marked, media_key, media_minutes, public_base, safe_type};
use super::{
    adapter_of, bridge_info, current_factory, param, text_of, BridgeInfo, DeliveryRequest, MediaFile, WebhookRequest,
    WebhookResponse, WebhookVerification, BRIDGE,
};
use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::edge::queue::{self, NewItem};
use crate::entity::{Condition, EntityFacade};
use crate::transaction::TransactionAttribute;
use crate::user::{now_millis, stamp_value, value_millis};

pub(super) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, apply_edge_webhook)?;
    rust_service!(map, replay_refused_webhooks)?;
    rust_service!(map, store_edge_media)?;
    Ok(())
}

/// The queue kind of a network's webhook request the edge node verified, for the local node.
pub const KIND_WEBHOOK: &str = "chatWebhook";
/// The queue kind of a file of a message the local node sends by address, for the edge node to serve.
pub const KIND_MEDIA: &str = "chatMedia";
/// The replicated set the edge node checks a webhook's signature with.
pub const REPLICA_SET: &str = "chatBridges";
/// The edge node's files, by paired node and the token's hash.
const MEDIA_ROOT: &str = "dbresource://moqui/edge/media/";
const EDGE_MEDIA: &str = "moqui.edge.EdgeMedia";
/// How long a send waits for its files to reach the edge node, while the link is up, before it is tried again later.
const MEDIA_WAIT: Duration = Duration::from_secs(2);
/// How long a send whose files are not on the edge node yet waits before it is tried again (not counted).
pub const MEDIA_RETRY_MILLIS: i64 = 5_000;
/// The largest file the edge node keeps: `channels/edge-limits@media-max-bytes`, 5 MiB by default (a provider's MMS limit is
/// about 1 MiB in all).
const MEDIA_MAX_BYTES: &str = "channels/edge-limits@media-max-bytes";
const DEFAULT_MEDIA_MAX_BYTES: i64 = 5 * 1024 * 1024;
/// The files of one local node the edge node keeps at most: `channels/edge-limits@media-node-megabytes`, 256 by default.
const MEDIA_NODE_MEGABYTES: &str = "channels/edge-limits@media-node-megabytes";
const DEFAULT_MEDIA_NODE_MEGABYTES: i64 = 256;
/// The expired files one sweep removes at most.
const EXPIRED_PAGE: i64 = 500;
/// Headers of a webhook request that are not kept with the queued request.
const DROPPED_HEADERS: &[&str] = &["cookie", "authorization", "proxy-authorization"];
/// The host's bound of a webhook body (`crates/moqui-host/src/channels.rs`), checked again before a request is queued.
const WEBHOOK_MAX_BYTES: &str = "channels@webhook-max-bytes";
const DEFAULT_WEBHOOK_MAX_BYTES: usize = 256 * 1024;
/// The requests a minute an edge node queues for one bridge whose webhooks it cannot verify, in all and from one client
/// address; the largest such body; and the most of the bridge's items that wait unverified ([`UnverifiedLimits`]).
const WEBHOOKS_PER_MINUTE: &str = "channels/edge-limits@webhooks-per-minute";
const DEFAULT_WEBHOOKS_PER_MINUTE: u32 = 600;
const WEBHOOKS_PER_CLIENT_MINUTE: &str = "channels/edge-limits@webhooks-per-client-minute";
const DEFAULT_WEBHOOKS_PER_CLIENT_MINUTE: u32 = 120;
const UNVERIFIED_MAX_BYTES: &str = "channels/edge-limits@unverified-max-bytes";
const DEFAULT_UNVERIFIED_MAX_BYTES: usize = 32 * 1024;
const UNVERIFIED_WAITING: &str = "channels/edge-limits@unverified-waiting";
const DEFAULT_UNVERIFIED_WAITING: i64 = 500;

fn sha256_hex(text: &str) -> String {
    moqui_lib::codec::hex_encode(&sha2::Sha256::digest(text.as_bytes()))
}

fn transaction<T>(
    facade: &EntityFacade,
    entity: &str,
    attribute: TransactionAttribute,
    body: impl FnOnce() -> Result<T, LibError>,
) -> Result<T, LibError> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    facade
        .transactions_for(entity)?
        .run(attribute, Some(Duration::from_secs(30)), body)
}

/// The result of a handler that cannot apply its item now for a reason that is not the item's: the queue rolls it
/// back and tries it again, not counted (`crate::edge::queue::apply`).
fn retry_later(reason: String) -> BTreeMap<String, Value> {
    tracing::warn!("{reason}; the item waits for another try");
    BTreeMap::from([("retryLater".to_string(), Value::string(reason))])
}

// ---- the edge node: webhooks in ----

/// The pairing a request's host name names: the node of its `moqui.edge.EdgeHost`, else `None` (the edge node's own
/// names, or a request with no host).
pub(super) fn host_pairing(facade: &EntityFacade, request: &WebhookRequest) -> Result<Option<String>, LibError> {
    host_name_pairing(facade, request.header("host"))
}

/// The pairing host name `host` (a `Host` header) names: the node of its `moqui.edge.EdgeHost`, else `None`.
fn host_name_pairing(facade: &EntityFacade, host: Option<&str>) -> Result<Option<String>, LibError> {
    let Some(host) = host.and_then(normal_host) else {
        return Ok(None);
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    Ok(facade
        .find("moqui.edge.EdgeHost")?
        .eq("hostName", Value::string(&host))
        .select_field("pairedNodeId")
        .disable_authz()
        .one()?
        .and_then(|row| text_of(&row, "pairedNodeId")))
}

/// A `Host` header's name as `EdgeHost` keeps it: lower case, with no port or trailing dot.
fn normal_host(host: &str) -> Option<String> {
    let host = host.trim();
    let host = match host.rsplit_once(':') {
        Some((name, port)) if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) => name,
        _ => host,
    };
    let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// The bridge `bridge_id` as a paired local node replicated it: the node and the set's fields. Looked up in the copies
/// of pairing `only` when the request's host name names one, else of every active pairing. None when no active node
/// has it, or (with no host naming a pairing) more than one does: ids of separate databases can collide, and then
/// each pairing's webhooks must come to its own host name (plans/Edge-Node2.md §4.1).
fn replicated_bridge(
    facade: &EntityFacade,
    bridge_id: &str,
    only: Option<&str>,
) -> Result<Option<(String, serde_json::Value)>, LibError> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut nodes = crate::edge::claims::active_nodes(facade)?;
    if let Some(only) = only {
        nodes.retain(|node| node == only);
    }
    let key = BTreeMap::from([("chatBridgeId".to_string(), bridge_id.to_string())]);
    let mut found = Vec::new();
    for node in nodes {
        if let Some(value) = crate::edge::replica::get_value(facade, Some(&node), REPLICA_SET, BRIDGE, &key)? {
            found.push((node, value.fields));
        }
    }
    if found.len() > 1 {
        let holders: Vec<&str> = found.iter().map(|(node, _)| node.as_str()).collect();
        tracing::error!(
            "chat bridge {bridge_id}: local nodes {} each replicated it, so its webhooks at this node's own names are \
             refused: each node's network must post to that node's own host name",
            holders.join(", ")
        );
        return Ok(None);
    }
    Ok(found.pop())
}

fn field(fields: &serde_json::Value, name: &str) -> String {
    fields[name].as_str().map(str::trim).unwrap_or_default().to_string()
}

fn number_field(fields: &serde_json::Value, name: &str) -> Option<i64> {
    let value = &fields[name];
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|number| number as i64))
        .or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<f64>().ok())
                .map(|n| n as i64)
        })
        .filter(|seconds| *seconds > 0)
}

/// Who answers a chat bridge's webhooks and serves its files on an all-in-one node (plans/archive/Edge-Node.md §10).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum BridgeOwner {
    /// The server's own `ChatBridge`, and no paired node's copy: the normal path.
    Own,
    /// Only a paired node's copy: its node and fields, the edge path.
    Paired(String, serde_json::Value),
    /// Both sides have the id: refused, as a claim conflict, naming the paired node.
    Both(String),
    /// Neither.
    None,
}

/// [`BridgeOwner`] from what each side has.
pub(super) fn owner_of(own: bool, paired: Option<(String, serde_json::Value)>) -> BridgeOwner {
    match (own, paired) {
        (true, None) => BridgeOwner::Own,
        (false, Some((node, fields))) => BridgeOwner::Paired(node, fields),
        (true, Some((node, _))) => BridgeOwner::Both(node),
        (false, None) => BridgeOwner::None,
    }
}

/// On an all-in-one node: who has bridge `bridge_id`, given whether the server has its own row. The replica is read
/// only then, as a dedicated edge node reads it. A request whose host name is a pairing's goes to that pairing's copy
/// alone, the server's own row aside (plans/Edge-Node2.md §4.1).
///
/// # Errors
///
/// Returns the database's error.
pub(super) fn bridge_owner(
    facade: &EntityFacade,
    bridge_id: &str,
    own: bool,
    request: &WebhookRequest,
) -> Result<BridgeOwner, LibError> {
    match host_pairing(facade, request)? {
        Some(node) => Ok(owner_of(false, replicated_bridge(facade, bridge_id, Some(&node))?)),
        None => Ok(owner_of(own, replicated_bridge(facade, bridge_id, None)?)),
    }
}

/// The answer to a webhook of a bridge both the server and paired node `node` have: refused, logged as the claim
/// conflict it is (the server's own setup and the replication refuse such an id, so it is one made before either).
pub(super) fn both_refused(bridge_id: &str, node: &str) -> WebhookResponse {
    tracing::error!(
        "chat bridge {bridge_id} is both the server's own ChatBridge and one paired node {node} replicated; its \
         webhooks are refused until one side removes it"
    );
    WebhookResponse::text(
        409,
        &format!("chat bridge {bridge_id} is held by both the server and a paired node"),
    )
}

/// A request to `/channels/{chatBridgeId}/webhook` on an edge node. See the module notes.
pub(super) fn webhook(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    bridge_id: &str,
    request: &WebhookRequest,
) -> WebhookResponse {
    let found = host_pairing(facade, request).and_then(|only| replicated_bridge(facade, bridge_id, only.as_deref()));
    match found {
        Ok(Some((node, fields))) => paired_webhook(factory, facade, bridge_id, request, &node, &fields),
        Ok(None) => WebhookResponse::not_signed(),
        Err(error) => {
            tracing::error!("chat bridge {bridge_id}: the edge node could not read its replicated bridge: {error}");
            WebhookResponse::text(503, "try again later")
        }
    }
}

/// The replicated copy's webhook key as the configuration an adapter that verifies on the edge node reads: only what
/// the signature check needs (`publicKey`, `webhookToleranceSeconds`); the provider's keys stay on the local node.
fn edge_config(bridge_id: &str, node: &str, fields: &serde_json::Value) -> serde_json::Value {
    let public_key = field(fields, "webhookPublicKey");
    if public_key.is_empty() {
        tracing::error!(
            "chat bridge {bridge_id} of local node {node} has no webhookPublicKey in its replicated copy, so its \
             webhooks are refused: set publicKey in its configuration on the local node"
        );
    }
    let mut config = serde_json::json!({});
    if !public_key.is_empty() {
        config["publicKey"] = serde_json::Value::String(public_key);
    }
    if let Some(tolerance) = number_field(fields, "webhookToleranceSeconds") {
        config["webhookToleranceSeconds"] = serde_json::Value::from(tolerance);
    }
    config
}

/// A webhook of bridge `bridge_id` as paired node `node` replicated it (`fields`): its signature checked here, or (an
/// adapter that verifies on the local node) the cheap checks of [`deferred_checks`] and the cap on what waits
/// unverified, and the request queued for the node. The local node checks the signature again, whichever way.
pub(super) fn paired_webhook(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    bridge_id: &str,
    request: &WebhookRequest,
    node: &str,
    fields: &serde_json::Value,
) -> WebhookResponse {
    paired_webhook_within(
        factory,
        facade,
        bridge_id,
        request,
        node,
        fields,
        &UnverifiedLimits::of_properties(),
    )
}

/// [`paired_webhook`] within `limits` (a test's own).
pub(super) fn paired_webhook_within(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    bridge_id: &str,
    request: &WebhookRequest,
    node: &str,
    fields: &serde_json::Value,
    limits: &UnverifiedLimits,
) -> WebhookResponse {
    let mut info = BridgeInfo {
        chat_bridge_id: bridge_id.to_string(),
        type_enum_id: field(fields, "typeEnumId"),
        adapter_name: field(fields, "adapterName"),
        name: String::new(),
        config: serde_json::json!({}),
    };
    let Some(adapter) = adapter_of(factory, facade, &info) else {
        return WebhookResponse::not_signed();
    };
    if !request.method.eq_ignore_ascii_case("POST") {
        return WebhookResponse::text(405, "a webhook is a POST");
    }
    let signed_by = match adapter.webhook_verification() {
        WebhookVerification::Edge => {
            info.config = edge_config(bridge_id, node, fields);
            match std::panic::catch_unwind(AssertUnwindSafe(|| adapter.verify_webhook(&info, request))) {
                Ok(None) => None,
                Ok(Some(refused)) => return refused,
                Err(_) => return WebhookResponse::not_signed(),
            }
        }
        WebhookVerification::Core { signature_header } => {
            // nothing here proves the sender, so a disabled bridge is answered as one that does not exist, before it
            // spends a rate or tells that it exists (SEC-COM-89)
            if field(fields, "statusId") == "CbsDisabled" {
                return WebhookResponse::not_signed();
            }
            let rates = factory.extension(|_| EdgeRates::default());
            let key = format!("{node}:{bridge_id}");
            if let Some(refused) = deferred_checks(&rates, &key, request, signature_header, limits) {
                return shaped(adapter.as_ref(), &info, refused);
            }
            match unverified_waiting(facade, node, bridge_id) {
                Ok(waiting) if waiting >= limits.waiting => {
                    tracing::warn!(
                        "chat bridge {bridge_id} of local node {node}: {waiting} unverified webhooks wait for it, the \
                         most it holds (channels/edge-limits@unverified-waiting); more are refused (429) until they go"
                    );
                    return shaped(
                        adapter.as_ref(),
                        &info,
                        WebhookResponse::text(429, "too many requests wait; try again later"),
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::error!("chat bridge {bridge_id}: the webhooks waiting for it were not counted: {error}");
                    return WebhookResponse::text(503, "try again later");
                }
            }
            request.header(signature_header)
        }
    };
    if field(fields, "statusId") == "CbsDisabled" {
        return WebhookResponse::text(503, "the bridge is disabled");
    }
    // in Core mode read from a body nobody verified yet: bounded by the size cap above, and only a key
    let Ok(Some(key)) = std::panic::catch_unwind(AssertUnwindSafe(|| adapter.webhook_event_id(&info, request))) else {
        return WebhookResponse::text(400, "the request names no event");
    };
    let item = webhook_item(bridge_id, &key, request, signed_by);
    match queue::enqueue(factory, facade, Some(node), &item) {
        Ok(queued) => {
            let outcome = if queued.created { "queued" } else { "duplicate" };
            tracing::info!(
                "chat bridge {bridge_id}: webhook event {} {outcome} for local node {node} as item {}",
                key.event_id,
                queued.queue_item_id
            );
            shaped(
                adapter.as_ref(),
                &info,
                WebhookResponse::json(200, &serde_json::json!({ "outcome": outcome })),
            )
        }
        Err(error) => {
            // not queued: the network tries again
            tracing::error!(
                "chat bridge {bridge_id}: webhook event {} was not queued: {error}",
                key.event_id
            );
            WebhookResponse::text(503, "try again later")
        }
    }
}

/// The network's answer in its own shape ([`super::ChatBridgeAdapter::edge_answer`]): a queued request's and a rate
/// refusal's, never an unsigned one's.
fn shaped(adapter: &dyn super::ChatBridgeAdapter, info: &BridgeInfo, response: WebhookResponse) -> WebhookResponse {
    if (200..300).contains(&response.status) || response.status == 429 {
        std::panic::catch_unwind(AssertUnwindSafe(|| adapter.edge_answer(info, response.clone()))).unwrap_or(response)
    } else {
        response
    }
}

/// A positive number from a property, else `default`.
fn positive_property<T: std::str::FromStr + PartialOrd + Default>(name: &str, default: T) -> T {
    crate::context::conf_env::setting_or_property(name)
        .trim()
        .parse::<T>()
        .ok()
        .filter(|limit| *limit > T::default())
        .unwrap_or(default)
}

/// What an edge node lets through, for a bridge whose webhooks it cannot verify ([`WebhookVerification::Core`]).
#[derive(Clone, Debug)]
pub(super) struct UnverifiedLimits {
    /// The largest body: `channels/edge-limits@unverified-max-bytes` (32 KiB), at most `channels@webhook-max-bytes`.
    pub max_bytes: usize,
    /// Requests a minute for the bridge: `channels/edge-limits@webhooks-per-minute` (600).
    pub per_minute: u32,
    /// Requests a minute for the bridge from one client address: `channels/edge-limits@webhooks-per-client-minute` (120).
    pub per_client_minute: u32,
    /// Its items waiting in the queue, unverified: `channels/edge-limits@unverified-waiting` (500). The bytes waiting are at
    /// most this many times `max_bytes` (16 MiB by default).
    pub waiting: i64,
}

impl UnverifiedLimits {
    fn of_properties() -> Self {
        Self {
            max_bytes: positive_property(UNVERIFIED_MAX_BYTES, DEFAULT_UNVERIFIED_MAX_BYTES)
                .min(positive_property(WEBHOOK_MAX_BYTES, DEFAULT_WEBHOOK_MAX_BYTES)),
            per_minute: positive_property(WEBHOOKS_PER_MINUTE, DEFAULT_WEBHOOKS_PER_MINUTE),
            per_client_minute: positive_property(WEBHOOKS_PER_CLIENT_MINUTE, DEFAULT_WEBHOOKS_PER_CLIENT_MINUTE),
            waiting: positive_property(UNVERIFIED_WAITING, DEFAULT_UNVERIFIED_WAITING),
        }
    }
}

/// A token bucket: `per_minute` requests a minute, as a burst or spread out.
#[derive(Clone, Copy, Debug)]
pub(super) struct Bucket {
    tokens: f64,
    at: i64,
}

impl Bucket {
    fn full(per_minute: u32, now: i64) -> Self {
        Self {
            tokens: f64::from(per_minute),
            at: now,
        }
    }

    /// Take one request at `now`, the bucket refilled since its last take at `per_minute` a minute, up to
    /// `per_minute`: whether one was there.
    pub(super) fn take(&mut self, per_minute: u32, now: i64) -> bool {
        let capacity = f64::from(per_minute);
        let elapsed = now.saturating_sub(self.at).max(0) as f64;
        self.tokens = (self.tokens + elapsed * capacity / 60_000.0).min(capacity);
        self.at = now.max(self.at);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// The most buckets an edge node keeps; past it the idle ones (a minute) go, and a new client is refused while none is.
const MAX_BUCKETS: usize = 10_000;

/// The edge node's buckets of the bridges whose webhooks it queues unverified: by node and bridge, and by node, bridge
/// and client address.
#[derive(Default)]
pub(super) struct EdgeRates {
    buckets: std::sync::Mutex<std::collections::HashMap<String, Bucket>>,
}

impl EdgeRates {
    /// Take one request of bridge `key` and of its `client` at `now`, at `per_minute` and `per_client_minute`:
    /// whether it may go. Both are taken or neither: the bridge's bucket is asked first, so while it is empty no
    /// client bucket is made (many source addresses cannot fill the table), and a client over its share spends
    /// nothing of the bridge's (SEC-COM-89).
    fn take(&self, bridge: Rate<'_>, client: Rate<'_>, now: i64) -> bool {
        let mut buckets = moqui_lib::sync::lock(&self.buckets);
        for Rate { key: name, per_minute } in [bridge, client] {
            if !buckets.contains_key(name) && buckets.len() >= MAX_BUCKETS {
                buckets.retain(|_, bucket| now.saturating_sub(bucket.at) < 60_000);
                if buckets.len() >= MAX_BUCKETS {
                    return false;
                }
            }
            let bucket = buckets
                .entry(name.to_string())
                .or_insert_with(|| Bucket::full(per_minute, now));
            // refilled to now, and asked without taking
            if !bucket.take(per_minute, now) {
                return false;
            }
            bucket.tokens += 1.0;
        }
        for name in [bridge.key, client.key] {
            if let Some(bucket) = buckets.get_mut(name) {
                bucket.tokens -= 1.0;
            }
        }
        true
    }
}

/// A bucket's key and its rate a minute, named so a bridge's and a client's cannot be passed the wrong way round.
#[derive(Clone, Copy)]
struct Rate<'a> {
    key: &'a str,
    per_minute: u32,
}

/// What an edge node checks of a webhook it cannot verify (the network signs with a secret only the local node holds,
/// [`WebhookVerification::Core`]), before it queues it: the size bound (413), the signature header there (else the
/// answer of an unsigned request, counted by web protection), and the rates: the bridge's (`key`, its node and id) and
/// the client address's share of it, both or neither, so one address cannot spend the bridge's (429, so the network
/// tries again). None when it may be queued. The local node verifies it before applying.
pub(super) fn deferred_checks(
    rates: &EdgeRates,
    key: &str,
    request: &WebhookRequest,
    signature_header: &str,
    limits: &UnverifiedLimits,
) -> Option<WebhookResponse> {
    if request.body.len() > limits.max_bytes {
        return Some(WebhookResponse::text(
            413,
            &format!("the request is over {} bytes", limits.max_bytes),
        ));
    }
    if request
        .header(signature_header)
        .is_none_or(|value| value.trim().is_empty())
    {
        return Some(WebhookResponse::not_signed());
    }
    let now = request.now_millis;
    let client = format!("{key}|{}", request.client_ip);
    let bridge = Rate {
        key,
        per_minute: limits.per_minute,
    };
    let from = Rate {
        key: &client,
        per_minute: limits.per_client_minute,
    };
    if !rates.take(bridge, from, now) {
        // not logged at warn: a flood would flood the log too
        tracing::debug!(
            "chat bridge {key}: webhooks over its rate from {}; refused for now (429)",
            request.client_ip
        );
        return Some(WebhookResponse::text(429, "too many requests; try again later"));
    }
    None
}

/// The items of bridge `bridge_id` of `node` waiting in the edge node's queue.
fn unverified_waiting(facade: &EntityFacade, node: &str, bridge_id: &str) -> Result<i64, LibError> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let prefix = format!("chatWebhook:{bridge_id}");
    facade
        .find(queue::QUEUE_ITEM)?
        .eq("pairedNodeId", Value::string(node))
        .eq("kind", Value::string(KIND_WEBHOOK))
        .eq("statusEnumId", Value::string(queue::PENDING))
        .condition(Condition::or(vec![
            Condition::equals("conversationKey", Value::string(&prefix)),
            Condition::like("conversationKey", Value::string(format!("{prefix}:%"))),
        ]))
        .disable_authz()
        .count()
}

/// The queue item of a webhook request: the request as the payload (the body in base64, the path and query it came
/// to), keyed by the network's event id, ordered by `key.conversation` within the bridge. A request the edge node did
/// not verify (`signed_by`, its signature) is keyed by its signature too: its event id comes from a body anyone could
/// have sent, so a forged one must not take the key of the network's own event.
fn webhook_item(
    bridge_id: &str,
    key: &super::WebhookEventKey,
    request: &WebhookRequest,
    signed_by: Option<&str>,
) -> NewItem {
    let headers: Vec<[&str; 2]> = request
        .headers
        .iter()
        .filter(|(name, _)| !DROPPED_HEADERS.contains(&name.as_str()))
        .map(|(name, value)| [name.as_str(), value.as_str()])
        .collect();
    let payload = serde_json::json!({
        "chatBridgeId": bridge_id,
        "method": request.method,
        "url": request.url,
        "headers": headers,
        "body": base64::engine::general_purpose::STANDARD.encode(&request.body),
        "clientIp": request.client_ip,
        "receivedMillis": request.now_millis,
    });
    let event = key.event_id.trim();
    let tail = match signed_by {
        Some(signature) => format!(
            "{event}:sig:{}",
            sha256_hex(signature.trim()).chars().take(16).collect::<String>()
        ),
        None => event.to_string(),
    };
    let idempotency = format!("chatWebhook:{bridge_id}:{tail}");
    let idempotency = if idempotency.len() > 200 {
        format!("chatWebhook:{bridge_id}:sha256:{}", sha256_hex(&tail))
    } else {
        idempotency
    };
    let conversation = match key.conversation.as_deref().map(str::trim) {
        Some(conversation) if !conversation.is_empty() => {
            format!(
                "chatWebhook:{bridge_id}:{}",
                conversation.chars().take(100).collect::<String>()
            )
        }
        _ => format!("chatWebhook:{bridge_id}"),
    };
    NewItem {
        kind: KIND_WEBHOOK.to_string(),
        conversation_key: conversation,
        idempotency_key: idempotency,
        payload: Some(payload.to_string()),
        content: None,
        content_location: None,
    }
}

/// The webhook request a `chatWebhook` payload carries.
fn request_of(payload: &serde_json::Value) -> Result<WebhookRequest, String> {
    let text = |name: &str| payload[name].as_str().unwrap_or_default().to_string();
    let body = base64::engine::general_purpose::STANDARD
        .decode(text("body"))
        .map_err(|error| format!("its body is not base64: {error}"))?;
    let headers = payload["headers"]
        .as_array()
        .ok_or("it has no headers")?
        .iter()
        .filter_map(|pair| Some((pair[0].as_str()?.to_ascii_lowercase(), pair[1].as_str()?.to_string())))
        .collect();
    Ok(WebhookRequest {
        method: text("method"),
        url: text("url"),
        headers,
        body,
        client_ip: text("clientIp"),
        now_millis: payload["receivedMillis"].as_i64().unwrap_or_else(now_millis),
    })
}

/// Whether the current call is the edge queue's apply of item `item` from node `node`: the user is the delegate user
/// and the item's receipt is there (the queue inserts it before it calls the handler, in the same transaction).
fn called_by_queue(facade: &EntityFacade, item: &str, node: &str) -> Result<bool, LibError> {
    let user = crate::user::current_username().map_err(|error| LibError::new(error.to_string()))?;
    if item.is_empty() || user.as_deref() != Some(crate::edge::delegate::EDGE_USERNAME) {
        return Ok(false);
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let receipt = facade
        .find(queue::RECEIPT)?
        .eq("pairedNodeId", Value::string(node))
        .eq("queueItemId", Value::string(item))
        .select_field("queueItemId")
        .disable_authz()
        .one()?;
    Ok(receipt.is_some())
}

// ---- the local node: webhooks checked here ----

/// The queued webhooks a local node refused, kept for `replay#RefusedWebhooks` (30 days, as a batch).
pub const REFUSED: &str = "moqui.chat.ChatBridgeWebhookRefused";
/// The refused webhooks kept for one bridge at most; past it they are counted and logged, not kept.
const REFUSED_KEPT: i64 = 1_000;
/// How long the local node waits before it logs again that a bridge's queued webhooks were refused.
const REFUSED_LOG_MILLIS: i64 = 10 * 60_000;

/// The queued webhooks this node refused, by bridge: in all, since when, and since the last log line.
#[derive(Default)]
struct RefusedWebhooks {
    by_bridge: std::sync::Mutex<std::collections::HashMap<String, RefusedCount>>,
}

#[derive(Default)]
struct RefusedCount {
    total: u64,
    unlogged: u64,
    logged_at: Option<i64>,
    latest_item: String,
}

/// The queued webhooks of bridge `bridge_id` this node refused since it started, their signature not the network's
/// (the kept ones, across nodes, are `moqui.chat.ChatBridgeWebhookRefused`: `find#Bridges` `refusedWebhookCount`).
pub fn refused_webhooks(factory: &ExecutionContextFactory, bridge_id: &str) -> u64 {
    let refused = factory.extension(|_| RefusedWebhooks::default());
    let counts = moqui_lib::sync::lock(&refused.by_bridge);
    counts.get(bridge_id).map_or(0, |count| count.total)
}

/// Log the refusals of each bridge since its last line, once every [`REFUSED_LOG_MILLIS`] (from each refusal, and
/// from the delivery worker's round, so the count is logged once refusals stop too).
pub(super) fn log_refused(factory: &ExecutionContextFactory, now: i64) {
    let refused = factory.extension(|_| RefusedWebhooks::default());
    let mut counts = moqui_lib::sync::lock(&refused.by_bridge);
    for (bridge_id, count) in counts.iter_mut() {
        if count.unlogged == 0
            || count
                .logged_at
                .is_some_and(|at| now.saturating_sub(at) < REFUSED_LOG_MILLIS)
        {
            continue;
        }
        tracing::error!(
            "chat bridge {bridge_id}: {} webhook(s) its edge node queued were refused here, their signature not the \
             network's (the latest item {}; {} since this node started). They are kept for replay#RefusedWebhooks: is \
             the bridge's signing secret or webhookUrl right?",
            count.unlogged,
            count.latest_item,
            count.total
        );
        count.unlogged = 0;
        count.logged_at = Some(now);
    }
}

/// What the local node found of a queued request's signature.
#[derive(Debug, PartialEq, Eq)]
enum Checked {
    /// The network's.
    Valid,
    /// This node cannot tell: what the check needs is missing (a key or secret, a configuration that did not read).
    CannotCheck(String),
    /// Not the network's.
    Refused,
}

/// The network's signature over a queued request, checked here with the bridge's whole configuration, at the time the
/// request came and over its URL, whichever node checked it before: an adapter that verifies on the edge node is
/// checked again, so a bridge whose adapter changed since the edge node queued its request is not applied unchecked.
fn check_here(adapter: &dyn super::ChatBridgeAdapter, info: &BridgeInfo, request: &WebhookRequest) -> Checked {
    match std::panic::catch_unwind(AssertUnwindSafe(|| adapter.webhook_verifiable(info))) {
        Ok(Ok(())) => {}
        Ok(Err(missing)) => return Checked::CannotCheck(missing),
        Err(_) => return Checked::CannotCheck("the adapter failed while it checked".to_string()),
    }
    match std::panic::catch_unwind(AssertUnwindSafe(|| adapter.verify_webhook(info, request))) {
        Ok(None) => Checked::Valid,
        Ok(Some(_)) => Checked::Refused,
        Err(_) => Checked::CannotCheck("the adapter failed while it checked".to_string()),
    }
}

/// A queued request whose signature is not the network's: counted and logged ([`log_refused`]), and kept (at most
/// [`REFUSED_KEPT`] a bridge) for `replay#RefusedWebhooks` after a configuration fix. The item is answered as done:
/// a failed one would hold the conversation's later items behind it for its retries.
fn refuse_here(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    bridge_id: &str,
    node: &str,
    item: &str,
    payload: &str,
    request: &WebhookRequest,
) -> Result<BTreeMap<String, Value>, LibError> {
    let now = now_millis();
    {
        let refused = factory.extension(|_| RefusedWebhooks::default());
        let mut counts = moqui_lib::sync::lock(&refused.by_bridge);
        let count = counts.entry(bridge_id.to_string()).or_default();
        count.total += 1;
        count.unlogged += 1;
        count.latest_item = item.to_string();
    }
    log_refused(factory, now);
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let kept = facade
        .find(REFUSED)?
        .eq("chatBridgeId", Value::string(bridge_id))
        .disable_authz()
        .count()?;
    if kept < REFUSED_KEPT {
        let mut row = facade.make_value(REFUSED)?;
        row.set("chatBridgeId", Value::string(bridge_id));
        row.set("pairedNodeId", Value::string(node));
        row.set("queueItemId", Value::string(item));
        row.set("receivedDate", stamp_value(request.now_millis));
        row.set("refusedDate", stamp_value(now));
        row.set("reason", Value::string("the signature is not the network's"));
        row.set("requestJson", Value::string(payload));
        row.create_or_update(facade)?;
    }
    Ok(BTreeMap::from([(
        "refused".to_string(),
        Value::string("the signature is not the network's; kept for replay#RefusedWebhooks"),
    )]))
}

/// `apply#EdgeWebhook`, on the local node: a webhook request its edge node queued, its signature checked here
/// ([`check_here`]) and handed to the bridge's adapter ([`super::ChatBridgeAdapter::apply_webhook`]) with the
/// transport-verified mark set. One this node cannot check (the bridge's key or secret missing), a bridge that is
/// disabled here, or a database that cannot be read answers `retryLater`, and so does an adapter's 409, 429 or 503; one
/// whose signature is not the network's is refused here and kept ([`refuse_here`]); any other refusal is an error,
/// counted.
fn apply_edge_webhook(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let factory = current_factory().ok_or_else(|| LibError::new("no execution context"))?;
    if crate::edge::settings(&factory).takes_links() {
        return Err(LibError::new(
            "apply#EdgeWebhook runs on a local node, not on an edge node",
        ));
    }
    let item = param(parameters, "queueItemId").unwrap_or_default();
    let node = param(parameters, "pairedNodeId").unwrap_or_default();
    // only the queue's apply may set the transport-verified mark: it runs as the delegate user, with the item's receipt
    // inserted in this transaction
    if !called_by_queue(facade, &item, &node)? {
        tracing::error!("apply#EdgeWebhook was called for item {item} other than by the edge queue; refused");
        return Err(LibError::new(
            "apply#EdgeWebhook runs only for an item the edge queue applies",
        ));
    }
    let payload_text = param(parameters, "payload").unwrap_or_default();
    let payload: serde_json::Value = serde_json::from_str(&payload_text)
        .map_err(|error| LibError::new(format!("chat webhook item {item}: its payload is not JSON: {error}")))?;
    let bridge_id = payload["chatBridgeId"].as_str().unwrap_or_default().to_string();
    if bridge_id.is_empty() {
        return Err(LibError::new(format!("chat webhook item {item} names no bridge")));
    }
    let request = request_of(&payload).map_err(|error| LibError::new(format!("chat webhook item {item}: {error}")))?;
    let row = {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        facade
            .find(BRIDGE)
            .and_then(|find| find.eq("chatBridgeId", Value::string(&bridge_id)).disable_authz().one())
    };
    let row = match row {
        Ok(Some(row)) => row,
        Ok(None) => {
            tracing::error!("chat webhook item {item}: bridge {bridge_id} is not on this node");
            return Err(LibError::new(format!("chat bridge {bridge_id} is not on this node")));
        }
        Err(error) => {
            return Ok(retry_later(format!(
                "chat webhook item {item}: bridge {bridge_id} was not read: {error}"
            )))
        }
    };
    if text_of(&row, "statusId").as_deref() == Some("CbsDisabled") {
        return Ok(retry_later(format!(
            "chat webhook item {item}: bridge {bridge_id} is disabled"
        )));
    }
    let info = bridge_info(&row);
    let Some(adapter) = adapter_of(&factory, facade, &info) else {
        return Err(LibError::new(format!(
            "chat bridge {bridge_id} has no adapter in this build"
        )));
    };
    match check_here(adapter.as_ref(), &info, &request) {
        Checked::Valid => {}
        Checked::CannotCheck(missing) => {
            return Ok(retry_later(format!(
                "chat webhook item {item}: bridge {bridge_id} cannot check its webhooks here: {missing}"
            )))
        }
        Checked::Refused => return refuse_here(&factory, facade, &bridge_id, &node, &item, &payload_text, &request),
    }
    let answer = marked(&bridge_id, || {
        std::panic::catch_unwind(AssertUnwindSafe(|| adapter.apply_webhook(&info, &request)))
    });
    let Ok(answer) = answer else {
        return Err(LibError::new(format!(
            "the chat bridge adapter {} failed on webhook item {item}",
            adapter.name()
        )));
    };
    let said = String::from_utf8_lossy(&answer.body)
        .chars()
        .take(500)
        .collect::<String>();
    match answer.status {
        200..=299 => {
            tracing::info!("chat webhook item {item} of bridge {bridge_id} applied: {said}");
            Ok(BTreeMap::new())
        }
        409 | 429 | 503 => Ok(retry_later(format!(
            "chat webhook item {item} of bridge {bridge_id}: {} {said}",
            answer.status
        ))),
        status => {
            tracing::error!("chat webhook item {item} of bridge {bridge_id} was refused: {status} {said}");
            Err(LibError::new(format!(
                "chat webhook item {item} of bridge {bridge_id}: {status} {said}"
            )))
        }
    }
}

/// `replay#RefusedWebhooks` (`ChatBridgeAdmin`): the queued webhooks of bridge `chatBridgeId` this cluster refused
/// ([`refuse_here`]), checked again with the bridge's configuration as it is now (after its secret or `webhookUrl` was
/// fixed) and applied when they pass; each in a transaction of its own, one that fails logged and the rest go on. Out:
/// appliedCount, refusedCount (still not the network's, kept), waitingCount (cannot be checked or applied now, kept).
fn replay_refused_webhooks(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let factory = current_factory().ok_or_else(|| LibError::new("no execution context"))?;
    let authz_disabled = ExecutionContext::with_current(|ec| ec.authz_disabled()).unwrap_or(false);
    if !authz_disabled && !crate::user::has_permission("ChatBridgeAdmin")? {
        return Err(LibError::new("Chat bridges are for administrators"));
    }
    let bridge_id = param(parameters, "chatBridgeId").unwrap_or_default();
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let Some(row) = facade
        .find(BRIDGE)?
        .eq("chatBridgeId", Value::string(&bridge_id))
        .disable_authz()
        .one()?
    else {
        return Err(LibError::new(format!("Chat bridge {bridge_id} not found")));
    };
    let info = bridge_info(&row);
    let adapter = adapter_of(&factory, facade, &info)
        .ok_or_else(|| LibError::new(format!("No adapter for bridge {bridge_id}")))?;
    let kept = facade
        .find(REFUSED)?
        .eq("chatBridgeId", Value::string(&bridge_id))
        .order_by("receivedDate", false)
        .disable_authz()
        .list()?;
    let (mut applied, mut refused, mut waiting) = (0_i64, 0_i64, 0_i64);
    for kept in kept.values() {
        let node = text_of(kept, "pairedNodeId").unwrap_or_default();
        let item = text_of(kept, "queueItemId").unwrap_or_default();
        let request = serde_json::from_str::<serde_json::Value>(&text_of(kept, "requestJson").unwrap_or_default())
            .map_err(|error| error.to_string())
            .and_then(|payload| request_of(&payload));
        let request = match request {
            Ok(request) => request,
            Err(error) => {
                tracing::error!("refused chat webhook item {item} of bridge {bridge_id} cannot be read: {error}");
                refused += 1;
                continue;
            }
        };
        match check_here(adapter.as_ref(), &info, &request) {
            Checked::Valid => {}
            Checked::Refused => {
                refused += 1;
                continue;
            }
            Checked::CannotCheck(missing) => {
                tracing::warn!("refused chat webhook item {item} of bridge {bridge_id} cannot be checked: {missing}");
                waiting += 1;
                continue;
            }
        }
        let worked = transaction(facade, REFUSED, TransactionAttribute::ForceNew, || {
            let answer = marked(&bridge_id, || {
                std::panic::catch_unwind(AssertUnwindSafe(|| adapter.apply_webhook(&info, &request)))
            })
            .map_err(|_| LibError::new("the adapter failed"))?;
            if !(200..300).contains(&answer.status) {
                return Ok(false);
            }
            facade
                .find(REFUSED)?
                .eq("chatBridgeId", Value::string(&bridge_id))
                .eq("pairedNodeId", Value::string(&node))
                .eq("queueItemId", Value::string(&item))
                .disable_authz()
                .delete_all()?;
            Ok(true)
        });
        match worked {
            Ok(true) => applied += 1,
            Ok(false) => waiting += 1,
            Err(error) => {
                tracing::error!("refused chat webhook item {item} of bridge {bridge_id} was not applied: {error}");
                waiting += 1;
            }
        }
    }
    tracing::info!(
        "chat bridge {bridge_id}: refused webhooks replayed: {applied} applied, {refused} still refused, {waiting} wait"
    );
    Ok(BTreeMap::from([
        ("appliedCount".to_string(), Value::I64(applied)),
        ("refusedCount".to_string(), Value::I64(refused)),
        ("waitingCount".to_string(), Value::I64(waiting)),
    ]))
}

// ---- the local node: files out ----

/// The addresses of a send's files, or why it cannot go now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaUrls {
    /// One per attachment, in order.
    Ready(Vec<String>),
    /// Not on the edge node yet: try the send again later.
    Wait(String),
    /// They cannot be given an address.
    Unreachable(String),
}

/// Whether this node sends its files by address through its edge node: a paired local node, with the edge link built
/// in, whose conf lists [`KIND_MEDIA`].
fn through_edge(factory: &ExecutionContextFactory) -> bool {
    let settings = crate::edge::settings(factory);
    !settings.takes_links()
        && settings.kinds.contains_key(KIND_MEDIA)
        && !crate::context::conf_env::setting(crate::edge::NODE_URL).trim().is_empty()
        // a build without the edge link (Lite, Dev) has no link to send the files on
        && crate::edge::link().is_ok()
}

/// Whether this node's link to its edge node is up now.
fn linked() -> bool {
    crate::edge::link().is_ok_and(|link| link.status().state == crate::edge::LinkState::Linked)
}

/// The addresses a network fetches the files of `request` (to bridge `bridge_id`) at, from `now`: through the edge
/// node on a paired local node (see the module notes), else this node's own ([`super::media_url`]).
pub fn media_urls(bridge_id: &str, request: &DeliveryRequest, now: i64) -> MediaUrls {
    if request.attachments.is_empty() {
        return MediaUrls::Ready(Vec::new());
    }
    let context = ExecutionContext::with_current(|ec| (std::sync::Arc::clone(ec.factory()), ec.factory().entity()));
    if let Some((factory, Some(facade))) = context {
        if through_edge(&factory) {
            return edge_media_urls(&factory, &facade, bridge_id, request, now);
        }
    }
    let mut urls = Vec::with_capacity(request.attachments.len());
    for file in &request.attachments {
        match super::media_url(bridge_id, &file.chat_attachment_id, now) {
            Some(url) => urls.push(url),
            None => {
                return MediaUrls::Unreachable(
                    "Files go by address: set channels@public-url (and channels@media-key)".to_string(),
                )
            }
        }
    }
    MediaUrls::Ready(urls)
}

/// The token of a file put on the edge node for the item keyed `key`: an HMAC under `channels@media-key`, so it is the
/// same for each attempt of one send and nobody without the key can make one.
fn media_token(media_key: &str, key: &str) -> Option<String> {
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(media_key.as_bytes()).ok()?;
    mac.update(format!("edge-media|{key}").as_bytes());
    Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

fn edge_media_urls(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    bridge_id: &str,
    request: &DeliveryRequest,
    now: i64,
) -> MediaUrls {
    let (Some(base), Some(media_key)) = (public_base(), media_key()) else {
        return MediaUrls::Unreachable(
            "Files go by address through the edge node: set channels@public-url to the edge node's address (and \
             channels@media-key)"
                .to_string(),
        );
    };
    let minutes = media_minutes();
    // a send tried again within half the address's life names the same item, so its file goes once; later, a new one,
    // so the address the network gets has at least half its life left
    let window = now / (minutes * 30_000);
    let mut urls = Vec::with_capacity(request.attachments.len());
    let mut items = Vec::with_capacity(request.attachments.len());
    for file in &request.attachments {
        let id = &file.chat_attachment_id;
        let row = {
            let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
            facade
                .find("moqui.chat.ChatAttachment")
                .and_then(|find| find.eq("chatAttachmentId", Value::string(id)).disable_authz().one())
        };
        let row = match row {
            Ok(Some(row)) => row,
            Ok(None) => return MediaUrls::Unreachable(format!("File {id} is gone")),
            Err(error) => return MediaUrls::Wait(format!("File {id} was not read: {error}")),
        };
        let Some(location) = text_of(&row, "location") else {
            return MediaUrls::Unreachable(format!("File {id} has no content"));
        };
        let key = format!("chatMedia:{bridge_id}:{}:{id}:{window}", request.idempotency_key);
        let Some(token) = media_token(&media_key, &key) else {
            return MediaUrls::Unreachable("channels@media-key is not a key".to_string());
        };
        let payload = serde_json::json!({
            "chatBridgeId": bridge_id,
            "tokenHash": sha256_hex(&token),
            "contentType": text_of(&row, "contentType").or_else(|| file.content_type.clone()),
            "fileName": text_of(&row, "fileName").or_else(|| file.file_name.clone()),
            "minutes": minutes,
        });
        let item = NewItem {
            kind: KIND_MEDIA.to_string(),
            conversation_key: format!("chatMedia:{id}"),
            idempotency_key: key,
            payload: Some(payload.to_string()),
            content: None,
            content_location: Some(location),
        };
        // committed now, so the link can send it while this send waits
        let queued = transaction(facade, queue::QUEUE_ITEM, TransactionAttribute::ForceNew, || {
            queue::enqueue(factory, facade, None, &item)
        });
        match queued {
            Ok(queued) => items.push(queued.queue_item_id),
            Err(error) => return MediaUrls::Wait(format!("File {id} was not queued for the edge node: {error}")),
        }
        urls.push(format!("{base}/channels/{bridge_id}/media/{token}"));
    }
    // with the link down the items cannot be delivered now: the send waits outside the worker
    let wait = if linked() { MEDIA_WAIT } else { Duration::ZERO };
    let started = Instant::now();
    loop {
        match items_state(facade, &items) {
            Ok(ItemsState::Delivered) => return MediaUrls::Ready(urls),
            Ok(ItemsState::Failed(error)) => {
                return MediaUrls::Unreachable(format!("A file did not reach the edge node: {error}"))
            }
            Ok(ItemsState::Pending) => {}
            Err(error) => tracing::warn!("chat bridge {bridge_id}: the files' items were not read: {error}"),
        }
        if started.elapsed() >= wait {
            return MediaUrls::Wait("The files are not on the edge node yet".to_string());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

enum ItemsState {
    Delivered,
    Pending,
    Failed(String),
}

fn items_state(facade: &EntityFacade, items: &[String]) -> Result<ItemsState, LibError> {
    let rows = transaction(facade, queue::QUEUE_ITEM, TransactionAttribute::ForceNew, || {
        facade
            .find(queue::QUEUE_ITEM)?
            .condition(Condition::is_in(
                "queueItemId",
                items.iter().map(Value::string).collect(),
            ))
            .select_field("queueItemId")
            .select_field("statusEnumId")
            .select_field("lastErrorText")
            .use_cache(false)
            .disable_authz()
            .list()
    })?;
    let mut delivered = 0;
    for row in rows.values() {
        match text_of(row, "statusEnumId").as_deref() {
            Some(queue::DELIVERED) => delivered += 1,
            Some(queue::FAILED) => {
                return Ok(ItemsState::Failed(
                    text_of(row, "lastErrorText").unwrap_or_else(|| "failed".to_string()),
                ))
            }
            _ => {}
        }
    }
    Ok(if delivered == items.len() {
        ItemsState::Delivered
    } else {
        ItemsState::Pending
    })
}

// ---- the edge node: files kept and served ----

/// Where the edge node keeps a file of node `node` by the token's hash.
fn media_location(node: &str, token_hash: &str) -> Option<String> {
    let plain = |text: &str| {
        !text.is_empty()
            && text.len() <= 128
            && text
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    };
    (plain(node) && plain(token_hash)).then(|| format!("{MEDIA_ROOT}{node}/{token_hash}"))
}

/// A positive number from a property, else `default`.
fn limit_property(name: &str, default: i64) -> i64 {
    crate::context::conf_env::setting_or_property(name)
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|limit| *limit > 0)
        .unwrap_or(default)
}

/// Why the edge node does not keep a file of `size` bytes for a node that has `node_total` bytes of files already,
/// with files of at most `max_file` bytes and `max_node` bytes per node; none when it does.
fn over_limits(size: i64, node_total: i64, max_file: i64, max_node: i64) -> Option<String> {
    if size > max_file {
        return Some(format!(
            "the file is {size} bytes, over the edge node's {max_file} ({MEDIA_MAX_BYTES})"
        ));
    }
    if node_total.saturating_add(size) > max_node {
        return Some(format!(
            "the local node's files would be {} bytes, over the edge node's {max_node} ({MEDIA_NODE_MEGABYTES})",
            node_total.saturating_add(size)
        ));
    }
    None
}

/// The bytes of the files the edge node keeps for `node` that have not run out at `now`, other than `except`.
fn node_media_bytes(facade: &EntityFacade, node: &str, except: &str, now: i64) -> Result<i64, LibError> {
    let rows = facade
        .find(EDGE_MEDIA)?
        .eq("pairedNodeId", Value::string(node))
        .condition(Condition::greater_equal("expireDate", stamp_value(now)))
        .select_field("edgeMediaId")
        .select_field("fileSize")
        .disable_authz()
        .list()?;
    Ok(rows
        .values()
        .iter()
        .filter(|row| text_of(row, "edgeMediaId").as_deref() != Some(except))
        .filter_map(|row| super::int_of(row, "fileSize"))
        .sum())
}

/// `store#EdgeMedia`, on the edge node: a file its local node sends by address, kept by the token's hash for
/// `minutes` (the local node's `channels@media-minutes`, at most this node's). A file over
/// `channels/edge-limits@media-max-bytes`, or past `channels/edge-limits@media-node-megabytes` of the node's files, is refused, counted.
fn store_edge_media(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let factory = current_factory().ok_or_else(|| LibError::new("no execution context"))?;
    if !crate::edge::settings(&factory).takes_links() {
        return Err(LibError::new(
            "store#EdgeMedia runs on an edge node, not on a local node",
        ));
    }
    let item = param(parameters, "queueItemId").unwrap_or_default();
    let node = param(parameters, "pairedNodeId").unwrap_or_default();
    // as apply#EdgeWebhook: only the queue's apply of an item it received, with that item's own received copy
    // (SEC-COM-97); a caller inside the server reads no other location through it
    if !called_by_queue(facade, &item, &node)? {
        tracing::error!("store#EdgeMedia was called for item {item} other than by the edge queue; refused");
        return Err(LibError::new(
            "store#EdgeMedia runs only for an item the edge queue applies",
        ));
    }
    let content_location = param(parameters, "contentLocation").unwrap_or_default();
    if !content_location.is_empty() && !queue::is_received(&node, &item, &content_location) {
        tracing::error!("store#EdgeMedia for item {item} names a file that is not the item's own; refused");
        return Err(LibError::new(format!(
            "chat file item {item} names a file that is not its own"
        )));
    }
    let payload: serde_json::Value = serde_json::from_str(&param(parameters, "payload").unwrap_or_default())
        .map_err(|error| LibError::new(format!("chat file item {item}: its payload is not JSON: {error}")))?;
    let text = |name: &str| payload[name].as_str().map(str::trim).unwrap_or_default().to_string();
    let (bridge_id, token_hash) = (text("chatBridgeId"), text("tokenHash").to_ascii_lowercase());
    let location = media_location(&node, &token_hash)
        .filter(|_| token_hash.len() == 64 && !bridge_id.is_empty())
        .ok_or_else(|| LibError::new(format!("chat file item {item} names no bridge or token")))?;
    let now = now_millis();
    // the files that ran out first, each in a transaction of its own, so the node's total is what it keeps now
    sweep_edge_media(&factory, facade, now);
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let bytes = if content_location.is_empty() {
        None
    } else {
        factory.resource().location_bytes(&content_location)?
    }
    .ok_or_else(|| LibError::new(format!("chat file item {item} has no file")))?;
    let size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
    let max_node = limit_property(MEDIA_NODE_MEGABYTES, DEFAULT_MEDIA_NODE_MEGABYTES).saturating_mul(1024 * 1024);
    let max_file = limit_property(MEDIA_MAX_BYTES, DEFAULT_MEDIA_MAX_BYTES);
    if let Some(refused) = over_limits(
        size,
        node_media_bytes(facade, &node, &token_hash, now)?,
        max_file,
        max_node,
    ) {
        tracing::error!("chat file item {item} of local node {node}, bridge {bridge_id}, is refused: {refused}");
        return Err(LibError::new(format!("chat file item {item} is refused: {refused}")));
    }
    let minutes = payload["minutes"].as_i64().unwrap_or(60).clamp(1, media_minutes());
    factory.resource().write_bytes(&location, &bytes)?;
    let mut row = facade.make_value(EDGE_MEDIA)?;
    row.set("edgeMediaId", Value::string(&token_hash));
    row.set("pairedNodeId", Value::string(&node));
    row.set("chatBridgeId", Value::string(&bridge_id));
    row.set("location", Value::string(&location));
    row.set("contentType", Value::string(safe_type(Some(&text("contentType")))));
    row.set(
        "fileName",
        Value::string(text("fileName").chars().take(120).collect::<String>()),
    );
    row.set("fileSize", Value::I64(size));
    row.set("expireDate", stamp_value(now + minutes * 60_000));
    row.set("createdDate", stamp_value(now));
    row.create_or_update(facade)?;
    tracing::info!(
        "chat file item {item}: a file of bridge {bridge_id} ({size} bytes) is served for {minutes} minutes"
    );
    Ok(BTreeMap::new())
}

/// On an edge node, remove the files that ran out before `now` (a page of them), each with its row in a transaction of
/// its own: one that fails is logged and the rest go on. The count removed. The link's retention pass calls it, and
/// each store.
pub fn sweep_edge_media(factory: &ExecutionContextFactory, facade: &EntityFacade, now: i64) -> usize {
    let expired = transaction(facade, EDGE_MEDIA, TransactionAttribute::ForceNew, || {
        facade
            .find(EDGE_MEDIA)?
            .condition(Condition::less("expireDate", stamp_value(now)))
            .select_field("edgeMediaId")
            .select_field("location")
            .limit(EXPIRED_PAGE)
            .disable_authz()
            .list()
    });
    let expired = match expired {
        Ok(expired) => expired,
        Err(error) => {
            tracing::warn!("The chat files that ran out were not read: {error}");
            return 0;
        }
    };
    let mut removed = 0;
    for row in expired.values() {
        let id = text_of(row, "edgeMediaId").unwrap_or_default();
        let location = text_of(row, "location");
        let done = transaction(facade, EDGE_MEDIA, TransactionAttribute::ForceNew, || {
            remove_media(factory.resource(), facade, &id, location.as_deref())
        });
        match done {
            Ok(()) => removed += 1,
            Err(error) => tracing::warn!("The chat file {id} that ran out was not removed: {error}"),
        }
    }
    removed
}

fn remove_media(
    resources: &crate::context::ResourceFacade,
    facade: &EntityFacade,
    id: &str,
    location: Option<&str>,
) -> Result<(), LibError> {
    if let Some(location) = location {
        resources.delete(location)?;
    }
    facade
        .find(EDGE_MEDIA)?
        .eq("edgeMediaId", Value::string(id))
        .disable_authz()
        .delete_all()?;
    Ok(())
}

/// Whether `node` is an active paired node of this edge node.
fn active_node(facade: &EntityFacade, node: &str) -> Result<bool, LibError> {
    let row = facade
        .find("moqui.edge.EdgeClient")?
        .eq("pairedNodeId", Value::string(node))
        .select_field("statusId")
        .disable_authz()
        .one()?;
    Ok(row.and_then(|row| text_of(&row, "statusId")).as_deref() == Some("EdgeClientActive"))
}

/// A file at `/channels/{chatBridgeId}/media/{token}` on an edge node: one its local node put there for that bridge,
/// while it has not run out, and at a pairing's host name (`host`, the request's `Host`) only that pairing's file
/// (plans/Edge-Node2.md §4.1). One that ran out is removed.
pub(super) fn media(
    factory: &ExecutionContextFactory,
    host: Option<&str>,
    bridge_id: &str,
    token: &str,
    now: i64,
) -> Option<MediaFile> {
    let facade = factory.entity()?;
    if token.is_empty() || token.len() > 100 {
        return None;
    }
    let only = match transaction(&facade, EDGE_MEDIA, TransactionAttribute::UseOrBegin, || {
        host_name_pairing(&facade, host)
    }) {
        Ok(only) => only,
        Err(error) => {
            tracing::warn!("chat bridge {bridge_id}: the pairing of a file's host was not read: {error}");
            return None;
        }
    };
    let id = sha256_hex(token);
    let row = transaction(&facade, EDGE_MEDIA, TransactionAttribute::UseOrBegin, || {
        facade
            .find(EDGE_MEDIA)?
            .eq("edgeMediaId", Value::string(&id))
            .disable_authz()
            .one()
    });
    let row = match row {
        // only for the bridge it was put there for (checked here: a find by the whole key answers by the key alone)
        Ok(row) => row.filter(|row| text_of(row, "chatBridgeId").as_deref() == Some(bridge_id))?,
        Err(error) => {
            tracing::warn!("chat bridge {bridge_id}: a file was not read: {error}");
            return None;
        }
    };
    // not for a node that is no longer paired, nor another pairing's at a pairing's host name
    let node = text_of(&row, "pairedNodeId").unwrap_or_default();
    if only.as_ref().is_some_and(|only| *only != node) {
        return None;
    }
    match transaction(&facade, EDGE_MEDIA, TransactionAttribute::UseOrBegin, || {
        active_node(&facade, &node)
    }) {
        Ok(true) => {}
        Ok(false) => return None,
        Err(error) => {
            tracing::warn!("chat bridge {bridge_id}: the node of a file was not read: {error}");
            return None;
        }
    }
    let location = text_of(&row, "location");
    if row
        .get("expireDate")
        .and_then(value_millis)
        .is_none_or(|expiry| expiry < now)
    {
        let removed = transaction(&facade, EDGE_MEDIA, TransactionAttribute::ForceNew, || {
            remove_media(factory.resource(), &facade, &id, location.as_deref())
        });
        if let Err(error) = removed {
            tracing::warn!("The chat file {id} that ran out was not removed: {error}");
        }
        return None;
    }
    let bytes = transaction(&facade, EDGE_MEDIA, TransactionAttribute::UseOrBegin, || {
        factory
            .resource()
            .location_bytes(location.as_deref().unwrap_or_default())
            .map_err(LibError::from)
    });
    let bytes = match bytes {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return None,
        Err(error) => {
            tracing::warn!("chat file {id} of bridge {bridge_id} was not read: {error}");
            return None;
        }
    };
    Some(MediaFile {
        content_type: safe_type(text_of(&row, "contentType").as_deref()),
        file_name: text_of(&row, "fileName").unwrap_or_else(|| "file".to_string()),
        bytes,
    })
}

// ---- the local node: the keys of bridges saved before E6 ----

/// On a local node, copy each bridge's `configJson` `publicKey` and `webhookToleranceSeconds` to `webhookPublicKey` and
/// `webhookToleranceSeconds` where they differ (a bridge saved before E6, or its configuration written another way),
/// so its replicated copy can check its webhooks. A Telnyx bridge (adapter `sms`) with no key is logged as an error.
/// Each bridge in a transaction of its own; one that fails is logged and the rest go on. The count changed.
pub fn fill_webhook_keys(facade: &EntityFacade) -> usize {
    let rows = transaction(facade, BRIDGE, TransactionAttribute::ForceNew, || {
        facade.find(BRIDGE)?.use_cache(false).disable_authz().list()
    });
    let rows = match rows {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!("The chat bridges' webhook keys were not checked: {error}");
            return 0;
        }
    };
    let mut changed = 0;
    for row in rows.values() {
        let info = bridge_info(row);
        let id = info.chat_bridge_id.clone();
        let key = info.config["publicKey"]
            .as_str()
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        let tolerance = number_field(&info.config, "webhookToleranceSeconds");
        // only Telnyx's adapter (`sms`, also a CbtSms bridge that names none) checks with a public key; the other SMS
        // providers sign with a secret the local node checks (WebhookVerification::Core)
        let telnyx = info.adapter_name == "sms" || (info.adapter_name.is_empty() && info.type_enum_id == "CbtSms");
        if key.is_empty() {
            if telnyx {
                tracing::error!(
                    "SMS bridge {id} has no publicKey in its configuration, so its webhooks are refused (through an \
                     edge node too)"
                );
            }
            continue;
        }
        let stored_tolerance = super::int_of(row, "webhookToleranceSeconds");
        if text_of(row, "webhookPublicKey").as_deref() == Some(key.as_str()) && stored_tolerance == tolerance {
            continue;
        }
        let done = transaction(facade, BRIDGE, TransactionAttribute::ForceNew, || {
            facade
                .find(BRIDGE)?
                .eq("chatBridgeId", Value::string(&id))
                .disable_authz()
                .update_all([
                    ("webhookPublicKey".to_string(), Value::string(&key)),
                    (
                        "webhookToleranceSeconds".to_string(),
                        tolerance.map_or(Value::Null, Value::I64),
                    ),
                ])
        });
        match done {
            Ok(_) => {
                tracing::info!("chat bridge {id}: its webhook key is kept for an edge node's copy");
                changed += 1;
            }
            Err(error) => tracing::error!("chat bridge {id}: its webhook key was not kept: {error}"),
        }
    }
    changed
}

#[cfg(test)]
#[path = "chat_bridge_edge_tests.rs"]
mod tests;
