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

//! The rows of a node that takes links (plans/archive/Edge-Node.md §5.3, plans/Edge-Node2.md §4.10): the public host
//! names it answers for each pairing (`moqui.edge.EdgeHost`) and the pairings themselves (`moqui.edge.EdgeClient`), as
//! the System app's Edge Clients screens and a pairing's Configure read and write them. They need no link: the host's
//! edge link keeps a table of the host names in memory and loads it again when told
//! ([`super::EdgeLink::hosts_changed`]), and adds each pairing's live link to [`clients`].

use std::time::Duration;

use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};

use super::{EdgeClientInfo, EdgeHostInfo, HOST_LINK_NAME, HOST_PASSTHROUGH, HOST_TERMINATE};
use crate::context::ExecutionContext;
use crate::entity::{EntityFacade, EntityValue};
use crate::transaction::TransactionAttribute;

const HOST: &str = "moqui.edge.EdgeHost";
const CLIENT: &str = "moqui.edge.EdgeClient";
/// A pairing that may link.
pub const ACTIVE: &str = "EdgeClientActive";

fn text(row: &EntityValue, field: &str) -> String {
    row.get(field).map(to_plain_string).unwrap_or_default()
}

/// `body` in a transaction on the edge rows (a new one with `own`, committed when it returns, for a change the caller
/// then acts on in memory), with authorization off: the edge node's own bookkeeping, which the services' callers are
/// authorized for.
fn transaction<T>(facade: &EntityFacade, own: bool, body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let attribute = if own {
        TransactionAttribute::ForceNew
    } else {
        TransactionAttribute::UseOrBegin
    };
    facade
        .transactions_for(HOST)?
        .run(attribute, Some(Duration::from_secs(30)), body)
}

/// A field of a pairing's kept setup (`EdgeClient.setupJson`) as names: a string or a list of strings, trimmed, the
/// empty ones left out.
pub fn setup_list(setup: &serde_json::Value, field: &str) -> Vec<String> {
    let names: Vec<&str> = match &setup[field] {
        serde_json::Value::String(name) => vec![name.as_str()],
        serde_json::Value::Array(names) => names.iter().filter_map(serde_json::Value::as_str).collect(),
        _ => Vec::new(),
    };
    names
        .into_iter()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

/// A host name's normal form: trimmed, lower case, with no port or trailing dot, or `None` when it is not a DNS name
/// (letters, digits, `-` and `.`, at most 253 bytes; an IP literal in brackets is not one).
pub fn normalize(name: &str) -> Option<String> {
    let name = name.trim();
    let name = match name.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) => host,
        _ => name,
    };
    let name = name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase();
    let valid = !name.is_empty()
        && name.len() <= 253
        && !name.starts_with('.')
        && !name.contains("..")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.');
    valid.then_some(name)
}

/// The longest sticky cookie name a host takes.
pub const MAX_COOKIE_NAME: usize = 64;

/// Whether `name` is a cookie name (an RFC 6265 token: visible ASCII but separators), at most [`MAX_COOKIE_NAME`].
pub fn valid_cookie_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_COOKIE_NAME
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
}

/// The host name rows, of one pairing when `node` is given, by name.
///
/// # Errors
///
/// Returns the entity facade's error.
pub fn list(facade: &EntityFacade, node: Option<&str>) -> Result<Vec<EdgeHostInfo>, Error> {
    transaction(facade, false, || {
        let mut find = facade.find(HOST)?;
        if let Some(node) = node.map(str::trim).filter(|node| !node.is_empty()) {
            find = find.eq("pairedNodeId", Value::string(node));
        }
        let rows = find.order_by("hostName", false).list()?;
        Ok(rows
            .iter()
            .map(|row| EdgeHostInfo {
                host_name: text(row, "hostName"),
                paired_node_id: text(row, "pairedNodeId"),
                mode_enum_id: text(row, "modeEnumId"),
                sticky_cookie: Some(text(row, "stickyCookie")),
            })
            .collect())
    })
}

