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

//! One received message as the mail client keeps it, for the sync, the POP fetch and the import: parsed with
//! mail-parser (headers, the HTML and text bodies, a snippet, the files), stored as an `EmailMessage` with its
//! `EmailMessageAttachment` rows (each file a DbResource under `dbresource://email/<account>/<message>/`), and threaded
//! by In-Reply-To and References into `rootEmailMessageId` and `parentEmailMessageId`. A reply that arrived before
//! the message it answers is taken into that message's thread when it comes.

use mail_parser::{MessageParser, MimeHeaders, PartType};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

pub(super) use super::implementation::header_value;
use super::inbound::{address_text, stamp_millis};
use moqui_core::context::ExecutionContext;
use moqui_core::entity::{Condition, EntityFacade, EntityValue};
use moqui_core::service::util::{field_text, now_millis};

pub(super) const MESSAGE: &str = "moqui.basic.email.EmailMessage";
pub(super) const ATTACHMENT: &str = "moqui.basic.email.EmailMessageAttachment";

/// The longest snippet, as `save#Draft` keeps it.
const SNIPPET_LENGTH: usize = 140;
/// The most of the raw header block kept in `headersString`.
pub(super) const HEADERS_MAX: usize = 65_536;

/// A file of a message: an attachment, or an inline part the HTML refers to by `cid:`.
pub(super) struct PartFile {
    pub(super) file_name: String,
    pub(super) content_type: String,
    pub(super) content_id: String,
    pub(super) inline: bool,
    pub(super) bytes: Vec<u8>,
}

/// A message read from its RFC 822 text.
pub(super) struct Parsed {
    /// With its angle brackets, as the send path writes it; empty when the message has none.
    pub(super) message_id: String,
    pub(super) in_reply_to: String,
    /// The References ids, each in angle brackets, separated by spaces.
    pub(super) references: String,
    pub(super) subject: String,
    pub(super) from_address: String,
    pub(super) from_name: String,
    pub(super) to: String,
    pub(super) cc: String,
    pub(super) bcc: String,
    pub(super) sent_millis: Option<i64>,
    /// The header block as it was written.
    pub(super) headers: String,
    pub(super) html: Option<String>,
    pub(super) text: Option<String>,
    pub(super) content_type: String,
    pub(super) snippet: String,
    pub(super) size: i64,
    pub(super) files: Vec<PartFile>,
    pub(super) has_attachment: bool,
}

impl Parsed {
    /// The ids this message answers, nearest first: In-Reply-To, then the References from the last.
    fn parent_candidates(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.in_reply_to.is_empty() {
            out.push(self.in_reply_to.clone());
        }
        for id in self.references.split_whitespace().rev() {
            if !out.iter().any(|known| known == id) {
                out.push(id.to_string());
            }
        }
        out.retain(|id| *id != self.message_id);
        out
    }

    /// The value of a header, by name in any case, as written; empty when absent.
    pub(super) fn header(&self, name: &str) -> String {
        header_value(&self.headers, name)
    }
}

