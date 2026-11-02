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

//! The calls area of the wizard on a local node (plans/Edge-Node.md §5.7, "As built (E10, calls)"), on E7's edge node
//! as media node (§5.4): the edge node's `CallMediaNode` row, which the link keeps ([`register_row`]), the local
//! setting that turns its preference off ([`CALLS_ENABLED`]), Apply's lines ([`apply`]) and the Test step's checks of
//! the row and the TURN credentials ([`row_check`], [`ice_check`]). The TURN allocation itself is the host's
//! ([`super::EdgeLink::turn_probe`]).
//!
//! **Calls off.** E7 keeps the row from the link and `Calls.js` `liveMediaNodes` puts it first, so a paired node's new
//! calls have their media on the edge node. With `edge_calls_enabled=false` in `runtime/.env` the link keeps the row
//! as `draining`: `Calls.js` gives a draining node no new call, and calls already on the edge node go on (the row
//! stays live, so `expire#Calls` does not end them and the delegated media events are still taken).

use std::net::{IpAddr, SocketAddr};

use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};

use super::setup::{set_env, CallsOffer, SetupLine};
use super::{EdgeInfo, LinkStatus};
use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::EntityFacade;

/// The local setting (`runtime/.env`) that turns the edge node's media node off for new calls: on unless it is
/// `false`, `n`, `no`, `0` or `off`. Apply writes it; unpairing removes it.
pub const CALLS_ENABLED: &str = "edge_calls_enabled";
/// A media node's row is live while its heartbeat is this recent (`MEDIA_NODE_STALE_MS` in `Calls.js`).
pub const STALE_MS: i64 = 60_000;
/// The leg of the Test step's TURN credential: no leg of a call, and new each run.
pub const PROBE_LEG: &str = "edgeSetupProbe";

const MEDIA_NODE: &str = "moqui.call.CallMediaNode";
const REGISTER_MEDIA_NODE: &str = "org.moqui.impl.CallServices.register#MediaNode";
const MAKE_ICE_SERVERS: &str = "org.moqui.impl.CallServices.make#IceServers";

/// Whether new calls may have their media on the edge node ([`CALLS_ENABLED`]).
pub fn calls_enabled() -> bool {
    !matches!(
        moqui_lib::env::get_prop_or_env(CALLS_ENABLED)
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "false" | "n" | "no" | "0" | "off"
    )
}

/// Write or remove (`stopping`) the `CallMediaNode` row of the edge node `edge`, as the link keeps it: `draining`
/// while [`calls_enabled`] is off. Internal work in the current context; its errors are taken out of the context, so
/// the caller's stay as they were.
///
/// # Errors
///
/// Returns why the row was not written.
pub fn register_row(edge: &EdgeInfo, stopping: bool) -> Result<(), String> {
    let parameters = std::collections::BTreeMap::from([
        ("instanceId".to_string(), Value::string(&edge.instance_id)),
        ("mediaAddress".to_string(), Value::string(&edge.media_address)),
        ("turnUrls".to_string(), Value::string(edge.turn_urls.join(","))),
        ("edgeLinked".to_string(), Value::Bool(true)),
        ("draining".to_string(), Value::Bool(!calls_enabled())),
        (
            "startedDate".to_string(),
            if edge.media_started_ms > 0 {
                crate::user::stamp_value(edge.media_started_ms)
            } else {
                Value::Null
            },
        ),
        ("stopping".to_string(), Value::Bool(stopping)),
    ]);
    internal(|| crate::service::sync(REGISTER_MEDIA_NODE, &parameters).map(drop))
}

/// Run `call` with authorization off and its errors apart from the caller's: the call's error, else the errors it
/// added.
fn internal<T, E: std::fmt::Display>(call: impl FnOnce() -> Result<T, E>) -> Result<T, String> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    ExecutionContext::with_current(|ec| ec.message().push_errors());
    let ran = call();
    let errors = ExecutionContext::with_current(|ec| {
        let mut messages = ec.message();
        let errors = messages.errors().to_vec();
        messages.clear_errors();
        messages.pop_errors();
        errors
    })
    .unwrap_or_default();
    match ran {
        Err(err) => Err(err.to_string()),
        Ok(_) if !errors.is_empty() => Err(errors.join("; ")),
        Ok(out) => Ok(out),
    }
}

/// A media node's `CallMediaNode` row, as the checks read it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MediaRow {
    pub media_address: String,
    pub turn_urls: Vec<String>,
    /// When its heartbeat was last written (epoch milliseconds).
    pub heartbeat_ms: i64,
    pub draining: bool,
    pub edge_linked: bool,
}

