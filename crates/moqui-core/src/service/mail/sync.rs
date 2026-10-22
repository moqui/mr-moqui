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

//! `EmailClientServices.sync#EmailAccount`: an IMAP or POP3 account's mail into `EmailMessage` rows, and its local
//! changes onto the server.
//!
//! One session, under the account's sync lock (`syncLockedUntil`, so no two run at once on any server):
//!
//! 1. The changes queued by `push#RemoteChanges` are made on the server, in order ([`super::remote`]).
//! 2. `LIST` maps the server's folders onto `EmailFolder` rows: INBOX and the special-use attributes (RFC 6154) onto
//!    the folders the account was made with, the others made, renamed or marked gone (`remoteRemovedDate`).
//! 3. Each subscribed folder, the Inbox first. `STATUS` skips one with nothing new. A UIDVALIDITY change voids its
//!    UIDs: its rows are kept for their Message-IDs and the folder is read again. New mail is the UIDs from
//!    `remoteUidNext` on, within the account's sync window (`SEARCH SINCE` on the first sync), at most
//!    `email-client` `sync-batch` a session. With CONDSTORE the flags that changed come from `CHANGEDSINCE`, and with QRESYNC
//!    the expunged UIDs from `VANISHED`; without them the flags of every message are read every `email-client` `flag-scan`
//!    minutes and an expunge is found by comparing the UIDs.
//! 4. A message is stored once: one already kept under its folder and UID is not fetched again; one whose
//!    Message-ID is a row that left another folder in this session is that row, moved (its links and labels kept);
//!    one whose Message-ID is a local copy waiting for its UID (a sent copy, a move made without UIDPLUS) is that
//!    copy. A row that left its folder and was found nowhere else is deleted (kept, out of the account, when a
//!    record refers to it).
//! 5. The account's rules run on the new Inbox mail (`run#EmailRules`), then the changes queued meanwhile go out.
//!
//! The email-ECA rules (`*.emecas.xml`, as poll#EmailServer runs them) run once on each new message as it is stored in
//! the Inbox, IMAP or POP3, in a transaction of their own, a failing rule logged ([`eca_due`] says which messages are new
//! mail; [`super::inbound::run_stored_ecas`] is what any other fetch path calls).
//!
//! Failure tolerant per account, folder and message: a message that does not parse is logged by account, folder and
//! UID and skipped; a folder that fails is logged and the others go on; a sign-in the server refuses marks the account
//! EasAuthFailed with `lastSyncError` and ends only that account's sync, as does an OAuth sign-in whose refresh token
//! the provider refuses ([`super::oauth`]; such an account signs in with XOAUTH2). A POP3 account is fetch-only into the Inbox,
//! by UIDL, deleting from the server only when its EmailServer says so (`storeDelete`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::net::TcpStream;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{to_plain_string, Value};

use super::implementation::{find_one, force_new, note_error, note_message, parse_port, require_host};
use super::inbound::{
    connect_tcp, folder_kind, name_attribute, run_stored_ecas, stamp_millis, use_tls, wrap_tls, Net, Pop3,
    StoreSettings, StoredMessage,
};
use super::store::{self, parse_message, update_fields, Placement, MESSAGE};
use crate::context::ExecutionContext;
use crate::entity::{Condition, EntityFacade, EntityValue};
use crate::service::util::{field_text, now_millis, param_text, take_errors_string};

pub(super) const ACCOUNT: &str = "moqui.basic.email.EmailAccount";
pub(super) const FOLDER: &str = "moqui.basic.email.EmailFolder";
pub(super) const SERVER: &str = "moqui.basic.email.EmailServer";
pub(super) const CHANGE: &str = "moqui.basic.email.EmailRemoteChange";
const SERVICES: &str = "org.moqui.impl.EmailClientServices";

/// How long a sync holds its account; it is renewed for each folder.
const LEASE_MILLIS: i64 = 15 * 60 * 1000;
/// UIDs in one FETCH.
const FETCH_CHUNK: usize = 25;
const HEADER_CHUNK: usize = 100;

// ---- the account ----

/// An access token, whose `Debug` does not show it.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct AccessToken(pub(super) String);

impl std::fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AccessToken(<redacted>)")
    }
}

#[derive(Clone, Debug)]
pub(super) struct ServerInfo {
    pub(super) server_id: String,
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) protocol: String,
    pub(super) user: String,
    pub(super) password: String,
    pub(super) auth_type: String,
    pub(super) store_delete: bool,
    /// For an EatOAuth2 server, the access token [`connect_imap`] signs in with (XOAUTH2).
    pub(super) access_token: Option<AccessToken>,
}

impl ServerInfo {
    pub(super) fn is_oauth(&self) -> bool {
        self.auth_type == super::oauth::OAUTH
    }

    pub(super) fn is_pop(&self) -> bool {
        matches!(self.protocol.as_str(), "pop3" | "pop3s")
    }

    pub(super) fn is_imap(&self) -> bool {
        matches!(self.protocol.as_str(), "imap" | "imaps")
    }

    /// What a change of these settings changes for a connection kept open, such as an IDLE worker's.
    pub(super) fn fingerprint(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            self.protocol, self.host, self.port, self.user, self.auth_type
        )
    }
}

#[derive(Clone, Debug)]
pub(super) struct AccountInfo {
    pub(super) id: String,
    pub(super) kind: String,
    pub(super) status: String,
    pub(super) window_days: Option<i64>,
    pub(super) interval_minutes: Option<i64>,
    pub(super) last_sync_millis: Option<i64>,
    pub(super) server: Option<ServerInfo>,
}

pub(super) fn value_millis(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Null => None,
        Value::Timestamp(stamp) => Some(stamp.as_millisecond()),
        Value::I64(millis) => Some(*millis),
        Value::I32(millis) => Some(i64::from(*millis)),
        other => {
            let text = to_plain_string(other);
            text.trim().parse::<i64>().ok().or_else(|| {
                moqui_lib::object::parse_timestamp(&text, &moqui_lib::object::text_time_zone())
                    .map(|stamp| stamp.as_millisecond())
            })
        }
    }
}

pub(super) fn value_number(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Null => None,
        Value::I64(number) => Some(*number),
        Value::I32(number) => Some(i64::from(*number)),
        other => to_plain_string(other)
            .trim()
            .parse::<f64>()
            .ok()
            .map(|number| number as i64),
    }
}

fn server_info(row: &EntityValue) -> Option<ServerInfo> {
    let host = field_text(row, "storeHost");
    if host.is_empty() {
        return None;
    }
    let mut protocol = field_text(row, "storeProtocol").to_ascii_lowercase();
    if protocol.is_empty() {
        protocol = "imaps".to_string();
    }
    let default_port = match protocol.as_str() {
        "imap" => 143,
        "pop3" => 110,
        "pop3s" => 995,
        _ => 993,
    };
    let port = parse_port(&field_text(row, "storePort"), default_port).unwrap_or(default_port);
    Some(ServerInfo {
        server_id: field_text(row, "emailServerId"),
        host,
        port,
        protocol,
        user: field_text(row, "mailUsername"),
        password: field_text(row, "mailPassword"),
        auth_type: field_text(row, "authTypeEnumId"),
        store_delete: field_text(row, "storeDelete") == "Y",
        access_token: None,
    })
}

pub(super) fn account_info(facade: &EntityFacade, row: &EntityValue) -> Result<AccountInfo, LibError> {
    let server_id = field_text(row, "emailServerId");
    let server = find_one(facade, SERVER, "emailServerId", &server_id)?
        .as_ref()
        .and_then(server_info);
    let status = field_text(row, "statusId");
    Ok(AccountInfo {
        id: field_text(row, "emailAccountId"),
        kind: field_text(row, "accountKindEnumId"),
        status: if status.is_empty() {
            "EasActive".to_string()
        } else {
            status
        },
        window_days: value_number(row.get("syncWindowDays")).filter(|days| *days > 0),
        interval_minutes: value_number(row.get("syncIntervalMinutes")).filter(|minutes| *minutes > 0),
        last_sync_millis: value_millis(row.get("lastSyncDate")),
        server,
    })
}

pub(super) fn load_account(facade: &EntityFacade, account_id: &str) -> Result<Option<AccountInfo>, LibError> {
    match find_one(facade, ACCOUNT, "emailAccountId", account_id)? {
        Some(row) => account_info(facade, &row).map(Some),
        None => Ok(None),
    }
}

// ---- folders ----

#[derive(Clone, Debug, Default)]
pub(super) struct FolderInfo {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) path: String,
    pub(super) parent_id: String,
    pub(super) kind: String,
    pub(super) delimiter: String,
    pub(super) uid_validity: Option<u32>,
    pub(super) uid_next: Option<u32>,
    pub(super) modseq: Option<u64>,
    pub(super) total: Option<u32>,
    pub(super) unread: Option<u32>,
    pub(super) last_scan: Option<i64>,
    pub(super) subscribed: bool,
    pub(super) removed: bool,
    pub(super) sequence: i64,
    /// A user chose the kind (`update#EmailFolder`): no server folder takes it by its special use or name.
    pub(super) kind_by_user: bool,
}

fn as_u32(value: Option<&Value>) -> Option<u32> {
    value_number(value).and_then(|number| u32::try_from(number).ok())
}

fn folder_info(row: &EntityValue) -> FolderInfo {
    FolderInfo {
        id: field_text(row, "emailFolderId"),
        name: field_text(row, "folderName"),
        path: field_text(row, "folderPath"),
        parent_id: field_text(row, "parentFolderId"),
        kind: {
            let kind = field_text(row, "folderKindEnumId");
            if kind.is_empty() {
                "EfkOther".to_string()
            } else {
                kind
            }
        },
        delimiter: field_text(row, "delimiter"),
        uid_validity: as_u32(row.get("remoteUidValidity")),
        uid_next: as_u32(row.get("remoteUidNext")),
        modseq: value_number(row.get("remoteHighestModSeq")).and_then(|number| u64::try_from(number).ok()),
        total: as_u32(row.get("totalCount")),
        unread: as_u32(row.get("unreadCount")),
        last_scan: value_millis(row.get("lastFlagScanDate")),
        subscribed: field_text(row, "subscribed") != "N",
        removed: row.get("remoteRemovedDate").is_some_and(|value| !value.is_null()),
        sequence: value_number(row.get("sequenceNum")).unwrap_or(999_999),
        kind_by_user: field_text(row, "kindByUser") == "Y",
    }
}

pub(super) fn load_folders(facade: &EntityFacade, account_id: &str) -> Result<Vec<FolderInfo>, LibError> {
    let rows = facade
        .find(FOLDER)?
        .eq("emailAccountId", Value::string(account_id))
        .order_by("sequenceNum", false)
        .order_by("emailFolderId", false)
        .list()?;
    Ok(rows.values().iter().map(folder_info).collect())
}

/// The order folders sync in: Inbox, Sent, Drafts, Archive, the others, Junk, Trash.
fn kind_rank(kind: &str) -> u8 {
    match kind {
        "EfkInbox" => 0,
        "EfkSent" => 1,
        "EfkDrafts" => 2,
        "EfkArchive" => 3,
        "EfkOther" => 4,
        "EfkJunk" => 5,
        "EfkTrash" => 6,
        _ => 7,
    }
}

// ---- IMAP connection ----

/// Why a session ended early.
#[derive(Debug)]
pub(super) enum SyncFailure {
    /// The server refused the sign-in: the account is EasAuthFailed until its settings change.
    Auth(String),
    /// The OAuth sign-in has ended (the provider refused the refresh token, or there is none): the account is
    /// EasAuthFailed until the user signs in again. The token store marked and logged it already.
    SignIn(String),
    /// The server could not be reached or the connection broke: EasError, tried again later.
    Connect(String),
}

impl std::fmt::Display for SyncFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auth(text) | Self::SignIn(text) | Self::Connect(text) => f.write_str(text),
        }
    }
}

impl From<super::oauth::TokenError> for SyncFailure {
    fn from(err: super::oauth::TokenError) -> Self {
        match err {
            super::oauth::TokenError::SignIn(text) => Self::SignIn(text),
            super::oauth::TokenError::Unavailable(text) => Self::Connect(text),
        }
    }
}

/// The server's capabilities after sign-in, upper case.
#[derive(Clone, Debug, Default)]
pub(super) struct Caps {
    names: HashSet<String>,
}

const KNOWN_CAPS: &[&str] = &[
    "CONDSTORE",
    "QRESYNC",
    "ENABLE",
    "UIDPLUS",
    "MOVE",
    "IDLE",
    "SPECIAL-USE",
    "X-GM-EXT-1",
    "STARTTLS",
    "LOGINDISABLED",
];

impl Caps {
    fn of(capabilities: &imap::types::Capabilities) -> Self {
        let names = KNOWN_CAPS
            .iter()
            .filter(|name| capabilities.has_str(name))
            .map(|name| (*name).to_string())
            .collect();
        Self { names }
    }

