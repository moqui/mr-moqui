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

//! What an adapter does at its network beside the deliveries (plans/Comms-Messaging.md §18.3 phase 13):
//!
//! - `send#BridgeTyping`: a user of ours types in a room; each outbound bridge of the room whose network has a typing
//!   indicator shows it to the other side ([`super::ChatBridgeAdapter::typing`]: business RCS). At most once in
//!   [`TYPING_EVERY_MILLIS`] for a room on this node, checked before anything is read; nothing is kept or retried, and a
//!   bridge whose indicator fails is logged and the next goes on. `ChatServices.signal#Typing` starts it
//!   asynchronously, so the user's signal never waits on a network.
//! - `clean#ProviderContent` and [`sweep_contents`]: the contents an adapter made at its network for a send (a Twilio
//!   Content resource of a rich message, `ChatBridgeDelivery.providerContentId`) that are still there on a delivery made
//!   more than [`CONTENT_SWEEP_AGE_MILLIS`] ago (its message's final receipt deletes it; this is for one that never came,
//!   or a send that never got through). Each is deleted through its adapter
//!   ([`super::ChatBridgeAdapter::delete_content`]), best effort: one that fails is logged and kept for the next sweep.
//!   The delivery worker sweeps once a day.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use moqui_db::sql::Condition;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;

use super::{
    adapter_of, bridge_info, current_factory, force_new, now_millis, param, stamp, text_of, BRIDGE, DELIVERY, MAPPING,
};
use crate::context::ExecutionContextFactory;
use crate::entity::EntityFacade;

/// A room's typing indicator goes to its bridges at most this often (a network shows one for about 20 seconds).
pub const TYPING_EVERY_MILLIS: i64 = 15_000;
/// A provider's content left on a delivery older than this is deleted by the sweep.
pub const CONTENT_SWEEP_AGE_MILLIS: i64 = 2 * 86_400_000;
/// The deliveries one sweep reads at most; the rest wait for the next.
const SWEEP_MAX: i64 = 500;

pub(super) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, send_bridge_typing)?;
    rust_service!(map, clean_provider_content)?;
    Ok(())
}

/// When each room's indicator last went out on this node.
#[derive(Default)]
struct Typing {
    sent: Mutex<HashMap<String, i64>>,
}

/// `send#BridgeTyping` (chatRoomId, nowMillis: a test's clock): the room's typing indicator to its outbound bridges.
/// Out: `sent` (the bridges whose adapter was asked), `throttled` (true when the room's last was too recent).
fn send_bridge_typing(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let factory = current_factory().ok_or_else(|| LibError::new("no execution context"))?;
    let room_id = param(parameters, "chatRoomId").unwrap_or_default();
    let now = param(parameters, "nowMillis")
        .and_then(|text| text.parse::<i64>().ok())
        .unwrap_or_else(now_millis);
    let answer = |sent: i64, throttled: bool| {
        Ok(BTreeMap::from([
            ("sent".to_string(), Value::I64(sent)),
            ("throttled".to_string(), Value::Bool(throttled)),
        ]))
    };
    if room_id.is_empty() {
        return answer(0, false);
    }
    {
        let typing = factory.extension(|_| Typing::default());
        let mut sent = lock(&typing.sent);
        if sent
            .get(&room_id)
            .is_some_and(|at| (0..TYPING_EVERY_MILLIS).contains(&(now - at)))
        {
            return answer(0, true);
        }
        sent.retain(|_, at| (0..TYPING_EVERY_MILLIS).contains(&(now - *at)));
        sent.insert(room_id.clone(), now);
    }
    let mappings = facade
        .find(MAPPING)?
        .eq("chatRoomId", Value::string(&room_id))
        .condition(Condition::is_null("thruDate"))
        .disable_authz()
        .list()?;
    let mut count = 0;
    for mapping in mappings.values() {
        if text_of(mapping, "directionEnumId").as_deref() == Some("CrbIn") {
            continue;
        }
        let (Some(bridge_id), Some(external)) = (text_of(mapping, "chatBridgeId"), text_of(mapping, "externalRoomId"))
        else {
            continue;
        };
        let row = match facade
            .find(BRIDGE)?
            .eq("chatBridgeId", Value::string(&bridge_id))
            .disable_authz()
            .one()
        {
            Ok(Some(row)) => row,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!("chat bridge {bridge_id} was not read for a typing indicator: {error}");
                continue;
            }
        };
        if text_of(&row, "statusId").as_deref() == Some("CbsDisabled") {
            continue;
        }
        let info = bridge_info(&row);
        let Some(adapter) = adapter_of(&factory, facade, &info) else {
            continue;
        };
        count += 1;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| adapter.typing(&info, &external)));
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!("chat bridge {bridge_id}: the typing indicator was not sent: {error}"),
            Err(_) => tracing::error!("chat bridge {bridge_id}: the adapter failed on a typing indicator"),
        }
    }
    answer(count, false)
}

