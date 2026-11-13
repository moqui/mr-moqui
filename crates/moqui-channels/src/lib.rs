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

//! Business messaging channels (plans/archive/Comms-Framework.md §4.3 "Business channels"): network adapters of the chat bridge
//! layer ([`moqui_core::service::chat_bridge`]). C4-SMS adds the first, [`telnyx::TelnyxSms`], the `sms` adapter: SMS and
//! MMS on the Telnyx Messaging API v2 (docs/interfaces/SMS.md).
//!
//! - Out: a text a user writes in a number pair's room goes through the delivery worker to
//!   [`deliver`](moqui_core::service::chat_bridge::ChatBridgeAdapter::deliver), which asks
//!   `org.moqui.call.SmsServices.check#SmsSend` (grants, the destination's class, the person's opt-out, the day's limits)
//!   before it posts to `/v2/messages`, with its files as addresses the provider fetches
//!   ([`moqui_core::service::chat_bridge::media_url`]). Telnyx does not dedupe a send, so an answer lost in flight is
//!   `Uncertain`, never sent again on its own.
//! - In: the host's `/channels/{chatBridgeId}/webhook` hands the raw body to [`webhook`]: the Ed25519 signature over the
//!   timestamp and the body, a replay window and cache, then `SmsServices.receive#Sms` for a text (the pair room, then
//!   `ingest#Event`) and `ChatBridgeServices.note#DeliveryReceipt` for a delivery receipt.
//!
//! The crate is on moqui-core (plans/archive/Crate-Split.md), which reaches it only through the
//! [`moqui_core::extension`] entry below. moqui-host links it under its `channels` feature (Enterprise and Maximal);
//! without it no `sms` adapter is registered, so an SMS bridge stays not connected and its webhook answers 404.

pub mod telnyx;
pub mod webhook;

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_core::context::ExecutionContext;
use moqui_core::service::call::{sync_checked, CallOptions};
use moqui_core::service::chat_bridge::ChatBridgeAdapter;
use moqui_lib::object::Value;

moqui_core::extension::submit! {
    moqui_core::extension::Extension {
        features: &["channels"],
        chat_bridge_adapters: adapters,
        ..moqui_core::extension::Extension::EMPTY
    }
}

fn adapters() -> Vec<Arc<dyn ChatBridgeAdapter>> {
    vec![Arc::new(telnyx::TelnyxSms::new())]
}

/// Call a framework service with authorization off (the adapters' internal calls), its recorded errors an error here and
/// taken back out of the message facade.
pub(crate) fn call(name: &str, parameters: BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>, String> {
    let options = CallOptions {
        disable_authz: true,
        ..CallOptions::default()
    };
    let mark = ExecutionContext::with_current(|ec| ec.message().error_mark());
    let result = sync_checked(name, &parameters, &options).map_err(|error| error.message().to_string());
    if let Some(mark) = mark {
        let _ = ExecutionContext::with_current(|ec| ec.message().take_errors_since(mark));
    }
    result
}

#[cfg(test)]
mod tests;