/// The `CallMediaNode` row of `instance_id`, read past the cache.
///
/// # Errors
///
/// Returns the entity facade's error.
pub fn media_row(facade: &EntityFacade, instance_id: &str) -> Result<Option<MediaRow>, Error> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let row = facade
        .find(MEDIA_NODE)?
        .eq("instanceId", Value::string(instance_id))
        .use_cache(false)
        .one()?;
    Ok(row.map(|row| {
        let text = |field: &str| row.get(field).map(to_plain_string).unwrap_or_default();
        MediaRow {
            media_address: text("mediaAddress"),
            turn_urls: text("turnUrls")
                .split(',')
                .map(str::trim)
                .filter(|url| !url.is_empty())
                .map(str::to_string)
                .collect(),
            heartbeat_ms: row
                .get("heartbeatDate")
                .and_then(crate::user::value_millis)
                .unwrap_or(0),
            draining: text("draining") == "Y",
            edge_linked: text("edgeLinked") == "Y",
        }
    }))
}

/// Why `address` (`ip:port`) cannot be the media address of outside browsers' calls, or `None` when it can be.
pub fn media_address_problem(address: &str) -> Option<String> {
    let Ok(address) = address.trim().parse::<SocketAddr>() else {
        return Some(format!("{address} is not an address and port"));
    };
    let ip = address.ip();
    let local = match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified(),
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            v6.is_loopback() || v6.is_unspecified() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    };
    local.then(|| {
        format!(
            "{ip} is not a public address, so browsers outside its network cannot reach the edge node's media: set \
             rtc_public_address there"
        )
    })
}

/// The row check of Apply and Test: the edge node's row is there, kept by the link, live (a heartbeat within
/// [`STALE_MS`] of `now_ms`), with the media address of the link's Hello, and draining only when calls are off. With
/// calls on, `Calls.js` `liveMediaNodes` puts such a row first, so it takes new calls before this node's own media
/// nodes. What is so, or why not.
pub fn row_check(facade: &EntityFacade, edge: &EdgeInfo, now_ms: i64) -> (bool, String) {
    let on = calls_enabled();
    let row = match media_row(facade, &edge.instance_id) {
        Ok(Some(row)) => row,
        Ok(None) => {
            return (
                false,
                "The edge node has no media node row here: the link writes it as it comes up and every 15 seconds"
                    .to_string(),
            )
        }
        Err(err) => return (false, format!("The edge node's media node row was not read: {err}")),
    };
    let age = (now_ms - row.heartbeat_ms).max(0) / 1000;
    if !row.edge_linked {
        (
            false,
            "The edge node's media node row is not kept by the link".to_string(),
        )
    } else if now_ms - row.heartbeat_ms > STALE_MS {
        (
            false,
            format!("The edge node's media node row is stale ({age} s old): is the link up?"),
        )
    } else if row.media_address != edge.media_address {
        (
            false,
            format!(
                "The edge node's media node row has {} and its Hello {}: the link writes it again within 15 seconds",
                row.media_address, edge.media_address
            ),
        )
    } else if row.draining == on {
        (
            false,
            if on {
                "The edge node's media node row takes no new calls yet: the link writes it again within 15 seconds"
            } else {
                "The edge node's media node row still takes new calls: the link writes it again within 15 seconds"
            }
            .to_string(),
        )
    } else if on {
        (
            true,
            format!(
                "New calls have their media on the edge node at {}, before this node's own media nodes (its row {age} s \
                 old)",
                row.media_address
            ),
        )
    } else {
        (
            true,
            "The edge node's media node takes no new calls; calls on it go on".to_string(),
        )
    }
}

