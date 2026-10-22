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

//! Inbound IMAP and POP3 for the email services, the `imap` feature: `poll#EmailServer` ([`poll_server`]), the
//! store half of `test#EmailServer`, the connections the mail client's sync uses, and inbound MIME.

#[cfg(test)]
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use indexmap::IndexMap;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{basic_convert, to_plain_string, Value};

use super::implementation::{
    extra_ca_certificates, fail, find_one, force_new, note_error, parse_port, require_host, SERVER,
};
use crate::context::{Error, ExecutionContext};
use crate::entity::{EntityFacade, EntityValue};
use crate::service::util::{field_text, has_error, param_text};

#[cfg(test)]
thread_local! {
    static FAKE: RefCell<Option<Vec<Inbound>>> = const { RefCell::new(None) };
    /// The ids the fake store was asked to delete.
    pub(super) static FAKE_DELETED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

pub(super) fn poll_server(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if facade.sim_active() {
        return fail("Email poll is disabled in LLM sim session");
    }
    crate::entity::fence::check_effect(crate::entity::fence::Effect::Mail, "poll#EmailServer")?;
    let server_id = param_text(parameters, "emailServerId");
    let Some(server) = find_one(facade, SERVER, "emailServerId", &server_id)? else {
        return fail(format!("No EmailServer found for ID [{server_id}]"));
    };
    let host = field_text(&server, "storeHost");
    let user = field_text(&server, "mailUsername");
    let password = field_text(&server, "mailPassword");
    if host.is_empty() {
        note_error(format!("EmailServer [{server_id}] has no storeHost"));
    }
    if user.is_empty() {
        note_error(format!("EmailServer [{server_id}] has no mailUsername"));
    }
    if password.is_empty() {
        note_error(format!("EmailServer [{server_id}] has no mailPassword"));
    }
    if has_error() {
        return Ok(BTreeMap::new());
    }
    if let Err(text) = require_host(&host) {
        return fail(text);
    }
    let protocol = {
        let given = field_text(&server, "storeProtocol");
        if given.is_empty() {
            "imaps".to_string()
        } else {
            given
        }
    };
    let port = match parse_port(&field_text(&server, "storePort"), 993) {
        Ok(port) => port,
        Err(text) => return fail(text),
    };
    let folder = {
        let given = field_text(&server, "storeFolder");
        if given.is_empty() {
            "INBOX".to_string()
        } else {
            given
        }
    };
    let settings = StoreSettings {
        host,
        port,
        user,
        password,
        protocol,
        folder,
        skip_seen: field_text(&server, "storeSkipSeen") == "Y",
        mark_seen: field_text(&server, "storeMarkSeen") == "Y",
        delete: field_text(&server, "storeDelete") == "Y",
    };
    let mut store = match open_store(&settings) {
        Ok(store) => store,
        Err(text) => return fail(text),
    };
    let ids = match store.list_ids() {
        Ok(ids) => ids,
        Err(text) => return fail(text),
    };
    // One message at a time, as Java's poll: fetched, run through the rules, then marked or deleted. Each message's
    // rules run in their own transaction, and the message is marked or deleted only once that has committed, so a later
    // failure or the poll's timeout cannot roll back a message already gone from the server. A message whose rules fail
    // is rolled back and left as it is, so the next poll sees it again. The errors are reported once all are done, so
    // one message's errors do not stop the services the next one's rules call.
    let mut errors = Vec::new();
    for id in ids {
        let message = match store.fetch(&id) {
            Ok(message) => message,
            Err(text) => {
                errors.push(format!("Error reading message {id} from {}: {text}", settings.host));
                break;
            }
        };
        if settings.skip_seen && message.seen {
            continue;
        }
        if let Err(text) = receive(facade, &message, &server_id) {
            tracing::error!("email ECA rules failed for message {id} of {server_id}, left on the server: {text}");
            errors.push(text.to_string());
            continue;
        }
        if settings.mark_seen {
            if let Err(err) = store.mark_seen(&id) {
                errors.push(err.to_string());
            }
        }
        if settings.delete {
            if let Err(err) = store.delete(&id) {
                errors.push(err.to_string());
            }
        }
    }
    if let Err(err) = store.close() {
        errors.push(err.to_string());
    }
    for text in errors {
        note_error(text);
    }
    Ok(BTreeMap::new())
}

/// Runs the email ECA rules for one message in a new transaction. A rule that fails, or an error a rule leaves in the
/// message facade, rolls it back; the errors are taken out of the facade and returned.
fn receive(facade: &EntityFacade, message: &Inbound, server_id: &str) -> Result<(), Error> {
    let fields = eca_fields(message, server_id);
    in_eca_transaction(facade, || crate::service::run_email_rules(&fields))
}

/// A message the mail client's sync stored ([`run_stored_ecas`]).
pub(super) struct StoredMessage<'a> {
    /// The message as it came from the server.
    pub(super) raw: &'a [u8],
    pub(super) server_id: &'a str,
    pub(super) account_id: &'a str,
    /// The `EmailMessage` row it was stored as.
    pub(super) email_message_id: &'a str,
    /// When the server received it (IMAP INTERNALDATE); none for POP3.
    pub(super) received_millis: Option<i64>,
    pub(super) seen: bool,
    pub(super) answered: bool,
    pub(super) flagged: bool,
}

