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

//! `org.moqui.impl.CallServices.send#MediaControl` (plans/Comms-Framework.md §6.3): the call services tell the node
//! holding a call's media that a leg left or was removed, or that the call ended, so the node closes its peers. The
//! message goes on the `rtc-signal` cluster topic to that node only ([`crate::cluster::send_to`]), after the caller's
//! transaction commits; moqui-host's rtc module takes it there. A node that is gone is logged and skipped: its calls
//! end on their own when its heartbeat stops (`expire#Calls`). `org.moqui.impl.VoiceServices.send#SipControl` (§5.2)
//! goes the same way to a node's SIP engine (moqui-host's sip module), with kind `sip`.
//!
//! `org.moqui.impl.CallRecordingServices.store#RecordingFile` (W5) moves a recorder's spooled segment file into the
//! recordings' `dbresource://` root, and `org.moqui.impl.VoicemailServices.store#VoicemailFile` (V2) a message the SIP
//! engine recorded into the voicemail root.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};
use moqui_lib::sync::lock;

use crate::entity::EntityFacade;
use crate::notify::{TopicMessage, RTC_SIGNAL};

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, send_media_control)?;
    rust_service!(map, make_ice_servers)?;
    rust_service!(map, store_recording_file)?;
    rust_service!(map, store_voicemail_file)?;
    rust_service!(map, send_sip_control)?;
    rust_service!(map, get_caption_speaker)?;
    rust_service!(map, transcribe_caption_audio)?;
    Ok(())
}

/// Where a media node's recorder spools its files, under the runtime directory (`SPOOL` in moqui-host
/// `src/rtc/recorder.rs`).
const RECORDING_SPOOL: &str = "tmp/call-recording";
/// Where the recordings' files are kept.
const RECORDING_ROOT: &str = "dbresource://moqui/call/recording/";

/// `store#RecordingFile`: a spooled segment file (`tempPath`, under the runtime's [`RECORDING_SPOOL`]) to
/// `contentLocation` (under [`RECORDING_ROOT`]); the spooled file is deleted once stored. Or, on a local node, a
/// segment its edge node's recorder queued (`fromLocation`, where this node's queue keeps an item's content, under
/// [`crate::edge::queue::CONTENT_IN`]; plans/Edge-Node.md §5.4, E7), copied there; the queue removes its copy. Anything
/// else is refused, so the service reads no other local file or location and writes no other place. A spooled file is
/// streamed to storage.
fn store_recording_file(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let from = parameters
        .get("fromLocation")
        .filter(|v| !is_empty(v))
        .map(to_plain_string);
    match from {
        Some(from) => store_received(parameters, &from, RECORDING_ROOT, "call recording"),
        None => store_spooled(parameters, RECORDING_SPOOL, RECORDING_ROOT, "call recording", false),
    }
}

/// Whether `location` is under `root` with no empty, `.` or `..` part after it.
fn under(location: &str, root: &str) -> bool {
    location.strip_prefix(root).is_some_and(|rest| {
        !rest.is_empty()
            && !rest
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
    })
}

/// A queued item's content this node received (`from`, under the queue's [`crate::edge::queue::CONTENT_IN`]) copied
/// to `contentLocation` under `root`. The received copy stays; the queue removes it once the delivery is answered.
fn store_received(
    parameters: &BTreeMap<String, Value>,
    from: &str,
    root: &str,
    what: &str,
) -> Result<BTreeMap<String, Value>, LibError> {
    let location = parameters
        .get("contentLocation")
        .filter(|v| !is_empty(v))
        .map(to_plain_string)
        .unwrap_or_default();
    if !under(&location, root) {
        return Err(LibError::new(format!("{location} is not a {what} location")));
    }
    if !under(from, crate::edge::queue::CONTENT_IN) {
        return Err(LibError::new(format!(
            "{from} is not the content of an item the edge link received"
        )));
    }
    let factory = crate::context::ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory()))
        .ok_or_else(|| LibError::new(format!("storing a {what} file needs an execution context")))?;
    let bytes = factory
        .resource()
        .location_bytes(from)
        .map_err(|e| LibError::new(format!("the received {what} {from}: {e}")))?
        .ok_or_else(|| LibError::new(format!("the received {what} {from} is not there")))?;
    let size = bytes.len() as u64;
    factory
        .resource()
        .write_from(&location, &mut std::io::Cursor::new(bytes), Some(size))
        .map_err(|e| LibError::new(format!("the {what} file {location} was not stored: {}", e.message())))?;
    let mut out = BTreeMap::new();
    out.insert(
        "byteSize".to_string(),
        Value::I64(i64::try_from(size).unwrap_or(i64::MAX)),
    );
    Ok(out)
}

