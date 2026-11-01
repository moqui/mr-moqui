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

//! The bridge layer's web side (plans/Comms-Framework.md §4.3 "Webhooks", C4-SMS): what the host's `/channels/` routes
//! call, and the files that go with a network's messages.
//!
//! - [`webhook`]: a request to `/channels/{chatBridgeId}/webhook`, handed to the bridge's adapter
//!   ([`super::ChatBridgeAdapter::webhook`]) with its raw body, which the adapter checks (a signature over the bytes, a replay
//!   window) before it parses anything. An unknown bridge, or one whose adapter this build lacks, is 404; a disabled one
//!   503, so the network tries again once it is enabled.
//! - [`media_url`] and [`media`]: a file of a message that goes out by address (MMS), at
//!   `/channels/{chatBridgeId}/media/{token}` under `channels_public_url`. The token names the `ChatAttachment` and an
//!   expiry, signed with HMAC-SHA-256 under `channels_media_key`; only a file of a message delivered to that bridge is
//!   served, and none without the key or the public URL.
//! - `store#BridgeMedia`: a file a network's message names, fetched by its adapter ([`super::ChatBridgeAdapter::fetch_media`])
//!   within a size bound and kept as a `ChatAttachment` of the room with no uploader, for `ingest#Event` to send with the
//!   message.
//! - `send#BridgeDirect`: one message straight to an adapter, outside any room (a sign-in code by SMS): no delivery row,
//!   no retry; the caller has checked it.

use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;

use base64::Engine;
use hmac::{KeyInit, Mac};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use super::{
    adapter_of, bridge_info, current_factory, now_millis, param, stamp, text_of, DeliveryKind, DeliveryOutcome,
    DeliveryRequest, BRIDGE, DELIVERY,
};
use crate::context::ExecutionContextFactory;
use crate::entity::EntityFacade;

pub(super) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, store_bridge_media)?;
    rust_service!(map, send_bridge_direct)?;
    Ok(())
}

/// The base URL the network reaches this server at, for the files of messages that go out by address.
const PUBLIC_URL_PROPERTY: &str = "channels_public_url";
/// The key a file's address is signed with (generated when unset).
const MEDIA_KEY_PROPERTY: &str = "channels_media_key";
/// How long a file's address works, in minutes.
const MEDIA_MINUTES_PROPERTY: &str = "channels_media_minutes";
/// The largest file `store#BridgeMedia` takes when the caller names no bound.
const DEFAULT_MEDIA_BYTES: u64 = 5 * 1024 * 1024;
/// The file types kept as they are; any other is kept as `application/octet-stream`, so a file from a network is never
/// served as a page or a script.
const SAFE_TYPES: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/webp",
    "image/bmp",
    "image/heic",
    "image/heif",
    "audio/",
    "video/",
    "text/vcard",
    "text/x-vcard",
    "text/plain",
    "application/pdf",
];

// ---- the webhook ----

/// The shared-map key that names the bridge whose webhook the current context is working (`ingest#Event`
/// `transportVerified` checks it).
pub const WEBHOOK_MARK: &str = "_chatBridgeWebhook";

/// A request to a bridge's webhook route.
#[derive(Clone, Debug, Default)]
pub struct WebhookRequest {
    pub method: String,
    /// Header names in lower case; a repeated header joined with `, `.
    pub headers: Vec<(String, String)>,
    /// The raw body, as the network signed it.
    pub body: Vec<u8>,
    pub client_ip: String,
    /// The time the request is checked at (a test's clock).
    pub now_millis: i64,
}

impl WebhookRequest {
    /// A header's value, by its lower-case name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, value)| value.as_str())
    }
}

/// What the route answers. `auth_failure` counts the request for the host's web protection, as a refused login does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebhookResponse {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    pub auth_failure: bool,
}

impl WebhookResponse {
    pub fn text(status: u16, text: &str) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8".to_string(),
            body: text.as_bytes().to_vec(),
            auth_failure: false,
        }
    }

    pub fn json(status: u16, value: &serde_json::Value) -> Self {
        Self {
            status,
            content_type: "application/json".to_string(),
            body: value.to_string().into_bytes(),
            auth_failure: false,
        }
    }

    /// A refused signature or credential.
    pub fn refused(status: u16, text: &str) -> Self {
        Self {
            auth_failure: true,
            ..Self::text(status, text)
        }
    }
}

