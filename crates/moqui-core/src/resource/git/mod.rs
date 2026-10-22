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

//! `git://<repositoryId>/<path>` locations: files on the configured branch of a git repository, with versions that are
//! the commits that changed each file. No Java counterpart. A `moqui.resource.GitRepository` row defines each
//! repository and its kind:
//!
//! - `remote`: a bare mirror of `remoteUrl` under `localPath`, read only, changed only by `fetch#GitRepository`.
//! - `local`: a bare repository under `localPath` (created on first use); a write is a commit on the branch.
//! - `db`: the objects and refs are `GitObject` and `GitRef` rows, so the database backup holds the repository.
//!
//! A write commits at once, by the current user, with a `Moqui-User-Id` trailer the history reads back. Guide:
//! `docs/Revision-Management.md`.

pub mod attributes;
pub(crate) mod browse;
pub(crate) mod db;
pub(crate) mod delta;
pub(crate) mod diff;
pub(crate) mod events;
pub(crate) mod fs;
pub mod gc;
pub(crate) mod index;
pub(crate) mod lfs_files;
pub(crate) mod hosting;
pub(crate) mod merge;
pub(crate) mod ops;
pub(crate) mod revwalk;
pub(crate) mod rules;
pub mod server;
pub(crate) mod store;
#[cfg(test)]
mod tests;
pub(crate) mod values;

/// An attribute of the conf's `git-server` element in the current context's factory, `${}` expanded and trimmed; empty
/// when there is no current context or the conf leaves it out.
pub(crate) fn setting(name: &str) -> String {
    crate::context::ExecutionContext::with_current(|ec| ec.factory().conf_setting("git-server", name)).unwrap_or_default()
}

/// [`setting`] as a number, or `default` when it is empty or not one.
pub(crate) fn number_setting<T: std::str::FromStr>(name: &str, default: T) -> T {
    setting(name).parse().unwrap_or(default)
}

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use gix::objs::tree::EntryKind;
use gix::objs::Kind;
use gix::ObjectId;
use moqui_lib::object::{basic_convert, Value};

use self::index::BranchIndex;
use self::ops::{Author, Edit};
use self::store::GitStore;
use crate::context::{
    ChangeLogEntry, ChangeSetOutcome, Error, ResourceChange, ResourceChangeInfo, ResourceEntry, ResourceStat,
    ResourceVersion,
};
use crate::entity::{EntityFacade, EntityValue};
use crate::script::bounded::BoundedMap;

pub(crate) const GIT_REPOSITORY: &str = "moqui.resource.GitRepository";

/// The artifact type of repository access over git and the `get#Git*` services: `view` to clone, fetch and read;
/// `create` and `update` to push new refs and fast-forwards; `delete` to force a push or delete a ref. The `git://`
/// resource scheme is not checked against it.
pub const GIT_ARTIFACT_TYPE: &str = "AT_GIT_REPOSITORY";

/// The `resource-reference` class that names this provider in `MoquiDefaultConf.xml`.
pub const GIT_CLASS: &str = crate::native::GIT_RESOURCE_CLASS;

const DEFAULT_BRANCH: &str = "main";

/// Path indexes kept in memory: at most this many branches, and about this many bytes.
const INDEX_BRANCHES: usize = 32;
const INDEX_BYTES: usize = 256 * 1024 * 1024;

/// How a repository is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepositoryKind {
    Remote,
    Local,
    Db,
}

impl RepositoryKind {
    fn parse(repository_id: &str, text: &str) -> Result<Self, Error> {
        match text {
            "remote" => Ok(Self::Remote),
            "local" | "" => Ok(Self::Local),
            "db" => Ok(Self::Db),
            other => Err(Error::new(format!(
                "git repository {repository_id} has kind {other}; it must be remote, local or db"
            ))),
        }
    }
}

/// A `GitRepository` row.
#[derive(Clone, Debug)]
pub(crate) struct Definition {
    pub id: String,
    pub kind: RepositoryKind,
    pub remote_url: Option<String>,
    pub branch: String,
    /// The row names its branch (else it is `main` until a first push names one).
    pub branch_explicit: bool,
    pub path: PathBuf,
    pub credentials: fs::Credentials,
    /// The row has an `archivedDate`: pushes, ref writes, merges and writes are refused.
    pub archived: bool,
}

/// An open repository.
#[derive(Debug)]
struct Repo {
    definition: Definition,
    store: Box<dyn GitStore>,
}

/// The `git` scheme of the resource facade, and the operations of `org.moqui.impl.GitServices`.
#[derive(Clone)]
pub struct GitProvider {
    inner: Arc<Inner>,
}

struct Inner {
    /// The factory's entity facade slot: it is installed after boot, so repositories are read on first use.
    entity: Arc<RwLock<Option<EntityFacade>>>,
    runtime_dir: PathBuf,
    repos: RwLock<HashMap<String, Arc<Repo>>>,
    /// Writers of a repository in this process take turns; across processes the ref compare-and-set keeps refs
    /// consistent. Kept apart from `repos`, so a refresh that reopens a repository does not give it a second lock.
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// The runs of each repository's gitECA post events take turns, so they run one change at a time, in order.
    event_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// When each repository's refs last changed (milliseconds): a push of commits dated in the past still changes the
    /// last-modified time of its files, which caches over `git://` locations go by.
    updated: Mutex<HashMap<String, i64>>,
    /// Decoded objects of the `db` repositories, made on first use by the conf of the context then current.
    objects: std::sync::OnceLock<Arc<db::ObjectCache>>,
    /// Parsed `.gitattributes` files, by blob.
    attributes: attributes::RulesCache,
    /// The path index of each branch ("repositoryId\nbranch"), brought to the branch's head when it is used.
    indexes: Mutex<BoundedMap<String, Arc<Mutex<BranchIndex>>>>,
}

impl std::fmt::Debug for GitProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitProvider")
            .field("runtime_dir", &self.inner.runtime_dir)
            .finish()
    }
}

/// `git://repo/a/b` as the repository id and the path segments. `.` and `..` segments are refused.
fn parse_location(location: &str) -> Result<(String, Vec<String>), Error> {
    let rest = location
        .strip_prefix("git://")
        .or_else(|| location.strip_prefix("git:"))
        .ok_or_else(|| Error::new(format!("not a git location: {location}")))?;
    let mut segments = rest.split('/').filter(|segment| !segment.is_empty());
    let repository = segments
        .next()
        .ok_or_else(|| Error::new(format!("no repository in git location {location}")))?
        .to_string();
    let path: Vec<String> = segments.map(str::to_string).collect();
    if path
        .iter()
        .any(|segment| segment == "." || segment == ".." || segment == ".git")
    {
        return Err(Error::new(format!("bad path in git location {location}")));
    }
    Ok((repository, path))
}

/// Authorization off while the provider reads and writes its own rows (`GitRepository`, `GitObject`, `GitRef`, and
/// the author's `UserAccount`): access to a repository is checked where it is reached, `AT_GIT_REPOSITORY` for the
/// git server and the `get#Git*` services, and the screen or service that uses a `git://` location.
pub(crate) fn rows_guard() -> Option<crate::context::AuthzGuard> {
    crate::context::ExecutionContext::with_current(crate::context::ExecutionContext::disable_authz)
}

