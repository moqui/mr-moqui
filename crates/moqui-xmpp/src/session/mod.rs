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

//! One client's stream (RFC 6120, RFC 6121; docs/interfaces/XMPP.md): no I/O of its own. [`crate::conn`] hands it the
//! frames it reads and the pushes for its user, and writes what it answers ([`Out`]).
//!
//! - `auth.rs`: STARTTLS, SASL and SASL2 with SCRAM, FAST and PLAIN; resource binding and Bind 2.
//! - `sm.rs`: stream management (XEP-0198), with resumption.
//! - `iq.rs`: discovery, ping, roster, blocking, carbons, push registration, upload slots, external services, vCard.
//! - `chat.rs`: messages and presence in and out, direct rooms and MUC.
//! - `mam.rs`: the archive (XEP-0313) over the room timelines.
//! - `pep.rs`: PEP (XEP-0163) for avatars, bookmarks and both OMEMO namespaces.

mod auth;
mod chat;
mod iq;
mod mam;
mod pep;
mod push;
mod sm;
pub mod xml;

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use minidom::Element;
use serde_json::{json, Value as Json};
use tokio::sync::{mpsc, Notify};

use crate::backend::{self, services, CallError, Caller};
use crate::framing::{self, Frame};
use crate::registry::{Entry, Inbound};
use crate::{jids, ns, Server};

use xml::{attr, stanza_error, stream_error, E};

/// What the connection writes or does next, in order.
#[derive(Debug)]
pub enum Out {
    /// Raw text: the stream header and footer over TCP; a whole frame over WebSocket.
    Raw(String),
    Element(Element),
    /// Upgrade the TCP connection to TLS once what came before is written.
    StartTls,
    /// Start a new XML document (after STARTTLS and after SASL).
    Reset,
    /// Close the stream and the connection.
    Close,
}

/// The transport a session runs over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// TCP; `tls` once STARTTLS ran or on the direct TLS port.
    Tcp { tls: bool },
    /// WebSocket (RFC 7395); `tls` when the HTTP request was HTTPS.
    WebSocket { tls: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Waiting for the stream header.
    Opening,
    /// Before authentication.
    Unauthenticated,
    /// Authenticated by RFC 6120 SASL; a resource is bound next.
    Authenticated,
    /// A full JID: stanzas flow.
    Bound,
    Closed,
}

/// The signed-in account.
#[derive(Debug, Clone)]
pub struct Account {
    pub user_id: String,
    pub username: String,
    pub display_name: String,
}

impl Account {
    pub(crate) fn from_json(value: &Json) -> Option<Account> {
        let user_id = backend::text(value, "userId");
        let username = backend::text(value, "username");
        if user_id.is_empty() || username.is_empty() {
            return None;
        }
        Some(Account {
            display_name: Some(backend::text(value, "displayName"))
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| username.clone()),
            user_id,
            username,
        })
    }
}

/// What the session knows of a room.
#[derive(Debug, Clone)]
pub(crate) struct RoomMeta {
    pub direct: bool,
    /// A direct room's other member.
    pub peer_user_id: Option<String>,
    pub name: String,
    pub topic: String,
    pub encrypted: bool,
    pub bridged: bool,
    pub public: bool,
    /// userId → (username, role)
    pub members: Vec<(String, String, String)>,
}

