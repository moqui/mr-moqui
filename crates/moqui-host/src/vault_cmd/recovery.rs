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

//! `moqui vault recovery ...` (plans/Vault-Instances.md §3.10; docs/security/Vaults.md, Recovery): the steps of an
//! organization's recovery holder, and a manager's turning recovery on, in the vault crate's own session on this
//! machine. The shares, the combined recovery key and the vault key stay in this process.
//!
//! - `list`: the recovery keys, and the open recoveries this user may seal a share to.
//! - `make --k K [--group G] <username>...`: make a recovery key split K of the holders named (each key checked).
//! - `on <vaultId> [--key ID]`: as a manager, list the recovery key in the vault's member list and wrap its key to it.
//! - `start <vaultId> <username> [--key ID]`: as a holder, recover the vault to the member's new key (checked): start
//!   the session, wait for K − 1 other holders' shares, combine them here and complete.
//! - `share <recoverySessionId>`: as another holder, see what the session recovers and seal this holder's share to it.

use std::time::{Duration, Instant};

use moqui_lib::error::Error;
use serde_json::{json, Value};

use super::client::VaultClient;
use super::remote::Remote;
use super::{ask, new_id, open_key, segment, sign_in, text, vault_err};

/// How long `start` waits for the other holders' shares.
const WAIT: Duration = Duration::from_secs(30 * 60);
const POLL: Duration = Duration::from_secs(5);

/// `moqui vault recovery <words>`.
pub(super) fn dispatch(remote: &mut Remote, client: &mut VaultClient, user: &str, words: &[&str]) -> Result<(), Error> {
    let mut positional = Vec::new();
    let mut k = 0_u64;
    let mut group = "";
    let mut key = "";
    let mut rest = words.iter();
    while let Some(word) = rest.next() {
        let mut value = |name: &str| {
            rest.next()
                .copied()
                .ok_or_else(|| Error::new(format!("{name} needs a value")))
        };
        match *word {
            "--k" => k = value("--k")?.parse().map_err(|_| Error::new("--k is a number"))?,
            "--group" => group = value("--group")?,
            "--key" => key = value("--key")?,
            other => positional.push(other),
        }
    }
    match positional.as_slice() {
        ["list"] => list(remote),
        ["make", holders @ ..] if k > 0 && !holders.is_empty() => make(remote, client, k, group, holders),
        ["on", vault_id] => turn_on(remote, client, vault_id, key),
        ["start", vault_id, username] => start(remote, client, user, vault_id, username, key),
        ["share", session_id] => share(remote, client, session_id),
        _ => Err(Error::new(super::USAGE)),
    }
}

/// The active recovery keys, as `list#VaultRecoveryKeys` gives them.
fn recovery_keys(remote: &mut Remote) -> Result<Vec<Value>, Error> {
    let keys = remote.get("/recoveryKeys", &[])?;
    Ok(keys["recoveryKeyList"].as_array().cloned().unwrap_or_default())
}

/// The recovery key named, or the only one this user holds a share of.
fn chosen_key(remote: &mut Remote, user_id: &str, key: &str) -> Result<Value, Error> {
    let keys = recovery_keys(remote)?;
    let mine: Vec<Value> = keys
        .into_iter()
        .filter(|row| key.is_empty() || text(row, "recoveryKeyId") == key)
        .filter(|row| !key.is_empty() || holder_of(row, user_id).is_some())
        .collect();
    match mine.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(Error::new("No such recovery key")),
        _ => Err(Error::new("More than one recovery key: name one with --key")),
    }
}

fn holder_of(key: &Value, user_id: &str) -> Option<Value> {
    key["holders"]
        .as_array()?
        .iter()
        .find(|holder| text(holder, "userId") == user_id)
        .cloned()
}

fn recovery_keys_arg(key: &Value) -> Value {
    json!([{ "record": key["record"], "maker_public": key["makerPublic"] }])
}

