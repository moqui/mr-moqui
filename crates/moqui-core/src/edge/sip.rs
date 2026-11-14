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

//! The phones area of the wizard on a local node (plans/archive/Edge-Node.md §5.7, "As built (E10, phones)"), on E8's SIP on
//! the edge node (§5.4 "Voice (SIP)"): the local setting that turns it off ([`SIP_ENABLED`]), the replicated sets it
//! holds back while off ([`withheld`]), Apply's lines ([`apply`]) and the Test step's checks of the edge node's
//! `SipNode` row, its engine's trunks and its copy of the fallbacks ([`row_check`], [`trunk_check`],
//! [`fallback_check`]).
//!
//! **Phones off.** E8 replicates the trunks (`voiceTrunks`) and each number's fallback (`voiceFallbacks`) to the
//! edge node, whose engine registers the trunks from its copy. With `edge_sip_enabled=false` in `runtime/.env` the
//! link sends both sets empty (a full send with no records, so the edge node's copy is emptied and its engine loads
//! again with no trunks and answers no number), and `Voice.js` `voiceNodes` and `SipDevices.js` `edgeSipNode` leave
//! out the edge node's link-kept `SipNode` row, which stays: no new call goes to the edge node's engine, and its
//! registrar signs no phone in, no phone rings through a binding there (`SipDevices.js` `liveSipNodes`), and Apply
//! removes its bindings ([`remove_edge_bindings`]). Calls already on it go on.
//!
//! **Several pairings** (plans/Edge-Node2.md §4.1, E16). The edge node's engine loads every pairing's trunks and reads
//! each one's copies. A number's owner is the claim of [`super::claims`] (`PhoneNumber` in `voiceTrunks`, so a number
//! with no fallback row is found too, or `PhoneNumberFallback` in `voiceFallbacks`; a number another owner holds is
//! held back as a replica conflict); here are each pairing's `sipDomain` ([`pairing_domains`]) and the realm a
//! pairing's phones register in ([`pairing_realm`]).

use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};

use super::replica::SetStatus;
use super::setup::{set_env, SetupLine, SipOffer};
use super::EdgeInfo;
use crate::context::ExecutionContext;
use crate::entity::EntityFacade;

/// The local setting (`runtime/.env`) that turns phones through the edge node off: on unless it is `false`, `n`,
/// `no`, `0` or `off`. Apply writes it; unpairing removes it.
pub const SIP_ENABLED: &str = "edge_sip_enabled";
/// The replicated sets of E8 (MoquiDefaultConf.xml `<edge>`): the trunks and the policy, and each number's fallback.
pub const TRUNKS_SET: &str = "voiceTrunks";
pub const FALLBACKS_SET: &str = "voiceFallbacks";
/// A `SipNode` row is live while its heartbeat is this recent (`NODE_STALE_MS` in `Voice.js`).
pub const STALE_MS: i64 = 60_000;

const SIP_NODE: &str = "moqui.call.SipNode";
const TRUNK: &str = "moqui.call.VoiceTrunk";
const NUMBER: &str = "moqui.call.PhoneNumber";
const FALLBACK: &str = "moqui.call.PhoneNumberFallback";
const DEVICE: &str = "moqui.call.SipDevice";
const REGISTRATION: &str = "moqui.call.SipRegistration";
const EDGE_CLIENT: &str = "moqui.edge.EdgeClient";

/// Whether phones go through the edge node ([`SIP_ENABLED`]).
pub fn sip_enabled() -> bool {
    !matches!(
        moqui_lib::env::get_prop_or_env(SIP_ENABLED)
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "false" | "n" | "no" | "0" | "off"
    )
}

/// Whether the replicated set `set_name` goes to the edge node empty now: the trunks and the fallbacks while phones are
/// off. The link's sender sends such a set whole again when this changes.
pub fn withheld(set_name: &str) -> bool {
    (set_name == TRUNKS_SET || set_name == FALLBACKS_SET) && !sip_enabled()
}