/// A file fetched from a network.
#[derive(Clone, Debug, Default)]
pub struct FetchedMedia {
    pub bytes: Vec<u8>,
    pub content_type: Option<String>,
}

/// A request to `/channels/{chatBridgeId}/webhook`, in the current context (the host's request worker): the bridge's
/// adapter answers it. See the module notes.
pub fn webhook(factory: &ExecutionContextFactory, bridge_id: &str, request: &WebhookRequest) -> WebhookResponse {
    let Some(facade) = factory.entity() else {
        return WebhookResponse::text(503, "the server is starting");
    };
    let row = facade
        .find(BRIDGE)
        .and_then(|find| find.eq("chatBridgeId", Value::string(bridge_id)).disable_authz().one());
    let row = match row {
        Ok(Some(row)) => row,
        Ok(None) => return WebhookResponse::text(404, "no such bridge"),
        Err(error) => {
            tracing::error!("chat bridge {bridge_id}: its webhook could not read the bridge: {error}");
            return WebhookResponse::text(500, "the bridge could not be read");
        }
    };
    let info = bridge_info(&row);
    let Some(adapter) = adapter_of(factory, &facade, &info) else {
        return WebhookResponse::text(404, "no such bridge");
    };
    if text_of(&row, "statusId").as_deref() == Some("CbsDisabled") {
        return WebhookResponse::text(503, "the bridge is disabled");
    }
    // what `ingest#Event` `transportVerified` asks for: only a batch of the bridge whose webhook this context is working
    // goes in without a token (a parameter alone, from any caller, is refused)
    let marked = crate::context::ExecutionContext::with_current(|ec| {
        ec.shared_map()
            .insert(WEBHOOK_MARK.to_string(), Value::string(bridge_id));
    })
    .is_some();
    let answer = std::panic::catch_unwind(AssertUnwindSafe(|| adapter.webhook(&info, request)));
    if marked {
        let _ = crate::context::ExecutionContext::with_current(|ec| ec.shared_map().shift_remove(WEBHOOK_MARK));
    }
    match answer {
        Ok(response) => response,
        Err(_) => {
            tracing::error!(
                "the chat bridge adapter {} failed on a webhook of {bridge_id}",
                adapter.name()
            );
            WebhookResponse::text(500, "the request failed")
        }
    }
}

// ---- the files of messages that go out ----

/// A file served at `/channels/{chatBridgeId}/media/{token}`.
#[derive(Clone, Debug)]
pub struct MediaFile {
    pub content_type: String,
    pub file_name: String,
    pub bytes: Vec<u8>,
}

fn media_key() -> Option<String> {
    let key = moqui_lib::env::get_prop_or_env(MEDIA_KEY_PROPERTY);
    (!key.trim().is_empty() && !key.ends_with("CHANGEME")).then_some(key)
}

fn media_signature(key: &str, bridge_id: &str, attachment_id: &str, expiry: i64) -> Option<Vec<u8>> {
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(key.as_bytes()).ok()?;
    mac.update(format!("{bridge_id}|{attachment_id}|{expiry}").as_bytes());
    Some(mac.finalize().into_bytes().to_vec())
}

/// The address a network fetches file `attachment_id` of a message to bridge `bridge_id` at, good for
/// `channels_media_minutes` from `now`; none without `channels_public_url` or `channels_media_key`.
pub fn media_url(bridge_id: &str, attachment_id: &str, now: i64) -> Option<String> {
    let base = moqui_lib::env::get_prop_or_env(PUBLIC_URL_PROPERTY);
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() || !(base.starts_with("https://") || base.starts_with("http://")) {
        return None;
    }
    let key = media_key()?;
    let minutes: i64 = moqui_lib::env::get_prop_or_env(MEDIA_MINUTES_PROPERTY)
        .trim()
        .parse()
        .unwrap_or(60)
        .clamp(1, 7 * 24 * 60);
    let expiry = now + minutes * 60_000;
    let signature = media_signature(&key, bridge_id, attachment_id, expiry)?;
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature);
    Some(format!(
        "{base}/channels/{bridge_id}/media/{attachment_id}.{expiry}.{signature}"
    ))
}