fn checks(remote: &mut Remote) -> Result<Value, Error> {
    Ok(remote.get("/keyChecks", &[])?["keyCheckList"].clone())
}

/// `list`: the recovery keys and the open recoveries.
fn list(remote: &mut Remote) -> Result<(), Error> {
    for key in recovery_keys(remote)? {
        let holders: Vec<String> = key["holders"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|holder| text(holder, "userId"))
            .collect();
        println!(
            "key  {}  {} of {}  holders {}",
            text(&key, "recoveryKeyId"),
            text(&key["record"], "k"),
            text(&key["record"], "n"),
            holders.join(", ")
        );
    }
    let Ok(sessions) = remote.get("/recoveries", &[]) else {
        return Ok(());
    };
    for session in sessions["sessionList"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        println!(
            "recovery  {}  vault {}  for {}  by {}  new key {}  shares from {}",
            text(session, "recoverySessionId"),
            text(&session["session"], "vault_id"),
            text(&session["session"], "subject_user_id"),
            text(&session["session"], "holder_user_id"),
            text(session, "subjectFingerprint"),
            session["deliveredBy"]
        );
    }
    Ok(())
}

/// `make`: a recovery key split `k` of the holders named, each holder's key checked by this user.
fn make(remote: &mut Remote, client: &mut VaultClient, k: u64, group: &str, holders: &[&str]) -> Result<(), Error> {
    open_key(remote, client)?;
    let mut rows = Vec::with_capacity(holders.len());
    for username in holders {
        let key = remote.get("/publicKey", &[("username", username)])?;
        if key["hasKey"] != json!(true) {
            return Err(Error::new(format!("{username} has no personal vault key")));
        }
        rows.push(json!({ "user_id": key["userId"], "key_id": key["personalKeyId"], "public": key["public"] }));
    }
    let recovery_key_id = new_id("rk");
    let made = client
        .call(
            "recovery_make",
            json!({ "recovery_key_id": recovery_key_id, "k": k, "holders": rows, "key_checks": checks(remote)? }),
        )
        .map_err(vault_err)?;
    remote.post(
        "/recoveryKeys",
        &json!({ "record": made["record"], "shares": made["shares"], "userGroupId": group }),
    )?;
    println!(
        "Recovery key {recovery_key_id}: {k} of {} holders' shares recover a vault.",
        rows.len()
    );
    Ok(())
}