/// Where the SIP engine spools a voicemail it records (`SPOOL` in moqui-host `src/sip/voicemail.rs`).
const VOICEMAIL_SPOOL: &str = "tmp/voicemail";
/// Where voicemail audio is kept.
const VOICEMAIL_ROOT: &str = "dbresource://moqui/call/voicemail/";

/// `store#VoicemailFile`: a message the SIP engine spooled (`tempPath`, under [`VOICEMAIL_SPOOL`]) to `contentLocation`
/// (under [`VOICEMAIL_ROOT`]), as `store#RecordingFile`; an empty `contentLocation` only deletes the spooled file.
fn store_voicemail_file(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    store_spooled(parameters, VOICEMAIL_SPOOL, VOICEMAIL_ROOT, "voicemail", true)
}

/// A spooled file under `spool` (in the runtime directory) to a location under `root`, then the spooled file deleted;
/// with `discard`, an empty location only deletes it.
fn store_spooled(
    parameters: &BTreeMap<String, Value>,
    spool: &str,
    root: &str,
    what: &str,
    discard: bool,
) -> Result<BTreeMap<String, Value>, LibError> {
    let text = |key: &str| {
        parameters
            .get(key)
            .filter(|v| !is_empty(v))
            .map(to_plain_string)
            .unwrap_or_default()
    };
    let (temp_path, location) = (text("tempPath"), text("contentLocation"));
    let rest = location.strip_prefix(root).unwrap_or_default();
    let only_discard = discard && location.is_empty();
    if !only_discard
        && (rest.is_empty()
            || rest
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == ".."))
    {
        return Err(LibError::new(format!("{location} is not a {what} location")));
    }
    let factory = crate::context::ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory()))
        .ok_or_else(|| LibError::new(format!("storing a {what} file needs an execution context")))?;
    let spool = factory.runtime_dir().join(spool);
    let path = std::path::Path::new(&temp_path);
    let inside = match (path.canonicalize(), spool.canonicalize()) {
        (Ok(file), Ok(root)) => file.starts_with(&root) && file.is_file(),
        _ => false,
    };
    if !inside {
        return Err(LibError::new(format!("{temp_path} is not a {what} spool file")));
    }
    let mut size = 0;
    if !only_discard {
        // streamed: a segment is up to call_recording_segment_mb, never read into memory whole
        let mut file =
            std::fs::File::open(path).map_err(|e| LibError::new(format!("the spooled {what} {temp_path}: {e}")))?;
        size = file
            .metadata()
            .map_err(|e| LibError::new(format!("the spooled {what} {temp_path}: {e}")))?
            .len();
        factory
            .resource()
            .write_from(&location, &mut file, Some(size))
            .map_err(|e| LibError::new(format!("the {what} file {location} was not stored: {}", e.message())))?;
    }
    if let Err(err) = std::fs::remove_file(path) {
        tracing::warn!("the spooled {what} {temp_path} was not removed: {err}");
    }
    let mut out = BTreeMap::new();
    out.insert(
        "byteSize".to_string(),
        Value::I64(i64::try_from(size).unwrap_or(i64::MAX)),
    );
    Ok(out)
}

/// The secret TURN credentials are signed with, shared by every media node and any coturn (`turn_auth_secret`).
const SECRET_PROPERTY: &str = "turn_auth_secret";
/// How long a leg's TURN credential lasts, in hours.
const HOURS_PROPERTY: &str = "turn_credential_hours";
/// TURN servers outside this one (coturn with use-auth-secret and the same secret), comma separated `turn:` URLs.
const EXTERNAL_PROPERTY: &str = "turn_external_urls";

