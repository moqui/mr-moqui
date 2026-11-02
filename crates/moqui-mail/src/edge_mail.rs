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

//! Mail through an edge node (plans/Edge-Node.md §5.4, E5a): the link's queue carries it, the message itself as the
//! item's content.
//!
//! - **In** (kind `mail`): the edge node's SMTP listener (moqui-host `smtp.rs`, its queue backend) answers `RCPT TO`
//!   from the replicated set `mailRecipients` ([`super::delivery::RecipientMap`]) and queues each message with its
//!   envelope and recipient addresses ([`inbound_item`]). The local node's `deliver#EdgeMail` resolves the addresses
//!   again from its own accounts, never the edge node's, and delivers it as the listener does
//!   ([`super::delivery::deliver`]); its Message-ID check makes a redelivery do nothing.
//! - **Out** (kind `mailOut`): a message sent through an `EmailServer` whose `smtpHost` is `edge:` is queued for the
//!   edge node instead ([`enqueue_outbound`]), so a send does not wait on or fail with the link; it is Sent once
//!   queued. The edge node's `deliver#EdgeOutboundMail` relays it to the MTA beside it (Postfix at
//!   `edge_mail_relay_host` and `edge_mail_relay_port`, which signs it with DKIM through Rspamd and delivers it to the
//!   MX), with the envelope sender `bounce+<item id>@<edge_mail_bounce_domain>` (VERP) when that is set. A relay that
//!   cannot be reached, answers 4xx or drops the connection answers `retryLater`: the item waits, not counted, to the
//!   end of the retention period. An answer, accepted or refused with 5xx, goes back as kind `mailStatus` for the
//!   `mailOut` item, queued in a transaction of its own right after the relay, so a later failure never relays twice.
//!   With the MTA in moqui (E5b, `edge_mail_mta` `moqui`, the default) the message is DKIM-signed and recorded for MX
//!   delivery in the item's transaction instead ([`super::mx`]), which sends the `mailStatus` answers.
//! - **Bounces after the relay**: a delivery status notification (RFC 3464) to the VERP address comes back to the edge
//!   node's listener, which takes it only for a `mailOut` item it received from a paired node, reads it
//!   ([`parse_dsn`]) and queues a `mailStatus` for that item ([`dsn_status_item`]) instead of mail.
//! - **Status** (kind `mailStatus`): `note#EdgeMailStatus` on the local node finds its own `mailOut` item by id, never
//!   by what the edge node names, and puts a refused message still Sent back in its account's Outbox (`ES_READY` with
//!   `sendError`), or marks a template's message `ES_BOUNCED`. A `mailOut` item that fails for good on the local node
//!   (unpairing, the retention period) does the same through `fail#EdgeOutboundMail`, its kind's failed-service.
//!
//! Each handler runs as `_EDGE_` in the transaction of the item's receipt, and checks the node's role: the local node
//! does not relay, and the edge node does not deliver to accounts.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use moqui_core::context::{Error, ExecutionContext, ExecutionContextFactory};
use moqui_core::edge::queue::{self, NewItem};
use moqui_core::entity::EntityFacade;
use moqui_core::service::util::{field_text, param_text};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use super::implementation::find_one;

/// The queue kind of a message the edge node received, for the local node.
pub const KIND_IN: &str = "mail";
/// The queue kind of a message the local node sends through its edge node.
pub const KIND_OUT: &str = "mailOut";
/// The queue kind of the edge node's answer to a `mailOut`.
pub const KIND_STATUS: &str = "mailStatus";
/// The replicated set the edge node answers `RCPT TO` from.
pub const RECIPIENT_SET: &str = "mailRecipients";
/// The `smtpHost` of an `EmailServer` that sends through the edge node.
pub const EDGE_HOST: &str = "edge:";
/// How long the relay to the MTA beside the edge node may take.
const RELAY_TIMEOUT: Duration = Duration::from_secs(30);
/// The local part of a VERP envelope sender, before the `mailOut` item's id.
const BOUNCE_PREFIX: &str = "bounce+";

const MESSAGE: &str = "moqui.basic.email.EmailMessage";

/// The result of a handler that cannot apply its item now for a reason that is not the item's: the queue rolls it
/// back and tries it again, not counted (`moqui_core::edge::queue::apply`).
fn retry_later(reason: String) -> BTreeMap<String, Value> {
    tracing::warn!("{reason}; the item waits for another try");
    BTreeMap::from([("retryLater".to_string(), Value::string(reason))])
}

