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

//! What a repository stores, and removing it (`get#GitRepositoryUsage`, `delete#GitRepository`; plans/Dogfood.md,
//! N-USAGE and N-DELETE). An application's quotas read usage here, so it never reads the content group's rows
//! (`GitObject`, `GitLfsObject`); the numbers are read from the rows and the directory as they are now, so they
//! are current right after a push. Callers check `AT_GIT_REPOSITORY` authorization; nothing here does.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use moqui_lib::object::Value;

use super::db::{GIT_OBJECT, GIT_REF};
use super::server::lfs::{GIT_LFS_LOCK, GIT_LFS_OBJECT};
use super::server::path::GIT_PATH_REDIRECT;
use super::{long, rows_guard, text, GitProvider, RepositoryKind, GIT_REPOSITORY};
use moqui_core::context::Error;
use moqui_core::entity::{Condition, EntityFacade, EntityValue};
use moqui_core::transaction::TransactionAttribute;

const GIT_OBJECT_USAGE: &str = "moqui.resource.GitObjectUsage";
const GIT_LFS_OBJECT_USAGE: &str = "moqui.resource.GitLfsObjectUsage";
const GIT_UPDATE: &str = "moqui.resource.GitUpdate";
const GIT_UPDATE_REF: &str = "moqui.resource.GitUpdateRef";
const GIT_COMMIT_STATUS: &str = "moqui.resource.GitCommitStatus";
const GIT_WEBHOOK: &str = "moqui.resource.GitWebhook";
const GIT_BRANCH_RULE: &str = "moqui.resource.GitBranchRule";
const GIT_MERGE_REQUEST: &str = "moqui.resource.GitMergeRequest";
const GIT_MERGE_REQUEST_REVIEWER: &str = "moqui.resource.GitMergeRequestReviewer";
const GIT_COMMIT_PUSH: &str = "moqui.resource.GitCommitPush";
const RESOURCE_DRIVE: &str = "moqui.resource.ResourceDrive";

/// Ids in one `IN` list.
const ID_BATCH: usize = 500;

/// What one repository stores.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Usage {
    pub repository_id: String,
    /// `remote`, `local` or `db`.
    pub kind: &'static str,
    /// The bytes of its git objects: a `db` repository's rows as stored (compressed, deltas as deltas) with the
    /// content blobs of its large objects; the files of a repository on disk.
    pub stored_bytes: u64,
    /// The git objects of a `db` repository; `None` on disk, where counting them means reading every pack.
    pub object_count: Option<u64>,
    /// The bytes of its Git LFS objects.
    pub lfs_bytes: u64,
    pub lfs_object_count: u64,
}

/// What [`GitProvider::delete_repository`] did with a repository's directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Directory {
    /// A `db` repository has none.
    None,
    /// Removed, or to be when the transaction commits: the directory, its links followed.
    Removed(PathBuf),
    /// Left where it is: outside the runtime directory, not a bare repository, or shared with another repository.
    Kept(PathBuf),
}

fn as_u64(value: Option<&Value>) -> u64 {
    long(value).and_then(|value| u64::try_from(value).ok()).unwrap_or(0)
}

/// The bytes of the files under `dir`; a file or directory that cannot be read counts as nothing. No recursion: a
/// directory on disk is as deep as whoever wrote there made it.
fn directory_bytes(dir: &Path) -> u64 {
    let mut bytes = 0u64;
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            // a link is not followed: a repository that borrows another's objects does not count them
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => pending.push(entry.path()),
                Ok(kind) if kind.is_file() => {
                    bytes = bytes.saturating_add(entry.metadata().map_or(0, |meta| meta.len()));
                }
                _ => {}
            }
        }
    }
    bytes
}

/// Delete the rows of `entity_name` that match, in one statement: these tables are not cached, and a repository's
/// objects are far too many to read and delete one by one.
fn delete_where(entity: &EntityFacade, entity_name: &str, condition: &Condition) -> Result<u64, Error> {
    if !entity.table_ready(entity_name) {
        return Ok(0);
    }
    let tx = entity.transactions_for(entity_name)?;
    let scope = tx.scope(None)?;
    let statement = entity.with_entity(entity_name, |definition, dialect, _| {
        moqui_db::sql::delete(definition, dialect, Some(condition))
    })?;
    let deleted = moqui_core::entity::exec::execute_tx(&tx, &statement.sql, &statement.params)?;
    scope.commit()?;
    Ok(deleted)
}

