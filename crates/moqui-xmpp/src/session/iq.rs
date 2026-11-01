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

//! The iq stanzas: where each goes by its address, and the server's own answers: service discovery (XEP-0030),
//! ping (XEP-0199), version (XEP-0092), the roster (RFC 6121: the people of the user's direct rooms), vCard (XEP-0054,
//! read only: the name comes from Moqui), carbons (XEP-0280), blocking (XEP-0191), push registration (XEP-0357), upload
//! slots (XEP-0363) and external services (XEP-0215).

use std::sync::atomic::Ordering;

use minidom::Element;
use serde_json::{json, Value as Json};

use super::xml::{attr, form, iq_result, text_el, E};
use super::Session;
use crate::backend::{self, services};
use crate::registry::Inbound;
use crate::{features, jids, ns};

/// Where a stanza is addressed.
pub(crate) enum Target {
    /// The server itself (no `to`, or the domain).
    Server,
    /// The user's own bare JID.
    Own,
    /// Another local user: their userId and resource.
    User(String, Option<String>),
    /// The rooms service, a room, or an occupant (chatRoomId, nick).
    Rooms,
    Room(String, Option<String>),
    Upload,
    /// A JID this server does not serve (another server: federation is C3b) or that does not exist.
    Unknown,
}

impl Session {
    pub(crate) async fn target(&mut self, to: &str) -> Target {
        if to.is_empty() {
            return Target::Server;
        }
        let Some(address) = jids::parse(to) else {
            return Target::Unknown;
        };
        let config = &self.server.config;
        if address.domain == config.domain {
            return match address.node {
                None => Target::Server,
                Some(node) if address.resource.is_none() && format!("{node}@{}", address.domain) == self.my_bare() => {
                    Target::Own
                }
                Some(node) => match self.user_id_of(&node).await {
                    Some(user_id) if user_id == self.user_id() && address.resource.is_none() => Target::Own,
                    Some(user_id) => Target::User(user_id, address.resource),
                    None => Target::Unknown,
                },
            };
        }
        if address.domain == config.rooms_domain {
            return match address.node.as_deref().and_then(jids::room_id) {
                Some(room) => Target::Room(room, address.resource),
                None if address.node.is_none() => Target::Rooms,
                None => Target::Unknown,
            };
        }
        if address.domain == config.upload_domain && address.node.is_none() {
            return Target::Upload;
        }
        Target::Unknown
    }

    pub(super) async fn on_iq(&mut self, iq: Element) {
        let kind = attr(&iq, "type");
        let to = attr(&iq, "to");
        if kind == "result" || kind == "error" {
            if let Some(ver) = self.caps_asked.remove(&attr(&iq, "id")) {
                self.caps_answered(&ver, &iq);
                return;
            }
        }
        let target = self.target(&to).await;
        if let Target::User(user_id, Some(resource)) = &target {
            // to another user's resource (Jingle, a disco of their client): routed as it is
            return self.route_to(user_id, Some(resource), iq).await;
        }
        if kind != "get" && kind != "set" {
            return;
        }
        let Some(payload) = iq.children().next().cloned() else {
            return self.refuse(&iq, "modify", "bad-request", None);
        };
        let namespace = payload.ns();
        match (target, payload.name(), namespace.as_str()) {
            (_, "ping", ns::PING) if kind == "get" => self.send(iq_result(&iq).build()),
            (target, "query", ns::DISCO_INFO) if kind == "get" => self.disco_info(&iq, &payload, target).await,
            (target, "query", ns::DISCO_ITEMS) if kind == "get" => self.disco_items(&iq, target).await,
            (Target::Server, "query", ns::VERSION) if kind == "get" => self.version(&iq),
            (Target::Server | Target::Own, "query", ns::ROSTER) => self.roster(&iq, &payload).await,
            (Target::Server | Target::Own, "session", ns::SESSION) => self.send(iq_result(&iq).build()),
            (Target::Server | Target::Own | Target::User(..), "vCard", ns::VCARD) => self.vcard(&iq, &to).await,
            (Target::Server | Target::Own, "enable" | "disable", ns::CARBONS) if kind == "set" => {
                self.carbons.store(payload.name() == "enable", Ordering::Relaxed);
                self.send(iq_result(&iq).build());
            }
            (Target::Server | Target::Own, _, ns::BLOCKING) => self.blocking(&iq, &payload).await,
            (Target::Server, "services", ns::EXTDISCO) if kind == "get" => self.external_services(&iq).await,
            (Target::Server | Target::Own, "enable" | "disable", ns::PUSH) if kind == "set" => {
                self.push_registration(&iq, &payload).await
            }
            (Target::Upload, "request", ns::UPLOAD) if kind == "get" => self.upload_slot(&iq, &payload).await,
            (Target::Own, "query", ns::MAM) | (Target::Server, "query", ns::MAM) if kind == "set" => {
                self.mam_query(&iq, &payload, None).await
            }
            (Target::Room(room, None), "query", ns::MAM) if kind == "set" => {
                self.mam_query(&iq, &payload, Some(room)).await
            }
            (Target::Own | Target::Server, "pubsub", ns::PUBSUB | ns::PUBSUB_OWNER) => {
                self.pep(&iq, &payload, None).await
            }
            (Target::User(user_id, None), "pubsub", ns::PUBSUB) if kind == "get" => {
                self.pep(&iq, &payload, Some(user_id)).await
            }
            (Target::Room(room, Some(nick)), "ping", ns::PING) => self.self_ping(&iq, &room, &nick),
            (Target::Unknown, ..) => self.refuse(&iq, "cancel", "remote-server-not-found", None),
            _ => self.refuse(&iq, "cancel", "service-unavailable", None),
        }
    }