/// The domain of the VERP envelope sender of relayed mail (`edge_mail_bounce_domain`), empty for none: then the
/// message's own sender gets the remote MX's bounces.
#[must_use]
pub fn bounce_domain() -> String {
    moqui_lib::env::get_prop_or_env("edge_mail_bounce_domain")
        .trim()
        .to_ascii_lowercase()
}

/// The `mailOut` item id of a VERP address `bounce+<id>@<domain>` of `domain`, or none.
#[must_use]
pub fn bounce_item_id(address: &str, domain: &str) -> Option<String> {
    if domain.is_empty() {
        return None;
    }
    let (local, at) = address.trim().rsplit_once('@')?;
    if !at.eq_ignore_ascii_case(domain) {
        return None;
    }
    let id = local
        .get(..BOUNCE_PREFIX.len())?
        .eq_ignore_ascii_case(BOUNCE_PREFIX)
        .then(|| &local[BOUNCE_PREFIX.len()..])?;
    let plain = !id.is_empty() && id.len() <= 64 && id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-');
    plain.then(|| id.to_ascii_lowercase())
}

/// What a delivery status notification says of its recipients: the worst action (`failed` over `delayed`), with each
/// such recipient's status and diagnostic.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DsnReport {
    /// `failed`, `delayed`, or empty when no recipient failed or was delayed (a `delivered` or `relayed` report).
    pub action: String,
    /// `Final-Recipient`, `Status` and `Diagnostic-Code` of each such recipient, for the message's send error.
    pub detail: String,
}

/// Read a delivery status notification (RFC 3464): the `message/delivery-status` part of a `multipart/report`, its
/// per-recipient field groups. None when the message has no such part.
#[must_use]
pub fn parse_dsn(raw: &[u8]) -> Option<DsnReport> {
    let text = String::from_utf8_lossy(raw);
    let lower = text.to_ascii_lowercase();
    let at = lower.find("message/delivery-status")?;
    // the part's own header ends at its first empty line; its body is field groups separated by empty lines, up to
    // the next boundary
    let rest = text.get(at..)?.replace("\r\n", "\n");
    let (_, body) = rest.split_once("\n\n")?;
    let body = body.split("\n--").next().unwrap_or(body);
    let mut report = DsnReport::default();
    let mut details = Vec::new();
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
        let field = |name: &str| {
            fields
                .iter()
                .find(|(field, _)| field == name)
                .map(|(_, value)| value.clone())
                .unwrap_or_default()
        };
        let action = field("action").to_ascii_lowercase();
        if action != "failed" && action != "delayed" {
            continue;
        }
        if report.action != "failed" {
            report.action = action;
        }
        details.push(
            [field("final-recipient"), field("status"), field("diagnostic-code")]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("; "),
        );
    }
    report.detail = details.join(" | ");
    Some(report)
}

/// The `mailStatus` item for the local node's `mailOut` item `mail_out_id`, from a delivery status notification
/// (`bounced` for a failed action, `delayed` for a delayed one). Keyed by the item and the report, so the MTA's retry of
/// the same notification is one item.
#[must_use]
pub fn dsn_status_item(mail_out_id: &str, report: &DsnReport) -> NewItem {
    let status = if report.action == "failed" {
        "bounced"
    } else {
        "delayed"
    };
    let key = moqui_lib::json::canonical_sha256(&serde_json::json!([mail_out_id, status, report.detail]));
    NewItem {
        kind: KIND_STATUS.to_string(),
        conversation_key: format!("mailStatus:{mail_out_id}"),
        idempotency_key: format!("dsn:{key}"),
        payload: Some(
            serde_json::json!({"queueItemId": mail_out_id, "status": status, "reply": report.detail}).to_string(),
        ),
        content: None,
        content_location: None,
    }
}

/// Whether an `EmailServer`'s `smtpHost` sends through the edge node.
#[must_use]
pub fn is_edge_host(host: &str) -> bool {
    host.trim().eq_ignore_ascii_case(EDGE_HOST)
}

fn current_factory() -> Result<Arc<ExecutionContextFactory>, LibError> {
    ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
        .ok_or_else(|| LibError::new("edge mail needs an execution context"))
}

/// Whether this node is an edge node, as its edge conf was read (per factory, so the two servers of a test process
/// keep their own roles).
fn on_edge_node(factory: &ExecutionContextFactory) -> bool {
    moqui_core::edge::settings(factory).edge_node
}

