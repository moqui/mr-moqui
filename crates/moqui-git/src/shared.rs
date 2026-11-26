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

//! Repositories on disk in a cluster (docs/interfaces/Git-Hosting.md, "Shared storage in a cluster"). A `local` or
//! `remote` repository is a directory of one node, so in a cluster (and behind a front or edge node that balances over
//! its members) it needs storage every member mounts: the conf's `git-server` `shared-storage`. Without it, a node that
//! is a cluster member refuses to make a new one (`GitNeedsSharedStorage`) and names the ones it has at start; with it
//! the writers of one such repository take its lease ([`crate::lease`]) as well as the lock of this process, so two
//! members never write it at once, and the pass of mirror fetches runs on one member at a time.
//!
//! What a node keeps in memory of an on-disk repository is checked against the disk on use: gix reads the loose refs
//! and `packed-refs` (by its modification time) at each read and looks for new packs on a miss, the path indexes are
//! brought to the branch head read then, and the reach sets read the tips again within seconds. The `GitRepository`
//! row of an open repository is forgotten when another member's write of it reaches this one as the cluster's entity
//! cache invalidation ([`GitProvider::listen_for_row_changes`]).

use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use super::lease::{self, Lease};
use super::{text, GitProvider, Inner, RepositoryKind, GIT_REPOSITORY};
use moqui_core::context::framework_thread;
use moqui_core::context::{Error, ExecutionContextFactory};

/// How long a node's answer to "am I a cluster member" is kept.
const CLUSTERED_FRESH: Duration = Duration::from_secs(30);

/// Whether the conf says `runtime/git` (and every `localPath`) is on storage every cluster member mounts: of the
/// current context's factory, else of `factory` (a provider's, read on a thread with no context entered).
pub(crate) fn shared_storage_of(factory: Option<&ExecutionContextFactory>) -> bool {
    let setting = moqui_core::context::ExecutionContext::with_current(|ec| {
        ec.factory().conf_setting("git-server", "shared-storage")
    })
    .or_else(|| factory.map(|factory| factory.conf_setting("git-server", "shared-storage")))
    .unwrap_or_default();
    setting == "true"
}

/// Whether `factory`'s node is a member of a cluster: another member's heartbeat is live, or the node is set up to be
/// one (`cluster-member@base-url`, `cluster-member@transport`).
pub(crate) fn is_cluster_member(factory: &ExecutionContextFactory) -> bool {
    member_answer(factory).unwrap_or(false)
}

/// Whether `factory`'s node is a cluster member, or `None` when that could not be read (the members' rows).
fn member_answer(factory: &ExecutionContextFactory) -> Option<bool> {
    let configured = ["cluster-member@base-url", "cluster-member@transport"]
        .iter()
        .any(|selector| !moqui_core::context::conf_env::setting_on(factory, selector).is_empty());
    if configured {
        return Some(true);
    }
    let facade = factory.entity()?;
    let own = moqui_core::cluster::instance_id(factory);
    match moqui_core::cluster::member_report(factory, &facade) {
        Ok(members) => Some(
            members
                .iter()
                .any(|member| member.live && Some(&member.instance_id) != own.as_ref()),
        ),
        Err(err) => {
            tracing::warn!("the cluster members were not read: {err}");
            None
        }
    }
}

/// The refusal of a new on-disk repository on a cluster member without shared storage.
fn needs_shared_storage(repository_id: &str) -> Error {
    Error::new(format!(
        "GitNeedsSharedStorage: git repository {repository_id} would be kept on this node's disk (kind local or \
         remote), and this node is a cluster member whose other members could not reach it: put runtime/git on \
         storage every member mounts and set git-server shared-storage=\"true\", or use kind db"
    ))
}

impl GitProvider {
    /// Remember the factory this provider is the `git` scheme of, for reads with no context entered.
    pub(crate) fn set_factory(&self, factory: &Arc<ExecutionContextFactory>) {
        let _ = self.inner.factory.set(Arc::downgrade(factory));
    }

    fn factory(&self) -> Option<Arc<ExecutionContextFactory>> {
        self.inner.factory.get().and_then(Weak::upgrade)
    }

    /// [`shared_storage_of`] this provider's factory.
    pub(crate) fn shared_storage(&self) -> bool {
        shared_storage_of(self.factory().as_deref())
    }