fn text(row: &crate::entity::EntityValue, field: &str) -> String {
    row.get(field).map(to_plain_string).unwrap_or_default()
}

/// The edge node's `SipNode` row, as the checks read it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SipRow {
    pub sip_address: String,
    pub tls_address: String,
    pub sip_domain: String,
    pub registrar: bool,
    /// When its heartbeat was last written (epoch milliseconds).
    pub heartbeat_ms: i64,
    pub edge_linked: bool,
}

/// The `SipNode` row of `instance_id`, read past the cache.
///
/// # Errors
///
/// Returns the entity facade's error.
pub fn sip_row(facade: &EntityFacade, instance_id: &str) -> Result<Option<SipRow>, Error> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let row = facade
        .find(SIP_NODE)?
        .eq("instanceId", Value::string(instance_id))
        .use_cache(false)
        .one()?;
    Ok(row.map(|row| SipRow {
        sip_address: text(&row, "sipAddress"),
        tls_address: text(&row, "tlsAddress"),
        sip_domain: text(&row, "sipDomain"),
        registrar: text(&row, "registrarEnabled") == "Y",
        heartbeat_ms: row
            .get("heartbeatDate")
            .and_then(crate::user::value_millis)
            .unwrap_or(0),
        edge_linked: text(&row, "edgeLinked") == "Y",
    }))
}

/// The row check of Apply and Test: the edge node's `SipNode` row is there, kept by the link, live (a heartbeat
/// within [`STALE_MS`] of `now_ms`), with the addresses of the link's Hello. With phones on, `Voice.js` `voiceNodes`
/// then sends the phone calls of its trunks and phones to it. What is so, or why not.
pub fn row_check(row: Option<&SipRow>, edge: &EdgeInfo, now_ms: i64) -> (bool, String) {
    let Some(row) = row else {
        return (
            false,
            "The edge node has no SIP node row here: the link writes it as it comes up and every 15 seconds"
                .to_string(),
        );
    };
    let age = (now_ms - row.heartbeat_ms).max(0) / 1000;
    if !row.edge_linked {
        (
            false,
            "The edge node's SIP node row is not kept by the link".to_string(),
        )
    } else if now_ms - row.heartbeat_ms > STALE_MS {
        (
            false,
            format!("The edge node's SIP node row is stale ({age} s old): is the link up?"),
        )
    } else if row.sip_address != edge.sip_address || row.tls_address != edge.sip_tls_address {
        (
            false,
            "The edge node's SIP node row does not have its Hello's addresses: the link writes it again within 15 \
             seconds"
                .to_string(),
        )
    } else if !sip_enabled() {
        (
            true,
            "The edge node's SIP node row stays but takes no new call; calls on it go on".to_string(),
        )
    } else {
        let addresses: Vec<&str> = [row.sip_address.as_str(), row.tls_address.as_str()]
            .into_iter()
            .filter(|address| !address.is_empty())
            .collect();
        (
            true,
            format!(
                "Phone calls of this node's trunks and phones go to the edge node's SIP engine at {} (its row {age} s \
                 old)",
                addresses.join(", ")
            ),
        )
    }
}

/// This node's trunks that are not disabled, as (id, name): those the edge node's engine registers from its copy.
///
/// # Errors
///
/// Returns the entity facade's error.
pub fn local_trunks(facade: &EntityFacade) -> Result<Vec<(String, String)>, Error> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let rows = facade
        .find(TRUNK)?
        .use_cache(false)
        .order_by("voiceTrunkId", false)
        .list()?;
    Ok(rows
        .iter()
        .filter(|row| text(row, "disabled") != "Y")
        .map(|row| {
            let id = text(row, "voiceTrunkId");
            let name = Some(text(row, "name"))
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| id.clone());
            (id, name)
        })
        .collect())
}

/// The numbers on this node's enabled trunks, and how many of them have a fallback row (`PhoneNumberFallback`): the
/// others are refused (480) while this node is away.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    pub numbers: usize,
    pub covered: usize,
    /// The fallback rows, all numbers counted: what the edge node's copy should hold.
    pub fallbacks: usize,
}