    pub(super) fn has(&self, name: &str) -> bool {
        self.names.contains(name)
    }
}

/// A signed-in IMAP session.
pub(super) struct ImapConn {
    pub(super) session: imap::Session<Net>,
    pub(super) caps: Caps,
    /// A second handle on the TCP connection, so another thread can end a wait (an IDLE) by shutting it down.
    pub(super) socket: Option<TcpStream>,
    /// QRESYNC (and so CONDSTORE) enabled for this session.
    pub(super) qresync: bool,
    pub(super) condstore: bool,
    /// The folder selected now.
    pub(super) selected: Option<Selected>,
}

#[derive(Clone, Debug)]
pub(super) struct Selected {
    pub(super) path: String,
    pub(super) mailbox: imap::types::Mailbox,
}

impl Selected {
    pub(super) fn keywords(&self) -> bool {
        self.mailbox
            .permanent_flags
            .iter()
            .any(|flag| matches!(flag, imap::types::Flag::MayCreate))
    }
}

/// One status line from a raw connection, before the imap client takes it over (the greeting, STARTTLS).
fn raw_line(stream: &mut TcpStream) -> Result<String, SyncFailure> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    while line.len() < 16_384 {
        match stream.read(&mut byte) {
            Ok(0) => {
                return Err(SyncFailure::Connect(
                    "the mail server closed the connection".to_string(),
                ))
            }
            Ok(_) => {
                if byte[0] == b'\n' {
                    break;
                }
                line.push(byte[0]);
            }
            Err(err) => return Err(SyncFailure::Connect(format!("reading from the mail server: {err}"))),
        }
    }
    Ok(String::from_utf8_lossy(&line).trim_end().to_string())
}

/// A command on a raw connection: the lines up to its tagged answer, which must be OK.
fn raw_command(stream: &mut TcpStream, tag: &str, command: &str) -> Result<Vec<String>, SyncFailure> {
    stream
        .write_all(format!("{tag} {command}\r\n").as_bytes())
        .and_then(|()| stream.flush())
        .map_err(|err| SyncFailure::Connect(format!("writing to the mail server: {err}")))?;
    let mut lines = Vec::new();
    loop {
        let line = raw_line(stream)?;
        if let Some(rest) = line.strip_prefix(&format!("{tag} ")) {
            if rest.to_ascii_uppercase().starts_with("OK") {
                return Ok(lines);
            }
            return Err(SyncFailure::Connect(format!("{command}: {rest}")));
        }
        lines.push(line);
    }
}

/// Connect and sign in. `imaps` (or port 993) is TLS from the start; `imap` upgrades with STARTTLS when the server
/// offers it, else goes on in plain text (logged).
pub(super) fn open_imap(server: &ServerInfo) -> Result<ImapConn, SyncFailure> {
    require_host(&server.host).map_err(|err| SyncFailure::Connect(err.to_string()))?;
    let tcp = connect_tcp(&server.host, server.port).map_err(|err| SyncFailure::Connect(err.to_string()))?;
    let socket = tcp.try_clone().ok();
    let client = if use_tls(&server.protocol, server.port) {
        let net = wrap_tls(tcp, &server.host).map_err(|err| SyncFailure::Connect(err.to_string()))?;
        let mut client = imap::Client::new(net);
        let greeting = client
            .read_greeting()
            .map_err(|err| SyncFailure::Connect(format!("IMAP {}: {err}", server.host)))?;
        if greeting.starts_with(b"* BYE") {
            return Err(SyncFailure::Connect(format!(
                "IMAP {} refused the connection: {}",
                server.host,
                String::from_utf8_lossy(&greeting).trim()
            )));
        }
        client
    } else {
        let mut tcp = tcp;
        let greeting = raw_line(&mut tcp)?;
        if greeting.to_ascii_uppercase().starts_with("* BYE") {
            return Err(SyncFailure::Connect(format!(
                "IMAP {} refused the connection: {greeting}",
                server.host
            )));
        }
        let capabilities = raw_command(&mut tcp, "s1", "CAPABILITY")?
            .join(" ")
            .to_ascii_uppercase();
        let net = if capabilities.split_whitespace().any(|word| word == "STARTTLS") {
            raw_command(&mut tcp, "s2", "STARTTLS")?;
            wrap_tls(tcp, &server.host).map_err(|err| SyncFailure::Connect(err.to_string()))?
        } else {
            tracing::warn!(
                host = %server.host,
                "IMAP server {} offers no STARTTLS; signing in without TLS",
                server.host
            );
            Net::Plain(tcp)
        };
        imap::Client::new(net)
    };
    let mut session = sign_in(client, server)?;
    let caps = session
        .capabilities()
        .map(|capabilities| Caps::of(&capabilities))
        .map_err(|err| SyncFailure::Connect(format!("IMAP CAPABILITY on {}: {err}", server.host)))?;
    Ok(ImapConn {
        session,
        caps,
        socket,
        qresync: false,
        condstore: false,
        selected: None,
    })
}

/// Sign in as the server's authentication type says: LOGIN with the password, or for EatOAuth2 `AUTHENTICATE XOAUTH2`
/// with the access token [`connect_imap`] put on the server.
fn sign_in(client: imap::Client<Net>, server: &ServerInfo) -> Result<imap::Session<Net>, SyncFailure> {
    if server.is_oauth() {
        let Some(token) = &server.access_token else {
            return Err(SyncFailure::Connect(format!(
                "IMAP {}: no access token to sign in with",
                server.host
            )));
        };
        let authenticator = super::oauth::XOauth2 {
            response: super::oauth::xoauth2(&server.user, &token.0),
        };
        return client
            .authenticate("XOAUTH2", &authenticator)
            .map_err(|(err, _)| classify_login(&err, &server.host));
    }
    client
        .login(&server.user, &server.password)
        .map_err(|(err, _)| classify_login(&err, &server.host))
}

/// [`open_imap`], first getting an access token for an EatOAuth2 server; a token the server refuses is replaced by a
/// new one once (it may have been revoked or have ended early).
pub(super) fn connect_imap(facade: &EntityFacade, server: &ServerInfo) -> Result<ImapConn, SyncFailure> {
    if !server.is_oauth() {
        return open_imap(server);
    }
    let mut signed = server.clone();
    let token = super::oauth::access_token(facade, &server.server_id, None)?;
    signed.access_token = Some(AccessToken(token.clone()));
    match open_imap(&signed) {
        Err(SyncFailure::Auth(text)) => {
            tracing::debug!(
                "IMAP {} refused the access token, trying a new one: {text}",
                server.host
            );
            let fresh = super::oauth::access_token(facade, &server.server_id, Some(&token))?;
            if fresh == token {
                return Err(SyncFailure::Auth(text));
            }
            signed.access_token = Some(AccessToken(fresh));
            open_imap(&signed)
        }
        other => other,
    }
}

fn classify_login(err: &imap::Error, host: &str) -> SyncFailure {
    let text = format!("{err}");
    let temporary = format!("{err:?}").to_ascii_uppercase().contains("UNAVAILABLE");
    match err {
        imap::Error::No(_) | imap::Error::Bad(_) if !temporary => {
            SyncFailure::Auth(format!("IMAP {host} refused the sign-in: {text}"))
        }
        _ => SyncFailure::Connect(format!("IMAP {host} sign-in: {text}")),
    }
}

/// Whether an IMAP error means the connection is gone, so nothing more can be done in this session.
pub(super) fn connection_lost(err: &imap::Error) -> bool {
    matches!(
        err,
        imap::Error::Io(_) | imap::Error::ConnectionLost | imap::Error::Bye(_) | imap::Error::TagMismatch(_)
    )
}

/// SELECT `path`, unless it is selected already and `fresh` is false.
pub(super) fn select(conn: &mut ImapConn, path: &str, fresh: bool) -> Result<Selected, imap::Error> {
    if !fresh {
        if let Some(selected) = &conn.selected {
            if selected.path == path {
                return Ok(selected.clone());
            }
        }
    }
    conn.selected = None;
    let mailbox = conn.session.select(path)?;
    // what came unasked before the SELECT belongs to the folder left
    let _ = conn.session.take_all_unsolicited().count();
    let selected = Selected {
        path: path.to_string(),
        mailbox,
    };
    conn.selected = Some(selected.clone());
    Ok(selected)
}

/// A UID set in IMAP's syntax: ranges where they run on.
pub(super) fn uid_set(uids: &[u32]) -> String {
    let mut sorted = uids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut parts = Vec::new();
    let mut index = 0;
    while index < sorted.len() {
        let start = sorted[index];
        let mut end = start;
        while index + 1 < sorted.len() && sorted[index + 1] == end + 1 {
            index += 1;
            end = sorted[index];
        }
        parts.push(if start == end {
            start.to_string()
        } else {
            format!("{start}:{end}")
        });
        index += 1;
    }
    parts.join(",")
}

/// The UIDs of an IMAP set (`1:3,7`), up to a million; `*` is not expanded.
pub(super) fn expand_set(set: &str) -> Vec<u32> {
    let mut out = Vec::new();
    for part in set.split(',') {
        if let Some((low, high)) = part.split_once(':') {
            let (Ok(low), Ok(high)) = (low.trim().parse::<u32>(), high.trim().parse::<u32>()) else {
                continue;
            };
            let (low, high) = if low <= high { (low, high) } else { (high, low) };
            if high - low > 1_000_000 {
                continue;
            }
            out.extend(low..=high);
        } else if let Ok(uid) = part.trim().parse::<u32>() {
            out.push(uid);
        }
    }
    out
}

/// A quoted IMAP string.
pub(super) fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The UID of the message with this Message-ID in the selected folder, when there is one.
pub(super) fn search_message_id(conn: &mut ImapConn, message_id: &str) -> Result<Option<u32>, imap::Error> {
    if message_id.is_empty() || message_id.contains(['\r', '\n']) {
        return Ok(None);
    }
    let found = conn
        .session
        .uid_search(format!("HEADER MESSAGE-ID {}", quoted(message_id)))?;
    Ok(found.into_iter().max())
}

/// The flags of a message the sync keeps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct FlagSet {
    pub(super) seen: bool,
    pub(super) flagged: bool,
    pub(super) answered: bool,
    pub(super) keywords: Vec<String>,
}

