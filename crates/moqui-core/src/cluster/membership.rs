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

//! Whether this member is in a cluster, and what follows when that changes (plans/Runtime-Config.md §3.5, RC6).
//!
//! A cluster is detected ([`cluster_detected`]) from a read of the member table that finds a live member with this
//! member's session key and cluster certificate fingerprints until one that finds no other member's row left
//! ([`cluster_after`]): rows whose heartbeats fell behind keep it as it was, so the cluster port does not close under a
//! late peer. Every read sets it ([`read_peers`]): the one at `start`, the heartbeat's, a send's re-read, and the short
//! check. Reads are numbered as they begin, and one applied after a later one is dropped ([`newest`]). A row with this
//! member's own base URL (a previous run of it) is not a peer.
//!
//! When it changes, the host's hook is told ([`on_membership`]; core cannot call the host), which asks the listener
//! registry for a pass under `cluster-member@listen="auto"`; and when this member joins a cluster its own entity caches
//! are cleared, since while it was alone nobody could tell it of a write ([`recover_after_join`]).
//!
//! Between heartbeats the heartbeat thread wakes every `cluster-member@lone-check-seconds` ([`wake_every`]) for a short
//! check ([`short_check`]): a read of the member table while this member is alone under `auto` (a clock query and one
//! read of the table), and a resync while a peer is marked as having missed messages, so a peer whose posts failed (a
//! member whose port was still closed) gets its clear of every entity cache once its backoff ends, not at the next
//! heartbeat. A wake with neither to do reads nothing.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use moqui_lib::sync::lock;

use super::{
    db_now, is_live, load_members, recover_missed, request_resync, same_material, trim_base, window_millis,
    ClusterState, PeerCache, NOTIFY_BASE_URL,
};
use crate::context::{Error, ExecutionContextFactory};
use crate::entity::EntityFacade;

/// `cluster-member@lone-check-seconds` when the conf leaves it empty or gives no whole number of seconds (§3.5, §6).
pub(super) const LONE_CHECK_DEFAULT: Duration = Duration::from_secs(15);

/// What the host does when this member's cluster is detected or is gone, told whether it is now: the cluster port's
/// entry asks the listener registry for a pass (plans/Runtime-Config.md §3.5). Core cannot call the host, so the host
/// installs it ([`on_membership`]).
pub type MembershipHook = Arc<dyn Fn(bool) + Send + Sync>;

/// Whether this member is in a cluster, as the member table was last read (in memory, no database read): from the
/// first read that finds a live member with this member's session key and cluster certificate fingerprints, at
/// `start` or later, until one that finds no other member's row left (plans/Runtime-Config.md §3.5). The cluster
/// port's entry opens the port on it under `cluster-member@listen="auto"`.
pub fn cluster_detected(factory: &ExecutionContextFactory) -> bool {
    lock(&ClusterState::of(factory).peers).cluster
}

/// Install `hook` on `factory`'s cluster, in place of one before: it is told each time [`cluster_detected`] changes,
/// on the thread that read the member table (the heartbeat's, or a sender's), with no lock held. The host's cluster
/// port entry installs it before `start`.
pub fn on_membership(factory: &ExecutionContextFactory, hook: MembershipHook) {
    *lock(&ClusterState::of(factory).membership) = Some(hook);
}

/// The number of a read of the member table about to begin, for [`newest`].
pub(super) fn next_read(state: &ClusterState) -> u64 {
    state.reads.fetch_add(1, Ordering::SeqCst) + 1
}

/// Whether the read numbered `read` may be applied to `peers`, which it then counts as applied: false for one that
/// began before the last applied, so a slow read does not undo what a later one found.
pub(super) fn newest(peers: &mut PeerCache, read: u64) -> bool {
    if read <= peers.applied_read {
        return false;
    }
    peers.applied_read = read;
    true
}

/// One read of the member table for this member: its live peers' base URLs, and whether any other member's row with
/// this member's material is left, live or not (a row with no heartbeat, which no cleanup deletes, is not counted).
pub(super) struct PeerRead {
    pub(super) urls: Vec<String>,
    pub(super) rows_left: bool,
}