/// Whether `token` (`attachment.expiry.signature`) is a file address of bridge `bridge_id` that still works at `now`;
/// the attachment id when it is.
pub fn media_token_valid(bridge_id: &str, token: &str, now: i64) -> Option<String> {
    let mut parts = token.splitn(3, '.');
    let (attachment, expiry, signature) = (parts.next()?, parts.next()?, parts.next()?);
    if attachment.is_empty()
        || attachment.len() > 64
        || !attachment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return None;
    }
    let expiry: i64 = expiry.parse().ok()?;
    if expiry < now {
        return None;
    }
    let given = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(signature)
        .ok()?;
    let key = media_key()?;
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(key.as_bytes()).ok()?;
    mac.update(format!("{bridge_id}|{attachment}|{expiry}").as_bytes());
    mac.verify_slice(&given).ok()?;
    Some(attachment.to_string())
}

/// The file a token names, in the current context: only a file of a message with a delivery to that bridge, while the
/// token works.
pub fn media(factory: &ExecutionContextFactory, bridge_id: &str, token: &str, now: i64) -> Option<MediaFile> {
    let attachment_id = media_token_valid(bridge_id, token, now)?;
    let facade = factory.entity()?;
    let row = facade
        .find("moqui.chat.ChatAttachment")
        .and_then(|find| {
            find.eq("chatAttachmentId", Value::string(&attachment_id))
                .disable_authz()
                .one()
        })
        .ok()
        .flatten()?;
    let room = text_of(&row, "chatRoomId")?;
    let seq = super::int_of(&row, "eventSeq")?;
    let delivered = facade
        .find(DELIVERY)
        .and_then(|find| {
            find.eq("chatRoomId", Value::string(&room))
                .eq("eventSeq", Value::I64(seq))
                .eq("chatBridgeId", Value::string(bridge_id))
                .disable_authz()
                .count()
        })
        .unwrap_or(0);
    if delivered == 0 {
        return None;
    }
    let location = text_of(&row, "location")?;
    let bytes = match factory.resource().location_bytes(&location) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return None,
        Err(error) => {
            tracing::warn!("chat file {attachment_id} ({location}) could not be read for bridge {bridge_id}: {error}");
            return None;
        }
    };
    Some(MediaFile {
        content_type: safe_type(text_of(&row, "contentType").as_deref()),
        file_name: text_of(&row, "fileName").unwrap_or_else(|| "file".to_string()),
        bytes,
    })
}

// ---- store#BridgeMedia ----

/// A content type as it is kept: one of [`SAFE_TYPES`], else `application/octet-stream`.
pub(super) fn safe_type(given: Option<&str>) -> String {
    let base = given
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let safe = !base.is_empty()
        && base.len() <= 100
        && base
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/.+-_".contains(&byte))
        && SAFE_TYPES.iter().any(|known| {
            if known.ends_with('/') {
                base.starts_with(known) && base.len() > known.len()
            } else {
                base == *known
            }
        });
    if safe {
        base
    } else {
        "application/octet-stream".to_string()
    }
}

/// A file name to keep: the last part of what was given, with no control characters, at most 120 characters; else
/// `file` with the type's extension.
fn safe_name(given: Option<&str>, content_type: &str) -> String {
    let name: String = given
        .unwrap_or_default()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    let name = name.trim();
    if !name.is_empty() && name != "." && name != ".." {
        let start = name.chars().count().saturating_sub(120);
        return name.chars().skip(start).collect();
    }
    let extension = match content_type {
        "image/jpeg" => ".jpg",
        "image/png" => ".png",
        "image/gif" => ".gif",
        "image/webp" => ".webp",
        "application/pdf" => ".pdf",
        "text/vcard" | "text/x-vcard" => ".vcf",
        "text/plain" => ".txt",
        _ => "",
    };
    format!("file{extension}")
}

