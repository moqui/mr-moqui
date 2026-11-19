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

//! How a chat push leaves the server (plans/Comms-Messaging.md §7.3): the [`PushTransport`] the worker
//! ([`super`]) sends through, the push services on a server ([`Network`]: FCM's HTTP v1 API and a Web Push POST) or a
//! test's stand-in ([`set_transport`]), and what an answer means ([`Sent`]).

use std::sync::{Arc, RwLock};

use moqui_lib::sync::{read, write};

use super::MAX_WAIT_MILLIS;
use crate::context::ExecutionContextFactory;
use crate::entity::EntityValue;
use crate::notify::push_crypto;
use crate::service::chat_bridge::text_of;
use crate::service::{fcm, web_push};

/// A device's push provider (`WebPushSubscription.pushProviderEnumId`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Provider {
    /// A browser's Web Push subscription.
    Web,
    /// A native app's FCM registration.
    Fcm,
    /// A native app's UnifiedPush distributor endpoint, posted to as Web Push is.
    Unified,
}

impl Provider {
    pub(super) fn of(device: &EntityValue) -> Self {
        match text_of(device, "pushProviderEnumId").as_deref() {
            Some("PushFcm") => Self::Fcm,
            Some("PushUnified") => Self::Unified,
            _ => Self::Web,
        }
    }
}

/// What one send came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sent {
    /// The push service took it.
    Delivered,
    /// The device is gone at its push service (FCM UNREGISTERED, Web Push 404 or 410).
    Gone,
    /// The push service refused this push (not the device): failed, not sent again.
    Refused(String),
    /// Not taken now: sent again after `after_millis` (the service's Retry-After) or the backoff.
    Retry { after_millis: Option<i64>, error: String },
}

/// A Web Push or UnifiedPush POST: the encrypted body and its headers (RFC 8030).
#[derive(Clone, Debug)]
pub struct WebPost<'a> {
    pub endpoint: &'a str,
    pub body: Vec<u8>,
    pub ttl_seconds: i64,
    /// high for a notification to show, normal for a clear.
    pub urgency: &'static str,
    /// Replaces a push of the same topic the device has not received: one per room.
    pub topic: &'a str,
}

/// How a push leaves the server: the push services on a server ([`Network`]), a recording stand-in in the tests
/// ([`set_transport`]).
pub trait PushTransport: Send + Sync {
    /// Whether this server can push to a device of `provider` (FCM's service account, the VAPID key, the `web-push`
    /// feature). A device it cannot is given no push.
    fn available(&self, provider: Provider) -> bool;
    /// Send an FCM `messages:send` body to the registration `device` (its id, for the log).
    fn fcm(&self, device: &str, message: &serde_json::Value) -> Sent;
    /// POST to a Web Push or UnifiedPush endpoint.
    fn web(&self, device: &str, provider: Provider, post: &WebPost<'_>) -> Sent;
    /// How long a Web Push or UnifiedPush send may take before it counts as a failure of its endpoint, though taken:
    /// half the request's timeout (SEC-COM-81).
    fn slow_after(&self) -> std::time::Duration {
        SLOW_SEND
    }
}

/// [`PushTransport::slow_after`] of the push services: half of `web_push`'s 10-second request timeout.
pub const SLOW_SEND: std::time::Duration = std::time::Duration::from_secs(5);

/// The push services: FCM's HTTP v1 API ([`fcm`]) and a Web Push POST ([`web_push::post`]), with the server's VAPID
/// token (required for a browser, given to a UnifiedPush distributor when the key is set).
struct Network;

impl PushTransport for Network {
    fn available(&self, provider: Provider) -> bool {
        match provider {
            Provider::Fcm => {
                let configured = fcm::configured();
                if !configured {
                    fcm::note_unset();
                }
                configured
            }
            Provider::Web => push_crypto::BUILT && web_push::vapid_signer().is_some(),
            Provider::Unified => push_crypto::BUILT,
        }
    }

    fn fcm(&self, device: &str, message: &serde_json::Value) -> Sent {
        let config = match fcm::config() {
            Ok(Some(config)) => config,
            Ok(None) => return Sent::Refused("FCM is not configured (push/fcm@service-account)".to_string()),
            Err(reason) => {
                return Sent::Retry {
                    after_millis: None,
                    error: reason,
                }
            }
        };
        let now_secs = jiff::Timestamp::now().as_second();
        match fcm::sender(&config).attempt(device, message, now_secs) {
            fcm::Attempt::Delivered => Sent::Delivered,
            fcm::Attempt::Gone => Sent::Gone,
            fcm::Attempt::Rejected(codes) => Sent::Refused(codes),
            fcm::Attempt::Retry { after, error } => Sent::Retry {
                after_millis: after.map(|wait| i64::try_from(wait.as_millis()).unwrap_or(MAX_WAIT_MILLIS)),
                error,
            },
        }
    }

    fn web(&self, device: &str, provider: Provider, post: &WebPost<'_>) -> Sent {
        let signer = web_push::vapid_signer();
        if provider == Provider::Web && signer.is_none() {
            return Sent::Refused("push/web@vapid-key is not set".to_string());
        }
        let now_secs = u64::try_from(jiff::Timestamp::now().as_second()).unwrap_or(0);
        let authorization = signer.and_then(|(key, subject)| key.authorization(post.endpoint, &subject, now_secs).ok());
        let delivery = web_push::Delivery {
            ttl: u32::try_from(post.ttl_seconds).unwrap_or(u32::MAX),
            urgency: post.urgency,
            topic: Some(post.topic.to_string()),
        };
        match web_push::post(post.endpoint, post.body.clone(), authorization.as_deref(), &delivery) {
            Ok((status, retry_after)) => web_outcome(status, retry_after),
            Err(error) => {
                tracing::info!(subscription_id = %device, "push not sent: {error}");
                Sent::Retry {
                    after_millis: None,
                    error,
                }
            }
        }
    }
}

/// What a Web Push or UnifiedPush service's answer means: 404 and 410 the subscription is gone, 429 and 5xx a wait
/// (with the Retry-After seconds), another a refusal of this push.
pub fn web_outcome(status: u16, retry_after_secs: Option<i64>) -> Sent {
    match status {
        200..=299 => Sent::Delivered,
        404 | 410 => Sent::Gone,
        429 | 500..=599 => Sent::Retry {
            after_millis: retry_after_secs.map(|secs| secs.saturating_mul(1000)),
            error: format!("the push service answered {status}"),
        },
        _ => Sent::Refused(format!("the push service answered {status}")),
    }
}

#[derive(Default)]
struct Slot(RwLock<Option<Arc<dyn PushTransport>>>);

/// Send this factory's pushes through `transport` (a test's stand-in) instead of the push services.
pub fn set_transport(factory: &ExecutionContextFactory, transport: Arc<dyn PushTransport>) {
    *write(&factory.extension(|_| Slot::default()).0) = Some(transport);
}

/// The factory's transport: a test's stand-in, else the push services.
pub(super) fn transport(factory: &ExecutionContextFactory) -> Arc<dyn PushTransport> {
    read(&factory.extension(|_| Slot::default()).0)
        .clone()
        .unwrap_or_else(|| Arc::new(Network))
}
