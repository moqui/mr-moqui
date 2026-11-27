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

//! User location (XEP-0080) mapped onto Moqui's location shares (plans/archive/Location-Sharing.md §3.6, LL5). The `geoloc`
//! PEP node is never a PEP node here (`pep::REFUSED_NODES`): no item is kept, fetched or subscribed to.
//!
//! - **In.** A publish from the user's own client is taken by each of the user's active location shares made in Moqui
//!   with XMPP contacts whose owner turned on its XMPP app's location (`LocationServices.report#XmppPosition`, with the
//!   share's rules: the rate, approximate rounding). With no such share it is refused `not-allowed`, so a client's
//!   setting alone never starts or feeds a share; one too soon or older than the last is answered as taken, with nothing
//!   new kept. Of XEP-0080's fields `lat`, `lon`, `accuracy` and `timestamp` are read, checked here and again by the
//!   service; the rest are left out. An empty `geoloc` (the XEP's "stopped") is answered and is no report: it ends
//!   nothing, as a client's setting starts nothing; the share stays the owner's to stop in Moqui.
//! - **Out.** The positions a share takes, from any device or page, reach its XMPP contacts' sessions on the bus topic
//!   `locationShareXmpp:<shareId>` (only those contacts' users, as the services wrote them), each as a `geoloc` event to
//!   a session that asked for it (`geoloc+notify`), from the owner's JID; an ended or revoked share sends an empty
//!   `geoloc` item, also when its time ends it (the session keeps each share's end from its pushes, and sends the item at
//!   its tick: the services end it only at their hourly clean). What the push carries is what the share's audience may
//!   see: an approximate share's cell, never the fix.
//! - **The opt-in.** A publish feeds only a share whose owner turned on `takeXmppLocation` in Moqui, and goes to its XMPP
//!   contacts only, never to its users, groups or link; the first one a share takes tells the owner's XMPP sessions by
//!   a headline (whom with, until when).
//!
//! No position is logged: a refused call is logged by its service's errors, which name no value.

use minidom::Element;
use serde_json::{json, Value as Json};

use super::xml::{attr, iq_result, parse_stamp, stamp, text_el, E};
use super::Session;
use crate::backend::{self, services};
use crate::ns;

/// How many shares' ends a session keeps (each a share that sent it a position).
const GEOLOC_ENDS_MAX: usize = 256;

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
            // taken; or older than what the share holds, or too soon after the last one (its rate): nothing new to take,
            // and a client that publishes on every move is not shown an error for it
            (Some(200 | 429), _) | (_, "STALE") => self.geoloc_published(iq, item_id),
            // no share of the user's own that takes its XMPP app's location (the owner's choice in Moqui), or one that
            // ended or went meanwhile: a client never starts or feeds one
            (_, "NO_XMPP_SHARE" | "OTHER_DEVICE") | (Some(403 | 404 | 409 | 410), _) => self.refuse(
                iq,
                "cancel",
                "not-allowed",
                Some("turn on your XMPP app's location for a location share in Moqui first"),
            ),
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

    /// A location share's push for this session's user (plans/archive/Location-Sharing.md §3.6). To an XMPP contact of the
    /// share: a position or the end as a `geoloc` event from the owner, to a session that asked for user location
    /// notifications and has not blocked the owner, and the share's end kept, so the empty item goes out when it comes
    /// ([`Session::geoloc_expire`]) though the services end it only at their hourly clean. To the owner: the headline that
    /// their XMPP app's location now feeds a share.
    pub(super) async fn geoloc_out(&mut self, message: &Json) {
        let owner = backend::text(message, "ownerUserId");
        let share_id = backend::text(message, "shareId");
        let event = backend::text(message, "event");
        if owner.is_empty() || share_id.is_empty() {
            return;
        }
        if owner == self.user_id() {
            if event == "feeding" {
                let text = backend::text(message, "text");
                let headline = E::new("message", ns::CLIENT)
                    .a("type", "headline")
                    .a("from", self.domain())
                    .a("to", self.my_full())
                    .a("id", super::xml::random_id())
                    .c(text_el("body", ns::CLIENT, text))
                    .build();
                self.send(headline);
            }
            return;
        }
        // a detached session (waiting for its resumption) keeps what it is sent as unacknowledged stanzas: another
        // user's positions, which no archive holds, are not kept for it, so a share's reports cannot fill its queue
        // and end the resumption (SEC-COM-19, SEC-COM-93); the end of a share whose position it was sent still is,
        // at most one for each of [`GEOLOC_ENDS_MAX`] shares
        let shown = self.geoloc_ends.contains_key(&share_id);
        if self.detached && !(matches!(event.as_str(), "ended" | "revoked") && shown) {
            return;
        }
        match event.as_str() {
            "ended" | "revoked" => {
                self.geoloc_ends.remove(&share_id);
            }
            "position" | "changed" => {
                if let Some(thru) = backend::number(message, "thruDate") {
                    if self.geoloc_ends.len() < GEOLOC_ENDS_MAX || self.geoloc_ends.contains_key(&share_id) {
                        self.geoloc_ends.insert(share_id.clone(), (owner.clone(), thru));
                    }
                }
            }
            _ => {}
        }
        if let Some(payload) = geoloc_payload(message) {
            self.geoloc_send(&owner, &share_id, payload).await;
        }
    }

    /// The shares whose end has come by `now` (Unix milliseconds), each sent its empty item, as an ended share's.
    pub(super) async fn geoloc_expire(&mut self, now: i64) {
        let ended: Vec<(String, String)> = self
            .geoloc_ends
            .iter()
            .filter(|(_, (_, thru))| *thru <= now)
            .map(|(share_id, (owner, _))| (share_id.clone(), owner.clone()))
            .collect();
        for (share_id, owner) in ended {
            self.geoloc_ends.remove(&share_id);
            self.geoloc_send(&owner, &share_id, E::new("geoloc", ns::GEOLOC).build())
                .await;
        }
    }

    async fn geoloc_send(&mut self, owner: &str, share_id: &str, payload: Element) {
        if !moqui_lib::sync::lock(&self.notify).contains(ns::GEOLOC) {
            return;
        }
        let Some(from) = self.jid_of(owner).await else {
            return;
        };
        if self.is_blocked(&from).await {
            return;
        }
        // one item a share, so a client that holds items by id sees each share's end
        let item = E::new("item", ns::PUBSUB_EVENT)
            .a("id", format!("share-{share_id}"))
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
