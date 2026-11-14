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

//! The rows of ACME (`moqui.server.AcmeName`, `AcmeChallenge`, `AcmeCertificate`, `AcmeOrderLock`) and the files each
//! member writes from them. Each function runs in a transaction of its own with authorization off: the screen's
//! services have checked their caller, and the rest is the order's own bookkeeping.
//!
//! The files of a name are `runtime/cert/<name>/fullchain.pem` and `privkey.pem`, certbot's layout, with a marker file
//! ([`MARKER`]) that says they are this module's. A directory with no marker is another tool's (certbot itself, say):
//! it is never written or removed here, and a name whose directory is one is refused at set up.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_core::transaction::TransactionAttribute;
use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};

use super::challenge::{parse_digest, Challenge};
use super::Acme;

pub(crate) const NAME: &str = "moqui.server.AcmeName";
pub(crate) const CHALLENGE: &str = "moqui.server.AcmeChallenge";
pub(crate) const CERTIFICATE: &str = "moqui.server.AcmeCertificate";
const LOCK: &str = "moqui.server.AcmeOrderLock";
pub(crate) const ORDERING: &str = "AcmeOrdering";
pub(crate) const ISSUED: &str = "AcmeIssued";
pub(crate) const FAILED: &str = "AcmeFailed";
/// The marker file in a name's directory that says its files are this module's.
pub(crate) const MARKER: &str = ".moqui-acme";
/// How long a member holds a name's order lock: an order that runs longer is taken over.
const LEASE: Duration = Duration::from_secs(15 * 60);

pub(crate) fn facade(factory: &ExecutionContextFactory) -> Result<EntityFacade, Error> {
    factory
        .entity()
        .ok_or_else(|| Error::new("the entity facade is not installed"))
}

/// `body` in a transaction of its own, committed when it returns, with authorization off.
pub(crate) fn committed<T>(
    facade: &EntityFacade,
    entity: &str,
    body: impl FnOnce() -> Result<T, Error>,
) -> Result<T, Error> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let tx = facade.transactions_for(entity)?;
    tx.run(TransactionAttribute::ForceNew, Some(Duration::from_secs(30)), body)
}

fn text(row: &moqui_core::entity::EntityValue, field: &str) -> String {
    row.get(field).map(to_plain_string).unwrap_or_default()
}

fn millis(row: &moqui_core::entity::EntityValue, field: &str) -> Option<i64> {
    row.get(field).and_then(moqui_core::user::value_millis)
}

/// A name set up here.
#[derive(Clone, Debug, Default)]
pub(crate) struct NameRow {
    pub(crate) host: String,
    pub(crate) directory_url: String,
    pub(crate) contact_email: String,
    pub(crate) renewing: bool,
    pub(crate) status: String,
    pub(crate) last_attempt: Option<i64>,
    pub(crate) last_error: String,
    pub(crate) issued: Option<i64>,
    pub(crate) not_after: Option<i64>,
    pub(crate) set_up: Option<i64>,
}

fn name_row(row: &moqui_core::entity::EntityValue) -> NameRow {
    NameRow {
        host: text(row, "hostName"),
        directory_url: text(row, "directoryUrl"),
        contact_email: text(row, "contactEmail"),
        renewing: text(row, "renewing") != "N",
        status: text(row, "statusId"),
        last_attempt: millis(row, "lastAttemptDate"),
        last_error: text(row, "lastError"),
        issued: millis(row, "issuedDate"),
        not_after: millis(row, "notAfter"),
        set_up: millis(row, "setUpDate"),
    }
}

/// Every name set up here, by name.
pub(crate) fn names(factory: &ExecutionContextFactory) -> Result<Vec<NameRow>, Error> {
    let facade = facade(factory)?;
    committed(&facade, NAME, || {
        let rows = facade.find(NAME)?.order_by("hostName", false).list()?;
        Ok(rows.iter().map(name_row).collect())
    })
}