/// `make#IceServers`: the ICE servers of a leg for its RTCPeerConnection: the TURN URLs of its call's media node
/// (`turnUrls`, which the node registered) and any external ones, with a credential for this leg in the TURN REST form
/// (`<expiry>:<legId>`, base64 HMAC-SHA1). Nothing when there are no URLs or no secret. A media node that is this local
/// node's edge node (`mediaNodeId`, plans/Edge-Node.md §5.4, E7) checks its own secret, which its link told this node
/// ([`crate::edge::media::turn_secret`]): its URLs get a credential signed with that, and the external ones another
/// signed with this node's.
fn make_ice_servers(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let text = |key: &str| {
        parameters
            .get(key)
            .filter(|v| !is_empty(v))
            .map(to_plain_string)
            .unwrap_or_default()
    };
    let leg = text("callLegId");
    let node_urls: Vec<String> = match parameters.get("turnUrls") {
        Some(Value::List(list)) => list.iter().map(to_plain_string).collect(),
        Some(v) if !is_empty(v) => to_plain_string(v).split(',').map(str::to_string).collect(),
        _ => Vec::new(),
    };
    let external: Vec<String> = moqui_lib::env::get_prop_or_env(EXTERNAL_PROPERTY)
        .split(',')
        .map(str::to_string)
        .collect();
    let own = moqui_lib::env::get_prop_or_env(SECRET_PROPERTY);
    let edge_secret = crate::context::ExecutionContext::with_current(|ec| {
        crate::edge::media::turn_secret(ec.factory(), &text("mediaNodeId"))
    })
    .flatten();
    let hours: u64 = moqui_lib::env::get_prop_or_env(HOURS_PROPERTY)
        .trim()
        .parse()
        .unwrap_or(4)
        .clamp(1, 24);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let username = format!("{}:{leg}", now + hours * 3600);
    let mut servers = Vec::new();
    if !leg.is_empty() {
        let groups = match edge_secret {
            Some(edge) => vec![(node_urls, edge), (external, own)],
            None => vec![(node_urls.into_iter().chain(external).collect(), own)],
        };
        for (urls, secret) in groups {
            if let Some(server) = ice_server(&urls, &username, &secret)? {
                servers.push(server);
            }
        }
    }
    let mut out = BTreeMap::new();
    out.insert("iceServers".to_string(), Value::List(servers));
    Ok(out)
}

/// One ICE server: the `turn:` and `turns:` URLs of `urls`, with `username` and its credential under `secret`; none
/// without URLs or a secret.
fn ice_server(urls: &[String], username: &str, secret: &str) -> Result<Option<Value>, LibError> {
    use base64::Engine;
    use hmac::{KeyInit, Mac};
    let urls: Vec<String> = urls
        .iter()
        .map(|u| u.trim().to_string())
        .filter(|u| u.starts_with("turn:") || u.starts_with("turns:"))
        .collect();
    if urls.is_empty() || secret.is_empty() || secret.ends_with("CHANGEME") {
        return Ok(None);
    }
    let mut mac = hmac::Hmac::<sha1::Sha1>::new_from_slice(secret.as_bytes())
        .map_err(|e| LibError::new(format!("the TURN secret does not work: {e}")))?;
    mac.update(username.as_bytes());
    let credential = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    Ok(Some(moqui_lib::json::from_json(
        &serde_json::json!({"urls": urls, "username": username, "credential": credential}),
    )))
}

fn send_media_control(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    send_to_node(parameters, facade, "mediaNodeId", "control")
}

/// `org.moqui.impl.VoiceServices.send#SipControl` (plans/Comms-Framework.md §5.2): the voice services tell the SIP
/// engine of a node (`instanceId`) to dial, answer, hang up, hold, send digits or reload its trunks, on the same
/// `rtc-signal` topic with kind `sip`, after the caller's commit; moqui-host's sip module takes it there.
fn send_sip_control(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    send_to_node(parameters, facade, "instanceId", "sip")
}

