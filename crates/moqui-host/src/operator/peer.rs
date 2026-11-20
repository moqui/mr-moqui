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

//! Who may use the operator socket (plans/Sealed-Secrets.md §3.3, §6 "Who may use the socket"): the peer's
//! credentials from the kernel (`SO_PEERCRED` on Linux, `getpeereid` on macOS). Root may run every command, and so may
//! a member of the operator group when root set that group on the socket ([`super::socket`]); no conf or environment
//! names it. The server's own user, and so every child and job of the server, may run only `status`. Group
//! membership is the peer's primary group, and on Linux its supplementary groups from `/proc/<pid>/status`.

use std::sync::atomic::{AtomicI64, Ordering};

/// What a peer may run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Access {
    /// `status` only.
    Status,
    /// Every command.
    All,
}

/// Who may use the socket besides root.
#[derive(Clone, Debug, Default)]
pub(crate) struct Settings {
    /// The operator group's id, when root set it on the socket.
    pub(crate) group: Option<u32>,
}

/// The service user's id a test sets, or -1 for this process's own.
static SERVICE_UID: AtomicI64 = AtomicI64::new(-1);

/// The operator group a test sets, as if root had given it the socket, or -1 for none.
static TEST_GROUP: AtomicI64 = AtomicI64::new(-1);

/// Count `uid` as the server's own user instead of this process's, so a test's own user is a peer like an operator.
#[doc(hidden)]
pub fn set_service_uid_for_tests(uid: u32) {
    SERVICE_UID.store(i64::from(uid), Ordering::SeqCst);
}

/// Grant `gid` as the operator group of a socket the server binds itself, as root's socket would, so a test's own
/// user can be an operator.
#[doc(hidden)]
pub fn set_operator_gid_for_tests(gid: u32) {
    TEST_GROUP.store(i64::from(gid), Ordering::SeqCst);
}

/// The operator group a test set.
pub(crate) fn group_for_tests() -> Option<u32> {
    u32::try_from(TEST_GROUP.load(Ordering::SeqCst)).ok()
}

/// The server's own user.
fn service_uid() -> u32 {
    u32::try_from(SERVICE_UID.load(Ordering::SeqCst)).unwrap_or_else(|_| rustix::process::geteuid().as_raw())
}

/// What the peer `uid`, `gid` (primary) and `pid` may run.
pub(crate) fn access(uid: u32, gid: u32, pid: Option<i32>, settings: &Settings) -> Access {
    if uid == 0 {
        return Access::All;
    }
    if uid == service_uid() {
        return Access::Status;
    }
    match settings.group.or_else(group_for_tests) {
        Some(group) if gid == group || pid.is_some_and(|pid| supplementary_groups(pid).contains(&group)) => Access::All,
        _ => Access::Status,
    }
}

/// The supplementary groups of process `pid` (Linux); none elsewhere, where only the primary group counts.
fn supplementary_groups(pid: i32) -> Vec<u32> {
    if !cfg!(target_os = "linux") {
        return Vec::new();
    }
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("Groups:"))
                .map(|groups| groups.split_whitespace().filter_map(|gid| gid.parse().ok()).collect())
        })
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "peer_tests.rs"]
mod tests;
