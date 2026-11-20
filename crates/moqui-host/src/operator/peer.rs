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
//! a member of the operator group; the server's own user, and so every child and job of the server, may run only
//! `status`. Group membership is the peer's primary group, and on Linux its supplementary groups from
//! `/proc/<pid>/status`.

use std::sync::atomic::{AtomicI64, Ordering};

use moqui_core::context::conf_env::conf_attribute;
use moqui_lib::node::MNode;

/// What a peer may run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Access {
    /// `status` only.
    Status,
    /// Every command.
    All,
}

/// The socket's peer settings from the conf.
#[derive(Clone, Debug, Default)]
pub(crate) struct Settings {
    /// The operator group's id, when `seal@operator-group` names one this host has.
    pub(crate) group: Option<u32>,
}

impl Settings {
    pub(crate) fn from_conf(conf: &MNode) -> Self {
        let name = conf_attribute(conf, "seal@operator-group");
        let group = resolve_group(&name);
        if group.is_none() && !name.is_empty() {
            tracing::info!("The operator group {name} is not on this host: only root may run operator commands");
        }
        Self { group }
    }
}

/// The service user's id a test sets, or -1 for this process's own.
static SERVICE_UID: AtomicI64 = AtomicI64::new(-1);

/// Count `uid` as the server's own user instead of this process's, so a test's own user is a peer like an operator.
#[doc(hidden)]
pub fn set_service_uid_for_tests(uid: u32) {
    SERVICE_UID.store(i64::from(uid), Ordering::SeqCst);
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
    match settings.group {
        Some(group) if gid == group || pid.is_some_and(|pid| supplementary_groups(pid).contains(&group)) => Access::All,
        _ => Access::Status,
    }
}

/// A group's id: a number as given, else the name's line in `/etc/group`.
fn resolve_group(name: &str) -> Option<u32> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    if let Ok(gid) = name.parse() {
        return Some(gid);
    }
    let groups = std::fs::read_to_string("/etc/group").ok()?;
    groups.lines().find_map(|line| {
        let mut fields = line.split(':');
        (fields.next() == Some(name))
            .then(|| fields.nth(1).and_then(|gid| gid.parse().ok()))
            .flatten()
    })
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