/// The [`Coverage`] of this node's numbers.
///
/// # Errors
///
/// Returns the entity facade's error.
pub fn coverage(facade: &EntityFacade) -> Result<Coverage, Error> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let trunks: Vec<String> = local_trunks(facade)?.into_iter().map(|(id, _)| id).collect();
    let fallbacks: std::collections::BTreeSet<String> = facade
        .find(FALLBACK)?
        .use_cache(false)
        .select_field("phoneNumber")
        .list()?
        .iter()
        .map(|row| text(row, "phoneNumber"))
        .collect();
    let numbers: Vec<String> = facade
        .find(NUMBER)?
        .use_cache(false)
        .list()?
        .iter()
        .filter(|row| trunks.contains(&text(row, "voiceTrunkId")))
        .map(|row| text(row, "phoneNumber"))
        .collect();
    Ok(Coverage {
        numbers: numbers.len(),
        covered: numbers.iter().filter(|number| fallbacks.contains(*number)).count(),
        fallbacks: fallbacks.len(),
    })
}

/// How many active phones of this node were made with verifiers for another realm than `realm`: they cannot register
/// to the edge node's registrar until their password is set again.
///
/// # Errors
///
/// Returns the entity facade's error.
pub fn other_realm_devices(facade: &EntityFacade, realm: &str) -> Result<usize, Error> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let rows = facade
        .find(DEVICE)?
        .eq("statusId", Value::string("SdsActive"))
        .use_cache(false)
        .list()?;
    Ok(rows.iter().filter(|row| text(row, "realm") != realm).count())
}

/// The Test step's trunks check: every one of this node's enabled trunks (`local`, as (id, name)) is loaded by the
/// edge node's engine (the offer's), and none failed. `None` when there is nothing to check.
pub fn trunk_check(local: &[(String, String)], offer: &SipOffer) -> (Option<bool>, String) {
    if local.is_empty() {
        return (None, "Not checked: this node has no enabled trunk".to_string());
    }
    let missing: Vec<&str> = local
        .iter()
        .filter(|(id, _)| !offer.trunks.iter().any(|trunk| trunk.voice_trunk_id == *id))
        .map(|(_, name)| name.as_str())
        .collect();
    if !missing.is_empty() {
        return (
            Some(false),
            format!(
                "The edge node's SIP engine has not loaded {} yet: the replicated set {TRUNKS_SET} brings it within \
                 seconds while the link is up",
                missing.join(", ")
            ),
        );
    }
    let failed: Vec<String> = offer
        .trunks
        .iter()
        .filter(|trunk| trunk.state == "failed")
        .map(|trunk| {
            let name = if trunk.name.is_empty() {
                &trunk.voice_trunk_id
            } else {
                &trunk.name
            };
            format!("{name} ({})", trunk.error)
        })
        .collect();
    if !failed.is_empty() {
        return (Some(false), format!("Failed on the edge node: {}", failed.join(", ")));
    }
    let ours: Vec<&super::media::SipTrunkState> = offer
        .trunks
        .iter()
        .filter(|trunk| local.iter().any(|(id, _)| *id == trunk.voice_trunk_id))
        .collect();
    let named = |trunk: &super::media::SipTrunkState| {
        let name = if trunk.name.is_empty() {
            &trunk.voice_trunk_id
        } else {
            &trunk.name
        };
        format!("{name} ({})", trunk.state)
    };
    // loaded and not answered yet (or let go) is not confirmed: only a registration or a ping's answer is
    let waiting: Vec<String> = ours
        .iter()
        .filter(|trunk| trunk.state != "registered" && trunk.state != "ok")
        .map(|trunk| named(trunk))
        .collect();
    if !waiting.is_empty() {
        return (
            Some(false),
            format!(
                "Loaded on the edge node, but not answering yet: {}. A trunk answers its registration, or its first \
                 ping, within seconds: test again",
                waiting.join(", ")
            ),
        );
    }
    let states: Vec<String> = ours.iter().map(|trunk| named(trunk)).collect();
    (
        Some(true),
        format!(
            "The edge node's SIP engine has this node's trunks: {}",
            states.join(", ")
        ),
    )
}