/// Save `host` for its pairing: a new name, or a new mode for a name the pairing has. It commits in a transaction of
/// its own, so a table in memory loaded again after it never holds a change a caller's transaction rolls back.
///
/// # Errors
///
/// Refuses a name that is not a DNS name, a mode other than passthrough or terminate, a pairing that is not active,
/// and a name another pairing has, with the generic [`super::claims::HELD_BY_ANOTHER`] (the first claim holds,
/// plans/Edge-Node2.md §4.1; the Domains screen shows which pairing has it); returns the entity facade's error.
pub fn add(facade: &EntityFacade, host: &EdgeHostInfo) -> Result<EdgeHostInfo, Error> {
    let name = normalize(&host.host_name)
        .ok_or_else(|| Error::new(format!("{} is not a host name", host.host_name.trim())))?;
    if host.mode_enum_id != HOST_PASSTHROUGH && host.mode_enum_id != HOST_TERMINATE {
        return Err(Error::new(format!(
            "the mode must be {HOST_PASSTHROUGH} or {HOST_TERMINATE}"
        )));
    }
    let cookie = match host.sticky_cookie.as_deref().map(str::trim) {
        Some(cookie) if !cookie.is_empty() && !valid_cookie_name(cookie) => {
            return Err(Error::new(format!(
                "{cookie} is not a cookie name (letters, digits and !#$%&'*+-.^_`|~, at most {MAX_COOKIE_NAME})"
            )))
        }
        cookie => cookie,
    };
    let node = host.paired_node_id.trim();
    transaction(facade, true, || {
        let active = facade
            .find(CLIENT)?
            .eq("pairedNodeId", Value::string(node))
            .one()?
            .is_some_and(|row| text(&row, "statusId") == ACTIVE);
        if node.is_empty() || !active {
            return Err(Error::new(format!(
                "no local node {node} is paired with this edge node, or it was revoked"
            )));
        }
        let existing = facade
            .find(HOST)?
            .eq("hostName", Value::string(&name))
            .for_update(true)
            .one()?;
        if let Some(row) = existing
            .as_ref()
            .filter(|row| text(row, "modeEnumId") == HOST_LINK_NAME)
        {
            return Err(link_name_held(&name, &text(row, "pairedNodeId"), node));
        }
        let existing = existing.map(|row| text(&row, "pairedNodeId"));
        save(facade, &name, node, &host.mode_enum_id, cookie, existing)?;
        Ok(EdgeHostInfo {
            host_name: name.clone(),
            paired_node_id: node.to_string(),
            mode_enum_id: host.mode_enum_id.clone(),
            sticky_cookie: cookie.map(str::to_string),
        })
    })
}

/// Write the row of `name` for `node`, given its holder as read (`existing`): a new name is inserted only if no row
/// has come since the read (a lock on a missing row holds nothing), else the holder is read again, so of two
/// concurrent claims of a new name the second is refused with [`super::claims::HELD_BY_ANOTHER`] rather than taking
/// it; the holder's own name gets its new mode, and its sticky cookie when `cookie` is given (empty clears it).
fn save(
    facade: &EntityFacade,
    name: &str,
    node: &str,
    mode: &str,
    cookie: Option<&str>,
    existing: Option<String>,
) -> Result<(), Error> {
    let held = || Error::new(format!("{name}: {}", super::claims::HELD_BY_ANOTHER));
    let cookie_value = |cookie: &str| {
        if cookie.is_empty() {
            Value::Null
        } else {
            Value::string(cookie)
        }
    };
    let mut row = facade.make_value(HOST)?;
    row.set("hostName", Value::string(name));
    row.set("pairedNodeId", Value::string(node));
    row.set("modeEnumId", Value::string(mode));
    if let Some(cookie) = cookie {
        row.set("stickyCookie", cookie_value(cookie));
    }
    let holder = match existing {
        Some(holder) => holder,
        None => {
            row.set("createdDate", crate::user::now_value());
            if row.create_if_missing(facade)? {
                return Ok(());
            }
            let Some(holder) = facade
                .find(HOST)?
                .eq("hostName", Value::string(name))
                .for_update(true)
                .one()?
            else {
                return Err(Error::new(format!("{name} changed while it was saved; try again")));
            };
            if text(&holder, "modeEnumId") == HOST_LINK_NAME {
                return Err(link_name_held(name, &text(&holder, "pairedNodeId"), node));
            }
            text(&holder, "pairedNodeId")
        }
    };
    if holder != node {
        return Err(held());
    }
    let mut update = facade.make_value(HOST)?;
    update.set("hostName", Value::string(name));
    update.set("modeEnumId", Value::string(mode));
    if let Some(cookie) = cookie {
        update.set("stickyCookie", cookie_value(cookie));
    }
    update.update(facade)?;
    Ok(())
}

