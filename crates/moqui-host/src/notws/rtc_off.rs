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

//! Call signaling when this build has no `rtc` feature: a call frame is answered with `notBuilt`.

use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;
use serde_json::{json, Value as Json};

use super::{Push, Slot};

pub(super) fn handle(_factory: &Arc<ExecutionContextFactory>, slot: &Arc<Slot>, text: &str) -> bool {
    if !text.trim_start().starts_with('{') || !text.contains("\"rtc\"") {
        return false;
    }
    let Ok(frame) = serde_json::from_str::<Json>(text) else {
        return false;
    };
    if frame.get("rtc").and_then(Json::as_str).is_none() {
        return false;
    }
    let call = frame.get("callSessionId").and_then(Json::as_str).unwrap_or_default();
    let reply = json!({"topic": format!("rtc:{call}"), "rtc": "error", "callSessionId": call,
        "callLegId": frame.get("callLegId"), "id": frame.get("id"), "code": "notBuilt",
        "message": "calls are not built into this server (the rtc feature)"});
    let _ = slot.tx.try_send(Push {
        text: reply.to_string(),
        message_id: String::new(),
    });
    true
}

pub(super) fn socket_closed(_factory: &Arc<ExecutionContextFactory>, _slot_id: u64) {}

pub(crate) fn install(_factory: &Arc<ExecutionContextFactory>, _shutdown: tokio::sync::watch::Receiver<bool>) {}

/// No calls here, so no reply of an edge node's media driver is for a socket of this server.
#[allow(dead_code, reason = "the edge link's receiver calls it when built with edge")]
pub(crate) fn deliver_edge_reply(_edge: &str, _message: &moqui_core::notify::TopicMessage) -> Result<(), String> {
    Err("calls are not built into this server (the rtc feature)".to_string())
}

/// No `/callws` without calls: a guest's page is told the link does not work.
pub(crate) fn guest_route(app: axum::Router<Arc<crate::http::HttpState>>) -> axum::Router<Arc<crate::http::HttpState>> {
    app
}
