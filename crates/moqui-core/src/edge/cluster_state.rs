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

//! What an edge node keeps in memory from the shared rows, made right on every member of an edge cluster
//! (plans/Edge-Node2.md §4.6). Each member keeps its own host table (the host names, the active pairings, their PASS
//! sites, chat bridges and XMPP domains) and builds its mail recipients and SIP trunks from the replicated copies when a
//! set's [`super::replica::applied_count`] moves. Those were built again only on the member that made the change, so
//! a host added, a pairing or a Configure on one member, or a batch applied there, did not reach the others until they
//! restarted.
//!
//! Now the member that changes them tells the others on the cluster topic [`crate::notify::EDGE_STATE`] ([`tell`],
//! after the change commits): `{"from", "hosts": bool, "sets": [..]}`. A member that hears it from another loads its
//! host table again (the host's callback, on a thread of its own, several notices in a row loading once) and counts each
//! set as applied, so what it builds from the set is built again at its next use ([`take`]). A member that may have
//! missed notices (the cluster's transport lost its connection, or a peer's gap, `crate::cluster::recover_missed`) does
//! both for everything ([`recovered`]). Nothing goes on a node with no cluster.
//!
//! The links a member holds are its own, so what ends or changes a pairing's links goes on the same topic
//! ([`tell_links`], `"links": [{"op", "node", "member", "keyChanged"}]`): a pairing revoked from the System app or
//! unpaired by its node, paired again, or a member's key forgotten. Each member then closes the links it holds, as the
//! member that took the change did ([`LinkOp`], the host's callback).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use moqui_lib::sync::lock;

use moqui_lib::error::Error;

use crate::context::ExecutionContextFactory;
use crate::entity::EntityFacade;

/// The most set names one notice carries.
const MAX_SETS: usize = 64;
/// The most link operations one notice carries.
const MAX_LINK_OPS: usize = 64;

/// What another member did to a pairing's links, for this member to do to the links it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkOp {
    /// The pairing was revoked (System › Edge Clients) or its node unpaired: its links close and it is refused.
    Revoked { node: String },
    /// The pairing was paired again: no longer refused here; with a new key its old links close.
    Paired { node: String, key_changed: bool },
    /// A member's key was forgotten: that member's link closes, and its next one is admitted afresh.
    MemberForgotten { node: String, member: String },
}

impl LinkOp {
    fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Revoked { node } => serde_json::json!({"op": "revoked", "node": node}),
            Self::Paired { node, key_changed } => {
                serde_json::json!({"op": "paired", "node": node, "keyChanged": key_changed})
            }
            Self::MemberForgotten { node, member } => {
                serde_json::json!({"op": "memberForgotten", "node": node, "member": member})
            }
        }
    }

    fn from_json(value: &serde_json::Value) -> Option<Self> {
        let text = |name: &str| {
            value
                .get(name)
                .and_then(serde_json::Value::as_str)
                .filter(|text| !text.is_empty() && text.len() <= 255)
                .map(str::to_string)
        };
        let node = text("node")?;
        match value.get("op").and_then(serde_json::Value::as_str)? {
            "revoked" => Some(Self::Revoked { node }),
            "paired" => Some(Self::Paired {
                node,
                key_changed: value
                    .get("keyChanged")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true),
            }),
            "memberForgotten" => Some(Self::MemberForgotten {
                node,
                member: text("member")?,
            }),
            _ => None,
        }
    }
}

/// The entity whose open transaction a notice waits for: a change of the host table or of a pairing commits there.
const HOST_ENTITY: &str = "moqui.edge.EdgeHost";

/// Tell the other members of this node's cluster that the host table (`hosts`) or the copies of `sets` changed here,
/// after the open transaction commits when there is one. Nothing on a node with no cluster; a failure is logged.
pub fn tell(facade: &EntityFacade, hosts: bool, sets: &[&str]) {
    if !hosts && sets.is_empty() {
        return;
    }
    send(facade, hosts, sets, &[]);
}