pub(crate) fn name(factory: &ExecutionContextFactory, host: &str) -> Result<Option<NameRow>, Error> {
    let facade = facade(factory)?;
    committed(&facade, NAME, || {
        let row = facade.find(NAME)?.eq("hostName", Value::string(host)).one()?;
        Ok(row.as_ref().map(name_row))
    })
}

/// Set a name up, or change the authority, contact and terms of one set up before; it renews again.
pub(crate) fn save_name(
    factory: &ExecutionContextFactory,
    host: &str,
    directory_url: &str,
    contact_email: &str,
    user_id: Option<&str>,
) -> Result<(), Error> {
    let facade = facade(factory)?;
    committed(&facade, NAME, || {
        let existing = facade.find(NAME)?.eq("hostName", Value::string(host)).one()?;
        let mut row = facade.make_value(NAME)?;
        row.set("hostName", Value::string(host));
        row.set("directoryUrl", Value::string(directory_url));
        row.set("contactEmail", Value::string(contact_email));
        row.set("termsAgreedDate", moqui_core::user::now_value());
        row.set("renewing", Value::string("Y"));
        if existing.is_none() {
            row.set("setUpDate", moqui_core::user::now_value());
            row.set("setUpByUserId", user_id.map_or(Value::Null, Value::string));
            row.create(&facade)
        } else {
            row.update(&facade)
        }
    })
}

pub(crate) fn set_renewing(factory: &ExecutionContextFactory, host: &str, renewing: bool) -> Result<bool, Error> {
    let facade = facade(factory)?;
    committed(&facade, NAME, || {
        if facade.find(NAME)?.eq("hostName", Value::string(host)).one()?.is_none() {
            return Ok(false);
        }
        let mut row = facade.make_value(NAME)?;
        row.set("hostName", Value::string(host));
        row.set("renewing", Value::string(if renewing { "Y" } else { "N" }));
        row.update(&facade)?;
        Ok(true)
    })
}

/// What an order did, on the name's row.
pub(crate) enum Outcome<'a> {
    Started,
    Issued { issued: i64, not_after: i64 },
    Failed(&'a str),
}

pub(crate) fn record(factory: &ExecutionContextFactory, host: &str, outcome: &Outcome<'_>) -> Result<(), Error> {
    let facade = facade(factory)?;
    committed(&facade, NAME, || {
        if facade.find(NAME)?.eq("hostName", Value::string(host)).one()?.is_none() {
            // removed while it was ordered
            return Ok(());
        }
        let mut row = facade.make_value(NAME)?;
        row.set("hostName", Value::string(host));
        match outcome {
            Outcome::Started => {
                row.set("statusId", Value::string(ORDERING));
                row.set("lastAttemptDate", moqui_core::user::now_value());
            }
            Outcome::Issued { issued, not_after } => {
                row.set("statusId", Value::string(ISSUED));
                row.set("lastError", Value::Null);
                row.set("issuedDate", moqui_core::user::stamp_value(*issued));
                row.set("notAfter", moqui_core::user::stamp_value(*not_after));
            }
            Outcome::Failed(error) => {
                row.set("statusId", Value::string(FAILED));
                row.set("lastError", Value::string(*error));
            }
        }
        row.update(&facade)
    })
}

/// Remove a name and its rows; its files go at each member's next sync, and here now. False when it was not set up.
pub(crate) fn remove(factory: &ExecutionContextFactory, host: &str) -> Result<bool, Error> {
    let facade = facade(factory)?;
    let removed = committed(&facade, NAME, || {
        // locked first, so an order's save_certificate waits for this and then finds the name gone
        let Some(row) = facade
            .find(NAME)?
            .eq("hostName", Value::string(host))
            .for_update(true)
            .one()?
        else {
            return Ok(false);
        };
        for entity in [CERTIFICATE, CHALLENGE, LOCK] {
            if let Some(other) = facade.find(entity)?.eq("hostName", Value::string(host)).one()? {
                other.delete(&facade)?;
            }
        }
        row.delete(&facade)?;
        Ok(true)
    })?;
    if removed {
        remove_files(&cert_dir(factory), host)?;
    }
    Ok(removed)
}