impl FlagSet {
    pub(super) fn of(flags: &[imap::types::Flag<'_>]) -> Self {
        let mut out = Self::default();
        for flag in flags {
            match flag {
                imap::types::Flag::Seen => out.seen = true,
                imap::types::Flag::Flagged => out.flagged = true,
                imap::types::Flag::Answered => out.answered = true,
                imap::types::Flag::Custom(name) => out.keywords.push(name.to_string()),
                _ => {}
            }
        }
        out
    }
}

/// The IMAP keyword a label is kept as: its name with what an atom may not hold made `_`.
pub(super) fn label_keyword(name: &str) -> String {
    let keyword: String = name
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric()
                || matches!(
                    ch,
                    '-' | '_' | '.' | '+' | '!' | '#' | '&' | '\'' | ',' | ';' | '=' | '@' | '^' | '`' | '|' | '~'
                )
            {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let keyword = keyword.trim_start_matches(['\\', '$']).to_string();
    if keyword.is_empty() {
        "label".to_string()
    } else {
        keyword
    }
}

// ---- modified UTF-7 (RFC 3501 5.1.3) ----

/// A folder name from the server's modified UTF-7.
pub(super) fn decode_mutf7(text: &str) -> String {
    use base64::Engine;
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('-') else {
            out.push('&');
            out.push_str(after);
            return out;
        };
        let encoded = &after[..end];
        if encoded.is_empty() {
            out.push('&');
        } else {
            let decoded = base64::engine::general_purpose::STANDARD_NO_PAD
                .decode(encoded.replace(',', "/"))
                .ok()
                .filter(|bytes| bytes.len() % 2 == 0)
                .map(|bytes| {
                    let units: Vec<u16> = bytes
                        .chunks(2)
                        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                        .collect();
                    String::from_utf16_lossy(&units)
                });
            match decoded {
                Some(text) => out.push_str(&text),
                None => {
                    out.push('&');
                    out.push_str(encoded);
                    out.push('-');
                }
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// A folder name in the server's modified UTF-7.
pub(super) fn encode_mutf7(text: &str) -> String {
    use base64::Engine;
    let mut out = String::new();
    let mut pending: Vec<u16> = Vec::new();
    let flush = |pending: &mut Vec<u16>, out: &mut String| {
        if pending.is_empty() {
            return;
        }
        let bytes: Vec<u8> = pending.iter().flat_map(|unit| unit.to_be_bytes()).collect();
        let encoded = base64::engine::general_purpose::STANDARD_NO_PAD
            .encode(bytes)
            .replace('/', ",");
        out.push('&');
        out.push_str(&encoded);
        out.push('-');
        pending.clear();
    };
    for ch in text.chars() {
        if (' '..='~').contains(&ch) {
            flush(&mut pending, &mut out);
            if ch == '&' {
                out.push_str("&-");
            } else {
                out.push(ch);
            }
        } else {
            let mut units = [0u16; 2];
            pending.extend_from_slice(ch.encode_utf16(&mut units));
        }
    }
    flush(&mut pending, &mut out);
    out
}

// ---- the session ----

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct SyncOptions {
    /// Only make the queued local changes on the server.
    pub(super) push_only: bool,
    /// The Inbox first, and the other folders only when mail left it (an IDLE wake-up).
    pub(super) inbox_only: bool,
    /// A user asked for it: a paused account says so, an EasAuthFailed one is tried.
    pub(super) by_user: bool,
}

#[derive(Debug, Default)]
pub(super) struct SyncReport {
    /// synced, busy, skipped, paused, failed or authFailed.
    pub(super) outcome: &'static str,
    pub(super) message: String,
    pub(super) new_count: usize,
    pub(super) updated_count: usize,
    pub(super) removed_count: usize,
    pub(super) pushed_count: usize,
    pub(super) folder_count: usize,
    pub(super) skipped_messages: usize,
    pub(super) folder_errors: Vec<String>,
    /// A folder had more new mail than `email-client` `sync-batch`: the job syncs the account again at its next run, whatever
    /// its interval, until the backlog is in (a first sync of a large mailbox).
    pub(super) capped: bool,
}

impl SyncReport {
    fn with(outcome: &'static str, message: impl Into<String>) -> Self {
        Self {
            outcome,
            message: message.into(),
            ..Self::default()
        }
    }

    fn into_out(self) -> BTreeMap<String, Value> {
        let count = |number: usize| Value::I64(i64::try_from(number).unwrap_or(i64::MAX));
        let mut out = BTreeMap::new();
        out.insert("outcome".into(), Value::string(self.outcome));
        out.insert("message".into(), Value::string(self.message));
        out.insert("newCount".into(), count(self.new_count));
        out.insert("updatedCount".into(), count(self.updated_count));
        out.insert("removedCount".into(), count(self.removed_count));
        out.insert("pushedCount".into(), count(self.pushed_count));
        out.insert("folderCount".into(), count(self.folder_count));
        out.insert("skippedCount".into(), count(self.skipped_messages));
        out.insert(
            "folderErrorList".into(),
            Value::List(self.folder_errors.into_iter().map(Value::String).collect()),
        );
        out
    }
}

/// `sync#EmailAccount`: a user's "Sync now" (they must reach the account), or the job's and the IDLE workers' with
/// authorization disabled.
pub(super) fn sync_account_service(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if facade.sim_active() {
        note_error("Email sync is disabled in LLM sim session");
        return Ok(BTreeMap::new());
    }
    crate::entity::fence::check_effect(crate::entity::fence::Effect::Mail, "sync#EmailAccount")?;
    let account_id = param_text(parameters, "emailAccountId");
    let flag = |name: &str| match parameters.get(name) {
        Some(Value::Bool(on)) => *on,
        Some(value) => matches!(to_plain_string(value).to_ascii_lowercase().as_str(), "true" | "y"),
        None => false,
    };
    let authz_off = ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(false);
    if !authz_off {
        let mut access_in = BTreeMap::new();
        access_in.insert("emailAccountId".to_string(), Value::string(&account_id));
        let access = crate::service::sync(&format!("{SERVICES}.get#EmailAccountAccess"), &access_in)
            .map_err(|err| LibError::new(err.message()))?;
        if crate::service::util::has_error() {
            return Ok(BTreeMap::new());
        }
        let level = value_number(access.get("level")).unwrap_or(0);
        if level < 1 {
            note_error(format!("Email account {account_id} not found"));
            return Ok(BTreeMap::new());
        }
    }
    let options = SyncOptions {
        push_only: flag("pushOnly"),
        inbox_only: flag("inboxOnly"),
        by_user: !authz_off,
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let report = run_sync(facade, &account_id, &options);
    if options.by_user {
        match report.outcome {
            "failed" | "authFailed" => note_error(report.message.clone()),
            "busy" | "paused" | "skipped" => note_message(report.message.clone()),
            _ => {}
        }
    }
    Ok(report.into_out())
}

fn instance_name() -> String {
    let instance = moqui_lib::env::get_prop_or_env("instance_id");
    if !instance.trim().is_empty() {
        return instance;
    }
    let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "localhost".to_string());
    format!("{host}:{}", std::process::id())
}

/// Take the account's sync lock for [`LEASE_MILLIS`], or renew the one this server holds. False when another sync has it.
///
/// The row is read `FOR UPDATE`, here and in [`release_lease`]: an update of a row read writes back each field read, so
/// without the row lock two syncs could both read the lock free and both take it, and a lock taken or released could
/// write back the status a sync running at the same time had just recorded (a failed sync's EasError read as EasActive
/// again). With it a second claim waits for the first, then reads the row it wrote: taken, so busy.
fn claim_lease(facade: &EntityFacade, account_id: &str, renew: bool) -> Result<bool, LibError> {
    let now = now_millis();
    let me = instance_name();
    force_new(facade, || {
        let mut find = facade
            .find(ACCOUNT)?
            .eq("emailAccountId", Value::string(account_id))
            .for_update(true);
        let free = Condition::or(vec![
            Condition::is_null("syncLockedUntil"),
            Condition::less("syncLockedUntil", stamp_millis(now)),
        ]);
        find = if renew {
            find.condition(Condition::or(vec![
                free,
                Condition::equals("syncLockedBy", Value::string(&me)),
            ]))
        } else {
            find.condition(free)
        };
        let count = find.update_all([
            ("syncLockedUntil".to_string(), stamp_millis(now + LEASE_MILLIS)),
            ("syncLockedBy".to_string(), Value::string(&me)),
        ])?;
        Ok(count > 0)
    })
}

fn release_lease(facade: &EntityFacade, account_id: &str) {
    let released = force_new(facade, || {
        facade
            .find(ACCOUNT)?
            .eq("emailAccountId", Value::string(account_id))
            .for_update(true)
            .update_all([
                ("syncLockedUntil".to_string(), Value::Null),
                ("syncLockedBy".to_string(), Value::Null),
            ])
            .map(|_| ())
    });
    if let Err(err) = released {
        tracing::error!("Could not release the sync lock of email account {account_id}: {err}");
    }
}

/// One sync session of an account, failure tolerant: what happened is the report, never an error.
pub(super) fn run_sync(facade: &EntityFacade, account_id: &str, options: &SyncOptions) -> SyncReport {
    let account = match load_account(facade, account_id) {
        Ok(Some(account)) => account,
        Ok(None) => return SyncReport::with("failed", format!("Email account {account_id} not found")),
        Err(err) => return SyncReport::with("failed", format!("Email account {account_id}: {err}")),
    };
    if account.kind == "EakInternal" {
        return SyncReport::with("skipped", "The Internal account has no server");
    }
    let Some(server) = account.server.clone() else {
        return SyncReport::with(
            "skipped",
            format!("Email account {account_id} has no incoming (IMAP or POP3) server"),
        );
    };
    if !server.is_imap() && !server.is_pop() {
        return SyncReport::with("failed", format!("storeProtocol {} is not supported", server.protocol));
    }
    match account.status.as_str() {
        "EasRemoved" => return SyncReport::with("skipped", format!("Email account {account_id} is removed")),
        "EasPaused" => return SyncReport::with("paused", format!("Email account {account_id} is paused")),
        "EasAuthFailed" if !options.by_user => {
            return SyncReport::with("skipped", format!("Email account {account_id} needs its sign-in fixed"))
        }
        _ => {}
    }
    match claim_lease(facade, account_id, false) {
        Ok(true) => {}
        Ok(false) => return SyncReport::with("busy", format!("Email account {account_id} is syncing already")),
        Err(err) => return SyncReport::with("failed", format!("Could not lock email account {account_id}: {err}")),
    }
    let mut report = SyncReport {
        outcome: "synced",
        ..SyncReport::default()
    };
    let started = std::time::Instant::now();
    let mut progress = Progress::new(facade, account_id);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if server.is_pop() {
            if options.push_only {
                Ok(())
            } else {
                pop_sync(facade, &account, &server, &mut report, &mut progress)
            }
        } else {
            imap_sync(facade, &account, &server, options, &mut report, &mut progress)
        }
    }))
    .unwrap_or_else(|payload| {
        Err(SyncFailure::Connect(format!(
            "the sync stopped: {}",
            crate::service::pool::panic_text(payload.as_ref())
        )))
    });
    record_outcome(facade, &account, &result, &mut report, options);
    set_backlog(account_id, report.capped && report.outcome == "synced");
    release_lease(facade, account_id);
    progress.finish(&report);
    tracing::info!(
        account = %account_id,
        outcome = report.outcome,
        new = report.new_count,
        updated = report.updated_count,
        removed = report.removed_count,
        pushed = report.pushed_count,
        "Email sync of account {account_id} in {}ms",
        started.elapsed().as_millis()
    );
    if report.new_count + report.updated_count + report.removed_count > 0 {
        publish_counts(account_id);
    }
    report
}

fn record_outcome(
    facade: &EntityFacade,
    account: &AccountInfo,
    result: &Result<(), SyncFailure>,
    report: &mut SyncReport,
    options: &SyncOptions,
) {
    let mut fields: Vec<(&str, Value)> = Vec::new();
    match result {
        Ok(()) => {
            if !options.push_only {
                fields.push(("lastSyncDate", stamp_millis(now_millis())));
                fields.push((
                    "lastSyncError",
                    if report.folder_errors.is_empty() {
                        Value::Null
                    } else {
                        Value::string(clip(&report.folder_errors.join("\n"), 4000))
                    },
                ));
                report.message = if report.folder_errors.is_empty() {
                    format!("Synced {} new messages", report.new_count)
                } else {
                    format!(
                        "Synced {} new messages; {} folders failed",
                        report.new_count,
                        report.folder_errors.len()
                    )
                };
            }
            if matches!(account.status.as_str(), "EasError" | "EasAuthFailed") {
                fields.push(("statusId", Value::string("EasActive")));
            }
        }
        Err(SyncFailure::Auth(text)) => {
            tracing::error!(account = %account.id, "Email account {} sign-in failed: {text}", account.id);
            report.outcome = "authFailed";
            report.message = text.clone();
            fields.push(("statusId", Value::string("EasAuthFailed")));
            fields.push(("lastSyncError", Value::string(clip(text, 4000))));
        }
        // the token store marked the account and logged it
        Err(SyncFailure::SignIn(text)) => {
            report.outcome = "authFailed";
            report.message = text.clone();
            fields.push(("statusId", Value::string("EasAuthFailed")));
            fields.push(("lastSyncError", Value::string(clip(text, 4000))));
        }
        Err(SyncFailure::Connect(text)) => {
            tracing::error!(account = %account.id, "Email account {} sync failed: {text}", account.id);
            report.outcome = "failed";
            report.message = text.clone();
            if !options.push_only {
                fields.push(("statusId", Value::string("EasError")));
                fields.push(("lastSyncError", Value::string(clip(text, 4000))));
            }
        }
    }
    if fields.is_empty() {
        return;
    }
    let written = force_new(facade, || {
        let mut row = facade.make_value(ACCOUNT)?;
        row.set("emailAccountId", Value::string(&account.id));
        for (name, value) in fields {
            row.set(name, value);
        }
        row.update(facade)
    });
    if let Err(err) = written {
        tracing::error!("Could not record the sync of email account {}: {err}", account.id);
    }
}

pub(super) fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        text.chars().take(max).collect()
    }
}

/// The topic of a sync's live progress (`NotificationMessage`, not persisted, no alert).
pub(super) const PROGRESS_TOPIC: &str = "emailSyncProgress";
/// At most one progress message of a sync in this time (and one when it ends).
const PROGRESS_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// A sync's live progress for a screen ("Syncing 1,204 of 8,000"), sent on [`PROGRESS_TOPIC`] to the account's users
/// (its owner and current members, as `publish#AccountCounts` finds them): `{emailAccountId, done, folderCount,
/// foldersDone, folderName, folderPath, folderKept, folderFetched, folderToFetch, folderPending, fetched, toFetch,
/// pending}`, and when it ends also `outcome`, `message` and `newCount`. A session fetches at most `email-client` `sync-batch`
/// messages a folder (`folderToFetch`); `folderPending` is all the new mail of the folder in the sync window, the rest
/// coming in later sessions, and `folderKept` what was kept of it before, so a first sync shows "{folderKept +
/// folderFetched} of {folderKept + folderPending}" across its sessions. `toFetch` and `pending` add up the folders read
/// so far, so they grow folder by folder. Throttled to one a second; nothing is sent until a folder has mail to
/// fetch, so a sync with nothing new sends nothing, and the last message (done) only after one was sent. Only while a
/// notification socket or a cluster peer can take it, as the shell counts.
pub(super) struct Progress {
    account_id: String,
    on: bool,
    users: Option<Vec<String>>,
    last: Option<std::time::Instant>,
    sent_any: bool,
    folder_count: usize,
    folders_done: usize,
    folder_name: String,
    folder_path: String,
    folder_kept: usize,
    folder_fetched: usize,
    folder_to_fetch: usize,
    folder_pending: usize,
    fetched: usize,
    to_fetch: usize,
    pending: usize,
}