/// The Test step's fallbacks check: the edge node's copy holds as many fallbacks as this node has (`local`), with
/// nothing of the set waiting to go (`set`, its replication state here).
pub fn fallback_check(local: usize, offer: &SipOffer, set: Option<&SetStatus>) -> (bool, String) {
    let Some(set) = set else {
        return (
            false,
            format!("This node does not replicate the set {FALLBACKS_SET}: is it paired?"),
        );
    };
    if set.snapshot_needed || set.waiting > 0 {
        return (
            false,
            format!(
                "The set {FALLBACKS_SET} is on its way to the edge node ({} changes waiting{}): test again in a few \
                 seconds",
                set.waiting,
                if set.snapshot_needed { ", a full send due" } else { "" }
            ),
        );
    }
    let held = usize::try_from(offer.fallbacks).unwrap_or(usize::MAX);
    if held != local {
        return (
            false,
            format!("The edge node holds {held} numbers' fallbacks and this node has {local}"),
        );
    }
    (
        true,
        if local == 0 {
            "No number has a fallback, so while this node is away every call to its numbers is refused (480)"
                .to_string()
        } else {
            format!("The edge node holds the fallback of each of the {local} numbers that have one")
        },
    )
}

/// A replicated set's line: its cursor (the epoch's start and the last batch acknowledged) and the changes waiting.
fn set_line(set: Option<&SetStatus>, name: &str, on: bool) -> SetupLine {
    let Some(set) = set else {
        return SetupLine::new(
            "sip",
            false,
            format!("This node does not replicate the set {name}: is it paired?"),
        );
    };
    let cursor = if set.epoch.is_empty() {
        "no cursor yet".to_string()
    } else {
        format!(
            "cursor {} at batch {}",
            set.epoch.chars().take(8).collect::<String>(),
            set.acked
        )
    };
    SetupLine::new(
        "sip",
        true,
        format!(
            "The set {name} goes to the edge node {} ({cursor}, {} changes waiting)",
            if on { "whole again" } else { "empty" },
            set.waiting
        ),
    )
}