/// Put `host`'s challenge in place: its row, for every member, and this process's memory.
pub(crate) fn put_challenge(
    factory: &ExecutionContextFactory,
    host: &str,
    digest: &[u8; 32],
    open_for: Duration,
) -> Result<(), Error> {
    let facade = facade(factory)?;
    let expires = moqui_core::user::now_millis() + i64::try_from(open_for.as_millis()).unwrap_or(i64::MAX / 2);
    committed(&facade, CHALLENGE, || {
        let mut row = facade.make_value(CHALLENGE)?;
        row.set("hostName", Value::string(host));
        row.set("keyAuthDigest", Value::string(crate::tls_pin::hex(digest)));
        row.set("expireDate", moqui_core::user::stamp_value(expires));
        row.create_or_update(&facade)
    })?;
    let acme = Acme::of(factory);
    acme.forget_read();
    read_challenges(factory, &acme)
}

/// Close `host`'s challenge, when its digest is `digest` (another order's stays).
pub(crate) fn close_challenge(factory: &ExecutionContextFactory, host: &str, digest: &[u8; 32]) -> Result<(), Error> {
    let facade = facade(factory)?;
    let hex = crate::tls_pin::hex(digest);
    committed(&facade, CHALLENGE, || {
        if let Some(row) = facade.find(CHALLENGE)?.eq("hostName", Value::string(host)).one()? {
            if text(&row, "keyAuthDigest") == hex {
                row.delete(&facade)?;
            }
        }
        Ok(())
    })?;
    let acme = Acme::of(factory);
    acme.forget(host, digest);
    acme.forget_read();
    Ok(())
}

/// Read the open challenges into `acme`.
pub(crate) fn read_challenges(factory: &ExecutionContextFactory, acme: &Acme) -> Result<(), Error> {
    let facade = facade(factory)?;
    let now = moqui_core::user::now_millis();
    let rows = committed(&facade, CHALLENGE, || {
        facade
            .find(CHALLENGE)?
            .condition(moqui_core::entity::Condition::greater(
                "expireDate",
                moqui_core::user::stamp_value(now),
            ))
            .list()
    })?;
    let mut challenges = HashMap::new();
    for row in rows.iter() {
        let host = text(row, "hostName");
        match (parse_digest(&text(row, "keyAuthDigest")), millis(row, "expireDate")) {
            (Some(digest), Some(expires)) => {
                challenges.insert(host, Arc::new(Challenge::new(digest, expires)));
            }
            _ => tracing::error!("The ACME challenge row of {host} is not usable; it is left out"),
        }
    }
    acme.replace_challenges(challenges);
    Ok(())
}

/// Take `host`'s order lock for `instance`: true when this member may order it now, false when another member holds
/// it and its lease has not run out.
pub(crate) fn claim_lock(factory: &ExecutionContextFactory, host: &str, instance: &str) -> Result<bool, Error> {
    let facade = facade(factory)?;
    let now = moqui_core::user::now_millis();
    let lease = now + i64::try_from(LEASE.as_millis()).unwrap_or(i64::MAX / 2);
    committed(&facade, LOCK, || {
        let held = facade
            .find(LOCK)?
            .eq("hostName", Value::string(host))
            .for_update(true)
            .one()?;
        let mut row = facade.make_value(LOCK)?;
        row.set("hostName", Value::string(host));
        row.set("instanceId", Value::string(instance));
        row.set("leaseExpireDate", moqui_core::user::stamp_value(lease));
        match held {
            None => row.create(&facade)?,
            Some(held) => {
                let other = text(&held, "instanceId");
                let live = millis(&held, "leaseExpireDate").is_some_and(|until| until > now);
                if other != instance && live {
                    return Ok(false);
                }
                row.update(&facade)?;
            }
        }
        Ok(true)
    })
}

pub(crate) fn release_lock(factory: &ExecutionContextFactory, host: &str, instance: &str) -> Result<(), Error> {
    let facade = facade(factory)?;
    committed(&facade, LOCK, || {
        if let Some(row) = facade.find(LOCK)?.eq("hostName", Value::string(host)).one()? {
            if text(&row, "instanceId") == instance {
                row.delete(&facade)?;
            }
        }
        Ok(())
    })
}