/// The email-ECA rules (`*.emecas.xml`) on a message the mail client's sync brought in (`sync#EmailAccount`, IMAP or
/// POP3; any later fetch path calls this the same way), as poll#EmailServer runs them on a polled message: the same
/// `fields`, `headers`, `bodyPartList`, `flags` and `message` from its text, with `emailAccountId` and `emailMessageId`
/// of the row it was stored as. The message is kept either way, so each rule runs in a transaction of its own: one that
/// fails is rolled back and logged with the account and the message, and the other rules and the sync go on. Nothing
/// runs when no rule is installed.
pub(super) fn run_stored_ecas(facade: &EntityFacade, stored: &StoredMessage<'_>) {
    let count = crate::service::email_rule_count();
    if count == 0 {
        return;
    }
    let mut message = parse_rfc822(stored.raw);
    message.seen = stored.seen;
    message.flags.answered = stored.answered;
    message.flags.flagged = stored.flagged;
    if let Some(millis) = stored.received_millis {
        message.received = stamp_millis(millis);
    }
    let mut fields = eca_fields(&message, stored.server_id);
    fields.insert("emailAccountId".into(), Value::string(stored.account_id));
    fields.insert("emailMessageId".into(), Value::string(stored.email_message_id));
    for index in 0..count {
        if let Err(err) = in_eca_transaction(facade, || crate::service::run_email_rule(index, &fields)) {
            tracing::error!(
                account = %stored.account_id,
                "Email ECA rule {} failed for email message {} of account {}, skipped: {err}",
                index + 1,
                stored.email_message_id,
                stored.account_id
            );
        }
    }
}

