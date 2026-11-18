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

//! The GroupMe adapter, `groupme` (plans/Comms-Messaging.md §18.3 phase 14, docs/interfaces/GroupMe.md): one person's
//! GroupMe account, on GroupMe's public API v3 with their access token, as a bridge of type `CbtGroupMe`. Not yet
//! tested with GroupMe: it is built against a fake GroupMe (`tests/groupme.rs`).
//!
//! - **The rooms.** Each group (`g:<group id>`), each Topic (`g:<topic id>`, "Group › Topic") and each direct
//!   conversation (`dm:<their id>`, its chat id the mapping's `externalThreadId`) is a private room the bridge's owner
//!   (`configJson.ownerUserId`) owns, made by `org.moqui.chat.GroupMeServices.ensure#Room`.
//! - **In** ([`push`], [`events`]): GroupMe's push channel (Faye long-polling over HTTPS, `push.groupme.com/faye`), one
//!   thread a bridge that [`ChatBridgeAdapter::start`] spawns and that holds the bridge's lease
//!   (`moqui.edge.EdgeSenderLock` `groupme:<bridge>`), so one node of a cluster connects and another takes over when it
//!   stops. After each subscription, and every `syncMinutes`, a catch-up through the messages API (the polling
//!   fallback). Each message is one `ingest#Event` batch (`gm:<message id>`) under the bridge's transport mark: the
//!   adapter's own TLS connection, signed in with the token, is the transport check. No webhook.
//! - **Out** ([`ChatBridgeAdapter::deliver`]): only the owner's events (`notBridgeOwner`). A message with the
//!   delivery's idempotency key as `source_guid`, pictures through the image service, `@room` as `@all` with every
//!   member mentioned; a heart reaction as a like, its removal as an unlike; nothing else (`reactionNotCarried`,
//!   `fileNotCarried`, `bridgeUnsupported`).
//!
//! The settings, from the bridge's `configJson` ([`Config`]): `accessToken` and `ownerUserId` (both required),
//! `groupIds` (empty: every group), `skipGroupIds`, `topics` and `directMessages` (both true), `maxRooms` (200; read by
//! `ensure#Room`), `syncMinutes` (60), `sendsPerMinute` (20), `maxImageBytes` (10 MB), `maxInboundMediaBytes` (20 MiB at
//! most), `apiBaseUrl`, `imageBaseUrl`, `pushUrl`, and for a test's fake `allowInternalAddresses` (never on
//! production), `mediaBaseUrl` (where its files are) and `push` (false: no thread; a test drives [`push::Session`]).

pub mod api;
pub mod events;
pub mod push;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use moqui_core::context::ExecutionContext;
use moqui_core::service::chat_bridge::{
    delivery_file, BridgeInfo, Capabilities, ChatBridgeAdapter, DeliveryKind, DeliveryOutcome, DeliveryRequest,
    Encryption, FetchedMedia,
};
use moqui_lib::http::RestClient;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;
use serde_json::{json, Value as Json};

use crate::sms::{flag_of, number_of, text_of};
use api::{Api, ApiError};

/// The adapter's name, a `ChatBridge`'s `adapterName`.
pub const ADAPTER: &str = "groupme";
/// The most a file fetched from GroupMe may be, whatever a bridge says.
pub const MEDIA_CAP: u64 = 20 * 1024 * 1024;
/// GroupMe's longest message (dev.groupme.com, messages create).
pub const MAX_TEXT: usize = 1000;
/// The prefix of every `source_guid` this server sends (the delivery's idempotency key): a message of the owner's that
/// comes back with it is the echo of a send.
pub const OUR_GUID: &str = "moqui-chat:";
/// The reactions GroupMe carries: a like is a heart.
pub const HEARTS: [&str; 4] = ["❤️", "❤", "♥️", "♥"];
/// GroupMe's two file hosts, the only ones a message's files are fetched from.
pub const MEDIA_HOSTS: [&str; 2] = ["https://i.groupme.com/", "https://v.groupme.com/"];
const MEMBERS_KEPT: Duration = Duration::from_secs(600);

// ---- the settings ----

