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

//! The edge node's rows of the link (plans/archive/Edge-Node.md §5.1, §5.7): pairing codes (`moqui.edge.EdgePairingCode`) and
//! paired local nodes (`moqui.edge.EdgeClient`), each keyed by the paired node (decision 1). Each function runs in a
//! transaction of its own (or the caller's), with authorization off: the link's own bookkeeping.

use std::net::SocketAddr;
use std::time::Duration;

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::edge::{EdgeClientInfo, PairingCode};
use moqui_core::transaction::TransactionAttribute;
use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};

use super::proto::Hello;

const CLIENT: &str = "moqui.edge.EdgeClient";
const PAIRING_CODE: &str = "moqui.edge.EdgePairingCode";
pub(crate) const ACTIVE: &str = moqui_core::edge::hosts::ACTIVE;
const REVOKED: &str = "EdgeClientRevoked";
/// One message for every code that does not pair, so a guess learns nothing.
const BAD_CODE: &str = "the pairing code is unknown, used or expired";
/// The pairing code's letters: no 0, O, 1 or I.
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

pub(crate) fn facade(factory: &ExecutionContextFactory) -> Result<moqui_core::entity::EntityFacade, Error> {
    factory
        .entity()
        .ok_or_else(|| Error::new("the entity facade is not installed"))
}

fn text(value: Option<&Value>) -> String {
    value.map(to_plain_string).unwrap_or_default()
}

pub(crate) fn transaction<T>(
    facade: &moqui_core::entity::EntityFacade,
    entity: &str,
    body: impl FnOnce() -> Result<T, Error>,
) -> Result<T, Error> {
    run(facade, entity, TransactionAttribute::UseOrBegin, body)
}

/// [`transaction`] in a transaction of its own, committed when it returns: for a change the caller then acts on in
/// memory, which must not be rolled back by a caller's transaction after that.
pub(crate) fn committed<T>(
    facade: &moqui_core::entity::EntityFacade,
    entity: &str,
    body: impl FnOnce() -> Result<T, Error>,
) -> Result<T, Error> {
    run(facade, entity, TransactionAttribute::ForceNew, body)
}

fn run<T>(
    facade: &moqui_core::entity::EntityFacade,
    entity: &str,
    attribute: TransactionAttribute,
    body: impl FnOnce() -> Result<T, Error>,
) -> Result<T, Error> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let tx = facade.transactions_for(entity)?;
    tx.run(attribute, Some(Duration::from_secs(30)), body)
}

/// The paired node of an active key, with its next generation saved: the up connection's identity.
pub(crate) fn link_up(
    factory: &ExecutionContextFactory,
    fingerprint: &str,
    remote: SocketAddr,
) -> Result<Option<(String, u64)>, Error> {
    let facade = facade(factory)?;
    transaction(&facade, CLIENT, || {
        let rows = facade
            .find(CLIENT)?
            .eq("keyFingerprint", Value::string(fingerprint))
            .eq("statusId", Value::string(ACTIVE))
            .for_update(true)
            .list()?;
        let Some(row) = rows.get(0) else {
            return Ok(None);
        };
        let node = text(row.get("pairedNodeId"));
        let generation = text(row.get("linkGeneration")).parse::<u64>().unwrap_or(0) + 1;
        let mut update = facade.make_value(CLIENT)?;
        update.set("pairedNodeId", Value::string(&node));
        update.set(
            "linkGeneration",
            Value::I64(i64::try_from(generation).unwrap_or(i64::MAX)),
        );
        update.set("lastConnectDate", moqui_core::user::now_value());
        update.set("lastAddress", Value::string(remote.ip().to_string()));
        update.update(&facade)?;
        Ok(Some((node, generation)))
    })
}

/// The canonical form of a pairing code: upper case, without the dashes and spaces it is shown and typed with.
pub(crate) fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(|ch| !matches!(ch, '-' | ' '))
        .map(|ch| ch.to_ascii_uppercase())
        .collect()
}

/// What [`pair_client`] did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Paired {
    /// Pinned; `key_changed` when the node had another key before (its old links must close).
    Pinned { key_changed: bool },
    /// Refused, with the reason to send back.
    Refused(String),
}

