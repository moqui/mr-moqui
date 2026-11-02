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

//! Chat bridges and SMS through an edge node (plans/Edge-Node.md §5.4, E6): the link's queue carries a network's
//! webhooks to the local node, and the files of the messages the local node sends by address to the edge node.
//!
//! - **Webhooks** (kind [`KIND_WEBHOOK`], edge node to local node). On the edge node [`webhook`] reads the bridge from
//!   the local node's replicated set [`REPLICA_SET`] (its network, adapter, status and the webhook's public key, never
//!   its `configJson`, whose provider keys stay on the local node), checks the network's signature with the adapter's
//!   [`verify_webhook`](super::ChatBridgeAdapter::verify_webhook), and queues the raw request, keyed by the network's
//!   event id ([`webhook_event_id`](super::ChatBridgeAdapter::webhook_event_id)) so a repeat is dropped, and ordered by
//!   their number. It answers 200 once the item is committed; a bad signature, an unknown bridge and a disabled one
//!   answer as on any node. The local node's `apply#EdgeWebhook` rebuilds the request and hands it to the adapter's
//!   [`apply_webhook`](super::ChatBridgeAdapter::apply_webhook) with the transport-verified mark set, without checking
//!   the signature again: its time window has passed by then.
//! - **Files** (kind [`KIND_MEDIA`], local node to edge node). A paired local node whose conf lists `chatMedia` sends
//!   every file of a message that goes by address (an MMS picture) through its edge node ([`media_urls`]): it queues
//!   the file as the item's content, with a token it makes (an HMAC under `channels_media_key`, so a later attempt of
//!   the same send names the same file), and does not send until the item is delivered, so the network never fetches
//!   an address the edge node does not have yet: while the link is up it waits up to [`MEDIA_WAIT`], else the send
//!   waits [`MEDIA_RETRY_MILLIS`] and is tried again, its attempt not counted (`DeliveryOutcome::Wait`). The address is
//!   under `channels_public_url`, which on a paired node names the edge node. The edge node's `store#EdgeMedia` keeps
//!   the file as a `moqui.edge.EdgeMedia` row by the token's hash, within its own limits (`channels_media_minutes`,
//!   `channels_edge_media_max_bytes`, `channels_edge_media_node_megabytes`), and [`media`] serves it until it runs
//!   out; [`sweep_edge_media`] (the link's retention pass, and each store) removes the files that ran out.
//! - **Keys of older bridges.** [`fill_webhook_keys`], at the delivery worker's start, copies an SMS bridge's
//!   `configJson` `publicKey` and `webhookToleranceSeconds` to the replicated fields of a bridge saved before E6.
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
    WebhookResponse, BRIDGE,
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
/// The largest file the edge node keeps: `channels_edge_media_max_bytes`, 5 MiB by default (a provider's MMS limit is
/// about 1 MiB in all).
const MEDIA_MAX_BYTES: &str = "channels_edge_media_max_bytes";
const DEFAULT_MEDIA_MAX_BYTES: i64 = 5 * 1024 * 1024;
/// The files of one local node the edge node keeps at most: `channels_edge_media_node_megabytes`, 256 by default.
const MEDIA_NODE_MEGABYTES: &str = "channels_edge_media_node_megabytes";
const DEFAULT_MEDIA_NODE_MEGABYTES: i64 = 256;
/// The expired files one sweep removes at most.
const EXPIRED_PAGE: i64 = 500;
/// Headers of a webhook request that are not kept with the queued request.
const DROPPED_HEADERS: &[&str] = &["cookie", "authorization", "proxy-authorization"];

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