pub struct Session {
    pub(crate) server: Arc<Server>,
    pub(crate) peer: IpAddr,
    pub(crate) transport: Transport,
    /// The TLS session's exporter for channel binding (RFC 9266), when this connection has one of its own.
    pub(crate) binding: Option<Vec<u8>>,
    state: State,
    pub(crate) stream_id: String,
    pub(crate) auth: auth::AuthState,
    pub(crate) account: Option<Account>,
    pub(crate) resource: Option<String>,
    pub(crate) entry_id: u64,
    pub(crate) rx: Option<mpsc::Receiver<Inbound>>,
    pub(crate) tx: Option<mpsc::Sender<Inbound>>,
    pub(crate) overflow: Arc<Notify>,
    pub(crate) available: Arc<AtomicBool>,
    pub(crate) carbons: Arc<AtomicBool>,
    pub(crate) notify: Arc<Mutex<HashSet<String>>>,
    pub(crate) sm: Option<sm::Sm>,
    pub(crate) csi_inactive: bool,
    /// Presence and chat states held while the client is inactive (XEP-0352), by sender.
    pub(crate) held: Vec<Element>,
    /// The rooms this session joined as MUC rooms: chatRoomId → nick.
    pub(crate) joined: HashMap<String, String>,
    pub(crate) rooms: HashMap<String, RoomMeta>,
    /// userId → username, and back.
    pub(crate) usernames: HashMap<String, String>,
    pub(crate) user_ids: HashMap<String, String>,
    /// The clientRefs of the messages this session sent, so a 1:1 message is not sent back to it.
    pub(crate) sent_refs: VecDeque<String>,
    /// A client's message id that is not a clientRef, by the clientRef used for it.
    pub(crate) ids: HashMap<String, String>,
    pub(crate) blocked: Option<HashSet<String>>,
    /// Capabilities (XEP-0115) asked of the client: iq id → ver; and what a ver answered.
    pub(crate) caps_asked: HashMap<String, String>,
    pub(crate) last_beat: Option<Instant>,
    /// The session ended and was kept for resumption: its offline presence waits.
    pub(crate) detached: bool,
    /// The last push notification (XEP-0357), and how many messages waited for it.
    pub(crate) last_push: Option<Instant>,
    pub(crate) pushed_count: u64,
    out: Vec<Out>,
}

/// How often presence is refreshed (`heartbeat#UserStatus`) while the session is available.
const HEARTBEAT: Duration = Duration::from_secs(60);
/// How many sent clientRefs are remembered.
const SENT_REFS: usize = 200;

impl Session {
    pub fn new(server: Arc<Server>, peer: IpAddr, transport: Transport, binding: Option<Vec<u8>>) -> Session {
        let (available, carbons) = crate::registry::flags();
        Session {
            server,
            peer,
            transport,
            binding,
            state: State::Opening,
            stream_id: xml::random_id(),
            auth: auth::AuthState::default(),
            account: None,
            resource: None,
            entry_id: 0,
            rx: None,
            tx: None,
            overflow: Arc::new(Notify::new()),
            available,
            carbons,
            notify: Arc::new(Mutex::new(HashSet::new())),
            sm: None,
            csi_inactive: false,
            held: Vec::new(),
            joined: HashMap::new(),
            rooms: HashMap::new(),
            usernames: HashMap::new(),
            user_ids: HashMap::new(),
            sent_refs: VecDeque::new(),
            ids: HashMap::new(),
            blocked: None,
            caps_asked: HashMap::new(),
            last_beat: None,
            detached: false,
            last_push: None,
            pushed_count: 0,
            out: Vec::new(),
        }
    }

    /// What to write, in order, since the last call.
    pub fn take_out(&mut self) -> Vec<Out> {
        std::mem::take(&mut self.out)
    }

    pub fn is_closed(&self) -> bool {
        self.state == State::Closed
    }

    pub fn is_websocket(&self) -> bool {
        matches!(self.transport, Transport::WebSocket { .. })
    }

    pub(crate) fn secure(&self) -> bool {
        match self.transport {
            Transport::Tcp { tls } | Transport::WebSocket { tls } => tls,
        }
    }

    /// The connection finished STARTTLS: the new TLS session's exporter.
    pub fn tls_started(&mut self, binding: Option<Vec<u8>>) {
        self.transport = Transport::Tcp { tls: true };
        self.binding = binding;
        self.state = State::Opening;
    }

    /// The pushes and stanzas for this session, once it is bound.
    pub fn take_receiver(&mut self) -> Option<mpsc::Receiver<Inbound>> {
        self.rx.take()
    }

    pub fn give_receiver(&mut self, rx: mpsc::Receiver<Inbound>) {
        self.rx = Some(rx);
    }

    pub fn overflow(&self) -> Arc<Notify> {
        Arc::clone(&self.overflow)
    }

    pub(crate) fn domain(&self) -> &str {
        &self.server.config.domain
    }

    pub(crate) fn my_bare(&self) -> String {
        self.account
            .as_ref()
            .map(|account| format!("{}@{}", jids::escape_node(&account.username), self.domain()))
            .unwrap_or_default()
    }

    pub(crate) fn my_full(&self) -> String {
        match &self.resource {
            Some(resource) => format!("{}/{resource}", self.my_bare()),
            None => self.my_bare(),
        }
    }

    pub(crate) fn user_id(&self) -> String {
        self.account
            .as_ref()
            .map(|account| account.user_id.clone())
            .unwrap_or_default()
    }