/// Use the pairing code and pin `fingerprint` for `node`. A node that is active with another key is refused: its
/// operator revokes it first, so a code cannot take over a linked node.
pub(crate) fn pair_client(
    factory: &ExecutionContextFactory,
    code: &str,
    node: &str,
    fingerprint: &str,
    local: &Hello,
) -> Result<Paired, Error> {
    let facade = facade(factory)?;
    let hash = moqui_core::user::single_use_hash(&normalize_code(code))
        .map_err(|err| Error::new(err.message().to_string()))?;
    transaction(&facade, PAIRING_CODE, || {
        let row = facade
            .find(PAIRING_CODE)?
            .eq("codeHash", Value::string(&hash))
            .for_update(true)
            .one()?;
        let Some(row) = row else {
            return Ok(Paired::Refused(BAD_CODE.to_string()));
        };
        let now = moqui_core::user::now_millis();
        let expired = row
            .get("expireDate")
            .and_then(moqui_core::user::value_millis)
            .is_none_or(|expire| expire <= now);
        if !text(row.get("usedDate")).is_empty() || expired {
            return Ok(Paired::Refused(BAD_CODE.to_string()));
        }
        let existing = facade
            .find(CLIENT)?
            .eq("pairedNodeId", Value::string(node))
            .for_update(true)
            .one()?;
        let old_key = existing.as_ref().map(|row| text(row.get("keyFingerprint")));
        let active = existing.as_ref().is_some_and(|row| text(row.get("statusId")) == ACTIVE);
        let key_changed = old_key.as_deref().is_some_and(|old| old != fingerprint);
        if active && key_changed {
            return Ok(Paired::Refused(format!(
                "local node {node} is already paired with another key; revoke it on the edge node first"
            )));
        }
        let mut used = facade.make_value(PAIRING_CODE)?;
        used.set(
            "pairingCodeId",
            row.get("pairingCodeId").cloned().unwrap_or(Value::Null),
        );
        used.set("usedDate", moqui_core::user::now_value());
        used.set("pairedNodeId", Value::string(node));
        used.update(&facade)?;
        let mut client = facade.make_value(CLIENT)?;
        client.set("pairedNodeId", Value::string(node));
        client.set("keyFingerprint", Value::string(fingerprint));
        client.set("statusId", Value::string(ACTIVE));
        client.set("pairedDate", moqui_core::user::now_value());
        client.set("revokedDate", Value::Null);
        client.set("binaryVersion", Value::string(&local.binary_version));
        client.set("linkVersion", Value::I64(i64::from(local.link_version)));
        client.set("features", Value::string(local.features.join(",")));
        client.set("nodeName", Value::string(&local.node_name));
        // the node that paired is the pairing's first member (plans/Edge-Node2.md §4.4)
        client.set("lastMemberId", Value::string(node));
        client.set("approvedMemberId", Value::Null);
        // a pairing starts with no member keys, takeover or hold of before
        client.set("memberKeys", Value::Null);
        client.set("takeoverFromId", Value::Null);
        client.create_or_update(&facade)?;
        Ok(Paired::Pinned { key_changed })
    })
}

/// What admission reads of a pairing's row (plans/Edge-Node2.md §4.4, §4.5).
#[derive(Debug, Default)]
pub(crate) struct MemberSettings {
    /// The member last linked, else the node that paired.
    pub last_member: String,
    pub hold: bool,
    pub approved: String,
    /// The member keys pinned, by instance id (plans/Edge-Node2.md §4.11).
    pub pins: Vec<(String, String)>,
    /// The node the last takeover displaced, until the operator clears it.
    pub displaced: String,
}

pub(crate) fn member_settings(factory: &ExecutionContextFactory, node: &str) -> Result<MemberSettings, Error> {
    let facade = facade(factory)?;
    transaction(&facade, CLIENT, || {
        let Some(row) = facade.find(CLIENT)?.eq("pairedNodeId", Value::string(node)).one()? else {
            return Ok(MemberSettings::default());
        };
        let last = text(row.get("lastMemberId"));
        Ok(MemberSettings {
            last_member: if last.is_empty() { node.to_string() } else { last },
            hold: text(row.get("holdNewMember")) == "Y",
            approved: text(row.get("approvedMemberId")),
            pins: moqui_core::edge::pool::pinned_keys(&text(row.get("memberKeys"))),
            displaced: text(row.get("takeoverFromId")),
        })
    })
}