/// The rows of a repository id in the content group and the hosting tables: every table but the repository's own
/// row and the redirects of its paths.
fn delete_rows_of(entity: &EntityFacade, repository_id: &str) -> Result<(), Error> {
    let of_repository = Condition::equals("repositoryId", Value::string(repository_id));
    // the content group: refs first, so nothing reaches the objects while they go
    delete_where(entity, GIT_REF, &of_repository)?;
    let self_references = entity.with_entity(GIT_OBJECT, |_, dialect, _| Ok(moqui_db::sql::is_mysql(dialect)))?;
    if self_references && entity.table_ready(GIT_OBJECT) {
        // MySQL checks the delta base's foreign key a row at a time: a delta goes before its base
        let deepest = entity
            .find(GIT_OBJECT)?
            .eq("repositoryId", Value::string(repository_id))
            .condition(Condition::greater("deltaDepth", Value::I64(0)))
            .select_field("deltaDepth")
            .order_by("deltaDepth", true)
            .limit(1)
            .list()?;
        let deepest = deepest
            .iter()
            .next()
            .and_then(|row| long(row.get("deltaDepth")))
            .unwrap_or(0);
        for depth in (1..=deepest).rev() {
            let at_depth = Condition::and(vec![
                of_repository.clone(),
                Condition::equals("deltaDepth", Value::I64(depth)),
            ]);
            delete_where(entity, GIT_OBJECT, &at_depth)?;
        }
    }
    delete_where(entity, GIT_OBJECT, &of_repository)?;
    delete_where(entity, GIT_LFS_OBJECT, &of_repository)?;
    // the hosting rows, children before the rows they belong to
    for (child, parent, key) in [
        (GIT_UPDATE_REF, GIT_UPDATE, "updateId"),
        (GIT_MERGE_REQUEST_REVIEWER, GIT_MERGE_REQUEST, "mergeRequestId"),
    ] {
        let ids = ids_of(entity, parent, repository_id, key)?;
        for chunk in ids.chunks(ID_BATCH) {
            delete_where(entity, child, &Condition::is_in(key, chunk.to_vec()))?;
        }
    }
    for name in [
        GIT_UPDATE,
        GIT_MERGE_REQUEST,
        GIT_COMMIT_STATUS,
        GIT_WEBHOOK,
        GIT_BRANCH_RULE,
        GIT_LFS_LOCK,
        GIT_COMMIT_PUSH,
    ] {
        delete_where(entity, name, &of_repository)?;
    }
    Ok(())
}

/// `path` with its links and `..` followed when it is there; else as it is written, under `base` when relative: what
/// two paths are compared by.
fn resolved(path: &Path, base: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            base.join(path)
        }
    })
}

/// The values of `key` in the rows of `entity_name` of a repository.
fn ids_of(entity: &EntityFacade, entity_name: &str, repository_id: &str, key: &str) -> Result<Vec<Value>, Error> {
    if !entity.table_ready(entity_name) {
        return Ok(Vec::new());
    }
    let rows = entity
        .find(entity_name)?
        .eq("repositoryId", Value::string(repository_id))
        .select_field(key)
        .list()?;
    Ok(rows.iter().filter_map(|row| row.get(key).cloned()).collect())
}