/// Parse a message. A message with no header fields at all (an empty fetch, a stray binary) is not mail and is
/// refused, so the sync logs it and goes on.
pub(super) fn parse_message(raw: &[u8]) -> Result<Parsed, String> {
    if raw.iter().all(u8::is_ascii_whitespace) {
        return Err("the message is empty".to_string());
    }
    let message = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| "it is not a MIME message".to_string())?;
    let known_headers = message
        .headers()
        .iter()
        .filter(|header| !header.name().trim().is_empty())
        .count();
    if known_headers == 0 {
        return Err("it has no header fields".to_string());
    }
    let header_block = raw_header_block(raw);
    if !header_block.contains(':') {
        return Err("it has no header fields".to_string());
    }
    let ids = |value: &mail_parser::HeaderValue<'_>| -> Vec<String> {
        match value {
            mail_parser::HeaderValue::Text(text) => vec![angle(text)],
            mail_parser::HeaderValue::TextList(list) => list.iter().map(|id| angle(id)).collect(),
            _ => Vec::new(),
        }
    };
    let list = |address: Option<&mail_parser::Address<'_>>| -> String {
        address
            .map(|address| address.iter().map(address_text).collect::<Vec<_>>().join(", "))
            .unwrap_or_default()
    };
    let (from_address, from_name) = message
        .from()
        .and_then(mail_parser::Address::first)
        .map(|addr| {
            (
                addr.address().unwrap_or_default().to_string(),
                addr.name().unwrap_or_default().to_string(),
            )
        })
        .unwrap_or_default();
    let mut htmls = Vec::new();
    for index in 0..message.html_body_count() {
        if let Some(html) = message.body_html(index) {
            htmls.push(html.into_owned());
        }
    }
    let mut texts = Vec::new();
    for index in 0..message.text_body_count() {
        if let Some(text) = message.body_text(index) {
            texts.push(text.into_owned());
        }
    }
    let html = (!htmls.is_empty()).then(|| htmls.join("\n"));
    let text = (!texts.is_empty()).then(|| texts.join("\n\n"));
    let content_type = message
        .content_type()
        .map(|kind| match kind.subtype() {
            Some(subtype) => format!("{}/{subtype}", kind.ctype()),
            None => kind.ctype().to_string(),
        })
        .unwrap_or_else(|| "text/plain".to_string())
        .to_ascii_lowercase();
    let snippet = snippet_of(text.as_deref().unwrap_or_default());
    let mut files = Vec::new();
    for (index, part) in message.attachments().enumerate() {
        files.push(part_file(part, index + 1));
    }
    let has_attachment = files.iter().any(|file| !file.inline);
    Ok(Parsed {
        message_id: message.message_id().map(angle).unwrap_or_default(),
        in_reply_to: ids(message.in_reply_to()).into_iter().next().unwrap_or_default(),
        references: ids(message.references()).join(" "),
        subject: message.subject().unwrap_or_default().to_string(),
        from_address,
        from_name,
        to: list(message.to()),
        cc: list(message.cc()),
        bcc: list(message.bcc()),
        sent_millis: message.date().map(|date| date.to_timestamp().saturating_mul(1000)),
        headers: header_block,
        html,
        text,
        content_type,
        snippet,
        size: i64::try_from(raw.len()).unwrap_or(i64::MAX),
        files,
        has_attachment,
    })
}

/// An id in angle brackets, as Message-ID headers are written.
fn angle(id: &str) -> String {
    let id = id.trim().trim_start_matches('<').trim_end_matches('>');
    if id.is_empty() {
        String::new()
    } else {
        format!("<{id}>")
    }
}

/// The header block of a raw message: up to the first empty line, at most [`HEADERS_MAX`] bytes.
fn raw_header_block(raw: &[u8]) -> String {
    let mut end = raw.len();
    for (index, window) in raw.windows(2).enumerate() {
        if window == b"\n\n" {
            end = index + 1;
            break;
        }
        if window == b"\n\r" && raw.get(index + 2) == Some(&b'\n') {
            end = index + 1;
            break;
        }
    }
    let end = end.min(HEADERS_MAX);
    String::from_utf8_lossy(&raw[..end]).trim_end().to_string()
}

/// The start of a body as one line, for the message list.
pub(super) fn snippet_of(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > SNIPPET_LENGTH {
        let cut: String = collapsed.chars().take(SNIPPET_LENGTH - 3).collect();
        format!("{cut}...")
    } else {
        collapsed
    }
}

fn part_file(part: &mail_parser::MessagePart<'_>, index: usize) -> PartFile {
    let nested = matches!(part.body, PartType::Message(_));
    let content_type = if nested {
        "message/rfc822".to_string()
    } else {
        part.content_type()
            .map(|kind| match kind.subtype() {
                Some(subtype) => format!("{}/{subtype}", kind.ctype()),
                None => kind.ctype().to_string(),
            })
            .unwrap_or_else(|| "application/octet-stream".to_string())
            .to_ascii_lowercase()
    };
    let content_id = part
        .content_id()
        .map(|id| id.trim().trim_start_matches('<').trim_end_matches('>').to_string())
        .unwrap_or_default();
    // inline: a part the HTML shows by its Content-ID; one with no Content-ID cannot be shown there, so it is listed
    let attached = part
        .content_disposition()
        .is_some_and(mail_parser::ContentType::is_attachment);
    let inline = !content_id.is_empty() && !attached;
    let file_name = part
        .attachment_name()
        .map(str::to_string)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| {
            let extension = match content_type.as_str() {
                "message/rfc822" => "eml",
                "text/plain" => "txt",
                "text/html" => "html",
                "text/calendar" => "ics",
                "image/png" => "png",
                "image/jpeg" => "jpg",
                "image/gif" => "gif",
                "application/pdf" => "pdf",
                _ => "bin",
            };
            format!("attachment-{index}.{extension}")
        });
    PartFile {
        file_name,
        content_type,
        content_id,
        inline,
        bytes: part.contents().to_vec(),
    }
}