impl Progress {
    pub(super) fn new(facade: &EntityFacade, account_id: &str) -> Self {
        let shutting_down = ExecutionContext::with_current(|ec| ec.factory().is_shutting_down()).unwrap_or(true);
        let on = crate::service::shell::would_deliver(
            shutting_down,
            crate::notify::has_listeners(crate::notify::NOTIFICATION_MESSAGE),
            crate::cluster::is_linked(facade),
        );
        Self {
            account_id: account_id.to_string(),
            on,
            users: None,
            last: None,
            sent_any: false,
            folder_count: 0,
            folders_done: 0,
            folder_name: String::new(),
            folder_path: String::new(),
            folder_kept: 0,
            folder_fetched: 0,
            folder_to_fetch: 0,
            folder_pending: 0,
            fetched: 0,
            to_fetch: 0,
            pending: 0,
        }
    }

    pub(super) fn folders(&mut self, count: usize) {
        self.folder_count = count;
    }

    pub(super) fn start_folder(&mut self, name: &str, path: &str) {
        self.folder_name = name.to_string();
        self.folder_path = path.to_string();
        self.folder_kept = 0;
        self.folder_fetched = 0;
        self.folder_to_fetch = 0;
        self.folder_pending = 0;
    }

    /// The new mail the current folder brings: `count` in this session, `pending` in all (the sessions after this one
    /// bring the rest), with `kept` messages of it kept before.
    pub(super) fn new_mail(&mut self, count: usize, pending: usize, kept: usize) {
        self.folder_to_fetch = count;
        self.folder_pending = pending.max(count);
        self.folder_kept = kept;
        self.to_fetch += count;
        self.pending += pending.max(count);
        self.send(false, None);
    }

    pub(super) fn fetched_one(&mut self) {
        self.folder_fetched += 1;
        self.fetched += 1;
        self.send(false, None);
    }

    pub(super) fn end_folder(&mut self) {
        self.folders_done += 1;
        self.send(false, None);
    }

    pub(super) fn finish(&mut self, report: &SyncReport) {
        if self.sent_any {
            self.send(true, Some(report));
        }
    }

    fn message(&self, report: Option<&SyncReport>) -> indexmap::IndexMap<String, Value> {
        let count = |number: usize| Value::I64(i64::try_from(number).unwrap_or(i64::MAX));
        let mut map = indexmap::IndexMap::new();
        map.insert("emailAccountId".to_string(), Value::string(&self.account_id));
        map.insert("done".to_string(), Value::Bool(report.is_some()));
        map.insert("folderCount".to_string(), count(self.folder_count));
        map.insert("foldersDone".to_string(), count(self.folders_done));
        map.insert("folderName".to_string(), Value::string(&self.folder_name));
        map.insert("folderPath".to_string(), Value::string(&self.folder_path));
        map.insert("folderKept".to_string(), count(self.folder_kept));
        map.insert("folderFetched".to_string(), count(self.folder_fetched));
        map.insert("folderToFetch".to_string(), count(self.folder_to_fetch));
        map.insert("folderPending".to_string(), count(self.folder_pending));
        map.insert("fetched".to_string(), count(self.fetched));
        map.insert("toFetch".to_string(), count(self.to_fetch.max(self.fetched)));
        map.insert("pending".to_string(), count(self.pending.max(self.fetched)));
        if let Some(report) = report {
            map.insert("outcome".to_string(), Value::string(report.outcome));
            map.insert("message".to_string(), Value::string(&report.message));
            map.insert("newCount".to_string(), count(report.new_count));
        }
        map
    }

    /// The account's owner and current members, once.
    fn users(&mut self) -> &[String] {
        if self.users.is_none() {
            let found = account_users(&self.account_id).unwrap_or_else(|err| {
                tracing::warn!("Could not read the users of email account {}: {err}", self.account_id);
                Vec::new()
            });
            self.users = Some(found);
        }
        self.users.as_deref().unwrap_or_default()
    }

    fn send(&mut self, last: bool, report: Option<&SyncReport>) {
        if !self.on || (!last && self.to_fetch == 0) {
            return;
        }
        let now = std::time::Instant::now();
        if !last && self.last.is_some_and(|at| now.duration_since(at) < PROGRESS_EVERY) {
            return;
        }
        let message = self.message(report);
        let users = self.users().to_vec();
        if users.is_empty() {
            return;
        }
        self.last = Some(now);
        self.sent_any = true;
        let sent = crate::notify::NotificationMessage::new()
            .topic(PROGRESS_TOPIC)
            .user_ids(users)
            .message_map(message)
            .persist(false)
            .show_alert(false)
            .send();
        if let Err(err) = sent {
            tracing::warn!(
                "Could not send the sync progress of email account {}: {err}",
                self.account_id
            );
        }
    }
}

/// The users of an account: its owner and its current members.
fn account_users(account_id: &str) -> Result<Vec<String>, LibError> {
    let Some(facade) = ExecutionContext::with_current(|ec| ec.factory().entity()).flatten() else {
        return Ok(Vec::new());
    };
    let mut users: Vec<String> = Vec::new();
    if let Some(account) = find_one(&facade, ACCOUNT, "emailAccountId", account_id)? {
        let owner = field_text(&account, "ownerUserId");
        if !owner.is_empty() {
            users.push(owner);
        }
    }
    let now = stamp_millis(now_millis());
    for row in facade
        .find("moqui.basic.email.EmailAccountUser")?
        .eq("emailAccountId", Value::string(account_id))
        .condition(Condition::or(vec![
            Condition::is_null("thruDate"),
            Condition::greater("thruDate", now.clone()),
        ]))
        .list()?
        .values()
    {
        let user = field_text(row, "userId");
        if !user.is_empty() && !users.contains(&user) {
            users.push(user);
        }
    }
    Ok(users)
}

/// The unread counts of the account's users changed.
pub(super) fn publish_counts(account_id: &str) {
    let mut parameters = BTreeMap::new();
    parameters.insert(
        "emailAccountIdList".to_string(),
        Value::List(vec![Value::string(account_id)]),
    );
    if let Err(err) = crate::service::sync(&format!("{SERVICES}.publish#AccountCounts"), &parameters) {
        tracing::warn!("Could not publish the counts of email account {account_id}: {err}");
    }
    let _ = take_errors_string();
}

/// What a sync session knows as it goes.
struct Session<'a> {
    facade: &'a EntityFacade,
    account: &'a AccountInfo,
    /// Messages with local changes still queued: the server's flags do not replace theirs, and they are not removed.
    pending_local: HashSet<String>,
    /// Rows whose message left its folder in this session, by emailMessageId, with the folder: claimed by Message-ID
    /// if the message shows up in another folder, else deleted at the end.
    vanished: HashMap<String, String>,
    /// Folders not read to the end in this session (the batch was full, or a message could not be stored): what left
    /// another folder may still be found in one, so a row with a Message-ID that left is kept for the next session.
    incomplete: HashSet<String>,
    /// Rows of folders gone from the server, which a message may claim but which are not deleted.
    claim_only: HashSet<String>,
    /// emailMessageIds by Message-ID, of `vanished` and `claim_only`.
    vanished_by_mid: HashMap<String, Vec<String>>,
    /// New UIDs whose Message-ID is a row of another folder that has not left it (yet).
    pending: Vec<PendingCopy>,
    new_inbox_ids: Vec<String>,
    /// SEARCH SINCE date, and its epoch milliseconds.
    window: Option<(String, i64)>,
    batch: usize,
    flag_scan_millis: i64,
    /// Label names by keyword, for the labels kept as IMAP keywords.
    labels: Option<HashMap<String, String>>,
}

struct PendingCopy {
    folder: FolderInfo,
    uid: u32,
    flags: FlagSet,
    internal: Option<i64>,
    size: Option<i64>,
    existing: Vec<String>,
}

/// A row the sync keeps for a folder and UID.
#[derive(Clone, Debug)]
struct LocalRow {
    id: String,
    message_id: String,
    read: bool,
    flagged: bool,
    answered: bool,
}

enum FolderError {
    /// The connection is gone: the session ends.
    Lost(String),
    /// This folder failed; the others go on.
    Folder(String),
}

impl From<imap::Error> for FolderError {
    fn from(err: imap::Error) -> Self {
        if connection_lost(&err) {
            Self::Lost(err.to_string())
        } else {
            Self::Folder(err.to_string())
        }
    }
}

impl From<LibError> for FolderError {
    fn from(err: LibError) -> Self {
        Self::Folder(err.to_string())
    }
}

/// The conf's `email-client` attribute `name` as a positive number, else `default`.
fn property_number(name: &str, default: i64) -> i64 {
    super::client_setting(name)
        .parse::<i64>()
        .ok()
        .filter(|number| *number > 0)
        .unwrap_or(default)
}

/// `SINCE` dates are day-month-year with the English month: `17-Jul-2026`.
fn imap_date(millis: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let date = jiff::Timestamp::from_millisecond(millis)
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
        .to_zoned(jiff::tz::TimeZone::UTC)
        .date();
    let month = MONTHS[usize::try_from(date.month() - 1).unwrap_or(0).min(11)];
    format!("{}-{month}-{}", date.day(), date.year())
}

impl<'a> Session<'a> {
    fn new(facade: &'a EntityFacade, account: &'a AccountInfo) -> Self {
        let window = account.window_days.map(|days| {
            let start = now_millis() - days * 86_400_000;
            (imap_date(start), start)
        });
        Self {
            facade,
            account,
            pending_local: HashSet::new(),
            vanished: HashMap::new(),
            incomplete: HashSet::new(),
            claim_only: HashSet::new(),
            vanished_by_mid: HashMap::new(),
            pending: Vec::new(),
            new_inbox_ids: Vec::new(),
            window,
            batch: usize::try_from(property_number("sync-batch", 500)).unwrap_or(500),
            flag_scan_millis: property_number("flag-scan", 15) * 60_000,
            labels: None,
        }
    }

    fn add_vanished(&mut self, row: &LocalRow, folder_id: &str, claim_only: bool) {
        if self.pending_local.contains(&row.id) {
            return;
        }
        if claim_only {
            self.claim_only.insert(row.id.clone());
        } else {
            self.vanished.insert(row.id.clone(), folder_id.to_string());
        }
        if !row.message_id.is_empty() {
            self.vanished_by_mid
                .entry(row.message_id.clone())
                .or_default()
                .push(row.id.clone());
        }
    }

    /// Take a left row with this Message-ID, if any.
    fn take_vanished(&mut self, message_id: &str) -> Option<String> {
        let ids = self.vanished_by_mid.get_mut(message_id)?;
        let position = ids
            .iter()
            .position(|id| self.vanished.contains_key(id) || self.claim_only.contains(id))?;
        let id = ids.remove(position);
        self.vanished.remove(&id);
        self.claim_only.remove(&id);
        Some(id)
    }

    fn label_keywords(&mut self) -> Result<&HashMap<String, String>, LibError> {
        if self.labels.is_none() {
            let rows = self
                .facade
                .find("moqui.basic.email.EmailLabel")?
                .eq("emailAccountId", Value::string(&self.account.id))
                .list()?;
            let mut map = HashMap::new();
            for row in rows.values() {
                map.insert(
                    label_keyword(&field_text(row, "labelName")),
                    field_text(row, "emailLabelId"),
                );
            }
            self.labels = Some(map);
        }
        Ok(self.labels.get_or_insert_with(HashMap::new))
    }
}

