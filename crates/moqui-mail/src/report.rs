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

//! Delivery status notifications (RFC 3464) and abuse feedback reports (ARF, RFC 5965) of a stored message:
//! `read#DeliveryReport` (mr-boss impl-marketing P6, F18), beside the DSN reader the edge node uses on a raw message
//! ([`super::edge_mail::parse_dsn`]).
//!
//! A report a hosted account received (a bounce address's mailbox) is stored as any message is (`store.rs`): its
//! `message/delivery-status` or `message/feedback-report` part and the returned message (`message/rfc822`, or its header
//! as `text/rfc822-headers`) are `EmailMessageAttachment` files, and its trace fields (`Delivered-To`, the address it came
//! to, such as a VERP address) are in `headersString`. The reader reads those, never the message's body text, and
//! answers what the report says: its kind, each recipient's action, status and diagnostic for a DSN, the feedback type
//! and the original envelope for an ARF, and the original message's Message-ID. What the report means for a delivery is
//! the caller's (Mantle's `process#DeliveryReport`); nothing is changed here.

use std::collections::BTreeMap;

use indexmap::IndexMap;
use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityFacade;
use moqui_core::service::util::{field_text, param_text};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use super::implementation::find_one;

const MESSAGE: &str = "moqui.basic.email.EmailMessage";
const ATTACHMENT: &str = "moqui.basic.email.EmailMessageAttachment";
/// The most of a report part or a returned message read: a report's fields are a few lines, and the original message's
/// header block is all that is read of it.
const PART_MAX: usize = 256 * 1024;
/// The most recipients a DSN answers: a report names one recipient as a rule, a large list's relay a few.
const RECIPIENTS_MAX: usize = 100;

/// The field groups of a report part's body (RFC 3464 §2.1, RFC 5965 §3.1): header-style fields, folded lines joined,
/// names in lower case, groups separated by empty lines.
pub(crate) fn field_groups(body: &str) -> Vec<Vec<(String, String)>> {
    let body = body.replace("\r\n", "\n");
    let mut groups = Vec::new();
    for group in body.split("\n\n") {
        let mut fields: Vec<(String, String)> = Vec::new();
        for line in group.lines() {
            match (line.starts_with([' ', '\t']), fields.last_mut()) {
                (true, Some((_, value))) => {
                    value.push(' ');
                    value.push_str(line.trim());
                }
                _ => {
                    if let Some((name, value)) = line.split_once(':') {
                        fields.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
                    }
                }
            }
        }
        if !fields.is_empty() {
            groups.push(fields);
        }
    }
    groups
}

/// The first value of field `name` (lower case) in a group, empty for none.
fn field(fields: &[(String, String)], name: &str) -> String {
    fields
        .iter()
        .find(|(field, _)| field == name)
        .map(|(_, value)| value.clone())
        .unwrap_or_default()
}

/// Every value of field `name` (lower case) in a group.
fn fields_named(fields: &[(String, String)], name: &str) -> Vec<String> {
    fields
        .iter()
        .filter(|(field, _)| field == name)
        .map(|(_, value)| value.clone())
        .collect()
}

/// An address field's address: `rfc822; a@b.example` is `a@b.example`, angle brackets dropped.
fn address_of(value: &str) -> String {
    let value = value.split_once(';').map_or(value, |(_, address)| address);
    value
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
        .to_string()
}

/// One recipient of a delivery status notification.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DsnRecipient {
    /// `Final-Recipient`'s address.
    pub final_recipient: String,
    /// `Original-Recipient`'s address, empty for none.
    pub original_recipient: String,
    /// `Action` in lower case: failed, delayed, delivered, relayed or expanded.
    pub action: String,
    /// `Status`, as `5.1.1`: 5 is a permanent failure, 4 a transient one, 2 a success.
    pub status: String,
    /// `Diagnostic-Code`, as the remote server wrote it.
    pub diagnostic: String,
}

/// A delivery status notification's fields: the per-message group's envelope id and the recipients.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeliveryStatus {
    pub original_envelope_id: String,
    pub recipients: Vec<DsnRecipient>,
}

/// Read a `message/delivery-status` part's body: its first group is the message's, each later one a recipient's.
#[must_use]
pub fn delivery_status(body: &str) -> DeliveryStatus {
    let mut groups = field_groups(body).into_iter();
    let mut status = DeliveryStatus::default();
    let Some(first) = groups.next() else {
        return status;
    };
    // a report with no per-message group (some MTAs) starts with a recipient
    let recipient_groups: Vec<Vec<(String, String)>> = if field(&first, "final-recipient").is_empty() {
        status.original_envelope_id = field(&first, "original-envelope-id");
        groups.collect()
    } else {
        std::iter::once(first).chain(groups).collect()
    };
    for fields in recipient_groups.iter().take(RECIPIENTS_MAX) {
        let final_recipient = address_of(&field(fields, "final-recipient"));
        if final_recipient.is_empty() {
            continue;
        }
        status.recipients.push(DsnRecipient {
            final_recipient,
            original_recipient: address_of(&field(fields, "original-recipient")),
            action: field(fields, "action").to_ascii_lowercase(),
            status: field(fields, "status")
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string(),
            diagnostic: field(fields, "diagnostic-code"),
        });
    }
    status
}

