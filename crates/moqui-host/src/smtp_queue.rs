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

//! The listener's edge mode (plans/archive/Edge-Node.md §5.4, E5a): on an edge node (`node_role=edge`) the mail is not stored
//! here, where no accounts are, but queued for the local node that has them (`moqui_mail::edge_mail`).
//!
//! `RCPT TO` is answered from the local nodes' replicated set `mailRecipients`, held in memory as a
//! [`RecipientMap`] per node and built again when a batch of the set is applied (`replica::applied_count`). So an
//! unknown address is refused (550) while the local node is away too; until a paired node's set has been sent whole
//! once, an address not found gets 451, so the gateway tries again rather than bounce mail for an account not copied
//! yet, and so does an address only a node no longer paired has. A bounce to the VERP address of a message this edge
//! node relayed (`moqui_mail::edge_mail::bounce_item_id`) is taken only for a `mailOut` item it received from a paired
//! node, and becomes a `mailStatus` for that node rather than mail.
//! After the data the message is queued, once per local node, with the recipient addresses (not the account ids) and
//! the message as the item's content; each recipient is answered stored once the item is committed, and 451 when the
//! queue refuses it.
//!
//! On an all-in-one node (`node_role=all-in-one`, plans/archive/Edge-Node.md §10, E12) the listener's backend is
//! [`SplitBackend`], which decides by owner: `RCPT TO` is looked up in the server's own accounts first, then in the
//! paired node's copies, whose account is tagged with its node (`node:<node>:<account>`) so the ids of two databases
//! never meet in one session. An address both sides have is refused with 451 and logged as an error naming it:
//! ownership is exclusive. After the data the groups are split by side: the server's own are delivered as on a normal
//! node, the paired node's queued for it as on an edge node and the VERP bounces become its `mailStatus`, so a message
//! with recipients on both sides is stored once on each side.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex, Weak};

use moqui_core::context::{Error as CoreError, ExecutionContextFactory};
use moqui_core::edge::claims::{claimed_message, ClaimKind};
use moqui_core::edge::{queue, replica};
use moqui_core::entity::EntityFacade;
use moqui_lib::sync::lock;
use moqui_mail::delivery::{Envelope, Options, RecipientDsn, RecipientMap, Recipients};
use moqui_mail::edge_mail::{self, RECIPIENT_SET};

use super::{AccountResult, AccountResults, Backend, DataAnswer, Message, RcptAnswer};

/// The most copies the recipients are built from; a set larger than this is a deployment this design does not serve.
const MAX_COPIES: usize = 1_000_000;

/// The recipients of each paired local node, from the edge node's copies.
#[derive(Default)]
struct Copies {
    /// The set's `applied_count` they were built at.
    built_at: Option<u64>,
    /// The nodes whose set has been sent whole at least once.
    loaded: HashSet<String>,
    nodes: Vec<(String, RecipientMap)>,
    /// The nodes paired and not revoked, read at each look-up: a revoked node's copies are kept (plans/archive/Edge-Node.md
    /// §5.2), but its mail is not taken.
    active: HashSet<String>,
}

/// Where mail to an address goes, as the edge node's copies say.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Lookup {
    /// A paired local node's account.
    Found { node: String, account: String },
    /// Only a node that is not paired now has it: 451, so the gateway keeps it while the node may pair again.
    NotPaired,
    /// Not found, and some paired node's set has not been sent whole yet: 451.
    NotLoaded,
    /// No paired node has it: 550.
    Unknown,
}

/// The recipients of each local node from the copies of its `mailRecipients` (`moqui_mail::delivery`).
fn build(values: &[replica::ReplicaValue]) -> Vec<(String, RecipientMap)> {
    moqui_mail::delivery::recipient_maps(values)
}