    /// Whether this provider's node is a cluster member, read again at most every [`CLUSTERED_FRESH`], by the current
    /// context's factory or else this provider's. An answer that could not be read is not kept, and a node once seen
    /// to be a member stays one: a member whose peers' heartbeats are all down (a restart of the others, a lost
    /// database connection) is still a member whose disk the others share nothing of.
    pub(crate) fn clustered(&self) -> bool {
        use std::sync::atomic::Ordering;
        if self.inner.was_clustered.load(Ordering::Acquire) {
            return true;
        }
        let mut known = moqui_lib::sync::lock(&self.inner.clustered);
        if let Some((at, answer)) = *known {
            if at.elapsed() < CLUSTERED_FRESH {
                return answer;
            }
        }
        let answer = moqui_core::context::ExecutionContext::with_current(|ec| member_answer(ec.factory()))
            .or_else(|| self.factory().map(|factory| member_answer(&factory)))
            .flatten();
        let Some(answer) = answer else {
            // not known now: the last answer, if any, and nothing kept
            return known.is_some_and(|(_, answer)| answer);
        };
        if answer {
            self.inner.was_clustered.store(true, Ordering::Release);
        }
        *known = Some((Instant::now(), answer));
        answer
    }

    /// Forget what [`Self::clustered`] answered, for a test that makes this node a member.
    #[cfg(test)]
    pub(crate) fn forget_clustered(&self) {
        *moqui_lib::sync::lock(&self.inner.clustered) = None;
        self.inner
            .was_clustered
            .store(false, std::sync::atomic::Ordering::Release);
    }

    /// May a new repository directory of `repository_id` be made on this node: not on a cluster member without shared
    /// storage.
    ///
    /// # Errors
    ///
    /// `GitNeedsSharedStorage` on such a node.
    pub(crate) fn may_make_on_disk(&self, repository_id: &str) -> Result<(), Error> {
        if !self.shared_storage() && self.clustered() {
            return Err(needs_shared_storage(repository_id));
        }
        Ok(())
    }

    /// The lease of the writers of an on-disk repository on shared storage, taken after the lock of this process
    /// ([`Self::lock_of`]) and held as long; `None` for a `db` repository, or with no shared storage.
    ///
    /// # Errors
    ///
    /// `GitLeaseBusy` when another member writes it past `git-server` `shared-lock-wait-seconds`.
    pub(crate) fn shared_turn(&self, repository_id: &str, kind: RepositoryKind) -> Result<Option<Lease>, Error> {
        if kind == RepositoryKind::Db || !self.shared_storage() {
            return Ok(None);
        }
        self.repository_lease(repository_id).map(Some)
    }

    /// The repository's lease, whatever its kind: the writers of one on shared storage, and a fork against a delete.
    pub(crate) fn repository_lease(&self, repository_id: &str) -> Result<Lease, Error> {
        let wait = Duration::from_secs(super::number_setting(
            "shared-lock-wait-seconds",
            lease::DEFAULT_WAIT.as_secs(),
        ));
        lease::take(
            &self.entity()?,
            &lease::repository_name(repository_id),
            lease::TTL,
            wait,
        )
    }

    /// [`Self::shared_turn`] for a repository known by its id only: its kind read from its row.
    pub(crate) fn shared_turn_of(&self, repository_id: &str) -> Result<Option<Lease>, Error> {
        if !self.shared_storage() {
            return Ok(None);
        }
        let kind = self.definition(repository_id)?.kind;
        self.shared_turn(repository_id, kind)
    }