/// Read the member table for this member ([`PeerRead`]). Rows left out are logged once while they stay: one of other
/// material (another cluster on the same database, or a row written by hand), and one that names this member's own base
/// URL with another id (a previous run of this member, or a member whose `cluster-member@base-url` is not its own).
pub(super) fn read_peers(
    state: &ClusterState,
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
) -> Result<PeerRead, Error> {
    let instance_id = state.instance_id();
    let now = db_now(facade);
    let window = window_millis(factory);
    let own = lock(&state.member).clone();
    let own_base = own.as_ref().map(|own| trim_base(&own.base_url)).unwrap_or_default();
    let mut bases = Vec::new();
    let mut rows_left = false;
    let mut left_out: Vec<(String, String)> = Vec::new();
    for member in load_members(facade)? {
        if member.instance_id == instance_id || member.heartbeat.is_none() {
            continue;
        }
        let base = trim_base(&member.base_url);
        if is_own_address(&base, &own_base) {
            if is_live(member.heartbeat, now, window) {
                left_out.push((
                    member.instance_id,
                    format!(
                        "its row names this member's own base URL {base} (a previous run of this member, or a member \
                         whose cluster-member@base-url is not its own)"
                    ),
                ));
            }
            continue;
        }
        let same = same_material(&member, own.as_ref());
        rows_left |= same;
        if !is_live(member.heartbeat, now, window) {
            continue;
        }
        if !same {
            left_out.push((
                member.instance_id,
                "its row names another session key or cluster certificate".to_string(),
            ));
            continue;
        }
        // Every notify member is reached by the one NOTIFY, so they are one peer here.
        let base = if base.starts_with(NOTIFY_BASE_URL) {
            NOTIFY_BASE_URL.to_string()
        } else {
            base
        };
        if !base.is_empty() && !bases.contains(&base) {
            bases.push(base);
        }
    }
    warn_left_out(state, left_out);
    Ok(PeerRead { urls: bases, rows_left })
}

/// Whether a row's base URL `base` is this member's own, `own` (both trimmed): never for the one NOTIFY address that
/// every notify member shares, nor before this member has a base URL.
pub(super) fn is_own_address(base: &str, own: &str) -> bool {
    !own.is_empty() && base == own && !own.starts_with(NOTIFY_BASE_URL)
}

/// Log each row left out that the last read did not leave out too, so a row that stays is named once, not at each read.
fn warn_left_out(state: &ClusterState, left_out: Vec<(String, String)>) {
    let mut warned = lock(&state.left_out_warned);
    for (id, why) in &left_out {
        if !warned.contains(id) {
            tracing::warn!("cluster member {id} is left out: {why}");
        }
    }
    *warned = left_out.into_iter().map(|(id, _)| id).collect();
}

/// Whether this member is in a cluster at `start`, read once: the fan-out's own list is left for its first send, as
/// before. Rows that cannot be read leave it as it was, logged.
pub(super) fn detect_cluster(state: &ClusterState, factory: &ExecutionContextFactory, facade: &EntityFacade) {
    let read = next_read(state);
    match read_peers(state, factory, facade) {
        Ok(PeerRead { urls, rows_left }) => {
            let changed = {
                let mut peers = lock(&state.peers);
                if !newest(&mut peers, read) {
                    return;
                }
                set_cluster(&mut peers, !urls.is_empty(), rows_left)
            };
            if let Some(cluster) = changed {
                tell_membership(state, cluster);
            }
        }
        Err(err) => tracing::warn!("cluster member list could not be read at the start: {err}"),
    }
}

/// Set `peers.cluster` from a read of the member table ([`cluster_after`]); what it became when that changed it.
pub(super) fn set_cluster(peers: &mut PeerCache, live_peers: bool, rows_left: bool) -> Option<bool> {
    let cluster = cluster_after(peers.cluster, live_peers, rows_left);
    (cluster != peers.cluster).then(|| {
        peers.cluster = cluster;
        cluster
    })
}

/// Whether a cluster is detected after a read of the member table, from whether it was before (`was`), whether the read
/// found a live peer and whether it found any other member's row left: a live peer makes one, no row left ends it (the
/// heartbeats delete a dead member's row after ten liveness windows), and between, rows whose heartbeats fell behind,
/// it stays as it was.
pub(super) fn cluster_after(was: bool, live_peers: bool, rows_left: bool) -> bool {
    if live_peers {
        true
    } else if rows_left {
        was
    } else {
        false
    }
}