/// `store#BridgeMedia`: fetch the file at `url` of a message from bridge `chatBridgeId` (its adapter's
/// [`fetch_media`](super::ChatBridgeAdapter::fetch_media), at most `maxBytes`) and keep it as a `ChatAttachment` of room
/// `chatRoomId` with no uploader and no event yet, at `chat_attachment_root`/(room)/(attachment)/(name), in the caller's
/// transaction. Out: chatAttachmentId, fileName, contentType, fileSize; or `error` (and no chatAttachmentId) when the file
/// cannot be fetched, which is not an error of the call, so the caller's transaction (the message's) goes on without it.
fn store_bridge_media(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let factory = current_factory().ok_or_else(|| LibError::new("no execution context"))?;
    let bridge_id = param(parameters, "chatBridgeId").unwrap_or_default();
    let room = param(parameters, "chatRoomId").unwrap_or_default();
    let url = param(parameters, "url").unwrap_or_default();
    if bridge_id.is_empty() || room.is_empty() || url.is_empty() {
        return Err(LibError::new("A file from a bridge names its bridge, room and address"));
    }
    let max = param(parameters, "maxBytes")
        .and_then(|text| text.parse::<u64>().ok())
        .filter(|max| *max > 0)
        .unwrap_or(DEFAULT_MEDIA_BYTES);
    let row = facade
        .find(BRIDGE)?
        .eq("chatBridgeId", Value::string(&bridge_id))
        .disable_authz()
        .one()?
        .ok_or_else(|| LibError::new(format!("Chat bridge {bridge_id} not found")))?;
    let info = bridge_info(&row);
    let adapter = adapter_of(&factory, facade, &info)
        .ok_or_else(|| LibError::new(format!("No adapter for bridge {bridge_id}")))?;
    let not_fetched = |error: String| {
        Ok(BTreeMap::from([(
            "error".to_string(),
            Value::string(format!("The file was not fetched: {error}")),
        )]))
    };
    let fetched = match std::panic::catch_unwind(AssertUnwindSafe(|| adapter.fetch_media(&info, &url, max))) {
        Ok(Ok(fetched)) => fetched,
        Ok(Err(error)) => return not_fetched(error),
        Err(_) => return not_fetched("the adapter failed".to_string()),
    };
    if u64::try_from(fetched.bytes.len()).unwrap_or(u64::MAX) > max {
        return not_fetched(format!("it is over {max} bytes"));
    }
    let content_type = safe_type(
        param(parameters, "contentType")
            .as_deref()
            .or(fetched.content_type.as_deref()),
    );
    let file_name = safe_name(param(parameters, "fileName").as_deref(), &content_type);
    let mut attachment = facade.make_value("moqui.chat.ChatAttachment")?;
    attachment.set_sequenced_id_primary(facade)?;
    let id = text_of(&attachment, "chatAttachmentId").unwrap_or_default();
    let root = moqui_lib::env::get_property("chat_attachment_root")
        .filter(|root| !root.trim().is_empty())
        .unwrap_or_else(|| "dbresource://chat".to_string());
    let location = format!("{}/{room}/{id}/{file_name}", root.trim_end_matches('/'));
    factory
        .resource()
        .write_bytes(&location, &fetched.bytes)
        .map_err(|error| LibError::new(format!("The file was not kept: {}", error.message())))?;
    let size = i64::try_from(fetched.bytes.len()).unwrap_or(i64::MAX);
    attachment.set("chatRoomId", Value::string(&room));
    attachment.set("location", Value::string(&location));
    attachment.set("fileName", Value::string(&file_name));
    attachment.set("contentType", Value::string(&content_type));
    attachment.set("fileSize", Value::I64(size));
    attachment.set("uploadedDate", stamp(now_millis()));
    attachment.create(facade)?;
    Ok(BTreeMap::from([
        ("chatAttachmentId".to_string(), Value::string(id)),
        ("fileName".to_string(), Value::string(file_name)),
        ("contentType".to_string(), Value::string(content_type)),
        ("fileSize".to_string(), Value::I64(size)),
    ]))
}

// ---- send#BridgeDirect ----

