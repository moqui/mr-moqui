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

//! The edge node's MX delivery, the MTA in moqui (plans/Edge-Node.md §5.4, E5b), when `edge_mail_mta` is `moqui`
//! (MoquiEdgeConf.xml sets it; empty or `postfix` keeps E5a's relay to the MTA beside the edge node).
//!
//! - **Recorded.** `deliver#EdgeOutboundMail` signs the message with the node's DKIM key (`super::auth`), queues it
//!   back to the local node for the recipients that are the local node's own accounts, and records one
//!   `moqui.edge.EdgeMailDelivery` per other recipient domain in the transaction of the `mailOut` item's receipt
//!   ([`record`]), with the signed message kept once under `dbresource://moqui/edge/mail/`. The item is delivered once
//!   these rows commit.
//! - **Delivered.** A thread of its own ([`start`]) starts each row that is due on a thread of its own, at most eight
//!   at once and never two rows of one message together, and does not wait for them: one domain that hangs holds only
//!   its own thread. A try ends within 10 minutes (each command a minute at most), inside the row's 15-minute lease. A
//!   panic is caught per row and per round. A domain's MX hosts are tried by preference (its address when it has no
//!   MX; a null MX refuses), each host's addresses in turn, until one answers. The session is EHLO, STARTTLS when
//!   offered (opportunistic: any certificate), MAIL FROM (the VERP sender of E5a when `edge_mail_bounce_domain` is
//!   set), RCPT TO each recipient, DATA. Rows of a local node that is no longer paired fail, logged.
//! - **TLS policy.** MTA-STS (RFC 8461): the `_mta-sts` TXT record names the policy's id; the policy (64 KiB at most) is
//!   fetched over HTTPS and kept until its `max_age` (fetched again when the id changes). With `mode: enforce` only MX
//!   hosts the policy names are used, and STARTTLS with a certificate valid for the host is required, else the try
//!   waits. DANE is later work.
//! - **Answers.** A 2xx after the data is sent; a 5xx (to RCPT, or to the data) refuses that recipient for good; a 4xx,
//!   a host not reached or a lost connection waits for the next try: 5 minutes (`mail_delivery_retry_seconds`), twice
//!   as long each time, to 4 hours, until `mail_delivery_days` (5) after the row was made, when the recipients still
//!   waiting are refused with the last reply. A try's answers are committed on the row first; its report is queued
//!   after, in a transaction of its own, and one not queued is tried again each round (`reportPending`), never by
//!   delivering again.
//! - **Back to the local node** (`mailStatus` for the `mailOut` item). While other rows of the message wait, refused
//!   recipients go back as "partial". The last report: "accepted" when every recipient got it, "partial" when some did
//!   (the message stays Sent, with the refused named in its send error, so a resend does not reach the others twice),
//!   "bounced" when none did (back in the Outbox with the replies). No delivery status notification (RFC 3464) is made:
//!   the local node's status is the report, for a refusal and for a give-up alike.
//!
//! `mail_mx_override` (testing and staging only) maps recipient domains to `host:port` (`example.net=127.0.0.1:2525`,
//! `*` for any), skipping MX lookup and MTA-STS.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use lettre::transport::smtp::client::{SmtpConnection, TlsParameters};
use lettre::transport::smtp::commands::{Data, Mail, Quit, Rcpt};
use lettre::transport::smtp::extension::{ClientId, Extension, MailBodyParameter, MailParameter};
use moqui_core::context::ExecutionContextFactory;
use moqui_core::edge::queue::{self, NewItem};
use moqui_core::entity::{Condition, EntityFacade, EntityValue};
use moqui_core::service::util::field_text;
use moqui_core::transaction::TransactionAttribute;
use moqui_core::user::{now_millis, stamp_value, value_millis};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use moqui_lib::sync::{lock, wait_timeout};

const DELIVERY: &str = "moqui.edge.EdgeMailDelivery";
const PENDING: &str = "EmdPending";
const SENT: &str = "EmdSent";
const BOUNCED: &str = "EmdBounced";
/// Where the signed messages wait for their deliveries.
const CONTENT_ROOT: &str = "dbresource://moqui/edge/mail/";
/// The rows read in one round.
const ROUND_ROWS: usize = 40;
/// The rows of nodes no longer paired failed in one round; more wait for the next.
const UNPAIRED_ROWS: i64 = 500;
/// The most sessions at once, each a thread of its own.
const MAX_IN_FLIGHT: usize = 8;
/// How long a row tried now is held from another try (a crash mid-session leaves it to be tried after).
const LEASE: Duration = Duration::from_secs(15 * 60);
/// The longest one domain's try may take, all its hosts together; within the lease.
const SESSION_DEADLINE: Duration = Duration::from_secs(10 * 60);
/// The longest status reply sent back (the queue's payload is at most 1 MiB).
const MAX_REPORT: usize = 16 * 1024;
/// The `domain` of the row of the recipients queued back to the local node itself.
const LOCAL_DOMAIN: &str = "(local node)";
/// The longest wait between rounds when nothing is due sooner.
const IDLE: Duration = Duration::from_secs(30);
/// How long a connection and each SMTP command may take.
const SMTP_TIMEOUT: Duration = Duration::from_secs(60);
/// The longest retry wait.
const MAX_RETRY: Duration = Duration::from_secs(4 * 3600);
/// The SMTP port of an MX.
const SMTP_PORT: u16 = 25;

/// Which MTA delivers an edge node's outbound mail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mta {
    /// This module: MX delivery from moqui (E5b).
    Moqui,
    /// Relay to Postfix beside the edge node (E5a).
    Postfix,
}

/// `edge_mail_mta`: `moqui` for the MTA in moqui (MoquiEdgeConf.xml sets it), else E5a's relay to Postfix, so an
/// E5a deployment that never set it keeps its Postfix.
#[must_use]
pub fn mta() -> Mta {
    if moqui_lib::env::get_prop_or_env("edge_mail_mta")
        .trim()
        .eq_ignore_ascii_case("moqui")
    {
        Mta::Moqui
    } else {
        Mta::Postfix
    }
}

