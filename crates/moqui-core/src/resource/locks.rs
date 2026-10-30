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
//! lose it); a delete or move that is done drops the locks of what it took away. Locks are held in memory on this
//! server for their timeout (at most an hour): in a cluster a client stays on one server for its session.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// The end of the message of a write refused by a lock (the message starts with the name of what is locked).
pub const LOCKED_ERROR: &str = "locked by another user";

#[derive(Clone, Debug)]
pub struct Lock {
    pub token: String,
    pub user_id: String,
    /// The `D:owner` element the client sent, as XML.
    pub owner: String,
    pub depth_infinity: bool,
    pub seconds: u64,
    pub expires: std::time::Instant,
}

fn table() -> &'static Mutex<HashMap<String, Lock>> {
    static TABLE: OnceLock<Mutex<HashMap<String, Lock>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The table with expired locks dropped.
fn live() -> std::sync::MutexGuard<'static, HashMap<String, Lock>> {
    let mut table = table().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if !table.is_empty() {
        let now = std::time::Instant::now();
        table.retain(|_, lock| lock.expires > now);
    }
    table
}

/// The lock key of a location: no `#version`, no trailing slash.
pub fn key(location: &str) -> String {
    let location = match location.split_once('#') {
        Some((path, _)) if !path.is_empty() => path,
        _ => location,
    };
    location.trim_end_matches('/').to_string()
}

/// The lock on `location` or on a folder above it with depth infinity.
pub fn holding(location: &str) -> Option<Lock> {
    let key = key(location);
    let table = live();
    if let Some(lock) = table.get(&key) {
        return Some(lock.clone());
    }
    table
        .iter()
        .find(|(path, lock)| lock.depth_infinity && key.starts_with(&format!("{path}/")))
        .map(|(_, lock)| lock.clone())
}

/// The lock of exactly `location`.
pub fn get(location: &str) -> Option<Lock> {
    live().get(&key(location)).cloned()
}

/// Whether `user_id` may change `location`: an error when another user (or no user) holds a lock on it or on a folder
/// above it with depth infinity, or with `tree` (a delete or move of a folder) on anything under it.
pub fn check(location: &str, user_id: Option<&str>, tree: bool) -> Result<(), String> {
    let key = key(location);
    let table = live();
    if table.is_empty() {
        return Ok(());
    }
    let below = format!("{key}/");
    let refused = table.iter().find(|(path, lock)| {
        user_id != Some(lock.user_id.as_str())
            && (**path == key
                || (lock.depth_infinity && key.starts_with(&format!("{path}/")))
                || (tree && path.starts_with(&below)))
    });
    match refused {
        // the name of what is locked, not its storage location
        Some((path, _)) => Err(format!("{} is {LOCKED_ERROR}", path.rsplit('/').next().unwrap_or(path))),
        None => Ok(()),
    }
}

/// [`check`] for the current user.
pub fn check_current(location: &str, tree: bool) -> Result<(), String> {
    if table()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_empty()
    {
        return Ok(());
    }
    let user_id = crate::user::current_user_id().ok().flatten();
    check(location, user_id.as_deref(), tree)
}

/// Set the lock of `location`, replacing any there.
pub fn put(location: &str, lock: Lock) {
    live().insert(key(location), lock);
}

/// Change the lock of exactly `location` in place; `None` when there is none.
pub fn update<T>(location: &str, change: impl FnOnce(&mut Lock) -> T) -> Option<T> {
    live().get_mut(&key(location)).map(change)
}

/// Remove the lock of exactly `location`.
pub fn remove(location: &str) -> Option<Lock> {
    live().remove(&key(location))
}

/// A moved or deleted location takes its locks, and those of everything under it, with it.
pub fn release(location: &str) {
    let key = key(location);
    let below = format!("{key}/");
    let mut table = live();
    if !table.is_empty() {
        table.retain(|path, _| path != &key && !path.starts_with(&below));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock(user_id: &str, depth_infinity: bool) -> Lock {
        Lock {
            token: format!("opaquelocktoken:{user_id}"),
            user_id: user_id.to_string(),
            owner: String::new(),
            depth_infinity,
            seconds: 60,
            expires: std::time::Instant::now() + std::time::Duration::from_secs(60),
        }
    }

    #[test]
    fn a_lock_holds_its_own_location_and_nothing_beside_it() {
        let root = "dbresource://locks-test-one";
        put(&format!("{root}/docs/a.txt"), lock("alice", false));
        put(&format!("{root}/shared/"), lock("alice", true));
        assert!(check(&format!("{root}/docs/a.txt"), Some("bob"), false).is_err());
        assert!(
            check(&format!("{root}/docs/a.txt#v2"), None, false).is_err(),
            "no user is another user"
        );
        assert!(
            check(&format!("{root}/docs/a.txt"), Some("alice"), true).is_ok(),
            "the owner writes"
        );
        assert!(
            check(&format!("{root}/docs/b.txt"), Some("bob"), false).is_ok(),
            "the file beside it"
        );
        assert!(
            check(&format!("{root}/docs"), Some("bob"), false).is_ok(),
            "a write in the folder"
        );
        assert!(
            check(&format!("{root}/docs"), Some("bob"), true).is_err(),
            "a move of the folder takes the file"
        );
        assert!(check(&format!("{root}/docs/a.txt.bak"), Some("bob"), true).is_ok());
        assert!(
            check(&format!("{root}/shared/deep/c.txt"), Some("bob"), false).is_err(),
            "depth infinity"
        );
        assert!(check(&format!("{root}/shared-not"), Some("bob"), false).is_ok());
        release(&format!("{root}/docs"));
        assert!(
            check(&format!("{root}/docs/a.txt"), Some("bob"), false).is_ok(),
            "released with its folder"
        );
        release(&format!("{root}/shared"));
        assert!(holding(&format!("{root}/shared/deep/c.txt")).is_none());
    }

    #[test]
    fn an_expired_lock_holds_nothing() {
        let location = "dbresource://locks-test-two/a.txt";
        let mut expired = lock("alice", false);
        expired.expires = std::time::Instant::now() - std::time::Duration::from_secs(1);
        put(location, expired);
        assert!(check(location, Some("bob"), false).is_ok());
        assert!(get(location).is_none());
    }

    #[test]
    fn a_key_drops_the_version_and_trailing_slashes() {
        assert_eq!(key("dbresource://é/a.txt#v2#v3"), "dbresource://é/a.txt");
        assert_eq!(key("dbresource://docs//"), "dbresource://docs");
        assert_eq!(key("#v2/"), "#v2");
        assert_eq!(key("dbresource://a.txt"), "dbresource://a.txt");
    }
}