/// Where and how a stored message lands.
pub(super) struct Placement<'a> {
    pub(super) account_id: &'a str,
    pub(super) folder_id: Option<&'a str>,
    pub(super) remote_folder_id: Option<&'a str>,
    pub(super) remote_uid: Option<u32>,
    pub(super) remote_uidl: Option<&'a str>,
    /// ES_RECEIVED, ES_SENT or ES_DRAFT.
    pub(super) status_id: &'a str,
    pub(super) read: bool,
    pub(super) flagged: bool,
    pub(super) answered: bool,
    /// The server's arrival time (IMAP INTERNALDATE), when known.
    pub(super) received_millis: Option<i64>,
    /// The server's size, when known.
    pub(super) server_size: Option<i64>,
    pub(super) archived: bool,
}

fn indicator(on: bool) -> Value {
    Value::string(if on { "Y" } else { "N" })
}

fn text_or_null(text: &str) -> Value {
    if text.is_empty() {
        Value::Null
    } else {
        Value::string(text)
    }
}

/// Store `parsed` as a new `EmailMessage`, with its files and its place in a thread, in the transaction in place.
/// Returns the new emailMessageId.
pub(super) fn store_message(facade: &EntityFacade, parsed: &Parsed, place: &Placement<'_>) -> Result<String, LibError> {
    let mut row = facade.make_value(MESSAGE)?;
    row.set_sequenced_id_primary(facade)?;
    let id = field_text(&row, "emailMessageId");
    let now = now_millis();
    let outbound = matches!(place.status_id, "ES_SENT" | "ES_DRAFT");
    let message_date = if outbound {
        parsed.sent_millis.or(place.received_millis).unwrap_or(now)
    } else {
        place.received_millis.or(parsed.sent_millis).unwrap_or(now)
    };
    let (parent_id, root_id) = find_thread(facade, place.account_id, parsed)?;
    let fields: Vec<(&str, Value)> = vec![
        ("emailAccountId", Value::string(place.account_id)),
        ("emailFolderId", place.folder_id.map_or(Value::Null, Value::string)),
        (
            "remoteFolderId",
            place.remote_folder_id.map_or(Value::Null, Value::string),
        ),
        (
            "remoteUid",
            place.remote_uid.map_or(Value::Null, |uid| Value::I64(i64::from(uid))),
        ),
        ("remoteUidl", place.remote_uidl.map_or(Value::Null, Value::string)),
        ("statusId", Value::string(place.status_id)),
        ("subject", text_or_null(&parsed.subject)),
        ("body", parsed.html.as_deref().map_or(Value::Null, Value::string)),
        ("bodyText", parsed.text.as_deref().map_or(Value::Null, Value::string)),
        ("headersString", text_or_null(&parsed.headers)),
        ("fromAddress", text_or_null(&parsed.from_address)),
        ("fromName", text_or_null(&parsed.from_name)),
        ("toAddresses", text_or_null(&parsed.to)),
        ("ccAddresses", text_or_null(&parsed.cc)),
        ("bccAddresses", text_or_null(&parsed.bcc)),
        ("contentType", text_or_null(&parsed.content_type)),
        ("messageId", text_or_null(&parsed.message_id)),
        ("inReplyTo", text_or_null(&parsed.in_reply_to)),
        ("referencesHeader", text_or_null(&parsed.references)),
        ("sentDate", parsed.sent_millis.map_or(Value::Null, stamp_millis)),
        (
            "receivedDate",
            if outbound {
                Value::Null
            } else {
                stamp_millis(place.received_millis.unwrap_or(now))
            },
        ),
        ("messageDate", stamp_millis(message_date)),
        ("isRead", indicator(place.read)),
        ("isFlagged", indicator(place.flagged)),
        ("isAnswered", indicator(place.answered)),
        ("hasAttachment", indicator(parsed.has_attachment)),
        ("snippet", text_or_null(&parsed.snippet)),
        ("messageSize", Value::I64(place.server_size.unwrap_or(parsed.size))),
        (
            "parentEmailMessageId",
            parent_id.as_deref().map_or(Value::Null, Value::string),
        ),
        (
            "rootEmailMessageId",
            root_id.as_deref().map_or(Value::Null, Value::string),
        ),
        (
            "archivedDate",
            if place.archived { stamp_millis(now) } else { Value::Null },
        ),
    ];
    for (name, value) in fields {
        row.set(name, value);
    }
    row.create(facade)?;
    store_files(facade, place.account_id, &id, &parsed.files)?;
    if !parsed.message_id.is_empty() {
        adopt_replies(
            facade,
            place.account_id,
            &id,
            root_id.as_deref().unwrap_or(&id),
            &parsed.message_id,
        )?;
    }
    Ok(id)
}