/// The name this node gives in EHLO: `smtp_hostname`, else `webapp_http_host`, else `localhost`.
#[must_use]
pub fn hostname() -> String {
    for name in ["smtp_hostname", "webapp_http_host"] {
        let value = moqui_lib::env::get_prop_or_env(name);
        if !value.trim().is_empty() {
            return value.trim().to_string();
        }
    }
    "localhost".to_string()
}

fn property_seconds(name: &str, default: f64) -> Duration {
    let seconds = moqui_lib::env::get_prop_or_env(name)
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .unwrap_or(default);
    Duration::from_secs_f64(seconds)
}

/// The wait before try `attempt + 1`, after `attempt` tries: `mail_delivery_retry_seconds` (300), doubled each time, to
/// 4 hours.
fn retry_after(attempt: u32) -> Duration {
    let first = property_seconds("mail_delivery_retry_seconds", 300.0);
    first
        .saturating_mul(1_u32 << attempt.saturating_sub(1).min(16))
        .min(MAX_RETRY.max(first))
}

/// How long a row is tried before its waiting recipients are refused: `mail_delivery_days` (5).
fn give_up_after() -> Duration {
    property_seconds("mail_delivery_days", 5.0) * 86_400
}

fn millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

/// The domain of an address, in lower case.
fn domain_of(address: &str) -> String {
    address
        .trim()
        .rsplit_once('@')
        .map(|(_, domain)| domain.trim_end_matches('.').to_ascii_lowercase())
        .unwrap_or_default()
}

/// Record the delivery of `raw` (signed already) from `mail_from` to `to`, sent by local node `node` as its `mailOut`
/// item `item`: the message kept once, and one row per recipient domain, in the caller's transaction; `local` are the
/// recipients the edge node queued back to the local node itself (its own accounts) or, on an all-in-one node, stored in
/// the server's own accounts, and `refused` (each `address: why`) those of the server's own accounts that did not take
/// it: one row of both, finished, reported with the rest. The delivery thread is woken once it commits.
///
/// # Errors
///
/// Returns why when a row or the message could not be written; the item is then not delivered.
#[allow(clippy::too_many_arguments)]
pub(super) fn record(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    node: &str,
    item: &str,
    mail_from: &str,
    to: &[String],
    local: &[String],
    refused: &[String],
    raw: &[u8],
) -> Result<usize, LibError> {
    let mut by_domain: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for address in to {
        let domain = domain_of(address);
        if domain.is_empty() {
            return Err(LibError::new(format!("{address} is not an address")));
        }
        let list = by_domain.entry(domain).or_default();
        if !list.iter().any(|seen| seen.eq_ignore_ascii_case(address)) {
            list.push(address.trim().to_string());
        }
    }
    let plain = |text: &str| {
        text.chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    };
    if !plain(node) || !plain(item) {
        return Err(LibError::new(format!(
            "mail item {item} of node {node} is not a plain name"
        )));
    }
    let location = if by_domain.is_empty() {
        String::new()
    } else {
        let location = format!("{CONTENT_ROOT}{node}/{item}.eml");
        factory.resource().write_bytes(&location, raw)?;
        location
    };
    let now = now_millis();
    let new_row = |domain: &str, recipients: &[String]| -> Result<EntityValue, LibError> {
        let mut row = facade.make_value(DELIVERY)?;
        row.set("mailDeliveryId", Value::string(moqui_core::entity::uuid_v4()?));
        row.set("pairedNodeId", Value::string(node));
        row.set("queueItemId", Value::string(item));
        row.set("domain", Value::string(domain));
        row.set(
            "recipientsJson",
            Value::string(serde_json::json!(recipients).to_string()),
        );
        row.set("mailFrom", Value::string(mail_from));
        row.set("contentLocation", Value::string(&location));
        row.set("attemptCount", Value::I64(0));
        row.set("bouncedCount", Value::I64(0));
        row.set("sentCount", Value::I64(0));
        row.set("reportPending", Value::string("N"));
        row.set("createdDate", stamp_value(now));
        Ok(row)
    };
    for (domain, recipients) in &by_domain {
        let mut row = new_row(domain, recipients)?;
        row.set("statusEnumId", Value::string(PENDING));
        row.set("nextAttemptDate", stamp_value(now));
        row.create(facade)?;
    }
    if !local.is_empty() || !refused.is_empty() {
        // queued back to the local node or stored here in this transaction: finished, and reported with the rest
        let mut row = new_row(LOCAL_DOMAIN, local)?;
        let status = if refused.is_empty() { SENT } else { BOUNCED };
        row.set("statusEnumId", Value::string(status));
        row.set("sentCount", Value::I64(i64::try_from(local.len()).unwrap_or(0)));
        row.set("bouncedCount", Value::I64(i64::try_from(refused.len()).unwrap_or(0)));
        row.set("refusedText", Value::string(clip(&refused.join(" | "), MAX_REPORT)));
        row.set("attemptCount", Value::I64(1));
        row.set(
            "lastReplyText",
            Value::string("queued back to the local node or stored here"),
        );
        row.set("reportPending", Value::string("Y"));
        row.set("finishedDate", stamp_value(now));
        row.create(facade)?;
    }
    // the rows are seen once the item's transaction commits
    match facade.transactions_for(DELIVERY).ok().filter(|tx| tx.in_transaction()) {
        Some(tx) => {
            if let Err(err) = tx.on_commit(wake) {
                tracing::debug!("MX delivery: no wake after commit: {err}");
                wake();
            }
        }
        None => wake(),
    }
    Ok(by_domain.len())
}

// ---- the delivery thread ----

/// Wakes the delivery thread for rows just recorded (they are tried once their transaction has committed: a round
/// that misses them sees them at the next) and when a session ends.
struct Signal {
    woken: Mutex<bool>,
    condvar: Condvar,
}

fn signal() -> &'static Signal {
    static SIGNAL: Signal = Signal {
        woken: Mutex::new(false),
        condvar: Condvar::new(),
    };
    &SIGNAL
}

/// Wake the delivery thread.
pub fn wake() {
    let signal = signal();
    *lock(&signal.woken) = true;
    signal.condvar.notify_all();
}