/// `run` (email ECA rules) in a new transaction. A rule that fails, or an error a rule leaves in the message facade,
/// rolls it back; the errors are taken out of the facade and returned.
fn in_eca_transaction(facade: &EntityFacade, run: impl FnOnce() -> Result<(), Error>) -> Result<(), Error> {
    let mark = ExecutionContext::with_current(|ctx| ctx.message().error_mark()).unwrap_or_default();
    force_new(facade, || {
        let ran = run();
        let mut errors =
            ExecutionContext::with_current(|ctx| ctx.message().take_errors_since(mark)).unwrap_or_default();
        if let Err(err) = ran {
            if !errors.iter().any(|text| text == err.message()) {
                errors.insert(0, err.message().to_string());
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(LibError::new(errors.join("\n")))
        }
    })
    .map_err(|err| Error::new(err.message()))
}

pub(super) fn store_steps(
    facade: &EntityFacade,
    server: &EntityValue,
    steps: &mut Vec<(&'static str, bool, String)>,
    folders: &mut Vec<Value>,
) {
    let host = field_text(server, "storeHost");
    let oauth = field_text(server, "authTypeEnumId") == super::oauth::OAUTH;
    let protocol = {
        let given = field_text(server, "storeProtocol");
        if given.is_empty() {
            "imaps".to_string()
        } else {
            given
        }
    };
    let pop = matches!(protocol.as_str(), "pop3" | "pop3s");
    let port = match parse_port(&field_text(server, "storePort"), if pop { 995 } else { 993 }) {
        Ok(port) => port,
        Err(err) => {
            steps.push(("storeLogin", false, err.to_string()));
            return;
        }
    };
    let place = format!("{host}:{port}");
    if let Err(err) = require_host(&host) {
        steps.push(("storeLogin", false, err.to_string()));
        return;
    }
    let settings = StoreSettings {
        host: host.clone(),
        port,
        user: field_text(server, "mailUsername"),
        password: field_text(server, "mailPassword"),
        protocol: protocol.clone(),
        folder: "INBOX".to_string(),
        skip_seen: false,
        mark_seen: false,
        delete: false,
    };
    // the connection the sync makes: TLS from the start on imaps or port 993, else STARTTLS when the server offers it
    let info = super::sync::ServerInfo {
        server_id: field_text(server, "emailServerId"),
        host: host.clone(),
        port,
        protocol: protocol.clone(),
        user: settings.user.clone(),
        password: settings.password.clone(),
        auth_type: field_text(server, "authTypeEnumId"),
        store_delete: false,
        access_token: None,
    };
    if pop {
        // as the sync signs in: USER and PASS, or AUTH XOAUTH2 for a provider sign-in
        let how = if oauth { " with XOAUTH2" } else { "" };
        match super::sync::open_pop(facade, &info, &settings).and_then(|mut pop3| {
            pop3.close()
                .map_err(|err| super::sync::SyncFailure::Connect(err.to_string()))
        }) {
            Ok(()) => steps.push(("storeLogin", true, format!("Signed in to POP3 {place}{how}"))),
            Err(err) => {
                let text = err.to_string();
                let text = if text.starts_with("POP3 ") {
                    text
                } else {
                    format!("POP3 {place}: {text}")
                };
                steps.push(("storeLogin", false, text));
            }
        }
        return;
    }
    if !matches!(protocol.as_str(), "imap" | "imaps") {
        steps.push((
            "storeLogin",
            false,
            format!("storeProtocol {protocol} is not supported"),
        ));
        return;
    }
    let mut session = match super::sync::connect_imap(facade, &info) {
        Ok(conn) => {
            steps.push(("storeLogin", true, format!("Signed in to IMAP {place}")));
            conn.session
        }
        Err(err) => {
            steps.push(("storeLogin", false, format!("IMAP {place}: {err}")));
            return;
        }
    };
    match session.list(Some(""), Some("*")) {
        Ok(names) => {
            for name in names.iter() {
                let attributes: Vec<String> = name
                    .attributes()
                    .iter()
                    .map(|attribute| name_attribute(&format!("{attribute:?}")))
                    .collect();
                let kind = folder_kind(name.name(), &attributes);
                let mut map = IndexMap::new();
                map.insert("folderPath".to_string(), Value::string(name.name()));
                map.insert(
                    "delimiter".to_string(),
                    name.delimiter().map_or(Value::Null, Value::string),
                );
                map.insert(
                    "attributeList".to_string(),
                    Value::List(attributes.into_iter().map(Value::String).collect()),
                );
                map.insert("folderKindEnumId".to_string(), Value::string(kind));
                folders.push(Value::Map(map));
            }
            steps.push(("storeList", true, format!("Found {} folders", folders.len())));
        }
        Err(err) => steps.push(("storeList", false, format!("IMAP LIST on {place}: {err}"))),
    }
    let _ = session.logout();
}

/// An IMAP LIST attribute as the server wrote it, such as `\\Sent`, from its `Debug` text: the imap crate does not
/// export the attribute type.
pub(super) fn name_attribute(debug: &str) -> String {
    if let Some(inner) = debug.strip_prefix("Extension(").and_then(|rest| rest.strip_suffix(')')) {
        return inner.trim_matches('"').replace("\\\\", "\\");
    }
    match debug {
        "NoInferiors" => "\\Noinferiors".to_string(),
        "NoSelect" => "\\Noselect".to_string(),
        other => format!("\\{other}"),
    }
}

/// The `EmailFolderKind` a folder maps to: INBOX, then its special-use attribute (RFC 6154), else EfkOther.
pub(super) fn folder_kind(path: &str, attributes: &[String]) -> &'static str {
    if path.eq_ignore_ascii_case("INBOX") {
        return "EfkInbox";
    }
    for attribute in attributes {
        match attribute.to_ascii_lowercase().as_str() {
            "\\sent" => return "EfkSent",
            "\\drafts" => return "EfkDrafts",
            "\\trash" => return "EfkTrash",
            "\\junk" => return "EfkJunk",
            "\\archive" => return "EfkArchive",
            _ => {}
        }
    }
    "EfkOther"
}

pub(super) struct StoreSettings {
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) user: String,
    pub(super) password: String,
    pub(super) protocol: String,
    pub(super) folder: String,
    pub(super) skip_seen: bool,
    pub(super) mark_seen: bool,
    pub(super) delete: bool,
}

fn eca_fields(message: &Inbound, server_id: &str) -> BTreeMap<String, Value> {
    let mut fields = IndexMap::new();
    fields.insert("toList".into(), string_list(&message.to));
    fields.insert("ccList".into(), string_list(&message.cc));
    fields.insert("bccList".into(), string_list(&message.bcc));
    fields.insert(
        "from".into(),
        if message.from.is_empty() {
            Value::Null
        } else {
            Value::string(&message.from)
        },
    );
    fields.insert("subject".into(), Value::string(&message.subject));
    fields.insert("sentDate".into(), message.sent.clone());
    fields.insert("receivedDate".into(), message.received.clone());
    let mut flags = IndexMap::new();
    flags.insert("answered".into(), Value::Bool(message.flags.answered));
    flags.insert("deleted".into(), Value::Bool(message.flags.deleted));
    flags.insert("draft".into(), Value::Bool(message.flags.draft));
    flags.insert("flagged".into(), Value::Bool(message.flags.flagged));
    flags.insert("recent".into(), Value::Bool(message.flags.recent));
    flags.insert("seen".into(), Value::Bool(message.seen));
    // Java puts the MimeMessage here; this is the part of it the rules read: its id, size and raw text.
    let mut raw = IndexMap::new();
    raw.insert(
        "messageId".into(),
        Value::string(header_one(&message.headers, "message-id")),
    );
    raw.insert("subject".into(), Value::string(&message.subject));
    raw.insert(
        "size".into(),
        Value::I64(i64::try_from(message.raw.len()).unwrap_or(i64::MAX)),
    );
    raw.insert("raw".into(), Value::string(String::from_utf8_lossy(&message.raw)));
    let mut out = BTreeMap::new();
    out.insert("emailServerId".into(), Value::string(server_id));
    out.insert("message".into(), Value::Map(raw));
    out.insert("fields".into(), Value::Map(fields));
    out.insert("headers".into(), Value::Map(message.headers.clone()));
    out.insert("bodyPartList".into(), Value::List(message.parts.clone()));
    out.insert("flags".into(), Value::Map(flags));
    out
}

