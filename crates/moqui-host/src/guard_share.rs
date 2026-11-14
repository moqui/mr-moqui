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

//! Web protection's bans shared (plans/Edge-Node2.md §4.3), and its view for the System app.
//!
//! A ban a rule starts here, and a lift from System › Security › Web Protection, is told on the cluster topic
//! [`moqui_core::notify::WEB_BAN`] to the other members of this node's cluster, and **up** each edge link this node
//! dialed (`moqui_core::edge::signal`) to its edge node, where most are found first on the local node (a sign-in that
//! keeps failing) and best refused (before the link is spent). Nothing goes **down** a link: an edge node's own bans
//! may come from any pairing's visitors, and one pairing is not told of another's; it refuses the client itself before
//! forwarding. What this node takes ([`decide`]):
//! - **from another member** (no link stamp): applied as this member's own ban, once the sender is a live member of this
//!   cluster by its row (read off the publishing thread);
//! - **from a paired node** (stamped `linkNode` by the edge node's link), on a node that takes links: held for that
//!   node's host names only, at most a week and [`crate::guard`]'s cap per node, so one pairing cannot shut a client out
//!   of another's sites; its lift lifts only those;
//! - nothing else.
//!
//! Never a trusted proxy, nor a client a rule exempts. A ban taken is not told on: each member has its own links.

use std::net::IpAddr;
use std::sync::{Arc, Mutex, Weak};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::notify::{TopicMessage, WEB_BAN};

use crate::guard::{BanNotice, Guard};

/// The longest ban name taken from another node.
const MAX_NAME: usize = 100;

/// The factory's listener on [`WEB_BAN`], kept with it.
#[derive(Default)]
struct Listening(Mutex<Option<moqui_core::notify::Subscription>>);

/// Share `guard`'s bans with this node's cluster and links, take theirs, and show it in the System app.
pub(crate) fn install(factory: &Arc<ExecutionContextFactory>, guard: &Arc<Guard>) {
    let telling = Arc::downgrade(factory);
    guard.set_notifier(Arc::new(move |notice: &BanNotice| {
        if let Some(factory) = telling.upgrade() {
            tell(&factory, notice);
        }
    }));
    let (taking, held) = (Arc::downgrade(factory), Arc::downgrade(guard));
    let listener: moqui_core::notify::Listener = Arc::new(move |message: &TopicMessage| {
        let (Some(factory), Some(guard)) = (taking.upgrade(), held.upgrade()) else {
            return;
        };
        let own = member(&factory).unwrap_or_default();
        let takes_links = moqui_core::edge::settings(&factory).takes_links();
        match decide(&message.body, &own, takes_links) {
            Decision::Ignore => {}
            Decision::Take(told) => {
                apply(&guard, &told);
            }
            Decision::IfMember(told) => {
                // a member's row is read off the publishing thread, which may be a connection's task; a member's ban
                // is rare (one per ban started on a member)
                let spawned = std::thread::Builder::new()
                    .name("moqui-web-ban".to_string())
                    .spawn(move || {
                        let member = factory.entity().is_some_and(|facade| {
                            moqui_core::cluster::is_live_member(&factory, &facade, &told.from).unwrap_or(false)
                        });
                        if member {
                            apply(&guard, &told);
                        } else {
                            tracing::debug!(
                                "web protection: a shared ban from {}, not a member of this cluster, is not taken",
                                told.from
                            );
                        }
                    });
                if let Err(err) = spawned {
                    tracing::error!("web protection: a shared ban was not taken: {err}");
                }
            }
        }
    });
    match moqui_core::notify::subscribe(WEB_BAN, listener) {
        Ok(subscription) => {
            *moqui_lib::sync::lock(&factory.extension(|_| Listening::default()).0) = Some(subscription);
        }
        Err(err) => tracing::error!(
            "web protection: the shared bans' topic was not subscribed: {}",
            err.message()
        ),
    }
    moqui_core::web::protection_view::install(
        factory,
        Arc::new(View {
            guard: Arc::downgrade(guard),
        }),
    );
}

/// This member's id, as its messages name it.
fn member(factory: &ExecutionContextFactory) -> Option<String> {
    moqui_core::cluster::instance_id(factory)
}