/// The refusal of a host name that is an edge member's link name (plans/Edge-Node2.md §4.7): another pairing's gets the
/// generic [`super::claims::HELD_BY_ANOTHER`], the pairing's own says what it is.
fn link_name_held(name: &str, holder: &str, node: &str) -> Error {
    if holder == node {
        Error::new(format!(
            "{name} is the link name of a member of this pairing, which the front keeps from its Hello"
        ))
    } else {
        Error::new(format!("{name}: {}", super::claims::HELD_BY_ANOTHER))
    }
}

/// An edge member's own link name on a front node (plans/Edge-Node2.md §4.7): an `EdgeHost` row in [`HOST_LINK_NAME`]
/// mode, whose connections pass through to that member alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkName {
    pub host_name: String,
    pub paired_node_id: String,
    /// The member's instance id.
    pub member_id: String,
}

/// The link names, by name.
///
/// # Errors
///
/// Returns the entity facade's error.
pub fn link_names(facade: &EntityFacade) -> Result<Vec<LinkName>, Error> {
    transaction(facade, false, || {
        let rows = facade
            .find(HOST)?
            .eq("modeEnumId", Value::string(HOST_LINK_NAME))
            .order_by("hostName", false)
            .list()?;
        Ok(rows
            .iter()
            .map(|row| LinkName {
                host_name: text(row, "hostName"),
                paired_node_id: text(row, "pairedNodeId"),
                member_id: text(row, "memberId"),
            })
            .collect())
    })
}

/// What [`claim_link_name`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkClaim {
    /// A row changed: the caller loads its table again.
    pub changed: bool,
    /// The name was another member's of the pairing, now this one's: a member back with another instance id, or two
    /// members sending one `edge_link_url`, which flip it back and forth.
    pub moved_from: Option<String>,
}

/// Keep `name` as member `member`'s link name for pairing `node` (plans/Edge-Node2.md §4.7), from the member's Hello
/// on a front node, in a transaction of its own: a new name is inserted only while it is free, a link name of the
/// pairing moves to `member` (a member that came back with another instance id), and the member's other link names go.
/// A name no member sends any more stays until it is removed on Domains.
///
/// # Errors
///
/// Refuses a name that is not a DNS name, a pairing that is not active, a name another pairing has (the generic
/// [`super::claims::HELD_BY_ANOTHER`]) and a name the pairing has as a host name of its own; returns the entity facade's
/// error.
pub fn claim_link_name(facade: &EntityFacade, name: &str, node: &str, member: &str) -> Result<LinkClaim, Error> {
    let name = normalize(name).ok_or_else(|| Error::new(format!("{} is not a host name", name.trim())))?;
    let (node, member) = (node.trim(), member.trim());
    if member.is_empty() {
        return Err(Error::new("a link name needs its member"));
    }
    transaction(facade, true, || {
        let active = facade
            .find(CLIENT)?
            .eq("pairedNodeId", Value::string(node))
            .one()?
            .is_some_and(|row| text(&row, "statusId") == ACTIVE);
        if node.is_empty() || !active {
            return Err(Error::new(format!(
                "no node {node} is paired with this front node, or it was revoked"
            )));
        }
        let read = || {
            facade
                .find(HOST)?
                .eq("hostName", Value::string(&name))
                .for_update(true)
                .one()
        };
        let mut existing = read()?;
        let mut claim = LinkClaim::default();
        if existing.is_none() {
            let mut row = facade.make_value(HOST)?;
            row.set("hostName", Value::string(&name));
            row.set("pairedNodeId", Value::string(node));
            row.set("modeEnumId", Value::string(HOST_LINK_NAME));
            row.set("memberId", Value::string(member));
            row.set("createdDate", crate::user::now_value());
            if row.create_if_missing(facade)? {
                claim.changed = true;
            } else {
                existing = read()?;
                if existing.is_none() {
                    return Err(Error::new(format!("{name} changed while it was saved; try again")));
                }
            }
        }
        if let Some(row) = existing {
            if text(&row, "pairedNodeId") != node {
                return Err(Error::new(format!("{name}: {}", super::claims::HELD_BY_ANOTHER)));
            }
            if text(&row, "modeEnumId") != HOST_LINK_NAME {
                return Err(Error::new(format!(
                    "{name} is a host name of this pairing, not a link name: remove it first"
                )));
            }
            let holder = text(&row, "memberId");
            if holder != member {
                let mut update = facade.make_value(HOST)?;
                update.set("hostName", Value::string(&name));
                update.set("memberId", Value::string(member));
                update.update(facade)?;
                claim.changed = true;
                claim.moved_from = Some(holder).filter(|holder| !holder.is_empty());
            }
        }
        let others = facade
            .find(HOST)?
            .eq("pairedNodeId", Value::string(node))
            .eq("modeEnumId", Value::string(HOST_LINK_NAME))
            .eq("memberId", Value::string(member))
            .list()?;
        for other in others.iter().filter(|row| text(row, "hostName") != name) {
            let mut row = facade.make_value(HOST)?;
            row.set("hostName", Value::string(text(other, "hostName")));
            row.delete(facade)?;
            claim.changed = true;
        }
        Ok(claim)
    })
}