fn string_list(values: &[String]) -> Value {
    Value::List(values.iter().cloned().map(Value::string).collect())
}

/// IMAP flags of a message other than seen.
#[derive(Clone, Copy, Debug, Default)]
struct Flags {
    answered: bool,
    deleted: bool,
    draft: bool,
    flagged: bool,
    recent: bool,
}

#[derive(Clone, Debug)]
pub(super) struct Inbound {
    pub(super) id: String,
    pub(super) seen: bool,
    flags: Flags,
    /// The message as it came from the server.
    pub(super) raw: Vec<u8>,
    pub(super) headers: IndexMap<String, Value>,
    pub(super) parts: Vec<Value>,
    pub(super) subject: String,
    pub(super) from: String,
    pub(super) to: Vec<String>,
    pub(super) cc: Vec<String>,
    pub(super) bcc: Vec<String>,
    pub(super) sent: Value,
    pub(super) received: Value,
}

impl Inbound {
    pub(super) fn blank() -> Self {
        Self {
            id: String::new(),
            seen: false,
            flags: Flags::default(),
            raw: Vec::new(),
            headers: IndexMap::new(),
            parts: Vec::new(),
            subject: String::new(),
            from: String::new(),
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            sent: Value::Null,
            received: Value::Null,
        }
    }
}

trait Mailbox {
    /// The ids of the messages to look at, in server order.
    fn list_ids(&mut self) -> Result<Vec<String>, Error>;
    /// One message, with its flags.
    fn fetch(&mut self, id: &str) -> Result<Inbound, Error>;
    fn mark_seen(&mut self, id: &str) -> Result<(), Error>;
    fn delete(&mut self, id: &str) -> Result<(), Error>;
    fn close(&mut self) -> Result<(), Error>;
}

#[cfg(test)]
struct FakeBox {
    messages: Vec<Inbound>,
}

#[cfg(test)]
impl Mailbox for FakeBox {
    fn list_ids(&mut self) -> Result<Vec<String>, Error> {
        Ok(self.messages.iter().map(|message| message.id.clone()).collect())
    }

    fn fetch(&mut self, id: &str) -> Result<Inbound, Error> {
        self.messages
            .iter()
            .find(|message| message.id == id)
            .cloned()
            .ok_or_else(|| Error::new(format!("no message {id}")))
    }

    fn mark_seen(&mut self, id: &str) -> Result<(), Error> {
        if let Some(message) = self.messages.iter_mut().find(|message| message.id == id) {
            message.seen = true;
        }
        Ok(())
    }

    fn delete(&mut self, id: &str) -> Result<(), Error> {
        self.messages.retain(|message| message.id != id);
        FAKE_DELETED.with(|deleted| deleted.borrow_mut().push(id.to_string()));
        Ok(())
    }

    fn close(&mut self) -> Result<(), Error> {
        Ok(())
    }
}

#[cfg(test)]
pub(super) struct FakeGuard {
    previous: Option<Vec<Inbound>>,
}

#[cfg(test)]
impl FakeGuard {
    pub(super) fn install(messages: Vec<Inbound>) -> Self {
        let previous = FAKE.with(|slot| slot.borrow_mut().replace(messages));
        Self { previous }
    }
}

#[cfg(test)]
impl Drop for FakeGuard {
    fn drop(&mut self) {
        FAKE.with(|slot| *slot.borrow_mut() = self.previous.take());
    }
}

/// The fake store a test installed on this thread, if any.
#[cfg(test)]
fn fake_store() -> Option<Box<dyn Mailbox>> {
    let messages = FAKE.with(|slot| slot.borrow().clone())?;
    Some(Box::new(FakeBox { messages }))
}

fn open_store(settings: &StoreSettings) -> Result<Box<dyn Mailbox>, Error> {
    #[cfg(test)]
    if let Some(fake) = fake_store() {
        return Ok(fake);
    }
    match settings.protocol.as_str() {
        "pop3" | "pop3s" => Ok(Box::new(Pop3::open(settings)?)),
        "imap" | "imaps" => Ok(Box::new(ImapBox::open(settings)?)),
        other => Err(Error::new(format!("storeProtocol {other} is not supported"))),
    }
}

