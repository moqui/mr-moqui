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

//! PEP (XEP-0163): each user's nodes, kept with their items (`XmppPepNode`, `XmppPepItem`, `XmppServices`).
//!
//! What clients publish here: avatars (XEP-0084), bookmarks (XEP-0402) and OMEMO device lists and bundles, in both the
//! legacy `eu.siacs.conversations.axolotl` namespace and `urn:xmpp:omemo:2`. Nodes are made on first publish, with
//! the publish-options the client gives (`pubsub#access_model`, `pubsub#max_items`, `pubsub#persist_items`); a later
//! publish whose options differ from the node's is refused `precondition-not-met`, and the owner can configure the
//! node (`pubsub#owner`) or delete it. Access: `open` is any user of this server, `presence` the people the owner
//! shares a room with, `whitelist` the owner alone. A publish notifies the sessions of the users who may read the node
//! and asked for it (`<node>+notify` in their capabilities, XEP-0115), the owner's own included.

use minidom::Element;
use serde_json::{json, Value as Json};

use super::xml::{attr, form, form_fields, iq_result, E};
use super::Session;
use crate::backend::{self, services};
use crate::ns;
use crate::registry::Inbound;

impl Session {
    pub(super) async fn pep(&mut self, iq: &Element, pubsub: &Element, owner: Option<String>) {
        let kind = attr(iq, "type");
        let Some(operation) = pubsub.children().next().cloned() else {
            return self.refuse(iq, "modify", "bad-request", None);
        };
        let node = attr(&operation, "node");
        if node.is_empty() && operation.name() != "publish-options" {
            return self.pubsub_error(iq, "modify", "bad-request", "nodeid-required");
        }
        let owner_ns = pubsub.ns() == ns::PUBSUB_OWNER;
        match (kind.as_str(), operation.name(), owner_ns) {
            ("get", "items", false) => self.pep_items(iq, &operation, &node, owner).await,
            ("set", "publish", false) => self.pep_publish(iq, pubsub, &operation, &node).await,
            ("set", "retract", false) => self.pep_retract(iq, &operation, &node).await,
            ("get", "configure", true) => self.pep_get_config(iq, &node).await,
            ("set", "configure", true) => {
                let options = publish_options(operation.get_child("x", ns::DATA));
                match self
                    .call(services::PEP_CONFIGURE, json!({"node": node, "options": options}))
                    .await
                {
                    Ok(_) => self.send(iq_result(iq).build()),
                    Err(_) => self.refuse(iq, "cancel", "item-not-found", None),
                }
            }
            ("set", "delete", true) => match self.call(services::PEP_DELETE, json!({"node": node})).await {
                Ok(_) => self.send(iq_result(iq).build()),
                Err(_) => self.refuse(iq, "cancel", "item-not-found", None),
            },
            _ => self.pubsub_error(iq, "cancel", "feature-not-implemented", "unsupported"),
        }
    }

    fn pubsub_error(&mut self, iq: &Element, kind: &str, condition: &str, specific: &str) {
        let mut error = super::xml::stanza_error(iq, kind, condition, None);
        if let Some(inner) = error.get_child_mut("error", ns::CLIENT) {
            inner.append_child(E::new(specific, ns::PUBSUB_ERRORS).build());
        }
        self.send(error);
    }

    async fn pep_items(&mut self, iq: &Element, operation: &Element, node: &str, owner: Option<String>) {
        let item_ids: Vec<String> = operation
            .children()
            .filter(|item| item.is("item", ns::PUBSUB))
            .map(|item| attr(item, "id"))
            .filter(|id| !id.is_empty())
            .collect();
        let max: Option<i64> = operation.attr("max_items").and_then(|max| max.parse().ok());
        let found = self
            .call(
                services::PEP_ITEMS,
                json!({"ownerUserId": owner, "node": node, "itemIdList": item_ids, "maxItems": max}),
            )
            .await;
        let found = match found {
            Ok(found) => found,
            Err(_) => return self.refuse(iq, "cancel", "item-not-found", None),
        };
        if backend::flag(&found, "denied") {
            return self.pubsub_error(iq, "auth", "forbidden", "presence-subscription-required");
        }
        if !backend::flag(&found, "exists") {
            return self.refuse(iq, "cancel", "item-not-found", None);
        }
        let mut items = E::new("items", ns::PUBSUB).a("node", node);
        for item in backend::list(&found, "itemList") {
            items = items.c(item_element(item, ns::PUBSUB));
        }
        self.send(iq_result(iq).c(E::new("pubsub", ns::PUBSUB).c(items)).build());
    }