/// Note on pairing `node`'s row that new node `to` took it over from `from` (plans/Edge-Node2.md §4.5); an approval
/// for it is used up.
pub(crate) fn note_takeover(factory: &ExecutionContextFactory, node: &str, from: &str, to: &str) -> Result<(), Error> {
    let facade = facade(factory)?;
    transaction(&facade, CLIENT, || {
        let approved = facade
            .find(CLIENT)?
            .eq("pairedNodeId", Value::string(node))
            .one()?
            .map(|row| text(row.get("approvedMemberId")))
            .unwrap_or_default();
        let mut client = facade.make_value(CLIENT)?;
        client.set("pairedNodeId", Value::string(node));
        client.set("takeoverDate", moqui_core::user::now_value());
        // the member last linked with a new key displaces no one: it is not refused as "taken over" after
        if from != to {
            client.set("takeoverFromId", Value::string(from));
        }
        client.set("takeoverToId", Value::string(to));
        if approved == to {
            client.set("approvedMemberId", Value::Null);
        }
        client.update(&facade)?;
        Ok(())
    })
}

/// Claim member `member`'s own key `member_key` (a fingerprint) for its id, before its link is registered
/// (plans/Edge-Node2.md §4.11): read and written in one transaction of its own, committed before the answer, so two
/// first links of one id cannot both pin (`moqui_core::edge::pool::claim_key`). False when another key is pinned and
/// no takeover was decided. `keep` are the ids never evicted past the cap.
/// What [`claim_key`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Claimed {
    /// The key is pinned (or was): `takeover` when the link still takes the pairing over.
    Pinned { takeover: bool },
    /// Another key is pinned for the id.
    Refused,
    /// Another member linked on another edge member since the admission was decided: dial again.
    Changed,
}

/// Claim member `member`'s key for its id on pairing `node`'s row, read for update, in one transaction committed
/// before the link registers (plans/Edge-Node2.md §4.11). In the same transaction, so two edge members of a cluster
/// admitting at once see each other (§4.6): `recheck` reads the other edge members' links again (`None`: another member
/// linked since, `Some(true)`: this member is linked on another one now, so this is not a takeover), and `write_own`
/// writes this edge member's row with the link.
pub(crate) fn claim_key(
    factory: &ExecutionContextFactory,
    node: &str,
    member: &str,
    member_key: &str,
    takeover: bool,
    keep: &[String],
    recheck: impl FnOnce() -> Result<Option<bool>, Error>,
    write_own: impl FnOnce() -> Result<(), Error>,
) -> Result<Claimed, Error> {
    let facade = facade(factory)?;
    committed(&facade, CLIENT, || {
        let Some(row) = facade
            .find(CLIENT)?
            .eq("pairedNodeId", Value::string(node))
            .for_update(true)
            .one()?
        else {
            return Ok(Claimed::Refused);
        };
        let takeover = match recheck()? {
            None => return Ok(Claimed::Changed),
            Some(true) => false,
            Some(false) => takeover,
        };
        let pins = moqui_core::edge::pool::pinned_keys(&text(row.get("memberKeys")));
        match moqui_core::edge::pool::claim_key(&pins, member, member_key, takeover, keep) {
            moqui_core::edge::pool::KeyClaim::Refuse => Ok(Claimed::Refused),
            moqui_core::edge::pool::KeyClaim::Pin(json) => {
                let mut client = facade.make_value(CLIENT)?;
                client.set("pairedNodeId", Value::string(node));
                client.set("memberKeys", Value::string(json));
                client.update(&facade)?;
                write_own()?;
                Ok(Claimed::Pinned { takeover })
            }
        }
    })
}

/// Whether pairing `node` is active, and its pinned member keys: `None` when it has no row.
pub(crate) fn pairing_state(
    factory: &ExecutionContextFactory,
    node: &str,
) -> Result<Option<(bool, Vec<(String, String)>)>, Error> {
    let facade = facade(factory)?;
    transaction(&facade, CLIENT, || {
        let row = facade
            .find(CLIENT)?
            .eq("pairedNodeId", Value::string(node))
            .use_cache(false)
            .one()?;
        Ok(row.map(|row| {
            (
                text(row.get("statusId")) == ACTIVE,
                moqui_core::edge::pool::pinned_keys(&text(row.get("memberKeys"))),
            )
        }))
    })
}