/// Tell the other members of this node's cluster what was done here to a pairing's links ([`LinkOp`]), so each does
/// the same to the links it holds; after the open transaction commits when there is one.
pub fn tell_links(facade: &EntityFacade, ops: &[LinkOp]) {
    if !ops.is_empty() {
        send(facade, false, &[], ops);
    }
}

fn send(facade: &EntityFacade, hosts: bool, sets: &[&str], ops: &[LinkOp]) {
    let Some(from) = crate::cluster::linked_instance_id(facade) else {
        return;
    };
    let links: Vec<serde_json::Value> = ops.iter().map(LinkOp::to_json).collect();
    let body = serde_json::json!({ "from": from, "hosts": hosts, "sets": sets, "links": links });
    let send = {
        let facade = facade.clone();
        move || {
            let message = crate::notify::TopicMessage {
                origin_id: crate::notify::origin_id().to_string(),
                message_id: String::new(),
                body,
            };
            if let Err(err) = crate::cluster::publish(&facade, crate::notify::EDGE_STATE, &message) {
                tracing::error!("Edge cluster: the other members were not told of a change here: {err}");
            }
        }
    };
    match facade.transactions_for(HOST_ENTITY) {
        Ok(tx) if tx.in_transaction() => {
            if let Err(err) = tx.on_commit(send) {
                tracing::error!("Edge cluster: the other members will not be told of a change here: {err}");
            }
        }
        _ => send(),
    }
}

/// What a notice asked of this member.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Taken {
    /// Load the host table again.
    pub hosts: bool,
    /// The sets counted as applied.
    pub sets: Vec<String>,
    /// What to do to the links this member holds.
    pub links: Vec<LinkOp>,
}

/// Take a notice another member sent ([`tell`]): each set it names counted as applied here, and whether the host
/// table is to be loaded again. A member's own notice, or one with no sender, asks nothing.
pub fn take(facade: &EntityFacade, body: &serde_json::Value) -> Taken {
    let from = body.get("from").and_then(serde_json::Value::as_str).unwrap_or_default();
    let own = crate::cluster::linked_instance_id(facade).unwrap_or_default();
    if from.is_empty() || from == own {
        return Taken::default();
    }
    let sets: Vec<String> = body
        .get("sets")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .filter(|set| !set.is_empty() && set.len() <= 255)
        .take(MAX_SETS)
        .map(str::to_string)
        .collect();
    for set in &sets {
        super::replica::note_applied(facade, set);
    }
    let links = body
        .get("links")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_LINK_OPS)
        .filter_map(LinkOp::from_json)
        .collect();
    Taken {
        hosts: body.get("hosts").and_then(serde_json::Value::as_bool).unwrap_or(false),
        sets,
        links,
    }
}

/// Loads the host table again on a thread of its own: a request while a load runs makes one more load after it, so a
/// burst of notices loads at most twice and the load always reads what came last.
struct Reloader {
    reload: Arc<dyn Fn() + Send + Sync>,
    pending: AtomicBool,
    running: AtomicBool,
}

impl Reloader {
    fn request(self: &Arc<Self>) {
        self.pending.store(true, Ordering::Release);
        if self.running.swap(true, Ordering::AcqRel) {
            return;
        }
        let me = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("moqui-edge-state".to_string())
            .spawn(move || loop {
                while me.pending.swap(false, Ordering::AcqRel) {
                    (me.reload)();
                }
                me.running.store(false, Ordering::Release);
                // a request that came between the last load and here, whose own call saw this thread running
                if !me.pending.load(Ordering::Acquire) || me.running.swap(true, Ordering::AcqRel) {
                    return;
                }
            });
        if let Err(err) = spawned {
            self.running.store(false, Ordering::Release);
            tracing::error!("Edge cluster: the host table was not loaded again after another member's change: {err}");
        }
    }
}

/// This factory's reloader, kept with it.
#[derive(Default)]
struct Installed(Mutex<Option<Arc<Reloader>>>);

/// What this member does to the links it holds for another member's [`LinkOp`]s, called on the thread that heard
/// them.
pub type LinkOps = Arc<dyn Fn(&[LinkOp]) + Send + Sync>;