    /// Send a stanza or nonza; a stanza counts for stream management.
    pub(crate) fn send(&mut self, element: Element) {
        let stanza = element.ns() == ns::CLIENT && matches!(element.name(), "message" | "presence" | "iq");
        if stanza {
            if let Some(sm) = &mut self.sm {
                sm.sent(element.clone());
            }
        }
        self.out.push(Out::Element(element));
        if stanza {
            if let Some(request) = self.sm.as_mut().and_then(sm::Sm::request_ack) {
                self.out.push(Out::Element(request));
            }
        }
    }

    /// A stanza the client may not need now (presence, chat states): held while it is inactive.
    pub(crate) fn send_deferrable(&mut self, element: Element) {
        if self.csi_inactive {
            if element.is("presence", ns::CLIENT) {
                let from = attr(&element, "from");
                self.held
                    .retain(|held| !(held.is("presence", ns::CLIENT) && attr(held, "from") == from));
                self.held.push(element);
            }
            // a chat state while inactive is stale by the time it would be read
            return;
        }
        self.send(element);
    }

    pub(crate) fn raw(&mut self, text: String) {
        self.out.push(Out::Raw(text));
    }

    /// End the stream with an error.
    pub(crate) fn fail(&mut self, condition: &str, text: Option<&str>) {
        if self.state == State::Opening {
            self.open_header(None);
        }
        self.out.push(Out::Element(stream_error(condition, text)));
        self.close();
    }

    pub(crate) fn close(&mut self) {
        if self.state == State::Closed {
            return;
        }
        let footer = if self.is_websocket() {
            framing::CLOSE_FRAME
        } else {
            framing::STREAM_FOOTER
        };
        self.out.push(Out::Raw(footer.to_string()));
        self.out.push(Out::Close);
        self.state = State::Closed;
    }

    fn open_header(&mut self, to: Option<&str>) {
        let domain = self.server.config.domain.clone();
        let header = if self.is_websocket() {
            framing::open_frame(&domain, &self.stream_id)
        } else {
            framing::stream_header(&domain, &self.stream_id, to)
        };
        self.raw(header);
    }

    /// A frame from the client.
    pub async fn on_frame(&mut self, frame: Frame) {
        match frame {
            Frame::Open(header) => self.on_open(&header),
            Frame::Close => {
                self.close();
            }
            Frame::Element(element) => self.on_element(element).await,
        }
    }

    fn on_open(&mut self, header: &std::collections::BTreeMap<String, String>) {
        if self.state != State::Opening {
            self.fail("invalid-xml", Some("a second stream header"));
            return;
        }
        let to = header.get("to").map(|to| to.to_lowercase());
        self.stream_id = xml::random_id();
        self.open_header(None);
        if to.as_deref().is_some_and(|to| to != self.domain()) {
            self.state = State::Unauthenticated;
            self.fail("host-unknown", None);
            return;
        }
        if crate::conn::blocked(&self.server, self.peer) {
            self.state = State::Unauthenticated;
            self.fail("policy-violation", Some("too many failed sign-ins from this address"));
            return;
        }
        self.state = if self.account.is_some() {
            State::Authenticated
        } else {
            State::Unauthenticated
        };
        let features = self.features();
        self.send(features);
    }

    fn features(&self) -> Element {
        let mut features = E::new("features", ns::STREAM);
        if self.account.is_some() {
            features = features
                .c(E::new("bind", ns::BIND))
                .c(E::new("session", ns::SESSION).c(E::new("optional", ns::SESSION)))
                .c(E::new("sm", ns::SM))
                .c(E::new("csi", ns::CSI));
        } else if matches!(self.transport, Transport::Tcp { tls: false }) {
            features = features.c(E::new("starttls", ns::TLS).c(E::new("required", ns::TLS)));
        } else {
            features = self.auth_features(features);
        }
        features.build()
    }

    async fn on_element(&mut self, element: Element) {
        if let Some(sm) = &mut self.sm {
            if element.ns() == ns::CLIENT && matches!(element.name(), "message" | "presence" | "iq") {
                sm.received();
            }
        }
        match self.state {
            State::Opening | State::Closed => {}
            State::Unauthenticated => self.pre_auth(element).await,
            State::Authenticated => self.pre_bind(element).await,
            State::Bound => self.bound(element).await,
        }
    }

