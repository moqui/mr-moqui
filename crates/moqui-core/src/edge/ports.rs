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

//! The TCP ports an edge node pipes to one pairing (plans/Edge-Node2.md §4.1 "SSH", §4.10 "Ports", E16): the
//! `moqui.edge.EdgePort` rows the System app sets, each a port, the pairing its connections go to, and the tunnel's
//! kind (SSH, XMPP with STARTTLS or direct TLS). They change while the node runs: [`add`] and [`remove`] write a row
//! and say so ([`changed`]), and the host's edge link opens or closes the port's listener within seconds, telling how
//! it went ([`set_states`], [`states`]).
//!
//! The conf's own tunnel ports are apart (`edge_ssh_tunnel_port`, `edge_xmpp_port`, `edge_xmpp_tls_port`), and a row
//! may not take one of them, nor another port of this node's own listeners ([`reserved`]).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;

use crate::context::ExecutionContextFactory;
use crate::entity::EntityFacade;

const PORT: &str = "moqui.edge.EdgePort";
const CLIENT: &str = "moqui.edge.EdgeClient";

/// The kinds of tunnel a port row may be: to the node's SSH server, or its XMPP server by STARTTLS or direct TLS (the
/// edge link's tunnel kinds).
pub const KINDS: [&str; 3] = ["ssh", "xmpp", "xmpps"];

/// The properties of this node's own listeners, whose ports no row may take, each with its port when the property is
/// unset (0 for none): a row never takes a service's usual port, set or not.
const OWN_PORTS: [(&str, u16); 18] = [
    ("webapp_http_port", 80),
    ("webapp_https_port", 443),
    ("webapp_https_listen_port", 443),
    ("cluster_https_port", 0),
    ("smtp_listen_port", 25),
    ("edge_mail_relay_port", 10025),
    ("ssh_port", 22),
    ("xmpp_port", 5222),
    ("xmpp_tls_port", 5223),
    ("sip_port", 5060),
    ("sip_tls_port", 5061),
    ("turn_port", 3478),
    ("turn_tls_port", 5349),
    ("rtc_media_port", 10000),
    ("rtc_media_tcp_port", 0),
    ("edge_ssh_tunnel_port", 0),
    ("edge_xmpp_port", 0),
    ("edge_xmpp_tls_port", 0),
];

/// Other ports a mail server's clients and peers use (submission, SMTPS, XMPP federation), which no row may take.
const WELL_KNOWN: [(u16, &str); 3] = [(465, "SMTPS"), (587, "mail submission"), (5269, "XMPP federation")];

/// The port ranges of this node's own sockets, with their defaults: the SIP engine's RTP and the TURN relays.
const OWN_RANGES: [(&str, &str); 2] = [("sip_rtp_ports", "20000-20999"), ("turn_relay_ports", "49152-65535")];

/// A port row: connections to `port` go down the link of the pairing `paired_node_id` as a tunnel of `tunnel_kind`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct EdgePort {
    pub port: u16,
    pub paired_node_id: String,
    pub tunnel_kind: String,
}

/// How a port row's listener is now, as the host's edge link last said.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PortState {
    pub listening: bool,
    /// Why it does not listen; empty while it does.
    pub error: String,
}

#[derive(Default)]
struct Live {
    generation: AtomicU64,
    states: Mutex<BTreeMap<u16, PortState>>,
}

/// The rows changed on this node: the edge link opens and closes listeners to match.
pub fn changed(factory: &ExecutionContextFactory) {
    factory
        .extension(|_| Live::default())
        .generation
        .fetch_add(1, Ordering::AcqRel);
}

/// How many times the rows changed on this node since it started: the edge link reads the rows again when it moves.
pub fn generation(factory: &ExecutionContextFactory) -> u64 {
    factory
        .extension(|_| Live::default())
        .generation
        .load(Ordering::Acquire)
}

/// The edge link's report of its row listeners, by port.
pub fn set_states(factory: &ExecutionContextFactory, states: BTreeMap<u16, PortState>) {
    *lock(&factory.extension(|_| Live::default()).states) = states;
}

/// The row listeners as the edge link last reported them.
pub fn states(factory: &ExecutionContextFactory) -> BTreeMap<u16, PortState> {
    lock(&factory.extension(|_| Live::default()).states).clone()
}

fn text(row: &crate::entity::EntityValue, field: &str) -> String {
    row.get(field).map(to_plain_string).unwrap_or_default()
}

/// The port rows, by port (of one pairing with `paired_node_id`). A row whose port is not a port is logged and left
/// out.
///
/// # Errors
///
/// Returns the entity facade's error.
pub fn list(facade: &EntityFacade, paired_node_id: Option<&str>) -> Result<Vec<EdgePort>, Error> {
    let mut find = facade.find(PORT)?.order_by("port", false).disable_authz();
    if let Some(node) = paired_node_id.map(str::trim).filter(|node| !node.is_empty()) {
        find = find.eq("pairedNodeId", Value::string(node));
    }
    let mut ports = Vec::new();
    for row in find.list()?.values() {
        let port = text(row, "port");
        let Ok(port) = port.split('.').next().unwrap_or_default().parse::<u16>() else {
            tracing::error!("Edge port row {port} is not a port; left out");
            continue;
        };
        ports.push(EdgePort {
            port,
            paired_node_id: text(row, "pairedNodeId"),
            tunnel_kind: text(row, "tunnelKind"),
        });
    }
    Ok(ports)
}