/// On a node that takes links: listen for the other members' notices, `reload_hosts` loading this member's host
/// table again (run on a thread of its own, with no execution context), and `links` doing to its links what another
/// member did to its own. Kept with the factory; a second call replaces the host callback and keeps the first `links`
/// callback.
pub fn install(factory: &Arc<ExecutionContextFactory>, reload_hosts: Arc<dyn Fn() + Send + Sync>, links: LinkOps) {
    struct Listening {
        _subscription: crate::notify::Subscription,
    }
    let reloader = Arc::new(Reloader {
        reload: reload_hosts,
        pending: AtomicBool::new(false),
        running: AtomicBool::new(false),
    });
    let installed = factory.extension(|_| Installed::default());
    let first = lock(&installed.0).replace(reloader).is_none();
    if !first {
        return;
    }
    let weak: Weak<ExecutionContextFactory> = Arc::downgrade(factory);
    let listener: crate::notify::Listener = Arc::new(move |message| {
        let Some(factory) = weak.upgrade() else {
            return;
        };
        let Some(facade) = factory.entity() else {
            return;
        };
        let taken = take(&facade, &message.body);
        if !taken.links.is_empty() {
            links(&taken.links);
        }
        if taken.hosts {
            request_reload(&factory);
        }
    });
    match crate::notify::subscribe(crate::notify::EDGE_STATE, listener) {
        Ok(subscription) => {
            factory.extension(move |_| Listening {
                _subscription: subscription,
            });
        }
        Err(err) => tracing::error!(
            "Edge cluster: the other members' changes will not be heard: {}",
            err.message()
        ),
    }
}

fn request_reload(factory: &ExecutionContextFactory) {
    let installed = factory.extension(|_| Installed::default());
    let reloader = lock(&installed.0).clone();
    if let Some(reloader) = reloader {
        reloader.request();
    }
}

/// This member may have missed notices: every set counts as applied, and the host table is loaded again where this
/// node keeps one ([`install`]).
pub fn recovered(factory: &ExecutionContextFactory, facade: &EntityFacade) {
    super::replica::note_all_applied(facade);
    request_reload(factory);
}

/// The row of the local members each edge member holds links of (plans/Edge-Node2.md §4.6, §4.11): one per edge
/// member, so its heartbeat is one write.
const MEMBER_LINKS: &str = "moqui.edge.EdgeMemberLinks";
/// How often an edge member writes its row again while another member of its cluster is live.
pub const MEMBER_LINK_BEAT: std::time::Duration = std::time::Duration::from_secs(5);
/// A row is another edge member's while its heartbeat is this recent (the database's time).
pub const MEMBER_LINK_FRESH_MS: i64 = 20_000;
/// A row this old is pruned: its edge member is gone.
pub const MEMBER_LINK_PRUNE_MS: i64 = 24 * 60 * 60 * 1000;

/// The local members an edge member holds links of (or is admitting), by pairing.
pub type HeldLinks = std::collections::BTreeMap<String, std::collections::BTreeSet<String>>;

fn member_link_transaction<T>(facade: &EntityFacade, body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    let _authz = crate::context::ExecutionContext::with_current(crate::context::ExecutionContext::disable_authz);
    let tx = facade.transactions_for(MEMBER_LINKS)?;
    if tx.in_transaction() {
        return body();
    }
    tx.run(
        crate::transaction::TransactionAttribute::ForceNew,
        Some(std::time::Duration::from_secs(30)),
        body,
    )
}

