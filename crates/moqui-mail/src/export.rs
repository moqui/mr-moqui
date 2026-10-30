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

//! `export#EmailMessage`: a stored message as an RFC 5322 file (`.eml`) to download, with its `EmailMessageAttachment`
//! files. The mail client keeps no raw message, so the MIME is made again by the send path's builder
//! ([`build_lettre`]) from the stored fields, as the copy of sent mail the sync appends to Sent is: the From, To, Cc,
//! Reply-To of its identity, Subject, Date, Message-ID, In-Reply-To and References; the text and HTML bodies as
//! alternatives; inline files by Content-ID beside the HTML and the others attached. Bcc is not written (lettre leaves it
//! out of the message, as a sent copy has it).
//!
//! A received message (one whose `headersString` holds the header block it came with, not only the headers the client
//! writes itself) keeps that block: its Received, Authentication-Results, DKIM-Signature, List-Unsubscribe and the rest
//! as they came, folded lines and all, with the MIME structure (MIME-Version, Content-Type, Content-Transfer-Encoding)
//! of the body made again in place of the original's. A header block cut at the most the store keeps loses its last,
//! perhaps partial, field.

use std::collections::BTreeMap;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use super::implementation::{
    build_lettre, client_headers, find_one, new_message_id, note_error, split_addresses, stored_attachments, Letter,
    CLIENT_HEADERS,
};
use super::store::{HEADERS_MAX, MESSAGE};
use super::sync::{value_millis, value_number};
use moqui_core::context::ExecutionContext;
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::service::util::{field_text, has_error, param_text};

const SERVICES: &str = "org.moqui.impl.EmailClientServices";

/// `export#EmailMessage`: `mimeBytes` (and the same as `mimeText`, which is 7-bit text), `fileName` (the subject made safe,
/// with `.eml`) and `contentType` (`message/rfc822`). A message the user may not read (level 1 on its account) is "not
/// found"; one with no mail client account only with authorization disabled. A file that cannot be read is an error, so no
/// export leaves out an attachment without saying so.
pub(super) fn export_message(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    let email_message_id = param_text(parameters, "emailMessageId");
    let not_found = || note_error(format!("Email message {email_message_id} not found"));
    let Some(message) = find_one(facade, MESSAGE, "emailMessageId", &email_message_id)? else {
        not_found();
        return Ok(out);
    };
    if !may_read(&message)? {
        if !has_error() {
            not_found();
        }
        return Ok(out);
    }
    let bytes = match message_mime(facade, &message) {
        Ok(bytes) => bytes,
        Err(text) => {
            note_error(format!(
                "Email message {email_message_id} could not be exported: {text}"
            ));
            return Ok(out);
        }
    };
    out.insert("mimeText".into(), Value::string(String::from_utf8_lossy(&bytes)));
    out.insert("mimeBytes".into(), Value::Bytes(bytes));
    out.insert(
        "fileName".into(),
        Value::string(file_name(&field_text(&message, "subject"))),
    );
    out.insert("contentType".into(), Value::string("message/rfc822"));
    Ok(out)
}

/// The user reaches the message's account at read level or more, or authorization is disabled.
fn may_read(message: &EntityValue) -> Result<bool, LibError> {
    if ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(false) {
        return Ok(true);
    }
    let account_id = field_text(message, "emailAccountId");
    if account_id.is_empty() {
        return Ok(false);
    }
    let mut access_in = BTreeMap::new();
    access_in.insert("emailAccountId".to_string(), Value::string(&account_id));
    let access = moqui_core::service::sync(&format!("{SERVICES}.get#EmailAccountAccess"), &access_in)
        .map_err(|err| LibError::new(err.message()))?;
    Ok(!has_error() && value_number(access.get("level")).unwrap_or(0) >= 1)
}