fn text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Null => None,
        value => Some(moqui_lib::object::to_plain_string(value)).filter(|text| !text.trim().is_empty()),
    }
}

fn long(value: Option<&Value>) -> Option<i64> {
    match basic_convert(value?, "Long").value {
        Value::I64(number) => Some(number),
        _ => None,
    }
}

fn millis(value: Option<&Value>) -> Option<i64> {
    match basic_convert(value?, "Timestamp").value {
        Value::Timestamp(stamp) => Some(stamp.as_millisecond()),
        _ => None,
    }
}

impl GitProvider {
    pub(crate) fn new(entity: Arc<RwLock<Option<EntityFacade>>>, runtime_dir: PathBuf) -> Self {
        Self {
            inner: Arc::new(Inner {
                entity,
                runtime_dir,
                repos: RwLock::new(HashMap::new()),
                locks: Mutex::new(HashMap::new()),
                event_locks: Mutex::new(HashMap::new()),
                updated: Mutex::new(HashMap::new()),
                objects: std::sync::OnceLock::new(),
                attributes: attributes::rules_cache(),
                indexes: Mutex::new(BoundedMap::with_bytes(INDEX_BRANCHES, INDEX_BYTES)),
            }),
        }
    }

    /// The cache of decoded objects, made on first use.
    pub(crate) fn objects(&self) -> Arc<db::ObjectCache> {
        Arc::clone(self.inner.objects.get_or_init(db::object_cache))
    }

    fn entity(&self) -> Result<EntityFacade, Error> {
        moqui_lib::sync::read(&self.inner.entity)
            .clone()
            .ok_or_else(|| Error::new("git resources need the entity facade, which is not installed"))
    }

    /// The `GitRepository` row of `repository_id`.
    pub(crate) fn definition(&self, repository_id: &str) -> Result<Definition, Error> {
        let _rows = rows_guard();
        let entity = self.entity()?;
        let row = entity
            .find(GIT_REPOSITORY)?
            .eq("repositoryId", Value::string(repository_id))
            .one()?
            .ok_or_else(|| {
                Error::new(format!(
                    "git repository {repository_id} is not defined (no moqui.resource.GitRepository row)"
                ))
            })?;
        let kind = RepositoryKind::parse(repository_id, &text(row.get("repositoryKind")).unwrap_or_default())?;
        if let Some(updated) = millis(row.get("lastUpdateDate")) {
            self.note_update(repository_id, updated);
        }
        let path = match text(row.get("localPath")) {
            Some(path) if Path::new(&path).is_absolute() => PathBuf::from(path),
            Some(path) => self.inner.runtime_dir.join(path),
            None => self.inner.runtime_dir.join("git").join(repository_id),
        };
        Ok(Definition {
            id: repository_id.to_string(),
            kind,
            remote_url: text(row.get("remoteUrl")),
            branch: text(row.get("branch")).unwrap_or_else(|| DEFAULT_BRANCH.to_string()),
            branch_explicit: text(row.get("branch")).is_some(),
            path,
            credentials: fs::Credentials {
                username: text(row.get("username")),
                token: text(row.get("accessToken")),
            },
            archived: text(row.get("archivedDate")).is_some(),
        })
    }

    fn open(&self, definition: Definition) -> Result<Repo, Error> {
        let store: Box<dyn GitStore> = match definition.kind {
            RepositoryKind::Remote => {
                if !definition.path.join("HEAD").is_file() {
                    return Err(Error::new(format!(
                        "git repository {} has not been fetched yet (org.moqui.impl.GitServices.fetch#GitRepository)",
                        definition.id
                    )));
                }
                Box::new(fs::FsStore::open(&definition.path)?)
            }
            RepositoryKind::Local => Box::new(fs::FsStore::init(&definition.path, &definition.branch)?),
            RepositoryKind::Db => Box::new(db::DbStore::new(
                self.entity()?,
                &definition.id,
                self.objects(),
            )),
        };
        Ok(Repo { definition, store })
    }

    /// The lock writers of `repository_id` take turns on.
    fn lock_of(&self, repository_id: &str) -> Arc<Mutex<()>> {
        Arc::clone(
            moqui_lib::sync::lock(&self.inner.locks)
                .entry(repository_id.to_string())
                .or_default(),
        )
    }

    fn repo(&self, repository_id: &str) -> Result<Arc<Repo>, Error> {
        if let Some(repo) = moqui_lib::sync::read(&self.inner.repos).get(repository_id) {
            return Ok(Arc::clone(repo));
        }
        let repo = Arc::new(self.open(self.definition(repository_id)?)?);
        moqui_lib::sync::write(&self.inner.repos).insert(repository_id.to_string(), Arc::clone(&repo));
        Ok(repo)
    }

    /// Forget an open repository, so its row is read again: after the row changed, or a service replaced its storage.
    pub fn refresh(&self, repository_id: &str) {
        moqui_lib::sync::write(&self.inner.repos).remove(repository_id);
    }

    fn located(&self, location: &str) -> Result<(Arc<Repo>, Vec<String>), Error> {
        let (repository_id, path) = parse_location(location)?;
        Ok((self.repo(&repository_id)?, path))
    }

    /// The current user as a commit author: the `UserAccount` name and email, else the username, else `moqui`.
    fn author(&self) -> Author {
        self.author_of(crate::user::current_user_id().ok().flatten(), true)
    }

    /// A user as a commit author, by their `UserAccount` (the author of a squash commit, say).
    pub(crate) fn author_for_user(&self, user_id: &str) -> Author {
        self.author_of(Some(user_id.to_string()), false)
    }

    /// A user as a commit author, by their `UserAccount`; `current` falls back to the current username.
    fn author_of(&self, user_id: Option<String>, current: bool) -> Author {
        let _rows = rows_guard();
        let account = user_id.as_ref().and_then(|user_id| {
            let entity = self.entity().ok()?;
            entity
                .find("moqui.security.UserAccount")
                .ok()?
                .eq("userId", Value::string(user_id))
                .one()
                .ok()
                .flatten()
        });
        let field = |name: &str| account.as_ref().and_then(|row| text(row.get(name)));
        let username = field("username").or_else(|| {
            if current {
                crate::user::current_username().ok().flatten()
            } else {
                user_id.clone()
            }
        });
        Author {
            name: field("userFullName")
                .or_else(|| username.clone())
                .unwrap_or_else(|| "moqui".to_string()),
            email: field("emailAddress")
                .unwrap_or_else(|| format!("{}@localhost", username.as_deref().unwrap_or("moqui"))),
            user_id,
            seconds: None,
        }
    }

    /// The author of the commits a write makes now: the version author set around the write
    /// ([`crate::context::with_version_author`]), with its date, else the committer (the current user).
    fn version_author(&self, committer: &Author) -> Author {
        match crate::context::version_author() {
            Some(set) => {
                let mut author = match set.user_id {
                    Some(user_id) => self.author_of(Some(user_id), false),
                    None => committer.clone(),
                };
                author.seconds = set.date.map(|millis| millis.div_euclid(1000));
                author
            }
            None => committer.clone(),
        }
    }