/// Apply's phones lines (plans/archive/Edge-Node.md §5.7, E10 phones): [`SIP_ENABLED`] in `env_file`, the two replicated
/// sets sent whole again (empty while off), then a line each for the sets' replication, the row, the phones' realm and
/// the registrar, and the numbers' fallbacks. `edge` is the edge node as its last Hello said. Nothing goes to the
/// edge node.
pub fn apply(
    facade: &EntityFacade,
    env_file: &std::path::Path,
    on: bool,
    edge: &EdgeInfo,
    offer: &SipOffer,
) -> Vec<SetupLine> {
    let mut lines = vec![set_env(
        "sip",
        env_file,
        &[(SIP_ENABLED, if on { "true" } else { "false" })],
        if on {
            "This node's trunks and phones go through the edge node's SIP engine"
        } else {
            "This node's trunks and fallbacks are not sent to the edge node, and its SIP engine takes no new call"
        },
    )];
    // what holds now: a property set at start overrides the file (the line above says so), and the rest follows it
    let on = sip_enabled();
    let replica = super::replica::replica(facade);
    let status = replica.as_ref().map(|replica| replica.status()).unwrap_or_default();
    if let Some(replica) = &replica {
        for (index, set) in replica.sets().iter().enumerate() {
            if set.name == TRUNKS_SET || set.name == FALLBACKS_SET {
                replica.resync(index);
            }
        }
    }
    for name in [TRUNKS_SET, FALLBACKS_SET] {
        lines.push(set_line(status.iter().find(|set| set.name == name), name, on));
    }
    let row = sip_row(facade, &edge.instance_id);
    let now = crate::user::now_millis();
    let (ok, text) = match &row {
        Ok(row) => row_check(row.as_ref(), edge, now),
        Err(err) => (false, format!("The edge node's SIP node row was not read: {err}")),
    };
    lines.push(SetupLine::new("sip", ok, text));
    if !on {
        // the bindings of phones registered at the edge node's registrar go: none of them rings from here now
        lines.push(match remove_edge_bindings(facade, &edge.instance_id) {
            Ok(0) => SetupLine::new("sip", true, "No phone is registered at the edge node's registrar"),
            Ok(count) => SetupLine::new(
                "sip",
                true,
                format!("The bindings of {count} phones registered at the edge node's registrar are removed"),
            ),
            Err(err) => SetupLine::new(
                "sip",
                false,
                format!("The bindings at the edge node's registrar were not removed: {err}"),
            ),
        });
        return lines;
    }
    if offer.registrar {
        lines.push(SetupLine::new(
            "sip",
            true,
            format!(
                "Phones made here get verifiers for the edge node's realm {}; outside phones register to its \
                 registrar only while this node is reachable",
                offer.domain
            ),
        ));
        match other_realm_devices(facade, &offer.domain) {
            Ok(0) => {}
            Ok(count) => lines.push(SetupLine::new(
                "sip",
                false,
                format!(
                    "{count} phones were made for another realm, so they cannot register to the edge node: set their \
                     passwords again"
                ),
            )),
            Err(err) => lines.push(SetupLine::new(
                "sip",
                false,
                format!("This node's phones were not read: {err}"),
            )),
        }
    } else {
        lines.push(SetupLine::new(
            "sip",
            true,
            "The edge node's registrar is off (sip_registrar_enabled there), so outside phones do not register to it",
        ));
    }
    lines.push(match coverage(facade) {
        Ok(Coverage { numbers: 0, .. }) => SetupLine::new("sip", true, "This node has no numbers on its trunks yet"),
        Ok(coverage) if coverage.covered == coverage.numbers => SetupLine::new(
            "sip",
            true,
            format!(
                "Each of the {} numbers has a fallback for while this node is away",
                coverage.numbers
            ),
        ),
        Ok(coverage) => SetupLine::new(
            "sip",
            true,
            format!(
                "{} of {} numbers have a fallback for while this node is away; calls to the others are refused (480) \
                 then",
                coverage.covered, coverage.numbers
            ),
        ),
        Err(err) => SetupLine::new("sip", false, format!("This node's numbers were not read: {err}")),
    });
    lines
}

/// Remove the `SipRegistration` rows of the edge node `instance_id` (its registrar's bindings), as phones are turned
/// off: how many.
///
/// # Errors
///
/// Returns the entity facade's error.
pub fn remove_edge_bindings(facade: &EntityFacade, instance_id: &str) -> Result<i64, Error> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    if instance_id.is_empty() {
        return Ok(0);
    }
    facade
        .find(REGISTRATION)?
        .eq("instanceId", Value::string(instance_id))
        .disable_authz()
        .delete_all()
}