/// Whether a factory's delivery thread is running.
#[derive(Default)]
struct Started(AtomicBool);

/// The rows being tried now, and their messages: a row is not tried twice at once, nor two rows of one message (so
/// the last row of a message to finish sees the others finished).
#[derive(Default)]
struct Flight {
    rows: HashSet<String>,
    items: HashSet<String>,
}

fn flight() -> &'static Mutex<Flight> {
    static FLIGHT: std::sync::OnceLock<Mutex<Flight>> = std::sync::OnceLock::new();
    FLIGHT.get_or_init(|| Mutex::new(Flight::default()))
}

/// Start the delivery thread on a node that takes links (a dedicated edge node, or an all-in-one node, plans/Edge-Node.md
/// §10) whose `edge_mail_mta` is `moqui`; once per factory. It holds the factory only for a round, so a stopped server
/// ends it. It delivers only the rows [`record`] made for a paired node's `mailOut` items: an all-in-one node's own
/// mail goes out by its `EmailServer` as on a normal node, never through these rows, so it is not delivered twice.
pub fn start(factory: &Arc<ExecutionContextFactory>) {
    if !moqui_core::edge::settings(factory).takes_links() || mta() != Mta::Moqui || factory.entity().is_none() {
        return;
    }
    if factory.extension(|_| Started::default()).0.swap(true, Ordering::SeqCst) {
        return;
    }
    let weak: Weak<ExecutionContextFactory> = Arc::downgrade(factory);
    let spawned = std::thread::Builder::new()
        .name("moqui-mx".to_string())
        .spawn(move || run_loop(&weak));
    match spawned {
        Ok(_) => tracing::info!("MX delivery started (edge_mail_mta moqui)"),
        Err(err) => tracing::error!("MX delivery did not start: {err}"),
    }
}

fn run_loop(weak: &Weak<ExecutionContextFactory>) {
    let mut last_clean = 0_i64;
    loop {
        let next = {
            let Some(factory) = weak.upgrade() else {
                return;
            };
            if factory.is_shutting_down() {
                return;
            }
            let ran = std::panic::catch_unwind(AssertUnwindSafe(|| {
                let next = round(&factory);
                if now_millis() - last_clean > 3_600_000 {
                    last_clean = now_millis();
                    clean(&factory);
                }
                next
            }));
            match ran {
                Ok(next) => next,
                Err(payload) => {
                    tracing::error!(
                        "An MX delivery round failed: {}",
                        moqui_core::service::pool::panic_text(payload.as_ref())
                    );
                    None
                }
            }
        };
        let wait = next.map_or(IDLE, |at| {
            Duration::from_millis(u64::try_from(at - now_millis()).unwrap_or(0)).min(IDLE)
        });
        if wait.is_zero() {
            continue;
        }
        let signal = signal();
        let guard = lock(&signal.woken);
        let (mut guard, _) = if *guard {
            (guard, None)
        } else {
            let (guard, timeout) = wait_timeout(&signal.condvar, guard, wait);
            (guard, Some(timeout))
        };
        *guard = false;
    }
}

/// One round: the rows of nodes no longer paired failed, the reports not yet queued queued, and each row due started
/// on a thread of its own, up to [`MAX_IN_FLIGHT`] at once, without waiting for them (a slow or hung domain holds
/// only its own thread). When the round should run again (epoch millis), if anything waits.
fn round(factory: &Arc<ExecutionContextFactory>) -> Option<i64> {
    let facade = factory.entity()?;
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let now = now_millis();
    if let Err(err) = fail_unpaired(factory, &facade) {
        tracing::error!("MX delivery could not check its rows' nodes: {err}");
    }
    match report_rows(&facade) {
        Ok(ids) => {
            for id in ids {
                if let Err(err) = report(factory, &facade, &id) {
                    tracing::error!("MX delivery {id}: its report was not queued, tried again next round: {err}");
                }
            }
        }
        Err(err) => tracing::error!("MX delivery could not read its reports: {err}"),
    }
    let due = match due_rows(&facade, now) {
        Ok(due) => due,
        Err(err) => {
            tracing::error!("MX delivery could not read its rows: {err}");
            return Some(now + millis(IDLE));
        }
    };
    let mut held_back = false;
    for row in due {
        let id = field_text(&row, "mailDeliveryId");
        let item = field_text(&row, "queueItemId");
        {
            let mut flight = lock(flight());
            if flight.rows.contains(&id) || flight.items.contains(&item) || flight.rows.len() >= MAX_IN_FLIGHT {
                held_back = true;
                continue;
            }
            flight.rows.insert(id.clone());
            flight.items.insert(item.clone());
        }
        let factory = Arc::clone(factory);
        let facade = facade.clone();
        let spawned = std::thread::Builder::new()
            .name("moqui-mx-session".to_string())
            .spawn(move || {
                let context = factory.new_context();
                let _entered = context.enter();
                let _authz = context.disable_authz();
                let tried = std::panic::catch_unwind(AssertUnwindSafe(|| try_row(&factory, &facade, &row)));
                match tried {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => tracing::error!("MX delivery {}: {err}", field_text(&row, "mailDeliveryId")),
                    Err(payload) => tracing::error!(
                        "MX delivery {} failed: {}",
                        field_text(&row, "mailDeliveryId"),
                        moqui_core::service::pool::panic_text(payload.as_ref())
                    ),
                }
                let mut flight = lock(flight());
                flight.rows.remove(&field_text(&row, "mailDeliveryId"));
                flight.items.remove(&field_text(&row, "queueItemId"));
                drop(flight);
                wake();
            });
        if let Err(err) = spawned {
            tracing::error!("MX delivery {id}: no thread for it: {err}");
            let mut flight = lock(flight());
            flight.rows.remove(&id);
            flight.items.remove(&item);
        }
    }
    let next = next_due(&facade).unwrap_or_else(|err| {
        tracing::error!("MX delivery could not read its next row: {err}");
        Some(now_millis() + millis(IDLE))
    });
    // rows held back wait for a session to end (which wakes the thread), not a busy loop
    if held_back {
        Some(next.unwrap_or(now).max(now_millis() + 1000))
    } else {
        next
    }
}