pub(super) fn use_tls(protocol: &str, port: u16) -> bool {
    matches!(protocol, "imaps" | "pop3s") || port == 993 || port == 995
}

struct ImapBox {
    session: imap::Session<Net>,
    closed: bool,
    deleted: bool,
    skip_seen: bool,
}

impl ImapBox {
    fn open(settings: &StoreSettings) -> Result<Self, Error> {
        let net = connect_net(
            &settings.host,
            settings.port,
            use_tls(&settings.protocol, settings.port),
        )?;
        let mut client = imap::Client::new(net);
        client.read_greeting().map_err(|err| Error::new(err.to_string()))?;
        let mut session = client
            .login(&settings.user, &settings.password)
            .map_err(|(err, _)| Error::new(err.to_string()))?;
        session
            .select(&settings.folder)
            .map_err(|_| Error::new(format!("No {} folder found", settings.folder)))?;
        Ok(Self {
            session,
            closed: false,
            deleted: false,
            skip_seen: settings.skip_seen,
        })
    }
}

impl Mailbox for ImapBox {
    fn list_ids(&mut self) -> Result<Vec<String>, Error> {
        let query = if self.skip_seen {
            "UNSEEN NOT DELETED"
        } else {
            "NOT DELETED"
        };
        let mut ids: Vec<u32> = self
            .session
            .search(query)
            .map_err(|err| Error::new(err.to_string()))?
            .into_iter()
            .collect();
        ids.sort_unstable();
        Ok(ids.into_iter().map(|id| id.to_string()).collect())
    }

    fn fetch(&mut self, id: &str) -> Result<Inbound, Error> {
        let fetches = self
            .session
            .fetch(id, "BODY.PEEK[] FLAGS INTERNALDATE")
            .map_err(|err| Error::new(err.to_string()))?;
        let fetch = fetches
            .iter()
            .next()
            .ok_or_else(|| Error::new(format!("message {id} is not on the server")))?;
        let mut message = parse_rfc822(fetch.body().unwrap_or_default());
        message.id = id.to_string();
        for flag in fetch.flags() {
            match flag {
                imap::types::Flag::Seen => message.seen = true,
                imap::types::Flag::Answered => message.flags.answered = true,
                imap::types::Flag::Deleted => message.flags.deleted = true,
                imap::types::Flag::Draft => message.flags.draft = true,
                imap::types::Flag::Flagged => message.flags.flagged = true,
                imap::types::Flag::Recent => message.flags.recent = true,
                _ => {}
            }
        }
        if let Some(date) = fetch.internal_date() {
            message.received = stamp_millis(date.timestamp_millis());
        }
        Ok(message)
    }

    fn mark_seen(&mut self, id: &str) -> Result<(), Error> {
        self.session
            .store(id, "+FLAGS (\\Seen)")
            .map(|_| ())
            .map_err(|err| Error::new(err.to_string()))
    }

    fn delete(&mut self, id: &str) -> Result<(), Error> {
        self.session
            .store(id, "+FLAGS (\\Deleted)")
            .map(|_| ())
            .map_err(|err| Error::new(err.to_string()))?;
        self.deleted = true;
        Ok(())
    }

    fn close(&mut self) -> Result<(), Error> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        // Only when this poll flagged a message: Java's close(true) also expunges what another client flagged and
        // could still undelete, on a server that is not set to delete.
        if self.deleted {
            self.session.expunge().map_err(|err| Error::new(err.to_string()))?;
        }
        self.session.logout().map_err(|err| Error::new(err.to_string()))
    }
}

impl Drop for ImapBox {
    fn drop(&mut self) {
        if !self.closed {
            if self.deleted {
                let _ = self.session.expunge();
            }
            let _ = self.session.logout();
            self.closed = true;
        }
    }
}

pub(super) struct Pop3 {
    reader: BufReader<Net>,
    closed: bool,
}

impl Pop3 {
    /// `UIDL`: each message's number and unique id, in server order.
    pub(super) fn uidl(&mut self) -> Result<Vec<(u32, String)>, Error> {
        self.command("UIDL")?;
        let listing = self.read_dot()?;
        Ok(String::from_utf8_lossy(&listing)
            .lines()
            .filter_map(|line| {
                let mut words = line.split_whitespace();
                let number = words.next()?.parse::<u32>().ok()?;
                let uidl = words.next()?.to_string();
                Some((number, uidl))
            })
            .collect())
    }

    /// `RETR`: one message as it is on the server.
    pub(super) fn retrieve(&mut self, number: u32) -> Result<Vec<u8>, Error> {
        self.command(&format!("RETR {number}"))?;
        self.read_dot()
    }

    /// `DELE`: the server removes the message at `QUIT`.
    pub(super) fn delete_number(&mut self, number: u32) -> Result<(), Error> {
        self.command(&format!("DELE {number}")).map(|_| ())
    }

    /// `QUIT`, when the server removes what was deleted.
    pub(super) fn quit(&mut self) -> Result<(), Error> {
        Mailbox::close(self)
    }