    /// Send a stanza to another local user's sessions: one resource, or every available one.
    pub(crate) async fn route_to(&mut self, user_id: &str, resource: Option<&str>, stanza: Element) {
        let entries = match resource {
            Some(resource) => self.server.registry.session(user_id, resource).into_iter().collect(),
            None => self
                .server
                .registry
                .sessions_of(user_id)
                .into_iter()
                .filter(|entry| entry.is_available())
                .collect::<Vec<_>>(),
        };
        if entries.is_empty() {
            if stanza.is("iq", ns::CLIENT) || resource.is_some() && stanza.is("message", ns::CLIENT) {
                self.refuse(&stanza, "cancel", "service-unavailable", None);
            }
            return;
        }
        for entry in entries {
            self.server.registry.send(&entry, Inbound::Stanza(stanza.clone()));
        }
    }

    /// A stanza another session routed here.
    pub(super) async fn on_routed(&mut self, stanza: Element) {
        let from = attr(&stanza, "from");
        if let Some(sender) = jids::parse(&from) {
            let bare = sender.bare();
            if self.is_blocked(&bare).await {
                return;
            }
        }
        let mut stanza = stanza;
        super::xml::set_attr(&mut stanza, "to", self.my_full());
        if stanza.is("presence", ns::CLIENT) {
            self.send_deferrable(stanza);
        } else {
            self.send(stanza);
        }
    }

    async fn disco_info(&mut self, iq: &Element, payload: &Element, target: super::iq::Target) {
        let node = attr(payload, "node");
        let (identity, list, extra): (Element, &[&str], Option<Element>) = match target {
            Target::Server => (
                E::new("identity", ns::DISCO_INFO)
                    .a("category", "server")
                    .a("type", "im")
                    .a("name", "Moqui")
                    .build(),
                features::SERVER,
                None,
            ),
            Target::Own | Target::User(_, None) => (
                E::new("identity", ns::DISCO_INFO)
                    .a("category", "account")
                    .a("type", "registered")
                    .build(),
                features::ACCOUNT,
                None,
            ),
            Target::Rooms => (
                E::new("identity", ns::DISCO_INFO)
                    .a("category", "conference")
                    .a("type", "text")
                    .a("name", "Rooms")
                    .build(),
                features::ROOMS,
                None,
            ),
            Target::Room(room, None) => {
                let Some(meta) = self.room(&room, false).await.filter(|meta| !meta.direct) else {
                    return self.refuse(iq, "cancel", "item-not-found", None);
                };
                let info = form(
                    "result",
                    "http://jabber.org/protocol/muc#roominfo",
                    &[("muc#roominfo_description", meta.topic.as_str())],
                );
                (
                    E::new("identity", ns::DISCO_INFO)
                        .a("category", "conference")
                        .a("type", "text")
                        .a("name", meta.name.clone())
                        .build(),
                    features::ROOM,
                    Some(info),
                )
            }
            Target::Upload => {
                let max = self.server.config.upload_max_bytes.to_string();
                (
                    E::new("identity", ns::DISCO_INFO)
                        .a("category", "store")
                        .a("type", "file")
                        .a("name", "Upload")
                        .build(),
                    features::UPLOAD,
                    Some(form("result", ns::UPLOAD, &[("max-file-size", max.as_str())])),
                )
            }
            _ => return self.refuse(iq, "cancel", "item-not-found", None),
        };
        let mut query = E::new("query", ns::DISCO_INFO).a("node", node).c(identity);
        if list == features::ACCOUNT {
            query = query.c(E::new("identity", ns::DISCO_INFO)
                .a("category", "pubsub")
                .a("type", "pep"));
        }
        for var in list {
            if *var == ns::VERSION && self.server.config.software_version.is_empty() {
                continue;
            }
            query = query.c(E::new("feature", ns::DISCO_INFO).a("var", *var));
        }
        if let Some(extra) = extra {
            query = query.c(extra);
        }
        self.send(iq_result(iq).c(query).build());
    }