fn imap_sync(
    facade: &EntityFacade,
    account: &AccountInfo,
    server: &ServerInfo,
    options: &SyncOptions,
    report: &mut SyncReport,
    progress: &mut Progress,
) -> Result<(), SyncFailure> {
    let mut conn = connect_imap(facade, server)?;
    if conn.caps.has("QRESYNC") && conn.caps.has("ENABLE") {
        if conn.session.run_command_and_check_ok("ENABLE QRESYNC").is_ok() {
            conn.qresync = true;
            conn.condstore = true;
        }
    } else if conn.caps.has("CONDSTORE") {
        if conn.caps.has("ENABLE") {
            let _ = conn.session.run_command_and_check_ok("ENABLE CONDSTORE");
        }
        conn.condstore = true;
    }
    let lost = |err: LibError| SyncFailure::Connect(err.to_string());
    // 1. what was changed here goes there first, so the server's state read next has it
    report.pushed_count += super::remote::apply_queue(&mut conn, facade, account)?;
    if options.push_only {
        let _ = conn.session.logout();
        return Ok(());
    }
    // 2. the folders
    let mut folders = if options.inbox_only {
        load_folders(facade, &account.id).map_err(lost)?
    } else {
        match list_folders(&mut conn, facade, account) {
            Ok(folders) => folders,
            Err(FolderError::Lost(text)) => return Err(SyncFailure::Connect(text)),
            Err(FolderError::Folder(text)) => {
                tracing::error!(account = %account.id, "Could not list the folders of email account {}: {text}", account.id);
                report.folder_errors.push(format!("LIST: {text}"));
                load_folders(facade, &account.id).map_err(lost)?
            }
        }
    };
    let mut session = Session::new(facade, account);
    session.pending_local = super::remote::pending_message_ids(facade, &account.id).map_err(lost)?;
    // the rows of folders gone from the server may be claimed by a message that moved out of them
    for folder in folders.iter().filter(|folder| folder.removed) {
        for row in local_rows(facade, &folder.id).map_err(lost)?.values() {
            session.add_vanished(row, &folder.id, true);
        }
    }
    folders.retain(|folder| folder.subscribed && !folder.removed && !folder.path.is_empty());
    folders.sort_by(|left, right| {
        kind_rank(&left.kind)
            .cmp(&kind_rank(&right.kind))
            .then(left.sequence.cmp(&right.sequence))
            .then(left.path.cmp(&right.path))
    });
    progress.folders(folders.len());
    for (index, folder) in folders.iter().enumerate() {
        if options.inbox_only && index > 0 && folder.kind != "EfkInbox" && session.vanished.is_empty() {
            break;
        }
        if index > 0 && claim_lease(facade, &account.id, true).is_err() {
            tracing::warn!("Could not renew the sync lock of email account {}", account.id);
        }
        progress.start_folder(&folder.name, &folder.path);
        let synced = sync_folder(&mut conn, &mut session, folder, report, progress);
        progress.end_folder();
        match synced {
            Ok(()) => report.folder_count += 1,
            Err(FolderError::Lost(text)) => return Err(SyncFailure::Connect(text)),
            Err(FolderError::Folder(text)) => {
                session.incomplete.insert(folder.id.clone());
                tracing::error!(
                    account = %account.id,
                    folder = %folder.path,
                    "Email folder {} of account {} failed to sync: {text}",
                    folder.path,
                    account.id
                );
                report.folder_errors.push(format!("{}: {text}", folder.path));
            }
        }
    }
    // 3. copies, and what left and went nowhere
    resolve_pending(&mut conn, &mut session, report)?;
    remove_vanished(&mut session, report);
    // 4. the rules, on the new mail of the Inbox, then what new mail brings: a snoozed thread back, the automatic reply
    run_rules(&account.id, &session.new_inbox_ids);
    after_new_mail(&account.id, &session.new_inbox_ids);
    // 5. what the rules and the user changed meanwhile
    report.pushed_count += super::remote::apply_queue(&mut conn, facade, account)?;
    let _ = conn.session.logout();
    Ok(())
}

// ---- LIST ----

/// The kind a server folder maps to: INBOX and the special-use attributes, else a usual name.
fn remote_kind(path: &str, attributes: &[String], delimiter: &str) -> &'static str {
    let kind = folder_kind(path, attributes);
    if kind != "EfkOther" {
        return kind;
    }
    let lower: Vec<String> = attributes
        .iter()
        .map(|attribute| attribute.to_ascii_lowercase())
        .collect();
    if lower.iter().any(|attribute| attribute == "\\all") {
        return "EfkArchive";
    }
    let segments: Vec<&str> = if delimiter.is_empty() {
        vec![path]
    } else {
        path.split(delimiter).collect()
    };
    // a top folder, or one under INBOX (Courier and Dovecot name them INBOX.Sent)
    let name = match segments.as_slice() {
        [name] => *name,
        [inbox, name] if inbox.eq_ignore_ascii_case("INBOX") => *name,
        _ => return "EfkOther",
    };
    match decode_mutf7(name).to_lowercase().as_str() {
        "sent" | "sent items" | "sent messages" | "sent mail" => "EfkSent",
        "drafts" | "draft" => "EfkDrafts",
        "trash" | "deleted" | "deleted items" | "deleted messages" | "bin" => "EfkTrash",
        "junk" | "spam" | "junk e-mail" | "junk email" | "bulk mail" => "EfkJunk",
        "archive" | "archives" => "EfkArchive",
        _ => "EfkOther",
    }
}

struct RemoteFolder {
    path: String,
    delimiter: String,
    attributes: Vec<String>,
}

impl RemoteFolder {
    fn has(&self, attribute: &str) -> bool {
        self.attributes
            .iter()
            .any(|known| known.eq_ignore_ascii_case(attribute))
    }

    fn display_name(&self) -> String {
        let last = if self.delimiter.is_empty() {
            self.path.as_str()
        } else {
            self.path.rsplit(self.delimiter.as_str()).next().unwrap_or(&self.path)
        };
        decode_mutf7(last)
    }

    fn parent_path(&self) -> Option<String> {
        if self.delimiter.is_empty() {
            return None;
        }
        self.path
            .rsplit_once(self.delimiter.as_str())
            .map(|(parent, _)| parent.to_string())
    }
}

fn list_folders(
    conn: &mut ImapConn,
    facade: &EntityFacade,
    account: &AccountInfo,
) -> Result<Vec<FolderInfo>, FolderError> {
    let names = conn.session.list(Some(""), Some("*"))?;
    let mut remote: Vec<RemoteFolder> = names
        .iter()
        .map(|name| RemoteFolder {
            path: name.name().to_string(),
            delimiter: name.delimiter().unwrap_or_default().to_string(),
            attributes: name
                .attributes()
                .iter()
                .map(|attribute| name_attribute(&format!("{attribute:?}")))
                .collect(),
        })
        .collect();
    drop(names);
    let subscribed: HashSet<String> = match conn.session.lsub(Some(""), Some("*")) {
        Ok(names) => names.iter().map(|name| name.name().to_string()).collect(),
        Err(err) if connection_lost(&err) => return Err(err.into()),
        Err(_) => HashSet::new(),
    };
    // parents before their children
    remote.sort_by(|left, right| {
        let depth = |folder: &RemoteFolder| {
            if folder.delimiter.is_empty() {
                0
            } else {
                folder.path.matches(folder.delimiter.as_str()).count()
            }
        };
        depth(left).cmp(&depth(right)).then(left.path.cmp(&right.path))
    });
    force_new(facade, || map_folders(facade, account, &remote, &subscribed))?;
    Ok(load_folders(facade, &account.id)?)
}

/// The server's folders onto the account's rows, in one transaction.
fn map_folders(
    facade: &EntityFacade,
    account: &AccountInfo,
    remote: &[RemoteFolder],
    subscribed: &HashSet<String>,
) -> Result<(), LibError> {
    let mut locals = load_folders(facade, &account.id)?;
    let selectable: Vec<&RemoteFolder> = remote
        .iter()
        .filter(|folder| !folder.has("\\Noselect") && !folder.has("\\NonExistent"))
        .collect();
    let has_archive = selectable.iter().any(|folder| folder.has("\\Archive"));
    let mut seen_paths = HashSet::new();
    let mut next_sequence = locals
        .iter()
        .map(|folder| folder.sequence)
        .filter(|seq| *seq < 999_999)
        .max()
        .unwrap_or(0);
    for folder in &selectable {
        seen_paths.insert(folder.path.clone());
        let mut kind = remote_kind(&folder.path, &folder.attributes, &folder.delimiter);
        if kind == "EfkArchive" && folder.has("\\All") && has_archive {
            kind = "EfkOther";
        }
        // Gmail's views of mail that is in other folders too
        let virtual_folder = folder.has("\\All") || folder.has("\\Flagged") || folder.has("\\Important");
        if let Some(local) = locals.iter_mut().find(|local| local.path == folder.path) {
            let mut fields: Vec<(&str, Value)> = Vec::new();
            if local.removed {
                fields.push(("remoteRemovedDate", Value::Null));
                local.removed = false;
            }
            if local.delimiter != folder.delimiter && !folder.delimiter.is_empty() {
                fields.push(("delimiter", Value::string(&folder.delimiter)));
            }
            if !fields.is_empty() {
                update_folder(facade, &local.id, &fields)?;
            }
            continue;
        }
        // a kind is taken by a folder mapped to the server, or by the folder a user chose for it, wherever it is
        let taken_kind = |locals: &[FolderInfo]| {
            locals
                .iter()
                .any(|local| local.kind == kind && (local.kind_by_user || (!local.path.is_empty() && !local.removed)))
        };
        if kind != "EfkOther" && !taken_kind(&locals) {
            if let Some(local) = locals
                .iter_mut()
                .find(|local| local.kind == kind && local.path.is_empty())
            {
                update_folder(
                    facade,
                    &local.id,
                    &[
                        ("folderPath", Value::string(&folder.path)),
                        ("delimiter", Value::string(&folder.delimiter)),
                    ],
                )?;
                local.path.clone_from(&folder.path);
                continue;
            }
        }
        let name = folder.display_name();
        let parent_id = folder
            .parent_path()
            .and_then(|parent| locals.iter().find(|local| local.path == parent))
            .map(|local| local.id.clone())
            .unwrap_or_default();
        if let Some(local) = locals.iter_mut().find(|local| {
            local.path.is_empty() && local.parent_id == parent_id && local.name.eq_ignore_ascii_case(&name)
        }) {
            update_folder(
                facade,
                &local.id,
                &[
                    ("folderPath", Value::string(&folder.path)),
                    ("delimiter", Value::string(&folder.delimiter)),
                ],
            )?;
            local.path.clone_from(&folder.path);
            continue;
        }
        let kind = if kind != "EfkOther" && taken_kind(&locals) {
            "EfkOther"
        } else {
            kind
        };
        let subscribe =
            kind == "EfkInbox" || (!virtual_folder && (subscribed.is_empty() || subscribed.contains(&folder.path)));
        next_sequence += 1;
        let mut row = facade.make_value(FOLDER)?;
        row.set_sequenced_id_primary(facade)?;
        let id = field_text(&row, "emailFolderId");
        row.set("emailAccountId", Value::string(&account.id));
        row.set("folderName", Value::string(&name));
        row.set("folderPath", Value::string(&folder.path));
        row.set("delimiter", Value::string(&folder.delimiter));
        row.set(
            "parentFolderId",
            if parent_id.is_empty() {
                Value::Null
            } else {
                Value::string(&parent_id)
            },
        );
        row.set("folderKindEnumId", Value::string(kind));
        row.set("subscribed", Value::string(if subscribe { "Y" } else { "N" }));
        row.set("sequenceNum", Value::I64(next_sequence));
        row.create(facade)?;
        locals.push(FolderInfo {
            id,
            name,
            path: folder.path.clone(),
            parent_id,
            kind: kind.to_string(),
            delimiter: folder.delimiter.clone(),
            subscribed: subscribe,
            sequence: next_sequence,
            ..FolderInfo::default()
        });
    }
    let now = stamp_millis(now_millis());
    for local in &locals {
        if !local.path.is_empty() && !local.removed && !seen_paths.contains(&local.path) {
            tracing::info!(
                account = %account.id,
                "Email folder {} of account {} is no longer on the server",
                local.path,
                account.id
            );
            update_folder(facade, &local.id, &[("remoteRemovedDate", now.clone())])?;
        }
    }
    Ok(())
}

pub(super) fn update_folder(facade: &EntityFacade, folder_id: &str, fields: &[(&str, Value)]) -> Result<(), LibError> {
    let mut row = facade.make_value(FOLDER)?;
    row.set("emailFolderId", Value::string(folder_id));
    for (name, value) in fields {
        row.set(*name, value.clone());
    }
    row.update(facade)
}

// ---- one folder ----

/// The rows kept under a folder's UIDs.
fn local_rows(facade: &EntityFacade, folder_id: &str) -> Result<HashMap<u32, LocalRow>, LibError> {
    let rows = facade
        .find(MESSAGE)?
        .eq("remoteFolderId", Value::string(folder_id))
        .condition(Condition::is_not_null("remoteUid"))
        .select_field("emailMessageId")
        .select_field("remoteUid")
        .select_field("messageId")
        .select_field("isRead")
        .select_field("isFlagged")
        .select_field("isAnswered")
        .list()?;
    let mut out = HashMap::new();
    for row in rows.values() {
        let Some(uid) = as_u32(row.get("remoteUid")) else {
            continue;
        };
        out.insert(
            uid,
            LocalRow {
                id: field_text(row, "emailMessageId"),
                message_id: field_text(row, "messageId"),
                read: field_text(row, "isRead") == "Y",
                flagged: field_text(row, "isFlagged") == "Y",
                answered: field_text(row, "isAnswered") == "Y",
            },
        );
    }
    Ok(out)
}