fn due_rows(facade: &EntityFacade, now: i64) -> Result<Vec<EntityValue>, LibError> {
    let rows = facade
        .find(DELIVERY)?
        .eq("statusEnumId", Value::string(PENDING))
        .condition(Condition::less_equal("nextAttemptDate", stamp_value(now)))
        .order_by("nextAttemptDate", false)
        .limit(ROUND_ROWS as i64)
        .disable_authz()
        .list()?;
    Ok(rows.values().to_vec())
}

/// The rows whose report is not queued yet.
fn report_rows(facade: &EntityFacade) -> Result<Vec<String>, LibError> {
    let rows = facade
        .find(DELIVERY)?
        .eq("reportPending", Value::string("Y"))
        .select_field("mailDeliveryId")
        .limit(ROUND_ROWS as i64)
        .disable_authz()
        .list()?;
    Ok(rows
        .values()
        .iter()
        .map(|row| field_text(row, "mailDeliveryId"))
        .collect())
}

/// Stop delivering for local nodes that are no longer paired (revoked, or gone): their rows fail, logged, with no
/// report (there is no node to tell), and their messages are removed. Only the pending rows of such nodes are read, at
/// most [`UNPAIRED_ROWS`] a round: the paired nodes are few, so they go in the condition, and a backlog of a paired
/// node's rows costs no row read a round (plans/High-Level-Review.md HL-039). The rows read, for the tests.
fn fail_unpaired(factory: &ExecutionContextFactory, facade: &EntityFacade) -> Result<usize, LibError> {
    let active: Vec<Value> = facade
        .find("moqui.edge.EdgeClient")?
        .eq("statusId", Value::string("EdgeClientActive"))
        .select_field("pairedNodeId")
        .disable_authz()
        .list()?
        .values()
        .iter()
        .map(|row| field_text(row, "pairedNodeId"))
        .filter(|node| !node.is_empty())
        .collect::<BTreeSet<String>>()
        .into_iter()
        .map(Value::string)
        .collect();
    let mut find = facade.find(DELIVERY)?.eq("statusEnumId", Value::string(PENDING));
    if !active.is_empty() {
        find = find.condition(Condition::or(vec![
            Condition::is_null("pairedNodeId"),
            Condition::not_in("pairedNodeId", active),
        ]));
    }
    let pending = find
        .select_field("mailDeliveryId")
        .select_field("pairedNodeId")
        .select_field("contentLocation")
        .limit(UNPAIRED_ROWS)
        .disable_authz()
        .list()?;
    for row in pending.values() {
        let node = field_text(row, "pairedNodeId");
        let id = field_text(row, "mailDeliveryId");
        tracing::error!("MX delivery {id} failed: its local node {node} is not paired now; not delivered");
        let failed = transaction(facade, || {
            facade
                .find(DELIVERY)?
                .eq("mailDeliveryId", Value::string(&id))
                .eq("statusEnumId", Value::string(PENDING))
                .disable_authz()
                .update_all([
                    ("statusEnumId".to_string(), Value::string(BOUNCED)),
                    (
                        "lastReplyText".to_string(),
                        Value::string("the local node is not paired"),
                    ),
                    ("finishedDate".to_string(), stamp_value(now_millis())),
                    ("nextAttemptDate".to_string(), Value::Null),
                    ("reportPending".to_string(), Value::string("N")),
                ])
        });
        if let Err(err) = failed {
            tracing::error!("MX delivery {id}: not failed: {err}");
            continue;
        }
        let location = field_text(row, "contentLocation");
        if !location.is_empty() {
            if let Err(err) = factory.resource().delete(&location) {
                tracing::warn!("MX delivery {id}: its message {location} was not removed: {err}");
            }
        }
    }
    Ok(pending.values().len())
}

fn next_due(facade: &EntityFacade) -> Result<Option<i64>, LibError> {
    let row = facade
        .find(DELIVERY)?
        .eq("statusEnumId", Value::string(PENDING))
        .select_field("nextAttemptDate")
        .order_by("nextAttemptDate", false)
        .limit(1)
        .disable_authz()
        .list()?;
    Ok(row
        .values()
        .first()
        .and_then(|row| row.get("nextAttemptDate").and_then(value_millis)))
}

fn transaction<T>(facade: &EntityFacade, body: impl FnOnce() -> Result<T, LibError>) -> Result<T, LibError> {
    facade
        .transactions_for(DELIVERY)?
        .run(TransactionAttribute::ForceNew, None, body)
}

/// Try one row now: held by a lease first, so a crash mid-session leaves it to be tried later and nothing else takes it.
/// The session ends by [`SESSION_DEADLINE`], within the lease.
fn try_row(factory: &ExecutionContextFactory, facade: &EntityFacade, row: &EntityValue) -> Result<(), LibError> {
    let id = field_text(row, "mailDeliveryId");
    let attempts = u32::try_from(number(row, "attemptCount")).unwrap_or(0);
    let now = now_millis();
    let held = transaction(facade, || {
        facade
            .find(DELIVERY)?
            .eq("mailDeliveryId", Value::string(&id))
            .eq("statusEnumId", Value::string(PENDING))
            .eq("attemptCount", Value::I64(i64::from(attempts)))
            .condition(Condition::less_equal("nextAttemptDate", stamp_value(now)))
            .disable_authz()
            .update_all([
                ("nextAttemptDate".to_string(), stamp_value(now + millis(LEASE))),
                ("lastAttemptDate".to_string(), stamp_value(now)),
            ])
    })?;
    if held != 1 {
        return Ok(());
    }
    let domain = field_text(row, "domain");
    let recipients: Vec<String> = serde_json::from_str(&field_text(row, "recipientsJson")).unwrap_or_default();
    let location = field_text(row, "contentLocation");
    let Some(raw) = factory.resource().location_bytes(&location)? else {
        return finish(
            factory,
            facade,
            row,
            attempts,
            &recipients
                .iter()
                .map(|address| {
                    (
                        address.clone(),
                        Answer::Refused(format!("the message to deliver, {location}, is gone")),
                    )
                })
                .collect::<Vec<_>>(),
        );
    };
    let deadline = Instant::now() + SESSION_DEADLINE;
    let answers = deliver_domain(&domain, &field_text(row, "mailFrom"), &recipients, &raw, deadline);
    finish(factory, facade, row, attempts, &answers)
}