fn text(body: &serde_json::Value, name: &str) -> String {
    body.get(name)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The message of `notice` from member `from`.
pub(crate) fn body(from: &str, notice: &BanNotice) -> serde_json::Value {
    serde_json::json!({
        "kind": if notice.lift { "lift" } else { "ban" },
        "address": notice.address.to_string(),
        "seconds": notice.seconds,
        "ban": notice.ban,
        "from": from,
    })
}

/// Tell the other members, and the edge node this node dialed, of a ban started or lifted here.
fn tell(factory: &ExecutionContextFactory, notice: &BanNotice) {
    let from = member(factory).unwrap_or_default();
    let message = TopicMessage {
        origin_id: moqui_core::notify::origin_id().to_string(),
        message_id: moqui_core::entity::uuid_v4().unwrap_or_default(),
        body: body(&from, notice),
    };
    if let Some(facade) = factory.entity() {
        if let Err(err) = moqui_core::cluster::publish(&facade, WEB_BAN, &message) {
            tracing::warn!("web protection: a ban was not told to the cluster: {}", err.message());
        }
    }
    // up only: the edge nodes this node is paired with (edge_node_id), never the pairings at the other end of the links
    // this node takes
    for edge in up_links(&moqui_lib::env::get_prop_or_env(moqui_core::edge::NODE_ID), &from) {
        if let Some(Err(err)) = moqui_core::edge::signal::send(&edge, WEB_BAN, &message) {
            tracing::debug!(
                "web protection: a ban was not told to edge node {edge}: {}",
                err.message()
            );
        }
    }
}

/// The edge nodes a ban goes up to: the ids `edge_node_id` names (comma separated), not this member's own.
fn up_links(edge_ids: &str, own: &str) -> Vec<String> {
    edge_ids
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty() && *id != own)
        .map(str::to_string)
        .collect()
}

/// A ban or lift as another node told it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Told {
    pub(crate) lift: bool,
    pub(crate) address: IpAddr,
    pub(crate) seconds: u64,
    /// What banned it, for the view.
    pub(crate) source: String,
    /// The paired node whose hosts it holds for; `None` for one of this node's own.
    pub(crate) node: Option<String>,
    /// The member that told it.
    pub(crate) from: String,
}

/// What to do with a message on [`WEB_BAN`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    /// This member's own, malformed, or not this node's to take.
    Ignore,
    /// A paired node's: take it now.
    Take(Told),
    /// Another member's: take it once the sender is a live member of this cluster.
    IfMember(Told),
}

/// What this node, member `own`, does with `body` (the module documentation says whose are taken): pure, so each
/// branch is tested.
pub(crate) fn decide(body: &serde_json::Value, own: &str, takes_links: bool) -> Decision {
    let from = text(body, "from");
    if from.is_empty() || from == own {
        return Decision::Ignore;
    }
    let lift = match text(body, "kind").as_str() {
        "ban" => false,
        "lift" => true,
        _ => return Decision::Ignore,
    };
    let Ok(address) = text(body, "address").parse::<IpAddr>() else {
        return Decision::Ignore;
    };
    let seconds = body.get("seconds").and_then(serde_json::Value::as_u64).unwrap_or(0);
    if !lift && seconds == 0 {
        return Decision::Ignore;
    }
    let mut ban = text(body, "ban");
    ban.truncate(ban.char_indices().nth(MAX_NAME).map_or(ban.len(), |(at, _)| at));
    match moqui_core::edge::signal::link_node(body) {
        // a paired node's, only where links are taken, and only for its hosts
        Some(node) if takes_links => Decision::Take(Told {
            lift,
            address,
            seconds,
            source: format!("node {node}: {ban}"),
            node: Some(node.to_string()),
            from,
        }),
        Some(_) => Decision::Ignore,
        None => Decision::IfMember(Told {
            lift,
            address,
            seconds,
            source: format!("member {from}: {ban}"),
            node: None,
            from,
        }),
    }
}

/// Apply what was told; true when it changed something here.
fn apply(guard: &Guard, told: &Told) -> bool {
    if told.lift {
        return guard.lift(told.address, told.node.as_deref(), false);
    }
    let applied = guard.apply_shared(told.address, told.seconds, &told.source, told.node.as_deref());
    if applied {
        tracing::info!(
            "web protection: {} is banned for {} seconds, told by {}",
            told.address,
            told.seconds,
            told.source
        );
    }
    applied
}

/// The guard as the System app sees it.
struct View {
    guard: Weak<Guard>,
}

impl moqui_core::web::protection_view::ProtectionView for View {
    fn report(&self) -> serde_json::Value {
        self.guard
            .upgrade()
            .map_or(serde_json::Value::Null, |guard| guard.report())
    }

    fn lift(&self, address: &str) -> Result<bool, String> {
        let address: IpAddr = address
            .trim()
            .parse()
            .map_err(|_| format!("{} is not an IP address", address.trim()))?;
        let guard = self.guard.upgrade().ok_or("this server's listeners have stopped")?;
        Ok(guard.lift(address, None, true))
    }
}

#[cfg(test)]
#[path = "guard_share_tests.rs"]
mod tests;
