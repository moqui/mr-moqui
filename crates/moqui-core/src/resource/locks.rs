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

//! Exclusive write locks on one resource location (a file or a folder of a drive), taken over WebDAV (`dav::locks`)
//! and kept by every writer: [`crate::context::ResourceFacade`] refuses a write, delete or move of a locked location by
//! a user other than the lock's owner, so the Files screens, SFTP drives, the drive services and scripts are held as a
//! WebDAV client is. A lock covers its own location, and with depth infinity everything under a folder; nothing else
//! is held, so writes of the files beside a locked one go on. Deleting or moving a folder is refused when anything
//! under it is locked by another user, as it would change that file. The owner may write without the token (clients
//! lose it); a delete or move that is done drops the locks of what it took away.
//!
//! Locks are `moqui.resource.ResourceLock` rows, so every member of a cluster holds them (plans/High-Level-Review.md
//! HL-036), each until its expiry (at most an hour, which WebDAV's LOCK sets). A row's key is a hash of its location,
//! so a write's check is one query by primary key of its location and the folders above it; a delete or move of a
//! folder also looks for the locks under it by path. Expired rows hold nothing and are removed as locks are taken.
//! A check that cannot read the locks refuses the write.

use base64::Engine as _;
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use sha2::{Digest as _, Sha256};

use crate::entity::{Condition, EntityFacade, EntityValue};
use crate::transaction::TransactionAttribute;
use crate::user::{now_millis, stamp_value, value_millis};

/// The end of the message of a write refused by a lock (the message starts with the name of what is locked).
pub const LOCKED_ERROR: &str = "locked by another user";

const LOCK_ENTITY: &str = "moqui.resource.ResourceLock";

#[derive(Clone, Debug)]
pub struct Lock {
    pub token: String,
    pub user_id: String,
    /// The `D:owner` element the client sent, as XML.
    pub owner: String,
    pub depth_infinity: bool,
    pub seconds: u64,
    /// When it ends, epoch milliseconds.
    pub expires_millis: i64,
}

impl Lock {
    /// A lock taken now for `seconds`.
    #[must_use]
    pub fn expiry(seconds: u64) -> i64 {
        now_millis().saturating_add(i64::try_from(seconds.saturating_mul(1000)).unwrap_or(i64::MAX))
    }
}

/// The lock key of a location: no `#version`, no trailing slash.
pub fn key(location: &str) -> String {
    let location = match location.split_once('#') {
        Some((path, _)) if !path.is_empty() => path,
        _ => location,
    };
    location.trim_end_matches('/').to_string()
}

/// The primary key of the row of the lock of `key`: SHA-256 of it, base64url, so a long location fits an index.
fn row_key(key: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(key.as_bytes()))
}

/// `key` and every folder above it, nearest first.
fn with_ancestors(key: &str) -> Vec<String> {
    let mut keys = vec![key.to_string()];
    let mut rest = key;
    while let Some((parent, _)) = rest.rsplit_once('/') {
        let parent = parent.trim_end_matches('/');
        if parent.is_empty() || parent.ends_with(':') {
            break;
        }
        keys.push(parent.to_string());
        rest = parent;
    }
    keys
}

fn lock_of(row: &EntityValue) -> (String, Lock) {
    let text = |name: &str| {
        row.get(name)
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default()
    };
    (
        text("lockPath"),
        Lock {
            token: text("lockToken"),
            user_id: text("userId"),
            owner: text("ownerXml"),
            depth_infinity: text("depthInfinity") == "Y",
            seconds: text("lockSeconds").parse().unwrap_or(0),
            expires_millis: row.get("expiresDate").and_then(value_millis).unwrap_or(0),
        },
    )
}

/// The live locks of `key` and the folders above it (by row key), and with `tree` those under it, as (path, lock).
fn live_around(facade: &EntityFacade, key: &str, tree: bool) -> Result<Vec<(String, Lock)>, Error> {
    // Locks are rare (WebDAV clients take them): with no row at all, nothing is read. The count is cached, and a write
    // of a lock, here or on a cluster peer, clears it.
    if facade.find(LOCK_ENTITY)?.use_cache(true).disable_authz().count()? == 0 {
        return Ok(Vec::new());
    }
    let keys: Vec<Value> = with_ancestors(key)
        .iter()
        .map(|key| Value::string(row_key(key)))
        .collect();
    let mut near = Condition::is_in("lockKey", keys);
    if tree {
        // narrows by path; what is really under the folder is decided below (`_` and `%` in a name match more)
        near = Condition::or(vec![
            near,
            Condition::like("lockPath", Value::string(format!("{key}/%"))),
        ]);
    }
    let rows = facade
        .find(LOCK_ENTITY)?
        .condition(near)
        .condition(Condition::greater("expiresDate", stamp_value(now_millis())))
        .disable_authz()
        .list()?;
    Ok(rows.values().iter().map(lock_of).collect())
}

fn applies(path: &str, lock: &Lock, key: &str, tree: bool) -> bool {
    path == key
        || (lock.depth_infinity && key.starts_with(&format!("{path}/")))
        || (tree && path.starts_with(&format!("{key}/")))
}