/// Tell the membership hook, outside every lock, that this member is now in a cluster or alone, and log it; joining,
/// clear this member's own caches ([`recover_after_join`]).
pub(super) fn tell_membership(state: &ClusterState, cluster: bool) {
    if cluster {
        tracing::info!("cluster member {}: another member is live", state.instance_id());
        recover_after_join(state);
    } else {
        tracing::info!("cluster member {}: no other member is left", state.instance_id());
    }
    let hook = lock(&state.membership).clone();
    if let Some(hook) = hook {
        hook(cluster);
    }
}

/// Clear this member's entity caches and retire the sessions ended elsewhere, as a peer's clear of every cache does
/// (`recover_missed`), on a thread of its own: while it was alone a member that joined could not tell it of its writes
/// (its posts failed, or it had not yet seen this member), so what this member cached since may be stale.
fn recover_after_join(state: &ClusterState) {
    let factory = lock(&state.factory).clone();
    let spawned = std::thread::Builder::new()
        .name("moqui-cluster-joined".to_string())
        .spawn(crate::context::framework_thread(move || {
            let Some(factory) = factory.upgrade() else {
                return;
            };
            let Some(facade) = factory.entity() else {
                return;
            };
            let context = factory.new_context();
            let _entered = context.enter();
            let _authz = context.disable_authz();
            if let Err(err) = recover_missed(Some(&factory), &facade) {
                tracing::error!("cluster member joined a cluster but its entity caches were not cleared: {err}");
            }
        }));
    if let Err(err) = spawned {
        tracing::error!("cluster member joined a cluster but no thread could clear its entity caches: {err}");
    }
}

/// How long the heartbeat thread sleeps: until the heartbeat is due, `until_beat` from now, or until its next short
/// check, `every` (plans/Runtime-Config.md §3.5), whichever is sooner.
pub(super) fn next_wake(until_beat: Duration, every: Duration) -> Duration {
    every.min(until_beat)
}

/// How often the heartbeat thread wakes for a short check between heartbeats: every
/// `cluster-member@lone-check-seconds`, read on each pass; one that is not a whole number of seconds is logged once and
/// the default taken.
pub(super) fn wake_every(factory: &ExecutionContextFactory) -> Duration {
    let state = ClusterState::of(factory);
    let text = factory.conf_setting("cluster-member", "lone-check-seconds");
    match lone_check_seconds(&text) {
        Ok(every) => {
            state.lone_setting_warned.store(false, Ordering::Relaxed);
            every
        }
        Err(err) => {
            if !state.lone_setting_warned.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    "{err}; a member that is alone reads the member table every {} s",
                    LONE_CHECK_DEFAULT.as_secs()
                );
            }
            LONE_CHECK_DEFAULT
        }
    }
}

/// Whether the cluster port follows the cluster (`cluster-member@listen="auto"`, which a conf that leaves it out
/// means): under `on` or `off` nothing waits on a second member, so a member alone does not read the table for one.
fn listens_auto(factory: &ExecutionContextFactory) -> bool {
    factory
        .conf_setting_if_set("cluster-member", "listen")
        .is_none_or(|listen| listen == "auto")
}

/// `cluster-member@lone-check-seconds` as written: a whole number of seconds, 1 or more; empty is the default.
///
/// # Errors
///
/// When it is neither.
pub(super) fn lone_check_seconds(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(LONE_CHECK_DEFAULT);
    }
    let refused =
        || format!("cluster-member@lone-check-seconds \"{text}\" is not a whole number of seconds of 1 or more");
    let seconds: u64 = text.parse().map_err(|_| refused())?;
    if seconds == 0 {
        return Err(refused());
    }
    Ok(Duration::from_secs(seconds))
}

/// A short check between heartbeats ([`wake_every`]): while this member is alone under `auto`, one read of the member
/// table (plans/Runtime-Config.md §6: no extra database connection on a quiet single server), where a live member found
/// is a cluster; then a resync, which sends a peer marked as having missed messages its clear of every entity cache
/// (a newly seen one, or one whose posts failed and whose backoff is over), and does nothing when none is.
pub(super) fn short_check(factory: &ExecutionContextFactory) {
    let state = ClusterState::of(factory);
    if state.instance_id().is_empty() {
        return;
    }
    let alone = !lock(&state.peers).cluster;
    if alone && listens_auto(factory) {
        super::refresh_peers(&state, factory);
    }
    request_resync(&state);
}
