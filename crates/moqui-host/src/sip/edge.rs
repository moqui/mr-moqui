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

//! The SIP engine on an edge node (plans/Edge-Node.md §5.4 "Voice (SIP)", decision 4, E8), which has none of the
//! business data: its routing, registrar, IVR and voicemail services are delegated to the local node (the `<edge>`
//! conf), and what it needs while the local node is away is here.
//!
//! - **Trunks.** `get#EngineConfig` is answered from the replicated set `voiceTrunks` ([`engine_config`]), so the trunks
//!   register and take calls whatever the link does; the engine loads it again when a batch of the set is applied.
//! - **The fallback.** A call whose routing finds the local node away (`moqui_core::edge::OFFLINE`) is answered from the
//!   number's copy in `voiceFallbacks` ([`fallback_of`]): ring the outside number first, then take a message, or 480.
//! - **Messages** the engine records, routed or not, are queued to the local node with their audio ([`queue_voicemail`]),
//!   which finds the box and tells its owner.
//! - **Audio.** A greeting or prompt the engine does not have is fetched from the local node and kept (their names are
//!   stamped, never written again), and a saved message played by phone is fetched for the call that opened its box
//!   ([`fetch_audio`]); the boxes' greetings also come ahead on the queue (`callAudio`), for the fallback.
//!
//! On an all-in-one node (plans/Edge-Node.md §10, E12) all of this is for the paired node's items only (`owners.rs`),
//! and what it keeps of the paired node's audio is under a location of that node's
//! (`moqui_core::edge::media::kept_audio_location`), apart from the server's own greetings at the same paths.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::edge::replica;
use moqui_lib::object::{to_plain_string, Value};
use serde_json::Value as Json;

/// The ids of a call the engine answers from the fallback, which the local node never heard of: its room, and its legs
/// (`-in`, `-out`), whose events go to no service.
pub(crate) const FALLBACK_PREFIX: &str = "edge-fallback-";
/// The replicated set of the trunks and the policy (MoquiDefaultConf.xml `<edge>`).
pub(crate) const TRUNKS_SET: &str = "voiceTrunks";
/// The replicated set of each number's fallback.
const FALLBACKS_SET: &str = "voiceFallbacks";
const FALLBACK_ENTITY: &str = "moqui.call.PhoneNumberFallback";
/// The queue kind of a message the engine recorded.
const VOICEMAIL_KIND: &str = "voicemail";
const CALL_AUDIO_SERVICE: &str = "org.moqui.call.VoiceServices.get#CallAudio";
const MESSAGE_AUDIO_SERVICE: &str = "org.moqui.call.VoicemailServices.get#PhoneVoicemailAudio";
/// Where saved messages are kept on the local node (`VOICEMAIL_ROOT` in moqui-core `service/call_media.rs`).
const VOICEMAIL_ROOT: &str = "dbresource://moqui/call/voicemail/";
/// The most trunks read from the copy.
const MAX_TRUNKS: usize = 1000;

/// Whether a service error says the local node is away: no link, or no answer in time.
pub(crate) fn is_offline(error: &str) -> bool {
    error.contains(moqui_core::edge::OFFLINE)
}

/// Whether a leg or room (the engine's key of it, E12) is one the fallback made, which the local node does not know.
pub(crate) fn is_fallback(id: &str) -> bool {
    super::owners::bare(id).starts_with(FALLBACK_PREFIX)
}

/// The only active paired local node, whose copies this node answers from; none while none is paired.
fn paired_node(facade: &moqui_core::entity::EntityFacade) -> Result<Option<String>, moqui_core::context::Error> {
    let rows = facade
        .find("moqui.edge.EdgeClient")?
        .eq("statusId", Value::string("EdgeClientActive"))
        .select_field("pairedNodeId")
        .disable_authz()
        .limit(2)
        .list()?;
    Ok(match rows.values() {
        [only] => only.get("pairedNodeId").map(to_plain_string),
        _ => None,
    })
}

