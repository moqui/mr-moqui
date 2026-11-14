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

//! A node's second pairing, with its front node (plans/Edge-Node2.md §4.7, E19): what goes on which pairing.
//!
//! - **An edge node that dials its front** (`front_edge` or `edge`) registers its host names there itself, on each new
//!   link and whenever they change ([`names_for_front`]): the hosts of its own sites (its mr-pass sites and its chat
//!   address) as terminate hosts, so the front sees the session cookie and balances them sticky by it; and with the
//!   front set (`front_edge`) every name of the local nodes linked to it, each in its own mode, which it forwards on.
//!   Its own `webapp_http_host` stays its own: its link and its System app are reached at it directly. The edge
//!   cluster's members share a database and a `.env`, so they are one pairing on the front, pooled and balanced there.
//! - **A local node with two pairings** (behind `edge` members, which forward no web access) sends the wizard's host
//!   names and XMPP domain to its front, and its mail, chat bridges and PASS sites to its edge cluster
//!   ([`split_setup`]); the offer it shows is the edge pairing's with the front's web access and ports ([`merge_offer`]).
//!   The queue, delegated calls, replication and comms are the edge pairing's alone: the front pairing's links carry
//!   none (`LocalConfig::front`).
//!
//! Media, TURN and SIP never go through a front: they are at each edge member's own address, as media never goes over a
//! link.

use std::collections::BTreeMap;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::edge::setup::{Configured, EdgeOffer, EdgeSetup, HostResult};
use moqui_core::edge::{EdgeHostInfo, NodeRole, HOST_TERMINATE};
use moqui_lib::error::Error;

/// The host names an edge node of `role` gives its front (plans/Edge-Node2.md §4.7), in name order: its own sites'
/// hosts and its chat address's host (and an all-in-one node's own `webapp_http_host`) as terminate hosts, and with the
/// front set every active pairing's names in their modes. A name that is not a public host name is left out, logged.
pub(crate) fn names_for_front(factory: &ExecutionContextFactory, role: NodeRole) -> Result<Vec<EdgeHostInfo>, Error> {
    let mut own = moqui_core::screen::public_site_hosts(factory);
    // an all-in-one node is a full server: its own app's name is reached through the front too. An edge node's own
    // name is its link's and its System app's, reached directly (19B makes the link names the front's passthrough
    // hosts)
    if role.all_in_one() {
        own.push(moqui_lib::env::get_prop_or_env("webapp_http_host"));
    }
    let chat = moqui_lib::env::get_prop_or_env(moqui_core::edge::setup::CHAT_PUBLIC_URL);
    own.extend(moqui_core::edge::claims::url_host(&chat));
    let mut forwarded = Vec::new();
    if role.has_front() {
        let facade = super::store::facade(factory)?;
        let active = moqui_core::edge::claims::active_nodes(&facade)?;
        forwarded = moqui_core::edge::hosts::list(&facade, None)?
            .into_iter()
            .filter(|host| active.contains(&host.paired_node_id))
            .collect();
    }
    Ok(names_of(&own, &forwarded))
}

/// [`names_for_front`]'s rule: `own` names as terminate hosts, then each of `forwarded` in its mode (a name of its own
/// keeps terminate), each name once and in its normal form; one that is not a public host name is logged and left out.
pub(crate) fn names_of(own: &[String], forwarded: &[EdgeHostInfo]) -> Vec<EdgeHostInfo> {
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    let mut take = |name: &str, mode: &str| match moqui_core::edge::setup::check_host_name(name) {
        Ok(name) => {
            names.entry(name).or_insert_with(|| mode.to_string());
        }
        Err(why) => tracing::warn!("Front link: {name} is not given to the front node: {why}"),
    };
    for name in own.iter().filter(|name| !name.trim().is_empty()) {
        take(name, HOST_TERMINATE);
    }
    for host in forwarded {
        take(&host.host_name, &host.mode_enum_id);
    }
    names
        .into_iter()
        .map(|(host_name, mode_enum_id)| EdgeHostInfo {
            host_name,
            paired_node_id: String::new(),
            mode_enum_id,
            sticky_cookie: None,
        })
        .collect()
}

