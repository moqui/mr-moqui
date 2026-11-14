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

//! Forks and the heads of merge requests (docs/interfaces/Git-Hosting.md, Forks and Merge requests; plans/Dogfood.md
//! F4, F5). A fork is a repository of the same kind as its parent, in the parent's fork network
//! (`GitRepository.forkNetworkId`, the root's id), with refs of its own:
//!
//! - a `db` fork has no object rows of its own: the network's objects are the rows of the root's id
//!   ([`crate::db::DbStore::in_network`]), so rows written before a repository had forks stay as they are, and a
//!   fork's new objects are marked with it (`addedRepositoryId`) for its usage;
//! - a `local` fork reads its parent's objects through `objects/info/alternates` and keeps its new ones in its own
//!   directory, so a merge request from it copies them to its target.
//!
//! What a repository's readers read by id is decided by its refs ([`crate::reach`]), never by what its store holds.

use std::collections::HashSet;

use gix::ObjectId;

use super::store::{RefEdit, RefOutcome};
use super::{fs, rows_guard, text, GitProvider, RepositoryKind, GIT_REPOSITORY};
use moqui_core::context::Error;
use moqui_core::entity::{Condition, EntityValue};
use moqui_lib::object::Value;

const GIT_LFS_OBJECT: &str = "moqui.resource.GitLfsObject";

/// The refs a fork starts with: its parent's branches and tags as they are now.
const FORKED: [&str; 2] = ["refs/heads/", "refs/tags/"];

impl GitProvider {
    /// The repositories of the fork network `network`, its root among them, by id.
    pub(crate) fn network_members(&self, network: &str) -> Result<Vec<String>, Error> {
        let _rows = rows_guard();
        let rows = self
            .entity()?
            .find(GIT_REPOSITORY)?
            .eq("forkNetworkId", Value::string(network))
            .select_field("repositoryId")
            .list()?;
        Ok(rows.iter().filter_map(|row| text(row.get("repositoryId"))).collect())
    }

    /// The repositories forked from `repository_id`, and those of the network it is the root of.
    pub(crate) fn forks_of(&self, repository_id: &str) -> Result<Vec<String>, Error> {
        let _rows = rows_guard();
        let rows = self
            .entity()?
            .find(GIT_REPOSITORY)?
            .condition(Condition::or(vec![
                Condition::equals("forkOfRepositoryId", Value::string(repository_id)),
                Condition::equals("forkNetworkId", Value::string(repository_id)),
            ]))
            .select_field("repositoryId")
            .list()?;
        Ok(rows
            .iter()
            .filter_map(|row| text(row.get("repositoryId")))
            .filter(|id| id != repository_id)
            .collect())
    }