/// Remove a name, in a transaction of its own; false when there was none.
///
/// # Errors
///
/// Returns the entity facade's error.
pub fn remove(facade: &EntityFacade, host_name: &str) -> Result<bool, Error> {
    let Some(name) = normalize(host_name) else {
        return Ok(false);
    };
    transaction(facade, true, || {
        if facade.find(HOST)?.eq("hostName", Value::string(&name)).one()?.is_none() {
            return Ok(false);
        }
        let mut row = facade.make_value(HOST)?;
        row.set("hostName", Value::string(&name));
        row.delete(facade)?;
        Ok(true)
    })
}

/// The pairings, by id, as their rows have them (no live link: the host's edge link adds it).
///
/// # Errors
///
/// Returns the entity facade's error.
pub fn clients(facade: &EntityFacade) -> Result<Vec<EdgeClientInfo>, Error> {
    transaction(facade, false, || {
        let rows = facade.find(CLIENT)?.order_by("pairedNodeId", false).list()?;
        Ok(rows
            .iter()
            .map(|row| EdgeClientInfo {
                paired_node_id: text(row, "pairedNodeId"),
                key_fingerprint: text(row, "keyFingerprint"),
                status_id: text(row, "statusId"),
                linked: false,
                down: false,
                generation: text(row, "linkGeneration").parse().unwrap_or(0),
                last_connect: row.get("lastConnectDate").and_then(crate::user::value_millis),
                remote_address: text(row, "lastAddress"),
                binary_version: text(row, "binaryVersion"),
                link_version: text(row, "linkVersion").parse().unwrap_or(0),
                setup_json: text(row, "setupJson"),
                setup_date: row.get("setupDate").and_then(crate::user::value_millis),
                queue_max_megabytes: text(row, "queueMaxMegabytes")
                    .split('.')
                    .next()
                    .and_then(|number| number.parse::<i64>().ok())
                    .filter(|megabytes| *megabytes > 0),
                members_linked: 0,
                last_member_id: text(row, "lastMemberId"),
                hold_new_member: text(row, "holdNewMember") == "Y",
                approved_member_id: text(row, "approvedMemberId"),
                takeover_date: row.get("takeoverDate").and_then(crate::user::value_millis),
                takeover_from_id: text(row, "takeoverFromId"),
                takeover_to_id: text(row, "takeoverToId"),
            })
            .collect())
    })
}

/// Set a pairing's own size cap on the items waiting for it (`EdgeClient.queueMaxMegabytes`, plans/Edge-Node2.md
/// §4.1), or with `None` unset it, so the conf's `queue-max-megabytes` holds for it. The retention pass reads it.
///
/// # Errors
///
/// Refuses a cap of 0 or less (unset it instead) and a pairing with no row; returns the entity facade's error.
pub fn set_queue_cap(facade: &EntityFacade, node: &str, megabytes: Option<i64>) -> Result<(), Error> {
    if megabytes.is_some_and(|megabytes| megabytes <= 0) {
        return Err(Error::new(
            "the queue cap is a number of megabytes above 0; leave it empty for the conf's",
        ));
    }
    let node = node.trim();
    transaction(facade, false, || {
        if node.is_empty()
            || facade
                .find(CLIENT)?
                .eq("pairedNodeId", Value::string(node))
                .one()?
                .is_none()
        {
            return Err(Error::new(format!(
                "no local node {node} is paired with this edge node"
            )));
        }
        let mut row = facade.make_value(CLIENT)?;
        row.set("pairedNodeId", Value::string(node));
        row.set("queueMaxMegabytes", megabytes.map_or(Value::Null, Value::I64));
        row.update(facade)?;
        Ok(())
    })
}

#[cfg(test)]
#[path = "hosts_tests.rs"]
mod tests;