/// A GroupMe bridge's settings (see the module notes).
#[derive(Clone, Debug)]
pub struct Config {
    pub token: Option<String>,
    pub owner: Option<String>,
    /// No trailing `/`.
    pub api_base: String,
    pub image_base: String,
    pub push_url: String,
    /// A test's fake: its files' base, else none.
    pub media_base: Option<String>,
    pub allow_internal: bool,
    /// Whether [`ChatBridgeAdapter::start`] spawns the push thread.
    pub push: bool,
    pub group_ids: Vec<String>,
    pub skip_group_ids: Vec<String>,
    pub topics: bool,
    pub direct_messages: bool,
    pub sync_minutes: u64,
    pub sends_per_minute: u32,
    pub max_image_bytes: i64,
    pub max_inbound_media_bytes: u64,
}

fn list_of(config: &Json, key: &str) -> Vec<String> {
    match config.get(key) {
        Some(Json::Array(values)) => values
            .iter()
            .filter_map(|value| match value {
                Json::String(text) => Some(text.trim().to_string()),
                Json::Number(number) => Some(number.to_string()),
                _ => None,
            })
            .filter(|text| !text.is_empty())
            .collect(),
        Some(Json::String(text)) => text
            .split(',')
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// A yes/no setting that is true unless it is false (or the text `false`).
fn on_unless_false(config: &Json, key: &str) -> bool {
    !config
        .get(key)
        .is_some_and(|value| value.as_bool() == Some(false) || value.as_str() == Some("false"))
}

impl Config {
    pub fn of(config: &Json) -> Self {
        let url = |key: &str, default: &str| {
            text_of(config, key)
                .unwrap_or_else(|| default.to_string())
                .trim_end_matches('/')
                .to_string()
        };
        let allow_internal = !moqui_core::web::is_production() && flag_of(config, "allowInternalAddresses");
        Self {
            token: text_of(config, "accessToken"),
            owner: text_of(config, "ownerUserId"),
            api_base: url("apiBaseUrl", "https://api.groupme.com/v3"),
            image_base: url("imageBaseUrl", "https://image.groupme.com"),
            push_url: url("pushUrl", "https://push.groupme.com/faye"),
            media_base: allow_internal
                .then(|| text_of(config, "mediaBaseUrl"))
                .flatten()
                .map(|base| base.trim_end_matches('/').to_string()),
            allow_internal,
            push: on_unless_false(config, "push"),
            group_ids: list_of(config, "groupIds"),
            skip_group_ids: list_of(config, "skipGroupIds"),
            topics: on_unless_false(config, "topics"),
            direct_messages: on_unless_false(config, "directMessages"),
            sync_minutes: number_of(config, "syncMinutes")
                .and_then(|n| u64::try_from(n).ok())
                .filter(|n| *n > 0)
                .unwrap_or(60),
            sends_per_minute: number_of(config, "sendsPerMinute")
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(20),
            max_image_bytes: number_of(config, "maxImageBytes")
                .filter(|n| *n > 0)
                .unwrap_or(10 * 1024 * 1024),
            max_inbound_media_bytes: number_of(config, "maxInboundMediaBytes")
                .and_then(|n| u64::try_from(n).ok())
                .filter(|n| *n > 0)
                .unwrap_or(MEDIA_CAP)
                .min(MEDIA_CAP),
        }
    }

    /// What is missing or unsafe for the bridge to connect.
    ///
    /// # Errors
    ///
    /// The first problem, in words.
    pub fn check(&self) -> Result<(), String> {
        if self.token.is_none() {
            return Err("The GroupMe bridge has no accessToken".to_string());
        }
        if self.owner.is_none() {
            return Err("The GroupMe bridge has no ownerUserId".to_string());
        }
        // the token goes with every request, so never in clear text, except to a test's fake (allowInternalAddresses)
        for base in [&self.api_base, &self.image_base, &self.push_url] {
            if !(base.starts_with("https://") || (self.allow_internal && base.starts_with("http://"))) {
                return Err(format!("The GroupMe bridge's {base} is not an https address"));
            }
        }
        Ok(())
    }

    /// Whether the group (or the Topic's group) `group_id` is one the bridge carries.
    pub fn chosen(&self, group_id: &str) -> bool {
        (self.group_ids.is_empty() || self.group_ids.iter().any(|id| id == group_id))
            && !self.skip_group_ids.iter().any(|id| id == group_id)
    }

    /// The token as an error message may name it: never.
    pub fn scrub(&self, text: &str) -> String {
        match self.token.as_deref() {
            Some(token) if !token.is_empty() => text.replace(token, "[token]"),
            _ => text.to_string(),
        }
    }

    /// What a connection depends on: a change starts a new handshake.
    pub fn fingerprint(&self) -> String {
        let text = format!(
            "{}|{}|{}|{}",
            self.token.as_deref().unwrap_or_default(),
            self.owner.as_deref().unwrap_or_default(),
            self.api_base,
            self.push_url
        );
        let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, text.as_bytes());
        digest
            .as_ref()
            .iter()
            .take(12)
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

// ---- a room's id ----

/// What a mapping's `externalRoomId` names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Room {
    /// A group or a Topic: its id, which its messages and likes go to.
    Group(String),
    /// A direct conversation with the person of this GroupMe id.
    Direct(String),
}

impl Room {
    pub fn parse(external_room_id: &str) -> Option<Self> {
        let valid = |id: &str| {
            !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        };
        if let Some(id) = external_room_id.strip_prefix("g:") {
            return valid(id).then(|| Room::Group(id.to_string()));
        }
        if let Some(id) = external_room_id.strip_prefix("dm:") {
            return valid(id).then(|| Room::Direct(id.to_string()));
        }
        None
    }

    pub fn external_id(&self) -> String {
        match self {
            Room::Group(id) => format!("g:{id}"),
            Room::Direct(id) => format!("dm:{id}"),
        }
    }
}

/// A direct conversation's chat id when the mapping has none: the two GroupMe ids, the smaller number first
/// (`a+b`, from the community's docs: not confirmed).
pub fn chat_id(own: &str, other: &str) -> String {
    let (a, b) = match (own.parse::<u64>(), other.parse::<u64>()) {
        (Ok(x), Ok(y)) if y < x => (other, own),
        _ => (own, other),
    };
    format!("{a}+{b}")
}

// ---- the adapter ----

/// What the adapter keeps between calls, shared with its push threads.
#[derive(Default)]
pub struct Inner {
    public: OnceLock<RestClient>,
    internal: OnceLock<RestClient>,
    /// Each bridge's push thread's stop flag.
    threads: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Each bridge's token user (by its config's fingerprint): `/users/me`'s id.
    own: Mutex<HashMap<String, (String, String)>>,
    /// Each group's members' ids, for `@all`, for a while.
    members: Mutex<HashMap<String, (Instant, Vec<String>)>>,
}

impl Inner {
    /// The guarded client: public addresses only, but a test's fake.
    pub fn client(&self, allow_internal: bool) -> &RestClient {
        let make = |deny: bool| {
            RestClient::new()
                .timeout(Duration::from_secs(30))
                .connect_timeout(Duration::from_secs(10))
                .max_response_bytes(MEDIA_CAP)
                .redirects(3, true)
                .deny_internal_addresses(deny)
        };
        if allow_internal {
            self.internal.get_or_init(|| make(false))
        } else {
            self.public.get_or_init(|| make(true))
        }
    }

    pub fn api<'a>(&'a self, config: &'a Config) -> Api<'a> {
        Api::new(self.client(config.allow_internal), config)
    }

    /// The token's GroupMe user id, asked once a configuration.
    ///
    /// # Errors
    ///
    /// GroupMe's refusal or the request's failure.
    pub fn own_id(&self, bridge_id: &str, config: &Config) -> Result<String, ApiError> {
        let fingerprint = config.fingerprint();
        if let Some((kept, id)) = lock(&self.own).get(bridge_id) {
            if *kept == fingerprint {
                return Ok(id.clone());
            }
        }
        let me = self.api(config).get("/users/me")?;
        let id = api::id_of(&me, "id").ok_or_else(|| ApiError::Status {
            status: 502,
            errors: "GroupMe's /users/me named no id".to_string(),
        })?;
        lock(&self.own).insert(bridge_id.to_string(), (fingerprint, id.clone()));
        Ok(id)
    }

    /// The members' GroupMe ids of group `group_id`, kept 10 minutes.
    fn members_of(&self, config: &Config, group_id: &str) -> Result<Vec<String>, ApiError> {
        if let Some((at, ids)) = lock(&self.members).get(group_id) {
            if at.elapsed() < MEMBERS_KEPT {
                return Ok(ids.clone());
            }
        }
        let group = self.api(config).get(&format!("/groups/{group_id}"))?;
        let ids: Vec<String> = group
            .get("members")
            .and_then(Json::as_array)
            .map(|members| {
                members
                    .iter()
                    .filter_map(|member| api::id_of(member, "user_id"))
                    .collect()
            })
            .unwrap_or_default();
        lock(&self.members).insert(group_id.to_string(), (Instant::now(), ids.clone()));
        Ok(ids)
    }
}

/// The GroupMe adapter. See the module notes.
#[derive(Clone, Default)]
pub struct GroupMeAdapter {
    inner: Arc<Inner>,
}

impl GroupMeAdapter {
    pub fn inner(&self) -> &Arc<Inner> {
        &self.inner
    }
}

fn failed(reason: &str, error: impl Into<String>) -> DeliveryOutcome {
    DeliveryOutcome::Failed {
        reason_code: reason.to_string(),
        error: error.into(),
    }
}

impl ChatBridgeAdapter for GroupMeAdapter {
    fn name(&self) -> &str {
        ADAPTER
    }

    fn capabilities(&self, bridge: &BridgeInfo) -> Capabilities {
        Capabilities {
            edit: false,
            delete: false,
            reaction: true,
            // GroupMe dedupes a source_guid for a minute only, shorter than a delivery's lease
            idempotent: false,
            sends_per_minute: Config::of(&bridge.config).sends_per_minute,
            // GroupMe has no end-to-end encryption, and the bridge reads every message
            encryption: Encryption::None,
            group_max: 0,
        }
    }

    fn start(&self, bridge: &BridgeInfo) -> Result<(), String> {
        let config = Config::of(&bridge.config);
        config.check()?;
        if !config.push {
            return Ok(());
        }
        let Some(factory) = ExecutionContext::with_current(|ec| Arc::downgrade(ec.factory())) else {
            return Err("The GroupMe bridge started outside a context".to_string());
        };
        let mut threads = lock(&self.inner.threads);
        if threads
            .get(&bridge.chat_bridge_id)
            .is_some_and(|stop| !stop.load(Ordering::SeqCst))
        {
            return Ok(());
        }
        let stop = Arc::new(AtomicBool::new(false));
        threads.insert(bridge.chat_bridge_id.clone(), Arc::clone(&stop));
        drop(threads);
        push::spawn(Arc::clone(&self.inner), factory, bridge.chat_bridge_id.clone(), stop)
    }

    fn stop(&self, chat_bridge_id: &str) {
        if let Some(stop) = lock(&self.inner.threads).remove(chat_bridge_id) {
            stop.store(true, Ordering::SeqCst);
        }
    }

    fn deliver(&self, bridge: &BridgeInfo, request: &DeliveryRequest) -> DeliveryOutcome {
        let config = Config::of(&bridge.config);
        if let Err(error) = config.check() {
            return DeliveryOutcome::Retry {
                error,
                not_connected: true,
            };
        }
        let Some(room) = Room::parse(&request.external_room_id) else {
            return failed("badMapping", "The room's mapping is not a GroupMe group or person");
        };
        // only the owner speaks for the account: no other local user posts as them
        let owner = config.owner.as_deref().unwrap_or_default();
        if request.sender_user_id.as_deref() != Some(owner) {
            return failed(
                "notBridgeOwner",
                format!("GroupMe messages go from {owner}'s account only: it is theirs"),
            );
        }
        match request.kind {
            DeliveryKind::Message => self.send(bridge, &config, &room, request),
            DeliveryKind::Reaction => self.like(bridge, &config, &room, request, true),
            DeliveryKind::Redaction if request.reaction_key.is_some() => {
                self.like(bridge, &config, &room, request, false)
            }
            DeliveryKind::Edit | DeliveryKind::Redaction => failed(
                "bridgeUnsupported",
                "A GroupMe message cannot be changed from here once sent",
            ),
        }
    }

    fn fetch_media(&self, bridge: &BridgeInfo, url: &str, max_bytes: u64) -> Result<FetchedMedia, String> {
        let config = Config::of(&bridge.config);
        let allowed = MEDIA_HOSTS.iter().any(|host| url.starts_with(host))
            || config
                .media_base
                .as_deref()
                .is_some_and(|base| url.starts_with(&format!("{base}/")));
        if !allowed {
            return Err("a file not on GroupMe's file hosts is not fetched".to_string());
        }
        let max = max_bytes.min(config.max_inbound_media_bytes);
        let response = self
            .inner
            .client(config.allow_internal)
            .get(url)
            .timeout(Duration::from_secs(20))
            .max_response_bytes(max)
            .send(false)
            .map_err(|error| {
                if error.message().contains("larger than the limit") {
                    format!("the file is over {max} bytes")
                } else {
                    config.scrub(error.message())
                }
            })?;
        if response.status() != 200 {
            return Err(format!("the file's address answered {}", response.status()));
        }
        if u64::try_from(response.bytes().len()).unwrap_or(u64::MAX) > max {
            return Err(format!("the file is over {max} bytes"));
        }
        let content_type = response
            .response_headers()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| value.clone());
        Ok(FetchedMedia {
            bytes: response.bytes().to_vec(),
            content_type,
        })
    }
}