/// The local nodes paired with this edge node and not revoked.
fn active_nodes(facade: &EntityFacade) -> Result<HashSet<String>, CoreError> {
    let rows = facade
        .find("moqui.edge.EdgeClient")?
        .eq("statusId", moqui_lib::object::Value::string("EdgeClientActive"))
        .select_field("pairedNodeId")
        .disable_authz()
        .list()?;
    Ok(rows
        .values()
        .iter()
        .map(|row| {
            row.get("pairedNodeId")
                .map(moqui_lib::object::to_plain_string)
                .unwrap_or_default()
        })
        .collect())
}

impl Copies {
    /// Read the paired nodes, and build the recipients again when a batch of the set was applied since.
    fn refresh(&mut self, facade: &EntityFacade) -> Result<(), CoreError> {
        self.active = active_nodes(facade)?;
        let count = replica::applied_count(facade, RECIPIENT_SET);
        if self.built_at == Some(count) {
            return Ok(());
        }
        let values = replica::list_values(facade, None, RECIPIENT_SET, None, MAX_COPIES)?;
        let loaded: HashSet<String> = replica::cursors(facade, None)?
            .into_iter()
            .filter(|cursor| cursor.set_name == RECIPIENT_SET && cursor.snapshot_date.is_some())
            .map(|cursor| cursor.paired_node_id)
            .collect();
        let nodes = build(&values);
        tracing::info!(
            "SMTP listener (edge mode): the recipients of {} local node(s) built from {} copies, {} sent whole",
            nodes.len(),
            values.len(),
            loaded.len()
        );
        self.nodes = nodes;
        self.loaded = loaded;
        self.built_at = Some(count);
        Ok(())
    }

    /// Where mail to `address` goes: a paired node's account first.
    fn find(&self, address: &str, delimiters: &str) -> Lookup {
        let mut not_paired = false;
        for (node, map) in &self.nodes {
            if let Some(account) = map.find(address, delimiters) {
                if self.active.contains(node) {
                    return Lookup::Found {
                        node: node.clone(),
                        account,
                    };
                }
                not_paired = true;
            }
        }
        if not_paired {
            Lookup::NotPaired
        } else if self.active.iter().any(|node| !self.loaded.contains(node)) {
            Lookup::NotLoaded
        } else {
            Lookup::Unknown
        }
    }
}

/// The paired local node a bounce to the VERP address of its `mailOut` item `item` is for: the node this edge node
/// received the item from, while it is paired. None for an item this edge node did not receive (a guess or a forgery).
fn bounce_node(facade: &EntityFacade, item: &str, active: &HashSet<String>) -> Result<Option<String>, CoreError> {
    let rows = facade
        .find(queue::RECEIPT)?
        .eq("queueItemId", moqui_lib::object::Value::string(item))
        .eq("kind", moqui_lib::object::Value::string(edge_mail::KIND_OUT))
        .select_field("pairedNodeId")
        .disable_authz()
        .list()?;
    Ok(rows
        .values()
        .iter()
        .map(|row| {
            row.get("pairedNodeId")
                .map(moqui_lib::object::to_plain_string)
                .unwrap_or_default()
        })
        .find(|node| active.contains(node)))
}

/// The account of a bounce recipient in the session, for the item it is for: an account id never has this form.
const BOUNCE_ACCOUNT: &str = "bounce:";
/// The account of a paired node's recipient in an all-in-one node's session, `node:<node>:<account>`: an account id
/// never has this form, so a paired node's account and the server's of the same id are two groups.
const NODE_ACCOUNT: &str = "node:";

/// The session's account for paired node `node`'s account `account` ([`NODE_ACCOUNT`]).
fn node_account(node: &str, account: &str) -> String {
    format!("{NODE_ACCOUNT}{node}:{account}")
}

/// The account id of a session's account: without its [`NODE_ACCOUNT`] tag.
fn bare_account(key: &str) -> &str {
    key.strip_prefix(NODE_ACCOUNT)
        .and_then(|rest| rest.rsplit_once(':'))
        .map_or(key, |(_, account)| account)
}

