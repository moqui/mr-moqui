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

//! The public host names an edge node answers for its paired local nodes (plans/archive/Edge-Node.md §5.3):
//! `moqui.edge.EdgeHost` rows, and the table in memory the listener reads for each connection and request. Only a
//! name in the table goes down a link, and only to its own node, so the edge node is no open proxy.
//!
//! Each member of an edge cluster keeps its own table (plans/Edge-Node2.md §4.6). A member that changes the rows loads
//! its table and tells the others ([`Hosts::changed`]), which load theirs on the notice
//! (`moqui_core::edge::cluster_state`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::edge::{EdgeHostInfo, HOST_PASSTHROUGH, HOST_TERMINATE};
use moqui_lib::error::Error;
use moqui_lib::sync::{read, write};

use super::store;

/// How the edge node handles a host's connections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// The TLS goes down the link unopened; the local node ends it.
    Passthrough,
    /// The edge node ends the TLS and forwards each request.
    Terminate,
}

impl Mode {
    fn from_enum(id: &str) -> Option<Self> {
        match id {
            HOST_PASSTHROUGH => Some(Self::Passthrough),
            HOST_TERMINATE => Some(Self::Terminate),
            _ => None,
        }
    }
}

/// A host's node and mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Route {
    pub node: String,
    pub mode: Mode,
    /// The cookie a terminate host's requests are sticky by (plans/Edge-Node2.md §4.4); `None` for the conf's.
    pub sticky_cookie: Option<std::sync::Arc<str>>,
}

/// The host names by their normal form ([`normalize`]) and what the active pairings' kept setups say, as one load
/// wrote them (plans/Edge-Node2.md §4.1).
#[derive(Default)]
pub(crate) struct Hosts {
    table: RwLock<Table>,
    loads: AtomicU64,
}

/// One load of [`Hosts`].
#[derive(Default)]
pub(crate) struct Table {
    /// The number of the load that wrote it.
    load: u64,
    routes: HashMap<String, Route>,
    /// The chat bridge ids each active pairing routes through this node, by node; empty when this node takes no
    /// webhooks (no `channels` feature, or `channels_webhooks_enabled` not true).
    chat: HashMap<String, Vec<String>>,
    /// The PASS sites (lower case) the active pairings' Configures name: the node, or `None` for a site two name.
    sites: HashMap<String, Option<String>>,
    /// The active pairings, in id order.
    active: Vec<String>,
    /// Each active pairing's XMPP domain (its Configure's `xmppDomain`, lower case), for the XMPP ports of every domain
    /// (`tunnel::xmpp_route`); empty for one that named none.
    xmpp: Vec<(String, String)>,
}

/// Whether this node answers chat webhooks itself, as `crate::channels` adds its routes.
fn takes_webhooks() -> bool {
    cfg!(feature = "channels") && moqui_lib::env::get_prop_or_env("channels_webhooks_enabled").trim() == "true"
}

/// What the active pairings' kept setups say, as a table with no routes: each node's chat bridges, the PASS sites by
/// node (a site two name is logged and kept as `None`), and the nodes.
fn setup_maps(setups: Vec<(String, serde_json::Value)>, webhooks: bool) -> Table {
    let names = moqui_core::edge::hosts::setup_list;
    let mut chat = HashMap::new();
    let mut sites: HashMap<String, Option<String>> = HashMap::new();
    let mut active = Vec::with_capacity(setups.len());
    let mut xmpp = Vec::with_capacity(setups.len());
    for (node, setup) in setups {
        let domain = setup["xmppDomain"]
            .as_str()
            .and_then(super::xmpp_domain::host_name)
            .unwrap_or_default();
        xmpp.push((node.clone(), domain));
        if webhooks {
            chat.insert(node.clone(), names(&setup, "chatBridgeIds"));
        }
        for site in names(&setup, "passSites") {
            let site = site.to_ascii_lowercase();
            match sites.get(&site) {
                Some(Some(other)) if *other != node => {
                    tracing::error!(
                        "Edge hosts: PASS site {site} is named by both {other} and {node}; its calls are refused until \
                         one Configures without it"
                    );
                    sites.insert(site, None);
                }
                Some(_) => {}
                None => {
                    sites.insert(site, Some(node.clone()));
                }
            }
        }
        active.push(node);
    }
    Table {
        chat,
        sites,
        active,
        xmpp,
        ..Table::default()
    }
}

/// The bridge id of a chat path this node answers itself on a pairing's terminate host: `/channels/{id}/webhook` (a
/// network's webhook, queued for the node) and `/channels/{id}/media/{token}` (a file the node put here).
fn chat_bridge_of(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/channels/")?;
    let (id, tail) = rest.split_once('/')?;
    (!id.is_empty() && (tail == "webhook" || tail.starts_with("media/"))).then_some(id)
}