fn search(conn: &mut ImapConn, query: &str) -> Result<Vec<u32>, imap::Error> {
    let mut uids: Vec<u32> = conn.session.uid_search(query)?.into_iter().collect();
    uids.sort_unstable();
    Ok(uids)
}

/// What a new UID is to the rows already kept.
enum Decision {
    /// This row is the message: it moved here, or it is a local copy waiting for its UID.
    Claim(String, bool),
    /// Rows in other folders have its Message-ID and have not left them: decided at the end of the session.
    Pending(Vec<String>),
    Store,
}

fn decide(session: &mut Session<'_>, folder: &FolderInfo, message_id: &str) -> Result<Decision, LibError> {
    if message_id.is_empty() {
        return Ok(Decision::Store);
    }
    if let Some(id) = session.take_vanished(message_id) {
        return Ok(Decision::Claim(id, true));
    }
    let rows = session
        .facade
        .find(MESSAGE)?
        .eq("emailAccountId", Value::string(&session.account.id))
        .eq("messageId", Value::string(message_id))
        .select_field("emailMessageId")
        .select_field("remoteFolderId")
        .select_field("remoteUid")
        .select_field("emailFolderId")
        .list()?;
    let mut others = Vec::new();
    for row in rows.values() {
        let id = field_text(row, "emailMessageId");
        let remote_folder = field_text(row, "remoteFolderId");
        let local_folder = field_text(row, "emailFolderId");
        let has_uid = row.get("remoteUid").is_some_and(|value| !value.is_null());
        if !has_uid {
            let here = if remote_folder.is_empty() {
                local_folder.is_empty() || local_folder == folder.id
            } else {
                remote_folder == folder.id
            };
            if here {
                return Ok(Decision::Claim(id, false));
            }
        } else if remote_folder != folder.id {
            others.push(id);
        }
    }
    if others.is_empty() {
        Ok(Decision::Store)
    } else {
        Ok(Decision::Pending(others))
    }
}

/// A row takes a UID in a folder: a message another client moved here (`moved`: it is in this folder here too, with
/// the server's flags), or a local copy waiting for its UID.
fn claim(
    session: &Session<'_>,
    row_id: &str,
    folder: &FolderInfo,
    uid: u32,
    flags: &FlagSet,
    moved: bool,
) -> Result<(), LibError> {
    let mut fields = vec![
        ("remoteFolderId", Value::string(&folder.id)),
        ("remoteUid", Value::I64(i64::from(uid))),
    ];
    if moved {
        fields.push(("emailFolderId", Value::string(&folder.id)));
        fields.push((
            "archivedDate",
            if folder.kind == "EfkArchive" {
                stamp_millis(now_millis())
            } else {
                Value::Null
            },
        ));
        if !session.pending_local.contains(row_id) {
            fields.push(("isRead", Value::string(if flags.seen { "Y" } else { "N" })));
            fields.push(("isFlagged", Value::string(if flags.flagged { "Y" } else { "N" })));
            fields.push(("isAnswered", Value::string(if flags.answered { "Y" } else { "N" })));
        }
    }
    force_new(session.facade, || update_fields(session.facade, row_id, &fields))
}

fn sync_folder(
    conn: &mut ImapConn,
    session: &mut Session<'_>,
    folder: &FolderInfo,
    report: &mut SyncReport,
    progress: &mut Progress,
) -> Result<(), FolderError> {
    let items = if conn.condstore {
        "(MESSAGES UIDNEXT UIDVALIDITY UNSEEN HIGHESTMODSEQ)"
    } else {
        "(MESSAGES UIDNEXT UIDVALIDITY UNSEEN)"
    };
    let status = match conn.session.status(&folder.path, items) {
        Ok(status) => Some(status),
        Err(err) if connection_lost(&err) => return Err(err.into()),
        Err(_) => None,
    };
    let now = now_millis();
    let uses_modseq = conn.condstore && folder.modseq.is_some();
    let scan_due = !uses_modseq
        && folder
            .last_scan
            .is_none_or(|last| now - last >= session.flag_scan_millis);
    let unchanged = status.as_ref().is_some_and(|status| {
        folder.uid_validity.is_some()
            && status.uid_validity == folder.uid_validity
            && status.uid_next == folder.uid_next
            && Some(status.exists) == folder.total
            && (!uses_modseq || status.highest_mod_seq == folder.modseq)
    });
    if unchanged && !scan_due {
        if let Some(unseen) = status.as_ref().and_then(|status| status.unseen) {
            if folder.unread != Some(unseen) {
                force_new(session.facade, || {
                    update_folder(
                        session.facade,
                        &folder.id,
                        &[("unreadCount", Value::I64(i64::from(unseen)))],
                    )
                })?;
            }
        }
        return Ok(());
    }
    let selected = select(conn, &folder.path, true)?;
    let mailbox = selected.mailbox.clone();
    let keywords = selected.keywords();
    let validity = mailbox.uid_validity.unwrap_or(0);
    let mut first = folder.uid_next.is_none() || folder.uid_validity.is_none();
    // on a first sync too: a first sync that stopped part way kept what it stored, which is not stored again
    let mut locals = local_rows(session.facade, &folder.id)?;
    if folder.uid_validity.is_some_and(|known| known != validity) {
        tracing::warn!(
            account = %session.account.id,
            "UIDVALIDITY of email folder {} of account {} changed from {:?} to {validity}: reading it again",
            folder.path,
            session.account.id,
            folder.uid_validity
        );
        // the old UIDs are void at once, so none is taken for a new message's; the rows wait for theirs by Message-ID
        force_new(session.facade, || {
            session
                .facade
                .find(MESSAGE)?
                .eq("remoteFolderId", Value::string(&folder.id))
                .condition(Condition::is_not_null("remoteUid"))
                .update_all([("remoteUid".to_string(), Value::Null)])
                .map(|_| ())
        })?;
        for row in locals.values() {
            session.add_vanished(row, &folder.id, false);
        }
        locals.clear();
        first = true;
    }
    let start = if first { 1 } else { folder.uid_next.unwrap_or(1).max(1) };
    // every UID from the start on (for the expunge count), then those in the sync window
    let all_new: Vec<u32> = if first {
        Vec::new()
    } else {
        search(conn, &format!("UID {start}:*"))?
            .into_iter()
            .filter(|uid| *uid >= start)
            .collect()
    };
    let mut wanted: Vec<u32> = match (&session.window, first) {
        (Some((since, _)), true) => search(conn, &format!("SINCE {since}"))?,
        (None, true) => search(conn, "ALL")?,
        (Some((since, _)), false) if !all_new.is_empty() => search(conn, &format!("UID {start}:* SINCE {since}"))?
            .into_iter()
            .filter(|uid| *uid >= start)
            .collect(),
        _ => all_new.clone(),
    };
    wanted.retain(|uid| !locals.contains_key(uid));
    // the flags and the expunges of the messages kept
    let mut scanned = false;
    if !first && !locals.is_empty() {
        let min_uid = locals.keys().copied().min().unwrap_or(1);
        let mut flags: Vec<(u32, FlagSet)> = Vec::new();
        let mut gone: Vec<u32> = Vec::new();
        let mut count_check = true;
        if uses_modseq && mailbox.highest_mod_seq.is_some() {
            let since = folder.modseq.unwrap_or(0);
            let query = if conn.qresync {
                format!("(UID FLAGS) (CHANGEDSINCE {since} VANISHED)")
            } else {
                format!("(UID FLAGS) (CHANGEDSINCE {since})")
            };
            let fetches = conn.session.uid_fetch("1:*", query)?;
            for fetch in fetches.iter() {
                if let Some(uid) = fetch.uid {
                    flags.push((uid, FlagSet::of(fetch.flags())));
                }
            }
            drop(fetches);
            for response in conn.session.take_all_unsolicited() {
                if let imap::types::UnsolicitedResponse::Vanished { uids, .. } = response {
                    for range in uids {
                        if range.end().saturating_sub(*range.start()) <= 1_000_000 {
                            gone.extend(range.filter(|uid| locals.contains_key(uid)));
                        }
                    }
                }
            }
            count_check = !conn.qresync;
        } else if scan_due {
            let fetches = conn.session.uid_fetch(format!("{min_uid}:*"), "(UID FLAGS)")?;
            let mut present = HashSet::new();
            for fetch in fetches.iter() {
                if let Some(uid) = fetch.uid {
                    present.insert(uid);
                    flags.push((uid, FlagSet::of(fetch.flags())));
                }
            }
            gone.extend(locals.keys().copied().filter(|uid| !present.contains(uid)));
            scanned = true;
            count_check = false;
        }
        if count_check {
            let expected = folder
                .total
                .unwrap_or(0)
                .saturating_add(u32::try_from(all_new.len()).unwrap_or(u32::MAX));
            if mailbox.exists < expected {
                let present: HashSet<u32> = search(conn, &format!("UID {min_uid}:*"))?.into_iter().collect();
                gone.extend(locals.keys().copied().filter(|uid| !present.contains(uid)));
            }
        }
        report.updated_count += apply_flags(session, &locals, &flags, keywords)?;
        gone.sort_unstable();
        gone.dedup();
        for uid in gone {
            if let Some(row) = locals.get(&uid) {
                session.add_vanished(row, &folder.id, false);
            }
        }
    }
    // the new messages
    let capped = wanted.len() > session.batch;
    report.capped |= capped;
    let pending = wanted.len();
    wanted.truncate(session.batch);
    progress.new_mail(wanted.len(), pending, locals.len());
    let mut failed_from: Option<u32> = None;
    for chunk in wanted.chunks(HEADER_CHUNK) {
        let heads = conn.session.uid_fetch(
            uid_set(chunk),
            "(UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)])",
        )?;
        let mut to_fetch: Vec<(u32, FlagSet, Option<i64>, Option<i64>)> = Vec::new();
        let mut heads_read = Vec::new();
        for head in heads.iter() {
            let Some(uid) = head.uid else {
                continue;
            };
            if uid < start || !chunk.contains(&uid) {
                continue;
            }
            let message_id = head
                .header()
                .map(|bytes| header_message_id(&String::from_utf8_lossy(bytes)))
                .unwrap_or_default();
            heads_read.push((
                uid,
                FlagSet::of(head.flags()),
                head.internal_date().map(|date| date.timestamp_millis()),
                head.size.map(i64::from),
                message_id,
            ));
        }
        drop(heads);
        for (uid, flags, internal, size, message_id) in heads_read {
            match decide(session, folder, &message_id)? {
                Decision::Claim(row_id, moved) => {
                    claim(session, &row_id, folder, uid, &flags, moved)?;
                    report.updated_count += 1;
                }
                Decision::Pending(existing) => session.pending.push(PendingCopy {
                    folder: folder.clone(),
                    uid,
                    flags,
                    internal,
                    size,
                    existing,
                }),
                Decision::Store => to_fetch.push((uid, flags, internal, size)),
            }
        }
        for batch in to_fetch.chunks(FETCH_CHUNK) {
            let uids: Vec<u32> = batch.iter().map(|item| item.0).collect();
            let bodies = conn.session.uid_fetch(uid_set(&uids), "(UID BODY.PEEK[])")?;
            let mut raw_by_uid: HashMap<u32, Vec<u8>> = HashMap::new();
            for fetch in bodies.iter() {
                if let (Some(uid), Some(body)) = (fetch.uid, fetch.body()) {
                    raw_by_uid.insert(uid, body.to_vec());
                }
            }
            drop(bodies);
            for (uid, flags, internal, size) in batch {
                let Some(raw) = raw_by_uid.remove(uid) else {
                    tracing::warn!(
                        account = %session.account.id,
                        "Email UID {uid} of folder {} of account {} has no body on the server; skipped",
                        folder.path,
                        session.account.id
                    );
                    report.skipped_messages += 1;
                    continue;
                };
                match store_new(session, folder, *uid, &raw, flags, *internal, *size, keywords) {
                    Ok(id) => {
                        report.new_count += 1;
                        progress.fetched_one();
                        if folder.kind == "EfkInbox" {
                            // a Message-ID new to the account (a copy or a move is not new mail)
                            if eca_due(session.account, *internal) {
                                run_stored_ecas(
                                    session.facade,
                                    &StoredMessage {
                                        raw: &raw,
                                        server_id: session
                                            .account
                                            .server
                                            .as_ref()
                                            .map_or("", |server| &server.server_id),
                                        account_id: &session.account.id,
                                        email_message_id: &id,
                                        received_millis: *internal,
                                        seen: flags.seen,
                                        answered: flags.answered,
                                        flagged: flags.flagged,
                                    },
                                );
                            }
                            session.new_inbox_ids.push(id);
                        }
                    }
                    Err(StoreFailure::Malformed(text)) => {
                        tracing::error!(
                            account = %session.account.id,
                            folder = %folder.path,
                            uid = uid,
                            "Skipping email UID {uid} of folder {} of account {}: {text}",
                            folder.path,
                            session.account.id
                        );
                        report.skipped_messages += 1;
                    }
                    Err(StoreFailure::Write(text)) => {
                        tracing::error!(
                            account = %session.account.id,
                            folder = %folder.path,
                            uid = uid,
                            "Could not store email UID {uid} of folder {} of account {}, read again next time: {text}",
                            folder.path,
                            session.account.id
                        );
                        report.skipped_messages += 1;
                        failed_from = Some(failed_from.map_or(*uid, |known| known.min(*uid)));
                    }
                }
            }
        }
    }
    if capped || failed_from.is_some() {
        session.incomplete.insert(folder.id.clone());
    }
    // where the next session starts
    let next = match failed_from {
        Some(uid) => uid,
        None if capped => wanted.last().map_or(start, |uid| uid + 1),
        None => mailbox.uid_next.unwrap_or_else(|| {
            wanted
                .iter()
                .chain(all_new.iter())
                .copied()
                .max()
                .map_or(start, |uid| uid + 1)
        }),
    };
    let mut fields = vec![
        ("remoteUidValidity", Value::I64(i64::from(validity))),
        ("remoteUidNext", Value::I64(i64::from(next))),
        (
            "remoteHighestModSeq",
            match mailbox.highest_mod_seq.filter(|_| conn.condstore) {
                Some(modseq) => Value::I64(i64::try_from(modseq).unwrap_or(i64::MAX)),
                None => Value::Null,
            },
        ),
        ("totalCount", Value::I64(i64::from(mailbox.exists))),
    ];
    if let Some(unseen) = status.as_ref().and_then(|status| status.unseen) {
        fields.push(("unreadCount", Value::I64(i64::from(unseen))));
    }
    if scanned || first {
        fields.push(("lastFlagScanDate", stamp_millis(now)));
    }
    force_new(session.facade, || update_folder(session.facade, &folder.id, &fields))?;
    Ok(())
}