/// The item's content: the message, where the receiver kept it.
fn content(facade: &EntityFacade, parameters: &BTreeMap<String, Value>) -> Result<Vec<u8>, LibError> {
    let location = param_text(parameters, "contentLocation");
    if location.is_empty() {
        return Err(LibError::new(format!(
            "edge queue item {} has no message",
            param_text(parameters, "queueItemId")
        )));
    }
    facade
        .read_db_resource_bytes(&location)?
        .ok_or_else(|| LibError::new(format!("the message of an edge queue item, {location}, is not there")))
}

fn payload(parameters: &BTreeMap<String, Value>) -> Result<serde_json::Value, LibError> {
    let text = param_text(parameters, "payload");
    serde_json::from_str(&text).map_err(|err| {
        LibError::new(format!(
            "edge queue item {} has a payload that is not JSON: {err}",
            param_text(parameters, "queueItemId")
        ))
    })
}

fn json_text(value: &serde_json::Value, name: &str) -> String {
    value[name].as_str().unwrap_or_default().to_string()
}

// ---- in: the edge node's listener to the local node's accounts ----

/// The queue item of a message the edge node's listener took, for the local node: its envelope and recipient
/// addresses with their DSN parameters as the payload (never account ids: the local node resolves them again), the
/// message as content. The idempotency key is its Message-ID (or, with none, a hash of the message) with the
/// recipients, so the gateway's retry of a message the listener had queued is the same item; the conversation key
/// is `conversation`, the recipient's mailbox.
#[cfg(feature = "imap")]
#[must_use]
pub fn inbound_item(
    raw: Vec<u8>,
    envelope: &super::delivery::Envelope,
    recipients: &[super::delivery::RecipientDsn],
    conversation: &str,
) -> NewItem {
    inbound_item_trusting(
        raw,
        envelope,
        recipients,
        conversation,
        &super::delivery::Options::default(),
    )
}

/// [`inbound_item`] with what the edge node's listener trusts (`trusted`): its authserv-id and spam verdict fields, which
/// the local node then trusts for this message, so the edge node's own `Authentication-Results` (the MTA in moqui) is
/// kept whatever the local node's `smtp_authserv_id` is.
#[cfg(feature = "imap")]
#[must_use]
pub fn inbound_item_trusting(
    raw: Vec<u8>,
    envelope: &super::delivery::Envelope,
    recipients: &[super::delivery::RecipientDsn],
    conversation: &str,
    trusted: &super::delivery::Options,
) -> NewItem {
    let payload = serde_json::json!({
        "authservId": trusted.authserv_id,
        "spamHeaders": trusted.spam_headers,
        "mailFrom": envelope.mail_from,
        "helo": envelope.helo,
        "clientIp": envelope.client_ip,
        "protocol": envelope.protocol,
        "sessionId": envelope.session_id,
        "hostname": envelope.hostname,
        "dsnRet": envelope.dsn_ret,
        "dsnEnvid": envelope.dsn_envid,
        "recipients": recipients
            .iter()
            .map(|recipient| serde_json::json!({
                "address": recipient.address,
                "notify": recipient.notify,
                "original": recipient.original,
            }))
            .collect::<Vec<_>>(),
    });
    let head_end = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .or_else(|| raw.windows(2).position(|window| window == b"\n\n"))
        .unwrap_or(raw.len());
    let head = String::from_utf8_lossy(&raw[..head_end]);
    let message_id = super::implementation::header_value(&head, "Message-ID");
    let identity = if message_id.trim().is_empty() {
        use sha2::Digest as _;
        format!("sha256:{}", moqui_lib::codec::hex_encode(&sha2::Sha256::digest(&raw)))
    } else {
        message_id.trim().to_string()
    };
    let mut addresses: Vec<String> = recipients
        .iter()
        .map(|recipient| recipient.address.to_lowercase())
        .collect();
    addresses.sort_unstable();
    addresses.dedup();
    let key = moqui_lib::json::canonical_sha256(&serde_json::json!([identity, addresses]));
    NewItem {
        kind: KIND_IN.to_string(),
        conversation_key: conversation.to_string(),
        idempotency_key: format!("mail:{key}"),
        payload: Some(payload.to_string()),
        content: Some(raw),
        content_location: None,
    }
}