    /// Connect and sign in with `USER` and `PASS`.
    pub(super) fn open(settings: &StoreSettings) -> Result<Self, Error> {
        Self::open_as(settings, None)
    }

    /// Connect and sign in with SASL XOAUTH2 and an access token (`AUTH XOAUTH2`, RFC 5034), as Microsoft 365,
    /// Outlook.com and Gmail take it for POP3.
    pub(super) fn open_xoauth2(settings: &StoreSettings, token: &str) -> Result<Self, Error> {
        Self::open_as(settings, Some(token))
    }

    fn open_as(settings: &StoreSettings, token: Option<&str>) -> Result<Self, Error> {
        if settings.folder != "INBOX" {
            return Err(Error::new(format!("No {} folder found", settings.folder)));
        }
        // They go into the USER and PASS lines (or the XOAUTH2 response) as they are.
        if [
            settings.user.as_str(),
            settings.password.as_str(),
            token.unwrap_or_default(),
        ]
        .iter()
        .any(|text| text.contains(['\r', '\n']))
        {
            return Err(Error::new("the POP3 username or password has a line break"));
        }
        let net = connect_net(
            &settings.host,
            settings.port,
            use_tls(&settings.protocol, settings.port),
        )?;
        let mut reader = BufReader::new(net);
        let greeting = read_line(&mut reader)?;
        expect_ok(&greeting)?;
        let mut client = Self { reader, closed: false };
        match token {
            Some(token) => client.authenticate_xoauth2(&settings.user, token)?,
            None => {
                client.command(&format!("USER {}", settings.user))?;
                client.command(&format!("PASS {}", settings.password))?;
            }
        }
        Ok(client)
    }

    /// `AUTH XOAUTH2`, then the response after the server's `+` continuation: never as an initial response on the AUTH
    /// line, which RFC 5034 keeps to 255 octets (an access token is longer). A refused token comes back as a `+`
    /// challenge (the provider's error, base64 JSON), which an empty line answers, and then `-ERR`. A refusal is "mail
    /// server rejected the command", as a refused PASS.
    fn authenticate_xoauth2(&mut self, user: &str, token: &str) -> Result<(), Error> {
        use base64::Engine;
        let rejected = |line: &str| Error::new(format!("mail server rejected the command: AUTH XOAUTH2: {line}"));
        let continuation = self.exchange("AUTH XOAUTH2")?;
        if !continuation.starts_with('+') || continuation.starts_with("+OK") {
            return Err(rejected(&continuation));
        }
        let response = base64::engine::general_purpose::STANDARD.encode(super::oauth::xoauth2(user, token));
        let answer = self.exchange(&response)?;
        if answer.starts_with("+OK") {
            return Ok(());
        }
        if let Some(challenge) = answer.strip_prefix('+') {
            let detail = base64::engine::general_purpose::STANDARD
                .decode(challenge.trim())
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .unwrap_or_default();
            let end = self.exchange("")?;
            return Err(rejected(format!("{end} {detail}").trim()));
        }
        Err(rejected(&answer))
    }

    fn command(&mut self, line: &str) -> Result<String, Error> {
        let response = self.exchange(line)?;
        expect_ok(&response)?;
        Ok(response)
    }

    /// A line to the server and its answer, whatever it is.
    fn exchange(&mut self, line: &str) -> Result<String, Error> {
        write!(self.reader.get_mut(), "{line}\r\n").map_err(|err| Error::new(err.to_string()))?;
        self.reader
            .get_mut()
            .flush()
            .map_err(|err| Error::new(err.to_string()))?;
        read_line(&mut self.reader)
    }

    /// A multi-line answer up to the line with only a dot, as bytes: 8-bit mail is not text in any one charset.
    fn read_dot(&mut self) -> Result<Vec<u8>, Error> {
        let mut raw = Vec::new();
        loop {
            let line = read_line_bytes(&mut self.reader)?;
            if line == b"." {
                break;
            }
            let line = line
                .strip_prefix(b".")
                .filter(|rest| rest.starts_with(b"."))
                .unwrap_or(&line);
            raw.extend_from_slice(line);
            raw.extend_from_slice(b"\r\n");
        }
        Ok(raw)
    }
}

impl Mailbox for Pop3 {
    fn list_ids(&mut self) -> Result<Vec<String>, Error> {
        self.command("LIST")?;
        let listing = self.read_dot()?;
        Ok(String::from_utf8_lossy(&listing)
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .filter(|id| id.parse::<u32>().is_ok())
            .map(str::to_string)
            .collect())
    }

    fn fetch(&mut self, id: &str) -> Result<Inbound, Error> {
        self.command(&format!("RETR {id}"))?;
        let bytes = self.read_dot()?;
        let mut message = parse_rfc822(&bytes);
        message.id = id.to_string();
        Ok(message)
    }

    fn mark_seen(&mut self, _id: &str) -> Result<(), Error> {
        Ok(())
    }