/// The Message-ID in a `HEADER.FIELDS (MESSAGE-ID)` fetch.
fn header_message_id(headers: &str) -> String {
    let value = store::header_value(headers, "message-id");
    let value = value.trim();
    match (value.find('<'), value.rfind('>')) {
        (Some(start), Some(end)) if start < end => value[start..=end].to_string(),
        _ if !value.is_empty() => format!("<{}>", value.trim_matches(['<', '>'])),
        _ => String::new(),
    }
}

/// The server's flags onto the rows, in one transaction; returns how many changed. A row with a local change queued
/// keeps its own. With keywords kept, a keyword that names a label adds it.
fn apply_flags(
    session: &mut Session<'_>,
    locals: &HashMap<u32, LocalRow>,
    flags: &[(u32, FlagSet)],
    keywords: bool,
) -> Result<usize, LibError> {
    if flags.is_empty() {
        return Ok(0);
    }
    let labels = if keywords {
        session.label_keywords()?.clone()
    } else {
        HashMap::new()
    };
    let facade = session.facade;
    let pending = &session.pending_local;
    force_new(facade, || {
        let mut changed = 0;
        for (uid, set) in flags {
            let Some(row) = locals.get(uid) else {
                continue;
            };
            if pending.contains(&row.id) {
                continue;
            }
            let mut fields = Vec::new();
            if row.read != set.seen {
                fields.push(("isRead", Value::string(if set.seen { "Y" } else { "N" })));
            }
            if row.flagged != set.flagged {
                fields.push(("isFlagged", Value::string(if set.flagged { "Y" } else { "N" })));
            }
            if row.answered != set.answered {
                fields.push(("isAnswered", Value::string(if set.answered { "Y" } else { "N" })));
            }
            let mut touched = !fields.is_empty();
            if !fields.is_empty() {
                update_fields(facade, &row.id, &fields)?;
            }
            if !labels.is_empty() {
                touched |= sync_labels(facade, &row.id, &labels, &set.keywords)?;
            }
            if touched {
                changed += 1;
            }
        }
        Ok(changed)
    })
}

/// The labels the keywords of a message name, added; true when one was. A label is not taken off for a keyword the
/// server lacks: labels made before the sync kept them as keywords were never stored there, and a label taken off here
/// is taken off there by the labels change (push#RemoteChanges).
fn sync_labels(
    facade: &EntityFacade,
    email_message_id: &str,
    labels: &HashMap<String, String>,
    keywords: &[String],
) -> Result<bool, LibError> {
    let have: HashSet<String> = facade
        .find("moqui.basic.email.EmailMessageLabel")?
        .eq("emailMessageId", Value::string(email_message_id))
        .list()?
        .values()
        .iter()
        .map(|row| field_text(row, "emailLabelId"))
        .collect();
    let want: HashSet<String> = keywords
        .iter()
        .filter_map(|keyword| labels.get(keyword).cloned())
        .collect();
    let mut changed = false;
    for label in want.difference(&have) {
        let mut row = facade.make_value("moqui.basic.email.EmailMessageLabel")?;
        row.set("emailMessageId", Value::string(email_message_id));
        row.set("emailLabelId", Value::string(label));
        row.create(facade)?;
        changed = true;
    }
    Ok(changed)
}

enum StoreFailure {
    /// The message is not mail: logged and skipped for good.
    Malformed(String),
    /// It could not be written: read again next session.
    Write(String),
}

#[allow(clippy::too_many_arguments)]
fn store_new(
    session: &mut Session<'_>,
    folder: &FolderInfo,
    uid: u32,
    raw: &[u8],
    flags: &FlagSet,
    internal: Option<i64>,
    size: Option<i64>,
    keywords: bool,
) -> Result<String, StoreFailure> {
    let parsed = parse_message(raw).map_err(StoreFailure::Malformed)?;
    let status = match folder.kind.as_str() {
        "EfkSent" => "ES_SENT",
        "EfkDrafts" => "ES_DRAFT",
        _ => "ES_RECEIVED",
    };
    let place = Placement {
        account_id: &session.account.id,
        folder_id: Some(&folder.id),
        remote_folder_id: Some(&folder.id),
        remote_uid: Some(uid),
        remote_uidl: None,
        status_id: status,
        read: flags.seen || status != "ES_RECEIVED",
        flagged: flags.flagged,
        answered: flags.answered,
        received_millis: internal,
        server_size: size,
        archived: folder.kind == "EfkArchive",
    };
    let labels = if keywords && !flags.keywords.is_empty() {
        session
            .label_keywords()
            .map_err(|err| StoreFailure::Write(err.to_string()))?
            .clone()
    } else {
        HashMap::new()
    };
    let facade = session.facade;
    force_new(facade, || {
        let id = store::store_message(facade, &parsed, &place)?;
        if !labels.is_empty() {
            sync_labels(facade, &id, &labels, &flags.keywords)?;
        }
        Ok(id)
    })
    .map_err(|err| StoreFailure::Write(err.to_string()))
}

/// The new UIDs whose Message-ID was a row of another folder: that row, when it left its folder in this session (a
/// move), else a copy of its own.
fn resolve_pending(conn: &mut ImapConn, session: &mut Session<'_>, report: &mut SyncReport) -> Result<(), SyncFailure> {
    let pending = std::mem::take(&mut session.pending);
    let mut copies: Vec<PendingCopy> = Vec::new();
    for item in pending {
        let left = item
            .existing
            .iter()
            .find(|id| session.vanished.contains_key(*id) || session.claim_only.contains(*id))
            .cloned();
        match left {
            Some(row_id) => {
                session.vanished.remove(&row_id);
                session.claim_only.remove(&row_id);
                match claim(session, &row_id, &item.folder, item.uid, &item.flags, true) {
                    Ok(()) => report.updated_count += 1,
                    Err(err) => tracing::error!(
                        "Could not move email message {row_id} to folder {} of account {}: {err}",
                        item.folder.path,
                        session.account.id
                    ),
                }
            }
            None => copies.push(item),
        }
    }
    copies.sort_by(|left, right| left.folder.path.cmp(&right.folder.path).then(left.uid.cmp(&right.uid)));
    let mut index = 0;
    while index < copies.len() {
        let folder = copies[index].folder.clone();
        let mut group = Vec::new();
        while index < copies.len() && copies[index].folder.id == folder.id {
            group.push(index);
            index += 1;
        }
        let selected = match select(conn, &folder.path, false) {
            Ok(selected) => selected,
            Err(err) if connection_lost(&err) => return Err(SyncFailure::Connect(err.to_string())),
            Err(err) => {
                report.folder_errors.push(format!("{}: {err}", folder.path));
                continue;
            }
        };
        let keywords = selected.keywords();
        for chunk in group.chunks(FETCH_CHUNK) {
            let uids: Vec<u32> = chunk.iter().map(|position| copies[*position].uid).collect();
            let bodies = match conn.session.uid_fetch(uid_set(&uids), "(UID BODY.PEEK[])") {
                Ok(bodies) => bodies,
                Err(err) if connection_lost(&err) => return Err(SyncFailure::Connect(err.to_string())),
                Err(err) => {
                    report.folder_errors.push(format!("{}: {err}", folder.path));
                    continue;
                }
            };
            let mut raw_by_uid: HashMap<u32, Vec<u8>> = HashMap::new();
            for fetch in bodies.iter() {
                if let (Some(uid), Some(body)) = (fetch.uid, fetch.body()) {
                    raw_by_uid.insert(uid, body.to_vec());
                }
            }
            drop(bodies);
            for position in chunk {
                let item = &copies[*position];
                let Some(raw) = raw_by_uid.remove(&item.uid) else {
                    continue;
                };
                match store_new(
                    session,
                    &folder,
                    item.uid,
                    &raw,
                    &item.flags,
                    item.internal,
                    item.size,
                    keywords,
                ) {
                    Ok(id) => {
                        report.new_count += 1;
                        if folder.kind == "EfkInbox" {
                            session.new_inbox_ids.push(id);
                        }
                    }
                    Err(StoreFailure::Malformed(text) | StoreFailure::Write(text)) => {
                        tracing::error!(
                            "Skipping email UID {} of folder {} of account {}: {text}",
                            item.uid,
                            folder.path,
                            session.account.id
                        );
                        report.skipped_messages += 1;
                    }
                }
            }
        }
    }
    Ok(())
}

/// The rows whose message left their folder and is nowhere else: deleted, each on its own; one a record refers to is
/// kept, out of the account.
fn remove_vanished(session: &mut Session<'_>, report: &mut SyncReport) {
    let with_message_id: HashSet<String> = session.vanished_by_mid.values().flatten().cloned().collect();
    // with a folder not read to the end, a message that left may yet be found there next time
    let keep_findable = !session.incomplete.is_empty();
    let ids: Vec<String> = session.vanished.keys().cloned().collect();
    for id in ids {
        if session.pending_local.contains(&id) || (keep_findable && with_message_id.contains(&id)) {
            continue;
        }
        let mut parameters = BTreeMap::new();
        parameters.insert("emailMessageIdList".to_string(), Value::List(vec![Value::string(&id)]));
        let deleted = crate::service::sync(&format!("{SERVICES}.delete#LocalMessages"), &parameters);
        let failed = match deleted {
            Err(err) => Some(err.message().to_string()),
            Ok(_) => take_errors_string(),
        };
        if let Some(text) = failed {
            tracing::warn!(
                "Email message {id} of account {} left the server and is kept, a record refers to it: {text}",
                session.account.id
            );
            let detached = crate::service::sync(&format!("{SERVICES}.detach#LocalMessages"), &parameters);
            if let Some(text) = detached
                .err()
                .map(|err| err.message().to_string())
                .or_else(take_errors_string)
            {
                tracing::error!(
                    "Could not take email message {id} out of account {}: {text}",
                    session.account.id
                );
            }
        }
        report.removed_count += 1;
    }
    session.vanished.clear();
}

/// How long before the account's last sync a message may have arrived and still be new mail for the email-ECA rules.
const ECA_MARGIN_MILLIS: i64 = 24 * 60 * 60 * 1000;

/// Whether the email-ECA rules run on a message just stored in the Inbox ([`run_stored_ecas`]), which happens once, as
/// it is stored: only a message stored new (a Message-ID the account did not have: a copy or a move, and a folder read
/// again after a UIDVALIDITY change, are not new mail), only once the account has synced before (its first sync is
/// the mailbox's history), and only one that arrived (`arrived`: IMAP INTERNALDATE, POP3 the Date header) no earlier
/// than a day before that sync, so the rest of a large first sync's backlog is history too.
fn eca_due(account: &AccountInfo, arrived: Option<i64>) -> bool {
    let Some(last) = account.last_sync_millis else {
        return false;
    };
    arrived.is_none_or(|millis| millis >= last.saturating_sub(ECA_MARGIN_MILLIS))
}