    async fn bound(&mut self, element: Element) {
        if element.ns() == ns::SM {
            self.sm_nonza(&element);
            return;
        }
        if element.ns() == ns::CSI {
            self.csi(element.name() == "inactive");
            return;
        }
        if element.ns() != ns::CLIENT {
            self.fail("unsupported-stanza-type", None);
            return;
        }
        // a stanza's from is the full JID; the client may leave it out
        let from = attr(&element, "from");
        if !from.is_empty() && from != self.my_full() && from != self.my_bare() {
            self.fail("invalid-from", None);
            return;
        }
        let mut element = element;
        xml::set_attr(&mut element, "from", self.my_full());
        match element.name() {
            "iq" => self.on_iq(element).await,
            "message" => self.on_message(element).await,
            "presence" => self.on_presence(element).await,
            _ => self.fail("unsupported-stanza-type", None),
        }
    }

    pub(crate) fn csi(&mut self, inactive: bool) {
        self.csi_inactive = inactive;
        if !inactive {
            for held in std::mem::take(&mut self.held) {
                self.send(held);
            }
        }
    }

    /// A push or a stanza from outside the connection.
    pub async fn on_inbound(&mut self, inbound: Inbound) {
        if self.state != State::Bound {
            return;
        }
        match inbound {
            Inbound::Stanza(element) => self.on_routed(element).await,
            Inbound::Notice { topic, message } => self.on_notice(&topic, &message).await,
        }
    }

    /// The periodic tick: presence refreshed, so the user stays online while connected.
    pub async fn tick(&mut self) {
        if self.state != State::Bound || !self.available.load(Ordering::Relaxed) {
            return;
        }
        if self.last_beat.is_some_and(|at| at.elapsed() < HEARTBEAT) {
            return;
        }
        self.last_beat = Some(Instant::now());
        let active = !self.csi_inactive;
        let _ = self
            .call(
                services::HEARTBEAT,
                json!({"idleSeconds": if active { 0 } else { 120 }, "active": active}),
            )
            .await;
    }

    /// The connection is gone and the session is not kept: leave the registry, and go offline when no session of the
    /// user is left.
    pub async fn end(&mut self) {
        if self.entry_id == 0 {
            return;
        }
        self.server.registry.unregister(self.entry_id);
        self.entry_id = 0;
        let user_id = self.user_id();
        if self.available.load(Ordering::Relaxed)
            && !self.server.registry.has_user(&user_id)
            && !self.server.registry.has_detached_for(&user_id)
        {
            let _ = self.call(services::DISCONNECT, json!({"userId": user_id})).await;
        }
    }

    /// Whether the session can wait for resumption.
    pub fn resumable(&self) -> Option<String> {
        self.sm.as_ref().filter(|sm| sm.resumable).map(|sm| sm.id.clone())
    }

    pub(crate) fn remember_ref(&mut self, client_ref: &str) {
        self.sent_refs.push_back(client_ref.to_string());
        if self.sent_refs.len() > SENT_REFS {
            self.sent_refs.pop_front();
        }
    }

    /// Call a service as the signed-in user.
    pub(crate) async fn call(&self, service: &str, parameters: Json) -> Result<Json, CallError> {
        let Some(account) = &self.account else {
            return Err(CallError("not signed in".to_string()));
        };
        let result = self
            .server
            .backend
            .call(Caller::User(account.username.clone()), service, parameters)
            .await;
        if let Err(err) = &result {
            tracing::debug!("XMPP {}: {service} refused: {err}", account.username);
        }
        result
    }

    pub(crate) async fn call_internal(&self, service: &str, parameters: Json) -> Result<Json, CallError> {
        self.server.backend.call(Caller::Internal, service, parameters).await
    }

    /// The registry entry of this session, made when it binds.
    pub(crate) fn register(&mut self) {
        let Some(account) = self.account.clone() else {
            return;
        };
        let (tx, rx) = mpsc::channel(crate::registry::QUEUE);
        self.entry_id = self.server.registry.next_id();
        self.server.registry.register(Entry {
            id: self.entry_id,
            user_id: account.user_id,
            username: account.username,
            resource: self.resource.clone().unwrap_or_default(),
            tx: tx.clone(),
            overflow: Arc::clone(&self.overflow),
            notify: Arc::clone(&self.notify),
            available: Arc::clone(&self.available),
            carbons: Arc::clone(&self.carbons),
        });
        self.tx = Some(tx);
        self.rx = Some(rx);
        self.state = State::Bound;
    }