    fn writable(repo: &Repo, location: &str) -> Result<(), Error> {
        if repo.definition.kind == RepositoryKind::Remote {
            return Err(Error::new(format!(
                "git repository {} is a read-only mirror: {location}",
                repo.definition.id
            )));
        }
        if repo.definition.archived {
            return Err(Error::new(format!(
                "git repository {} is archived and read only: {location}",
                repo.definition.id
            )));
        }
        Ok(())
    }

    /// Commit on the repository's branch, under its lock, and remember the new head on its row.
    fn commit(
        &self,
        repo: &Repo,
        edits: impl FnMut(Option<&ops::CommitInfo>) -> Result<Option<(Vec<Edit>, String)>, Error>,
    ) -> Result<Option<ObjectId>, Error> {
        let lock = self.lock_of(&repo.definition.id);
        let _turn = moqui_lib::sync::lock(&lock);
        // the refs before, only when a gitECA rule listens for writes
        let prior = if crate::service::git_eca::listening_after("write") {
            repo.store.list_refs("refs/").ok()
        } else {
            None
        };
        let committer = self.author();
        let author = self.version_author(&committer);
        let made = ops::commit_as(repo.store.as_ref(), &repo.definition.branch, &author, &committer, edits)?;
        if let Some(id) = made {
            self.log_protected_write(repo, id);
            self.remember_head(&repo.definition.id, Some(id), None);
            if let Some(prior) = prior {
                let name = ops::branch_ref(&repo.definition.branch);
                let old = prior
                    .iter()
                    .find(|(candidate, _)| *candidate == name)
                    .map(|(_, id)| *id);
                let origin = events::UpdateOrigin {
                    source: "write",
                    transport: "internal",
                    user_id: crate::user::current_user_id().ok().flatten(),
                    ..Default::default()
                };
                let change = store::RefChange {
                    name,
                    old,
                    new: Some(id),
                };
                self.record_update(&repo.definition.id, &origin, repo.store.as_ref(), &prior, &[change]);
            }
        }
        Ok(made)
    }

    fn note_update(&self, repository_id: &str, millis: i64) {
        let mut updated = moqui_lib::sync::lock(&self.inner.updated);
        let entry = updated.entry(repository_id.to_string()).or_insert(millis);
        *entry = (*entry).max(millis);
    }

    /// When a repository's refs last changed here, or as its row last said.
    fn last_update(&self, repository_id: &str) -> Option<i64> {
        moqui_lib::sync::lock(&self.inner.updated).get(repository_id).copied()
    }

    /// Set `headCommitId` and `lastUpdateDate` (and the fetch outcome, when given) on a repository's row. A failure
    /// is logged: the commit, push or fetch it describes already happened.
    fn remember_head(&self, repository_id: &str, head: Option<ObjectId>, fetch: Option<Option<&str>>) {
        self.write_row(repository_id, |row| {
            row.set(
                "headCommitId",
                head.map_or(Value::Null, |id| Value::string(id.to_string())),
            );
            if let Some(error) = fetch {
                row.set("lastFetchDate", crate::user::now_effective_value());
                row.set("lastFetchError", error.map_or(Value::Null, Value::string));
            }
        });
    }

    /// Set only `lastUpdateDate`: refs changed, the default branch did not.
    fn remember_update(&self, repository_id: &str) {
        self.write_row(repository_id, |_| {});
    }

    /// Name the default branch of a repository whose row named none, after the first push to it.
    fn set_default_branch(&self, repository_id: &str, branch: &str) {
        self.write_row(repository_id, |row| row.set("branch", Value::string(branch)));
        if let Ok(definition) = self.definition(repository_id) {
            if definition.kind != RepositoryKind::Db {
                if let Err(err) = fs::set_head(&definition.path, branch) {
                    tracing::error!(repository = repository_id, "cannot point HEAD at {branch}: {err}");
                }
            }
        }
        self.refresh(repository_id);
    }

    fn write_row(&self, repository_id: &str, fields: impl FnOnce(&mut EntityValue)) {
        let now = jiff::Timestamp::now().as_millisecond();
        self.note_update(repository_id, now);
        let _rows = rows_guard();
        let outcome = (|| -> Result<(), Error> {
            let entity = self.entity()?;
            let mut row = EntityValue::make(&entity, GIT_REPOSITORY)?;
            row.set("repositoryId", Value::string(repository_id));
            row.set("lastUpdateDate", crate::user::now_value());
            fields(&mut row);
            row.update(&entity)?;
            Ok(())
        })();
        if let Err(err) = outcome {
            tracing::error!(
                repository = repository_id,
                "cannot record the git repository head: {err}"
            );
        }
    }

    fn head(repo: &Repo) -> Result<Option<ops::CommitInfo>, Error> {
        ops::head(repo.store.as_ref(), &repo.definition.branch)
    }