fn number(row: &EntityValue, field: &str) -> i64 {
    match row.get(field) {
        Some(Value::I64(number)) => *number,
        Some(value) => moqui_lib::object::to_plain_string(value).parse().unwrap_or(0),
        None => 0,
    }
}

/// A recipient's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Sent(String),
    /// Refused for good.
    Refused(String),
    /// Not now: try again.
    Later(String),
}

/// Text at most `max` characters long, cut with a note.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push_str(" ... (cut)");
    out
}

/// Record the answers of a try of `row` (its `attempts` tries before) in its own transaction first: the sent and the
/// refused counted, the waiting left to wait (or, past `mail_delivery_days`, refused), a row with none left done. Then
/// its report is queued ([`report`]); a report that fails stays marked and is queued at the next round, and the
/// delivery is never run again for it.
fn finish(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    row: &EntityValue,
    attempts: u32,
    answers: &[(String, Answer)],
) -> Result<(), LibError> {
    let id = field_text(row, "mailDeliveryId");
    let item = field_text(row, "queueItemId");
    let domain = field_text(row, "domain");
    let now = now_millis();
    let attempt = attempts + 1;
    let created = row.get("createdDate").and_then(value_millis).unwrap_or(now);
    let give_up = now - created >= millis(give_up_after());
    let mut refused: Vec<String> = Vec::new();
    let mut waiting: Vec<String> = Vec::new();
    let mut sent: Vec<String> = Vec::new();
    let mut last = String::new();
    for (address, answer) in answers {
        match answer {
            Answer::Sent(reply) => {
                sent.push(address.clone());
                last.clone_from(reply);
            }
            Answer::Refused(reply) => {
                refused.push(format!("{address}: {}", clip(reply, 500)));
                last.clone_from(reply);
            }
            Answer::Later(reply) if give_up => {
                refused.push(format!(
                    "{address}: not delivered in {} days, last: {}",
                    give_up_after().as_secs() / 86_400,
                    clip(reply, 500)
                ));
                last.clone_from(reply);
            }
            Answer::Later(reply) => {
                waiting.push(address.clone());
                last.clone_from(reply);
            }
        }
    }
    let wait = retry_after(attempt);
    tracing::info!(
        "MX delivery {id} (item {item}) to {domain}, try {attempt}: {} sent, {} refused, {} waiting{}; {last}",
        sent.len(),
        refused.len(),
        waiting.len(),
        if waiting.is_empty() {
            String::new()
        } else {
            format!(" (again in {wait:?})")
        }
    );
    let done = waiting.is_empty();
    let bounced = number(row, "bouncedCount") + i64::try_from(refused.len()).unwrap_or(0);
    let sent_count = number(row, "sentCount") + i64::try_from(sent.len()).unwrap_or(0);
    let mut refused_text = field_text(row, "refusedText");
    for line in &refused {
        if !refused_text.is_empty() {
            refused_text.push_str(" | ");
        }
        refused_text.push_str(line);
    }
    let refused_text = clip(&refused_text, MAX_REPORT);
    transaction(facade, || {
        let mut fields: Vec<(String, Value)> = vec![
            ("attemptCount".to_string(), Value::I64(i64::from(attempt))),
            ("bouncedCount".to_string(), Value::I64(bounced)),
            ("sentCount".to_string(), Value::I64(sent_count)),
            ("refusedText".to_string(), Value::string(&refused_text)),
            ("lastReplyText".to_string(), Value::string(clip(&last, 4000))),
        ];
        if done || !refused.is_empty() {
            fields.push(("reportPending".to_string(), Value::string("Y")));
        }
        if done {
            let status = if bounced == 0 { SENT } else { BOUNCED };
            fields.push(("statusEnumId".to_string(), Value::string(status)));
            fields.push(("finishedDate".to_string(), stamp_value(now)));
            fields.push(("nextAttemptDate".to_string(), Value::Null));
        } else {
            fields.push((
                "recipientsJson".to_string(),
                Value::string(serde_json::json!(waiting).to_string()),
            ));
            fields.push(("nextAttemptDate".to_string(), stamp_value(now + millis(wait))));
        }
        facade
            .find(DELIVERY)?
            .eq("mailDeliveryId", Value::string(&id))
            .disable_authz()
            .update_all(fields)?;
        Ok(())
    })?;
    if done || !refused.is_empty() {
        if let Err(err) = report(factory, facade, &id) {
            tracing::error!("MX delivery {id}: its report was not queued, tried again next round: {err}");
        }
    }
    Ok(())
}

/// The `mailStatus` for the local node's `mailOut` item `item`, once per `key`.
fn status_item(item: &str, key: &str, status: &str, reply: &str) -> NewItem {
    NewItem {
        kind: super::edge_mail::KIND_STATUS.to_string(),
        conversation_key: format!("mailStatus:{item}"),
        idempotency_key: format!("mx:{item}:{key}"),
        payload: Some(
            serde_json::json!({"queueItemId": item, "status": status, "reply": clip(reply, MAX_REPORT)}).to_string(),
        ),
        content: None,
        content_location: None,
    }
}