    /// A stanza error back to the client.
    pub(crate) fn refuse(&mut self, stanza: &Element, kind: &str, condition: &str, text: Option<&str>) {
        if attr(stanza, "type") == "error" {
            return;
        }
        let error = stanza_error(stanza, kind, condition, text);
        self.send(error);
    }

    /// The usernames of user ids, from the cache or the backend.
    pub(crate) async fn usernames_of(&mut self, user_ids: &[String]) -> HashMap<String, String> {
        let missing: Vec<&String> = user_ids.iter().filter(|id| !self.usernames.contains_key(*id)).collect();
        if !missing.is_empty() {
            if let Ok(found) = self.call(services::USERS, json!({"userIdList": missing})).await {
                for user in backend::list(&found, "userList") {
                    let (id, name) = (backend::text(user, "userId"), backend::text(user, "username"));
                    if !id.is_empty() && !name.is_empty() {
                        self.user_ids.insert(name.to_lowercase(), id.clone());
                        self.usernames.insert(id, name);
                    }
                }
            }
        }
        user_ids
            .iter()
            .filter_map(|id| self.usernames.get(id).map(|name| (id.clone(), name.clone())))
            .collect()
    }

    pub(crate) async fn username_of(&mut self, user_id: &str) -> Option<String> {
        self.usernames_of(&[user_id.to_string()]).await.remove(user_id)
    }

    /// The userId of a localpart, from the cache or the backend.
    pub(crate) async fn user_id_of(&mut self, node: &str) -> Option<String> {
        let username = jids::unescape_node(node).to_lowercase();
        if let Some(found) = self.user_ids.get(&username) {
            return Some(found.clone());
        }
        let found = self
            .call(services::USERS, json!({"usernameList": [username]}))
            .await
            .ok()?;
        let user = backend::list(&found, "userList").first()?;
        let (id, name) = (backend::text(user, "userId"), backend::text(user, "username"));
        if id.is_empty() {
            return None;
        }
        self.user_ids.insert(name.to_lowercase(), id.clone());
        self.usernames.insert(id.clone(), name);
        Some(id)
    }

    /// The bare JID of a user id.
    pub(crate) async fn jid_of(&mut self, user_id: &str) -> Option<String> {
        let username = self.username_of(user_id).await?;
        Some(format!("{}@{}", jids::escape_node(&username), self.domain()))
    }

    pub(crate) fn room_jid(&self, chat_room_id: &str) -> String {
        format!("{}@{}", jids::room_node(chat_room_id), self.server.config.rooms_domain)
    }

    /// A room's facts, from the cache or the backend; None when the user may not read it.
    pub(crate) async fn room(&mut self, chat_room_id: &str, fresh: bool) -> Option<RoomMeta> {
        if !fresh {
            if let Some(found) = self.rooms.get(chat_room_id) {
                return Some(found.clone());
            }
        }
        let found = self
            .call(services::ROOM, json!({"chatRoomId": chat_room_id}))
            .await
            .ok()?;
        let room = found.get("room")?;
        let members: Vec<(String, String, String)> = backend::list(&found, "memberList")
            .iter()
            .map(|member| {
                (
                    backend::text(member, "userId"),
                    backend::text(member, "username"),
                    backend::text(member, "roleEnumId"),
                )
            })
            .filter(|(id, name, _)| !id.is_empty() && !name.is_empty())
            .collect();
        for (id, name, _) in &members {
            self.usernames.insert(id.clone(), name.clone());
            self.user_ids.insert(name.to_lowercase(), id.clone());
        }
        let me = self.user_id();
        let meta = RoomMeta {
            direct: backend::text(room, "roomTypeEnumId") == "CrtDirect",
            peer_user_id: members.iter().map(|(id, _, _)| id.clone()).find(|id| *id != me),
            name: backend::text(room, "roomName"),
            topic: backend::text(room, "topic"),
            encrypted: backend::flag(room, "encrypted"),
            bridged: backend::flag(&found, "bridged"),
            public: backend::text(room, "visibilityEnumId") == "CrvUsers",
            members,
        };
        self.rooms.insert(chat_room_id.to_string(), meta.clone());
        Some(meta)
    }
}

#[cfg(test)]
mod tests;