/// Runs `job` on a request worker with authorization off, as the engine's own work.
async fn on_pool<T: Send + 'static>(
    factory: &Arc<ExecutionContextFactory>,
    job: impl FnOnce(&ExecutionContextFactory) -> Result<T, moqui_core::context::Error> + Send + 'static,
) -> Result<T, String> {
    crate::worker::on_pool(&Arc::downgrade(factory), move |factory| {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        job(factory)
    })
    .await
    .map_err(|err| err.message().to_string())
}

/// `get#EngineConfig`'s answer for the paired node, from its copy of `voiceTrunks`, with the node: none while no node
/// is paired.
pub(crate) async fn engine_config(factory: &Arc<ExecutionContextFactory>) -> Result<Option<(String, Json)>, String> {
    on_pool(factory, |factory| {
        let facade = factory
            .entity()
            .ok_or_else(|| moqui_core::context::Error::new("no entity facade"))?;
        let Some(node) = paired_node(&facade)? else {
            return Ok(None);
        };
        let values = replica::list_values(&facade, Some(&node), TRUNKS_SET, None, MAX_TRUNKS)?;
        Ok(Some((node, config_of(&values))))
    })
    .await
}

fn field_text(fields: &Json, name: &str) -> String {
    match fields.get(name) {
        Some(Json::String(text)) => text.trim().to_string(),
        Some(Json::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn comma_list(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

/// The engine's configuration from the copies, as `Voice.js` `engineConfig` answers on a local node: the enabled
/// trunks with their credentials (a trunk whose credentials do not read is logged and left out), and the policy, with
/// its defaults for what its copy leaves empty. The blocked sources are this engine's own.
pub(crate) fn config_of(values: &[replica::ReplicaValue]) -> Json {
    let mut trunks = Vec::new();
    let mut policy = serde_json::Map::new();
    for value in values {
        let fields = &value.fields;
        match value.entity_name.as_str() {
            "moqui.call.VoiceTrunk" => {
                let id = field_text(&value.pk, "voiceTrunkId");
                if id.is_empty() || field_text(fields, "disabled") == "Y" {
                    continue;
                }
                let auth_text = field_text(fields, "authJson");
                let auth: Json = if auth_text.is_empty() {
                    Json::Object(serde_json::Map::new())
                } else {
                    match serde_json::from_str(&auth_text) {
                        Ok(auth @ Json::Object(_)) => auth,
                        _ => {
                            tracing::error!("SIP trunk {id} is left out on the edge node: its credentials do not read");
                            continue;
                        }
                    }
                };
                let algorithms: Vec<String> = match auth.get("algorithms") {
                    Some(Json::Array(list)) => list.iter().filter_map(|a| a.as_str().map(str::to_string)).collect(),
                    Some(Json::String(text)) => comma_list(text),
                    _ => Vec::new(),
                };
                let algorithms: Vec<String> = algorithms
                    .into_iter()
                    .filter(|a| ["SHA-256", "SHA-512-256", "MD5"].contains(&a.as_str()))
                    .collect();
                let transport = match field_text(fields, "transportEnumId").as_str() {
                    "VtrTcp" => "tcp",
                    "VtrUdp" => "udp",
                    _ => "tls",
                };
                let media = if field_text(fields, "mediaEncryptionEnumId") == "VmePlain" {
                    "plain"
                } else {
                    "srtp"
                };
                let port: Option<u64> = field_text(fields, "port").parse().ok();
                let auth_text = |name: &str| auth.get(name).and_then(Json::as_str).filter(|v| !v.is_empty());
                trunks.push(serde_json::json!({
                    "voiceTrunkId": id,
                    "name": Some(field_text(fields, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone()),
                    "host": field_text(fields, "host"),
                    "port": port,
                    "transport": transport,
                    "mediaEncryption": media,
                    "username": auth_text("username"),
                    "password": auth_text("password"),
                    "authUsername": auth_text("authUsername"),
                    "realm": auth_text("realm"),
                    "algorithms": if algorithms.is_empty() { vec!["SHA-256".to_string(), "MD5".to_string()] } else { algorithms },
                    "register": field_text(fields, "registerFlag") == "Y",
                    "inboundAddresses": comma_list(&field_text(fields, "inboundAddresses")),
                    "emergencyCapable": field_text(fields, "emergencyCapable") == "Y",
                    "fromDomain": Some(field_text(fields, "fromDomain")).filter(|d| !d.is_empty()),
                    // a PBX's profile (Voice.js PBX_PROFILES): its credentials may sign a call in
                    "inboundDigest": field_text(fields, "providerProfileEnumId") == "VppPlainLan",
                }));
            }
            "moqui.call.VoicePolicy" if field_text(&value.pk, "voicePolicyId") == "DEFAULT" => {
                for (name, value) in fields.as_object().into_iter().flatten() {
                    if !value.is_null() && !(value.is_string() && value.as_str().is_some_and(|v| v.trim().is_empty())) {
                        policy.insert(name.clone(), value.clone());
                    }
                }
            }
            _ => {}
        }
    }
    let yes = |name: &str| policy.get(name).and_then(Json::as_str) == Some("Y");
    let mut out_policy = serde_json::json!({
        "internationalEnabled": yes("internationalEnabled"),
        "premiumEnabled": yes("premiumEnabled"),
        "maxConcurrentCalls": policy.get("maxConcurrentCalls").cloned().unwrap_or(Json::from(20)),
    });
    for name in ["authFailureLimit", "authFailureWindowSeconds", "authBlockSeconds"] {
        if let Some(value) = policy.get(name) {
            out_policy[name] = value.clone();
        }
    }
    if let Some(code) = policy.get("homeCountryCode") {
        let digits: String = to_text(code).chars().filter(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            out_policy["homeCountryCode"] = Json::String(digits);
        }
    }
    for name in ["emergencyNumbers", "premiumPrefixes"] {
        if let Some(value) = policy.get(name) {
            out_policy[name] = Json::from(comma_list(&to_text(value)));
        }
    }
    serde_json::json!({"trunkList": trunks, "policy": out_policy, "blockedSourceList": []})
}

fn to_text(value: &Json) -> String {
    match value {
        Json::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// How many batches of `voiceTrunks` this edge node applied: the engine reloads when it moves.
pub(crate) async fn trunks_applied(factory: &Arc<ExecutionContextFactory>) -> Option<u64> {
    on_pool(factory, |factory| {
        Ok(factory
            .entity()
            .map(|facade| replica::applied_count(&facade, TRUNKS_SET)))
    })
    .await
    .ok()
    .flatten()
}

/// What the engine does with a call to a number while the local node is away.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Fallback {
    /// Take a message (after the forward, when there is one).
    pub voicemail: bool,
    pub greeting: Option<String>,
    /// Ring this outside number first, for `forward_seconds`.
    pub forward: Option<String>,
    pub forward_seconds: u64,
    /// The most forwards of the number at once, and in a day (the local policy's and the user's limits); 0 forwards
    /// none.
    pub forward_max_concurrent: u64,
    pub forward_max_per_day: u64,
    /// The trunk of the number, which a forward goes out through.
    pub trunk: Option<String>,
}

/// A number's fallback from its copy's fields; none when it has nothing to do.
pub(crate) fn fallback_from(fields: &Json) -> Option<Fallback> {
    let text = |name: &str| Some(field_text(fields, name)).filter(|v| !v.is_empty());
    let fallback = Fallback {
        voicemail: field_text(fields, "voicemailFlag") == "Y",
        greeting: text("greetingLocation"),
        forward: text("forwardNumber"),
        forward_seconds: field_text(fields, "forwardSeconds")
            .parse::<u64>()
            .unwrap_or(30)
            .clamp(10, 120),
        trunk: text("voiceTrunkId"),
        forward_max_concurrent: field_text(fields, "forwardMaxConcurrent").parse().unwrap_or(0),
        forward_max_per_day: field_text(fields, "forwardMaxPerDay").parse().unwrap_or(0),
    };
    (fallback.voicemail || fallback.forward.is_some()).then_some(fallback)
}

/// The fallback of the number `to` (E.164), from the copy of `voiceFallbacks`; none for a number with none, or when it
/// cannot be read (logged).
pub(crate) async fn fallback_of(factory: &Arc<ExecutionContextFactory>, to: &str) -> Option<Fallback> {
    let number = to.to_string();
    let read = on_pool(factory, move |factory| {
        let facade = factory
            .entity()
            .ok_or_else(|| moqui_core::context::Error::new("no entity facade"))?;
        let Some(node) = paired_node(&facade)? else {
            return Ok(None);
        };
        let pk = BTreeMap::from([("phoneNumber".to_string(), number)]);
        Ok(replica::get_value(&facade, Some(&node), FALLBACKS_SET, FALLBACK_ENTITY, &pk)?.map(|value| value.fields))
    })
    .await;
    match read {
        Ok(fields) => fields.as_ref().and_then(fallback_from),
        Err(err) => {
            tracing::error!("SIP: the fallback of {to} was not read, the call is refused: {err}");
            None
        }
    }
}

/// The file beside a spool file that keeps what its message is queued with, until it is queued.
fn sidecar(path: &Path) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".json");
    std::path::PathBuf::from(name)
}

/// The first wait before a message that did not queue is tried again, doubled each time up to [`QUEUE_RETRY_MAX`].
const QUEUE_RETRY_FIRST: std::time::Duration = std::time::Duration::from_secs(2);
const QUEUE_RETRY_MAX: std::time::Duration = std::time::Duration::from_secs(300);

/// Queues a message the engine recorded (`path`, its spool file) to the local node, with `payload` (the number called,
/// the caller, its length, when, and the call when the local node routed it), keyed by its spool file's name so a retry
/// is the same item. What it is queued with is kept beside it first (`<file>.json`), so a message that does not queue
/// now, or whose process stops before, is not lost: it is tried again with a backoff from 2 s to 5 minutes while the
/// engine runs, and [`requeue_spool`] queues it at the next start. The spool file and its sidecar go once it is queued.
pub(crate) async fn queue_voicemail(factory: &Arc<ExecutionContextFactory>, path: &Path, mut payload: Json) {
    // the local node's own ids (E12: the engine keys them by node)
    super::owners::bare_ids(&mut payload);
    if let Err(err) = std::fs::write(sidecar(path), payload.to_string()) {
        tracing::error!(
            "SIP voicemail: {}'s details were not kept beside it: {err}",
            path.display()
        );
    }
    let weak = Arc::downgrade(factory);
    let mut wait = QUEUE_RETRY_FIRST;
    loop {
        let Some(factory) = weak.upgrade() else { return };
        match queue_once(&factory, path, &payload).await {
            Ok(()) => return,
            Err(err) => tracing::error!(
                "SIP voicemail: the message {} was not queued for the local node, tried again in {wait:?}: {err}",
                path.display()
            ),
        }
        drop(factory);
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(QUEUE_RETRY_MAX);
    }
}

/// At the engine's start on an edge node: the messages left in the spool with their details (they did not queue before
/// the process stopped) are queued now, each on its own.
pub(crate) fn requeue_spool(factory: &Arc<ExecutionContextFactory>) {
    let spool = factory.runtime_dir().join(super::voicemail::SPOOL);
    let Ok(entries) = std::fs::read_dir(&spool) else { return };
    for entry in entries.flatten() {
        let side = entry.path();
        if side.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let file = side.with_extension("");
        let payload: Option<Json> = std::fs::read_to_string(&side)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok());
        match payload {
            Some(payload) if file.is_file() => {
                tracing::info!("SIP voicemail: {} was left in the spool, queued now", file.display());
                let factory = Arc::clone(factory);
                tokio::spawn(async move { queue_voicemail(&factory, &file, payload).await });
            }
            _ => tracing::error!(
                "SIP voicemail: {} has no message or its details do not read, left",
                side.display()
            ),
        }
    }
}

/// One try of [`queue_voicemail`]: queued, and the spool file and its sidecar removed.
async fn queue_once(factory: &Arc<ExecutionContextFactory>, path: &Path, payload: &Json) -> Result<(), String> {
    let payload = payload.clone();
    let file = path.to_path_buf();
    let shown = path.display().to_string();
    let queued = on_pool(factory, move |factory| {
        let facade = factory
            .entity()
            .ok_or_else(|| moqui_core::context::Error::new("no entity facade"))?;
        let content = std::fs::read(&file)
            .map_err(|err| moqui_core::context::Error::new(format!("its spool file did not read: {err}")))?;
        let key = file
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let to = payload.get("toNumber").and_then(Json::as_str).unwrap_or_default();
        let item = moqui_core::edge::queue::NewItem {
            kind: VOICEMAIL_KIND.to_string(),
            conversation_key: format!("voicemail:{to}"),
            idempotency_key: format!("voicemail:{key}"),
            payload: Some(payload.to_string()),
            content: Some(content),
            content_location: None,
        };
        let queued = moqui_core::edge::queue::enqueue(factory, &facade, None, &item)?;
        Ok(queued.queue_item_id)
    })
    .await;
    let item = queued?;
    tracing::info!("SIP voicemail: the message is queued for the local node (item {item})");
    for file in [path.to_path_buf(), sidecar(path)] {
        if let Err(err) = std::fs::remove_file(&file) {
            tracing::debug!("SIP voicemail: {shown}: {} not removed: {err}", file.display());
        }
    }
    Ok(())
}

/// Runs a delegated service in the current context, its errors taken out of it: its answer, or what went wrong.
fn delegated(service: &str, parameters: &BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>, String> {
    ExecutionContext::with_current(|ec| ec.message().push_errors());
    let called = moqui_core::service::sync(service, parameters);
    let errors = ExecutionContext::with_current(|ec| {
        let mut messages = ec.message();
        let errors = messages.errors().join("; ");
        messages.clear_errors();
        messages.pop_errors();
        errors
    })
    .unwrap_or_default();
    match called {
        Err(err) => Err(err.message().to_string()),
        Ok(_) if !errors.is_empty() => Err(errors),
        Ok(out) => Ok(out),
    }
}

/// The audio in an answer's `audioBase64`.
fn decoded(out: &BTreeMap<String, Value>, location: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    let text = out.get("audioBase64").map(to_plain_string).unwrap_or_default();
    if text.is_empty() {
        return Err(format!("{location}: the local node has none"));
    }
    base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|err| format!("{location}: the local node's answer is not base64: {err}"))
}

/// On a request worker, for a call of the paired node `node`, the audio at `location` that this node does not keep
/// (`moqui_core::edge::media::kept_audio_location`): a greeting or prompt from the local node, kept for the next time; a saved message for the call
/// `call` that opened its box (never kept). An error says why not, or that the local node is away. Run as the paired
/// node's (E12), so the services go down its link.
pub(crate) fn fetch_audio(
    factory: &ExecutionContextFactory,
    node: &str,
    location: &str,
    call: Option<&str>,
) -> Result<Vec<u8>, String> {
    moqui_core::edge::owner::with(moqui_core::edge::owner::Owner::paired(node), || {
        fetch_paired_audio(factory, node, location, call)
    })
}

fn fetch_paired_audio(
    factory: &ExecutionContextFactory,
    node: &str,
    location: &str,
    call: Option<&str>,
) -> Result<Vec<u8>, String> {
    if super::audio::greeting_location(location) {
        let parameters = BTreeMap::from([("location".to_string(), Value::string(location))]);
        let bytes = decoded(&delegated(CALL_AUDIO_SERVICE, &parameters)?, location)?;
        let size = bytes.len() as u64;
        let kept = moqui_core::edge::media::kept_audio_location(factory, node, location);
        if let Err(err) = factory
            .resource()
            .write_from(&kept, &mut std::io::Cursor::new(bytes.clone()), Some(size))
        {
            tracing::warn!("SIP: {location} from the local node was not kept: {}", err.message());
        }
        return Ok(bytes);
    }
    match call {
        Some(call) if location.starts_with(VOICEMAIL_ROOT) && !is_fallback(call) => {
            let parameters = BTreeMap::from([
                ("callSessionId".to_string(), Value::string(super::owners::bare(call))),
                ("audioLocation".to_string(), Value::string(location)),
            ]);
            decoded(&delegated(MESSAGE_AUDIO_SERVICE, &parameters)?, location)
        }
        _ => Err(format!("{location}: no file here")),
    }
}

#[cfg(test)]
#[path = "edge_tests.rs"]
mod tests;