/// Which side of an all-in-one node a session's account is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    /// The server's own account.
    Own,
    /// A paired node's account ([`NODE_ACCOUNT`]).
    Paired,
    /// A VERP bounce of a paired node's `mailOut` item ([`BOUNCE_ACCOUNT`]).
    Bounce,
}

fn side(key: &str) -> Side {
    if key.starts_with(BOUNCE_ACCOUNT) {
        Side::Bounce
    } else if key.starts_with(NODE_ACCOUNT) {
        Side::Paired
    } else {
        Side::Own
    }
}

/// The groups of a message split by side: the server's own, and those that go to the link (the paired node's and the
/// bounces).
fn split_sides(groups: Vec<Recipients>) -> (Vec<Recipients>, Vec<Recipients>) {
    groups
        .into_iter()
        .partition(|group| side(&group.email_account_id) == Side::Own)
}

/// The answer to `RCPT TO` of what the copies say.
fn lookup_answer(lookup: Lookup) -> RcptAnswer {
    match lookup {
        Lookup::Found { account, .. } => RcptAnswer::Local(account),
        Lookup::Unknown => RcptAnswer::Unknown,
        Lookup::NotPaired => RcptAnswer::TempFail("the local node of the address is not paired now".to_string()),
        Lookup::NotLoaded => {
            RcptAnswer::TempFail("the local node's accounts have not been replicated here yet".to_string())
        }
    }
}

/// How an address matched an account: as it is, or only without its plus tag (`bob+work@` as `bob@`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Level {
    Exact,
    Stripped,
}

/// The level of a match found looking `address` up both ways: `exact` finds it as it is (no delimiters), `any` also
/// without its plus tag. The exact is tried first, so the stripped look-up is only made when it finds nothing.
fn leveled<T>(
    exact: impl FnOnce() -> Result<Option<T>, CoreError>,
    any: impl FnOnce() -> Result<Option<T>, CoreError>,
) -> Result<Option<(T, Level)>, CoreError> {
    if let Some(found) = exact()? {
        return Ok(Some((found, Level::Exact)));
    }
    Ok(any()?.map(|found| (found, Level::Stripped)))
}

/// What the paired node's copies say of an address, with the level of a [`Lookup::Found`] (`Exact` for the others).
fn leveled_lookup(copies: &Copies, address: &str, delimiters: &str) -> (Lookup, Level) {
    match copies.find(address, "") {
        found @ Lookup::Found { .. } => (found, Level::Exact),
        _ => match copies.find(address, delimiters) {
            found @ Lookup::Found { .. } => (found, Level::Stripped),
            other => (other, Level::Exact),
        },
    }
}

/// The answer to `RCPT TO` on an all-in-one node, from the server's own account of `address` (`own`) and what the
/// paired node's copies say (`replica`), each with how it matched. When both sides have it, an exact match wins over
/// one only without the plus tag (the server's `bob@` and the paired node's `bob+work@`: `bob+work@` is the paired
/// node's, `bob+home@` the server's). Both at the same level is refused with 451 (an operator fixes it, and the sender
/// keeps the mail meanwhile), logged. Else the side that has it, the paired node's account tagged.
fn choose(address: &str, own: Option<(String, Level)>, replica: (Lookup, Level)) -> RcptAnswer {
    match (own, replica) {
        (Some((account, own_level)), (Lookup::Found { node, account: theirs }, replica_level)) => {
            if own_level == Level::Exact && replica_level == Level::Stripped {
                return RcptAnswer::Local(account);
            }
            if own_level == Level::Stripped && replica_level == Level::Exact {
                return RcptAnswer::Local(node_account(&node, &theirs));
            }
            let text = format!(
                "{}, and also by paired node {node}: an address belongs to one side",
                claimed_message(
                    ClaimKind::MailAddress,
                    address,
                    &format!("the server's account {account}")
                )
            );
            tracing::error!("SMTP listener (all-in-one): recipient refused, {text}");
            RcptAnswer::TempFail(text)
        }
        (Some((account, _)), _) => RcptAnswer::Local(account),
        (None, (Lookup::Found { node, account }, _)) => RcptAnswer::Local(node_account(&node, &account)),
        (None, (other, _)) => lookup_answer(other),
    }
}