/// `deliver#EdgeMail`, on the local node: a message its edge node took, delivered to the accounts its recipient
/// addresses resolve to here, as the SMTP listener delivers. An address no account takes now is logged and left (the
/// edge node accepted it while the account was there). A store that failed answers `retryLater`, so the queue tries it
/// again, not counted; the accounts that stored it have its Message-ID and do not store it twice. A message that is not
/// mail is an error, counted.
#[cfg(feature = "imap")]
pub(super) fn deliver_edge_mail(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    use super::delivery::{deliver, find_recipient, listener_options, Envelope, RecipientDsn, Recipients};
    let factory = current_factory()?;
    if on_edge_node(&factory) {
        return Err(LibError::new(
            "deliver#EdgeMail runs on a local node, not on an edge node",
        ));
    }
    let item = param_text(parameters, "queueItemId");
    let payload = payload(parameters)?;
    let raw = content(facade, parameters)?;
    let envelope = Envelope {
        mail_from: json_text(&payload, "mailFrom"),
        helo: json_text(&payload, "helo"),
        client_ip: json_text(&payload, "clientIp"),
        protocol: json_text(&payload, "protocol"),
        session_id: json_text(&payload, "sessionId"),
        hostname: json_text(&payload, "hostname"),
        dsn_ret: json_text(&payload, "dsnRet"),
        dsn_envid: json_text(&payload, "dsnEnvid"),
    };
    let (mut options, delimiters) = listener_options(&factory);
    // what the edge node's listener trusted, it wrote itself or checked: its authserv-id and spam fields
    let edge_authserv = json_text(&payload, "authservId");
    if !edge_authserv.trim().is_empty() {
        options.authserv_id = edge_authserv;
    }
    for name in payload["spamHeaders"].as_array().map(Vec::as_slice).unwrap_or_default() {
        if let Some(name) = name
            .as_str()
            .filter(|name| !options.spam_headers.iter().any(|seen| seen.eq_ignore_ascii_case(name)))
        {
            options.spam_headers.push(name.to_string());
        }
    }
    // the delivery reads and writes the accounts as the listener does, with authorization off: _EDGE_ was authorized
    // for this service, and the addresses are resolved here
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut recipients: Vec<Recipients> = Vec::new();
    for entry in payload["recipients"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let dsn = RecipientDsn {
            address: json_text(entry, "address"),
            notify: json_text(entry, "notify"),
            original: json_text(entry, "original"),
        };
        let account = match find_recipient(facade, &dsn.address, &delimiters) {
            Ok(account) => account,
            Err(err) => {
                return Ok(retry_later(format!(
                    "Edge mail item {item}: the accounts were not read: {err}"
                )))
            }
        };
        let Some(account) = account else {
            tracing::warn!(
                "Edge mail item {item}: no account here takes {}, which the edge node accepted; it is left out",
                dsn.address
            );
            continue;
        };
        let group = match recipients.iter().position(|group| group.email_account_id == account) {
            Some(at) => &mut recipients[at],
            None => {
                recipients.push(Recipients {
                    email_account_id: account,
                    ..Recipients::default()
                });
                let last = recipients.len() - 1;
                &mut recipients[last]
            }
        };
        if !group.addresses.contains(&dsn.address) {
            group.addresses.push(dsn.address.clone());
        }
        if !(dsn.notify.is_empty() && dsn.original.is_empty())
            && !group.dsn.iter().any(|seen| seen.address == dsn.address)
        {
            group.dsn.push(dsn);
        }
    }
    if recipients.is_empty() {
        return Ok(BTreeMap::new());
    }
    let report = match deliver(facade, &raw, &envelope, &recipients, &options) {
        Ok(report) => report,
        // not mail: an error, counted, so it fails for good and is kept for inspection
        Err(text) => {
            tracing::error!("Edge mail item {item} is not mail: {text}");
            return Err(LibError::new(format!("edge mail item {item} is not mail: {text}")));
        }
    };
    if report.store_failed() {
        let failed: Vec<String> = report
            .deliveries
            .iter()
            .filter_map(|delivery| match &delivery.result {
                Err(text) => Some(format!("account {}: {text}", delivery.email_account_id)),
                Ok(_) => None,
            })
            .collect();
        // a store fails for this node's own reasons (its database, its files): not the item's, so not counted
        return Ok(retry_later(format!(
            "Edge mail item {item} was not stored for every account: {}",
            failed.join("; ")
        )));
    }
    tracing::info!(
        "Edge mail item {item} delivered to {} account(s) from <{}>",
        report.deliveries.len(),
        envelope.mail_from
    );
    Ok(BTreeMap::new())
}

// ---- out: the local node's sends through the edge node ----

