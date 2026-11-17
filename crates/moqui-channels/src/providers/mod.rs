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
//! | Module | Adapter (`adapterName`) | Webhook signature, checked on |
//! | --- | --- | --- |
//! | [`telnyx`] | `sms` | Ed25519 under the account's public key: the edge node |
//! | [`twilio`] | `sms-twilio` | HMAC-SHA1 under the Auth Token over the URL and the form: the local node |

pub mod telnyx;
pub mod twilio;