/// The text with `@room` as `@all`, and where `@all` is in it (its first character and length, in characters), when
/// the message mentions the room.
pub fn all_mention(text: &str, html: Option<&str>) -> (String, Option<(usize, usize)>) {
    let mentions_room = html.is_some_and(|html| html.contains("data-mention-id=\"@room\""));
    if !mentions_room {
        return (text.to_string(), None);
    }
    let text = if text.contains("@room") {
        text.replacen("@room", "@all", 1)
    } else if text.contains("@all") {
        text.to_string()
    } else {
        format!("@all {text}")
    };
    let at = text.find("@all").map(|byte| text[..byte].chars().count());
    (text, at.map(|at| (at, 4)))
}

impl GroupMeAdapter {
    /// A message: its text, its pictures through the image service, `@room` as `@all`.
    fn send(&self, bridge: &BridgeInfo, config: &Config, room: &Room, request: &DeliveryRequest) -> DeliveryOutcome {
        let (text, all) = all_mention(
            request.text.as_deref().unwrap_or_default().trim(),
            request.html.as_deref(),
        );
        if text.chars().count() > MAX_TEXT {
            return failed(
                "textTooLong",
                format!("A GroupMe message is at most {MAX_TEXT} characters"),
            );
        }
        // the files first, all or none: GroupMe takes pictures only from here
        for file in &request.attachments {
            let image = file
                .content_type
                .as_deref()
                .is_some_and(|kind| kind.to_ascii_lowercase().starts_with("image/"));
            if !image {
                return failed(
                    "fileNotCarried",
                    format!(
                        "GroupMe takes only pictures from this server: {} was not sent",
                        file.file_name.as_deref().unwrap_or("a file")
                    ),
                );
            }
            if file.file_size.unwrap_or(0) > config.max_image_bytes {
                return failed(
                    "mediaTooLarge",
                    format!("A picture to GroupMe is at most {} bytes", config.max_image_bytes),
                );
            }
        }
        if text.is_empty() && request.attachments.is_empty() {
            return failed("emptyMessage", "A GroupMe message has text or a picture");
        }
        let api = self.inner.api(config);
        let mut attachments: Vec<Json> = Vec::new();
        if !request.attachments.is_empty() {
            let Some(factory) = ExecutionContext::with_current(|ec| Arc::clone(ec.factory())) else {
                return DeliveryOutcome::Retry {
                    error: "no context to read the files in".to_string(),
                    not_connected: false,
                };
            };
            for file in &request.attachments {
                let Some(media) = delivery_file(&factory, &bridge.chat_bridge_id, &file.chat_attachment_id) else {
                    return failed("fileMissing", "A file of the message could not be read");
                };
                match api.upload_image(media.bytes, &media.content_type) {
                    Ok(url) => attachments.push(json!({"type": "image", "url": url})),
                    Err(error) => return error.outcome("The picture was not taken by GroupMe's image service"),
                }
            }
        }
        if let (Some((at, length)), Room::Group(group)) = (all, room) {
            match self.inner.members_of(config, group) {
                Ok(members) => {
                    let own = self.inner.own_id(&bridge.chat_bridge_id, config).ok();
                    let ids: Vec<String> = members.into_iter().filter(|id| Some(id) != own.as_ref()).collect();
                    if !ids.is_empty() {
                        let loci: Vec<Json> = ids.iter().map(|_| json!([at, length])).collect();
                        attachments.push(json!({"type": "mentions", "user_ids": ids, "loci": loci}));
                    }
                }
                Err(error) => return error.outcome("The group's members could not be read for @all"),
            }
        }
        let guid = &request.idempotency_key;
        let (path, body, key) = match room {
            Room::Group(group) => (
                format!("/groups/{group}/messages"),
                json!({"message": {"source_guid": guid, "text": text, "attachments": attachments}}),
                "message",
            ),
            Room::Direct(other) => (
                "/direct_messages".to_string(),
                json!({"direct_message": {"source_guid": guid, "recipient_id": other, "text": text,
                    "attachments": attachments}}),
                "direct_message",
            ),
        };
        let found = || match self.find_sent(config, room, guid) {
            Some(id) => DeliveryOutcome::Accepted { remote_event_id: id },
            None => DeliveryOutcome::Uncertain {
                error: "GroupMe's answer was lost, and the message was not found".to_string(),
            },
        };
        match api.post(&path, &body) {
            Ok(answer) => {
                let sent = answer.get(key).unwrap_or(&answer);
                match api::id_of(sent, "id") {
                    Some(id) => {
                        if let (Room::Direct(_), None, Some(chat)) = (
                            room,
                            request.external_thread_id.as_deref(),
                            api::id_of(sent, "conversation_id").or_else(|| api::id_of(sent, "chat_id")),
                        ) {
                            if let Err(error) = crate::sms::store_thread(bridge, &request.external_room_id, &chat) {
                                tracing::warn!(
                                    "GroupMe bridge {}: the conversation's chat id was not kept: {error}",
                                    bridge.chat_bridge_id
                                );
                            }
                        }
                        DeliveryOutcome::Accepted { remote_event_id: id }
                    }
                    None => found(),
                }
            }
            // a copy within the minute (409), GroupMe's own error (500) or a lost answer: it may be there
            Err(ApiError::Status { status: 409 | 500, .. }) | Err(ApiError::Lost(_)) => found(),
            Err(error) => error.outcome("GroupMe did not take the message"),
        }
    }