/// Queue a message built whole (`raw`) for the edge node to relay, from `from` (the null reverse path when empty) to
/// `to`; the edge node's answer comes back as a `mailStatus` for `email_message_id` (none for a message with no row).
/// The item is in the caller's transaction when there is one.
pub(super) fn enqueue_outbound(
    facade: &EntityFacade,
    email_message_id: &str,
    message_id: &str,
    from: &str,
    to: &[String],
    raw: Vec<u8>,
) -> Result<(), Error> {
    if to.is_empty() {
        return Err(Error::new("a message for the edge node needs a recipient"));
    }
    let factory = current_factory().map_err(Error::from)?;
    // after unpairing nothing queues for whichever edge node comes next: the send fails, retryable, and the message
    // stays in the Outbox (plans/Edge-Node.md §5.7)
    if !moqui_core::edge::settings(&factory).edge_node && !moqui_core::edge::setup::is_paired(facade) {
        return Err(Error::new(
            "this node is not paired with an edge node, so mail for its edge: server is not queued",
        ));
    }
    let payload = serde_json::json!({
        "emailMessageId": email_message_id,
        "messageId": message_id,
        "from": from,
        "to": to,
    });
    let key = match (email_message_id, message_id) {
        ("", "") => {
            // a notice built whole with no row and no Message-ID of its own (a delivered DSN): its own hash
            use sha2::Digest as _;
            moqui_lib::codec::hex_encode(&sha2::Sha256::digest(&raw))
        }
        ("", message_id) => message_id.to_string(),
        (email_message_id, _) => email_message_id.to_string(),
    };
    let item = NewItem {
        kind: KIND_OUT.to_string(),
        // one key per message, so a message the relay keeps refusing holds no other
        conversation_key: format!("mailOut:{key}"),
        idempotency_key: String::new(),
        payload: Some(payload.to_string()),
        content: Some(raw),
        content_location: None,
    };
    let queued = queue::enqueue(&factory, facade, None, &item).map_err(Error::from)?;
    tracing::info!(
        "Email message {email_message_id} ({message_id}) queued for the edge node as item {}",
        queued.queue_item_id
    );
    Ok(())
}

/// What the relay answered.
enum Relayed {
    Accepted(String),
    /// Refused for good: a 5xx, or an address or envelope the relay could never take.
    Refused(String),
    /// Not now: the relay not reached, a 4xx, a lost connection.
    NotNow(String),
}

/// The MTA beside the edge node: `edge_mail_relay_host` (default 127.0.0.1) and `edge_mail_relay_port` (default 10025).
fn relay_address() -> (String, u16) {
    let host = moqui_lib::env::get_prop_or_env("edge_mail_relay_host");
    let host = match host.trim() {
        "" => "127.0.0.1".to_string(),
        host => host.to_string(),
    };
    let port = moqui_lib::env::get_prop_or_env("edge_mail_relay_port")
        .trim()
        .parse()
        .unwrap_or(10025);
    (host, port)
}

/// Relay `raw` to the MTA beside the edge node from envelope sender `from` (the null reverse path when empty).
fn relay(from: &str, to: &[String], raw: &[u8]) -> Relayed {
    let (host, port) = relay_address();
    if let Err(err) = moqui_core::entity::fence::check_effect(moqui_core::entity::fence::Effect::Mail, &host) {
        return Relayed::NotNow(err.to_string());
    }
    let sender = match from.trim() {
        "" => None,
        from => match from.parse::<lettre::Address>() {
            Ok(address) => Some(address),
            Err(err) => return Relayed::Refused(format!("{from} is not an address: {err}")),
        },
    };
    let mut recipients = Vec::with_capacity(to.len());
    for address in to {
        match address.parse::<lettre::Address>() {
            Ok(address) => recipients.push(address),
            Err(err) => return Relayed::Refused(format!("{address} is not an address: {err}")),
        }
    }
    let envelope = match lettre::address::Envelope::new(sender, recipients) {
        Ok(envelope) => envelope,
        Err(err) => return Relayed::Refused(err.to_string()),
    };
    let transport = lettre::SmtpTransport::builder_dangerous(&host)
        .port(port)
        .timeout(Some(RELAY_TIMEOUT))
        .build();
    match lettre::Transport::send_raw(&transport, &envelope, raw) {
        Ok(response) => Relayed::Accepted(format!(
            "{} {}",
            response.code(),
            response.message().collect::<Vec<_>>().join(" ")
        )),
        Err(err) if err.is_permanent() => Relayed::Refused(err.to_string()),
        Err(err) => Relayed::NotNow(format!("the relay {host}:{port}: {err}")),
    }
}