impl GitProvider {
    /// What each of `repository_ids` stores now, in their order; an id with no row is left out.
    pub(crate) fn usage(&self, repository_ids: &[String]) -> Result<Vec<Usage>, Error> {
        let _rows = rows_guard();
        let entity = self.entity()?;
        // one that is not there, or whose row does not read, is left out; the others are still answered
        let definitions: Vec<super::Definition> = repository_ids
            .iter()
            .filter_map(|id| self.definition(id).ok())
            .collect();
        // the object rows of a fork network are the root's: a fork's are those it added, the root's the others
        let mut stores: Vec<String> = definitions
            .iter()
            .filter(|definition| definition.kind == RepositoryKind::Db)
            .map(|definition| definition.objects_id().to_string())
            .collect();
        stores.sort();
        stores.dedup();
        let mut objects: HashMap<(String, Option<String>), (u64, u64)> = HashMap::new();
        let mut lfs: HashMap<String, (u64, u64)> = HashMap::new();
        for chunk in stores.chunks(ID_BATCH) {
            let ids: Vec<Value> = chunk.iter().map(Value::string).collect();
            if entity.table_ready(GIT_OBJECT) {
                let rows = entity
                    .find(GIT_OBJECT_USAGE)?
                    .condition(Condition::is_in("repositoryId", ids))
                    .list()?;
                for row in rows.iter() {
                    let found = objects
                        .entry((
                            text(row.get("repositoryId")).unwrap_or_default(),
                            text(row.get("addedRepositoryId")),
                        ))
                        .or_default();
                    found.0 += as_u64(row.get("objectCount"));
                    // an object kept as a content blob has no data in its row: its content is what is stored
                    found.1 += match text(row.get("storage")).as_deref() {
                        Some("blob") => as_u64(row.get("objectBytes")),
                        _ => as_u64(row.get("storedBytes")),
                    };
                }
            }
        }
        for chunk in repository_ids.chunks(ID_BATCH) {
            let ids: Vec<Value> = chunk.iter().map(Value::string).collect();
            if entity.table_ready(GIT_LFS_OBJECT) {
                let rows = entity
                    .find(GIT_LFS_OBJECT_USAGE)?
                    .condition(Condition::is_in("repositoryId", ids))
                    .list()?;
                for row in rows.iter() {
                    let found = lfs
                        .entry(text(row.get("repositoryId")).unwrap_or_default())
                        .or_default();
                    found.0 += as_u64(row.get("objectCount"));
                    found.1 += as_u64(row.get("lfsBytes"));
                }
            }
        }
        let mut usages = Vec::with_capacity(repository_ids.len());
        for definition in &definitions {
            let id = &definition.id;
            let (lfs_object_count, lfs_bytes) = lfs.get(id).copied().unwrap_or_default();
            let (kind, stored_bytes, object_count) = match definition.kind {
                RepositoryKind::Db => {
                    let key = (
                        definition.objects_id().to_string(),
                        definition.is_fork().then(|| id.clone()),
                    );
                    let (count, bytes) = objects.get(&key).copied().unwrap_or_default();
                    ("db", bytes, Some(count))
                }
                RepositoryKind::Local => ("local", directory_bytes(&definition.path), None),
                RepositoryKind::Remote => ("remote", directory_bytes(&definition.path), None),
            };
            usages.push(Usage {
                repository_id: id.clone(),
                kind,
                stored_bytes,
                object_count,
                lfs_bytes,
                lfs_object_count,
            });
        }
        Ok(usages)
    }

    /// The directory of a repository on disk as a delete may remove it, else why it stays ([`Directory::Kept`]). It is
    /// removed only when all of this holds, each read from the disk and the rows as they are, not from the path's
    /// spelling: it is there; followed through its links and `..` it is under the runtime directory and is not that
    /// directory; it looks like a bare repository (a `HEAD` file and an `objects` directory), so a `localPath` that
    /// names something else removes nothing; and no other repository's row names it, a directory in it, or one it is
    /// in. A `localPath` can name anything, a developer's own clone too.
    fn removable_directory(&self, entity: &EntityFacade, repository_id: &str, path: &Path) -> Result<Directory, Error> {
        let Ok(target) = std::fs::canonicalize(path) else {
            return Ok(Directory::None);
        };
        let kept = || Ok(Directory::Kept(path.to_path_buf()));
        let Ok(runtime) = std::fs::canonicalize(&self.inner.runtime_dir) else {
            return kept();
        };
        if target == runtime || !target.starts_with(&runtime) {
            return kept();
        }
        if !target.join("HEAD").is_file() || !target.join("objects").is_dir() {
            return kept();
        }
        let others = entity
            .find(GIT_REPOSITORY)?
            .select_field("repositoryId")
            .select_field("localPath")
            .list()?;
        for row in others.iter() {
            let other_id = text(row.get("repositoryId")).unwrap_or_default();
            if other_id == repository_id {
                continue;
            }
            let other = resolved(&self.path_of(&other_id, text(row.get("localPath"))), &runtime);
            if other.starts_with(&target) || target.starts_with(&other) {
                return kept();
            }
        }
        Ok(Directory::Removed(target))
    }