/// The files of a message as DbResource files with their `EmailMessageAttachment` rows.
pub(super) fn store_files(
    facade: &EntityFacade,
    account_id: &str,
    email_message_id: &str,
    files: &[PartFile],
) -> Result<(), LibError> {
    for (index, file) in files.iter().enumerate() {
        let location = format!(
            "dbresource://email/{}/{}/{}-{}",
            safe_segment(account_id),
            safe_segment(email_message_id),
            index + 1,
            safe_segment(&file.file_name)
        );
        let written = ExecutionContext::with_current(|ec| ec.factory().resource().write_bytes(&location, &file.bytes))
            .ok_or_else(|| LibError::new("no execution context is entered"))?;
        written.map_err(|err| {
            LibError::new(format!(
                "could not keep attachment {}: {}",
                file.file_name,
                err.message()
            ))
        })?;
        let mut row = facade.make_value(ATTACHMENT)?;
        row.set_sequenced_id_primary(facade)?;
        row.set("emailMessageId", Value::string(email_message_id));
        row.set("attachmentLocation", Value::string(&location));
        row.set("fileName", Value::string(&file.file_name));
        row.set(
            "fileSize",
            Value::I64(i64::try_from(file.bytes.len()).unwrap_or(i64::MAX)),
        );
        row.set("contentType", Value::string(&file.content_type));
        row.set("contentId", text_or_null(&file.content_id));
        row.set("isInline", indicator(file.inline));
        row.create(facade)?;
    }
    Ok(())
}

/// A path segment for a DbResource location: no slash, no control character, at most 120 characters.
pub(super) fn safe_segment(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '/' | '\\' | '?' | '#' | '%' | ':') {
                '_'
            } else {
                ch
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    let cleaned = if cleaned.is_empty() {
        "file".to_string()
    } else {
        cleaned
    };
    cleaned.chars().take(120).collect()
}

/// The parent and root a message joins: the newest message of the account with the id it answers, nearest first.
fn find_thread(
    facade: &EntityFacade,
    account_id: &str,
    parsed: &Parsed,
) -> Result<(Option<String>, Option<String>), LibError> {
    for candidate in parsed.parent_candidates() {
        let rows = facade
            .find(MESSAGE)?
            .eq("emailAccountId", Value::string(account_id))
            .eq("messageId", Value::string(&candidate))
            .select_field("emailMessageId")
            .select_field("rootEmailMessageId")
            .order_by("messageDate", true)
            .limit(1)
            .list()?;
        if let Some(parent) = rows.values().first() {
            let parent_id = field_text(parent, "emailMessageId");
            let root = field_text(parent, "rootEmailMessageId");
            let root = if root.is_empty() { parent_id.clone() } else { root };
            return Ok((Some(parent_id), Some(root)));
        }
    }
    Ok((None, None))
}

/// Replies stored before the message they answer: each that starts a thread of its own joins this message's thread,
/// with its replies.
fn adopt_replies(
    facade: &EntityFacade,
    account_id: &str,
    email_message_id: &str,
    root_id: &str,
    message_id: &str,
) -> Result<(), LibError> {
    let orphans = facade
        .find(MESSAGE)?
        .eq("emailAccountId", Value::string(account_id))
        .eq("inReplyTo", Value::string(message_id))
        .condition(Condition::is_null("parentEmailMessageId"))
        .condition(Condition::is_null("rootEmailMessageId"))
        .select_field("emailMessageId")
        .list()?;
    for orphan in orphans.values() {
        let orphan_id = field_text(orphan, "emailMessageId");
        if orphan_id == email_message_id {
            continue;
        }
        facade
            .find(MESSAGE)?
            .eq("rootEmailMessageId", Value::string(&orphan_id))
            .update_all([("rootEmailMessageId".to_string(), Value::string(root_id))])?;
        update_fields(
            facade,
            &orphan_id,
            &[
                ("parentEmailMessageId", Value::string(email_message_id)),
                ("rootEmailMessageId", Value::string(root_id)),
            ],
        )?;
    }
    Ok(())
}

/// Set these fields of one message.
pub(super) fn update_fields(
    facade: &EntityFacade,
    email_message_id: &str,
    fields: &[(&str, Value)],
) -> Result<(), LibError> {
    let mut row: EntityValue = facade.make_value(MESSAGE)?;
    row.set("emailMessageId", Value::string(email_message_id));
    for (name, value) in fields {
        row.set(*name, value.clone());
    }
    row.update(facade)
        .map_err(|err| LibError::new(format!("could not update email message {email_message_id}: {err}")))
}