    /// Forget the open repositories whose rows another cluster member wrote, as the cluster's entity cache
    /// invalidation of `GitRepository` tells it: the key's repository, or every one for a whole-entity or clear-all
    /// message. This process's own writes refresh as they are made.
    pub(crate) fn listen_for_row_changes(&self, factory: &Arc<ExecutionContextFactory>) {
        let weak: Weak<Inner> = Arc::downgrade(&self.inner);
        let listener: moqui_core::notify::Listener = Arc::new(move |message| {
            if message.origin_id == moqui_core::notify::origin_id() {
                return;
            }
            let Some(inner) = weak.upgrade() else { return };
            let provider = GitProvider { inner };
            let body = &message.body;
            let clear_all = body.get("clearAll").and_then(serde_json::Value::as_bool) == Some(true);
            if !clear_all && body.get("entityName").and_then(serde_json::Value::as_str) != Some(GIT_REPOSITORY) {
                return;
            }
            let keys: Vec<&serde_json::Value> = match body.get("primaryKeys").and_then(serde_json::Value::as_array) {
                Some(keys) => keys.iter().collect(),
                None => body.get("primaryKey").into_iter().collect(),
            };
            let ids: Vec<String> = keys
                .iter()
                .filter_map(|key| key.get("repositoryId").and_then(serde_json::Value::as_str))
                .map(str::to_string)
                .collect();
            if clear_all || ids.is_empty() || ids.len() != keys.len() {
                provider.forget_all_open();
            } else {
                for id in ids {
                    provider.refresh(&id);
                }
            }
        });
        match moqui_core::notify::subscribe(moqui_core::notify::ENTITY_CACHE_INVALIDATE, listener) {
            Ok(subscription) => {
                factory.extension(move |_| RowChanges {
                    _subscription: Some(subscription),
                });
            }
            Err(err) => tracing::warn!("git repository rows changed on other cluster members are not seen: {err}"),
        }
    }

    /// Forget every open repository.
    pub(crate) fn forget_all_open(&self) {
        let mut changing = moqui_lib::sync::lock(&self.inner.changing);
        changing.forgets += 1;
        moqui_lib::sync::write(&self.inner.repos).clear();
    }

    /// The on-disk repositories of a cluster member without shared storage, each named in a warning: the other members
    /// cannot reach them, and copies of one would differ. They are left as they are (the node starts); a new one is
    /// refused.
    pub(crate) fn warn_unshared(&self, factory: &ExecutionContextFactory) -> Vec<String> {
        if factory.conf_setting("git-server", "shared-storage") == "true" || !is_cluster_member(factory) {
            return Vec::new();
        }
        let _rows = super::rows_guard();
        let rows = match self
            .entity()
            .and_then(|entity| Ok(entity.find(GIT_REPOSITORY)?.disable_authz().list()?))
        {
            Ok(rows) => rows,
            Err(err) => {
                tracing::error!("the git repositories were not read for the shared storage check: {err}");
                return Vec::new();
            }
        };
        let mut named = Vec::new();
        for row in rows.iter() {
            let Some(id) = text(row.get("repositoryId")) else {
                continue;
            };
            let kind = text(row.get("repositoryKind")).unwrap_or_default();
            if kind == "db" {
                continue;
            }
            tracing::warn!(
                repository = %id,
                "git repository {id} is kept on this node's disk (kind {}), and this node is a cluster member without \
                 git-server shared-storage: the other members cannot reach it. Put runtime/git on shared storage and \
                 set shared-storage=\"true\", or move it to kind db",
                if kind.is_empty() { "local" } else { kind.as_str() }
            );
            named.push(id);
        }
        named
    }

    /// Check for [`Self::warn_unshared`] once this node has joined its cluster: on a thread of its own, after the
    /// entity facade is installed and the cluster started (or a while has passed).
    pub(crate) fn warn_unshared_after_start(&self, factory: &Arc<ExecutionContextFactory>) {
        let weak = Arc::downgrade(factory);
        let provider = self.clone();
        let spawned = std::thread::Builder::new()
            .name("git-shared-check".into())
            .spawn(framework_thread(move || {
                for _ in 0..120 {
                    std::thread::sleep(Duration::from_secs(1));
                    let Some(factory) = weak.upgrade() else { return };
                    if factory.entity().is_some() && moqui_core::cluster::instance_id(&factory).is_some() {
                        break;
                    }
                }
                let Some(factory) = weak.upgrade() else { return };
                if factory.entity().is_none() {
                    return;
                }
                let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let context = factory.new_context();
                    let _entered = context.enter();
                    provider.warn_unshared(&factory);
                }));
                if checked.is_err() {
                    tracing::error!("the git shared storage check at start panicked");
                }
            }));
        if let Err(err) = spawned {
            tracing::warn!("the git shared storage check at start did not run: {err}");
        }
    }
}

/// The subscription of a factory's provider to row changes, kept as long as the factory.
#[derive(Default)]
struct RowChanges {
    _subscription: Option<moqui_core::notify::Subscription>,
}

#[cfg(test)]
#[path = "shared_tests.rs"]
mod tests;
