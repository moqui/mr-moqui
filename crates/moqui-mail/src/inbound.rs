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
    extra_ca_certificates, fail, find_one, force_new, note_error, parse_port, private_network_denied, require_host,
    TestStep, SERVER,
};
use moqui_core::context::{Error, ExecutionContext};
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::service::util::{field_text, has_error, param_text};

#[cfg(test)]
thread_local! {
    static FAKE: RefCell<Option<Vec<Inbound>>> = const { RefCell::new(None) };
    /// The ids the fake store was asked to delete.
    pub(super) static FAKE_DELETED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// False to treat a loopback host as any other in [`plain_allowed`], for a test of the refusal on the local servers.
    pub(super) static LOOPBACK_PLAIN: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

pub(super) fn poll_server(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if facade.sim_active() {
        return fail("Email poll is disabled in LLM sim session");
    }
    moqui_core::entity::fence::check_effect(moqui_core::entity::fence::Effect::Mail, "poll#EmailServer")?;
    let server_id = param_text(parameters, "emailServerId");
    // a mail account's own server is read by the mail client's sync, as its user, never polled by id (SEC-MAIL-04): it
    // answers as one that does not exist
    let owned = moqui_core::entity::sensitive::account_owned_email_server(facade, SERVER, parameters)?;
    let found = if owned {
        None
    } else {
        find_one(facade, SERVER, "emailServerId", &server_id)?
    };
    let Some(server) = found else {
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
        allow_plain: field_text(&server, "storeAllowPlain") == "Y",
        deny_internal: private_network_denied(&server),
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
    let max_message = max_message();
    for id in ids {
        // a message over the largest size is left on the server, as its read would fail the poll each time
        match store.size(&id) {
            Ok(Some(size)) if size > max_message => {
                tracing::warn!(
                    "Message {id} on {} is {size} bytes, over email-client max-message-size ({max_message}); left \
                    on the server",
                    settings.host
                );
                continue;
            }
            Ok(_) => {}
            Err(text) => {
                errors.push(format!(
                    "Error reading the size of message {id} from {}: {text}",
                    settings.host
                ));
                break;
            }
        }
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
    in_eca_transaction(facade, || moqui_core::service::run_email_rules(&fields))
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
    let count = moqui_core::service::email_rule_count();
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
        if let Err(err) = in_eca_transaction(facade, || moqui_core::service::run_email_rule(index, &fields)) {
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
    steps: &mut Vec<TestStep>,
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
            steps.push(TestStep::failed("storeLogin", err.to_string()));
            return;
        }
    };
    let place = format!("{host}:{port}");
    if let Err(err) = require_host(&host) {
        steps.push(TestStep::failed("storeLogin", err.to_string()));
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
        allow_plain: field_text(server, "storeAllowPlain") == "Y",
        deny_internal: private_network_denied(server),
    };
    // the connection the sync makes: TLS from the start on imaps or port 993, else STARTTLS when the server offers it
    // (without it, refused unless the server allows plain text or is on this machine)
    let info = super::sync::ServerInfo {
        server_id: field_text(server, "emailServerId"),
        host: host.clone(),
        port,
        protocol: protocol.clone(),
        user: settings.user.clone(),
        password: settings.password.clone(),
        auth_type: field_text(server, "authTypeEnumId"),
        store_delete: false,
        allow_plain: settings.allow_plain,
        deny_internal: settings.deny_internal,
        access_token: None,
    };
    // this server's own refusal of a store with no TLS is shown as it is; any other failure as its kind only
    let own = [plain_refused("POP3", &host, "STLS"), plain_refused("IMAP", &host, "STARTTLS")];
    if pop {
        // as the sync signs in: USER and PASS, or AUTH XOAUTH2 for a provider sign-in
        let how = if oauth { " with XOAUTH2" } else { "" };
        match super::sync::open_pop(facade, &info, &settings).and_then(|mut pop3| {
            pop3.close()
                .map_err(|err| super::sync::SyncFailure::Connect(err.to_string()))
        }) {
            Ok(()) => steps.push(TestStep::passed(
                "storeLogin",
                format!("Signed in to POP3 {place}{how}"),
            )),
            Err(err) => {
                let kind = super::implementation::test_failure("storeLogin", &place, &err.to_string(), &own);
                steps.push(TestStep::failed("storeLogin", format!("POP3 {place}: {kind}")));
            }
        }
        return;
    }
    if !matches!(protocol.as_str(), "imap" | "imaps") {
        steps.push(TestStep::failed(
            "storeLogin",
            format!("storeProtocol {protocol} is not supported"),
        ));
        return;
    }
    let mut session = match super::sync::connect_imap(facade, &info) {
        Ok(conn) => {
            steps.push(TestStep::passed("storeLogin", format!("Signed in to IMAP {place}")));
            conn.session
        }
        Err(err) => {
            let kind = super::implementation::test_failure("storeLogin", &place, &err.to_string(), &own);
            steps.push(TestStep::failed("storeLogin", format!("IMAP {place}: {kind}")));
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
                let mut map = IndexMap::default();
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
            steps.push(TestStep::passed(
                "storeList",
                format!("Found {} folders", folders.len()),
            ));
        }
        Err(err) => {
            let kind = super::implementation::test_failure("storeList", &place, &err.to_string(), &[]);
            steps.push(TestStep::failed("storeList", format!("IMAP LIST on {place}: {kind}")));
        }
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
    /// storeAllowPlain: a plain imap or pop3 store may sign in without TLS when the server offers no STARTTLS or STLS.
    pub(super) allow_plain: bool,
    /// The server may reach no internal address (`implementation::private_network_denied`).
    pub(super) deny_internal: bool,
}

fn eca_fields(message: &Inbound, server_id: &str) -> BTreeMap<String, Value> {
    let mut fields = IndexMap::default();
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
    let mut flags = IndexMap::default();
    flags.insert("answered".into(), Value::Bool(message.flags.answered));
    flags.insert("deleted".into(), Value::Bool(message.flags.deleted));
    flags.insert("draft".into(), Value::Bool(message.flags.draft));
    flags.insert("flagged".into(), Value::Bool(message.flags.flagged));
    flags.insert("recent".into(), Value::Bool(message.flags.recent));
    flags.insert("seen".into(), Value::Bool(message.seen));
    // Java puts the MimeMessage here; this is the part of it the rules read: its id, size and raw text.
    let mut raw = IndexMap::default();
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
    pub(super) headers: moqui_lib::object::ValueMap,
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
            headers: IndexMap::default(),
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
    /// The size the server gives for a message, when it gives one.
    fn size(&mut self, _id: &str) -> Result<Option<u64>, Error> {
        Ok(None)
    }
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

    fn size(&mut self, id: &str) -> Result<Option<u64>, Error> {
        Ok(self
            .messages
            .iter()
            .find(|message| message.id == id)
            .map(|message| message.raw.len() as u64))
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

/// Whether a plain imap or pop3 store whose server offers no STARTTLS (STLS) signs in anyway, sending the password
/// in plain text: only with the server's storeAllowPlain, or on a loopback host, where nothing leaves the machine.
pub(super) fn plain_allowed(host: &str, allow_plain: bool) -> bool {
    #[cfg(test)]
    if !LOOPBACK_PLAIN.with(std::cell::Cell::get) {
        return allow_plain;
    }
    allow_plain || is_loopback(host)
}

/// localhost, 127.0.0.0/8 or ::1 (bracketed or not).
pub(super) fn is_loopback(host: &str) -> bool {
    let host = host.trim();
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<std::net::IpAddr>()
        .is_ok_and(|address| address.is_loopback())
}

/// Why a store with no TLS is not signed in to, and how to allow it.
pub(super) fn plain_refused(kind: &str, host: &str, upgrade: &str) -> String {
    format!(
        "{kind} server {host} offers no {upgrade}, and the password is not sent without TLS; use {} or set \
         storeAllowPlain to Y on the email server to sign in without TLS",
        if kind == "POP3" { "pop3s" } else { "imaps" }
    )
}

struct ImapBox {
    session: imap::Session<Net>,
    closed: bool,
    deleted: bool,
    skip_seen: bool,
}

impl ImapBox {
    /// Signed in as the sync signs in ([`super::sync::open_imap`]): TLS from the start on imaps or port 993, else
    /// STARTTLS, and without it refused unless [`plain_allowed`].
    fn open(settings: &StoreSettings) -> Result<Self, Error> {
        let info = super::sync::ServerInfo {
            server_id: String::new(),
            host: settings.host.clone(),
            port: settings.port,
            protocol: settings.protocol.clone(),
            user: settings.user.clone(),
            password: settings.password.clone(),
            auth_type: String::new(),
            store_delete: settings.delete,
            allow_plain: settings.allow_plain,
            deny_internal: settings.deny_internal,
            access_token: None,
        };
        let mut session = super::sync::open_imap(&info)
            .map_err(|err| Error::new(err.to_string()))?
            .session;
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

    fn size(&mut self, id: &str) -> Result<Option<u64>, Error> {
        let fetches = self
            .session
            .fetch(id, "RFC822.SIZE")
            .map_err(|err| Error::new(err.to_string()))?;
        Ok(fetches.iter().next().and_then(|fetch| fetch.size).map(u64::from))
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

    /// `LIST n`: the size the server gives for one message, None when it gives none.
    pub(super) fn size(&mut self, number: u32) -> Result<Option<u64>, Error> {
        let answer = self.command(&format!("LIST {number}"))?;
        Ok(answer.split_whitespace().nth(2).and_then(|size| size.parse().ok()))
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
        let tls = use_tls(&settings.protocol, settings.port);
        let net = connect_net(&settings.host, settings.port, tls, settings.deny_internal)?;
        let mut reader = BufReader::new(net);
        let greeting = read_line(&mut reader)?;
        expect_ok(&greeting)?;
        if !tls {
            reader = pop_start_tls(reader, settings)?;
        }
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
        pop_exchange(&mut self.reader, line)
    }

    /// A multi-line answer up to the line with only a dot, as bytes: 8-bit mail is not text in any one charset.
    fn read_dot(&mut self) -> Result<Vec<u8>, Error> {
        pop_read_dot(&mut self.reader)
    }
}

fn pop_exchange(reader: &mut BufReader<Net>, line: &str) -> Result<String, Error> {
    write!(reader.get_mut(), "{line}\r\n").map_err(|err| Error::new(err.to_string()))?;
    reader.get_mut().flush().map_err(|err| Error::new(err.to_string()))?;
    read_line(reader)
}

fn pop_read_dot(reader: &mut BufReader<Net>) -> Result<Vec<u8>, Error> {
    let mut raw = Vec::new();
    loop {
        let line = read_line_bytes(reader)?;
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

/// On plain pop3, `STLS` (RFC 2595) when `CAPA` lists it and TLS from there on, as STARTTLS on imap; without it the
/// sign-in is refused unless [`plain_allowed`].
fn pop_start_tls(mut reader: BufReader<Net>, settings: &StoreSettings) -> Result<BufReader<Net>, Error> {
    // a server without CAPA answers -ERR: it has no STLS either
    let capabilities = if pop_exchange(&mut reader, "CAPA")?.starts_with("+OK") {
        String::from_utf8_lossy(&pop_read_dot(&mut reader)?).to_ascii_uppercase()
    } else {
        String::new()
    };
    if capabilities.lines().any(|line| line.trim() == "STLS") {
        expect_ok(&pop_exchange(&mut reader, "STLS")?)?;
        // the server sends nothing more before the handshake, so the buffer holds nothing
        let Some(tcp) = reader.into_inner().into_plain() else {
            return Err(Error::new("POP3 STLS on a connection already in TLS"));
        };
        return Ok(BufReader::new(wrap_tls(tcp, &settings.host)?));
    }
    if !plain_allowed(&settings.host, settings.allow_plain) {
        return Err(Error::new(plain_refused("POP3", &settings.host, "STLS")));
    }
    if !is_loopback(&settings.host) {
        tracing::warn!(
            host = %settings.host,
            "POP3 server {} offers no STLS; signing in without TLS (storeAllowPlain)",
            settings.host
        );
    }
    Ok(reader)
}

impl Mailbox for Pop3 {
    fn size(&mut self, id: &str) -> Result<Option<u64>, Error> {
        match id.parse::<u32>() {
            Ok(number) => Pop3::size(self, number),
            Err(_) => Ok(None),
        }
    }

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

/// A connection to an IMAP or POP3 server. What the server sends in answer to one command, counted from the client's
/// last write, is at most [`answer_limit`] bytes (SEC-MAIL-85): the IMAP client holds a whole response, a literal
/// included, and the POP3 client a whole line and a whole multi-line answer, so a server that sends without end would
/// otherwise grow the process's memory until it fails. An answer also takes at most `email-client` `answer-timeout`
/// from the client's last write: a read times out only after a minute with no byte at all, so a server that sends a
/// byte now and then would hold a sync or IDLE worker without end. Past either limit a read fails, and the connection
/// with it.
pub(super) struct Net {
    stream: Stream,
    /// The bytes read since the last write.
    read: u64,
    limit: u64,
    /// When the last write was (or the connection was made), and how long its answer may take.
    since: std::time::Instant,
    answer_time: Duration,
}

enum Stream {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

/// Room for what an answer holds beside its message: the lines and flags around a body fetch.
const ANSWER_MARGIN: u64 = 1024 * 1024;

/// A batch of the sync's body fetches is at most this, by the sizes the server gave (`sync.rs` `FETCH_BYTES`), so an
/// answer may be this large whatever `max-message-size` is.
const BATCH_BYTES: u64 = 8 * 1024 * 1024;

/// The largest message read from an IMAP or POP3 server: `email-client` `max-message-size`. A larger one, by the size
/// the server gives, is skipped.
pub(super) fn max_message() -> u64 {
    u64::try_from(super::sync::property_number("max-message-size", 64 * 1024 * 1024)).unwrap_or(u64::MAX)
}

/// The most bytes one answer of a mail server may be: the larger of [`max_message`] and [`BATCH_BYTES`], and
/// [`ANSWER_MARGIN`].
fn answer_limit() -> u64 {
    max_message().max(BATCH_BYTES).saturating_add(ANSWER_MARGIN)
}

/// How long one answer of a mail server may take: `email-client` `answer-timeout`, seconds. It is longer than an IDLE,
/// whose answer lasts until the client ends it (`idle.rs` `IDLE_RENEW`).
pub(super) fn answer_time() -> Duration {
    Duration::from_secs(u64::try_from(super::sync::property_number("answer-timeout", 900)).unwrap_or(900))
}

impl Net {
    pub(super) fn plain(tcp: TcpStream) -> Net {
        Net::over(Stream::Plain(tcp))
    }

    fn over(stream: Stream) -> Net {
        Net {
            stream,
            read: 0,
            limit: answer_limit(),
            since: std::time::Instant::now(),
            answer_time: answer_time(),
        }
    }

    /// The TCP connection, when no TLS is on it yet (for STARTTLS).
    pub(super) fn into_plain(self) -> Option<TcpStream> {
        match self.stream {
            Stream::Plain(tcp) => Some(tcp),
            Stream::Tls(_) => None,
        }
    }
}

impl Read for Net {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.limit.saturating_sub(self.read);
        if left == 0 {
            tracing::warn!(
                "a mail server sent more than {} bytes in answer to one command; the connection is closed",
                self.limit
            );
            return Err(std::io::Error::other(format!(
                "the mail server's answer is larger than {} bytes (email-client max-message-size)",
                self.limit
            )));
        }
        if self.since.elapsed() > self.answer_time {
            tracing::warn!(
                "a mail server took more than {} seconds to answer one command; the connection is closed",
                self.answer_time.as_secs()
            );
            return Err(std::io::Error::other(format!(
                "the mail server's answer took more than {} seconds (email-client answer-timeout)",
                self.answer_time.as_secs()
            )));
        }
        let room = buf.len().min(usize::try_from(left).unwrap_or(usize::MAX));
        let read = match &mut self.stream {
            Stream::Plain(stream) => stream.read(&mut buf[..room]),
            Stream::Tls(stream) => stream.read(&mut buf[..room]),
        }?;
        self.read = self.read.saturating_add(read as u64);
        Ok(read)
    }
}

impl Write for Net {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = match &mut self.stream {
            Stream::Plain(stream) => stream.write(buf),
            Stream::Tls(stream) => stream.write(buf),
        }?;
        if written > 0 {
            // a command (or a literal of one): what comes next is its answer
            self.read = 0;
            self.since = std::time::Instant::now();
        }
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match &mut self.stream {
            Stream::Plain(stream) => stream.flush(),
            Stream::Tls(stream) => stream.flush(),
        }
    }
}

fn connect_net(host: &str, port: u16, tls: bool, deny_internal: bool) -> Result<Net, Error> {
    let tcp = connect_tcp(host, port, deny_internal)?;
    if !tls {
        return Ok(Net::plain(tcp));
    }
    wrap_tls(tcp, host)
}

/// A TCP connection to a mail server, with read and write timeouts of a minute. The name is looked up once and the
/// addresses found are checked and connected to, so a second lookup cannot answer another: with `deny_internal` none
/// may be internal (`moqui_lib::net::public_addresses`), and none is ever link-local or a metadata address.
pub(super) fn connect_tcp(host: &str, port: u16, deny_internal: bool) -> Result<TcpStream, Error> {
    // every mail service checks the fence; this is the one place each mail connection goes through
    moqui_core::entity::fence::check_effect(moqui_core::entity::fence::Effect::Mail, &format!("{host}:{port}"))
        .map_err(|err| Error::new(err.message()))?;
    let found = if deny_internal {
        moqui_lib::net::public_addresses(host, port)
    } else {
        moqui_lib::net::non_link_local_addresses(host, port)
    };
    let addresses = found.map_err(|err| {
        let text = err.to_string();
        if text.contains("address and is") {
            Error::new(format!("{host} {}", super::implementation::INTERNAL_SERVER))
        } else {
            Error::new(format!("could not resolve {host}:{port}: {text}"))
        }
    })?;
    // A blackholed host fails in 30 seconds instead of the system default of minutes.
    let mut last = None;
    let mut connected = None;
    for address in &addresses {
        match TcpStream::connect_timeout(address, Duration::from_secs(30)) {
            Ok(tcp) => {
                connected = Some(tcp);
                break;
            }
            Err(err) => last = Some(err),
        }
    }
    let tcp = match (connected, last) {
        (Some(tcp), _) => tcp,
        (None, Some(err)) => return Err(Error::new(format!("could not connect to {host}:{port}: {err}"))),
        (None, None) => return Err(Error::new(format!("could not resolve {host}:{port}"))),
    };
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
    Ok(Net::over(Stream::Tls(Box::new(rustls::StreamOwned::new(conn, tcp)))))
}

impl Net {
    /// The TCP connection under the stream, for its timeouts.
    pub(super) fn socket(&self) -> &TcpStream {
        match &self.stream {
            Stream::Plain(stream) => stream,
            Stream::Tls(stream) => &stream.sock,
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
        push_header(
            &mut message.headers,
            header.raw_name(),
            raw_header_value(bytes, &header),
        );
    }
    message.subject = parsed.subject().unwrap_or_default().to_string();
    message.from = parsed
        .from()
        .and_then(|list| list.first())
        .map(|mailbox| address_text(&mailbox))
        .unwrap_or_default();
    let list = |list: Option<mail_parser::AddressList<'_>>| -> Vec<String> {
        list.map(|list| list.mailboxes().map(|mailbox| address_text(&mailbox)).collect())
            .unwrap_or_default()
    };
    message.to = list(parsed.to());
    message.cc = list(parsed.cc());
    message.bcc = list(parsed.bcc());
    message.sent = parsed.date().map_or(Value::Null, |date| {
        stamp_millis(date.to_timestamp().saturating_mul(1000))
    });
    leaf_parts(parsed.root_part(), bytes, &mut message.parts, 0);
    message
}

/// A header's value as written, unfolded: what JavaMail's `Header.getValue` gives.
fn raw_header_value(bytes: &[u8], header: &mail_parser::Header<'_>) -> String {
    let start = usize::try_from(header.offset_start()).unwrap_or(0).min(bytes.len());
    let end = usize::try_from(header.offset_end())
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
pub(super) fn address_text(addr: &mail_parser::Mailbox<'_>) -> String {
    let address = addr.address().unwrap_or_default();
    match addr.name().filter(|name| !name.is_empty()) {
        Some(name) if name.contains([',', '"', '<', '>', '@', ';', ':']) => {
            format!("\"{}\" <{address}>", name.replace('"', "\\\""))
        }
        Some(name) => format!("{name} <{address}>"),
        None => address.to_string(),
    }
}

/// The parts with content under `part`, depth first, as Java's `makeBodyPartList`. A nested message is skipped, as in
/// Java.
fn leaf_parts(part: mail_parser::MessagePart<'_>, raw: &[u8], out: &mut Vec<Value>, depth: usize) {
    use mail_parser::PartKind;
    if depth > 32 {
        return;
    }
    match part.kind() {
        PartKind::Multipart => {
            for child in part.children() {
                leaf_parts(child, raw, out, depth + 1);
            }
            return;
        }
        PartKind::Message(_) => return,
        _ => {}
    }
    let mut map = IndexMap::default();
    let content_type = part
        .headers()
        .iter()
        .find(|header| header.raw_name().eq_ignore_ascii_case("content-type"))
        .map_or_else(|| "text/plain".to_string(), |header| raw_header_value(raw, &header));
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
    match part.kind() {
        PartKind::Text | PartKind::Html => {
            map.insert(
                "contentText".into(),
                Value::string(part.text().unwrap_or_default().as_ref()),
            );
        }
        PartKind::Binary | PartKind::InlineBinary => {
            map.insert("contentBytes".into(), Value::Bytes(part.decoded().into_owned()));
        }
        PartKind::Multipart | PartKind::Message(_) => {}
    }
    out.push(Value::Map(map));
}

fn push_header(headers: &mut moqui_lib::object::ValueMap, name: &str, value: String) {
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

pub(super) fn header_one(headers: &moqui_lib::object::ValueMap, name: &str) -> String {
    match headers.get(name) {
        Some(Value::List(items)) => items.first().map(to_plain_string).unwrap_or_default(),
        Some(value) => to_plain_string(value),
        None => String::new(),
    }
}

pub(super) fn stamp_millis(millis: i64) -> Value {
    basic_convert(&Value::I64(millis), "Timestamp").value
}
