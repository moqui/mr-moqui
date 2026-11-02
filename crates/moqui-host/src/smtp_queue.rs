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

//! The listener's edge mode (plans/Edge-Node.md §5.4, E5a): on an edge node (`node_role=edge`) the mail is not stored
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

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex, Weak};

use moqui_core::context::{Error as CoreError, ExecutionContextFactory};
use moqui_core::edge::{queue, replica};
use moqui_core::entity::EntityFacade;
use moqui_lib::sync::lock;
use moqui_mail::delivery::{Options, RecipientDsn, RecipientMap};
use moqui_mail::edge_mail::{self, RECIPIENT_SET};

use super::{AccountResult, Backend, DataAnswer, Message, RcptAnswer};

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
    /// The nodes paired and not revoked, read at each look-up: a revoked node's copies are kept (plans/Edge-Node.md
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
            if let Some(item) = edge_mail::bounce_item_id(&address, &edge_mail::bounce_domain()) {
                return Ok(match bounce_node(&facade, &item, &copies.active)? {
                    Some(_) => Lookup::Found {
                        node: String::new(),
                        account: format!("{BOUNCE_ACCOUNT}{item}"),
                    },
                    None => Lookup::Unknown,
                });
            }
            Ok(copies.find(&address, &delimiters))
        })
        .await;
        match found {
            Ok(Lookup::Found { account, .. }) => RcptAnswer::Local(account),
            Ok(Lookup::Unknown) => RcptAnswer::Unknown,
            Ok(Lookup::NotPaired) => {
                RcptAnswer::TempFail("the local node of the address is not paired now".to_string())
            }
            Ok(Lookup::NotLoaded) => {
                RcptAnswer::TempFail("the local node's accounts have not been replicated here yet".to_string())
            }
            Err(err) => RcptAnswer::TempFail(err.message().to_string()),
        }
    }

    async fn deliver(&self, message: Message) -> DataAnswer {
        let copies = Arc::clone(&self.copies);
        let delimiters = self.delimiters.clone();
        let trusted = self.trusted.clone();
        let queued = crate::worker::on_pool_without_authz(&self.factory, move |factory| {
            let facade = entity(factory)?;
            let mut accounts: Vec<(String, AccountResult)> = Vec::new();
            let mut summary = Vec::new();
            // the recipients by the local node that has their account now
            let mut by_node: BTreeMap<String, (Vec<RecipientDsn>, Vec<String>)> = BTreeMap::new();
            let mut bounces: Vec<(String, String)> = Vec::new();
            {
                let mut copies = lock(&copies);
                copies.refresh(&facade)?;
                for group in &message.recipients {
                    if let Some(item) = group.email_account_id.strip_prefix(BOUNCE_ACCOUNT) {
                        match bounce_node(&facade, item, &copies.active)? {
                            Some(node) => bounces.push((group.email_account_id.clone(), node)),
                            None => accounts.push((group.email_account_id.clone(), AccountResult::Gone)),
                        }
                        continue;
                    }
                    let node = group.addresses.iter().find_map(|address| match copies.find(address, &delimiters) {
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
            }
            // a bounce is the status of the message it is for, not mail for an account
            for (account, node) in bounces {
                let item = account.trim_start_matches(BOUNCE_ACCOUNT);
                let Some(report) = edge_mail::parse_dsn(&message.raw).filter(|report| !report.action.is_empty()) else {
                    tracing::info!("SMTP listener (edge mode): mail to the bounce address of item {item} is not a failure report; dropped");
                    summary.push(format!("not a failure report for item {item}, dropped"));
                    accounts.push((account, AccountResult::Stored));
                    continue;
                };
                match queue::enqueue(factory, &facade, Some(&node), &edge_mail::dsn_status_item(item, &report)) {
                    Ok(queued) => {
                        summary.push(format!("{} report for item {item} queued as {}", report.action, queued.queue_item_id));
                        accounts.push((account, AccountResult::Stored));
                    }
                    Err(err) => {
                        tracing::error!("SMTP listener (edge mode): the report for item {item} was not queued: {err}");
                        accounts.push((account, AccountResult::Failed));
                    }
                }
            }
            for (node, (recipients, mut ids)) in by_node {
                ids.sort_unstable();
                // ordered per recipient mailbox: a message to several goes under the first
                let conversation = format!("mail:{}", ids.first().map_or("", String::as_str));
                let item = edge_mail::inbound_item_trusting(
                    message.raw.clone(),
                    &message.envelope,
                    &recipients,
                    &conversation,
                    &trusted,
                );
                match queue::enqueue(factory, &facade, Some(&node), &item) {
                    Ok(queued) => {
                        summary.push(format!(
                            "queued for local node {node} as item {}{}",
                            queued.queue_item_id,
                            if queued.created { "" } else { " (queued before)" }
                        ));
                        accounts.extend(ids.into_iter().map(|id| (id, AccountResult::Stored)));
                    }
                    Err(err) => {
                        tracing::error!("SMTP listener (edge mode): a message for local node {node} was not queued: {err}");
                        summary.push(format!("not queued for local node {node}: {err}"));
                        accounts.extend(ids.into_iter().map(|id| (id, AccountResult::Failed)));
                    }
                }
            }
            Ok(DataAnswer::Stored {
                accounts,
                summary: summary.join("; "),
            })
        })
        .await;
        queued.unwrap_or_else(|err| DataAnswer::TempFail(err.message().to_string()))
    }
}

#[cfg(test)]
#[path = "smtp_queue_tests.rs"]
mod tests;