/// Keep what member `member`'s Hello said on its pairing's row, and that it linked last (its key was claimed before,
/// [`claim_key`]).
pub(crate) fn save_hello(
    factory: &ExecutionContextFactory,
    node: &str,
    member: &str,
    local: &Hello,
) -> Result<(), Error> {
    let facade = facade(factory)?;
    transaction(&facade, CLIENT, || {
        let mut client = facade.make_value(CLIENT)?;
        client.set("pairedNodeId", Value::string(node));
        client.set("lastMemberId", Value::string(member));
        client.set("binaryVersion", Value::string(&local.binary_version));
        client.set("linkVersion", Value::I64(i64::from(local.link_version)));
        client.set("features", Value::string(local.features.join(",")));
        client.set("nodeName", Value::string(&local.node_name));
        client.update(&facade)?;
        Ok(())
    })
}

pub(crate) fn revoke_client(factory: &ExecutionContextFactory, node: &str) -> Result<bool, Error> {
    let facade = facade(factory)?;
    transaction(&facade, CLIENT, || {
        let Some(row) = facade.find(CLIENT)?.eq("pairedNodeId", Value::string(node)).one()? else {
            return Ok(false);
        };
        if text(row.get("statusId")) != REVOKED {
            let mut client = facade.make_value(CLIENT)?;
            client.set("pairedNodeId", Value::string(node));
            client.set("statusId", Value::string(REVOKED));
            client.set("revokedDate", moqui_core::user::now_value());
            client.update(&facade)?;
        }
        Ok(true)
    })
}

pub(crate) fn list_clients(factory: &ExecutionContextFactory) -> Result<Vec<EdgeClientInfo>, Error> {
    moqui_core::edge::hosts::clients(&facade(factory)?)
}

/// Keep what a node's Configure set besides its host names (a JSON object, checked by the caller).
pub(crate) fn save_setup(factory: &ExecutionContextFactory, node: &str, setup_json: &str) -> Result<(), Error> {
    let facade = facade(factory)?;
    committed(&facade, CLIENT, || {
        let active = facade
            .find(CLIENT)?
            .eq("pairedNodeId", Value::string(node))
            .one()?
            .is_some_and(|row| text(row.get("statusId")) == ACTIVE);
        if !active {
            return Err(Error::new(format!(
                "local node {node} is not paired with this edge node, or was revoked"
            )));
        }
        let mut client = facade.make_value(CLIENT)?;
        client.set("pairedNodeId", Value::string(node));
        client.set("setupJson", Value::string(setup_json));
        client.set("setupDate", moqui_core::user::now_value());
        client.update(&facade)?;
        Ok(())
    })
}

/// The kept setup of each active pairing (`moqui_core::edge::claims::setups`), in id order: what the host table keeps
/// of each pairing's chat bridges and PASS sites.
pub(crate) fn setups(factory: &ExecutionContextFactory) -> Result<Vec<(String, serde_json::Value)>, Error> {
    let facade = facade(factory)?;
    transaction(&facade, CLIENT, || moqui_core::edge::claims::setups(&facade))
}

/// A new pairing code: shown once, kept as a hash.
pub(crate) fn create_code(factory: &ExecutionContextFactory, valid_minutes: u32) -> Result<PairingCode, Error> {
    let mut random = [0_u8; 12];
    rustls::crypto::aws_lc_rs::default_provider()
        .secure_random
        .fill(&mut random)
        .map_err(|_| Error::new("could not read random bytes for a pairing code"))?;
    let groups: Vec<String> = random
        .chunks(4)
        .map(|group| {
            group
                .iter()
                .map(|byte| char::from(CODE_ALPHABET[usize::from(*byte) % CODE_ALPHABET.len()]))
                .collect()
        })
        .collect();
    let code = groups.join("-");
    let hash = moqui_core::user::single_use_hash(&normalize_code(&code))
        .map_err(|err| Error::new(err.message().to_string()))?;
    let expire = moqui_core::user::now_millis() + i64::from(valid_minutes) * 60_000;
    let user = moqui_core::user::session::current_user_id().ok().flatten();
    let facade = facade(factory)?;
    let id = transaction(&facade, PAIRING_CODE, || {
        let mut row = facade.make_value(PAIRING_CODE)?;
        row.set_sequenced_id_primary(&facade)?;
        row.set("codeHash", Value::string(&hash));
        row.set("createdDate", moqui_core::user::now_value());
        row.set("createdByUserId", user.map_or(Value::Null, Value::string));
        row.set("expireDate", moqui_core::user::stamp_value(expire));
        row.create(&facade)?;
        Ok(text(row.get("pairingCodeId")))
    })?;
    Ok(PairingCode {
        pairing_code_id: id,
        code,
        expire_date: expire,
    })
}
