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

//! The SMS providers: each module implements [`crate::sms::SmsProvider`] for one provider's messaging API and
//! webhooks, and [`crate::adapters`] registers one [`crate::sms::SmsAdapter`] for it (docs/interfaces/SMS.md).
//!
//! | Module | Adapter (`adapterName`) | Webhook signature, checked on | Replay | Retries a 5xx |
//! | --- | --- | --- | --- | --- |
//! | [`telnyx`] | `sms` | Ed25519 under the account's public key: the edge node, and the local node again | its timestamp's window, then the core's dedupes | yes |
//! | [`twilio`] | `sms-twilio` | HMAC-SHA1 under the Auth Token over the URL and the form: the local node | the replay cache, then the core's dedupes | no, without a fallback URL or connection overrides |
//! | [`sinch`] | `sms-sinch` | HMAC-SHA256 under the webhook's secret: the local node | its timestamp's window, then the core's dedupes | yes |
//! | [`twilio::rcs`] | `rcs-twilio` | as `sms-twilio` | as `sms-twilio` | as `sms-twilio` |
//! | [`sinch::rcs`] | `rcs-sinch` | as `sms-sinch` | as `sms-sinch` | yes |
//!
//! A provider whose signature has no timestamp (Twilio) relies on the replay cache and then the core's dedupes (texts by
//! their ids, receipts never backwards, opt signals once by event id); one that does not retry a 5xx (Twilio, without
//! a fallback URL or connection overrides) loses an event the server answered 503 (docs/interfaces/Chat-Bridges.md).

pub mod sinch;
pub mod telnyx;
pub mod twilio;
