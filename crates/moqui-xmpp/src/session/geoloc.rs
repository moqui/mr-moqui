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

//! User location (XEP-0080) mapped onto Moqui's location shares (plans/Location-Sharing.md §3.6, LL5). The `geoloc`
//! PEP node is never a PEP node here (`pep::REFUSED_NODES`): no item is kept, fetched or subscribed to.
//!
//! - **In.** A publish from the user's own client is a report of the user's active location share made in Moqui with
//!   XMPP contacts in its audience (`LocationServices.report#XmppPosition`, through `report#Position` with the share's
//!   rules: the rate, approximate rounding). With no such share it is refused `not-allowed`, so a client's setting alone
//!   never starts sharing. Of XEP-0080's fields `lat`, `lon`, `accuracy` and `timestamp` are read, checked here and again
//!   by the service; the rest are left out. An empty `geoloc` (the XEP's "stopped") is answered and is no report: it
//!   ends nothing, as a client's setting starts nothing; the share stays the owner's to stop in Moqui.
//! - **Out.** The positions a share takes, from any device or page, reach its XMPP contacts' sessions on the bus topic
//!   `locationShareXmpp:<shareId>` (only those contacts' users, as the services wrote them), each as a `geoloc` event to
//!   a session that asked for it (`geoloc+notify`), from the owner's JID; an ended or revoked share sends an empty
//!   `geoloc` item. What the push carries is what the share's audience may see: an approximate share's cell, never the
//!   fix.
//!
//! No position is logged: a refused call is logged by its service's errors, which name no value.

use minidom::Element;
use serde_json::{json, Value as Json};

use super::xml::{attr, iq_result, parse_stamp, stamp, text_el, E};
use super::Session;
use crate::backend::{self, services};
use crate::ns;

/// A position as XEP-0080 gives it, checked.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Fix {
    pub latitude: f64,
    pub longitude: f64,
    /// Metres.
    pub accuracy: Option<f64>,
    /// Unix milliseconds of `timestamp`.
    pub observed: Option<i64>,
}

/// The fix of a `geoloc` payload: `Ok(None)` for an empty one (stopped), `Err` for one that is not a `geoloc`, has no
/// `lat` or `lon`, or a value out of range or not a number or a time.
pub(super) fn read_fix(payload: &Element) -> Result<Option<Fix>, ()> {
    if !payload.is("geoloc", ns::GEOLOC) {
        return Err(());
    }
    if payload.children().next().is_none() {
        return Ok(None);
    }
    let field = |name: &str| {
        payload
            .get_child(name, ns::GEOLOC)
            .map(|found| found.text().trim().to_string())
    };
    let number = |name: &str, max: f64| -> Result<Option<f64>, ()> {
        match field(name) {
            None => Ok(None),
            Some(text) => match text.parse::<f64>() {
                Ok(value) if value.is_finite() && value.abs() <= max => Ok(Some(value)),
                _ => Err(()),
            },
        }
    };
    let latitude = number("lat", 90.0)?.ok_or(())?;
    let longitude = number("lon", 180.0)?.ok_or(())?;
    let accuracy = number("accuracy", 1e7)?;
    if accuracy.is_some_and(|metres| metres < 0.0) {
        return Err(());
    }
    let observed = match field("timestamp") {
        None => None,
        Some(text) => Some(parse_stamp(&text).ok_or(())?),
    };
    Ok(Some(Fix {
        latitude,
        longitude,
        accuracy,
        observed,
    }))
}

/// A `geoloc` payload of a pushed position (lat, lon, accuracy, timestamp), or the empty one of a share that ended.
pub(super) fn geoloc_payload(message: &Json) -> Option<Element> {
    let mut geoloc = E::new("geoloc", ns::GEOLOC);
    match backend::text(message, "event").as_str() {
        "position" => {
            let (latitude, longitude) = (decimal(message, "latitude")?, decimal(message, "longitude")?);
            if let Some(accuracy) = decimal(message, "accuracy") {
                geoloc = geoloc.c(text_el("accuracy", ns::GEOLOC, accuracy));
            }
            geoloc = geoloc
                .c(text_el("lat", ns::GEOLOC, latitude))
                .c(text_el("lon", ns::GEOLOC, longitude));
            if let Some(observed) = backend::number(message, "observedDate") {
                geoloc = geoloc.c(text_el("timestamp", ns::GEOLOC, stamp(observed)));
            }
        }
        "ended" | "revoked" => {}
        _ => return None,
    }
    Some(geoloc.build())
}