impl Hosts {
    /// The route of `name` (a `Host` header or a TLS server name, normalized here).
    pub(crate) fn route(&self, name: &str) -> Option<Route> {
        let name = normalize(name)?;
        read(&self.table).routes.get(&name).cloned()
    }

    /// The pairing whose Configure names PASS site `site`: `Ok(None)` when none does, an error when two do.
    pub(crate) fn site_node(&self, site: &str) -> Result<Option<String>, Error> {
        match read(&self.table).sites.get(&site.trim().to_ascii_lowercase()) {
            None => Ok(None),
            Some(Some(node)) => Ok(Some(node.clone())),
            Some(None) => Err(Error::new(format!(
                "PASS site {} is named by more than one paired node, so its authority cannot be told",
                site.trim()
            ))),
        }
    }

    /// The active pairings, as the last load read them.
    pub(crate) fn active(&self) -> Vec<String> {
        read(&self.table).active.clone()
    }

    /// Each active pairing and its XMPP domain, as the last load read them (E16).
    pub(crate) fn xmpp_domains(&self) -> Vec<(String, String)> {
        read(&self.table).xmpp.clone()
    }

    /// Whether this node answers `path` itself on a terminate host of `node`, rather than sending it down the link: a
    /// chat bridge's webhook or file, for a bridge `node` routes through this node, so a network's webhook at the
    /// pairing's own name is queued while the node is away and goes by the bridge id within that name.
    pub(crate) fn answers_here(&self, node: &str, path: &str) -> bool {
        let Some(bridge) = chat_bridge_of(path) else {
            return false;
        };
        read(&self.table)
            .chat
            .get(node)
            .is_some_and(|bridges| bridges.iter().any(|id| id == bridge))
    }

    /// After a change of the rows or the pairings made here: load the table, and tell the other members of an edge
    /// cluster to load theirs (after the change commits), whether or not this load worked.
    pub(crate) fn changed(&self, factory: &ExecutionContextFactory) -> Result<usize, Error> {
        if let Ok(facade) = store::facade(factory) {
            moqui_core::edge::cluster_state::tell(&facade, true, &[]);
        }
        self.load(factory)
    }

    /// Replace the table with the rows, unless a load that began later has written it already.
    pub(crate) fn load(&self, factory: &ExecutionContextFactory) -> Result<usize, Error> {
        let load = self.loads.fetch_add(1, Ordering::AcqRel) + 1;
        let rows = list(factory, None)?;
        let mut table = HashMap::with_capacity(rows.len());
        for row in rows {
            let Some(name) = normalize(&row.host_name) else {
                tracing::error!("Edge host {} is not a host name; it is left out", row.host_name);
                continue;
            };
            match Mode::from_enum(&row.mode_enum_id) {
                Some(mode) => {
                    table.insert(
                        name,
                        Route {
                            node: row.paired_node_id,
                            mode,
                            sticky_cookie: row
                                .sticky_cookie
                                .filter(|cookie| !cookie.is_empty())
                                .map(|cookie| std::sync::Arc::from(cookie.as_str())),
                        },
                    );
                }
                None => tracing::error!(
                    "Edge host {} has mode {}, which is not passthrough or terminate; it is left out",
                    row.host_name,
                    row.mode_enum_id
                ),
            }
        }
        let setups = store::setups(factory)?;
        let mut loaded = setup_maps(setups, takes_webhooks());
        loaded.load = load;
        loaded.routes = table;
        let count = loaded.routes.len();
        let mut current = write(&self.table);
        if current.load < load {
            *current = loaded;
        }
        Ok(count)
    }
}

/// A host name's normal form ([`moqui_core::edge::hosts::normalize`]).
pub(crate) use moqui_core::edge::hosts::normalize;

/// The rows, of one node when `node` is given, by name ([`moqui_core::edge::hosts::list`]).
pub(crate) fn list(factory: &ExecutionContextFactory, node: Option<&str>) -> Result<Vec<EdgeHostInfo>, Error> {
    moqui_core::edge::hosts::list(&store::facade(factory)?, node)
}

/// Save `host` for its node, committed in a transaction of its own ([`moqui_core::edge::hosts::add`]): the caller
/// loads the table again after it.
pub(crate) fn add(factory: &ExecutionContextFactory, host: &EdgeHostInfo) -> Result<EdgeHostInfo, Error> {
    moqui_core::edge::hosts::add(&store::facade(factory)?, host)
}

/// Remove a name; false when there was none ([`moqui_core::edge::hosts::remove`]).
pub(crate) fn remove(factory: &ExecutionContextFactory, host_name: &str) -> Result<bool, Error> {
    moqui_core::edge::hosts::remove(&store::facade(factory)?, host_name)
}

#[cfg(test)]
#[path = "hosts_tests.rs"]
mod tests;