/// The account's rules on its new Inbox mail; a rule that fails is the rules' to log.
pub(super) fn run_rules(account_id: &str, ids: &[String]) {
    if ids.is_empty() {
        return;
    }
    let mut parameters = BTreeMap::new();
    parameters.insert("emailAccountId".to_string(), Value::string(account_id));
    parameters.insert(
        "emailMessageIdList".to_string(),
        Value::List(ids.iter().map(Value::string).collect()),
    );
    if let Err(err) = crate::service::sync(&format!("{SERVICES}.run#EmailRules"), &parameters) {
        tracing::error!("Email rules of account {account_id} failed: {err}");
    }
    if let Some(text) = take_errors_string() {
        tracing::error!("Email rules of account {account_id} failed: {text}");
    }
}

/// What new Inbox mail brings once the rules ran, each part on its own (a part that fails is logged and the other
/// runs): a snoozed thread with new mail is back in the Inbox (`wake#SnoozedThreads`, as Gmail does), and the account's
/// automatic reply goes to the senders it is for (`send#OutOfOfficeReplies`).
pub(super) fn after_new_mail(account_id: &str, ids: &[String]) {
    if ids.is_empty() {
        return;
    }
    for service in ["wake#SnoozedThreads", "send#OutOfOfficeReplies"] {
        let mut parameters = BTreeMap::new();
        parameters.insert("emailAccountId".to_string(), Value::string(account_id));
        parameters.insert(
            "emailMessageIdList".to_string(),
            Value::List(ids.iter().map(Value::string).collect()),
        );
        if let Err(err) = crate::service::sync(&format!("{SERVICES}.{service}"), &parameters) {
            tracing::error!("{service} on the new mail of email account {account_id} failed: {err}");
        }
        if let Some(text) = take_errors_string() {
            tracing::error!("{service} on the new mail of email account {account_id} failed: {text}");
        }
    }
}

// ---- POP3 ----

fn pop_sync(
    facade: &EntityFacade,
    account: &AccountInfo,
    server: &ServerInfo,
    report: &mut SyncReport,
    progress: &mut Progress,
) -> Result<(), SyncFailure> {
    require_host(&server.host).map_err(|err| SyncFailure::Connect(err.to_string()))?;
    let lost = |err: LibError| SyncFailure::Connect(err.to_string());
    let folders = load_folders(facade, &account.id).map_err(lost)?;
    let Some(inbox) = folders.iter().find(|folder| folder.kind == "EfkInbox").cloned() else {
        return Err(SyncFailure::Connect(format!(
            "Email account {} has no Inbox folder",
            account.id
        )));
    };
    let settings = StoreSettings {
        host: server.host.clone(),
        port: server.port,
        user: server.user.clone(),
        password: server.password.clone(),
        protocol: server.protocol.clone(),
        folder: "INBOX".to_string(),
        skip_seen: false,
        mark_seen: false,
        delete: server.store_delete,
    };
    let mut pop = open_pop(facade, server, &settings)?;
    let total_folders = 1;
    progress.folders(total_folders);
    progress.start_folder(&inbox.name, "INBOX");
    let listing = pop
        .uidl()
        .map_err(|err| SyncFailure::Connect(format!("POP3 UIDL on {}: {}", server.host, err.message())))?;
    let known: HashSet<String> = facade
        .find(MESSAGE)
        .and_then(|find| {
            find.eq("emailAccountId", Value::string(&account.id))
                .condition(Condition::is_not_null("remoteUidl"))
                .select_field("remoteUidl")
                .list()
        })
        .map_err(lost)?
        .values()
        .iter()
        .map(|row| field_text(row, "remoteUidl"))
        .collect();
    // the newest first, a batch at a time
    let mut unknown: Vec<(u32, String)> = listing.into_iter().filter(|(_, uidl)| !known.contains(uidl)).collect();
    unknown.sort_by_key(|item| std::cmp::Reverse(item.0));
    let pending = unknown.len();
    unknown.truncate(usize::try_from(property_number("sync-batch", 500)).unwrap_or(500));
    progress.new_mail(unknown.len(), pending, known.len());
    let mut new_ids = Vec::new();
    for (number, uidl) in unknown {
        let raw = match pop.retrieve(number) {
            Ok(raw) => raw,
            Err(err) => {
                return Err(SyncFailure::Connect(format!(
                    "POP3 RETR {number} on {}: {}",
                    server.host,
                    err.message()
                )))
            }
        };
        let parsed = match parse_message(&raw) {
            Ok(parsed) => parsed,
            Err(text) => {
                tracing::error!(
                    account = %account.id,
                    "Skipping POP3 message {uidl} of account {}: {text}",
                    account.id
                );
                report.skipped_messages += 1;
                continue;
            }
        };
        let place = Placement {
            account_id: &account.id,
            folder_id: Some(&inbox.id),
            remote_folder_id: None,
            remote_uid: None,
            remote_uidl: Some(&uidl),
            status_id: "ES_RECEIVED",
            read: false,
            flagged: false,
            answered: false,
            received_millis: Some(now_millis()),
            server_size: None,
            archived: false,
        };
        match force_new(facade, || store::store_message(facade, &parsed, &place)) {
            Ok(id) => {
                report.new_count += 1;
                progress.fetched_one();
                // POP3 has no server date: when it was sent (the newest first, so a backlog is older mail)
                if eca_due(account, parsed.sent_millis) {
                    run_stored_ecas(
                        facade,
                        &StoredMessage {
                            raw: &raw,
                            server_id: &server.server_id,
                            account_id: &account.id,
                            email_message_id: &id,
                            received_millis: None,
                            seen: false,
                            answered: false,
                            flagged: false,
                        },
                    );
                }
                new_ids.push(id);
                if server.store_delete {
                    if let Err(err) = pop.delete_number(number) {
                        tracing::warn!("Could not delete POP3 message {uidl} of account {}: {err}", account.id);
                    }
                }
            }
            Err(err) => {
                tracing::error!("Could not store POP3 message {uidl} of account {}: {err}", account.id);
                report.skipped_messages += 1;
            }
        }
    }
    if let Err(err) = pop.quit() {
        tracing::warn!("POP3 QUIT on {}: {err}", server.host);
    }
    report.folder_count = total_folders;
    progress.end_folder();
    run_rules(&account.id, &new_ids);
    after_new_mail(&account.id, &new_ids);
    Ok(())
}

/// Connect to a POP3 server and sign in: `USER` and `PASS`, or for an EatOAuth2 server `AUTH XOAUTH2` with an access
/// token, replaced by a new one once when the server refuses it (it may have been revoked or have ended early). A
/// refused sign-in is [`SyncFailure::Auth`].
pub(super) fn open_pop(
    facade: &EntityFacade,
    server: &ServerInfo,
    settings: &StoreSettings,
) -> Result<Pop3, SyncFailure> {
    let classify = |err: crate::context::Error| {
        let text = err.message().to_string();
        if text.starts_with("mail server rejected the command") {
            SyncFailure::Auth(format!("POP3 {} refused the sign-in: {text}", server.host))
        } else {
            SyncFailure::Connect(format!("POP3 {}: {text}", server.host))
        }
    };
    if !server.is_oauth() {
        return Pop3::open(settings).map_err(classify);
    }
    let token = super::oauth::access_token(facade, &server.server_id, None)?;
    match Pop3::open_xoauth2(settings, &token).map_err(classify) {
        Err(SyncFailure::Auth(text)) => {
            tracing::debug!(
                "POP3 {} refused the access token, trying a new one: {text}",
                server.host
            );
            let fresh = super::oauth::access_token(facade, &server.server_id, Some(&token))?;
            if fresh == token {
                return Err(SyncFailure::Auth(text));
            }
            Pop3::open_xoauth2(settings, &fresh).map_err(classify)
        }
        other => other,
    }
}

// ---- the job ----

/// The accounts of this server whose last sync stopped at `email-client` `sync-batch` with new mail left on the server.
#[derive(Default)]
struct Backlog(std::sync::Mutex<HashSet<String>>);

fn backlog() -> Option<std::sync::Arc<Backlog>> {
    ExecutionContext::with_current(|ec| ec.factory().extension(|_| Backlog::default()))
}

fn set_backlog(account_id: &str, capped: bool) {
    if let Some(backlog) = backlog() {
        let mut ids = moqui_lib::sync::lock(&backlog.0);
        if capped {
            ids.insert(account_id.to_string());
        } else {
            ids.remove(account_id);
        }
    }
}

fn has_backlog(account_id: &str) -> bool {
    backlog().is_some_and(|backlog| moqui_lib::sync::lock(&backlog.0).contains(account_id))
}

/// `sync#DueEmailAccounts`, the job `sync_EmailAccounts_frequent`: each account with an incoming server whose last
/// sync is older than its interval (`syncIntervalMinutes`, else `email-client` `sync-interval`, 5 minutes; three times that
/// after an error), or whose last sync stopped at `email-client` `sync-batch` with mail left (at every run of the job, so a
/// large first sync goes on a batch a minute), each on its own. A paused account, one whose sign-in failed and one
/// syncing now are left.
pub(super) fn sync_due_accounts(
    _parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    if facade.sim_active() {
        return Ok(out);
    }
    crate::entity::fence::check_effect(crate::entity::fence::Effect::Mail, "sync#DueEmailAccounts")?;
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let default_interval = property_number("sync-interval", 5);
    let rows = facade
        .find(ACCOUNT)?
        .condition(Condition::is_not_null("emailServerId"))
        .condition(Condition::not_equals("accountKindEnumId", Value::string("EakInternal")))
        .condition(Condition::or(vec![
            Condition::is_null("statusId"),
            Condition::is_in("statusId", vec![Value::string("EasActive"), Value::string("EasError")]),
        ]))
        .order_by("lastSyncDate", false)
        .list()?;
    let now = now_millis();
    let (mut synced, mut failed, mut busy) = (0i64, 0i64, 0i64);
    for row in rows.values() {
        let account = match account_info(facade, row) {
            Ok(account) => account,
            Err(err) => {
                tracing::error!("Email account {}: {err}", field_text(row, "emailAccountId"));
                failed += 1;
                continue;
            }
        };
        if account.server.is_none() {
            continue;
        }
        let mut interval = account.interval_minutes.unwrap_or(default_interval) * 60_000;
        if account.status == "EasError" {
            interval *= 3;
        }
        // a few seconds' slack, so a job every minute syncs an account every interval, not every interval and a minute
        if !has_backlog(&account.id)
            && account
                .last_sync_millis
                .is_some_and(|last| now - last + 5_000 < interval)
        {
            continue;
        }
        let report = run_sync(facade, &account.id, &SyncOptions::default());
        match report.outcome {
            "synced" => synced += 1,
            "busy" => busy += 1,
            "failed" | "authFailed" => failed += 1,
            _ => {}
        }
        let _ = take_errors_string();
    }
    out.insert("syncedCount".into(), Value::I64(synced));
    out.insert("failedCount".into(), Value::I64(failed));
    out.insert("busyCount".into(), Value::I64(busy));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{
        decode_mutf7, encode_mutf7, expand_set, header_message_id, imap_date, label_keyword, remote_kind, uid_set,
    };

    #[test]
    fn uid_sets_compress_and_expand() {
        assert_eq!(uid_set(&[5, 1, 2, 3, 9, 10]), "1:3,5,9:10");
        assert_eq!(expand_set("1:3,5,10:9"), vec![1, 2, 3, 5, 9, 10]);
    }

    #[test]
    fn modified_utf7_round_trips() {
        assert_eq!(decode_mutf7("&ZeVnLIqe-"), "日本語");
        assert_eq!(encode_mutf7("日本語"), "&ZeVnLIqe-");
        assert_eq!(decode_mutf7("Tom &- Jerry"), "Tom & Jerry");
        assert_eq!(encode_mutf7("Tom & Jerry"), "Tom &- Jerry");
        assert_eq!(decode_mutf7(&encode_mutf7("Entwürfe")), "Entwürfe");
    }

    #[test]
    fn folder_kinds_by_attribute_then_name() {
        assert_eq!(remote_kind("INBOX", &[], "/"), "EfkInbox");
        assert_eq!(
            remote_kind("[Gmail]/Sent Mail", &["\\Sent".to_string()], "/"),
            "EfkSent"
        );
        assert_eq!(remote_kind("INBOX.Trash", &[], "."), "EfkTrash");
        assert_eq!(remote_kind("Spam", &[], "/"), "EfkJunk");
        assert_eq!(remote_kind("Clients/Sent", &[], "/"), "EfkOther");
        assert_eq!(
            remote_kind("[Gmail]/All Mail", &["\\All".to_string()], "/"),
            "EfkArchive"
        );
    }

    #[test]
    fn small_parsers() {
        assert_eq!(header_message_id("Message-ID: <a@b>\r\n\r\n"), "<a@b>");
        assert_eq!(header_message_id("Message-Id:\r\n <c@d>\r\n"), "<c@d>");
        assert_eq!(imap_date(0), "1-Jan-1970");
        assert_eq!(label_keyword("Follow up!"), "Follow_up!");
        // a label cannot pass for a system keyword such as $Junk
        assert_eq!(label_keyword("$Junk"), "_Junk");
    }
}