/// `clean#ProviderContent` (nowMillis: a test's clock): [`sweep_contents`] now. Out: `deleted`, `kept`.
fn clean_provider_content(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let factory = current_factory().ok_or_else(|| LibError::new("no execution context"))?;
    let now = param(parameters, "nowMillis")
        .and_then(|text| text.parse::<i64>().ok())
        .unwrap_or_else(now_millis);
    let (deleted, kept) = sweep_contents(&factory, facade, now);
    let as_value = |count: usize| Value::I64(i64::try_from(count).unwrap_or(i64::MAX));
    Ok(BTreeMap::from([
        ("deleted".to_string(), as_value(deleted)),
        ("kept".to_string(), as_value(kept)),
    ]))
}

/// Delete the provider's contents left on deliveries made before `now` less [`CONTENT_SWEEP_AGE_MILLIS`], each through
/// its bridge's adapter, and clear the field of each one deleted. Each is on its own: one that fails is logged and kept.
/// Returns (deleted, kept).
pub fn sweep_contents(factory: &ExecutionContextFactory, facade: &EntityFacade, now: i64) -> (usize, usize) {
    let rows = facade.find(DELIVERY).and_then(|find| {
        find.condition(Condition::is_not_null("providerContentId"))
            .condition(Condition::less("createdDate", stamp(now - CONTENT_SWEEP_AGE_MILLIS)))
            .limit(SWEEP_MAX)
            .disable_authz()
            .list()
    });
    let rows = match rows {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!("the chat bridges' provider contents were not read: {error}");
            return (0, 0);
        }
    };
    let mut bridges = HashMap::new();
    let (mut deleted, mut kept) = (0, 0);
    for row in rows.values() {
        let (Some(delivery_id), Some(bridge_id), Some(content_id)) = (
            text_of(row, "chatBridgeDeliveryId"),
            text_of(row, "chatBridgeId"),
            text_of(row, "providerContentId"),
        ) else {
            continue;
        };
        if !bridges.contains_key(&bridge_id) {
            let found = facade
                .find(BRIDGE)
                .and_then(|find| find.eq("chatBridgeId", Value::string(&bridge_id)).disable_authz().one())
                .ok()
                .flatten()
                .map(|row| bridge_info(&row))
                .and_then(|info| adapter_of(factory, facade, &info).map(|adapter| (info, adapter)));
            bridges.insert(bridge_id.clone(), found);
        }
        let Some(Some((info, adapter))) = bridges.get(&bridge_id) else {
            kept += 1;
            continue;
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            adapter.delete_content(info, &content_id)
        }));
        match result {
            Ok(Ok(())) => {
                let cleared = force_new(facade, DELIVERY, || {
                    facade
                        .find(DELIVERY)?
                        .eq("chatBridgeDeliveryId", Value::string(&delivery_id))
                        .eq("providerContentId", Value::string(&content_id))
                        .disable_authz()
                        .update_all([("providerContentId".to_string(), Value::Null)])
                });
                if let Err(error) = cleared {
                    tracing::warn!("delivery {delivery_id}: its deleted content {content_id} stays named: {error}");
                }
                deleted += 1;
            }
            Ok(Err(error)) => {
                tracing::warn!("chat bridge {bridge_id}: content {content_id} was not deleted: {error}");
                kept += 1;
            }
            Err(_) => {
                tracing::error!("chat bridge {bridge_id}: the adapter failed deleting content {content_id}");
                kept += 1;
            }
        }
    }
    (deleted, kept)
}