/// The lock on `location` or on a folder above it with depth infinity.
///
/// # Errors
///
/// When the locks cannot be read.
pub fn holding(facade: &EntityFacade, location: &str) -> Result<Option<Lock>, Error> {
    let key = key(location);
    let mut found = live_around(facade, &key, false)?;
    // its own first, then the nearest folder above
    found.sort_by_key(|(path, _)| std::cmp::Reverse(path.len()));
    Ok(found
        .into_iter()
        .find(|(path, lock)| applies(path, lock, &key, false))
        .map(|(_, lock)| lock))
}

/// The lock of exactly `location`.
///
/// # Errors
///
/// When the locks cannot be read.
pub fn get(facade: &EntityFacade, location: &str) -> Result<Option<Lock>, Error> {
    let key = key(location);
    Ok(live_around(facade, &key, false)?
        .into_iter()
        .find(|(path, _)| *path == key)
        .map(|(_, lock)| lock))
}

/// Whether `user_id` may change `location`: an error when another user (or no user) holds a lock on it or on a folder
/// above it with depth infinity, or with `tree` (a delete or move of a folder) on anything under it; and when the
/// locks cannot be read, as a write that might lose another user's change is not let through.
pub fn check(facade: &EntityFacade, location: &str, user_id: Option<&str>, tree: bool) -> Result<(), String> {
    let key = key(location);
    let found =
        live_around(facade, &key, tree).map_err(|err| format!("the locks of {key} could not be read: {err}"))?;
    let refused = found
        .iter()
        .find(|(path, lock)| user_id != Some(lock.user_id.as_str()) && applies(path, lock, &key, tree));
    match refused {
        // the name of what is locked, not its storage location
        Some((path, _)) => Err(format!("{} is {LOCKED_ERROR}", path.rsplit('/').next().unwrap_or(path))),
        None => Ok(()),
    }
}

/// [`check`] for the current user.
pub fn check_current(facade: &EntityFacade, location: &str, tree: bool) -> Result<(), String> {
    let user_id = crate::user::current_user_id().ok().flatten();
    check(facade, location, user_id.as_deref(), tree)
}

fn in_own_transaction<T>(facade: &EntityFacade, body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    facade
        .transactions_for(LOCK_ENTITY)?
        .run(TransactionAttribute::ForceNew, None, body)
}

/// Set the lock of `location`, replacing any there, in a transaction of its own; the expired rows go with it.
///
/// # Errors
///
/// When the lock cannot be written.
pub fn put(facade: &EntityFacade, location: &str, lock: &Lock) -> Result<(), Error> {
    let key = key(location);
    in_own_transaction(facade, || {
        facade
            .find(LOCK_ENTITY)?
            .condition(Condition::less_equal("expiresDate", stamp_value(now_millis())))
            .disable_authz()
            .delete_all()?;
        let mut row = facade.make_value(LOCK_ENTITY)?;
        row.set("lockKey", Value::string(row_key(&key)));
        row.set("lockPath", Value::string(&key));
        row.set("lockToken", Value::string(&lock.token));
        row.set("userId", Value::string(&lock.user_id));
        row.set("ownerXml", Value::string(&lock.owner));
        row.set(
            "depthInfinity",
            Value::string(if lock.depth_infinity { "Y" } else { "N" }),
        );
        row.set(
            "lockSeconds",
            Value::I64(i64::try_from(lock.seconds).unwrap_or(i64::MAX)),
        );
        row.set("expiresDate", stamp_value(lock.expires_millis));
        row.create_or_update(facade)
    })
}

/// Change the lock of exactly `location`, if it is live, and keep the change; `None` when there is none.
///
/// # Errors
///
/// When the lock cannot be read or written.
pub fn update<T>(
    facade: &EntityFacade,
    location: &str,
    change: impl FnOnce(&mut Lock) -> T,
) -> Result<Option<T>, Error> {
    let Some(mut lock) = get(facade, location)? else {
        return Ok(None);
    };
    let changed = change(&mut lock);
    put(facade, location, &lock)?;
    Ok(Some(changed))
}

/// Remove the lock of exactly `location`.
///
/// # Errors
///
/// When the lock cannot be removed.
pub fn remove(facade: &EntityFacade, location: &str) -> Result<(), Error> {
    let key = key(location);
    in_own_transaction(facade, || {
        facade
            .find(LOCK_ENTITY)?
            .eq("lockKey", Value::string(row_key(&key)))
            .disable_authz()
            .delete_all()
            .map(|_| ())
    })
}

/// A moved or deleted location takes its locks, and those of everything under it, with it. One that cannot be
/// removed is logged; it ends at its expiry.
pub fn release(facade: &EntityFacade, location: &str) {
    let key = key(location);
    let below = format!("{key}/");
    let released = in_own_transaction(facade, || {
        let rows = facade
            .find(LOCK_ENTITY)?
            .condition(Condition::or(vec![
                Condition::equals("lockKey", Value::string(row_key(&key))),
                Condition::like("lockPath", Value::string(format!("{below}%"))),
            ]))
            .disable_authz()
            .list()?;
        for row in rows.values() {
            let (path, _) = lock_of(row);
            if path == key || path.starts_with(&below) {
                row.delete(facade)?;
            }
        }
        Ok(())
    });
    if let Err(err) = released {
        tracing::error!("The WebDAV locks of {key} were not released: {err}");
    }
}

#[cfg(test)]
#[path = "locks_tests.rs"]
mod tests;