    /// The commit a version name names: a full id, or a prefix of at least 7 hex digits of one of the file's versions.
    fn commit_named(&self, repo: &Repo, path: &[String], name: &str) -> Result<Option<ObjectId>, Error> {
        let name = name.trim();
        if name.len() == 40 {
            return Ok(ObjectId::from_hex(name.as_bytes()).ok());
        }
        if name.len() < 7 || !name.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Ok(None);
        }
        let versions = self.file_versions(repo, path)?;
        let mut found = versions
            .iter()
            .map(|(version, _)| version)
            .filter(|version| version.version_name.starts_with(name));
        match (found.next(), found.next()) {
            (Some(version), None) => Ok(ObjectId::from_hex(version.version_name.as_bytes()).ok()),
            _ => Ok(None),
        }
    }

    fn history(&self, location: &str) -> Result<Vec<ResourceVersion>, Error> {
        let (repo, path) = self.located(location)?;
        Ok(self
            .file_versions(&repo, &path)?
            .into_iter()
            .map(|(version, _)| version)
            .collect())
    }

    /// The versions of the file at `path` from the branch's path index, newest first, each with the path and blob the
    /// file had in it (an older version can be at the path the file was moved from).
    fn file_versions(&self, repo: &Repo, path: &[String]) -> Result<Vec<(ResourceVersion, index::FileVersion)>, Error> {
        let Some(head) = Self::head(repo)? else {
            return Ok(Vec::new());
        };
        let joined = path.join("/");
        self.indexed(repo, &head, |index| index.versions(&joined))
    }

    /// Run `query` on the path index of the repository's branch, brought to `head` first (built on first use, extended
    /// when the branch moved forward, rebuilt when it was replaced).
    fn indexed<T>(
        &self,
        repo: &Repo,
        head: &ops::CommitInfo,
        query: impl FnOnce(&BranchIndex) -> T,
    ) -> Result<T, Error> {
        self.indexed_at(repo, &repo.definition.branch, head, query)
    }

    /// [`Self::indexed`] for any branch (`name`), or for a commit not on a branch (`name` `@<commitId>`).
    fn indexed_at<T>(
        &self,
        repo: &Repo,
        name: &str,
        head: &ops::CommitInfo,
        query: impl FnOnce(&BranchIndex) -> T,
    ) -> Result<T, Error> {
        let key = format!("{}\n{name}", repo.definition.id);
        let cached = moqui_lib::sync::lock(&self.inner.indexes).get(&key);
        let (index, fresh) = match cached {
            Some(index) => (index, false),
            None => (
                Arc::new(Mutex::new(BranchIndex::build(repo.store.as_ref(), &head.id)?)),
                true,
            ),
        };
        let mut guard = moqui_lib::sync::lock(&index);
        let moved = guard.head != head.id;
        if moved {
            guard.advance(repo.store.as_ref(), &head.id)?;
        }
        if fresh || moved {
            let bytes = guard.bytes;
            moqui_lib::sync::lock(&self.inner.indexes).insert_sized(key, Arc::clone(&index), bytes);
        }
        Ok(query(&guard))
    }

    /// Apply a change set as one commit on the branch: each change against the tree the changes before it made, a
    /// change that cannot apply (a move of nothing, a write onto a directory) reported and left out.
    fn apply_change_set(
        &self,
        root: &str,
        changes: &[ResourceChange],
        message: Option<&str>,
    ) -> Result<ChangeSetOutcome, Error> {
        let (repo, root_path) = self.located(root)?;
        Self::writable(&repo, root)?;
        let store = repo.store.as_ref();
        let full = |path: &str| -> Vec<String> {
            root_path
                .iter()
                .cloned()
                .chain(path.split('/').map(str::to_string))
                .collect()
        };
        let mut blobs = Vec::with_capacity(changes.len());
        for change in changes {
            blobs.push(match change {
                // a file the attributes keep in LFS is its content blob behind a pointer, as for a single write
                ResourceChange::Write { path, bytes } => {
                    Some(self.store_file(&repo, &full(path), lfs_files::FileContent::Bytes(bytes))?)
                }
                ResourceChange::MakeDirectory { .. } => Some(store.write(Kind::Blob, b"")?),
                _ => None,
            });
        }
        let message = message
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| change_set_message(&root_path, changes));
        let mut outcome = ChangeSetOutcome::default();
        let made = self.commit(&repo, |head| {
            outcome = ChangeSetOutcome::default();
            let mut tree = head.map(|head| head.tree);
            let mut edits = Vec::new();
            let resolve = |tree: Option<ObjectId>, path: &[String]| -> Result<Option<_>, Error> {
                match tree {
                    Some(tree) => ops::resolve(store, &tree, path),
                    None => Ok(None),
                }
            };
            for (change, blob) in changes.iter().zip(&blobs) {
                let step = (|| -> Result<Vec<Edit>, Error> {
                    match change {
                        ResourceChange::Write { path, .. } => {
                            let at = full(path);
                            let blob = blob.ok_or_else(|| Error::new("no blob for a write"))?;
                            match resolve(tree, &at)? {
                                Some((mode, _)) if mode.is_tree() => Err(Error::new(format!("is a directory: {path}"))),
                                Some((_, id)) if id == blob => Ok(Vec::new()),
                                Some((mode, _)) if mode.is_executable() => {
                                    Ok(vec![Edit::Put(at, EntryKind::BlobExecutable, blob)])
                                }
                                _ => Ok(vec![Edit::Put(at, EntryKind::Blob, blob)]),
                            }
                        }
                        ResourceChange::Delete { path } => {
                            let at = full(path);
                            Ok(match resolve(tree, &at)? {
                                Some(_) => vec![Edit::Remove(at)],
                                None => Vec::new(),
                            })
                        }
                        ResourceChange::Move { from, to } => {
                            let (from_at, to_at) = (full(from), full(to));
                            let Some((mode, id)) = resolve(tree, &from_at)? else {
                                return Err(Error::new(format!("resource not found: {from}")));
                            };
                            if resolve(tree, &to_at)?.is_some() {
                                return Err(Error::new(format!("already exists: {to}")));
                            }
                            Ok(vec![Edit::Remove(from_at), Edit::Put(to_at, mode.kind(), id)])
                        }
                        ResourceChange::MakeDirectory { path } => {
                            let at = full(path);
                            match resolve(tree, &at)? {
                                Some((mode, _)) if mode.is_tree() => Ok(Vec::new()),
                                Some(_) => Err(Error::new(format!("found file instead of directory: {path}"))),
                                None => {
                                    let blob = blob.ok_or_else(|| Error::new("no blob for a directory"))?;
                                    let mut keep = at;
                                    keep.push(".gitkeep".to_string());
                                    Ok(vec![Edit::Put(keep, EntryKind::Blob, blob)])
                                }
                            }
                        }
                    }
                })();
                match step {
                    Ok(step) => {
                        if !step.is_empty() {
                            tree = Some(ops::apply_edits(store, tree.as_ref(), &step)?);
                            edits.extend(step);
                        }
                        outcome.applied.push(change.path().to_string());
                    }
                    Err(err) => {
                        tracing::error!(root, path = change.path(), "change set change failed: {err}");
                        outcome
                            .failures
                            .push((change.path().to_string(), err.message().to_string()));
                    }
                }
            }
            if edits.is_empty() {
                return Ok(None);
            }
            Ok(Some((edits, message.clone())))
        })?;
        outcome.version_name = made.map(|id| id.to_string());
        Ok(outcome)
    }

    // ---- operations of org.moqui.impl.GitServices ----

    /// Fetch a `remote` repository's branches and tags into its mirror (created on the first fetch), and record the
    /// outcome on its row. The new head of its branch.
    pub fn fetch(&self, repository_id: &str) -> Result<Option<String>, Error> {
        let definition = self.definition(repository_id)?;
        if definition.kind != RepositoryKind::Remote {
            return Err(Error::new(format!(
                "git repository {repository_id} is not a remote mirror; import#GitRepository copies from another repository"
            )));
        }
        let url = definition
            .remote_url
            .clone()
            .ok_or_else(|| Error::new(format!("git repository {repository_id} has no remoteUrl")))?;
        if self.entity()?.sim_active() {
            return Err(Error::new(format!(
                "fetch of git repository {repository_id} denied because sim mode is active"
            )));
        }
        crate::entity::fence::check_effect(
            crate::entity::fence::Effect::RemoteCall,
            &format!("fetch of git repository {repository_id}"),
        )?;
        let outcome = {
            let lock = self.lock_of(repository_id);
            let _turn = moqui_lib::sync::lock(&lock);
            fs::fetch(
                &definition.path,
                &url,
                &definition.branch,
                &definition.credentials,
                true,
            )
        };
        self.refresh(repository_id);
        match outcome {
            Ok(fetched) => {
                if !fetched.changes.is_empty() {
                    tracing::info!(
                        repository = repository_id,
                        "fetch changed {} refs: {}",
                        fetched.changes.len(),
                        fetched
                            .changes
                            .iter()
                            .map(|change| change.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
                self.remember_head(repository_id, fetched.head, Some(None));
                if let Ok(repo) = self.repo(repository_id) {
                    let origin = events::UpdateOrigin {
                        source: "fetch",
                        transport: "internal",
                        user_id: crate::user::current_user_id().ok().flatten(),
                        ..Default::default()
                    };
                    self.record_update(
                        repository_id,
                        &origin,
                        repo.store.as_ref(),
                        &fetched.before,
                        &fetched.changes,
                    );
                }
                Ok(fetched.head.map(|id| id.to_string()))
            }
            Err(err) => {
                let current = fs::FsStore::open(&definition.path)
                    .ok()
                    .and_then(|store| store.read_ref(&ops::branch_ref(&definition.branch)).ok().flatten());
                self.remember_head(repository_id, current, Some(Some(err.message())));
                Err(err)
            }
        }
    }

    /// Fetch every `remote` repository whose `fetchIntervalMinutes` has passed since its last fetch. A failing
    /// repository is recorded on its row and logged, and the others are still fetched. (fetched, failed) ids.
    pub fn fetch_due(&self) -> Result<(Vec<String>, Vec<String>), Error> {
        let entity = self.entity()?;
        let rows_authz = rows_guard();
        let rows = entity
            .find(GIT_REPOSITORY)?
            .eq("repositoryKind", Value::string("remote"))
            .list()?;
        drop(rows_authz);
        let now = jiff::Timestamp::now().as_millisecond();
        let (mut fetched, mut failed) = (Vec::new(), Vec::new());
        for row in rows.iter() {
            let Some(id) = text(row.get("repositoryId")) else {
                continue;
            };
            let Some(interval) = long(row.get("fetchIntervalMinutes")).filter(|minutes| *minutes > 0) else {
                continue;
            };
            if millis(row.get("lastFetchDate")).is_some_and(|last| last + interval * 60_000 > now) {
                continue;
            }
            match self.fetch(&id) {
                Ok(_) => fetched.push(id),
                Err(err) => {
                    tracing::error!(repository = %id, "git fetch failed: {err}");
                    failed.push(id);
                }
            }
        }
        Ok((fetched, failed))
    }

    /// Make a repository ready: create the bare repository of a `local` kind, fetch a `remote` one. A `db` repository
    /// needs nothing. What was done, for the message.
    pub fn init(&self, repository_id: &str) -> Result<String, Error> {
        let definition = self.definition(repository_id)?;
        let done = match definition.kind {
            RepositoryKind::Local => {
                let existed = definition.path.join("HEAD").is_file();
                fs::FsStore::init(&definition.path, &definition.branch)?;
                if existed {
                    format!(
                        "git repository {repository_id} is already at {}",
                        definition.path.display()
                    )
                } else {
                    format!(
                        "created bare git repository {repository_id} at {}",
                        definition.path.display()
                    )
                }
            }
            RepositoryKind::Remote => {
                let head = self.fetch(repository_id)?;
                format!(
                    "fetched git repository {repository_id} at {}",
                    head.unwrap_or_else(|| "an empty branch".into())
                )
            }
            RepositoryKind::Db => format!("git repository {repository_id} is kept in the database; nothing to create"),
        };
        self.refresh(repository_id);
        Ok(done)
    }

    /// Write a `db` repository as a bare repository on disk (at `target`, or its `localPath`), with its branch and
    /// `HEAD`, so it can be cloned and pushed with the git command line. (path, objects written).
    pub fn export(&self, repository_id: &str, target: Option<&str>) -> Result<(PathBuf, usize), Error> {
        let definition = self.definition(repository_id)?;
        if definition.kind != RepositoryKind::Db {
            return Err(Error::new(format!(
                "git repository {repository_id} is already on disk at {}",
                definition.path.display()
            )));
        }
        let path = match target.map(str::trim).filter(|target| !target.is_empty()) {
            Some(target) if Path::new(target).is_absolute() => PathBuf::from(target),
            Some(target) => self.inner.runtime_dir.join(target),
            None => definition.path.clone(),
        };
        let repo = self.repo(repository_id)?;
        let lock = self.lock_of(repository_id);
        let _turn = moqui_lib::sync::lock(&lock);
        let target = fs::FsStore::init(&path, &definition.branch)?;
        let Some(head) = Self::head(&repo)? else {
            return Ok((path, 0));
        };
        let haves: Vec<ObjectId> = target.list_refs("refs/")?.into_iter().map(|(_, id)| id).collect();
        let count = server::copy_objects(
            repo.store.as_ref(),
            &target,
            head.id,
            &haves,
            &self.scratch("export", repository_id),
        )?;
        let name = ops::branch_ref(&definition.branch);
        let current = target.read_ref(&name)?;
        if !target.compare_and_set_ref(&name, current, head.id)? {
            return Err(Error::new(format!(
                "branch {} of {} moved during the export",
                definition.branch,
                path.display()
            )));
        }
        fs::set_head(&path, &definition.branch)?;
        Ok((path, count))
    }

    /// Copy a repository's branch into a `db` repository: from `source` (a `git://` repository id, a bare or
    /// non-bare repository directory, or a URL fetched into a scratch directory under `runtime/tmp`), else from the
    /// repository's `remoteUrl`. The branch is replaced. (new head, objects copied).
    pub fn import(&self, repository_id: &str, source: Option<&str>) -> Result<(String, usize), Error> {
        let definition = self.definition(repository_id)?;
        if definition.kind != RepositoryKind::Db {
            return Err(Error::new(format!(
                "import is for db repositories; {repository_id} is not one"
            )));
        }
        let source = source
            .map(str::trim)
            .filter(|source| !source.is_empty())
            .map(str::to_string)
            .or_else(|| definition.remote_url.clone())
            .ok_or_else(|| Error::new(format!("no source to import git repository {repository_id} from")))?;
        let mut scratch = None;
        let (from, branch): (Box<dyn GitStore>, String) = if let Some(other) = source.strip_prefix("git://") {
            let other = other.trim_matches('/');
            if other == repository_id {
                return Err(Error::new("a repository cannot be imported into itself"));
            }
            let other = self.definition(other)?;
            let store: Box<dyn GitStore> = match other.kind {
                RepositoryKind::Db => Box::new(db::DbStore::new(
                    self.entity()?,
                    &other.id,
                    self.objects(),
                )),
                _ => Box::new(fs::FsStore::open(&other.path)?),
            };
            (store, other.branch)
        } else if Path::new(&source).is_dir() {
            let path = Path::new(&source);
            let path = if path.join(".git").is_dir() {
                path.join(".git")
            } else {
                path.to_path_buf()
            };
            (Box::new(fs::FsStore::open(&path)?), definition.branch.clone())
        } else {
            if self.entity()?.sim_active() {
                return Err(Error::new(format!(
                    "import of {} denied because sim mode is active",
                    crate::outbound::redacted(&source)
                )));
            }
            crate::entity::fence::check_effect(
                crate::entity::fence::Effect::RemoteCall,
                &format!("import of {}", crate::outbound::redacted(&source)),
            )?;
            let path = self.inner.runtime_dir.join("tmp").join(format!(
                "git-import-{repository_id}-{}",
                jiff::Timestamp::now().as_millisecond()
            ));
            fs::fetch(&path, &source, &definition.branch, &definition.credentials, false)?;
            scratch = Some(path.clone());
            (Box::new(fs::FsStore::open(&path)?), definition.branch.clone())
        };
        let outcome = self.copy_branch(repository_id, &definition.branch, from.as_ref(), &branch);
        if let Some(path) = scratch {
            if let Err(err) = std::fs::remove_dir_all(&path) {
                tracing::warn!("cannot remove {}: {err}", path.display());
            }
        }
        outcome
    }

    fn copy_branch(
        &self,
        repository_id: &str,
        branch: &str,
        from: &dyn GitStore,
        from_branch: &str,
    ) -> Result<(String, usize), Error> {
        let head = from
            .read_ref(&ops::branch_ref(from_branch))?
            .ok_or_else(|| Error::new(format!("the source has no branch {from_branch}")))?;
        let repo = self.repo(repository_id)?;
        let lock = self.lock_of(repository_id);
        let _turn = moqui_lib::sync::lock(&lock);
        let haves: Vec<ObjectId> = repo.store.list_refs("refs/")?.into_iter().map(|(_, id)| id).collect();
        let count = server::copy_objects(
            from,
            repo.store.as_ref(),
            head,
            &haves,
            &self.scratch("import", repository_id),
        )?;
        let name = ops::branch_ref(branch);
        let prior = repo.store.list_refs("refs/")?;
        let current = repo.store.read_ref(&name)?;
        if !repo.store.compare_and_set_ref(&name, current, head)? {
            return Err(Error::new(format!(
                "branch {branch} of {repository_id} moved during the import"
            )));
        }
        drop(_turn);
        self.remember_head(repository_id, Some(head), None);
        if current != Some(head) {
            let origin = events::UpdateOrigin {
                source: "import",
                transport: "internal",
                user_id: crate::user::current_user_id().ok().flatten(),
                ..Default::default()
            };
            let change = store::RefChange {
                name,
                old: current,
                new: Some(head),
            };
            self.record_update(repository_id, &origin, repo.store.as_ref(), &prior, &[change]);
        }
        Ok((head.to_string(), count))
    }
}

impl GitProvider {
    /// Commit the stored `blob` as the file at `path`, with `note` as the message (else `Create <path>` or `Update
    /// <path>`); nothing when the file is that blob already.
    fn put_file(
        &self,
        repo: &Repo,
        location: &str,
        path: Vec<String>,
        blob: ObjectId,
        note: Option<&str>,
    ) -> Result<(), Error> {
        let shown = path.join("/");
        self.commit(repo, |head| {
            let existing = match head {
                Some(head) => ops::resolve(repo.store.as_ref(), &head.tree, &path)?,
                None => None,
            };
            let kind = match existing {
                Some((mode, _)) if mode.is_tree() => {
                    return Err(Error::new(format!("is a directory: {location}")));
                }
                Some((_, id)) if id == blob => return Ok(None),
                Some((mode, _)) if mode.is_executable() => EntryKind::BlobExecutable,
                _ => EntryKind::Blob,
            };
            let verb = if existing.is_some() { "Update" } else { "Create" };
            let message = note
                .map(str::trim)
                .filter(|note| !note.is_empty())
                .map_or_else(|| format!("{verb} {shown}"), str::to_string);
            Ok(Some((vec![Edit::Put(path.clone(), kind, blob)], message)))
        })?;
        Ok(())
    }

    /// A new scratch directory under `runtime/tmp` for a copy of objects.
    fn scratch(&self, what: &str, repository_id: &str) -> PathBuf {
        static SCRATCHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        self.inner.runtime_dir.join("tmp").join(format!(
            "git-{what}-{repository_id}-{}-{}",
            std::process::id(),
            SCRATCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }
}

fn join(location: &str, name: &str) -> String {
    format!("{}/{name}", location.trim_end_matches('/'))
}

impl crate::native::ResourceProvider for GitProvider {
    fn read_bytes(&self, location: &str) -> Result<Vec<u8>, Error> {
        let (repo, path) = self.located(location)?;
        let not_found = || Error::new(format!("resource not found: {location}"));
        let head = Self::head(&repo)?.ok_or_else(not_found)?;
        match ops::resolve(repo.store.as_ref(), &head.tree, &path)? {
            Some((mode, blob)) if !mode.is_tree() => self.file_content(&repo, &blob),
            _ => Err(not_found()),
        }
    }

    /// The file at the branch head: its blob, which the `ETag` names, read on first use, or a content blob's
    /// chunks a window at a time.
    fn open(self: Arc<Self>, location: &str) -> Result<crate::native::OpenedFile, Error> {
        let Some(stat) = self.stat(location)?.filter(|stat| stat.is_file) else {
            return Ok(None);
        };
        let (repo, path) = self.located(location)?;
        let Some(head) = Self::head(&repo)? else {
            return Ok(None);
        };
        let Some((_, blob)) = ops::resolve(repo.store.as_ref(), &head.tree, &path)?.filter(|(mode, _)| !mode.is_tree())
        else {
            return Ok(None);
        };
        if let Some(info) = self.lfs_content(&repo, &blob)? {
            let stat = ResourceStat {
                size: info.size,
                ..stat
            };
            return Ok(Some((stat, self.lfs_source(&info)?)));
        }
        if let Some(source) = repo.store.content_source(&blob)? {
            return Ok(Some((stat, source)));
        }
        let source = crate::resource::source::LazySource::new(stat.size, true, move || {
            Ok(ops::object(repo.store.as_ref(), &blob, Kind::Blob)?)
        })
        .with_tag(format!("git-{blob}"));
        Ok(Some((stat, Arc::new(source))))
    }

    fn stat(&self, location: &str) -> Result<Option<ResourceStat>, Error> {
        let (repo, path) = self.located(location)?;
        let Some(head) = Self::head(&repo)? else {
            // an empty branch still has its root directory
            return Ok(path.is_empty().then_some(ResourceStat {
                is_file: false,
                size: 0,
                last_modified: None,
            }));
        };
        // the file's last change; the newest is when the refs last moved here, as a push of commits dated in the past,
        // or two saves within a second, still change what caches over git:// locations go by
        let joined = path.join("/");
        let changed = self.indexed(&repo, &head, |index| {
            index
                .last_change(&joined)
                .map(|commit| (commit.info.id, commit.info.seconds))
        })?;
        let head_millis = head.seconds.saturating_mul(1000);
        let last_modified = Some(match changed {
            Some((id, seconds)) if id != head.id => seconds.saturating_mul(1000),
            _ => self
                .last_update(&repo.definition.id)
                .map_or(head_millis, |updated| updated.max(head_millis)),
        });
        Ok(match ops::resolve(repo.store.as_ref(), &head.tree, &path)? {
            Some((mode, _)) if mode.is_tree() => Some(ResourceStat {
                is_file: false,
                size: 0,
                last_modified,
            }),
            Some((_, blob)) => Some(ResourceStat {
                is_file: true,
                size: self.file_size(&repo, &blob)?,
                last_modified,
            }),
            None => None,
        })
    }

    fn list(&self, location: &str) -> Result<Vec<ResourceEntry>, Error> {
        let (repo, path) = self.located(location)?;
        let Some(head) = Self::head(&repo)? else {
            return if path.is_empty() {
                Ok(Vec::new())
            } else {
                Err(Error::new(format!("not a directory: {location}")))
            };
        };
        match ops::resolve(repo.store.as_ref(), &head.tree, &path)? {
            Some((mode, tree)) if mode.is_tree() => Ok(ops::list(repo.store.as_ref(), &tree)?
                .into_iter()
                .map(|entry| ResourceEntry {
                    location: join(location, &entry.name),
                    name: entry.name,
                    is_file: entry.is_file,
                })
                .collect()),
            _ => Err(Error::new(format!("not a directory: {location}"))),
        }
    }

    fn write_bytes(&self, location: &str, bytes: &[u8]) -> Result<(), Error> {
        self.write_bytes_noted(location, bytes, None)
    }

    /// A write is a commit whose message is the note, else `Create <path>` or `Update <path>`.
    fn write_bytes_noted(&self, location: &str, bytes: &[u8], note: Option<&str>) -> Result<(), Error> {
        let (repo, path) = self.located(location)?;
        Self::writable(&repo, location)?;
        if path.is_empty() {
            return Err(Error::new(format!("cannot write the root of a repository: {location}")));
        }
        let blob = self.store_file(&repo, &path, lfs_files::FileContent::Bytes(bytes))?;
        self.put_file(&repo, location, path, blob, note)
    }

    /// A large file is stored as it is read (a content blob of a `db` repository), never all in memory.
    fn write_from(&self, location: &str, input: &mut dyn std::io::Read, size: Option<u64>) -> Result<(), Error> {
        let Some(size) = size else {
            let mut bytes = Vec::new();
            input
                .read_to_end(&mut bytes)
                .map_err(|err| Error::new(format!("reading the content for {location}: {err}")))?;
            return self.write_bytes(location, &bytes);
        };
        let (repo, path) = self.located(location)?;
        Self::writable(&repo, location)?;
        if path.is_empty() {
            return Err(Error::new(format!("cannot write the root of a repository: {location}")));
        }
        let blob = self.store_file(&repo, &path, lfs_files::FileContent::Stream(input, size))?;
        self.put_file(&repo, location, path, blob, None)
    }

    /// Git keeps no empty directories, so a new directory gets an empty `.gitkeep` file, the usual convention. A
    /// directory already there is left alone; a file there is an error.
    fn make_directory(&self, location: &str) -> Result<(), Error> {
        let (repo, path) = self.located(location)?;
        Self::writable(&repo, location)?;
        if path.is_empty() {
            return Ok(());
        }
        let blob = repo.store.write(Kind::Blob, b"")?;
        let mut keep = path.clone();
        keep.push(".gitkeep".to_string());
        let shown = path.join("/");
        self.commit(&repo, |head| {
            let existing = match head {
                Some(head) => ops::resolve(repo.store.as_ref(), &head.tree, &path)?,
                None => None,
            };
            match existing {
                Some((mode, _)) if mode.is_tree() => Ok(None),
                Some(_) => Err(Error::new(format!("found file instead of directory: {location}"))),
                None => Ok(Some((
                    vec![Edit::Put(keep.clone(), EntryKind::Blob, blob)],
                    format!("Create directory {shown}"),
                ))),
            }
        })?;
        Ok(())
    }

    fn delete(&self, location: &str) -> Result<bool, Error> {
        let (repo, path) = self.located(location)?;
        Self::writable(&repo, location)?;
        if path.is_empty() {
            return Err(Error::new(format!(
                "cannot delete the root of a repository: {location}"
            )));
        }
        let shown = path.join("/");
        let made = self.commit(&repo, |head| {
            let Some(head) = head else { return Ok(None) };
            match ops::resolve(repo.store.as_ref(), &head.tree, &path)? {
                None => Ok(None),
                Some((mode, _)) if mode.is_tree() => Err(Error::new(format!("directory is not empty: {location}"))),
                Some(_) => Ok(Some((vec![Edit::Remove(path.clone())], format!("Delete {shown}")))),
            }
        })?;
        Ok(made.is_some())
    }

    fn rename(&self, location: &str, new_location: &str) -> Result<(), Error> {
        let (repo, path) = self.located(location)?;
        let (other, new_path) = parse_location(new_location)?;
        if other != repo.definition.id {
            return Err(Error::new(format!(
                "cannot move between git repositories: {location} to {new_location}"
            )));
        }
        Self::writable(&repo, location)?;
        if path.is_empty() || new_path.is_empty() {
            return Err(Error::new("cannot move the root of a repository"));
        }
        // a move onto itself is nothing to do, as for a dbresource (update#WikiPage asks for one on every save)
        if path == new_path {
            return Ok(());
        }
        let message = format!("Move {} to {}", path.join("/"), new_path.join("/"));
        let made = self.commit(&repo, |head| {
            let found = match head {
                Some(head) => ops::resolve(repo.store.as_ref(), &head.tree, &path)?,
                None => None,
            };
            let Some((mode, id)) = found else {
                return Err(Error::new(format!("resource not found: {location}")));
            };
            if let Some(head) = head {
                if ops::resolve(repo.store.as_ref(), &head.tree, &new_path)?.is_some() {
                    return Err(Error::new(format!("already exists: {new_location}")));
                }
            }
            Ok(Some((
                vec![Edit::Remove(path.clone()), Edit::Put(new_path.clone(), mode.kind(), id)],
                message.clone(),
            )))
        })?;
        if made.is_none() {
            return Err(Error::new(format!("nothing moved: {location}")));
        }
        Ok(())
    }

    fn supports_version(&self, location: &str) -> bool {
        parse_location(location).is_ok()
    }

    fn version(&self, location: &str, version_name: &str) -> Result<Option<ResourceVersion>, Error> {
        let (repo, path) = self.located(location)?;
        let Some(id) = self.commit_named(&repo, &path, version_name)? else {
            return Ok(None);
        };
        let id = id.to_string();
        Ok(self
            .history(location)?
            .into_iter()
            .find(|version| version.version_name == id))
    }

    fn current_version(&self, location: &str) -> Result<Option<ResourceVersion>, Error> {
        Ok(self.history(location)?.into_iter().next())
    }

    fn root_version(&self, location: &str) -> Result<Option<ResourceVersion>, Error> {
        Ok(self.history(location)?.pop())
    }

    fn version_history(&self, location: &str) -> Result<Vec<ResourceVersion>, Error> {
        self.history(location)
    }

    fn next_versions(&self, location: &str, version_name: &str) -> Result<Vec<ResourceVersion>, Error> {
        let (repo, path) = self.located(location)?;
        let Some(id) = self.commit_named(&repo, &path, version_name)? else {
            return Ok(Vec::new());
        };
        let id = id.to_string();
        Ok(self
            .history(location)?
            .into_iter()
            .filter(|version| version.previous_version_name.as_deref() == Some(id.as_str()))
            .collect())
    }

    fn read_version(&self, location: &str, version_name: &str) -> Result<Option<Vec<u8>>, Error> {
        let (repo, path) = self.located(location)?;
        let Some(id) = self.commit_named(&repo, &path, version_name)? else {
            return Ok(None);
        };
        let id = id.to_string();
        // the blob the file had in that version, at the path it had then (before a move, its old path)
        match self
            .file_versions(&repo, &path)?
            .into_iter()
            .find(|(version, _)| version.version_name == id)
        {
            Some((_, version)) => self.file_content(&repo, &version.blob).map(Some),
            None => Ok(None),
        }
    }

    fn apply_changes(
        &self,
        root: &str,
        changes: &[ResourceChange],
        message: Option<&str>,
    ) -> Result<ChangeSetOutcome, Error> {
        self.apply_change_set(root, changes, message)
    }

    fn last_change(&self, location: &str) -> Result<Option<ResourceChangeInfo>, Error> {
        let (repo, path) = self.located(location)?;
        let Some(head) = Self::head(&repo)? else {
            return Ok(path.is_empty().then(ResourceChangeInfo::default));
        };
        let Some((mode, id)) = ops::resolve(repo.store.as_ref(), &head.tree, &path)? else {
            return Ok(None);
        };
        let joined = path.join("/");
        let change = self.indexed(&repo, &head, |index| {
            index.last_change(&joined).map(|commit| commit.version(None))
        })?;
        let size = if mode.is_tree() {
            None
        } else {
            Some(self.file_size(&repo, &id)?)
        };
        Ok(Some(change_info(!mode.is_tree(), size, change)))
    }

    fn child_changes(&self, location: &str) -> Result<Vec<(String, ResourceChangeInfo)>, Error> {
        let (repo, path) = self.located(location)?;
        let Some(head) = Self::head(&repo)? else {
            return Ok(Vec::new());
        };
        let tree = match ops::resolve(repo.store.as_ref(), &head.tree, &path)? {
            Some((mode, tree)) if mode.is_tree() => tree,
            _ => return Err(Error::new(format!("not a directory: {location}"))),
        };
        let entries = ops::list(repo.store.as_ref(), &tree)?;
        let prefix = path.join("/");
        let changes = self.indexed(&repo, &head, |index| {
            entries
                .iter()
                .map(|entry| {
                    let child = if prefix.is_empty() {
                        entry.name.clone()
                    } else {
                        format!("{prefix}/{}", entry.name)
                    };
                    index.last_change(&child).map(|commit| commit.version(None))
                })
                .collect::<Vec<_>>()
        })?;
        let mut found = Vec::with_capacity(entries.len());
        for (entry, change) in entries.into_iter().zip(changes) {
            let size = if entry.is_file {
                Some(self.file_size(&repo, &entry.id)?)
            } else {
                None
            };
            found.push((entry.name, change_info(entry.is_file, size, change)));
        }
        Ok(found)
    }

    fn change_log(&self, location: &str, limit: usize) -> Result<Vec<ChangeLogEntry>, Error> {
        let (repo, path) = self.located(location)?;
        let Some(head) = Self::head(&repo)? else {
            return Ok(Vec::new());
        };
        let joined = path.join("/");
        Ok(self
            .indexed(&repo, &head, |index| index.log(&joined, limit))?
            .into_iter()
            .map(|(version, paths)| ChangeLogEntry { version, paths })
            .collect())
    }

    fn restore_version(&self, location: &str, version_name: &str, note: Option<&str>) -> Result<Option<String>, Error> {
        let (repo, path) = self.located(location)?;
        Self::writable(&repo, location)?;
        if path.is_empty() {
            return Err(Error::new(format!(
                "cannot restore the root of a repository: {location}"
            )));
        }
        let store = repo.store.as_ref();
        let name = version_name.trim();
        let commit_id = if name.len() == 40 {
            ObjectId::from_hex(name.as_bytes()).ok()
        } else {
            self.commit_named(&repo, &path, name)?
        };
        let not_found = || Error::new(format!("version {version_name} not found: {location}"));
        let commit_id = commit_id.ok_or_else(not_found)?;
        if store.find(&commit_id)?.is_none() {
            return Err(not_found());
        }
        let commit = ops::read_commit(store, &commit_id)?;
        // the entry at the path in that commit, else (a file moved since) the blob of that version of the file
        let entry = match ops::resolve(store, &commit.tree, &path)? {
            Some((mode, id)) => (mode.kind(), id),
            None => {
                let id = commit_id.to_string();
                let found = self
                    .file_versions(&repo, &path)?
                    .into_iter()
                    .find(|(version, _)| version.version_name == id)
                    .ok_or_else(not_found)?;
                (EntryKind::Blob, found.1.blob)
            }
        };
        let short: String = commit_id.to_string().chars().take(10).collect();
        let message = note
            .map(str::trim)
            .filter(|note| !note.is_empty())
            .map_or_else(|| format!("Restore {} from {short}", path.join("/")), str::to_string);
        let made = self.commit(&repo, |head| {
            if let Some(head) = head {
                if let Some((mode, id)) = ops::resolve(store, &head.tree, &path)? {
                    if mode.kind() == entry.0 && id == entry.1 {
                        return Ok(None);
                    }
                }
            }
            Ok(Some((vec![Edit::Put(path.clone(), entry.0, entry.1)], message.clone())))
        })?;
        Ok(match made {
            Some(id) => Some(id.to_string()),
            None => Self::head(&repo)?.map(|head| head.id.to_string()),
        })
    }

    fn tree_size(&self, location: &str) -> Result<u64, Error> {
        let (repo, path) = self.located(location)?;
        let Some(head) = Self::head(&repo)? else {
            return Ok(0);
        };
        let Some((mode, id)) = ops::resolve(repo.store.as_ref(), &head.tree, &path)? else {
            return Ok(0);
        };
        if !mode.is_tree() {
            return self.file_size(&repo, &id);
        }
        let mut total = 0;
        let mut pending = vec![id];
        while let Some(tree) = pending.pop() {
            for entry in ops::read_tree(repo.store.as_ref(), &tree)?.entries {
                if entry.mode.is_tree() {
                    pending.push(entry.oid);
                } else if !entry.mode.is_commit() {
                    total += self.file_size(&repo, &entry.oid)?;
                }
            }
        }
        Ok(total)
    }
}

/// A last change from the commit that made it.
fn change_info(is_file: bool, size: Option<u64>, version: Option<ResourceVersion>) -> ResourceChangeInfo {
    ResourceChangeInfo {
        is_file,
        size,
        date: version
            .as_ref()
            .and_then(|version| crate::context::millis_of(&version.version_date)),
        version_name: version.as_ref().map(|version| version.version_name.clone()),
        user_id: version.as_ref().and_then(|version| version.user_id.clone()),
        note: version.and_then(|version| version.note),
    }
}

/// The message of a change set given none: the change itself for one, else how many under the root.
fn change_set_message(root: &[String], changes: &[ResourceChange]) -> String {
    match changes {
        [ResourceChange::Write { path, .. }] => format!("Update {path}"),
        [ResourceChange::Delete { path }] => format!("Delete {path}"),
        [ResourceChange::Move { from, to }] => format!("Move {from} to {to}"),
        [ResourceChange::MakeDirectory { path }] => format!("Create directory {path}"),
        _ if root.is_empty() => format!("{} changes", changes.len()),
        _ => format!("{} changes under {}", changes.len(), root.join("/")),
    }
}