/// The names whose order lock some member holds now: those being ordered.
pub(crate) fn live_locks(factory: &ExecutionContextFactory) -> Result<HashSet<String>, Error> {
    let facade = facade(factory)?;
    let now = moqui_core::user::now_millis();
    committed(&facade, LOCK, || {
        let rows = facade.find(LOCK)?.list()?;
        Ok(rows
            .iter()
            .filter(|row| millis(row, "leaseExpireDate").is_some_and(|until| until > now))
            .map(|row| text(row, "hostName"))
            .collect())
    })
}

/// Whether another member holds `host`'s order lock now.
pub(crate) fn locked_elsewhere(factory: &ExecutionContextFactory, host: &str, instance: &str) -> Result<bool, Error> {
    let facade = facade(factory)?;
    let now = moqui_core::user::now_millis();
    committed(&facade, LOCK, || {
        let held = facade.find(LOCK)?.eq("hostName", Value::string(host)).one()?;
        Ok(held.is_some_and(|held| {
            text(&held, "instanceId") != instance && millis(&held, "leaseExpireDate").is_some_and(|until| until > now)
        }))
    })
}

/// A certificate an order got.
pub(crate) struct Issued {
    pub(crate) chain_pem: String,
    pub(crate) key_pem: String,
    pub(crate) issuer: String,
    pub(crate) directory_url: String,
    pub(crate) issued: i64,
    pub(crate) not_after: i64,
}

/// Store the certificate an order got and write its files; refused when the name was removed while it was ordered (its
/// row is read locked, so a Remove waits for this or this sees it).
pub(crate) fn save_certificate(factory: &ExecutionContextFactory, host: &str, issued: &Issued) -> Result<(), Error> {
    let facade = facade(factory)?;
    committed(&facade, NAME, || {
        let set_up = facade
            .find(NAME)?
            .eq("hostName", Value::string(host))
            .for_update(true)
            .one()?;
        if set_up.is_none() {
            return Err(Error::new(format!("{host} was removed while it was ordered")));
        }
        let mut row = facade.make_value(CERTIFICATE)?;
        row.set("hostName", Value::string(host));
        row.set("certificatePem", Value::string(&issued.chain_pem));
        row.set("privateKeyPem", Value::string(&issued.key_pem));
        row.set("issuer", Value::string(&issued.issuer));
        row.set("directoryUrl", Value::string(&issued.directory_url));
        row.set("issuedDate", moqui_core::user::stamp_value(issued.issued));
        row.set("notAfter", moqui_core::user::stamp_value(issued.not_after));
        row.create_or_update(&facade)
    })?;
    write_files(&cert_dir(factory), host, &issued.chain_pem, &issued.key_pem)
}

pub(crate) fn cert_dir(factory: &ExecutionContextFactory) -> PathBuf {
    factory.runtime_dir().join("cert")
}

/// Whether `dir/<host>/` holds another tool's files: a directory with no [`MARKER`].
pub(crate) fn foreign_dir(dir: &Path, host: &str) -> bool {
    let sub = dir.join(host);
    sub.is_dir() && !sub.join(MARKER).is_file()
}

/// Write `host`'s files when they differ: the key, then the chain, each by a rename so the listener never reads half
/// a file. A directory that is another tool's is refused.
pub(crate) fn write_files(dir: &Path, host: &str, chain_pem: &str, key_pem: &str) -> Result<(), Error> {
    if foreign_dir(dir, host) {
        return Err(Error::new(format!(
            "{} holds files another tool wrote; this node does not write over them",
            dir.join(host).display()
        )));
    }
    let sub = dir.join(host);
    let fail = |err: std::io::Error| Error::new(format!("could not write {}: {err}", sub.display()));
    std::fs::create_dir_all(&sub).map_err(fail)?;
    let marker = sub.join(MARKER);
    if !marker.is_file() {
        std::fs::write(
            &marker,
            "Written by System › Certificates (ACME) from the database; changed or removed there.\n",
        )
        .map_err(fail)?;
    }
    let same = |file: &str, text: &str| std::fs::read_to_string(sub.join(file)).is_ok_and(|old| old == text);
    if !same("privkey.pem", key_pem) {
        let staged = sub.join(".privkey.pem.new");
        crate::tls::write_private(&staged, key_pem)?;
        std::fs::rename(&staged, sub.join("privkey.pem")).map_err(fail)?;
    }
    if !same("fullchain.pem", chain_pem) {
        let staged = sub.join(".fullchain.pem.new");
        std::fs::write(&staged, chain_pem).map_err(fail)?;
        std::fs::rename(&staged, sub.join("fullchain.pem")).map_err(fail)?;
    }
    Ok(())
}