/// `on`: as a manager, the recovery key listed in the vault's member list (its maker's key checked) and the vault key
/// wrapped to it.
fn turn_on(remote: &mut Remote, client: &mut VaultClient, vault_id: &str, key: &str) -> Result<(), Error> {
    open_key(remote, client)?;
    let keys = recovery_keys(remote)?;
    let chosen = match key {
        "" => match keys.as_slice() {
            [one] => one.clone(),
            _ => return Err(Error::new("Name the recovery key with --key")),
        },
        _ => keys
            .into_iter()
            .find(|row| text(row, "recoveryKeyId") == key)
            .ok_or_else(|| Error::new("No such recovery key"))?,
    };
    let vault = remote.get(&format!("/vaults/{}", segment(vault_id)), &[])?;
    let key_checks = checks(remote)?;
    let version = vault["vault"]["keyVersion"].as_u64().unwrap_or(0);
    let wrap = vault["wraps"]
        .as_array()
        .and_then(|wraps| wraps.iter().find(|wrap| wrap["key_version"].as_u64() == Some(version)))
        .cloned()
        .ok_or_else(|| Error::new("You hold no wrap of this vault's current key version"))?;
    let path = super::state::state_file()?;
    let mut state = super::state::State::read(&path);
    let opened = client
        .call(
            "vault_key_open",
            json!({ "entries": vault["entries"], "key_checks": key_checks,
                    "remembered": state.remembered(remote.base(), vault_id), "wrap": wrap }),
        )
        .map_err(vault_err)?;
    let recovery = recovery_keys_arg(&chosen);
    let member = client
        .call(
            "recovery_member",
            json!({ "record": chosen["record"], "maker_public": chosen["makerPublic"], "key_checks": key_checks }),
        )
        .map_err(|err| {
            Error::new(format!(
                "{err}: check the key of {}, who made the recovery key, first (moqui vault key check)",
                text(&chosen["record"], "maker_user_id")
            ))
        })?;
    let entries = vault["entries"].as_array().cloned().unwrap_or_default();
    let latest = entries.last().cloned().unwrap_or(Value::Null);
    let mut members = latest["members"].as_array().cloned().unwrap_or_default();
    members.push(member.clone());
    let entry = client
        .call(
            "key_version_next",
            json!({ "entries": entries, "members": members, "version": version,
                    "revision": latest["revision"].as_u64().unwrap_or(0) + 1, "key_checks": key_checks,
                    "recovery_keys": recovery }),
        )
        .map_err(vault_err)?;
    let wrap = client
        .call(
            "vault_key_wrap_member",
            json!({ "vault_id": vault_id, "key_version": version, "member_user_id": member["user_id"],
                    "member_key_id": member["key_id"], "member_public": member["public"], "key_checks": key_checks,
                    "recovery_keys": recovery }),
        )
        .map_err(vault_err)?;
    remote.post(
        &format!("/vaults/{}/members", segment(vault_id)),
        &json!({ "expectedRevision": vault["vault"]["revision"], "entry": entry, "wrap": wrap }),
    )?;
    state.remember(remote.base(), vault_id, &opened["remembered"]);
    state.write(&path)?;
    println!(
        "Recovery is on for {vault_id} with recovery key {}.",
        text(&chosen, "recoveryKeyId")
    );
    Ok(())
}

/// A POST that asks for a new sign-in once when the server wants a recent one (a recovery waits for other holders).
fn post_recent(remote: &mut Remote, user: &str, path: &str, body: &Value) -> Result<Value, Error> {
    match remote.post(path, body) {
        Err(err) if err.to_string().contains("reauthRequired") => {
            eprintln!("Your sign-in is no longer recent: sign in again.");
            sign_in(remote, user)?;
            remote.post(path, body)
        }
        other => other,
    }
}