    /// The id of the message this server sent with `guid` as its `source_guid`, among the room's latest 20.
    fn find_sent(&self, config: &Config, room: &Room, guid: &str) -> Option<String> {
        let api = self.inner.api(config);
        let (path, key) = match room {
            Room::Group(group) => (format!("/groups/{group}/messages?limit=20"), "messages"),
            Room::Direct(other) => (format!("/direct_messages?other_user_id={other}"), "direct_messages"),
        };
        let page = api.get(&path).ok()?;
        page.get(key)?
            .as_array()?
            .iter()
            .find(|message| message.get("source_guid").and_then(Json::as_str) == Some(guid))
            .and_then(|message| api::id_of(message, "id"))
    }

    /// A heart as a like (`add`), or its removal as an unlike.
    fn like(
        &self,
        bridge: &BridgeInfo,
        config: &Config,
        room: &Room,
        request: &DeliveryRequest,
        add: bool,
    ) -> DeliveryOutcome {
        let key = request.reaction_key.as_deref().unwrap_or_default();
        if !HEARTS.contains(&key) {
            return failed("reactionNotCarried", "GroupMe carries only the heart, as a like");
        }
        let target = request.target_remote_id.as_deref().unwrap_or_default();
        let (conversation, message) = if add {
            let conversation = match room {
                Room::Group(group) => group.clone(),
                Room::Direct(other) => match request.external_thread_id.as_deref() {
                    Some(chat) => chat.to_string(),
                    None => match self.inner.own_id(&bridge.chat_bridge_id, config) {
                        Ok(own) => chat_id(&own, other),
                        Err(error) => return error.outcome("GroupMe's account could not be read"),
                    },
                },
            };
            (conversation, target.to_string())
        } else {
            // the like this server sent: like:<conversation>:<message>
            let mut parts = target.splitn(3, ':');
            match (parts.next(), parts.next(), parts.next()) {
                (Some("like"), Some(conversation), Some(message)) => (conversation.to_string(), message.to_string()),
                _ => {
                    return failed(
                        "bridgeUnsupported",
                        "Only a like sent from here is taken back on GroupMe",
                    )
                }
            }
        };
        if message.is_empty() || message.contains(['/', '?', '#']) || conversation.contains(['/', '?', '#']) {
            return failed("badTarget", "Not a GroupMe message");
        }
        let verb = if add { "like" } else { "unlike" };
        match self
            .inner
            .api(config)
            .post(&format!("/messages/{conversation}/{message}/{verb}"), &json!({}))
        {
            Ok(_) => DeliveryOutcome::Accepted {
                remote_event_id: format!("{verb}:{conversation}:{message}"),
            },
            // a like twice changes nothing, so a lost answer is sent again
            Err(ApiError::Lost(error)) => DeliveryOutcome::Retry {
                error,
                not_connected: false,
            },
            Err(error) => error.outcome("GroupMe did not take the like"),
        }
    }
}

/// The parameters of a framework service call from JSON.
pub(crate) fn parameters(value: &Json) -> BTreeMap<String, Value> {
    match moqui_lib::json::from_json(value) {
        Value::Map(map) => map.into_iter().map(|(key, value)| (key.to_string(), value)).collect(),
        _ => BTreeMap::new(),
    }
}