/// What of this node's own takes `port`, when something does: a listener's property (set, or unset with that port as
/// its default), a mail or XMPP port, or a range of its own sockets.
pub fn reserved(port: u16) -> Option<String> {
    reserved_in(port, moqui_lib::env::get_prop_or_env)
}

/// [`reserved`] with the properties `property` gives.
fn reserved_in(port: u16, property: impl Fn(&str) -> String) -> Option<String> {
    for (name, default) in OWN_PORTS {
        let set = property(name);
        let taken = match set.trim() {
            "" => default != 0 && default == port,
            text => text.parse::<u16>().ok() == Some(port),
        };
        if taken {
            return Some(name.to_string());
        }
    }
    if let Some((_, what)) = WELL_KNOWN.iter().find(|(known, _)| *known == port) {
        return Some((*what).to_string());
    }
    for (name, default) in OWN_RANGES {
        let set = property(name);
        let range = if set.trim().is_empty() {
            default.to_string()
        } else {
            set.trim().to_string()
        };
        let (low, high) = range.split_once('-').unwrap_or((range.as_str(), range.as_str()));
        if let (Ok(low), Ok(high)) = (low.trim().parse::<u16>(), high.trim().parse::<u16>()) {
            if (low..=high).contains(&port) {
                return Some(format!("{name} {range}"));
            }
        }
    }
    None
}

/// A row for `port` to the active pairing `paired_node_id`, of `tunnel_kind` ([`KINDS`]): written, and the edge link
/// told. The same row again is no change.
///
/// # Errors
///
/// Refuses a port that is not one, a kind that is not one of [`KINDS`], a pairing that is not active, a port of this
/// node's own listeners ([`reserved`]) and a port another row has; returns the entity facade's error.
pub fn add(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    port: &str,
    paired_node_id: &str,
    tunnel_kind: &str,
) -> Result<EdgePort, Error> {
    let _authz = crate::context::ExecutionContext::with_current(crate::context::ExecutionContext::disable_authz);
    let port: u16 = port
        .trim()
        .parse()
        .ok()
        .filter(|port| *port > 0)
        .ok_or_else(|| Error::new(format!("{} is not a port", port.trim())))?;
    let kind = tunnel_kind.trim().to_ascii_lowercase();
    if !KINDS.contains(&kind.as_str()) {
        return Err(Error::new(format!("tunnelKind must be one of {}", KINDS.join(", "))));
    }
    let node = paired_node_id.trim();
    let active = facade
        .find(CLIENT)?
        .eq("pairedNodeId", Value::string(node))
        .eq("statusId", Value::string("EdgeClientActive"))
        .disable_authz()
        .count()?;
    if node.is_empty() || active == 0 {
        return Err(Error::new(format!("{node} is not an active paired node")));
    }
    if let Some(what) = reserved(port) {
        return Err(Error::new(format!("port {port} is this node's own ({what})")));
    }
    let wanted = EdgePort {
        port,
        paired_node_id: node.to_string(),
        tunnel_kind: kind,
    };
    if let Some(held) = list(facade, None)?.into_iter().find(|held| held.port == port) {
        if held == wanted {
            return Ok(held);
        }
        return Err(Error::new(format!(
            "port {port} goes to another paired node or kind already: remove it first"
        )));
    }
    let mut row = facade.make_value(PORT)?;
    row.set("port", Value::I64(i64::from(port)));
    row.set("pairedNodeId", Value::string(&wanted.paired_node_id));
    row.set("tunnelKind", Value::string(&wanted.tunnel_kind));
    row.set("createdDate", crate::user::now_value());
    row.create(facade)?;
    changed(factory);
    tracing::info!(
        "Edge node: port {port} goes to local node {} ({})",
        wanted.paired_node_id,
        wanted.tunnel_kind
    );
    Ok(wanted)
}

/// Remove the row of `port`, and tell the edge link: whether there was one.
///
/// # Errors
///
/// Returns the entity facade's error.
pub fn remove(factory: &ExecutionContextFactory, facade: &EntityFacade, port: &str) -> Result<bool, Error> {
    let _authz = crate::context::ExecutionContext::with_current(crate::context::ExecutionContext::disable_authz);
    let port: u16 = port
        .trim()
        .parse()
        .map_err(|_| Error::new(format!("{} is not a port", port.trim())))?;
    let removed = facade
        .find(PORT)?
        .eq("port", Value::I64(i64::from(port)))
        .disable_authz()
        .delete_all()?;
    if removed > 0 {
        changed(factory);
        tracing::info!("Edge node: port {port} removed");
    }
    Ok(removed > 0)
}

#[cfg(test)]
#[path = "ports_tests.rs"]
mod tests;