/// Queue a message for the paired nodes of `groups` (their recipients and VERP bounces), as an edge node does, with
/// `copies` refreshed: each group's result, and the summary's parts. The item of each node covers only its own
/// recipients' addresses, ordered under the first of its accounts.
#[allow(clippy::too_many_arguments)]
fn deliver_queued(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    copies: &Copies,
    delimiters: &str,
    trusted: &Options,
    raw: &[u8],
    envelope: &Envelope,
    groups: &[Recipients],
) -> Result<(AccountResults, Vec<String>), CoreError> {
    let mut accounts: AccountResults = Vec::new();
    let mut summary = Vec::new();
    // the recipients by the local node that has their account now
    let mut by_node: BTreeMap<String, (Vec<RecipientDsn>, Vec<String>)> = BTreeMap::new();
    let mut bounces: Vec<(String, String)> = Vec::new();
    for group in groups {
        if let Some(item) = group.email_account_id.strip_prefix(BOUNCE_ACCOUNT) {
            match bounce_node(facade, item, &copies.active)? {
                Some(node) => bounces.push((group.email_account_id.clone(), node)),
                None => accounts.push((group.email_account_id.clone(), AccountResult::Gone)),
            }
            continue;
        }
        let node = group
            .addresses
            .iter()
            .find_map(|address| match copies.find(address, delimiters) {
                Lookup::Found { node, .. } => Some(node),
                _ => None,
            });
        let Some(node) = node else {
            accounts.push((group.email_account_id.clone(), AccountResult::Gone));
            continue;
        };
        let entry = by_node.entry(node).or_default();
        for address in &group.addresses {
            let dsn = group.dsn.iter().find(|dsn| dsn.address == *address);
            entry.0.push(RecipientDsn {
                address: address.clone(),
                notify: dsn.map(|dsn| dsn.notify.clone()).unwrap_or_default(),
                original: dsn.map(|dsn| dsn.original.clone()).unwrap_or_default(),
            });
        }
        entry.1.push(group.email_account_id.clone());
    }
    // a bounce is the status of the message it is for, not mail for an account
    for (account, node) in bounces {
        let item = account.trim_start_matches(BOUNCE_ACCOUNT);
        let Some(report) = edge_mail::parse_dsn(raw).filter(|report| !report.action.is_empty()) else {
            tracing::info!(
                "SMTP listener (edge mode): mail to the bounce address of item {item} is not a failure report; dropped"
            );
            summary.push(format!("not a failure report for item {item}, dropped"));
            accounts.push((account, AccountResult::Stored));
            continue;
        };
        match queue::enqueue(factory, facade, Some(&node), &edge_mail::dsn_status_item(item, &report)) {
            Ok(queued) => {
                summary.push(format!(
                    "{} report for item {item} queued as {}",
                    report.action, queued.queue_item_id
                ));
                accounts.push((account, AccountResult::Stored));
            }
            Err(err) => {
                tracing::error!("SMTP listener (edge mode): the report for item {item} was not queued: {err}");
                accounts.push((account, AccountResult::Failed));
            }
        }
    }
    for (node, (recipients, keys)) in by_node {
        // ordered per recipient mailbox, by its account id on the local node: a message to several goes under the first
        let mut ids: Vec<&str> = keys.iter().map(|key| bare_account(key)).collect();
        ids.sort_unstable();
        let conversation = format!("mail:{}", ids.first().copied().unwrap_or_default());
        let item = edge_mail::inbound_item_trusting(raw.to_vec(), envelope, &recipients, &conversation, trusted);
        match queue::enqueue(factory, facade, Some(&node), &item) {
            Ok(queued) => {
                summary.push(format!(
                    "queued for local node {node} as item {}{}",
                    queued.queue_item_id,
                    if queued.created { "" } else { " (queued before)" }
                ));
                accounts.extend(keys.into_iter().map(|key| (key, AccountResult::Stored)));
            }
            Err(err) => {
                tracing::error!("SMTP listener (edge mode): a message for local node {node} was not queued: {err}");
                summary.push(format!("not queued for local node {node}: {err}"));
                accounts.extend(keys.into_iter().map(|key| (key, AccountResult::Failed)));
            }
        }
    }
    Ok((accounts, summary))
}