    fn delete(&mut self, id: &str) -> Result<(), Error> {
        self.command(&format!("DELE {id}")).map(|_| ())
    }

    /// `QUIT` is when a POP3 server removes the deleted messages, so its answer is read.
    fn close(&mut self) -> Result<(), Error> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.command("QUIT").map(|_| ())
    }
}

impl Drop for Pop3 {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.reader.get_mut().write_all(b"QUIT\r\n");
            let _ = self.reader.get_mut().flush();
            self.closed = true;
        }
    }
}

pub(super) enum Net {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Read for Net {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Net::Plain(stream) => stream.read(buf),
            Net::Tls(stream) => stream.read(buf),
        }
    }
}

impl Write for Net {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Net::Plain(stream) => stream.write(buf),
            Net::Tls(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Net::Plain(stream) => stream.flush(),
            Net::Tls(stream) => stream.flush(),
        }
    }
}

fn connect_net(host: &str, port: u16, tls: bool) -> Result<Net, Error> {
    let tcp = connect_tcp(host, port)?;
    if !tls {
        return Ok(Net::Plain(tcp));
    }
    wrap_tls(tcp, host)
}

/// A TCP connection to a mail server, with read and write timeouts of a minute.
pub(super) fn connect_tcp(host: &str, port: u16) -> Result<TcpStream, Error> {
    // every mail service checks the fence; this is the one place each mail connection goes through
    crate::entity::fence::check_effect(crate::entity::fence::Effect::Mail, &format!("{host}:{port}"))
        .map_err(|err| Error::new(err.message()))?;
    // A blackholed host fails in 30 seconds instead of the system default of minutes.
    let address = std::net::ToSocketAddrs::to_socket_addrs(&(host, port))
        .map_err(|err| Error::new(format!("could not resolve {host}:{port}: {err}")))?
        .next()
        .ok_or_else(|| Error::new(format!("could not resolve {host}:{port}")))?;
    let tcp = TcpStream::connect_timeout(&address, Duration::from_secs(30))
        .map_err(|err| Error::new(format!("could not connect to {host}:{port}: {err}")))?;
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(60)));
    let _ = tcp.set_write_timeout(Some(Duration::from_secs(60)));
    Ok(tcp)
}

/// TLS over a connection already made: from the start (imaps, pop3s) or after STARTTLS.
pub(super) fn wrap_tls(tcp: TcpStream, host: &str) -> Result<Net, Error> {
    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| Error::new(format!("email host {host} is not a DNS name")))?;
    let config = tls_config()?;
    let conn = rustls::ClientConnection::new(config, name).map_err(|err| Error::new(err.to_string()))?;
    Ok(Net::Tls(Box::new(rustls::StreamOwned::new(conn, tcp))))
}

impl Net {
    /// The TCP connection under the stream, for its timeouts.
    pub(super) fn socket(&self) -> &TcpStream {
        match self {
            Net::Plain(stream) => stream,
            Net::Tls(stream) => &stream.sock,
        }
    }
}

/// So an IMAP IDLE can wait with a timeout.
impl imap::extensions::idle::SetReadTimeout for Net {
    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> imap::error::Result<()> {
        self.socket().set_read_timeout(timeout).map_err(imap::error::Error::Io)
    }
}