/// `start`: as a holder, recover the vault to the member's current personal key, which this user has checked.
fn start(
    remote: &mut Remote,
    client: &mut VaultClient,
    user: &str,
    vault_id: &str,
    username: &str,
    key: &str,
) -> Result<(), Error> {
    open_key(remote, client)?;
    let me = remote.get("/personalKey", &[])?;
    let chosen = chosen_key(remote, &text(&me, "userId"), key)?;
    let held = holder_of(&chosen, &text(&me, "userId"))
        .map(|holder| holder["held"].clone())
        .ok_or_else(|| Error::new("You hold no share of this recovery key"))?;
    let subject = remote.get("/publicKey", &[("username", username)])?;
    if subject["hasKey"] != json!(true) {
        return Err(Error::new(format!(
            "{username} has no personal vault key: they make a new one first"
        )));
    }
    println!("{username}'s new key fingerprint:\n  {}", text(&subject, "fingerprint"));
    let session_id = new_id("rs");
    let session = client
        .call(
            "recovery_session_start",
            json!({ "session_id": session_id, "record": chosen["record"], "maker_public": chosen["makerPublic"],
                    "held": held, "vault_id": vault_id, "subject_user_id": subject["userId"],
                    "subject_key_id": subject["personalKeyId"], "subject_public": subject["public"],
                    "key_checks": checks(remote)? }),
        )
        .map_err(|err| {
            Error::new(format!(
                "{err} (check {username}'s new key first: moqui vault key check)"
            ))
        })?;
    post_recent(remote, user, "/recoveries", &json!({ "session": session }))?;
    let needed = chosen["record"]["k"].as_u64().unwrap_or(2).saturating_sub(1);
    println!("Recovery {session_id} started: {needed} other holders run  moqui vault recovery share {session_id}");
    println!("Waiting for their shares (keep this running; its key is only in this process)...");
    let path = format!("/recoveries/{}", segment(&session_id));
    let began = Instant::now();
    let open = loop {
        let open = remote.get(&path, &[])?;
        let count = open["deliveries"].as_array().map_or(0, Vec::len) as u64;
        if count >= needed {
            break open;
        }
        if began.elapsed() > WAIT {
            return Err(Error::new(format!(
                "{count} of {needed} shares after 30 minutes: start the recovery again"
            )));
        }
        std::thread::sleep(POLL);
    };
    let key_checks = checks(remote)?;
    client
        .call(
            "recovery_combine",
            json!({ "session_id": session_id, "record": open["record"], "maker_public": open["makerPublic"],
                    "held": open["held"], "deliveries": open["deliveries"], "key_checks": key_checks }),
        )
        .map_err(vault_err)?;
    client
        .call(
            "recovery_open_vault",
            json!({ "entries": open["entries"], "key_checks": key_checks,
                    "recovery_key_id": text(&chosen, "recoveryKeyId"), "wrap": open["recoveryWrap"] }),
        )
        .map_err(vault_err)?;
    let rekeyed = client
        .call(
            "recovery_rekey",
            json!({ "session_id": session_id, "entries": open["entries"], "key_checks": key_checks }),
        )
        .map_err(vault_err)?;
    post_recent(
        remote,
        user,
        &format!("{path}/complete"),
        &json!({ "expectedRevision": open["revision"], "entry": rekeyed["entry"], "wrap": rekeyed["wrap"],
                 "rewraps": rekeyed["rewraps"] }),
    )?;
    println!("Recovered: {username} opens {vault_id} with their new key. Every member was told.");
    Ok(())
}

/// `share`: as another holder, the session shown (vault, member, new key) and, on a yes, this holder's share sealed to
/// it.
fn share(remote: &mut Remote, client: &mut VaultClient, session_id: &str) -> Result<(), Error> {
    let sessions = remote.get("/recoveries", &[])?;
    let row = sessions["sessionList"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| text(row, "recoverySessionId") == session_id))
        .cloned()
        .ok_or_else(|| Error::new(format!("No open recovery {session_id} of a recovery key you hold")))?;
    let session = &row["session"];
    println!(
        "Recovery {session_id}: vault {} for {}, to their new key\n  {}\nstarted by {}.",
        text(session, "vault_id"),
        text(session, "subject_user_id"),
        text(&row, "subjectFingerprint"),
        text(session, "holder_user_id")
    );
    let yes = ask("Seal your share to this recovery? [y/N]", false)?;
    if !matches!(yes.trim(), "y" | "Y" | "yes") {
        return Err(Error::new("No share sealed"));
    }
    open_key(remote, client)?;
    let me = remote.get("/personalKey", &[])?;
    let chosen = chosen_key(remote, &text(&me, "userId"), &text(session, "recovery_key_id"))?;
    let held = holder_of(&chosen, &text(&me, "userId"))
        .map(|holder| holder["held"].clone())
        .ok_or_else(|| Error::new("You hold no share of this recovery key"))?;
    let sealed = client
        .call(
            "recovery_share_deliver",
            json!({ "record": chosen["record"], "maker_public": chosen["makerPublic"], "held": held,
                    "session": session, "session_holder_public": row["holderPublic"],
                    "key_checks": checks(remote)? }),
        )
        .map_err(|err| {
            Error::new(format!(
                "{err} (check the keys of {} and of the recovery key's maker first: moqui vault key check)",
                text(session, "holder_user_id")
            ))
        })?;
    remote.post(
        &format!("/recoveries/{}/shares", segment(session_id)),
        &json!({ "share": sealed }),
    )?;
    println!("Your share is sealed to recovery {session_id}.");
    Ok(())
}