/// An abuse feedback report's machine-readable part (RFC 5965 §3.1).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeedbackReport {
    /// `Feedback-Type` in lower case: abuse, fraud, virus, other, not-spam (RFC 6650) or auth-failure.
    pub feedback_type: String,
    /// `Original-Mail-From`: the original message's envelope sender, a VERP address when it was sent with one.
    pub original_mail_from: String,
    /// `Original-Rcpt-To`: the original message's envelope recipients, the one who complained.
    pub original_rcpt_to: Vec<String>,
    pub arrival_date: String,
    pub reported_domain: String,
    pub user_agent: String,
}

/// Read a `message/feedback-report` part's body (one group of fields).
#[must_use]
pub fn feedback_report(body: &str) -> FeedbackReport {
    let fields: Vec<(String, String)> = field_groups(body).into_iter().flatten().collect();
    FeedbackReport {
        feedback_type: field(&fields, "feedback-type").to_ascii_lowercase(),
        original_mail_from: address_of(&field(&fields, "original-mail-from")),
        original_rcpt_to: fields_named(&fields, "original-rcpt-to")
            .iter()
            .map(|value| address_of(value))
            .filter(|address| !address.is_empty())
            .take(RECIPIENTS_MAX)
            .collect(),
        arrival_date: field(&fields, "arrival-date"),
        reported_domain: field(&fields, "reported-domain"),
        user_agent: field(&fields, "user-agent"),
    }
}

/// Read an abuse feedback report (RFC 5965) from a whole message: its `message/feedback-report` part. None when the
/// message has no such part. The edge node's [`super::edge_mail::parse_dsn`] is the DSN reader on a raw message.
#[must_use]
pub fn parse_arf(raw: &[u8]) -> Option<FeedbackReport> {
    let text = String::from_utf8_lossy(raw);
    let lower = text.to_ascii_lowercase();
    let at = lower.find("content-type: message/feedback-report")?;
    // the part's own header ends at its first empty line; its body runs to the next boundary
    let rest = text.get(at..)?.replace("\r\n", "\n");
    let (_, body) = rest.split_once("\n\n")?;
    let body = body.split("\n--").next().unwrap_or(body);
    Some(feedback_report(body))
}

/// The values of header `name` (any case) in a header block, folded lines joined, in order.
fn header_values(block: &str, name: &str) -> Vec<String> {
    let mut values: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    let block = block.replace("\r\n", "\n");
    for line in block.lines() {
        if line.is_empty() {
            break;
        }
        if line.starts_with([' ', '\t']) {
            if let Some(value) = current.as_mut() {
                value.push(' ');
                value.push_str(line.trim());
            }
            continue;
        }
        if let Some(value) = current.take() {
            values.push(value);
        }
        if let Some((field, value)) = line.split_once(':') {
            if field.trim().eq_ignore_ascii_case(name) {
                current = Some(value.trim().to_string());
            }
        }
    }
    if let Some(value) = current {
        values.push(value);
    }
    values
}

/// The Message-ID of a returned message (its header block), with its angle brackets; empty for none.
fn message_id_of(original: &str) -> String {
    header_values(original, "message-id")
        .into_iter()
        .next()
        .map(|id| {
            let id = id.trim().trim_start_matches('<').trim_end_matches('>');
            if id.is_empty() {
                String::new()
            } else {
                format!("<{id}>")
            }
        })
        .unwrap_or_default()
}

/// What a stored report says: [`read_delivery_report`]'s answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoredReport {
    /// `dsn`, `arf`, or empty when the message is neither.
    pub report_type: String,
    pub status: DeliveryStatus,
    pub feedback: FeedbackReport,
    /// The returned message's Message-ID: the message the report is about.
    pub original_message_id: String,
    /// The addresses the report came to (its `Delivered-To` trace fields, else its To), as a VERP address.
    pub delivered_to: Vec<String>,
}