    async fn disco_items(&mut self, iq: &Element, target: Target) {
        let mut query = E::new("query", ns::DISCO_ITEMS);
        match target {
            Target::Server => {
                let config = &self.server.config;
                query = query
                    .c(E::new("item", ns::DISCO_ITEMS)
                        .a("jid", config.rooms_domain.clone())
                        .a("name", "Rooms"))
                    .c(E::new("item", ns::DISCO_ITEMS)
                        .a("jid", config.upload_domain.clone())
                        .a("name", "Upload"));
            }
            Target::Rooms => {
                let found = self
                    .call(
                        services::FIND_ROOMS,
                        json!({"filter": "rooms", "includeJoinable": true}),
                    )
                    .await;
                for room in found
                    .as_ref()
                    .map(|found| backend::list(found, "roomList"))
                    .unwrap_or_default()
                {
                    if backend::flag(room, "encrypted") {
                        continue;
                    }
                    let id = backend::text(room, "chatRoomId");
                    query = query.c(E::new("item", ns::DISCO_ITEMS)
                        .a("jid", self.room_jid(&id))
                        .a("name", backend::text(room, "displayName")));
                }
            }
            Target::Own | Target::User(_, None) | Target::Room(_, None) | Target::Upload => {}
            _ => return self.refuse(iq, "cancel", "item-not-found", None),
        }
        self.send(iq_result(iq).c(query).build());
    }

    fn version(&mut self, iq: &Element) {
        let version = self.server.config.software_version.clone();
        if version.is_empty() {
            return self.refuse(iq, "cancel", "service-unavailable", None);
        }
        let query = E::new("query", ns::VERSION)
            .c(text_el("name", ns::VERSION, "Moqui"))
            .c(text_el("version", ns::VERSION, version));
        self.send(iq_result(iq).c(query).build());
    }

    /// The roster: the other members of the user's direct rooms, subscribed both ways. Adding a local user opens a
    /// direct room with them; removing changes nothing in Moqui.
    async fn roster(&mut self, iq: &Element, payload: &Element) {
        if attr(iq, "type") == "set" {
            let Some(item) = payload.get_child("item", ns::ROSTER) else {
                return self.refuse(iq, "modify", "bad-request", None);
            };
            if attr(item, "subscription") != "remove" {
                let target = self.target(&attr(item, "jid")).await;
                let Target::User(user_id, None) = target else {
                    return self.refuse(iq, "cancel", "item-not-found", Some("only this server's users"));
                };
                if self
                    .call(services::DIRECT_ROOM, json!({"userId": user_id}))
                    .await
                    .is_err()
                {
                    return self.refuse(iq, "cancel", "not-allowed", None);
                }
                let push = self.roster_query(&[user_id]).await;
                self.send(
                    E::new("iq", ns::CLIENT)
                        .a("type", "set")
                        .a("id", super::xml::random_id())
                        .c(push)
                        .build(),
                );
            }
            return self.send(iq_result(iq).build());
        }
        let found = self.call(services::ROSTER, json!({})).await.unwrap_or(Json::Null);
        let user_ids: Vec<String> = backend::list(&found, "userIdList")
            .iter()
            .filter_map(Json::as_str)
            .map(str::to_string)
            .collect();
        let query = self.roster_query(&user_ids).await;
        self.send(iq_result(iq).c(query).build());
    }