/// Remove `host`'s files when they are this module's.
pub(crate) fn remove_files(dir: &Path, host: &str) -> Result<(), Error> {
    let sub = dir.join(host);
    if !sub.join(MARKER).is_file() {
        return Ok(());
    }
    std::fs::remove_dir_all(&sub).map_err(|err| Error::new(format!("could not remove {}: {err}", sub.display())))
}

/// Bring this member's files up to date with the rows: write each certificate whose files differ, and remove the
/// files of a name no longer set up. Once any name is set up it wants the HTTPS listener and keeps the renewal job
/// running (a Remove racing a set-up could have paused it). One name that fails is logged and the others go on.
pub(crate) fn sync_files(factory: &ExecutionContextFactory) -> Result<(), Error> {
    let facade = facade(factory)?;
    let (names, certificates) = committed(&facade, NAME, || {
        let names = facade.find(NAME)?.select_field("hostName").list()?;
        let certificates = if names.is_empty() {
            moqui_core::entity::EntityList::new(Vec::new())
        } else {
            facade.find(CERTIFICATE)?.list()?
        };
        Ok((names, certificates))
    })?;
    if !names.is_empty() {
        Acme::of(factory).want();
        if let Err(err) = renewal_job(factory, true) {
            tracing::error!("ACME: the renewal job could not be started: {err}");
        }
    }
    let dir = cert_dir(factory);
    let set_up: HashSet<String> = names.iter().map(|row| text(row, "hostName")).collect();
    for row in certificates.iter() {
        let host = text(row, "hostName");
        if !set_up.contains(&host) {
            continue;
        }
        if let Err(err) = write_files(&dir, &host, &text(row, "certificatePem"), &text(row, "privateKeyPem")) {
            tracing::error!("The ACME certificate of {host} could not be written: {err}");
        }
    }
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let Ok(host) = entry.file_name().into_string() else {
            continue;
        };
        // only a name not set up: a certificate the ordering member wrote after the rows were read stays
        if !set_up.contains(&host) && entry.path().join(MARKER).is_file() {
            match remove_files(&dir, &host) {
                Ok(()) => tracing::info!("Removed the ACME certificate files of {host}: it is no longer set up"),
                Err(err) => tracing::error!("{err}"),
            }
        }
    }
    Ok(())
}

/// Pause the renewal job, or start it: started when a name is set up, paused when the last one is removed.
pub(crate) fn renewal_job(factory: &ExecutionContextFactory, on: bool) -> Result<(), Error> {
    const JOB: &str = "moqui.service.job.ServiceJob";
    let facade = facade(factory)?;
    committed(&facade, JOB, || {
        let paused = if on { "N" } else { "Y" };
        let Some(job) = facade
            .find(JOB)?
            .eq("jobName", Value::string(super::order::RENEWAL_JOB))
            .one()?
        else {
            tracing::warn!(
                "The ServiceJob {} is not in the database; ACME certificates are not renewed",
                super::order::RENEWAL_JOB
            );
            return Ok(());
        };
        if text(&job, "paused") == paused {
            return Ok(());
        }
        let mut row = facade.make_value(JOB)?;
        row.set("jobName", Value::string(super::order::RENEWAL_JOB));
        row.set("paused", Value::string(paused));
        row.update(&facade)
    })
}