/// A report from the parts of a stored message: each `(content type, bytes)` with the message's header block.
#[must_use]
pub fn stored_report(headers: &str, to_addresses: &str, parts: &[(String, Vec<u8>)]) -> StoredReport {
    let mut report = StoredReport::default();
    let text_of = |bytes: &[u8]| String::from_utf8_lossy(&bytes[..bytes.len().min(PART_MAX)]).into_owned();
    for (content_type, bytes) in parts {
        let kind = content_type
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        match kind.as_str() {
            "message/delivery-status" | "message/global-delivery-status" if report.report_type.is_empty() => {
                report.report_type = "dsn".to_string();
                report.status = delivery_status(&text_of(bytes));
            }
            "message/feedback-report" if report.report_type.is_empty() => {
                report.report_type = "arf".to_string();
                report.feedback = feedback_report(&text_of(bytes));
            }
            "message/rfc822" | "text/rfc822-headers" | "message/global" | "message/global-headers"
                if report.original_message_id.is_empty() =>
            {
                report.original_message_id = message_id_of(&text_of(bytes));
            }
            _ => {}
        }
    }
    let mut delivered: Vec<String> = header_values(headers, "delivered-to")
        .iter()
        .map(|value| address_of(value))
        .filter(|address| !address.is_empty())
        .collect();
    if delivered.is_empty() {
        delivered = to_addresses
            .split(',')
            .map(|item| {
                let item = item.trim();
                item.rsplit_once('<')
                    .map_or(item, |(_, rest)| rest.trim_end_matches('>'))
                    .trim()
                    .to_string()
            })
            .filter(|address| address.contains('@'))
            .collect();
    }
    delivered.dedup();
    report.delivered_to = delivered;
    report
}

/// `read#DeliveryReport`: the report a stored message is ([`StoredReport`]), from its attachments and header block. A
/// message that is not there is an error; one that is not a report answers an empty `reportType`.
pub(super) fn read_delivery_report(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let email_message_id = param_text(parameters, "emailMessageId");
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let Some(message) = find_one(facade, MESSAGE, "emailMessageId", &email_message_id)? else {
        return Err(LibError::new(format!(
            "No EmailMessage record found for ID {email_message_id}"
        )));
    };
    let attachments = facade
        .find(ATTACHMENT)?
        .eq("emailMessageId", Value::string(&email_message_id))
        .order_by("emailMessageAttachmentId", false)
        .disable_authz()
        .list()?;
    let mut parts = Vec::new();
    for row in attachments.values() {
        let content_type = field_text(row, "contentType");
        let kind = content_type.to_ascii_lowercase();
        if !(kind.starts_with("message/") || kind.starts_with("text/rfc822-headers")) {
            continue;
        }
        let location = field_text(row, "attachmentLocation");
        let bytes = ExecutionContext::with_current(|ec| ec.factory().resource().location_bytes(&location))
            .ok_or_else(|| LibError::new("read#DeliveryReport needs an execution context"))?;
        match bytes {
            Ok(Some(bytes)) => parts.push((content_type, bytes)),
            Ok(None) => tracing::warn!("Email message {email_message_id}: its part {location} is not there, skipped"),
            Err(err) => {
                tracing::warn!("Email message {email_message_id}: its part {location} was not read, skipped: {err}")
            }
        }
    }
    let report = stored_report(
        &field_text(&message, "headersString"),
        &field_text(&message, "toAddresses"),
        &parts,
    );
    Ok(report_out(&report))
}

fn strings(items: &[String]) -> Value {
    Value::List(items.iter().map(Value::string).collect())
}

/// The service's out-parameters of a report.
pub(crate) fn report_out(report: &StoredReport) -> BTreeMap<String, Value> {
    let recipients = report
        .status
        .recipients
        .iter()
        .map(|recipient| {
            let mut map = IndexMap::default();
            map.insert("finalRecipient".into(), Value::string(&recipient.final_recipient));
            map.insert("originalRecipient".into(), Value::string(&recipient.original_recipient));
            map.insert("action".into(), Value::string(&recipient.action));
            map.insert("status".into(), Value::string(&recipient.status));
            map.insert("diagnosticCode".into(), Value::string(&recipient.diagnostic));
            Value::Map(map)
        })
        .collect();
    BTreeMap::from([
        ("reportType".to_string(), Value::string(&report.report_type)),
        ("recipientList".to_string(), Value::List(recipients)),
        (
            "originalEnvelopeId".to_string(),
            Value::string(&report.status.original_envelope_id),
        ),
        (
            "feedbackType".to_string(),
            Value::string(&report.feedback.feedback_type),
        ),
        (
            "originalMailFrom".to_string(),
            Value::string(&report.feedback.original_mail_from),
        ),
        (
            "originalRcptToList".to_string(),
            strings(&report.feedback.original_rcpt_to),
        ),
        ("arrivalDate".to_string(), Value::string(&report.feedback.arrival_date)),
        (
            "reportedDomain".to_string(),
            Value::string(&report.feedback.reported_domain),
        ),
        (
            "originalMessageId".to_string(),
            Value::string(&report.original_message_id),
        ),
        ("deliveredToList".to_string(), strings(&report.delivered_to)),
    ])
}

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;