    async fn roster_query(&mut self, user_ids: &[String]) -> Element {
        let names = self.usernames_of(user_ids).await;
        let mut query = E::new("query", ns::ROSTER);
        for user_id in user_ids {
            let Some(username) = names.get(user_id) else {
                continue;
            };
            query = query.c(E::new("item", ns::ROSTER)
                .a("jid", format!("{}@{}", jids::escape_node(username), self.domain()))
                .a("name", username.clone())
                .a("subscription", "both"));
        }
        query.build()
    }

    async fn vcard(&mut self, iq: &Element, to: &str) {
        if attr(iq, "type") == "set" {
            return self.refuse(
                iq,
                "cancel",
                "not-allowed",
                Some("the name comes from the Moqui account"),
            );
        }
        let username = match self.target(to).await {
            Target::User(user_id, _) => self.username_of(&user_id).await,
            _ => self.account.as_ref().map(|account| account.username.clone()),
        };
        let Some(username) = username else {
            return self.refuse(iq, "cancel", "item-not-found", None);
        };
        let card = E::new("vCard", ns::VCARD)
            .c(text_el("FN", ns::VCARD, username.clone()))
            .c(text_el("NICKNAME", ns::VCARD, username));
        self.send(iq_result(iq).c(card).build());
    }

    pub(crate) async fn is_blocked(&mut self, bare: &str) -> bool {
        if self.blocked.is_none() {
            let found = self.call(services::BLOCKS, json!({})).await.unwrap_or(Json::Null);
            let set = backend::list(&found, "jidList")
                .iter()
                .filter_map(Json::as_str)
                .map(str::to_string)
                .collect();
            self.blocked = Some(set);
        }
        self.blocked.as_ref().is_some_and(|blocked| blocked.contains(bare))
    }

    async fn blocking(&mut self, iq: &Element, payload: &Element) {
        let jids: Vec<String> = payload
            .children()
            .filter(|item| item.is("item", ns::BLOCKING))
            .filter_map(|item| jids::parse(&attr(item, "jid")).map(|jid| jid.bare()))
            .collect();
        match (attr(iq, "type").as_str(), payload.name()) {
            ("get", "blocklist") => {
                self.is_blocked("").await;
                let mut list = E::new("blocklist", ns::BLOCKING);
                for jid in self.blocked.clone().unwrap_or_default() {
                    list = list.c(E::new("item", ns::BLOCKING).a("jid", jid));
                }
                self.send(iq_result(iq).c(list).build());
            }
            ("set", "block") if !jids.is_empty() => {
                if self.call(services::BLOCK, json!({"jidList": jids})).await.is_err() {
                    return self.refuse(iq, "wait", "internal-server-error", None);
                }
                self.blocked = None;
                self.send(iq_result(iq).build());
            }
            ("set", "unblock") => {
                if self.call(services::UNBLOCK, json!({"jidList": jids})).await.is_err() {
                    return self.refuse(iq, "wait", "internal-server-error", None);
                }
                self.blocked = None;
                self.send(iq_result(iq).build());
            }
            _ => self.refuse(iq, "modify", "bad-request", None),
        }
    }

    /// XEP-0215: the embedded TURN server (and any external ones) with a credential for this user.
    async fn external_services(&mut self, iq: &Element) {
        let found = self.call(services::ICE_SERVERS, json!({})).await.unwrap_or(Json::Null);
        let mut services_el = E::new("services", ns::EXTDISCO);
        for server in backend::list(&found, "serviceList") {
            services_el = services_el.c(E::new("service", ns::EXTDISCO)
                .a("type", backend::text(server, "type"))
                .a("transport", backend::text(server, "transport"))
                .a("host", backend::text(server, "host"))
                .a("port", backend::text(server, "port"))
                .a("username", backend::text(server, "username"))
                .a("password", backend::text(server, "password"))
                .a("expires", backend::text(server, "expires"))
                .a(
                    "restricted",
                    if backend::text(server, "username").is_empty() {
                        ""
                    } else {
                        "true"
                    },
                ));
        }
        self.send(iq_result(iq).c(services_el).build());
    }

