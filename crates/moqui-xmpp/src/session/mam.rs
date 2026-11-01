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

//! Message archive management (XEP-0313) over the rooms' timelines (`XmppServices.get#Archive`).
//!
//! A room's archive is its messages, by eventSeq (the stanza-id). The user's own archive is the messages of their
//! direct rooms, by `<room>:<seq>`; `with` narrows it to one person's room, and without it the rooms' messages come
//! merged by time. Paging is RSM (XEP-0059): `after` and `before` an id, `<before/>` for the last page, `max` up to
//! 250; `start` and `end` limit by time. Edits come applied (the latest text, under the message's own id) and deleted
//! messages are left out; reactions are not in the archive.

use minidom::Element;
use serde_json::{json, Value as Json};

use super::xml::{attr, child_text, form_fields, iq_result, parse_stamp, text_el, E};
use super::Session;
use crate::backend::{self, services};
use crate::{jids, ns};

const DEFAULT_MAX: i64 = 50;
const MOST: i64 = 250;

/// An archive id: (chatRoomId when in the user's archive, eventSeq).
fn parse_id(id: &str) -> Option<(Option<String>, i64)> {
    match id.rsplit_once(':') {
        Some((node, seq)) => Some((Some(jids::room_id(node)?), seq.parse().ok()?)),
        None => Some((None, id.parse().ok()?)),
    }
}

impl Session {
    pub(super) async fn mam_query(&mut self, iq: &Element, query: &Element, room: Option<String>) {
        let query_id = attr(query, "queryid");
        let mut parameters = json!({});
        if let Some(room) = &room {
            match self.room(room, false).await {
                Some(meta) if !meta.direct && !meta.encrypted => {}
                _ => return self.refuse(iq, "cancel", "item-not-found", None),
            }
            parameters["chatRoomId"] = json!(room);
        }
        for (var, values) in query.get_child("x", ns::DATA).map(form_fields).unwrap_or_default() {
            let value = values.first().cloned().unwrap_or_default();
            match var.as_str() {
                "with" if room.is_none() => match self.target(&value).await {
                    super::iq::Target::User(user_id, _) => parameters["withUserId"] = json!(user_id),
                    _ => return self.empty_fin(iq, &query_id),
                },
                "start" => match parse_stamp(&value) {
                    Some(at) => parameters["startDate"] = json!(at),
                    None => return self.refuse(iq, "modify", "bad-request", Some("start is not a timestamp")),
                },
                "end" => match parse_stamp(&value) {
                    Some(at) => parameters["endDate"] = json!(at),
                    None => return self.refuse(iq, "modify", "bad-request", Some("end is not a timestamp")),
                },
                "after-id" => parameters["after"] = json!(value),
                "before-id" => parameters["before"] = json!(value),
                "FORM_TYPE" | "" => {}
                _ => return self.refuse(iq, "modify", "bad-request", Some("a field this archive does not take")),
            }
        }
        let mut limit = DEFAULT_MAX;
        let mut flip = false;
        if let Some(set) = query.get_child("set", ns::RSM) {
            if let Ok(max) = child_text(set, "max", ns::RSM).parse::<i64>() {
                limit = max.clamp(0, MOST);
            }
            if let Some(after) = set.get_child("after", ns::RSM) {
                parameters["after"] = json!(after.text());
            }
            if let Some(before) = set.get_child("before", ns::RSM) {
                flip = true;
                if !before.text().is_empty() {
                    parameters["before"] = json!(before.text());
                }
            }
        }
        for (key, out) in [("after", "afterSeq"), ("before", "beforeSeq")] {
            let Some(id) = parameters.get(key).and_then(Json::as_str).map(str::to_string) else {
                continue;
            };
            let Some((id_room, seq)) = parse_id(&id) else {
                return self.item_not_found(iq);
            };
            parameters[out] = json!(seq);
            if let Some(id_room) = id_room {
                parameters[format!("{key}RoomId")] = json!(id_room);
            }
        }
        parameters["limit"] = json!(limit);
        parameters["latest"] = json!(flip);
        let found = match self.call(services::ARCHIVE, parameters).await {
            Ok(found) => found,
            Err(err) => return self.refuse(iq, "cancel", "item-not-found", Some(&err.0)),
        };
        let mut first = None;
        let mut last = None;
        for event in backend::list(&found, "eventList").to_vec() {
            let event_room = room.clone().unwrap_or_else(|| backend::text(&event, "chatRoomId"));
            let Some(meta) = self.room(&event_room, false).await else {
                continue;
            };
            let Some(seq) = backend::number(&event, "eventSeq") else {
                continue;
            };
            let Some(inner) = self.event_message(&event_room, &meta, &event, false).await else {
                continue;
            };
            let id = self.stanza_id(&event_room, seq, &meta);
            first.get_or_insert_with(|| id.clone());
            last = Some(id.clone());
            let stamp = super::xml::stamp(backend::number(&event, "sentDate").unwrap_or(0));
            let result = E::new("message", ns::CLIENT)
                .a("to", self.my_full())
                .a(
                    "from",
                    if room.is_some() {
                        self.room_jid(&event_room)
                    } else {
                        self.my_bare()
                    },
                )
                .a("id", super::xml::random_id())
                .c(E::new("result", ns::MAM)
                    .a("queryid", query_id.clone())
                    .a("id", id)
                    .c(E::new("forwarded", ns::FORWARD)
                        .c(E::new("delay", ns::DELAY).a("stamp", stamp))
                        .c(inner)))
                .build();
            self.send(result);
        }
        let mut set = E::new("set", ns::RSM);
        if let (Some(first), Some(last)) = (first, last) {
            set = set
                .c(text_el("first", ns::RSM, first))
                .c(text_el("last", ns::RSM, last));
        }
        if let Some(count) = backend::number(&found, "count") {
            set = set.c(text_el("count", ns::RSM, count.to_string()));
        }
        let complete = backend::flag(&found, "complete");
        let mut fin = E::new("fin", ns::MAM).a("queryid", query_id).c(set);
        if complete {
            fin = fin.a("complete", "true");
        }
        self.send(iq_result(iq).c(fin).build());
    }

    fn empty_fin(&mut self, iq: &Element, query_id: &str) {
        let fin = E::new("fin", ns::MAM)
            .a("queryid", query_id)
            .a("complete", "true")
            .c(E::new("set", ns::RSM));
        self.send(iq_result(iq).c(fin).build());
    }

    fn item_not_found(&mut self, iq: &Element) {
        self.refuse(iq, "cancel", "item-not-found", None);
    }
}