/// Sends `message` to the node named by `node_parameter`, with `kind`, after the caller's transaction commits.
fn send_to_node(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
    node_parameter: &str,
    kind: &str,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some(node) = parameters
        .get(node_parameter)
        .filter(|v| !is_empty(v))
        .map(to_plain_string)
    else {
        return Ok(BTreeMap::new());
    };
    let mut body = match parameters.get("message").map(moqui_lib::json::to_json) {
        Some(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    body.insert("kind".to_string(), serde_json::Value::String(kind.to_string()));
    body.insert("target".to_string(), serde_json::Value::String(node.clone()));
    let message = TopicMessage {
        origin_id: crate::notify::origin_id().to_string(),
        message_id: crate::entity::uuid_v4()?,
        body: serde_json::Value::Object(body),
    };
    let target = facade.clone();
    let kind = kind.to_string();
    let send = move || {
        if let Err(err) = crate::cluster::send_to(&target, &node, RTC_SIGNAL, &message) {
            tracing::warn!("call {kind} message for node {node} not sent: {err}");
        }
    };
    match facade.default_transactions() {
        Ok(tx) if tx.in_transaction() => {
            let pending = Arc::new(Mutex::new(Some(send)));
            let held = Arc::clone(&pending);
            let registered = tx.on_commit(move || {
                if let Some(send) = lock(&held).take() {
                    send();
                }
            });
            if let Err(err) = registered {
                tracing::error!("Could not wait for the commit to send a call media control, sending now: {err}");
                if let Some(send) = lock(&pending).take() {
                    send();
                }
            }
        }
        _ => send(),
    }
    Ok(BTreeMap::new())
}

/// `org.moqui.impl.CallCaptionServices.get#CaptionSpeaker` (W6): the participant of a leg (`callLegId`) and the name the
/// call shows for them: the participant's `displayName`, else its user's full name or username. Empty for a leg not
/// found. The media node's captions call it, so on an edge node it runs on the local node (plans/Edge-Node.md §5.4).
fn get_caption_speaker(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let leg = parameters
        .get("callLegId")
        .filter(|v| !is_empty(v))
        .map(to_plain_string)
        .unwrap_or_default();
    let text = |row: &crate::entity::EntityValue, name: &str| {
        row.get(name).map(to_plain_string).filter(|value| !value.is_empty())
    };
    let mut out = BTreeMap::new();
    let Some(leg_row) = facade
        .find("moqui.call.CallLeg")?
        .eq("callLegId", Value::string(&leg))
        .disable_authz()
        .one()?
    else {
        return Ok(out);
    };
    let call = text(&leg_row, "callSessionId").unwrap_or_default();
    if !edge_caller_allowed(facade, &call, text(&leg_row, "mediaNodeId").as_deref())? {
        return Err(LibError::new(
            "the edge node names speakers only of calls whose media is on it",
        ));
    }
    let Some(participant_id) = text(&leg_row, "callParticipantId") else {
        return Ok(out);
    };
    let participant = facade
        .find("moqui.call.CallParticipant")?
        .eq("callParticipantId", Value::string(&participant_id))
        .disable_authz()
        .one()?;
    let mut name = participant.as_ref().and_then(|row| text(row, "displayName"));
    if name.is_none() {
        if let Some(user_id) = participant.as_ref().and_then(|row| text(row, "userId")) {
            let account = facade
                .find("moqui.security.UserAccount")?
                .eq("userId", Value::string(&user_id))
                .disable_authz()
                .one()?;
            name = account
                .as_ref()
                .and_then(|row| text(row, "userFullName").or_else(|| text(row, "username")));
        }
    }
    out.insert("callParticipantId".to_string(), Value::string(participant_id));
    out.insert("name".to_string(), name.map_or(Value::Null, Value::String));
    Ok(out)
}

/// Whether the current user may act for the call `call`'s media (a leg's own media node `leg_node` when it has one):
/// anyone but the edge link's delegate user (`_edge_`, plans/Edge-Node.md §5.4), which may only for a call whose
/// media is on this node's edge node (its `CallMediaNode` row is `edgeLinked`).
fn edge_caller_allowed(facade: &EntityFacade, call: &str, leg_node: Option<&str>) -> Result<bool, LibError> {
    let user = crate::user::current_username().ok().flatten();
    if user.as_deref() != Some(crate::edge::delegate::EDGE_USERNAME) {
        return Ok(true);
    }
    let node = match leg_node.filter(|node| !node.is_empty()) {
        Some(node) => node.to_string(),
        None => facade
            .find("moqui.call.CallSession")?
            .eq("callSessionId", Value::string(call))
            .disable_authz()
            .one()?
            .and_then(|row| row.get("mediaNodeId").map(to_plain_string))
            .unwrap_or_default(),
    };
    if node.is_empty() {
        return Ok(false);
    }
    let linked = facade
        .find("moqui.call.CallMediaNode")?
        .eq("instanceId", Value::string(&node))
        .disable_authz()
        .one()?
        .and_then(|row| row.get("edgeLinked").map(to_plain_string));
    Ok(linked.as_deref() == Some("Y"))
}

/// The most audio one caption chunk may carry: 30 seconds of 16 kHz mono WAV is under 1 MiB; this leaves room.
const MAX_CAPTION_AUDIO: usize = 4 * 1024 * 1024;

/// `org.moqui.impl.CallCaptionServices.transcribe#CaptionAudio` (W6, plans/Edge-Node.md §5.4 E7): an edge node's chunk
/// of a call's audio (`audioBase64`, WAV) through [`crate::llm_hooks::transcribe`] on this node, counted against
/// `userId` (who must be a participant of `callSessionId`) and kept as an `LlmCall` with the client
/// `captions:<callSessionId>`. A refusal is `errorCode` and `errorMessage`, not a service error, so the media node keeps
/// its code.
fn transcribe_caption_audio(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    use base64::Engine as _;
    let text = |key: &str| {
        parameters
            .get(key)
            .filter(|v| !is_empty(v))
            .map(to_plain_string)
            .unwrap_or_default()
    };
    let refused = |code: &str, message: &str| {
        BTreeMap::from([
            ("errorCode".to_string(), Value::string(code)),
            ("errorMessage".to_string(), Value::string(message)),
        ])
    };
    let call = text("callSessionId");
    let user = text("userId");
    let participant = facade
        .find("moqui.call.CallParticipant")?
        .eq("callSessionId", Value::string(&call))
        .eq("userId", Value::string(&user))
        .disable_authz()
        .count()?;
    if user.is_empty() || participant == 0 {
        return Ok(refused("notAllowed", "the user is not a participant of the call"));
    }
    if !edge_caller_allowed(facade, &call, None)? {
        return Ok(refused(
            "notAllowed",
            "the call's media is not on this node's edge node",
        ));
    }
    let encoded = text("audioBase64");
    if encoded.len() > MAX_CAPTION_AUDIO * 4 / 3 + 4 {
        return Ok(refused("invalid", "the audio chunk is too large"));
    }
    let Ok(audio) = base64::engine::general_purpose::STANDARD.decode(encoded.trim()) else {
        return Ok(refused("invalid", "the audio is not base64"));
    };
    let factory = crate::context::ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory()))
        .ok_or_else(|| LibError::new("transcribing needs an execution context"))?;
    let language = text("language");
    let seconds: u64 = text("timeoutSeconds").parse().unwrap_or(20).clamp(1, 300);
    let purpose = format!("captions:{call}");
    let request = crate::llm_hooks::SpeechRequest {
        audio: &audio,
        media_type: "audio/wav",
        language: (!language.is_empty()).then_some(language.as_str()),
        prompt: None,
        user_id: (!user.is_empty()).then_some(user.as_str()),
        purpose: &purpose,
        timeout: Some(std::time::Duration::from_secs(seconds)),
    };
    match crate::llm_hooks::transcribe(&factory, &request) {
        Ok(text) => Ok(BTreeMap::from([("text".to_string(), Value::string(text))])),
        Err(err) => Ok(refused(err.code, &err.message)),
    }
}

#[cfg(test)]
#[path = "call_media_tests.rs"]
mod tests;