    async fn push_registration(&mut self, iq: &Element, payload: &Element) {
        let jid = attr(payload, "jid");
        let node = attr(payload, "node");
        let resource = self.resource.clone().unwrap_or_default();
        let result = if payload.name() == "enable" {
            if jid.is_empty() {
                return self.refuse(iq, "modify", "bad-request", None);
            }
            let options = payload
                .get_child("x", ns::DATA)
                .map(super::xml::form_fields)
                .unwrap_or_default()
                .into_iter()
                .filter(|(var, _)| var != "FORM_TYPE")
                .map(|(var, values)| (var, Json::String(values.join(" "))))
                .collect::<serde_json::Map<String, Json>>();
            self.call(
                services::PUSH_ENABLE,
                json!({"pushJid": jid, "node": node, "resource": resource, "options": options}),
            )
            .await
        } else {
            self.call(services::PUSH_DISABLE, json!({"pushJid": jid, "node": node}))
                .await
        };
        match result {
            Ok(_) => self.send(iq_result(iq).build()),
            Err(_) => self.refuse(iq, "cancel", "not-allowed", None),
        }
    }

    async fn upload_slot(&mut self, iq: &Element, request: &Element) {
        let size: u64 = attr(request, "size").parse().unwrap_or(0);
        if size == 0 {
            return self.refuse(iq, "modify", "bad-request", Some("a size is required"));
        }
        if size > self.server.config.upload_max_bytes {
            let max = self.server.config.upload_max_bytes.to_string();
            let error = super::xml::stanza_error(iq, "modify", "not-acceptable", Some("the file is too large"));
            let mut error = error;
            if let Some(inner) = error.get_child_mut("error", ns::CLIENT) {
                inner.append_child(
                    E::new("file-too-large", ns::UPLOAD)
                        .c(text_el("max-file-size", ns::UPLOAD, max))
                        .build(),
                );
            }
            return self.send(error);
        }
        let found = self
            .call(
                services::UPLOAD_SLOT,
                json!({"fileName": attr(request, "filename"), "fileSize": size,
                    "contentType": attr(request, "content-type")}),
            )
            .await;
        let Ok(found) = found else {
            return self.refuse(iq, "wait", "resource-constraint", None);
        };
        let url = format!(
            "{}/xmpp-upload/{}/{}",
            self.server.config.upload_base_url,
            backend::text(&found, "token"),
            backend::text(&found, "urlName")
        );
        let slot = E::new("slot", ns::UPLOAD)
            .c(E::new("put", ns::UPLOAD).a("url", url.clone()))
            .c(E::new("get", ns::UPLOAD).a("url", url));
        self.send(iq_result(iq).c(slot).build());
    }

    /// XEP-0410: an occupant's ping to itself answers whether it is still joined.
    fn self_ping(&mut self, iq: &Element, room: &str, nick: &str) {
        match self.joined.get(room) {
            Some(joined) if joined == nick => self.send(iq_result(iq).build()),
            _ => self.refuse(iq, "cancel", "not-acceptable", None),
        }
    }

    /// A capabilities hash (XEP-0115) this session has not resolved: ask the client what it means.
    pub(super) fn ask_caps(&mut self, node: &str, ver: &str) {
        // a few questions at a time: a client that keeps changing its hash is not asked each one
        if ver.is_empty() || self.caps_asked.len() >= 4 || self.caps_asked.values().any(|asked| asked == ver) {
            return;
        }
        let id = super::xml::random_id();
        self.caps_asked.insert(id.clone(), ver.to_string());
        let query = E::new("query", ns::DISCO_INFO).a("node", format!("{node}#{ver}"));
        let iq = E::new("iq", ns::CLIENT)
            .a("type", "get")
            .a("id", id)
            .a("from", self.my_bare())
            .a("to", self.my_full())
            .c(query)
            .build();
        self.send(iq);
    }

    fn caps_answered(&mut self, _ver: &str, iq: &Element) {
        let Some(query) = iq.get_child("query", ns::DISCO_INFO) else {
            return;
        };
        let wanted: std::collections::HashSet<String> = query
            .children()
            .filter(|feature| feature.is("feature", ns::DISCO_INFO))
            .filter_map(|feature| attr(feature, "var").strip_suffix("+notify").map(str::to_string))
            .collect();
        *moqui_lib::sync::lock(&self.notify) = wanted;
    }
}