/// The idempotency key of the relay's answer to `mailOut` item `item`.
fn status_key(item: &str) -> String {
    format!("status:{item}")
}

/// Whether the relay's answer to `mailOut` item `item` of `node` is queued already: it was relayed, and an apply that
/// ran again after a rollback must not relay it twice.
fn relayed_before(facade: &EntityFacade, node: &str, item: &str) -> Result<bool, LibError> {
    let found = facade
        .find(queue::QUEUE_ITEM)?
        .eq("pairedNodeId", Value::string(node))
        .eq("directionEnumId", Value::string(queue::TO_LOCAL))
        .eq("idempotencyKey", Value::string(status_key(item)))
        .select_field("queueItemId")
        .disable_authz()
        .one()?;
    Ok(found.is_some())
}

/// `deliver#EdgeOutboundMail`, on the edge node: a message its local node sent, relayed to the MTA beside it, from the
/// VERP sender when `edge_mail_bounce_domain` is set. The answer is queued back (`mailStatus`) in a transaction of its
/// own right after the relay, and an apply that finds it queued does not relay again. A relay that cannot take it now
/// answers `retryLater`, so the item waits, not counted.
pub(super) fn deliver_edge_outbound_mail(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let factory = current_factory()?;
    if !on_edge_node(&factory) {
        return Err(LibError::new(
            "deliver#EdgeOutboundMail runs on an edge node, not on a local node",
        ));
    }
    let item = param_text(parameters, "queueItemId");
    let node = param_text(parameters, "pairedNodeId");
    if relayed_before(facade, &node, &item)? {
        tracing::info!("Edge outbound mail item {item} was relayed before; not again");
        return Ok(BTreeMap::new());
    }
    let payload = payload(parameters)?;
    let raw = content(facade, parameters)?;
    let to: Vec<String> = payload["to"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|entry| entry.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let from = json_text(&payload, "from");
    #[cfg(feature = "imap")]
    let original_from = from.clone();
    let domain = bounce_domain();
    // the null reverse path (a notice) stays null; any other gets the VERP sender, so a bounce names this item
    let sender = if from.trim().is_empty() || domain.is_empty() {
        from
    } else {
        format!("{BOUNCE_PREFIX}{item}@{domain}")
    };
    #[cfg(feature = "imap")]
    if super::mx::mta() == super::mx::Mta::Moqui {
        // the MTA in moqui (E5b): signed here, recorded per recipient domain in this item's transaction, delivered to
        // the MX by the delivery thread; the answers come back as mailStatus from there
        let hosted = super::auth::hosted_domains(facade, true)?;
        let signed = super::auth::sign_for(raw, |domain| hosted.contains(domain));
        // the local node's own accounts are queued back to it, not sent round through this node's port 25
        let (local, external) = split_local(facade, &factory, &node, &to)?;
        if !local.is_empty() {
            queue_back(&factory, facade, &node, &item, &original_from, &local, &signed)?;
        }
        let rows = super::mx::record(facade, &node, &item, &sender, &external, &local, &signed)?;
        tracing::info!(
            "Edge outbound mail item {item}: {} queued back to the local node, {rows} domain(s) to deliver to by MX",
            local.len()
        );
        return Ok(BTreeMap::new());
    }
    let (status, reply) = match relay(&sender, &to, &raw) {
        Relayed::Accepted(reply) => ("accepted", reply),
        Relayed::Refused(reply) => ("bounced", reply),
        Relayed::NotNow(reason) => return Ok(retry_later(format!("Edge outbound mail item {item}: {reason}"))),
    };
    tracing::info!("Edge outbound mail item {item} to {}: {status}, {reply}", to.join(", "));
    let back = NewItem {
        kind: KIND_STATUS.to_string(),
        conversation_key: format!("mailStatus:{item}"),
        // one answer per item, which also marks it relayed (relayed_before)
        idempotency_key: status_key(&item),
        payload: Some(serde_json::json!({"queueItemId": item, "status": status, "reply": reply}).to_string()),
        content: None,
        content_location: None,
    };
    // its own transaction, committed now: the relay took it, so whatever happens to this apply it is not relayed again
    let node = Some(node.as_str()).filter(|node| !node.is_empty());
    let queued = facade.transactions_for(queue::QUEUE_ITEM).and_then(|tx| {
        tx.run(moqui_core::transaction::TransactionAttribute::ForceNew, None, || {
            queue::enqueue(&factory, facade, node, &back)
        })
    });
    if let Err(err) = queued {
        tracing::error!("Edge outbound mail item {item} was relayed ({status}), but its answer was not queued: {err}");
    }
    Ok(BTreeMap::new())
}

/// The recipients of `to` that are local node `node`'s own accounts (its replicated `mailRecipients`), and the rest.
#[cfg(feature = "imap")]
fn split_local(
    facade: &EntityFacade,
    factory: &ExecutionContextFactory,
    node: &str,
    to: &[String],
) -> Result<(Vec<String>, Vec<String>), LibError> {
    let values = moqui_core::edge::replica::list_values(facade, Some(node), RECIPIENT_SET, None, 1_000_000)?;
    let maps = super::delivery::recipient_maps(&values);
    let (_, delimiters) = super::delivery::listener_options(factory);
    let (mut local, mut external) = (Vec::new(), Vec::new());
    for address in to {
        let ours = maps
            .iter()
            .any(|(of, map)| of == node && map.find(address, &delimiters).is_some());
        if ours {
            local.push(address.clone());
        } else {
            external.push(address.clone());
        }
    }
    Ok((local, external))
}

/// Queue `signed` back to local node `node` as mail for its own accounts `local` (internal mail through the edge node),
/// with the `Authentication-Results` of this node's own key on it, in the item's transaction.
#[cfg(feature = "imap")]
fn queue_back(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    node: &str,
    item: &str,
    from: &str,
    local: &[String],
    signed: &[u8],
) -> Result<(), LibError> {
    use super::delivery::{Envelope, Options, RecipientDsn};
    let hostname = super::mx::hostname();
    let mut raw = Vec::with_capacity(signed.len() + 512);
    if let Some(field) = super::auth::verify_internal(signed, &hostname) {
        raw.extend_from_slice(field.as_bytes());
    }
    raw.extend_from_slice(signed);
    let envelope = Envelope {
        mail_from: from.to_string(),
        helo: hostname.clone(),
        client_ip: "127.0.0.1".to_string(),
        protocol: "ESMTP".to_string(),
        session_id: item.to_string(),
        hostname: hostname.clone(),
        ..Envelope::default()
    };
    let recipients: Vec<RecipientDsn> = local
        .iter()
        .map(|address| RecipientDsn {
            address: address.clone(),
            ..RecipientDsn::default()
        })
        .collect();
    let trusted = Options {
        spam_headers: Vec::new(),
        authserv_id: hostname,
    };
    let back = inbound_item_trusting(raw, &envelope, &recipients, &format!("mail:{item}"), &trusted);
    queue::enqueue(factory, facade, Some(node), &back)?;
    Ok(())
}

/// The local node's `mailOut` item `item`: the message it carried, `(emailMessageId, messageId)`.
fn outbound_message(facade: &EntityFacade, item: &str) -> Result<Option<(String, String)>, LibError> {
    let Some(row) = facade
        .find(queue::QUEUE_ITEM)?
        .eq("queueItemId", Value::string(item))
        .eq("directionEnumId", Value::string(queue::TO_EDGE))
        .eq("kind", Value::string(KIND_OUT))
        .select_field("payload")
        .disable_authz()
        .one()?
    else {
        return Ok(None);
    };
    let payload: serde_json::Value = serde_json::from_str(&field_text(&row, "payload")).unwrap_or_default();
    Ok(Some((
        json_text(&payload, "emailMessageId"),
        json_text(&payload, "messageId"),
    )))
}

/// Put message `email_message_id` back where a failed send leaves it, with `error`: a mail client's message in its
/// account's Outbox (`ES_READY` with `sendError`, no folder), another `ES_BOUNCED` with `sendError`. Only while it is
/// still as the send left it (Sent, with Message-ID `message_id`), so a message sent again or cancelled since is left.
fn back_to_outbox(
    facade: &EntityFacade,
    email_message_id: &str,
    message_id: &str,
    error: &str,
) -> Result<(), LibError> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let Some(message) = find_one(facade, MESSAGE, "emailMessageId", email_message_id)? else {
        tracing::warn!(
            "Email message {email_message_id} did not go out through the edge node, but it is gone: {error}"
        );
        return Ok(());
    };
    if field_text(&message, "statusId") != "ES_SENT" || field_text(&message, "messageId") != message_id {
        tracing::warn!(
            "Email message {email_message_id} ({message_id}) did not go out through the edge node, but it is {} {} now; \
             left as it is: {error}",
            field_text(&message, "statusId"),
            field_text(&message, "messageId")
        );
        return Ok(());
    }
    let client = !field_text(&message, "emailAccountId").is_empty();
    let mut row = facade.make_value(MESSAGE)?;
    row.set("emailMessageId", Value::string(email_message_id));
    row.set("sendError", Value::string(error.chars().take(4000).collect::<String>()));
    if client {
        // the Outbox: ready with an error and no folder (EmailClientServices record#SendResult)
        row.set("statusId", Value::string("ES_READY"));
        row.set("emailFolderId", Value::Null);
        row.set("sendClaimDate", Value::Null);
    } else {
        row.set("statusId", Value::string("ES_BOUNCED"));
    }
    row.update(facade)?;
    tracing::warn!("Email message {email_message_id} ({message_id}) did not go out through the edge node: {error}");
    Ok(())
}