/// The answer to a bounce recipient: the VERP address of a `mailOut` item this node received from a paired node now
/// paired. `None` when `address` is not a VERP address; `Some(None)` when it is one, but of no such item.
fn bounce_recipient(
    facade: &EntityFacade,
    copies: &Copies,
    address: &str,
) -> Result<Option<Option<String>>, CoreError> {
    let Some(item) = edge_mail::bounce_item_id(address, &edge_mail::bounce_domain()) else {
        return Ok(None);
    };
    Ok(Some(
        bounce_node(facade, &item, &copies.active)?.map(|_| format!("{BOUNCE_ACCOUNT}{item}")),
    ))
}

/// The listener's backend on an edge node: recipients from the replicated copies, messages to the link's queue.
pub(crate) struct QueueBackend {
    factory: Weak<ExecutionContextFactory>,
    delimiters: String,
    copies: Arc<Mutex<Copies>>,
    /// What this listener trusts (its authserv-id, its spam fields), which the local node trusts for its mail.
    trusted: Options,
}

impl QueueBackend {
    pub(crate) fn new(factory: Weak<ExecutionContextFactory>, delimiters: String, trusted: Options) -> Self {
        Self {
            factory,
            delimiters,
            copies: Arc::new(Mutex::new(Copies::default())),
            trusted,
        }
    }
}

fn entity(factory: &ExecutionContextFactory) -> Result<EntityFacade, CoreError> {
    factory
        .entity()
        .ok_or_else(|| CoreError::new("the entity facade is not ready"))
}

impl Backend for QueueBackend {
    async fn recipient(&self, address: String) -> RcptAnswer {
        let copies = Arc::clone(&self.copies);
        let delimiters = self.delimiters.clone();
        let found = crate::worker::on_pool_without_authz(&self.factory, move |factory| {
            let facade = entity(factory)?;
            let mut copies = lock(&copies);
            copies.refresh(&facade)?;
            // a bounce to the VERP sender of a message this edge node relayed for a paired node
            if let Some(bounce) = bounce_recipient(&facade, &copies, &address)? {
                return Ok(bounce.map_or(RcptAnswer::Unknown, RcptAnswer::Local));
            }
            Ok(lookup_answer(copies.find(&address, &delimiters)))
        })
        .await;
        found.unwrap_or_else(|err| RcptAnswer::TempFail(err.message().to_string()))
    }

    async fn deliver(&self, message: Message) -> DataAnswer {
        let copies = Arc::clone(&self.copies);
        let delimiters = self.delimiters.clone();
        let trusted = self.trusted.clone();
        let queued = crate::worker::on_pool_without_authz(&self.factory, move |factory| {
            let facade = entity(factory)?;
            let mut copies = lock(&copies);
            copies.refresh(&facade)?;
            let (accounts, summary) = deliver_queued(
                factory,
                &facade,
                &copies,
                &delimiters,
                &trusted,
                &message.raw,
                &message.envelope,
                &message.recipients,
            )?;
            Ok(DataAnswer::Stored {
                accounts,
                summary: summary.join("; "),
            })
        })
        .await;
        queued.unwrap_or_else(|err| DataAnswer::TempFail(err.message().to_string()))
    }
}