    /// Make `repository_id` a fork of `source_id`: a repository of the same kind in the source's fork network (the
    /// source becomes a network's root when it is in none), with the source's branches and tags as its own refs, its
    /// default branch, and the rows of its LFS objects (the content is stored once). A `local` fork's directory is
    /// `local_path` (as `GitRepository.localPath`), else `runtime/git/<repositoryId>`. How many refs it has.
    ///
    /// # Errors
    ///
    /// A source that is a `remote` mirror, and an id that is already a repository's.
    pub(crate) fn fork(
        &self,
        source_id: &str,
        repository_id: &str,
        description: Option<&str>,
        local_path: Option<&str>,
    ) -> Result<usize, Error> {
        let source = self.repo(source_id)?;
        if source.definition.kind == RepositoryKind::Remote {
            return Err(Error::new(format!(
                "git repository {source_id} is a mirror of another repository, and is not forked"
            )));
        }
        let entity = self.entity()?;
        let network = source.definition.objects_id().to_string();
        {
            let _rows = rows_guard();
            if entity
                .find(GIT_REPOSITORY)?
                .eq("repositoryId", Value::string(repository_id))
                .one()?
                .is_some()
            {
                return Err(Error::new(format!(
                    "GitRepositoryExists: git repository {repository_id} is there already"
                )));
            }
            if source.definition.network.is_none() {
                // the source's objects are the network's from now on: for a db one, the rows it has
                let mut row = EntityValue::make(&entity, GIT_REPOSITORY)?;
                row.set("repositoryId", Value::string(source_id));
                row.set("forkNetworkId", Value::string(source_id));
                row.update(&entity)?;
                self.refresh(source_id);
            }
            let mut row = EntityValue::make(&entity, GIT_REPOSITORY)?;
            row.set("repositoryId", Value::string(repository_id));
            row.set(
                "repositoryKind",
                Value::string(match source.definition.kind {
                    RepositoryKind::Db => "db",
                    _ => "local",
                }),
            );
            row.set("branch", Value::string(&source.definition.branch));
            if let (RepositoryKind::Local, Some(path)) = (source.definition.kind, local_path) {
                row.set("localPath", Value::string(path));
            }
            row.set("description", description.map_or(Value::Null, Value::string));
            row.set("forkOfRepositoryId", Value::string(source_id));
            row.set("forkNetworkId", Value::string(&network));
            row.create(&entity)?;
        }
        self.refresh(repository_id);
        let mut refs = Vec::new();
        for prefix in FORKED {
            refs.extend(source.store.list_refs(prefix)?);
        }
        let fork = self.definition(repository_id)?;
        if fork.kind == RepositoryKind::Local {
            if fork.path.join("HEAD").exists() {
                return Err(Error::new(format!(
                    "the directory {} of the new repository is a repository already",
                    fork.path.display()
                )));
            }
            fs::FsStore::init(&fork.path, &fork.branch)?;
            // the parent's objects, read where they are; the fork's new ones go in its own directory
            let objects = std::fs::canonicalize(source.definition.path.join("objects")).map_err(|err| {
                Error::new(format!(
                    "the objects of git repository {source_id} at {}: {err}",
                    source.definition.path.display()
                ))
            })?;
            let alternates = fork.path.join("objects").join("info").join("alternates");
            if let Some(parent) = alternates.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|err| Error::new(format!("cannot create {}: {err}", parent.display())))?;
            }
            std::fs::write(&alternates, format!("{}\n", objects.display()))
                .map_err(|err| Error::new(format!("cannot write {}: {err}", alternates.display())))?;
        }
        let opened = self.open(fork)?;
        let edits: Vec<RefEdit> = refs
            .iter()
            .map(|(name, id)| RefEdit {
                name: name.clone(),
                expected: None,
                new: Some(*id),
            })
            .collect();
        if !edits.is_empty() {
            let outcomes = opened.store.update_refs(&edits, true)?;
            if outcomes.iter().any(|outcome| *outcome != RefOutcome::Updated) {
                return Err(Error::new(format!(
                    "the refs of the new git repository {repository_id} were not written"
                )));
            }
        }
        let head = opened
            .store
            .read_ref(&super::ops::branch_ref(&opened.definition.branch))?;
        self.remember_head(repository_id, head, None);
        self.copy_lfs_rows(source_id, repository_id)?;
        self.refresh(repository_id);
        Ok(refs.len())
    }

    /// Give `to` the LFS objects of `from`: rows only, as the content is a content blob stored once.
    fn copy_lfs_rows(&self, from: &str, to: &str) -> Result<(), Error> {
        let _rows = rows_guard();
        let entity = self.entity()?;
        if !entity.table_ready(GIT_LFS_OBJECT) {
            return Ok(());
        }
        let rows = entity
            .find(GIT_LFS_OBJECT)?
            .eq("repositoryId", Value::string(from))
            .list()?;
        for row in rows.iter() {
            let mut copy = EntityValue::make(&entity, GIT_LFS_OBJECT)?;
            for field in ["oid", "objectSize", "uploadedByUserId", "uploadedDate"] {
                copy.set(field, row.get(field).cloned().unwrap_or(Value::Null));
            }
            copy.set("repositoryId", Value::string(to));
            copy.create(&entity)?;
        }
        Ok(())
    }

    /// Bring the commit `head` of repository `from` and what it reaches to repository `to`, when `to` does not have
    /// it: nothing for two `db` repositories of one network, a copy as a push makes it between directories. How
    /// many objects were copied.
    pub(crate) fn share_objects(&self, from: &str, to: &str, head: ObjectId) -> Result<usize, Error> {
        if from == to {
            return Ok(0);
        }
        let (source, target) = (self.repo(from)?, self.repo(to)?);
        if target.store.object_size(&head)?.is_some() {
            return Ok(0);
        }
        // what both have: the target's branches the source can read (a fork reads its parent's objects as they were)
        let tips: Vec<ObjectId> = target
            .store
            .list_refs("refs/heads/")?
            .into_iter()
            .map(|(_, id)| id)
            .collect();
        let known: HashSet<ObjectId> = source.store.contains_many(&tips)?;
        let haves: Vec<ObjectId> = tips.into_iter().filter(|id| known.contains(id)).collect();
        let lock = self.lock_of(to);
        let copied = {
            let _turn = moqui_lib::sync::lock(&lock);
            self.still_writable(to)?;
            super::server::copy_objects(
                source.store.as_ref(),
                target.store.as_ref(),
                head,
                &haves,
                &self.scratch("merge-request", to),
            )?
        };
        if target.definition.kind != RepositoryKind::Db {
            // gix sizes its pack slots when it opens: the new pack is read by a fresh open
            self.refresh(to);
        }
        Ok(copied)
    }

    /// Point `refs/merge-requests/<num>/head` of a repository at `head`, whatever it pointed at: the ref is the
    /// merge request's alone, moved by its services and by no push, so it makes no `GitUpdate` and runs no rule.
    pub(crate) fn set_merge_request_head(&self, repository_id: &str, num: i64, head: ObjectId) -> Result<(), Error> {
        let repo = self.repo(repository_id)?;
        let name = super::reach::merge_request_ref(num);
        let lock = self.lock_of(repository_id);
        let _turn = moqui_lib::sync::lock(&lock);
        self.still_writable(repository_id)?;
        for _ in 0..4 {
            let current = repo.store.read_ref(&name)?;
            if current == Some(head) {
                return Ok(());
            }
            let outcomes = repo.store.update_refs(
                &[RefEdit {
                    name: name.clone(),
                    expected: current,
                    new: Some(head),
                }],
                true,
            )?;
            if outcomes.first() == Some(&RefOutcome::Updated) {
                return Ok(());
            }
        }
        Err(Error::new(format!(
            "{name} of {repository_id} kept moving; it was not set"
        )))
    }

    /// The object rows a deleted fork added become the root's, so a usage counts them to a repository that is
    /// there until a collection lets go of those no ref reaches.
    pub(crate) fn disown_objects(&self, network: &str, repository_id: &str) -> Result<(), Error> {
        let _rows = rows_guard();
        let entity = self.entity()?;
        if !entity.table_ready(super::db::GIT_OBJECT) {
            return Ok(());
        }
        entity
            .find(super::db::GIT_OBJECT)?
            .eq("repositoryId", Value::string(network))
            .eq("addedRepositoryId", Value::string(repository_id))
            .update_all([("addedRepositoryId".to_string(), Value::Null)])?;
        Ok(())
    }

    /// The ids a collection of repository `repository_id` must also keep: what the refs of the other repositories of
    /// its fork network point to, and the ids of their changes in progress and of the reflog window, as
    /// [`GitProvider::gc_roots`] reads them for one repository. They share its objects (a `db` network's rows, a
    /// `local` parent's directory through alternates). Empty for a repository in no network.
    pub(crate) fn network_roots(
        &self,
        repository_id: &str,
        network: Option<&str>,
        options: &super::gc::GcOptions,
    ) -> Result<super::gc::Roots, Error> {
        let mut roots = super::gc::Roots {
            refs: Vec::new(),
            others: Vec::new(),
        };
        let Some(network) = network else {
            return Ok(roots);
        };
        let entity = self.entity()?;
        for member in self.network_members(network)? {
            if member == repository_id {
                continue;
            }
            // a member that does not open stops the run: its refs are not known, and what they reach must stay
            let repo = self.repo(&member).map_err(|err| {
                Error::new(format!(
                    "git repository {member} of the fork network {network} cannot be opened, so the network is not \
                     collected: {}",
                    err.message()
                ))
            })?;
            let found = self.gc_roots(&entity, &member, repo.store.as_ref(), options)?;
            roots.refs.extend(found.refs);
            roots.others.extend(found.others);
        }
        Ok(roots)
    }
}