/// The bridge `bridge_id` as a paired local node replicated it: the node and the set's fields. None when no active
/// node has it, or more than one does (one local node per edge node, decision 1).
fn replicated_bridge(facade: &EntityFacade, bridge_id: &str) -> Result<Option<(String, serde_json::Value)>, LibError> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let nodes = facade
        .find("moqui.edge.EdgeClient")?
        .eq("statusId", Value::string("EdgeClientActive"))
        .select_field("pairedNodeId")
        .disable_authz()
        .list()?;
    let key = BTreeMap::from([("chatBridgeId".to_string(), bridge_id.to_string())]);
    let mut found = Vec::new();
    for node in nodes.values() {
        let node = text_of(node, "pairedNodeId").unwrap_or_default();
        if let Some(value) = crate::edge::replica::get_value(facade, Some(&node), REPLICA_SET, BRIDGE, &key)? {
            found.push((node, value.fields));
        }
    }
    if found.len() > 1 {
        tracing::error!("chat bridge {bridge_id}: more than one local node replicated it; its webhooks are refused");
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

/// A request to `/channels/{chatBridgeId}/webhook` on an edge node. See the module notes.
pub(super) fn webhook(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    bridge_id: &str,
    request: &WebhookRequest,
) -> WebhookResponse {
    let (node, fields) = match replicated_bridge(facade, bridge_id) {
        Ok(Some(found)) => found,
        Ok(None) => return WebhookResponse::not_signed(),
        Err(error) => {
            tracing::error!("chat bridge {bridge_id}: the edge node could not read its replicated bridge: {error}");
            return WebhookResponse::text(503, "try again later");
        }
    };
    let public_key = field(&fields, "webhookPublicKey");
    if public_key.is_empty() {
        tracing::error!(
            "chat bridge {bridge_id} of local node {node} has no webhookPublicKey in its replicated copy, so its \
             webhooks are refused: set publicKey in its configuration on the local node"
        );
    }
    // only what the signature check needs: the provider's keys stay on the local node
    let mut config = serde_json::json!({});
    if !public_key.is_empty() {
        config["publicKey"] = serde_json::Value::String(public_key);
    }
    if let Some(tolerance) = number_field(&fields, "webhookToleranceSeconds") {
        config["webhookToleranceSeconds"] = serde_json::Value::from(tolerance);
    }
    let info = BridgeInfo {
        chat_bridge_id: bridge_id.to_string(),
        type_enum_id: field(&fields, "typeEnumId"),
        adapter_name: field(&fields, "adapterName"),
        name: String::new(),
        config,
    };
    let Some(adapter) = adapter_of(factory, facade, &info) else {
        return WebhookResponse::not_signed();
    };
    if !request.method.eq_ignore_ascii_case("POST") {
        return WebhookResponse::text(405, "a webhook is a POST");
    }
    match std::panic::catch_unwind(AssertUnwindSafe(|| adapter.verify_webhook(&info, request))) {
        Ok(None) => {}
        Ok(Some(refused)) => return refused,
        Err(_) => return WebhookResponse::not_signed(),
    }
    if field(&fields, "statusId") == "CbsDisabled" {
        return WebhookResponse::text(503, "the bridge is disabled");
    }
    let Ok(Some(key)) = std::panic::catch_unwind(AssertUnwindSafe(|| adapter.webhook_event_id(&info, request))) else {
        return WebhookResponse::text(400, "the request names no event");
    };
    let item = webhook_item(bridge_id, &key, request);
    match queue::enqueue(factory, facade, Some(&node), &item) {
        Ok(queued) => {
            let outcome = if queued.created { "queued" } else { "duplicate" };
            tracing::info!(
                "chat bridge {bridge_id}: webhook event {} {outcome} for local node {node} as item {}",
                key.event_id,
                queued.queue_item_id
            );
            WebhookResponse::json(200, &serde_json::json!({ "outcome": outcome }))
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

/// The queue item of a verified webhook request: the request as the payload (the body in base64), keyed by the
/// network's event id, ordered by `key.conversation` within the bridge.
fn webhook_item(bridge_id: &str, key: &super::WebhookEventKey, request: &WebhookRequest) -> NewItem {
    let headers: Vec<[&str; 2]> = request
        .headers
        .iter()
        .filter(|(name, _)| !DROPPED_HEADERS.contains(&name.as_str()))
        .map(|(name, value)| [name.as_str(), value.as_str()])
        .collect();
    let payload = serde_json::json!({
        "chatBridgeId": bridge_id,
        "method": request.method,
        "headers": headers,
        "body": base64::engine::general_purpose::STANDARD.encode(&request.body),
        "clientIp": request.client_ip,
        "receivedMillis": request.now_millis,
    });
    let idempotency = format!("chatWebhook:{bridge_id}:{}", key.event_id.trim());
    let idempotency = if idempotency.len() > 200 {
        format!("chatWebhook:{bridge_id}:sha256:{}", sha256_hex(key.event_id.trim()))
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

/// `apply#EdgeWebhook`, on the local node: a webhook request its edge node verified and queued, handed to the
/// bridge's adapter ([`super::ChatBridgeAdapter::apply_webhook`]) with the transport-verified mark set. A bridge that is
/// disabled here, or a database that cannot be read, answers `retryLater`, and so does an adapter's 409, 429 or 503;
/// any other refusal is an error, counted.
fn apply_edge_webhook(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let factory = current_factory().ok_or_else(|| LibError::new("no execution context"))?;
    if crate::edge::settings(&factory).edge_node {
        return Err(LibError::new(
            "apply#EdgeWebhook runs on a local node, not on an edge node",
        ));
    }
    let item = param(parameters, "queueItemId").unwrap_or_default();
    // only the queue's apply may set the transport-verified mark: it runs as the delegate user, with the item's receipt
    // inserted in this transaction
    if !called_by_queue(facade, &item, &param(parameters, "pairedNodeId").unwrap_or_default())? {
        tracing::error!("apply#EdgeWebhook was called for item {item} other than by the edge queue; refused");
        return Err(LibError::new(
            "apply#EdgeWebhook runs only for an item the edge queue applies",
        ));
    }
    let payload: serde_json::Value = serde_json::from_str(&param(parameters, "payload").unwrap_or_default())
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
    !settings.edge_node
        && settings.kinds.contains_key(KIND_MEDIA)
        && !moqui_lib::env::get_prop_or_env(crate::edge::NODE_URL).trim().is_empty()
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
                    "Files go by address: set channels_public_url (and channels_media_key)".to_string(),
                )
            }
        }
    }
    MediaUrls::Ready(urls)
}

/// The token of a file put on the edge node for the item keyed `key`: an HMAC under `channels_media_key`, so it is the
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
            "Files go by address through the edge node: set channels_public_url to the edge node's address (and \
             channels_media_key)"
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
            return MediaUrls::Unreachable("channels_media_key is not a key".to_string());
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
    moqui_lib::env::get_prop_or_env(name)
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
/// `minutes` (the local node's `channels_media_minutes`, at most this node's). A file over
/// `channels_edge_media_max_bytes`, or past `channels_edge_media_node_megabytes` of the node's files, is refused, counted.
fn store_edge_media(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let factory = current_factory().ok_or_else(|| LibError::new("no execution context"))?;
    if !crate::edge::settings(&factory).edge_node {
        return Err(LibError::new(
            "store#EdgeMedia runs on an edge node, not on a local node",
        ));
    }
    let item = param(parameters, "queueItemId").unwrap_or_default();
    let node = param(parameters, "pairedNodeId").unwrap_or_default();
    let payload: serde_json::Value = serde_json::from_str(&param(parameters, "payload").unwrap_or_default())
        .map_err(|error| LibError::new(format!("chat file item {item}: its payload is not JSON: {error}")))?;
    let text = |name: &str| payload[name].as_str().map(str::trim).unwrap_or_default().to_string();
    let (bridge_id, token_hash) = (text("chatBridgeId"), text("tokenHash").to_ascii_lowercase());
    let location = media_location(&node, &token_hash)
        .filter(|_| token_hash.len() == 64 && !bridge_id.is_empty())
        .ok_or_else(|| LibError::new(format!("chat file item {item} names no bridge or token")))?;
    let content_location = param(parameters, "contentLocation").unwrap_or_default();
    let now = now_millis();
    // the files that ran out first, each in a transaction of its own, so the node's total is what it keeps now
    sweep_edge_media(facade, now);
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let bytes = if content_location.is_empty() {
        None
    } else {
        facade.read_db_resource_bytes(&content_location)?
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
    facade.write_db_resource(&location, &bytes)?;
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
pub fn sweep_edge_media(facade: &EntityFacade, now: i64) -> usize {
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
            remove_media(facade, &id, location.as_deref())
        });
        match done {
            Ok(()) => removed += 1,
            Err(error) => tracing::warn!("The chat file {id} that ran out was not removed: {error}"),
        }
    }
    removed
}

fn remove_media(facade: &EntityFacade, id: &str, location: Option<&str>) -> Result<(), LibError> {
    if let Some(location) = location {
        facade.delete_db_resource(location)?;
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
/// while it has not run out. One that ran out is removed.
pub(super) fn media(factory: &ExecutionContextFactory, bridge_id: &str, token: &str, now: i64) -> Option<MediaFile> {
    let facade = factory.entity()?;
    if token.is_empty() || token.len() > 100 {
        return None;
    }
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
    // not for a node that is no longer paired
    let node = text_of(&row, "pairedNodeId").unwrap_or_default();
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
            remove_media(&facade, &id, location.as_deref())
        });
        if let Err(error) = removed {
            tracing::warn!("The chat file {id} that ran out was not removed: {error}");
        }
        return None;
    }
    let bytes = transaction(&facade, EDGE_MEDIA, TransactionAttribute::UseOrBegin, || {
        facade.read_db_resource_bytes(location.as_deref().unwrap_or_default())
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
/// so its replicated copy can check its webhooks. An SMS bridge with no key is logged as an error. Each bridge in a
/// transaction of its own; one that fails is logged and the rest go on. The count changed.
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
        let sms = info.adapter_name == "sms" || info.type_enum_id == "CbtSms";
        if key.is_empty() {
            if sms {
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