/// A pushed number as an `xs:decimal` (no exponent, no trailing `.0`), whether the push holds it as a number or text.
fn decimal(message: &Json, field: &str) -> Option<String> {
    let value = match message.get(field)? {
        Json::Number(number) => number.as_f64()?,
        Json::String(text) => text.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    value.is_finite().then(|| format!("{value}"))
}

impl Session {
    /// A user location publish: a report of the user's location share with XMPP contacts, never a PEP item.
    pub(super) async fn geoloc_publish(&mut self, iq: &Element, publish: &Element) {
        let Some(item) = publish.get_child("item", ns::PUBSUB) else {
            return self.pubsub_error(iq, "modify", "bad-request", "item-required");
        };
        let Some(payload) = item.children().next() else {
            return self.pubsub_error(iq, "modify", "bad-request", "payload-required");
        };
        let Ok(fix) = read_fix(payload) else {
            return self.pubsub_error(iq, "modify", "bad-request", "invalid-payload");
        };
        let item_id = Some(attr(item, "id"))
            .filter(|id| !id.is_empty())
            .unwrap_or_else(super::xml::random_id);
        // "stopped" is no report, and ends nothing: the share is the owner's to stop in Moqui, where they see it
        let Some(fix) = fix else {
            return self.geoloc_published(iq, item_id);
        };
        let found = self
            .call(
                services::LOCATION_REPORT,
                json!({"latitude": fix.latitude.to_string(), "longitude": fix.longitude.to_string(),
                    "accuracy": fix.accuracy.map(|metres| metres.to_string()), "observedMillis": fix.observed}),
            )
            .await;
        let Ok(found) = found else {
            return self.refuse(iq, "wait", "internal-server-error", None);
        };
        match (
            backend::number(&found, "statusCode"),
            backend::text(&found, "reasonCode").as_str(),
        ) {
            // taken, or older than what the share holds already (nothing new to take)
            (Some(200), _) | (_, "STALE") => self.geoloc_published(iq, item_id),
            // no share of the user's own with XMPP contacts that this client may send: a client never starts one
            (_, "NO_XMPP_SHARE") => self.refuse(
                iq,
                "cancel",
                "not-allowed",
                Some("start a location share with XMPP contacts in Moqui first"),
            ),
            (Some(429), _) => self.refuse(iq, "wait", "policy-violation", Some("too soon after the last position")),
            (Some(400), _) => self.pubsub_error(iq, "modify", "bad-request", "invalid-payload"),
            _ => self.refuse(iq, "wait", "internal-server-error", None),
        }
    }

    fn geoloc_published(&mut self, iq: &Element, item_id: String) {
        let result = iq_result(iq)
            .c(
                E::new("pubsub", ns::PUBSUB).c(E::new("publish", ns::PUBSUB).a("node", ns::GEOLOC).c(E::new(
                    "item",
                    ns::PUBSUB,
                )
                .a("id", item_id))),
            )
            .build();
        self.send(result);
    }

    /// A location share's position or end for this session's user, an XMPP contact of the share: a `geoloc` event from
    /// the owner, to a session that asked for user location notifications and has not blocked the owner.
    pub(super) async fn geoloc_out(&mut self, message: &Json) {
        if !moqui_lib::sync::lock(&self.notify).contains(ns::GEOLOC) {
            return;
        }
        let owner = backend::text(message, "ownerUserId");
        if owner.is_empty() || owner == self.user_id() {
            return;
        }
        let Some(payload) = geoloc_payload(message) else {
            return;
        };
        let Some(from) = self.jid_of(&owner).await else {
            return;
        };
        if self.is_blocked(&from).await {
            return;
        }
        // one item a share, so a client that holds items by id sees each share's end
        let item = E::new("item", ns::PUBSUB_EVENT)
            .a("id", format!("share-{}", backend::text(message, "shareId")))
            .c(payload);
        let event =
            E::new("event", ns::PUBSUB_EVENT).c(E::new("items", ns::PUBSUB_EVENT).a("node", ns::GEOLOC).c(item));
        let stanza = E::new("message", ns::CLIENT)
            .a("type", "headline")
            .a("from", from)
            .a("to", self.my_full())
            .a("id", super::xml::random_id())
            .c(event)
            .build();
        self.send(stanza);
    }
}