/// The listener's backend on an all-in-one node (plans/archive/Edge-Node.md §10): the server's own accounts as on a normal
/// node ([`super::FactoryBackend`]), a paired node's from its copies and queued for it as on an edge node
/// ([`QueueBackend`]).
pub(crate) struct SplitBackend {
    factory: Weak<ExecutionContextFactory>,
    delimiters: String,
    copies: Arc<Mutex<Copies>>,
    /// What this listener trusts (its authserv-id, its spam fields): for the server's own delivery, and for the
    /// paired node's mail.
    options: Options,
}

impl SplitBackend {
    pub(crate) fn new(factory: Weak<ExecutionContextFactory>, delimiters: String, options: Options) -> Self {
        Self {
            factory,
            delimiters,
            copies: Arc::new(Mutex::new(Copies::default())),
            options,
        }
    }
}

impl Backend for SplitBackend {
    async fn recipient(&self, address: String) -> RcptAnswer {
        let copies = Arc::clone(&self.copies);
        let delimiters = self.delimiters.clone();
        let found = crate::worker::on_pool_without_authz(&self.factory, move |factory| {
            let facade = entity(factory)?;
            let own = leveled(
                || moqui_mail::delivery::find_recipient(&facade, &address, "").map_err(CoreError::from),
                || moqui_mail::delivery::find_recipient(&facade, &address, &delimiters).map_err(CoreError::from),
            )?;
            let mut copies = lock(&copies);
            copies.refresh(&facade)?;
            // a bounce to the VERP sender of a message relayed for the paired node; a VERP address of no such item
            // is looked up as any other
            if let Some(Some(account)) = bounce_recipient(&facade, &copies, &address)? {
                return Ok(RcptAnswer::Local(account));
            }
            Ok(choose(&address, own, leveled_lookup(&copies, &address, &delimiters)))
        })
        .await;
        found.unwrap_or_else(|err| RcptAnswer::TempFail(err.message().to_string()))
    }

    async fn deliver(&self, message: Message) -> DataAnswer {
        let copies = Arc::clone(&self.copies);
        let delimiters = self.delimiters.clone();
        let options = self.options.clone();
        let delivered = crate::worker::on_pool_without_authz(&self.factory, move |factory| {
            let facade = entity(factory)?;
            let Message {
                raw,
                envelope,
                recipients,
            } = message;
            let (own, link) = split_sides(recipients);
            let mut accounts: AccountResults = Vec::new();
            let mut summary = Vec::new();
            // the server's own first: a message it refuses (not a message) is refused for the paired node too
            if !own.is_empty() {
                match super::deliver_own(&facade, &raw, &envelope, &own, &options) {
                    Ok((own_accounts, own_summary)) => {
                        accounts.extend(own_accounts);
                        summary.push(own_summary);
                    }
                    Err(text) => return Ok(DataAnswer::Rejected(text)),
                }
            }
            if !link.is_empty() {
                let queued = {
                    let mut copies = lock(&copies);
                    copies.refresh(&facade).and_then(|()| {
                        deliver_queued(factory, &facade, &copies, &delimiters, &options, &raw, &envelope, &link)
                    })
                };
                match queued {
                    Ok((link_accounts, link_summary)) => {
                        accounts.extend(link_accounts);
                        summary.extend(link_summary);
                    }
                    // the server's own stay stored; the paired node's get 451, and a retry finds the own had it
                    Err(err) => {
                        tracing::error!(
                            "SMTP listener (all-in-one): a message for the paired node was not queued: {err}"
                        );
                        summary.push(format!("not queued for the paired node: {err}"));
                        accounts.extend(
                            link.iter()
                                .map(|group| (group.email_account_id.clone(), AccountResult::Failed)),
                        );
                    }
                }
            }
            Ok(DataAnswer::Stored {
                accounts,
                summary: summary.join("; "),
            })
        })
        .await;
        delivered.unwrap_or_else(|err| DataAnswer::TempFail(err.message().to_string()))
    }
}

#[cfg(test)]
#[path = "smtp_queue_tests.rs"]
mod tests;