    async fn pep_publish(&mut self, iq: &Element, pubsub: &Element, publish: &Element, node: &str) {
        let Some(item) = publish.get_child("item", ns::PUBSUB) else {
            return self.pubsub_error(iq, "modify", "bad-request", "item-required");
        };
        let Some(payload) = item.children().next() else {
            return self.pubsub_error(iq, "modify", "bad-request", "payload-required");
        };
        let item_id = Some(attr(item, "id"))
            .filter(|id| !id.is_empty())
            .unwrap_or_else(super::xml::random_id);
        let options = publish_options(
            pubsub
                .get_child("publish-options", ns::PUBSUB)
                .and_then(|options| options.get_child("x", ns::DATA)),
        );
        let found = self
            .call(
                services::PEP_PUBLISH,
                json!({"node": node, "itemId": item_id, "payloadXml": crate::framing::write(payload),
                    "options": options}),
            )
            .await;
        let found = match found {
            Ok(found) => found,
            Err(_) => return self.refuse(iq, "wait", "internal-server-error", None),
        };
        if backend::flag(&found, "preconditionFailed") {
            return self.pubsub_error(iq, "cancel", "conflict", "precondition-not-met");
        }
        let result = iq_result(iq)
            .c(
                E::new("pubsub", ns::PUBSUB).c(E::new("publish", ns::PUBSUB).a("node", node).c(E::new(
                    "item",
                    ns::PUBSUB,
                )
                .a("id", item_id.clone()))),
            )
            .build();
        self.send(result);
        let event_item = E::new("item", ns::PUBSUB_EVENT)
            .a("id", item_id)
            .c(payload.clone())
            .build();
        let items = E::new("items", ns::PUBSUB_EVENT).a("node", node).c(event_item).build();
        self.notify_pep(node, items, &found);
    }

    async fn pep_retract(&mut self, iq: &Element, retract: &Element, node: &str) {
        let Some(item_id) = retract.get_child("item", ns::PUBSUB).map(|item| attr(item, "id")) else {
            return self.pubsub_error(iq, "modify", "bad-request", "item-required");
        };
        let found = match self
            .call(services::PEP_RETRACT, json!({"node": node, "itemId": item_id}))
            .await
        {
            Ok(found) => found,
            Err(_) => return self.refuse(iq, "cancel", "item-not-found", None),
        };
        self.send(iq_result(iq).build());
        if matches!(attr(retract, "notify").as_str(), "true" | "1") {
            let items = E::new("items", ns::PUBSUB_EVENT)
                .a("node", node)
                .c(E::new("retract", ns::PUBSUB_EVENT).a("id", item_id))
                .build();
            self.notify_pep(node, items, &found);
        }
    }

    async fn pep_get_config(&mut self, iq: &Element, node: &str) {
        let found = match self
            .call(
                services::PEP_ITEMS,
                json!({"node": node, "itemIdList": [], "maxItems": 0}),
            )
            .await
        {
            Ok(found) if backend::flag(&found, "exists") => found,
            _ => return self.refuse(iq, "cancel", "item-not-found", None),
        };
        let access = backend::text(&found, "accessModel");
        let max = backend::text(&found, "nodeMaxItems");
        let config = form(
            "form",
            "http://jabber.org/protocol/pubsub#node_config",
            &[
                ("pubsub#access_model", access.as_str()),
                ("pubsub#max_items", max.as_str()),
                ("pubsub#persist_items", "true"),
            ],
        );
        let configure = E::new("configure", ns::PUBSUB_OWNER).a("node", node).c(config);
        self.send(iq_result(iq).c(E::new("pubsub", ns::PUBSUB_OWNER).c(configure)).build());
    }

    /// The event of a publish or retract to every session that may read the node and asked for it.
    fn notify_pep(&mut self, node: &str, items: Element, found: &Json) {
        let owner = self.my_bare();
        let mut users: Vec<String> = backend::list(found, "notifyUserIdList")
            .iter()
            .filter_map(Json::as_str)
            .map(str::to_string)
            .collect();
        let me = self.user_id();
        if !users.contains(&me) {
            users.push(me);
        }
        for user_id in users {
            for entry in self.server.registry.sessions_of(&user_id) {
                if !moqui_lib::sync::lock(&entry.notify).contains(node) {
                    continue;
                }
                let to = format!(
                    "{}@{}/{}",
                    crate::jids::escape_node(&entry.username),
                    self.domain(),
                    entry.resource
                );
                let message = E::new("message", ns::CLIENT)
                    .a("type", "headline")
                    .a("from", owner.clone())
                    .a("to", to.clone())
                    .a("id", super::xml::random_id())
                    .c(E::new("event", ns::PUBSUB_EVENT).c(items.clone()))
                    .build();
                if entry.id == self.entry_id {
                    self.send(message);
                } else {
                    self.server.registry.send(&entry, Inbound::Stanza(message));
                }
            }
        }
    }
}

/// The node options of a publish-options or configure form: `pubsub#` fields by their short name.
fn publish_options(form: Option<&Element>) -> Json {
    let mut options = serde_json::Map::new();
    for (var, values) in form.map(form_fields).unwrap_or_default() {
        if let Some(name) = var.strip_prefix("pubsub#") {
            options.insert(name.to_string(), json!(values.first().cloned().unwrap_or_default()));
        }
    }
    Json::Object(options)
}

/// A stored item as an element in `namespace`.
fn item_element(item: &Json, namespace: &str) -> Element {
    let mut built = E::new("item", namespace).a("id", backend::text(item, "itemId"));
    if let Ok(payload) = crate::framing::parse_element(&backend::text(item, "payloadXml")) {
        built = built.c(payload);
    }
    built.build()
}