/// Apply's calls lines (plans/Edge-Node.md §5.7, E10 calls): [`CALLS_ENABLED`] in `env_file`, the row written again
/// at once with it (the link writes it every 15 s after that), then a line each for the media address, the TURN
/// secret the link's Hello handed this node, and the row. `edge` is the edge node as its last Hello said.
pub fn apply(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    env_file: &std::path::Path,
    on: bool,
    edge: &EdgeInfo,
    offer: &CallsOffer,
) -> Vec<SetupLine> {
    let mut lines = vec![set_env(
        "calls",
        env_file,
        &[(CALLS_ENABLED, if on { "true" } else { "false" })],
        if on {
            "New calls have their media on the edge node's media node"
        } else {
            "New calls have their media on this node's own media nodes, not the edge node's"
        },
    )];
    if edge.instance_id.is_empty() || edge.media_address.is_empty() {
        lines.push(SetupLine::new(
            "calls",
            false,
            "The link's Hello names no media node of the edge node yet: is the link up?",
        ));
        return lines;
    }
    if let Err(err) = register_row(edge, false) {
        tracing::error!("Edge setup: the edge node's media node row was not written: {err}");
        lines.push(SetupLine::new(
            "calls",
            false,
            format!("The edge node's media node row was not written: {err}"),
        ));
    }
    if !on {
        let (ok, text) = row_check(facade, edge, crate::user::now_millis());
        lines.push(SetupLine::new("calls", ok, text));
        return lines;
    }
    lines.push(match media_address_problem(&edge.media_address) {
        Some(problem) => SetupLine::new("calls", false, problem),
        None => SetupLine::new(
            "calls",
            true,
            format!("Browsers send call media to {}", edge.media_address),
        ),
    });
    if offer.other_nodes {
        lines.push(SetupLine::new(
            "calls",
            false,
            "Other local nodes are linked to the edge node too, so it takes call messages from none of them (one \
             local node per edge node for calls)",
        ));
    }
    lines.push(if edge.turn_urls.is_empty() {
        SetupLine::new(
            "calls",
            false,
            "The edge node runs no TURN (turn_port), so browsers behind strict firewalls may not connect",
        )
    } else if super::media::turn_secret(factory, &edge.instance_id).is_some() {
        SetupLine::new(
            "calls",
            true,
            format!(
                "The edge node's TURN ({}) takes credentials signed with the secret its link handed this node",
                edge.turn_urls.join(", ")
            ),
        )
    } else {
        SetupLine::new(
            "calls",
            false,
            "The edge node's link handed this node no TURN secret, so its TURN refuses this node's browsers: set \
             turn_auth_secret there and restart it",
        )
    });
    let (ok, text) = row_check(facade, edge, crate::user::now_millis());
    lines.push(SetupLine::new("calls", ok, text));
    lines
}

/// The credential `make#IceServers` gives the Test step's probe leg for the edge node's TURN: (the URLs, username,
/// credential), checked to be the edge node's URLs signed with the secret its link handed this node, or why not.
///
/// # Errors
///
/// Returns why the credential is not one the edge node's TURN takes.
pub fn ice_check(
    factory: &ExecutionContextFactory,
    edge: &EdgeInfo,
    row_urls: &[String],
    leg: &str,
) -> Result<(Vec<String>, String, String), String> {
    if super::media::turn_secret(factory, &edge.instance_id).is_none() {
        return Err(
            "The edge node's link handed this node no TURN secret, so make#IceServers signs its URLs with \
                    this node's own, which its TURN refuses"
                .to_string(),
        );
    }
    let parameters = std::collections::BTreeMap::from([
        ("callLegId".to_string(), Value::string(leg)),
        ("turnUrls".to_string(), Value::string(row_urls.join(","))),
        ("mediaNodeId".to_string(), Value::string(&edge.instance_id)),
    ]);
    let out = internal(|| crate::service::sync(MAKE_ICE_SERVERS, &parameters))?;
    let Some(Value::List(servers)) = out.get("iceServers") else {
        return Err("make#IceServers gave no ICE servers for the edge node".to_string());
    };
    let Some(Value::Map(first)) = servers.first() else {
        return Err("make#IceServers gave no ICE servers for the edge node".to_string());
    };
    let urls: Vec<String> = match first.get("urls") {
        Some(Value::List(urls)) => urls.iter().map(to_plain_string).collect(),
        _ => Vec::new(),
    };
    let field = |name: &str| first.get(name).map(to_plain_string).unwrap_or_default();
    let wanted: Vec<&str> = edge.turn_urls.iter().map(|url| url.trim()).collect();
    if urls.iter().map(|url| url.trim()).collect::<Vec<_>>() != wanted {
        return Err(format!(
            "make#IceServers gave {} for the edge node, and its TURN URLs are {}",
            urls.join(", "),
            edge.turn_urls.join(", ")
        ));
    }
    Ok((urls, field("username"), field("credential")))
}

/// The edge node of a link that is up, as its last Hello said, or why there is none.
///
/// # Errors
///
/// Returns why the link names no media node.
pub fn linked_media_node(status: &LinkStatus) -> Result<&EdgeInfo, String> {
    if status.state != super::LinkState::Linked {
        return Err(format!("The link is {}", status.state.as_str()));
    }
    if status.peer.instance_id.is_empty() || status.peer.media_address.is_empty() {
        return Err("The edge node runs no media node (rtc_media_port)".to_string());
    }
    Ok(&status.peer)
}

#[cfg(test)]
#[path = "calls_tests.rs"]
mod tests;