/// Queue the report of row `id` (marked `reportPending`), in a transaction of its own, and unmark it. When no row of
/// its message waits any more, the report is the message's last: "accepted" when every recipient got it, "partial"
/// when some did and some were refused (the message stays sent, with the refused named in its send error), "bounced"
/// when none did (back to the Outbox); its message is removed. Before that, a row with refused recipients reports them
/// as "partial".
///
/// # Errors
///
/// Returns why when the report was not queued; the row stays marked.
fn report(factory: &ExecutionContextFactory, facade: &EntityFacade, id: &str) -> Result<(), LibError> {
    transaction(facade, || {
        let Some(row) = facade
            .find(DELIVERY)?
            .eq("mailDeliveryId", Value::string(id))
            .eq("reportPending", Value::string("Y"))
            .disable_authz()
            .one()?
        else {
            return Ok(());
        };
        let item = field_text(&row, "queueItemId");
        let node = field_text(&row, "pairedNodeId");
        let rows = facade
            .find(DELIVERY)?
            .eq("pairedNodeId", Value::string(&node))
            .eq("queueItemId", Value::string(&item))
            .disable_authz()
            .list()?;
        let rows = rows.values();
        let waiting = rows.iter().any(|row| field_text(row, "statusEnumId") == PENDING);
        if waiting {
            let refused = field_text(&row, "refusedText");
            if !refused.is_empty() {
                let key = format!("{id}:{}", number(&row, "attemptCount"));
                queue::enqueue(
                    factory,
                    facade,
                    Some(&node),
                    &status_item(&item, &key, "partial", &refused),
                )?;
            }
        } else {
            let sent: i64 = rows.iter().map(|row| number(row, "sentCount")).sum();
            let refused: Vec<String> = rows
                .iter()
                .map(|row| field_text(row, "refusedText"))
                .filter(|text| !text.is_empty())
                .collect();
            let mut domains: Vec<String> = rows
                .iter()
                .filter(|row| number(row, "sentCount") > 0)
                .map(|row| field_text(row, "domain"))
                .collect();
            domains.sort_unstable();
            let (status, reply) = match (sent, refused.is_empty()) {
                (0, _) => ("bounced", refused.join(" | ")),
                (_, true) => ("accepted", format!("delivered: {}", domains.join(", "))),
                (_, false) => ("partial", refused.join(" | ")),
            };
            queue::enqueue(
                factory,
                facade,
                Some(&node),
                &status_item(&item, "final", status, &reply),
            )?;
            for location in rows.iter().map(|row| field_text(row, "contentLocation")) {
                if !location.is_empty() && factory.resource().delete(&location).is_err() {
                    tracing::warn!("MX delivery of item {item}: its message {location} was not removed");
                }
            }
            facade
                .find(DELIVERY)?
                .eq("pairedNodeId", Value::string(&node))
                .eq("queueItemId", Value::string(&item))
                .disable_authz()
                .update_all([("reportPending".to_string(), Value::string("N"))])?;
        }
        facade
            .find(DELIVERY)?
            .eq("mailDeliveryId", Value::string(id))
            .disable_authz()
            .update_all([("reportPending".to_string(), Value::string("N"))])?;
        Ok(())
    })
}

/// Remove the rows finished more than the queue's retention period ago.
fn clean(factory: &ExecutionContextFactory) {
    let Some(facade) = factory.entity() else {
        return;
    };
    let days = i64::from(moqui_core::edge::settings(factory).retention_days.max(1));
    let before = now_millis() - days * 86_400_000;
    let removed = transaction(&facade, || {
        facade
            .find(DELIVERY)?
            .condition(Condition::not_equals("statusEnumId", Value::string(PENDING)))
            .condition(Condition::less("finishedDate", stamp_value(before)))
            .disable_authz()
            .delete_all()
    });
    match removed {
        Ok(0) => {}
        Ok(count) => tracing::info!("MX delivery: {count} finished rows past the retention period removed"),
        Err(err) => tracing::error!("MX delivery: the finished rows were not removed: {err}"),
    }
}

// ---- one domain ----

/// `mail_mx_override`: where mail to `domain` goes instead of its MX (`domain=host:port`, `*` for any), testing and
/// staging only.
fn override_for(domain: &str) -> Option<(String, u16)> {
    let setting = moqui_lib::env::get_prop_or_env("mail_mx_override");
    let mut any = None;
    for entry in setting.split(',') {
        let Some((name, target)) = entry.trim().split_once('=') else {
            continue;
        };
        let Some((host, port)) = target.trim().rsplit_once(':') else {
            continue;
        };
        let Ok(port) = port.trim().parse::<u16>() else {
            continue;
        };
        let target = (host.trim().trim_matches(['[', ']']).to_string(), port);
        let name = name.trim().to_ascii_lowercase();
        if name == domain {
            return Some(target);
        }
        if name == "*" {
            any = Some(target);
        }
    }
    any
}

/// The hosts to try for `domain`, by preference: (host name, its addresses, port).
type Hosts = Vec<(String, Vec<IpAddr>, u16)>;

/// The MX hosts of `domain` and their addresses, or the answer every recipient gets when there is none to try.
fn hosts(domain: &str, policy: Option<&sts::Policy>) -> Result<Hosts, Answer> {
    if let Some((host, port)) = override_for(domain) {
        let ips = match host.parse::<IpAddr>() {
            Ok(ip) => vec![ip],
            Err(_) => super::dns::ips(&host).map_err(Answer::Later)?,
        };
        return Ok(vec![(host, ips, port)]);
    }
    let mut mx = super::dns::mx(domain).map_err(Answer::Later)?;
    if let [(_, host)] = mx.as_slice() {
        if host.is_empty() || host == "." {
            // a null MX (RFC 7505)
            return Err(Answer::Refused(format!(
                "556 5.1.10 {domain} accepts no mail (null MX)"
            )));
        }
    }
    if mx.is_empty() {
        // no MX: the domain's own address is its implicit MX (RFC 5321 section 5.1)
        mx = vec![(0, domain.to_string())];
    }
    if let Some(policy) = policy.filter(|policy| policy.enforce) {
        mx.retain(|(_, host)| policy.allows(host));
        if mx.is_empty() {
            return Err(Answer::Later(format!(
                "no MX host of {domain} is in its MTA-STS policy (enforce)"
            )));
        }
    }
    let mut hosts = Vec::new();
    let mut failed = Vec::new();
    for (_, host) in mx {
        match super::dns::ips(&host) {
            Ok(ips) if !ips.is_empty() => hosts.push((host, ips, SMTP_PORT)),
            Ok(_) => failed.push(format!("{host} has no address")),
            Err(err) => failed.push(err),
        }
    }
    if hosts.is_empty() {
        let text = failed.join("; ");
        // a domain whose only host does not exist is refused; a lookup that failed is tried again
        return Err(if failed.iter().all(|reason| reason.ends_with("has no address")) {
            Answer::Refused(format!("550 5.1.2 {domain} has no mail server: {text}"))
        } else {
            Answer::Later(text)
        });
    }
    Ok(hosts)
}

