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
//! end on their own when its heartbeat stops (`expire#Calls`).

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
    Ok(())
}

fn send_media_control(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some(node) = parameters
        .get("mediaNodeId")
        .filter(|v| !is_empty(v))
        .map(to_plain_string)
    else {
        return Ok(BTreeMap::new());
    };
    let mut body = match parameters.get("message").map(moqui_lib::json::to_json) {
        Some(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    body.insert("kind".to_string(), serde_json::Value::String("control".to_string()));
    body.insert("target".to_string(), serde_json::Value::String(node.clone()));
    let message = TopicMessage {
        origin_id: crate::notify::origin_id().to_string(),
        message_id: crate::entity::uuid_v4()?,
        body: serde_json::Value::Object(body),
    };
    let target = facade.clone();
    let send = move || {
        if let Err(err) = crate::cluster::send_to(&target, &node, RTC_SIGNAL, &message) {
            tracing::warn!("call media control for node {node} not sent: {err}");
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