/// Some recipients of message `email_message_id` did not get it and others did: it stays Sent (a resend would reach
/// those that got it again), with `sendError` naming the refused, only while it is still as the send left it.
fn note_refused(
    facade: &EntityFacade,
    email_message_id: &str,
    message_id: &str,
    refused: &str,
) -> Result<(), LibError> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let Some(message) = find_one(facade, MESSAGE, "emailMessageId", email_message_id)? else {
        return Ok(());
    };
    if field_text(&message, "statusId") != "ES_SENT" || field_text(&message, "messageId") != message_id {
        tracing::warn!("Email message {email_message_id} ({message_id}) was not delivered to all; it is not as sent now: {refused}");
        return Ok(());
    }
    let mut row = facade.make_value(MESSAGE)?;
    row.set("emailMessageId", Value::string(email_message_id));
    row.set(
        "sendError",
        Value::string(
            format!("Not delivered to: {refused}")
                .chars()
                .take(4000)
                .collect::<String>(),
        ),
    );
    row.update(facade)?;
    tracing::warn!("Email message {email_message_id} ({message_id}) was sent, but not delivered to: {refused}");
    Ok(())
}

/// `note#EdgeMailStatus`, on the local node: the edge node's answer for one of this node's `mailOut` items, found by
/// its id in this node's own outbox (what the edge node names is never trusted for the message). A refusal (bounced,
/// from the relay or a later notification, or no recipient reached) puts the message back ([`back_to_outbox`]); a
/// partial delivery (some recipients refused, others reached) leaves it Sent with the refused named in its send error;
/// accepted and delayed are logged.
pub(super) fn note_edge_mail_status(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if on_edge_node(&*current_factory()?) {
        return Err(LibError::new(
            "note#EdgeMailStatus runs on a local node, not on an edge node",
        ));
    }
    let payload = payload(parameters)?;
    let item = json_text(&payload, "queueItemId");
    let status = json_text(&payload, "status");
    let reply = json_text(&payload, "reply");
    let Some((email_message_id, message_id)) = outbound_message(facade, &item)? else {
        tracing::warn!(
            "The edge node answered {status} for mail item {item}, which is not in this node's outbox: {reply}"
        );
        return Ok(BTreeMap::new());
    };
    if status == "partial" && !email_message_id.is_empty() {
        note_refused(facade, &email_message_id, &message_id, &reply)?;
        return Ok(BTreeMap::new());
    }
    if status != "bounced" || email_message_id.is_empty() {
        tracing::info!(
            "Email message {email_message_id} ({message_id}), item {item}: the edge node says {status}: {reply}"
        );
        return Ok(BTreeMap::new());
    }
    back_to_outbox(facade, &email_message_id, &message_id, &format!("Bounced: {reply}"))?;
    Ok(BTreeMap::new())
}

/// `fail#EdgeOutboundMail`, on the local node, the failed-service of kind `mailOut`: an item that failed for good
/// (unpairing, the retention period, the size cap) puts its message back ([`back_to_outbox`]), so the failure is not
/// silent.
pub(super) fn fail_edge_outbound_mail(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let payload = payload(parameters)?;
    let email_message_id = json_text(&payload, "emailMessageId");
    if email_message_id.is_empty() {
        tracing::error!(
            "Mail item {} for the edge node failed for good: {}",
            param_text(parameters, "queueItemId"),
            param_text(parameters, "error")
        );
        return Ok(BTreeMap::new());
    }
    back_to_outbox(
        facade,
        &email_message_id,
        &json_text(&payload, "messageId"),
        &format!("Not sent through the edge node: {}", param_text(parameters, "error")),
    )?;
    Ok(BTreeMap::new())
}

#[cfg(all(test, feature = "imap"))]
#[path = "edge_mail_tests.rs"]
mod tests;
