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

//! Push notifications (XEP-0357): a session that is detached (its connection dropped and it waits for resumption, as
//! a phone's app does when the system suspends it) and gets a message tells each of its user's app servers that
//! something waits: a `urn:xmpp:push:summary` form with the message count and nothing else, no sender or text. At most
//! one a minute per session. The app servers (Conversations' `p2.siacs.eu`, Monal's) are other XMPP servers, so the
//! notification goes out through [`crate::Outbound`], which needs federation (C3b); until then the host's outbound
//! logs it as not sent.

use std::time::{Duration, Instant};

use minidom::Element;
use serde_json::{json, Value as Json};

use super::xml::{form, random_id, E};
use super::Session;
use crate::backend::{self, services};
use crate::ns;

const EVERY: Duration = Duration::from_secs(60);

impl Session {
    /// A message arrived while the session is detached: notify the user's app servers, at most once a minute.
    pub(super) async fn push_waiting(&mut self) {
        if !self.detached || self.last_push.is_some_and(|at| at.elapsed() < EVERY) {
            return;
        }
        self.last_push = Some(Instant::now());
        self.pushed_count += 1;
        let found = self
            .call_internal(services::PUSH_TARGETS, json!({"userId": self.user_id()}))
            .await
            .unwrap_or(Json::Null);
        for target in backend::list(&found, "targetList") {
            let notification = self.push_notification(target);
            if let Err(err) = self.server.outbound.send(notification) {
                tracing::debug!("XMPP push to {} not sent: {err}", backend::text(target, "pushJid"));
            }
        }
    }

    /// The publish to one app server: the count of messages waiting, and the registration's options (its secret).
    pub(crate) fn push_notification(&self, target: &Json) -> Element {
        let count = self.pushed_count.to_string();
        let summary = form("submit", "urn:xmpp:push:summary", &[("message-count", count.as_str())]);
        let options: Vec<(String, String)> = target
            .get("options")
            .and_then(Json::as_object)
            .map(|options| {
                options
                    .iter()
                    .map(|(var, value)| (var.clone(), value.as_str().unwrap_or_default().to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let fields: Vec<(&str, &str)> = options
            .iter()
            .map(|(var, value)| (var.as_str(), value.as_str()))
            .collect();
        let publish_options = form("submit", "http://jabber.org/protocol/pubsub#publish-options", &fields);
        E::new("iq", ns::CLIENT)
            .a("type", "set")
            .a("id", random_id())
            .a("from", self.my_bare())
            .a("to", backend::text(target, "pushJid"))
            .c(E::new("pubsub", ns::PUBSUB)
                .c(E::new("publish", ns::PUBSUB)
                    .a("node", backend::text(target, "node"))
                    .c(E::new("item", ns::PUBSUB).c(E::new("notification", ns::PUSH).c(summary))))
                .c(E::new("publish-options", ns::PUBSUB).c(publish_options)))
            .build()
    }
}
