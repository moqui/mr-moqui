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

//! The edge members' own link names on a front node (plans/Edge-Node2.md §4.7, E19). Each member of an edge cluster
//! behind a front has a name of its own, its `edge_link_url`'s host, where local nodes dial it (§4.6). The name points
//! at the front, which passes its connections through untouched to that member alone, so a local node's link (its
//! ALPN `moqui-edge/1` and client certificate) reaches the member as if dialed directly.
//!
//! The member names its link address in the Hello of its front link (`Hello.link_url`). The front keeps the name as an
//! `EdgeHost` row in `EdgeHostLinkName` mode with the member's id ([`moqui_core::edge::hosts::claim_link_name`]): the
//! first claim holds against other pairings, a name of the front's own is refused, and the row stays while the member
//! is away, so the name is refused then rather than taken by anyone else; a name no member sends any more stays until it
//! is removed on System › Edge Clients › Domains, the only cleanup. Only a front node (the front set without the edge
//! set) keeps link names, and only from a node with the edge set. The listener picks a link name before the ALPN
//! ([`super::Acceptor::accept`]).

use std::sync::Arc;

use super::EdgeServer;

/// Keep member `member`'s link name for pairing `node` from the `url` its Hello named, on a front node (the front set
/// without the edge set) and from a node with the edge set (its Hello's `role`): written, and the host table loaded
/// again when it changed. A refusal is logged here only; the member's link goes on without the name, and its Hello
/// answer does not say so (the front's log and Domains do).
pub(super) async fn claim(server: &Arc<EdgeServer>, node: &str, member: &str, role: &str, url: &str) {
    let front_only = server.config.front && !server.config.edge;
    if url.trim().is_empty() || !front_only {
        return;
    }
    if !moqui_core::edge::NodeRole::parse(role).has_edge() {
        tracing::warn!(
            "Edge link of {node} member {member}: it names a link address but is no node with the edge set (role {}); \
             not kept",
            role.chars().take(20).collect::<String>()
        );
        return;
    }
    let Some(name) = moqui_core::edge::claims::url_host(url) else {
        tracing::warn!("Edge link of {node} member {member}: its link address {url} names no host; not kept");
        return;
    };
    let (key, id, host) = (node.to_string(), member.to_string(), name.clone());
    let loading = Arc::clone(server);
    let claimed = server
        .on_pool(move |factory| {
            let reserved = super::super::offer::reserved_names(factory, &loading.config);
            if moqui_core::edge::setup::is_reserved(&host, &reserved) {
                return Err(moqui_lib::error::Error::new(format!(
                    "{host} is a name of this node's own"
                )));
            }
            let facade = super::super::store::facade(factory)?;
            let claim = moqui_core::edge::hosts::claim_link_name(&facade, &host, &key, &id)?;
            if claim.changed {
                loading.hosts.changed(factory)?;
            }
            Ok(claim)
        })
        .await;
    match claimed {
        Ok(claim) => {
            if let Some(from) = &claim.moved_from {
                tracing::warn!(
                    "Edge link of {node}: link name {name} was member {from}'s and is now member {member}'s; two \
                     members that send one edge_link_url take it from each other, so give each its own"
                );
            }
            if claim.changed {
                tracing::info!("Edge link of {node} member {member}: its link name {name} passes through to it");
            }
        }
        Err(err) => tracing::warn!("Edge link of {node} member {member}: its link name {name} is not kept: {err}"),
    }
}