/// Deliver `raw` from `mail_from` to `recipients` of `domain` by `deadline`: each recipient's answer.
#[must_use]
pub fn deliver_domain(
    domain: &str,
    mail_from: &str,
    recipients: &[String],
    raw: &[u8],
    deadline: Instant,
) -> Vec<(String, Answer)> {
    let all = |answer: Answer| -> Vec<(String, Answer)> {
        recipients
            .iter()
            .map(|address| (address.clone(), answer.clone()))
            .collect()
    };
    let policy = if override_for(domain).is_some() {
        None
    } else {
        sts::policy(domain)
    };
    let hosts = match hosts(domain, policy.as_ref()) {
        Ok(hosts) => hosts,
        Err(answer) => return all(answer),
    };
    let secure = policy.as_ref().is_some_and(|policy| policy.enforce);
    let mut reasons: Vec<String> = Vec::new();
    for (host, ips, port) in &hosts {
        if let Err(err) = moqui_core::entity::fence::check_effect(moqui_core::entity::fence::Effect::Mail, host) {
            return all(Answer::Later(err.to_string()));
        }
        for ip in ips {
            if Instant::now() >= deadline {
                reasons.push("the try's time ran out".to_string());
                break;
            }
            let target = Target {
                host,
                address: SocketAddr::new(*ip, *port),
                secure,
                deadline,
            };
            match session(&target, mail_from, recipients, raw) {
                Ok(answers) => return answers,
                Err(reason) => reasons.push(format!("{host} [{ip}]: {reason}")),
            }
        }
    }
    all(Answer::Later(format!(
        "no MX of {domain} took the message: {}",
        reasons.join("; ")
    )))
}

/// The text of an SMTP error: the server's reply when there is one.
fn reply_text(err: &lettre::transport::smtp::Error) -> String {
    err.to_string()
}

/// Where one session goes: the MX host's name (for TLS), its address, whether MTA-STS requires a valid certificate,
/// and when the try must end.
struct Target<'a> {
    host: &'a str,
    address: SocketAddr,
    secure: bool,
    deadline: Instant,
}

impl Target<'_> {
    /// The time one step may take: a command's timeout, within what is left of the try.
    fn step(&self) -> Result<Duration, String> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("the try's time ran out".to_string());
        }
        Ok(left.min(SMTP_TIMEOUT))
    }
}

/// One SMTP session with `target`: each recipient's answer, or why the host gave none (not reached, the greeting or
/// MAIL refused for now, TLS that MTA-STS requires missing, the try's time out), so the next host is tried. Each
/// command waits at most a minute, and all of them together end by the target's deadline.
fn session(
    target: &Target<'_>,
    mail_from: &str,
    recipients: &[String],
    raw: &[u8],
) -> Result<Vec<(String, Answer)>, String> {
    let (host, address, secure) = (target.host, target.address, target.secure);
    let client = ClientId::Domain(hostname());
    let connect = || {
        let step = target.step()?;
        SmtpConnection::connect(address, Some(step), &client, None, None).map_err(|err| reply_text(&err))
    };
    let mut connection = connect()?;
    // each step within the try's deadline
    macro_rules! within {
        () => {
            match target.step() {
                Ok(step) => {
                    let _ = connection.set_timeout(Some(step));
                }
                Err(err) => {
                    connection.abort();
                    return Err(err);
                }
            }
        };
    }
    within!();
    if connection.can_starttls() {
        let parameters = if secure {
            TlsParameters::builder(host.to_string()).build_rustls()
        } else {
            // opportunistic: encrypted, whatever certificate the server shows
            TlsParameters::builder(host.to_string())
                .dangerous_accept_invalid_certs(true)
                .dangerous_accept_invalid_hostnames(true)
                .build_rustls()
        }
        .map_err(|err| format!("TLS parameters: {err}"))?;
        if let Err(err) = connection.starttls(&parameters, &client) {
            if secure {
                connection.abort();
                return Err(format!("STARTTLS failed, and MTA-STS requires it: {err}"));
            }
            tracing::warn!("MX {host} [{address}]: STARTTLS failed ({err}); sending without TLS");
            connection.abort();
            connection = connect()?;
        }
    } else if secure {
        connection.abort();
        return Err("no STARTTLS, and MTA-STS requires it".to_string());
    }
    let sender = match mail_from.trim() {
        "" => None,
        from => Some(
            from.parse::<lettre::Address>()
                .map_err(|err| format!("the sender {from}: {err}"))?,
        ),
    };
    within!();
    let mut parameters = Vec::new();
    if !raw.is_ascii() && connection.server_info().supports_feature(Extension::EightBitMime) {
        parameters.push(MailParameter::Body(MailBodyParameter::EightBitMime));
    }
    if let Err(err) = connection.command(Mail::new(sender, parameters)) {
        connection.abort();
        if err.is_permanent() {
            let reply = format!("MAIL FROM refused: {}", reply_text(&err));
            return Ok(recipients
                .iter()
                .map(|address| (address.clone(), Answer::Refused(reply.clone())))
                .collect());
        }
        return Err(format!("MAIL FROM: {}", reply_text(&err)));
    }
    let mut answers: HashMap<String, Answer> = HashMap::new();
    let mut accepted: Vec<&String> = Vec::new();
    for address in recipients {
        let parsed = match address.parse::<lettre::Address>() {
            Ok(parsed) => parsed,
            Err(err) => {
                answers.insert(address.clone(), Answer::Refused(format!("not an address: {err}")));
                continue;
            }
        };
        within!();
        match connection.command(Rcpt::new(parsed, Vec::new())) {
            Ok(_) => accepted.push(address),
            Err(err) if err.is_permanent() => {
                answers.insert(address.clone(), Answer::Refused(reply_text(&err)));
            }
            Err(err) => {
                answers.insert(address.clone(), Answer::Later(reply_text(&err)));
                if !err.is_transient() {
                    // the connection is gone
                    break;
                }
            }
        }
    }
    if !accepted.is_empty() {
        within!();
        let after_data = connection.command(Data).and_then(|_| connection.message(raw));
        let answer = match after_data {
            Ok(response) => Answer::Sent(format!(
                "{} {}",
                response.code(),
                response.message().collect::<Vec<_>>().join(" ")
            )),
            Err(err) if err.is_permanent() => Answer::Refused(reply_text(&err)),
            Err(err) => Answer::Later(reply_text(&err)),
        };
        for address in accepted {
            answers.insert(address.clone(), answer.clone());
        }
    }
    let _ = connection.command(Quit);
    Ok(recipients
        .iter()
        .map(|address| {
            let answer = answers
                .remove(address)
                .unwrap_or_else(|| Answer::Later(format!("{host} closed the session before RCPT TO")));
            (address.clone(), answer)
        })
        .collect())
}

