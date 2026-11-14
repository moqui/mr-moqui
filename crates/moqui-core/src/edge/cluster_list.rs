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

//! An edge cluster's members as a node that takes links lists them (plans/Edge-Node2.md §4.6): from the
//! `ClusterMember` rows, each with its link address (`edge_link_url`), and the check of an address the Hello answer
//! sends. One place for both, so `get#EdgeCluster` and the link's own list agree.

use moqui_lib::error::Error;

use super::EdgeClusterMember;
use crate::context::ExecutionContextFactory;
use crate::entity::EntityFacade;

/// The longest link address a member's list takes.
pub const MAX_LINK_URL: usize = 512;

/// `url` trimmed when it is a link address a local node can dial: an `https` URL with a host (a name, an IPv4 address
/// or a bracketed IPv6 one) and an optional port, at most [`MAX_LINK_URL`] long, with no space or control character.
/// `None` otherwise: the Hello answer then sends the member with no address.
pub fn usable_link_url(url: &str) -> Option<&str> {
    let url = url.trim();
    if url.len() > MAX_LINK_URL || url.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
        return None;
    }
    let rest = url
        .get(..8)
        .filter(|scheme| scheme.eq_ignore_ascii_case("https://"))
        .map(|_| &url[8..])?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return None;
    }
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, after) = bracketed.split_once(']')?;
        if !host.chars().all(|ch| ch.is_ascii_hexdigit() || ch == ':' || ch == '.') {
            return None;
        }
        let port = if after.is_empty() {
            None
        } else {
            Some(after.strip_prefix(':')?)
        };
        (host, port)
    } else {
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        };
        if !host
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '.' || ch == '_')
        {
            return None;
        }
        (host, port)
    };
    if host.is_empty() {
        return None;
    }
    if let Some(port) = port {
        if !port.bytes().all(|byte| byte.is_ascii_digit()) || port.parse::<u16>().map_or(true, |port| port == 0) {
            return None;
        }
    }
    Some(url)
}

/// The members of `factory`'s cluster with their link addresses, live or not, in id order: this member (`own_id`) with
/// `own_url`, its `edge_link_url` as this process has it, and listed live when its row is not written yet.
///
/// # Errors
///
/// When the member rows cannot be read.
pub fn members(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    own_id: &str,
    own_url: &str,
) -> Result<Vec<EdgeClusterMember>, Error> {
    let mut members: Vec<EdgeClusterMember> = crate::cluster::member_report(factory, facade)?
        .into_iter()
        .map(|row| {
            let this_member = row.instance_id == own_id;
            EdgeClusterMember {
                link_url: if this_member {
                    own_url.to_string()
                } else {
                    row.edge_link_url
                },
                instance_id: row.instance_id,
                live: row.live,
                heartbeat_age_ms: row.heartbeat_age_ms,
                version: row.version,
                this_member,
            }
        })
        .collect();
    if !own_id.is_empty() && !members.iter().any(|member| member.this_member) {
        members.push(EdgeClusterMember {
            instance_id: own_id.to_string(),
            link_url: own_url.to_string(),
            live: true,
            heartbeat_age_ms: 0,
            version: env!("CARGO_PKG_VERSION").to_string(),
            this_member: true,
        });
        members.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
    }
    Ok(members)
}

/// The member that registers the SIP trunks now, by the id `get#EdgeCluster` lists it with: the holder of the lock row
/// [`super::sip::TRUNK_LOCK`] while its lease lasts by the database's clock, as the SIP engines claim it. A node with
/// no cluster instance id claims it as `local` (the host's `rtc::own_node`), which is this member (`own_id`). `None`
/// with no holder, a lapsed lease, or a row that cannot be read (logged).
pub fn trunk_registrar(factory: &ExecutionContextFactory, facade: &EntityFacade, own_id: &str) -> Option<String> {
    let holder = match super::replica::sender::holder(facade, super::sip::TRUNK_LOCK) {
        Ok(holder) => holder,
        Err(err) => {
            tracing::error!("Edge cluster: the SIP trunk lock was not read: {err}");
            return None;
        }
    };
    let (instance, until) = holder?;
    if until.is_none_or(|until| until <= crate::cluster::db_now(facade)) {
        return None;
    }
    if instance == "local" && crate::cluster::instance_id(factory).is_none() {
        return Some(own_id.to_string());
    }
    Some(instance)
}

#[cfg(test)]
#[path = "cluster_list_tests.rs"]
mod tests;