fn tls_config() -> Result<Arc<rustls::ClientConfig>, Error> {
    let mut roots = rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    for der in extra_ca_certificates()? {
        roots
            .add(der)
            .map_err(|err| Error::new(format!("tls-ca-file: {err}")))?;
    }
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|err| Error::new(err.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// A status line, which is ASCII; any other byte is replaced.
fn read_line(reader: &mut impl BufRead) -> Result<String, Error> {
    read_line_bytes(reader).map(|line| String::from_utf8_lossy(&line).into_owned())
}

pub(super) fn read_line_bytes(reader: &mut impl BufRead) -> Result<Vec<u8>, Error> {
    let mut line = Vec::new();
    let read = reader
        .read_until(b'\n', &mut line)
        .map_err(|err| Error::new(err.to_string()))?;
    if read == 0 {
        return Err(Error::new("mail server closed the connection"));
    }
    if line.ends_with(b"\n") {
        line.pop();
    }
    if line.ends_with(b"\r") {
        line.pop();
    }
    Ok(line)
}

fn expect_ok(line: &str) -> Result<(), Error> {
    if line.starts_with("+OK") {
        Ok(())
    } else {
        Err(Error::new(format!("mail server rejected the command: {line}")))
    }
}

/// A message as the email-ECA rules see it, as Java's EmailEcaRule makes it from a MimeMessage: the decoded subject,
/// the addresses as `Name <address>`, the header values as they were written (by lower-case name, a list when one
/// repeats), and the leaf parts of every multipart in order, text in its charset as `contentText` and anything else as
/// `contentBytes`.
pub(super) fn parse_rfc822(bytes: &[u8]) -> Inbound {
    let mut message = Inbound::blank();
    message.raw = bytes.to_vec();
    let Some(parsed) = mail_parser::MessageParser::default().parse(bytes) else {
        return message;
    };
    for header in parsed.headers() {
        push_header(&mut message.headers, header.name(), raw_header_value(bytes, header));
    }
    message.subject = parsed.subject().unwrap_or_default().to_string();
    message.from = parsed
        .from()
        .and_then(mail_parser::Address::first)
        .map(address_text)
        .unwrap_or_default();
    let list = |address: Option<&mail_parser::Address<'_>>| -> Vec<String> {
        address
            .map(|address| address.iter().map(address_text).collect())
            .unwrap_or_default()
    };
    message.to = list(parsed.to());
    message.cc = list(parsed.cc());
    message.bcc = list(parsed.bcc());
    message.sent = parsed.date().map_or(Value::Null, |date| {
        stamp_millis(date.to_timestamp().saturating_mul(1000))
    });
    leaf_parts(&parsed, bytes, 0, &mut message.parts, 0);
    message
}

/// A header's value as written, unfolded: what JavaMail's `Header.getValue` gives.
fn raw_header_value(bytes: &[u8], header: &mail_parser::Header<'_>) -> String {
    let start = usize::try_from(header.offset_start).unwrap_or(0).min(bytes.len());
    let end = usize::try_from(header.offset_end)
        .unwrap_or(0)
        .clamp(start, bytes.len());
    let text = String::from_utf8_lossy(&bytes[start..end]);
    let mut out = String::new();
    for (index, line) in text.split('\n').enumerate() {
        let line = line.trim_end_matches('\r');
        if index == 0 {
            out.push_str(line.trim());
        } else if !line.trim().is_empty() {
            out.push(' ');
            out.push_str(line.trim());
        }
    }
    out
}

/// `InternetAddress.toString`: the name and the address in angle brackets, or the address alone.
pub(super) fn address_text(addr: &mail_parser::Addr<'_>) -> String {
    let address = addr.address().unwrap_or_default();
    match addr.name().filter(|name| !name.is_empty()) {
        Some(name) if name.contains([',', '"', '<', '>', '@', ';', ':']) => {
            format!("\"{}\" <{address}>", name.replace('"', "\\\""))
        }
        Some(name) => format!("{name} <{address}>"),
        None => address.to_string(),
    }
}

/// The parts with content under `id`, depth first, as Java's `makeBodyPartList`. A nested message is skipped, as in Java.
fn leaf_parts(message: &mail_parser::Message<'_>, raw: &[u8], id: u32, out: &mut Vec<Value>, depth: usize) {
    use mail_parser::{MimeHeaders, PartType};
    let Some(part) = message.part(id) else {
        return;
    };
    if depth > 32 {
        return;
    }
    match &part.body {
        PartType::Multipart(children) => {
            for child in children {
                leaf_parts(message, raw, *child, out, depth + 1);
            }
            return;
        }
        PartType::Message(_) => return,
        _ => {}
    }
    let mut map = IndexMap::new();
    let content_type = part
        .headers()
        .iter()
        .find(|header| header.name().eq_ignore_ascii_case("content-type"))
        .map_or_else(|| "text/plain".to_string(), |header| raw_header_value(raw, header));
    map.insert("contentType".into(), Value::string(content_type));
    if let Some(name) = part.attachment_name() {
        map.insert("filename".into(), Value::string(name));
    }
    if let Some(disposition) = part.content_disposition() {
        map.insert(
            "disposition".into(),
            Value::string(disposition.ctype().to_ascii_lowercase()),
        );
    }
    match &part.body {
        PartType::Text(text) | PartType::Html(text) => {
            map.insert("contentText".into(), Value::string(text.as_ref()));
        }
        PartType::Binary(bytes) | PartType::InlineBinary(bytes) => {
            map.insert("contentBytes".into(), Value::Bytes(bytes.to_vec()));
        }
        PartType::Multipart(_) | PartType::Message(_) => {}
    }
    out.push(Value::Map(map));
}

fn push_header(headers: &mut IndexMap<String, Value>, name: &str, value: String) {
    let key = name.to_ascii_lowercase();
    if let Some(existing) = headers.shift_remove(&key) {
        let list = match existing {
            Value::List(mut items) => {
                items.push(Value::string(value));
                items
            }
            other => vec![other, Value::string(value)],
        };
        headers.insert(key, Value::List(list));
    } else {
        headers.insert(key, Value::string(value));
    }
}

pub(super) fn header_one(headers: &IndexMap<String, Value>, name: &str) -> String {
    match headers.get(name) {
        Some(Value::List(items)) => items.first().map(to_plain_string).unwrap_or_default(),
        Some(value) => to_plain_string(value),
        None => String::new(),
    }
}

pub(super) fn stamp_millis(millis: i64) -> Value {
    basic_convert(&Value::I64(millis), "java.sql.Timestamp").value
}
