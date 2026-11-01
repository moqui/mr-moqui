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

//! The edge node's rows of the link (plans/Edge-Node.md §5.1, §5.7): pairing codes (`moqui.edge.EdgePairingCode`) and
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
pub(crate) const ACTIVE: &str = "EdgeClientActive";
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
        client.create_or_update(&facade)?;
        Ok(Paired::Pinned { key_changed })
    })
}

pub(crate) fn save_hello(factory: &ExecutionContextFactory, node: &str, local: &Hello) -> Result<(), Error> {
    let facade = facade(factory)?;
    transaction(&facade, CLIENT, || {
        let mut client = facade.make_value(CLIENT)?;
        client.set("pairedNodeId", Value::string(node));
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
    let facade = facade(factory)?;
    transaction(&facade, CLIENT, || {
        let rows = facade.find(CLIENT)?.order_by("pairedNodeId", false).list()?;
        Ok(rows
            .iter()
            .map(|row| EdgeClientInfo {
                paired_node_id: text(row.get("pairedNodeId")),
                key_fingerprint: text(row.get("keyFingerprint")),
                status_id: text(row.get("statusId")),
                linked: false,
                down: false,
                generation: text(row.get("linkGeneration")).parse().unwrap_or(0),
                last_connect: row.get("lastConnectDate").and_then(moqui_core::user::value_millis),
                remote_address: text(row.get("lastAddress")),
                binary_version: text(row.get("binaryVersion")),
                link_version: text(row.get("linkVersion")).parse().unwrap_or(0),
            })
            .collect())
    })
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