    /// Mark a repository as going, in a transaction of its own that commits now: its `archivedDate` is set, which
    /// every push and write reads again under the repository's lock before it stores anything
    /// ([`GitProvider::still_writable`]), on every node. So nothing is written to a repository while its rows go, or
    /// after. True when this set it (an archived repository refuses writes already): a delete that rolls back sets
    /// it back. False too when the row is held by a transaction not ended, the caller's own when it wrote the row:
    /// the mark would wait for that transaction for ever, so the delete goes on without it and its second pass
    /// ([`GitProvider::delete_left_rows`]) takes what a write then leaves.
    fn mark_going(&self, entity: &EntityFacade, repository_id: &str) -> Result<bool, Error> {
        let transactions = entity.transactions_for(GIT_REPOSITORY)?;
        transactions
            .run(TransactionAttribute::ForceNew, None, || {
                let row = entity
                    .find(GIT_REPOSITORY)?
                    .eq("repositoryId", Value::string(repository_id))
                    .for_update(true)
                    .skip_locked(true)
                    .one()?;
                let Some(mut row) = row else {
                    return Ok(false);
                };
                if text(row.get("archivedDate")).is_some() {
                    return Ok(false);
                }
                row.set("archivedDate", moqui_core::user::now_value());
                row.update(entity)?;
                Ok(true)
            })
            .map_err(Error::from)
    }

    /// Set back what [`GitProvider::mark_going`] set, after the delete it was for rolled back.
    fn unmark_going(&self, repository_id: &str) {
        let _rows = rows_guard();
        let cleared = self.entity().and_then(|entity| {
            let transactions = entity.transactions_for(GIT_REPOSITORY)?;
            transactions
                .run(TransactionAttribute::ForceNew, None, || {
                    let mut row = EntityValue::make(&entity, GIT_REPOSITORY)?;
                    row.set("repositoryId", Value::string(repository_id));
                    row.set("archivedDate", Value::Null);
                    row.update(&entity)?;
                    Ok(())
                })
                .map_err(Error::from)
        });
        if let Err(err) = cleared {
            tracing::error!(
                repository = repository_id,
                "a delete of the git repository rolled back and it is still archived (unarchive#GitRepository): {}",
                err.message()
            );
        }
        self.refresh(repository_id);
    }

    /// After a delete committed: the rows a push or write that was already storing left under the id, when the
    /// repository is still gone. Under the repository's lock, so a write of this process has ended.
    pub(crate) fn delete_left_rows(&self, repository_id: &str) {
        let _rows = rows_guard();
        let lock = self.lock_of(repository_id);
        let _writer = moqui_lib::sync::lock(&lock);
        let swept = self.entity().and_then(|entity| {
            let there = entity
                .find(GIT_REPOSITORY)?
                .eq("repositoryId", Value::string(repository_id))
                .one()?
                .is_some();
            if there {
                // an id made again already: its rows are the new repository's
                return Ok(());
            }
            delete_rows_of(&entity, repository_id)
        });
        if let Err(err) = swept {
            tracing::error!(
                repository = repository_id,
                "rows written to the deleted git repository while it went were not removed: {}",
                err.message()
            );
        }
        self.refresh(repository_id);
    }

    /// Whether a push or write may still store into the repository: its row read now, not as it was when the
    /// repository was opened. Asked under the repository's lock before anything is stored.
    ///
    /// # Errors
    ///
    /// A repository deleted since, and one archived since or being deleted ([`GitProvider::mark_going`]).
    pub(crate) fn still_writable(&self, repository_id: &str) -> Result<(), Error> {
        if self.definition(repository_id)?.archived {
            return Err(Error::new(format!(
                "git repository {repository_id} is archived or being deleted, and read only"
            )));
        }
        Ok(())
    }