/// The RFC 5322 bytes of a stored message and its files.
fn message_mime(facade: &EntityFacade, message: &EntityValue) -> Result<Vec<u8>, String> {
    let id = field_text(message, "emailMessageId");
    let mut reply_to = Vec::new();
    let identity = find_one(
        facade,
        "moqui.basic.email.EmailIdentity",
        "emailIdentityId",
        &field_text(message, "emailIdentityId"),
    )
    .map_err(|err| err.to_string())?;
    if let Some(identity) = identity {
        reply_to = split_addresses(&field_text(&identity, "replyToAddress"));
    }
    let from_address = field_text(message, "fromAddress");
    let mut message_id = field_text(message, "messageId").trim().to_string();
    if message_id.is_empty() {
        // a draft that was never sent nor appended has none yet
        message_id = new_message_id(&from_address).map_err(|err| err.message().to_string())?;
    }
    let date = ["sentDate", "messageDate", "receivedDate", "lastUpdatedStamp"]
        .iter()
        .find_map(|name| value_millis(message.get(name)));
    let mut letter = Letter {
        subject: field_text(message, "subject"),
        from_address,
        from_name: field_text(message, "fromName"),
        to: split_addresses(&field_text(message, "toAddresses")),
        cc: split_addresses(&field_text(message, "ccAddresses")),
        bcc: Vec::new(),
        reply_to,
        bounce: String::new(),
        body: field_text(message, "body"),
        body_text: field_text(message, "bodyText"),
        attachments: Vec::new(),
        inline: Vec::new(),
        message_id,
        in_reply_to: field_text(message, "inReplyTo"),
        references: field_text(message, "referencesHeader"),
        date,
        extra_headers: client_headers(&field_text(message, "headersString")),
    };
    stored_attachments(facade, &id, &mut letter).map_err(|err| err.message().to_string())?;
    let built = build_lettre(&letter)
        .map_err(|err| err.message().to_string())?
        .formatted();
    Ok(with_received_headers(built, &field_text(message, "headersString")))
}

/// The header fields the body made again has its own of: the MIME structure of the top part.
const STRUCTURE_HEADERS: [&str; 7] = [
    "MIME-Version",
    "Content-Type",
    "Content-Transfer-Encoding",
    "Content-Disposition",
    "Content-ID",
    "Content-Length",
    "Content-MD5",
];

/// The fields of a header block, each its first line and the folded lines after it, without line ends. A line that
/// is not a field (a stray line, an mbox `From ` line) is left out with its folded lines.
fn header_fields(block: &str) -> Vec<(String, Vec<String>)> {
    let mut fields: Vec<(String, Vec<String>)> = Vec::new();
    let mut skipping = false;
    for line in block.split('\n').map(|line| line.trim_end_matches('\r')) {
        if line.starts_with([' ', '\t']) {
            if !skipping {
                if let Some((_, lines)) = fields.last_mut() {
                    lines.push(line.to_string());
                }
            }
            continue;
        }
        let name = line.split_once(':').map(|(name, _)| name).unwrap_or_default();
        // RFC 5322 field name: printable ASCII but space and colon
        skipping = name.is_empty() || !name.bytes().all(|byte| (33..=126).contains(&byte) && byte != b':');
        if !skipping {
            fields.push((name.to_string(), vec![line.to_string()]));
        }
    }
    fields
}

fn named(name: &str, names: &[&str]) -> bool {
    names.iter().any(|known| known.eq_ignore_ascii_case(name))
}

/// The message with the header block a received message came with in place of the one made again, keeping the made
/// MIME structure. A message whose stored headers are only the client's own (a draft, a reply sent from here), or none,
/// is as it was made.
fn with_received_headers(built: Vec<u8>, stored: &str) -> Vec<u8> {
    let mut fields = header_fields(stored);
    if !fields.iter().any(|(name, _)| !named(name, &CLIENT_HEADERS)) {
        return built;
    }
    // a block cut at the most the store keeps may end in the middle of a field
    if stored.len() + 2 >= HEADERS_MAX {
        fields.pop();
    }
    let Some(split) = built.windows(4).position(|window| window == b"\r\n\r\n") else {
        return built;
    };
    let made = String::from_utf8_lossy(&built[..split]).into_owned();
    let mut out = String::new();
    for (_, lines) in fields.iter().filter(|(name, _)| !named(name, &STRUCTURE_HEADERS)) {
        out.push_str(&lines.join("\r\n"));
        out.push_str("\r\n");
    }
    for (_, lines) in header_fields(&made)
        .iter()
        .filter(|(name, _)| named(name, &STRUCTURE_HEADERS[..3]))
    {
        out.push_str(&lines.join("\r\n"));
        out.push_str("\r\n");
    }
    let mut bytes = out.into_bytes();
    bytes.extend_from_slice(&built[split + 2..]);
    bytes
}

/// The subject as a file name: what is not a word character, space, dot or hyphen becomes `_`, at most 80 characters,
/// `message` when nothing is left, and `.eml`.
pub(super) fn file_name(subject: &str) -> String {
    let mut name = String::new();
    let mut replaced = false;
    for ch in subject.chars() {
        if ch.is_alphanumeric() || matches!(ch, '_' | ' ' | '.' | '-') {
            name.push(ch);
            replaced = false;
        } else if !replaced {
            name.push('_');
            replaced = true;
        }
    }
    let name: String = name.trim().trim_matches('.').chars().take(80).collect();
    let name = name.trim();
    format!("{}.eml", if name.is_empty() { "message" } else { name })
}

#[cfg(test)]
#[path = "export_tests.rs"]
mod tests;