/// The SIP device domain of a kept Configure (`EdgeClient.setupJson`'s `sipDomain`), lower case with no trailing dot.
pub fn setup_domain(setup: &str) -> String {
    let setup: serde_json::Value = serde_json::from_str(setup).unwrap_or_default();
    setup["sipDomain"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

/// On a node that takes links, each active pairing (`EdgeClientActive`) in id order, with the SIP device domain of
/// its last Configure ([`setup_domain`]; empty when it named none).
///
/// # Errors
///
/// Returns the entity facade's error.
pub fn pairing_domains(facade: &EntityFacade) -> Result<Vec<(String, String)>, Error> {
    let rows = facade
        .find(EDGE_CLIENT)?
        .eq("statusId", Value::string("EdgeClientActive"))
        .select_field("pairedNodeId")
        .select_field("setupJson")
        .order_by("pairedNodeId", false)
        .disable_authz()
        .list()?;
    Ok(rows
        .values()
        .iter()
        .map(|row| (text(row, "pairedNodeId"), setup_domain(&text(row, "setupJson"))))
        .filter(|(node, _)| !node.is_empty())
        .collect())
}

/// The realm the phones of the pairing `node` register in on this edge node, and whether its registrar takes them,
/// as its Hello and offer tell that pairing (E16): the `sipDomain` of its last Configure when it named one; else, on
/// a dedicated edge node, the registrar's own realm (`sip_domain`), and on an all-in-one node none, as the server's
/// own realm is the server's. The domain is the one the SIP engine's registrar takes now
/// ([`super::media::applied_sip_domain`]), or with `kept` the one kept from the last Configure, which the engine
/// takes within seconds. `None` while this node runs no SIP engine. A read that fails is logged and taken as no
/// domain.
pub fn pairing_realm(
    factory: &crate::context::ExecutionContextFactory,
    node: &str,
    kept: bool,
) -> Option<(String, bool)> {
    let advert = super::media::sip_advert(factory)?;
    let applied = (!kept)
        .then(|| super::media::applied_sip_domain(factory, node))
        .flatten();
    if let Some(domain) = applied {
        return Some(realm_of(&domain, super::settings(factory).all_in_one(), &advert));
    }
    let domain = match factory.entity() {
        Some(facade) => facade
            .find(EDGE_CLIENT)
            .and_then(|find| {
                find.eq("pairedNodeId", Value::string(node))
                    .select_field("setupJson")
                    .disable_authz()
                    .one()
            })
            .map(|row| {
                row.map(|row| setup_domain(&text(&row, "setupJson")))
                    .unwrap_or_default()
            })
            .unwrap_or_else(|err| {
                tracing::error!("SIP: the domain of local node {node}'s phones was not read: {err}");
                String::new()
            }),
        None => String::new(),
    };
    Some(realm_of(&domain, super::settings(factory).all_in_one(), &advert))
}

/// [`pairing_realm`] from the pairing's domain (`domain`), the node's role and its SIP engine's advert.
pub fn realm_of(domain: &str, all_in_one: bool, advert: &super::media::SipAdvert) -> (String, bool) {
    if !domain.is_empty() {
        (domain.to_string(), advert.registrar)
    } else if all_in_one {
        (String::new(), false)
    } else {
        (advert.domain.clone(), advert.registrar)
    }
}

/// The edge node of a link that is up, as its last Hello said, when it runs a SIP engine, or why there is none.
///
/// # Errors
///
/// Returns why the link names no SIP engine.
pub fn linked_sip_node(status: &super::LinkStatus) -> Result<&EdgeInfo, String> {
    if status.state != super::LinkState::Linked {
        return Err(format!("The link is {}", status.state.as_str()));
    }
    if status.peer.instance_id.is_empty()
        || (status.peer.sip_address.is_empty() && status.peer.sip_tls_address.is_empty())
    {
        return Err("The edge node runs no SIP engine (sip_port, sip_tls_port)".to_string());
    }
    Ok(&status.peer)
}

/// The lock row (`moqui.edge.EdgeSenderLock`) whose holder registers the trunks, on a node that takes links
/// (plans/Edge-Node2.md §4.6): of the members of an edge cluster, each with a SIP engine that loads every trunk, only
/// the holder registers them, so a provider's calls go to one contact.
pub const TRUNK_LOCK: &str = "sipTrunks";

/// Claim or renew the trunk registration lock for `member` for `lease`: true when it holds it now (it held it, or it
/// was free or its lease lapsed by the database's clock), false while another member's lease lasts.
///
/// # Errors
///
/// When the lock row cannot be read or written.
pub fn claim_trunk_lock(facade: &EntityFacade, member: &str, lease: std::time::Duration) -> Result<bool, Error> {
    super::replica::sender::claim(facade, TRUNK_LOCK, member, lease).map(|claim| claim.holds())
}

/// Let the trunk registration lock go when `member` holds it, as its engine stops. True when it did.
///
/// # Errors
///
/// When the lock row cannot be written.
pub fn release_trunk_lock(facade: &EntityFacade, member: &str) -> Result<bool, Error> {
    super::replica::sender::release(facade, TRUNK_LOCK, member)
}

#[cfg(test)]
#[path = "sip_tests.rs"]
mod tests;