    /// Delete a repository: its refs and objects (the rows of a `db` one, the directory of one on disk), its LFS
    /// objects and locks, its push log, commit statuses, webhooks, branch rules, merge requests with their
    /// reviewers, the redirects of paths it had, and its row. The content blobs of its large and LFS objects are
    /// left to `gc#ContentBlobs`, as other repositories may hold the same content.
    ///
    /// First the repository is marked as going, committed at once, so no push or write stores into it from then
    /// ([`GitProvider::mark_going`]). The rows then go in the caller's transaction when there is one. Once that
    /// commits the directory is removed ([`GitProvider::removable_directory`] says when it may be) and the rows are
    /// deleted once more, for what a write that was already storing left; a rollback keeps the directory and sets
    /// the mark back.
    ///
    /// # Errors
    ///
    /// A repository that holds a drive (the drive is deleted first), and a row that cannot be deleted, such as one
    /// another component's row still refers to.
    pub(crate) fn delete_repository(&self, repository_id: &str) -> Result<Directory, Error> {
        let _rows = rows_guard();
        let definition = self.definition(repository_id)?;
        let entity = self.entity()?;
        if entity.table_ready(RESOURCE_DRIVE) {
            let drive = entity
                .find(RESOURCE_DRIVE)?
                .eq("repositoryId", Value::string(repository_id))
                .select_field("driveId")
                .limit(1)
                .list()?;
            let drive_id = drive
                .iter()
                .next()
                .map(|drive| text(drive.get("driveId")).unwrap_or_default());
            if let Some(drive_id) = drive_id {
                return Err(Error::new(format!(
                    "git repository {repository_id} holds drive {drive_id}: delete the drive first"
                )));
            }
        }
        let directory = match definition.kind {
            RepositoryKind::Db => Directory::None,
            RepositoryKind::Local | RepositoryKind::Remote => {
                self.removable_directory(&entity, repository_id, &definition.path)?
            }
        };
        let transactions = entity.transactions_for(GIT_REPOSITORY)?;
        let in_transaction = transactions.in_transaction();
        let marked = self.mark_going(&entity, repository_id)?;
        if marked {
            self.refresh(repository_id);
            if in_transaction {
                let (provider, id) = (self.clone(), repository_id.to_string());
                transactions.on_rollback(move || provider.unmark_going(&id))?;
            }
        }
        {
            // no push or write of this process stores while the rows go; one that waited reads the mark next
            let lock = self.lock_of(repository_id);
            let _writer = moqui_lib::sync::lock(&lock);
            // a repository others were forked from keeps their objects (a db network's rows are its own; a local
            // fork reads its directory): it goes after its forks, or stays, archived. Looked for under the mark and
            // the lock, which a fork being made takes too, so none is made after this
            let forks = self.forks_of(repository_id)?;
            if !forks.is_empty() {
                if marked && !in_transaction {
                    self.unmark_going(repository_id);
                }
                let mut shown: Vec<&str> = forks.iter().take(5).map(String::as_str).collect();
                if forks.len() > shown.len() {
                    shown.push("...");
                }
                return Err(Error::new(format!(
                    "GitHasForks: git repository {repository_id} has forks ({}), which share its objects: delete \
                     them first, or archive it",
                    shown.join(", ")
                )));
            }
            // merge requests of other repositories that follow a branch of this one keep the head they have: an id
            // used again must not feed them
            if entity.table_ready(GIT_MERGE_REQUEST) {
                entity
                    .find(GIT_MERGE_REQUEST)?
                    .eq("sourceRepositoryId", Value::string(repository_id))
                    .condition(Condition::is_null("sourceDetachedDate"))
                    .update_all([("sourceDetachedDate".to_string(), moqui_core::user::now_value())])?;
            }
            delete_rows_of(&entity, repository_id)?;
            if entity.table_ready(GIT_PATH_REDIRECT) {
                entity
                    .find(GIT_PATH_REDIRECT)?
                    .eq("repositoryId", Value::string(repository_id))
                    .delete_all()?;
            }
            if definition.kind == RepositoryKind::Db && definition.is_fork() {
                // its objects are the network's rows: they stay, as nobody's, until a collection lets them go
                self.disown_objects(definition.objects_id(), repository_id)?;
            }
            let mut row = EntityValue::make(&entity, GIT_REPOSITORY)?;
            row.set("repositoryId", Value::string(repository_id));
            row.delete(&entity)?;
        }
        self.forget_reached(repository_id);

        // what this process remembers of it: a repository made later with this id starts with nothing
        self.refresh(repository_id);
        moqui_lib::sync::lock(&self.inner.updated).remove(repository_id);
        moqui_lib::sync::lock(&self.inner.indexes).clear();
        if let Some(objects) = self.inner.objects.get() {
            moqui_lib::sync::lock(objects).clear();
        }

        let after = {
            let (provider, id) = (self.clone(), repository_id.to_string());
            let remove = match &directory {
                Directory::Removed(path) => Some(path.clone()),
                Directory::None | Directory::Kept(_) => None,
            };
            move || {
                provider.delete_left_rows(&id);
                let Some(path) = remove else {
                    return;
                };
                match std::fs::remove_dir_all(&path) {
                    Ok(()) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                    Err(err) => tracing::error!(
                        repository = %id,
                        "the directory {} of a deleted git repository was not removed: {err}",
                        path.display()
                    ),
                }
            }
        };
        // only once the rows are gone for good: a rollback keeps the repository, so it keeps its objects
        if in_transaction {
            transactions.on_commit(after)?;
        } else {
            after();
        }
        Ok(directory)
    }
}