/// A local node's setup split between its two pairings (plans/Edge-Node2.md §4.7): the web access names and the XMPP
/// domain (its tunnel ports are the front's) for the front, the rest for the edge pairing. Each pairing hears every
/// Configure, as each keeps its part whole: an XMPP domain cleared here is cleared on the front too. A setup that sets
/// the host names sets none on the edge pairing, so an edge node with the front set (`front_edge`) does not forward
/// them to the front as well, where the two pairings would claim the same names.
pub(crate) fn split_setup(setup: &EdgeSetup) -> (EdgeSetup, EdgeSetup) {
    let front = EdgeSetup {
        hosts: setup.hosts.clone(),
        set_hosts: setup.set_hosts,
        xmpp_domain: setup.xmpp_domain.clone(),
        ..EdgeSetup::default()
    };
    let edge = EdgeSetup {
        set_hosts: setup.set_hosts,
        mail_domains: setup.mail_domains.clone(),
        chat_bridge_ids: setup.chat_bridge_ids.clone(),
        pass_sites: setup.pass_sites.clone(),
        ..EdgeSetup::default()
    };
    (front, edge)
}

/// The answers for a part of a setup a pairing did not take as a whole, because its Configure failed (`why`): each of
/// its items as a result with that error, so the rest of the answer stands (plans/Edge-Node2.md §4.7).
pub(crate) fn failed_results(part: &EdgeSetup, pairing: &str, why: &str) -> Vec<HostResult> {
    let error = format!("the {pairing} did not take it: {why}");
    let mut results: Vec<HostResult> = part
        .hosts
        .iter()
        .map(|host| HostResult {
            host_name: host.host_name.clone(),
            mode_enum_id: host.mode_enum_id.clone(),
            error: error.clone(),
        })
        .collect();
    for (field, items) in [
        ("mailDomains", &part.mail_domains),
        ("chatBridgeIds", &part.chat_bridge_ids),
        ("passSites", &part.pass_sites),
    ] {
        results.extend(items.iter().map(|item| HostResult {
            host_name: item.clone(),
            mode_enum_id: field.to_string(),
            error: error.clone(),
        }));
    }
    if !part.xmpp_domain.is_empty() {
        results.push(HostResult {
            host_name: part.xmpp_domain.clone(),
            mode_enum_id: "xmppDomain".to_string(),
            error: error.clone(),
        });
    }
    if results.is_empty() {
        // a part that named nothing (a clearing) failed too: one line says so
        results.push(HostResult {
            host_name: String::new(),
            mode_enum_id: String::new(),
            error,
        });
    }
    results
}

/// The offer a local node with two pairings shows (plans/Edge-Node2.md §4.7): its edge pairing's, with the front's web
/// access (its names, modes and this node's hosts there), XMPP ports and public addresses. What the front offers is
/// taken off the edge offer's areas not available.
pub(crate) fn merge_offer(edge: EdgeOffer, front: EdgeOffer) -> EdgeOffer {
    let mut merged = edge;
    merged.public_names = front.public_names;
    merged.host_modes = front.host_modes;
    merged.node_hosts = front.node_hosts;
    merged.xmpp = front.xmpp;
    if !front.addresses.is_empty() {
        merged.addresses = front.addresses;
    }
    let offered: Vec<&str> = [
        (!merged.host_modes.is_empty()).then_some("web"),
        merged.xmpp.available.then_some("xmpp"),
    ]
    .into_iter()
    .flatten()
    .collect();
    merged.not_available.retain(|area| !offered.contains(&area.as_str()));
    merged
}

/// A Configure's answers from both pairings as one: the host results of each, and the offer [`merge_offer`] makes.
pub(crate) fn merge_configured(edge: Option<Configured>, front: Option<Configured>) -> Configured {
    match (edge, front) {
        (Some(edge), Some(front)) => Configured {
            offer: merge_offer(edge.offer, front.offer),
            hosts: front.hosts.into_iter().chain(edge.hosts).collect(),
        },
        (Some(one), None) | (None, Some(one)) => one,
        (None, None) => Configured::default(),
    }
}

#[cfg(test)]
#[path = "front_tests.rs"]
mod tests;
