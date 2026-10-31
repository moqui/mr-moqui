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
    Ok(())
}

/// Where a media node's recorder spools its files, under the runtime directory (`SPOOL` in moqui-host
/// `src/rtc/recorder.rs`).
const RECORDING_SPOOL: &str = "tmp/call-recording";
/// Where the recordings' files are kept.
const RECORDING_ROOT: &str = "dbresource://moqui/call/recording/";

/// `store#RecordingFile`: a spooled segment file (`tempPath`, under the runtime's [`RECORDING_SPOOL`]) to
/// `contentLocation` (under [`RECORDING_ROOT`]); the spooled file is deleted once stored. Anything else is refused, so
/// the service reads no other local file and writes no other place. The file is streamed to storage.
fn store_recording_file(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    store_spooled(parameters, RECORDING_SPOOL, RECORDING_ROOT, "call recording", false)
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
/// (`<expiry>:<legId>`, base64 HMAC-SHA1). Nothing when there are no URLs or no secret.
fn make_ice_servers(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    use base64::Engine;
    use hmac::{KeyInit, Mac};
    let leg = parameters
        .get("callLegId")
        .filter(|v| !is_empty(v))
        .map(to_plain_string)
        .unwrap_or_default();
    let mut urls: Vec<String> = match parameters.get("turnUrls") {
        Some(Value::List(list)) => list.iter().map(to_plain_string).collect(),
        Some(v) if !is_empty(v) => to_plain_string(v).split(',').map(str::to_string).collect(),
        _ => Vec::new(),
    };
    urls.extend(
        moqui_lib::env::get_prop_or_env(EXTERNAL_PROPERTY)
            .split(',')
            .map(str::to_string),
    );
    let urls: Vec<String> = urls
        .into_iter()
        .map(|u| u.trim().to_string())
        .filter(|u| u.starts_with("turn:") || u.starts_with("turns:"))
        .collect();
    let secret = moqui_lib::env::get_prop_or_env(SECRET_PROPERTY);
    let mut out = BTreeMap::new();
    if leg.is_empty() || urls.is_empty() || secret.is_empty() || secret.ends_with("CHANGEME") {
        out.insert("iceServers".to_string(), Value::List(Vec::new()));
        return Ok(out);
    }
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
    let mut mac = hmac::Hmac::<sha1::Sha1>::new_from_slice(secret.as_bytes())
        .map_err(|e| LibError::new(format!("the TURN secret does not work: {e}")))?;
    mac.update(username.as_bytes());
    let credential = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    let server =
        moqui_lib::json::from_json(&serde_json::json!({"urls": urls, "username": username, "credential": credential}));
    out.insert("iceServers".to_string(), Value::List(vec![server]));
    Ok(out)
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