/// `send#BridgeDirect`: one message straight to bridge `chatBridgeId`'s adapter, to `externalRoomId`, outside any room:
/// `text`, `idempotencyKey` (made when empty), `senderUserId`. No delivery row is kept and nothing is tried again; the
/// caller checked what the network's rules need. Out: outcome (accepted, retry, failed, uncertain), remoteEventId,
/// reasonCode, error.
fn send_bridge_direct(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let factory = current_factory().ok_or_else(|| LibError::new("no execution context"))?;
    let bridge_id = param(parameters, "chatBridgeId").unwrap_or_default();
    let answer = |outcome: &str, remote: Option<String>, reason: Option<&str>, error: Option<String>| {
        let text = |value: Option<String>| value.map_or(Value::Null, Value::string);
        Ok(BTreeMap::from([
            ("outcome".to_string(), Value::string(outcome)),
            ("remoteEventId".to_string(), text(remote)),
            ("reasonCode".to_string(), text(reason.map(str::to_string))),
            ("error".to_string(), text(error)),
        ]))
    };
    let Some(row) = facade
        .find(BRIDGE)?
        .eq("chatBridgeId", Value::string(&bridge_id))
        .disable_authz()
        .one()?
    else {
        return answer(
            "failed",
            None,
            Some("unknownBridge"),
            Some("Unknown chat bridge".to_string()),
        );
    };
    if text_of(&row, "statusId").as_deref() == Some("CbsDisabled") {
        return answer(
            "failed",
            None,
            Some("bridgeDisabled"),
            Some("The chat bridge is disabled".to_string()),
        );
    }
    let info = bridge_info(&row);
    let Some(adapter) = adapter_of(&factory, facade, &info) else {
        return answer(
            "failed",
            None,
            Some("noAdapter"),
            Some(format!("No chat bridge adapter for {bridge_id} in this build")),
        );
    };
    let key = param(parameters, "idempotencyKey").unwrap_or_else(|| {
        let mut bytes = [0u8; 12];
        let _ = getrandom::fill(&mut bytes);
        format!(
            "moqui-direct:{}",
            bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>()
        )
    });
    let request = DeliveryRequest {
        chat_bridge_delivery_id: String::new(),
        idempotency_key: key,
        chat_room_id: String::new(),
        event_seq: 0,
        external_room_id: param(parameters, "externalRoomId").unwrap_or_default(),
        kind: DeliveryKind::Message,
        text: parameters
            .get("text")
            .filter(|value| !is_empty(value))
            .map(to_plain_string),
        html: None,
        sender_name: None,
        sender_user_id: param(parameters, "senderUserId"),
        attachments: Vec::new(),
        target_remote_id: None,
        reaction_key: None,
        attempt: 1,
    };
    match std::panic::catch_unwind(AssertUnwindSafe(|| adapter.deliver(&info, &request))) {
        Ok(DeliveryOutcome::Accepted { remote_event_id }) => answer("accepted", Some(remote_event_id), None, None),
        Ok(DeliveryOutcome::Retry { error, .. }) => answer("retry", None, Some("retry"), Some(error)),
        Ok(DeliveryOutcome::Failed { reason_code, error }) => answer("failed", None, Some(&reason_code), Some(error)),
        Ok(DeliveryOutcome::Uncertain { error }) => answer("uncertain", None, Some("uncertain"), Some(error)),
        Err(_) => answer(
            "uncertain",
            None,
            Some("uncertain"),
            Some("the adapter failed while it sent".to_string()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{media_token_valid, media_url, safe_name, safe_type};

    #[test]
    fn a_file_address_is_signed_for_its_bridge_and_runs_out() {
        moqui_lib::env::set_property("channels_public_url", "https://example.test/");
        moqui_lib::env::set_property("channels_media_key", "a-test-key");
        moqui_lib::env::set_property("channels_media_minutes", "10");
        let url = media_url("B1", "100200", 1_000).unwrap();
        let token = url.rsplit('/').next().unwrap();
        assert!(url.starts_with("https://example.test/channels/B1/media/100200."));
        assert_eq!(media_token_valid("B1", token, 2_000).as_deref(), Some("100200"));
        // another bridge, after it runs out, a changed attachment or signature
        assert!(media_token_valid("B2", token, 2_000).is_none());
        assert!(media_token_valid("B1", token, 1_000 + 10 * 60_000 + 1).is_none());
        assert!(media_token_valid("B1", &token.replacen("100200", "100201", 1), 2_000).is_none());
        assert!(media_token_valid("B1", &format!("{token}x"), 2_000).is_none());
        assert!(media_token_valid("B1", "../x.1.2", 2_000).is_none());
    }

    #[test]
    fn a_file_from_a_network_is_never_kept_as_a_page() {
        assert_eq!(safe_type(Some("image/jpeg; q=1")), "image/jpeg");
        assert_eq!(safe_type(Some("video/mp4")), "video/mp4");
        assert_eq!(safe_type(Some("text/html")), "application/octet-stream");
        assert_eq!(safe_type(Some("image/svg+xml")), "application/octet-stream");
        assert_eq!(safe_type(Some("audio/")), "application/octet-stream");
        assert_eq!(safe_type(None), "application/octet-stream");
        assert_eq!(
            safe_name(Some("https://x/y/photo.jpg?sig=1"), "image/jpeg"),
            "photo.jpg"
        );
        assert_eq!(safe_name(Some(".."), "image/png"), "file.png");
        assert_eq!(safe_name(None, "text/plain"), "file.txt");
    }
}