/// Write edge member `edge`'s row (`moqui.edge.EdgeMemberLinks`): the links it holds or is admitting now, with a
/// heartbeat of the database's time. In the caller's transaction when there is one (admission writes it with the
/// pairing's row locked), else in one of its own.
///
/// # Errors
///
/// When the row cannot be written.
pub fn write_member_links(facade: &EntityFacade, edge: &str, held: &HeldLinks) -> Result<(), Error> {
    if edge.is_empty() {
        return Ok(());
    }
    let now = crate::cluster::db_now(facade);
    let links = serde_json::to_string(held).map_err(|err| Error::new(err.to_string()))?;
    member_link_transaction(facade, || {
        let mut row = facade.make_value(MEMBER_LINKS)?;
        row.set("edgeInstanceId", moqui_lib::object::Value::string(edge));
        row.set("linksJson", moqui_lib::object::Value::string(links));
        row.set("heartbeatDate", crate::user::stamp_value(now));
        row.create_or_update(facade)?;
        Ok(())
    })
}

/// Remove edge member `edge`'s row (it stops), and the rows of edge members gone for a day ([`MEMBER_LINK_PRUNE_MS`]).
///
/// # Errors
///
/// When the rows cannot be removed.
pub fn remove_member_links(facade: &EntityFacade, edge: Option<&str>) -> Result<(), Error> {
    let before = crate::cluster::db_now(facade).saturating_sub(MEMBER_LINK_PRUNE_MS);
    member_link_transaction(facade, || {
        if let Some(edge) = edge.filter(|edge| !edge.is_empty()) {
            facade
                .find(MEMBER_LINKS)?
                .eq("edgeInstanceId", moqui_lib::object::Value::string(edge))
                .disable_authz()
                .delete_all()?;
        }
        facade
            .find(MEMBER_LINKS)?
            .condition(crate::entity::Condition::less(
                "heartbeatDate",
                crate::user::stamp_value(before),
            ))
            .disable_authz()
            .delete_all()?;
        Ok(())
    })
}

/// What the other edge members' rows say of pairing `node`, for admission.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Elsewhere {
    /// The local members linked on another edge member (a fresh row), in id order.
    pub members: Vec<String>,
    /// A live member of this edge cluster with an `edge_link_url` has no fresh row: its links are not known (it has
    /// not written since another member came up), so a node with no pinned key is not admitted yet.
    pub unknown: bool,
}

/// The members of pairing `node` linked on edge members other than `edge`, by rows whose heartbeat is within
/// [`MEMBER_LINK_FRESH_MS`] of the database's time, and whether a live member of the edge cluster has none
/// (plans/Edge-Node2.md §4.11).
///
/// # Errors
///
/// When the rows cannot be read.
pub fn linked_elsewhere(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    edge: &str,
    node: &str,
) -> Result<Elsewhere, Error> {
    let since = crate::cluster::db_now(facade).saturating_sub(MEMBER_LINK_FRESH_MS);
    let fresh: Vec<(String, HeldLinks)> = member_link_transaction(facade, || {
        let rows = facade.find(MEMBER_LINKS)?.use_cache(false).disable_authz().list()?;
        Ok(rows
            .values()
            .iter()
            .filter_map(|row| {
                let id = row.get("edgeInstanceId").map(moqui_lib::object::to_plain_string)?;
                let beat = row.get("heartbeatDate").and_then(crate::user::value_millis)?;
                if id == edge || beat < since {
                    return None;
                }
                let json = row
                    .get("linksJson")
                    .map(moqui_lib::object::to_plain_string)
                    .unwrap_or_default();
                match serde_json::from_str::<HeldLinks>(&json) {
                    Ok(held) => Some((id, held)),
                    Err(err) => {
                        tracing::error!("Edge cluster: edge member {id}'s links do not read ({err}); left out");
                        None
                    }
                }
            })
            .collect())
    })?;
    let mut members: Vec<String> = fresh
        .iter()
        .filter_map(|(_, held)| held.get(node))
        .flatten()
        .cloned()
        .collect();
    members.sort();
    members.dedup();
    let unknown = crate::cluster::member_report(factory, facade)?.iter().any(|member| {
        member.live
            && member.instance_id != edge
            && !member.edge_link_url.is_empty()
            && !fresh.iter().any(|(id, _)| *id == member.instance_id)
    });
    Ok(Elsewhere { members, unknown })
}

#[cfg(test)]
#[path = "cluster_state_tests.rs"]
mod tests;