/// MTA-STS (RFC 8461): the policy of a recipient domain, fetched over HTTPS and kept until its `max_age`.
pub mod sts {
    use super::{lock, Duration, HashMap, Instant, Mutex};

    /// A domain's policy.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Policy {
        /// The `_mta-sts` record's `id` it was fetched for.
        pub id: String,
        /// `mode: enforce`; `testing` and `none` change nothing here.
        pub enforce: bool,
        /// The `mx` patterns: `mail.example.com`, `*.example.net`.
        pub mx: Vec<String>,
        pub max_age: Duration,
    }

    impl Policy {
        /// Parse a policy file (RFC 8461 section 3.2). None when it is not version STSv1 or has no mode.
        #[must_use]
        pub fn parse(text: &str, id: &str) -> Option<Self> {
            let mut policy = Self {
                id: id.to_string(),
                ..Self::default()
            };
            let (mut version, mut mode) = (false, false);
            for line in text.lines() {
                let Some((key, value)) = line.split_once(':') else {
                    continue;
                };
                let value = value.trim();
                match key.trim().to_ascii_lowercase().as_str() {
                    "version" => version = value == "STSv1",
                    "mode" => {
                        mode = true;
                        policy.enforce = value.eq_ignore_ascii_case("enforce");
                    }
                    "mx" => policy.mx.push(value.trim_end_matches('.').to_ascii_lowercase()),
                    "max_age" => {
                        policy.max_age = Duration::from_secs(value.parse::<u64>().unwrap_or(0).min(31_557_600));
                    }
                    _ => {}
                }
            }
            (version && mode).then_some(policy)
        }

        /// Whether MX host `host` matches one of the policy's patterns (a `*.` pattern matches one label).
        #[must_use]
        pub fn allows(&self, host: &str) -> bool {
            let host = host.trim_end_matches('.').to_ascii_lowercase();
            self.mx.iter().any(|pattern| match pattern.strip_prefix("*.") {
                Some(parent) => host
                    .split_once('.')
                    .is_some_and(|(label, rest)| !label.is_empty() && rest == parent),
                None => *pattern == host,
            })
        }
    }

    /// The policies fetched, by domain, with when each was fetched.
    fn kept() -> &'static Mutex<HashMap<String, (Policy, Instant)>> {
        static KEPT: std::sync::OnceLock<Mutex<HashMap<String, (Policy, Instant)>>> = std::sync::OnceLock::new();
        KEPT.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// The `id` of `domain`'s `_mta-sts` TXT record (`v=STSv1; id=...`); none when it has none.
    fn record_id(domain: &str) -> Result<Option<String>, String> {
        let records = super::super::dns::txt(&format!("_mta-sts.{domain}"))?;
        Ok(records.iter().find_map(|record| {
            let mut fields = record.split(';').map(str::trim);
            (fields.next() == Some("v=STSv1")).then_some(())?;
            fields.find_map(|field| field.strip_prefix("id=").map(|id| id.trim().to_string()))
        }))
    }

    /// The policy file of `domain`.
    fn fetch(domain: &str) -> Result<String, String> {
        let url = format!("https://mta-sts.{domain}/.well-known/mta-sts.txt");
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|err| err.to_string())?;
        let response = client.get(&url).send().map_err(|err| format!("{url}: {err}"))?;
        if !response.status().is_success() {
            return Err(format!("{url}: {}", response.status()));
        }
        // at most 64 KiB read, whatever the server sends (RFC 8461 policies are a few lines)
        let mut body = Vec::new();
        std::io::Read::read_to_end(&mut std::io::Read::take(response, 64 * 1024), &mut body)
            .map_err(|err| format!("{url}: {err}"))?;
        Ok(String::from_utf8_lossy(&body).into_owned())
    }

    /// The MTA-STS policy of `domain`, or none. A policy kept and not expired is used while its record's id is the
    /// same, and also when the record or the fetch fails (RFC 8461 section 5.1).
    #[must_use]
    pub fn policy(domain: &str) -> Option<Policy> {
        let cached = lock(kept())
            .get(domain)
            .filter(|(policy, at)| at.elapsed() < policy.max_age)
            .map(|(policy, _)| policy.clone());
        let id = match record_id(domain) {
            Ok(Some(id)) => id,
            Ok(None) | Err(_) => return cached,
        };
        if let Some(policy) = cached.as_ref().filter(|policy| policy.id == id) {
            return Some(policy.clone());
        }
        match fetch(domain).map(|text| Policy::parse(&text, &id)) {
            Ok(Some(policy)) => {
                lock(kept()).insert(domain.to_string(), (policy.clone(), Instant::now()));
                Some(policy)
            }
            Ok(None) => {
                tracing::warn!("The MTA-STS policy of {domain} does not parse; not used");
                cached
            }
            Err(err) => {
                tracing::warn!("The MTA-STS policy of {domain} was not fetched: {err}");
                cached
            }
        }
    }
}

#[cfg(test)]
#[path = "mx_tests.rs"]
mod tests;
