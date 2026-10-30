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

//! Version notes, change sets, and the last change of a path, over every storage of the facade. Guide:
//! `docs/data/Revision-Management.md` ("Version notes", "Change sets", "Last changes and the git path index").
//!
//! - A **version note** is written with a version: `write_bytes_noted`. On `git://` it is the commit message, on
//!   `dbresource://` the file's `versionNote`; other storage keeps none.
//! - A **change set** is several writes, deletes, moves and new directories under one root with one message,
//!   [`ResourceFacade::apply_change_set`]: one commit on a `git://` root, the plain changes in order elsewhere (on
//!   `dbresource://` in the caller's transaction). A delete of a directory takes everything under it. A change that
//!   fails is reported and the rest still apply.
//! - The **last change** of a path is the version that last changed it (on `git://` from the path index, so a file's
//!   date and user are its own commit's, not the branch head's), with its size.
//! - A **version author** names who wrote a version and when, for a version copied from elsewhere (a wiki page moved
//!   into other storage with its history replayed): [`with_version_author`] around the writes. On `git://` it is the
//!   commit's author (name, email and date; the committer stays the current user, now) and its `Moqui-User-Id`; on
//!   `dbresource://` the history row's `userId` and `versionDate`.

use std::cell::RefCell;

use moqui_lib::object::Value;

use super::{join_location, scheme_of, strip_hash, DbSnap, Error, ResourceFacade, ResourceVersion};
use crate::context::{ResourceEntry, ResourceStat};
use crate::native::ResourceProvider;

/// One change of a change set. Paths are relative to the change set's root, `/`-separated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceChange {
    /// Create or replace a file; missing directories are made.
    Write { path: String, bytes: Vec<u8> },
    /// Delete a file, or a directory with everything under it. Nothing there is not an error.
    Delete { path: String },
    /// Move a file or directory; the target must not exist.
    Move { from: String, to: String },
    /// Make a directory and its missing parents.
    MakeDirectory { path: String },
}

impl ResourceChange {
    /// The path the change is reported under: the target of a move.
    pub fn path(&self) -> &str {
        match self {
            Self::Write { path, .. } | Self::Delete { path } | Self::MakeDirectory { path } => path,
            Self::Move { to, .. } => to,
        }
    }

    /// The change with its paths trimmed of `/`, or why a path is not one: empty, or with a `.` or `..` segment.
    pub(crate) fn normalized(&self) -> Result<Self, String> {
        let clean = |path: &str| -> Result<String, String> {
            let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
            if parts.is_empty() {
                return Err("the path is empty".to_string());
            }
            if parts
                .iter()
                .any(|part| *part == "." || *part == ".." || part.contains('\\'))
            {
                return Err(format!("bad path {path}"));
            }
            Ok(parts.join("/"))
        };
        Ok(match self {
            Self::Write { path, bytes } => Self::Write {
                path: clean(path)?,
                bytes: bytes.clone(),
            },
            Self::Delete { path } => Self::Delete { path: clean(path)? },
            Self::MakeDirectory { path } => Self::MakeDirectory { path: clean(path)? },
            Self::Move { from, to } => {
                let (from, to) = (clean(from)?, clean(to)?);
                if to == from || to.starts_with(&format!("{from}/")) {
                    return Err(format!("cannot move {from} into itself"));
                }
                Self::Move { from, to }
            }
        })
    }
}

/// Who wrote a version and when, when that is not the current user now: a version copied with its history from other
/// storage. Either part may be empty, which is the current user or now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VersionAuthor {
    pub user_id: Option<String>,
    /// Milliseconds since the epoch.
    pub date: Option<i64>,
}

thread_local! {
    static VERSION_AUTHOR: RefCell<Option<VersionAuthor>> = const { RefCell::new(None) };
}

/// Run `body` with the versions it writes (on `git://` and `dbresource://`) attributed to `author`, then put back the
/// author that was set before. `None` writes as the current user, now.
pub fn with_version_author<T>(author: Option<VersionAuthor>, body: impl FnOnce() -> T) -> T {
    struct Restore(Option<VersionAuthor>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            VERSION_AUTHOR.with(|cell| *cell.borrow_mut() = previous);
        }
    }
    let previous = VERSION_AUTHOR.with(|cell| cell.replace(author));
    let _restore = Restore(previous);
    body()
}

/// The author [`with_version_author`] set for the writes on this thread, if any.
pub(crate) fn version_author() -> Option<VersionAuthor> {
    VERSION_AUTHOR.with(|cell| cell.borrow().clone())
}

/// What a change set did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeSetOutcome {
    /// The version made for the whole set: the commit id on `git://`; `None` elsewhere or when nothing changed.
    pub version_name: Option<String>,
    /// The paths of the changes that applied, in order.
    pub applied: Vec<String>,
    /// `(path, error)` for each change that did not apply.
    pub failures: Vec<(String, String)>,
}

/// The last change of a file or directory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResourceChangeInfo {
    pub is_file: bool,
    /// A file's bytes.
    pub size: Option<u64>,
    /// The version that last changed it: a commit id on `git://` (for a directory, the last commit under it), the
    /// current version on `dbresource://`.
    pub version_name: Option<String>,
    /// Milliseconds since the epoch.
    pub date: Option<i64>,
    pub user_id: Option<String>,
    pub note: Option<String>,
}

impl ResourceChangeInfo {
    /// `{isFile, size, versionName, date, userId, note}` for a script.
    pub fn to_value(&self) -> Value {
        let mut map = indexmap::IndexMap::new();
        map.insert("isFile".to_string(), Value::Bool(self.is_file));
        map.insert(
            "size".to_string(),
            self.size
                .map_or(Value::Null, |size| Value::I64(i64::try_from(size).unwrap_or(i64::MAX))),
        );
        let text = |value: &Option<String>| value.as_ref().map_or(Value::Null, Value::string);
        map.insert("versionName".to_string(), text(&self.version_name));
        map.insert("date".to_string(), self.date.map_or(Value::Null, Value::I64));
        map.insert("userId".to_string(), text(&self.user_id));
        map.insert("note".to_string(), text(&self.note));
        Value::Map(map)
    }
}

/// One entry of a change log: a version, and the paths it changed under the location asked about (relative to it;
/// empty for a file's own version).
#[derive(Clone, Debug, PartialEq)]
pub struct ChangeLogEntry {
    pub version: ResourceVersion,
    pub paths: Vec<String>,
}

/// The operations a change set applied one at a time needs.
pub(crate) trait ChangeTarget {
    fn stat(&self, location: &str) -> Result<Option<ResourceStat>, Error>;
    fn list(&self, location: &str) -> Result<Vec<ResourceEntry>, Error>;
    fn write_noted(&self, location: &str, bytes: &[u8], note: Option<&str>) -> Result<(), Error>;
    fn delete(&self, location: &str) -> Result<bool, Error>;
    fn make_directory(&self, location: &str) -> Result<(), Error>;
    fn rename(&self, location: &str, new_location: &str) -> Result<(), Error>;
}

struct ProviderTarget<'a, P: ?Sized>(&'a P);

impl<P: ResourceProvider + ?Sized> ChangeTarget for ProviderTarget<'_, P> {
    fn stat(&self, location: &str) -> Result<Option<ResourceStat>, Error> {
        self.0.stat(location)
    }
    fn list(&self, location: &str) -> Result<Vec<ResourceEntry>, Error> {
        self.0.list(location)
    }
    fn write_noted(&self, location: &str, bytes: &[u8], note: Option<&str>) -> Result<(), Error> {
        self.0.write_bytes_noted(location, bytes, note)
    }
    fn delete(&self, location: &str) -> Result<bool, Error> {
        self.0.delete(location)
    }
    fn make_directory(&self, location: &str) -> Result<(), Error> {
        self.0.make_directory(location)
    }
    fn rename(&self, location: &str, new_location: &str) -> Result<(), Error> {
        self.0.rename(location, new_location)
    }
}

struct FacadeTarget<'a>(&'a ResourceFacade);

impl ChangeTarget for FacadeTarget<'_> {
    fn stat(&self, location: &str) -> Result<Option<ResourceStat>, Error> {
        self.0.stat(location)
    }
    fn list(&self, location: &str) -> Result<Vec<ResourceEntry>, Error> {
        self.0.list(location)
    }
    fn write_noted(&self, location: &str, bytes: &[u8], note: Option<&str>) -> Result<(), Error> {
        self.0.write_bytes_noted(location, bytes, note)
    }
    fn delete(&self, location: &str) -> Result<bool, Error> {
        if scheme_of(location) == "dbresource" {
            return Ok(self
                .0
                .db_facade()?
                .delete_db_resource_tree(location)
                .map_err(Error::from)?
                > 0);
        }
        self.0.delete(location)
    }
    fn make_directory(&self, location: &str) -> Result<(), Error> {
        self.0.make_directory(location)
    }
    fn rename(&self, location: &str, new_location: &str) -> Result<(), Error> {
        self.0.rename(location, new_location)
    }
}

/// Delete a file, or a directory after everything under it.
fn delete_recursive(target: &dyn ChangeTarget, location: &str, depth: usize) -> Result<bool, Error> {
    let Some(stat) = target.stat(location)? else {
        return Ok(false);
    };
    if !stat.is_file {
        if depth > 64 {
            return Err(Error::new(format!("directory tree too deep: {location}")));
        }
        for entry in target.list(location)? {
            delete_recursive(target, &entry.location, depth + 1)?;
        }
    }
    target.delete(location)
}

/// Apply `changes` under `root` one at a time, in order: what a storage without change sets of its own does. A change
/// that fails is logged and reported, and the others still apply.
pub(crate) fn apply_changes_each<P: ResourceProvider + ?Sized>(
    provider: &P,
    root: &str,
    changes: &[ResourceChange],
    message: Option<&str>,
) -> ChangeSetOutcome {
    apply_each(&ProviderTarget(provider), root, changes, message)
}

fn apply_each(
    target: &dyn ChangeTarget,
    root: &str,
    changes: &[ResourceChange],
    message: Option<&str>,
) -> ChangeSetOutcome {
    let mut outcome = ChangeSetOutcome::default();
    for change in changes {
        let applied = match change {
            ResourceChange::Write { path, bytes } => target.write_noted(&join_location(root, path), bytes, message),
            ResourceChange::Delete { path } => delete_recursive(target, &join_location(root, path), 0).map(|_| ()),
            ResourceChange::MakeDirectory { path } => target.make_directory(&join_location(root, path)),
            ResourceChange::Move { from, to } => {
                let (from_location, to_location) = (join_location(root, from), join_location(root, to));
                match (target.stat(&from_location), target.stat(&to_location)) {
                    (Ok(None), _) => Err(Error::new(format!("resource not found: {from_location}"))),
                    (_, Ok(Some(_))) => Err(Error::new(format!("already exists: {to_location}"))),
                    (Err(err), _) | (_, Err(err)) => Err(err),
                    (Ok(Some(_)), Ok(None)) => target.rename(&from_location, &to_location),
                }
            }
        };
        match applied {
            Ok(()) => outcome.applied.push(change.path().to_string()),
            Err(err) => {
                tracing::error!(root, path = change.path(), "change set change failed: {err}");
                outcome
                    .failures
                    .push((change.path().to_string(), err.message().to_string()));
            }
        }
    }
    outcome
}

/// Why `change` under `root` is refused: a WebDAV lock another user holds on what it writes, or for a delete or a
/// move on anything under what it takes away.
fn locked(root: &str, change: &ResourceChange) -> Option<String> {
    let check = |path: &str, tree: bool| crate::resource::locks::check_current(&join_location(root, path), tree);
    match change {
        ResourceChange::Write { path, .. } | ResourceChange::MakeDirectory { path } => check(path, false),
        ResourceChange::Delete { path } => check(path, true),
        ResourceChange::Move { from, to } => check(from, true).and_then(|()| check(to, false)),
    }
    .err()
}

/// The last change of a location from its stat, and its current version when the storage keeps versions.
pub(crate) fn last_change_by_stat<P: ResourceProvider + ?Sized>(
    provider: &P,
    location: &str,
) -> Result<Option<ResourceChangeInfo>, Error> {
    let Some(stat) = provider.stat(location)? else {
        return Ok(None);
    };
    let version = if stat.is_file && provider.supports_version(location) {
        provider.current_version(location)?
    } else {
        None
    };
    Ok(Some(info_of(&stat, version.as_ref())))
}

fn info_of(stat: &ResourceStat, version: Option<&ResourceVersion>) -> ResourceChangeInfo {
    ResourceChangeInfo {
        is_file: stat.is_file,
        size: stat.is_file.then_some(stat.size),
        version_name: version.map(|version| version.version_name.clone()),
        date: version
            .and_then(|version| millis_of(&version.version_date))
            .or(stat.last_modified),
        user_id: version.and_then(|version| version.user_id.clone()),
        note: version.and_then(|version| version.note.clone()),
    }
}

/// A version date as milliseconds since the epoch.
pub(crate) fn millis_of(value: &Value) -> Option<i64> {
    match moqui_lib::object::basic_convert(value, "Timestamp").value {
        Value::Timestamp(stamp) => Some(stamp.as_millisecond()),
        _ => None,
    }
}

/// The bytes of every file under a location, by walking it.
pub(crate) fn tree_size_by_walk<P: ResourceProvider + ?Sized>(provider: &P, location: &str) -> Result<u64, Error> {
    let mut total = 0;
    let mut pending = vec![(location.to_string(), 0usize)];
    while let Some((location, depth)) = pending.pop() {
        let Some(stat) = provider.stat(&location)? else {
            continue;
        };
        if stat.is_file {
            total += stat.size;
        } else if depth <= 64 {
            pending.extend(
                provider
                    .list(&location)?
                    .into_iter()
                    .map(|entry| (entry.location, depth + 1)),
            );
        }
    }
    Ok(total)
}

/// The size of a `dbresource://` reference: of its bytes when they were read, else its `fileSize` (else its data
/// length) read now.
pub(super) fn db_file_size(location: &str, snap: &DbSnap) -> Option<u64> {
    if !snap.exists || !snap.is_file {
        return None;
    }
    if let Some(Ok(bytes)) = snap.bytes.get() {
        return u64::try_from(bytes.len()).ok();
    }
    snap.facade
        .as_ref()?
        .stat_db_resource(location)
        .ok()
        .flatten()
        .map(|stat| stat.size)
}

impl ResourceFacade {
    /// Write bytes with a note for the version they make (the commit message on `git://`, `versionNote` on
    /// `dbresource://`). Storage that keeps no versions writes the bytes and drops the note.
    pub fn write_bytes_noted(&self, location: &str, bytes: &[u8], note: Option<&str>) -> Result<(), Error> {
        let note = note.map(str::trim).filter(|note| !note.is_empty());
        super::check_lock(location, false)?;
        if let Some(provider) = self.provider(location) {
            return provider.write_bytes_noted(location, bytes, note);
        }
        if scheme_of(location) == "dbresource" {
            return self
                .db_facade()?
                .write_db_resource_noted(location, bytes, note)
                .map_err(Error::from);
        }
        self.write_bytes(location, bytes)
    }

    /// Apply a change set under the directory `root`: one commit with `message` on a `git://` root, else each change in
    /// order (with `message` as each written file's version note on `dbresource://`). Paths are relative to the root;
    /// one that is empty or has a `.` or `..` segment is refused as a failure. A change that fails is reported in the
    /// outcome and the others still apply.
    pub fn apply_change_set(
        &self,
        root: &str,
        changes: &[ResourceChange],
        message: Option<&str>,
    ) -> Result<ChangeSetOutcome, Error> {
        let root = strip_hash(root).trim_end_matches('/');
        let message = message.map(str::trim).filter(|message| !message.is_empty());
        let mut refused = Vec::new();
        let mut valid = Vec::with_capacity(changes.len());
        for change in changes {
            match change.normalized() {
                // a change of a location another user holds a WebDAV lock on is refused, and only that change
                Ok(change) => match locked(root, &change) {
                    Some(reason) => refused.push((change.path().to_string(), reason)),
                    None => valid.push(change),
                },
                Err(reason) => refused.push((change.path().to_string(), reason)),
            }
        }
        let mut outcome = if valid.is_empty() {
            ChangeSetOutcome::default()
        } else if let Some(provider) = self.provider(root) {
            provider.apply_changes(root, &valid, message)?
        } else {
            apply_each(&FacadeTarget(self), root, &valid, message)
        };
        for change in &valid {
            let taken = match change {
                ResourceChange::Delete { path } | ResourceChange::Move { from: path, .. } => path,
                _ => continue,
            };
            if !outcome.failures.iter().any(|(path, _)| path == change.path()) {
                crate::resource::locks::release(&join_location(root, taken));
            }
        }
        outcome.failures.extend(refused);
        Ok(outcome)
    }

    /// The last change of a file or directory, or `None` when nothing is there: on `git://` the commit that last
    /// changed it (a directory: anything under it), on `dbresource://` a file's current version, elsewhere the stat.
    pub fn last_change(&self, location: &str) -> Result<Option<ResourceChangeInfo>, Error> {
        let location = strip_hash(location);
        if let Some(provider) = self.provider(location) {
            return provider.last_change(location);
        }
        if scheme_of(location) == "dbresource" {
            return self.db_facade()?.db_resource_last_change(location).map_err(Error::from);
        }
        Ok(self.stat(location)?.map(|stat| info_of(&stat, None)))
    }

    /// [`Self::last_change`] of each entry of a directory, by name, in name order.
    pub fn child_changes(&self, location: &str) -> Result<Vec<(String, ResourceChangeInfo)>, Error> {
        let location = strip_hash(location);
        let mut found = if let Some(provider) = self.provider(location) {
            provider.child_changes(location)?
        } else if scheme_of(location) == "dbresource" {
            self.db_facade()?
                .db_resource_child_changes(location)
                .map_err(Error::from)?
        } else {
            let mut found = Vec::new();
            for entry in self.list(location)? {
                if let Some(stat) = self.stat(&entry.location)? {
                    found.push((entry.name, info_of(&stat, None)));
                }
            }
            found
        };
        found.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(found)
    }

    /// The versions under a location, newest first, at most `limit`: on `git://` every commit that changed the file
    /// or anything under the directory, with the paths it changed; on `dbresource://` a file's versions, or for a
    /// directory the versions of the files under it (the first 500 files) merged by date.
    pub fn change_log(&self, location: &str, limit: usize) -> Result<Vec<ChangeLogEntry>, Error> {
        let location = strip_hash(location).trim_end_matches('/');
        if let Some(provider) = self.provider(location) {
            return provider.change_log(location, limit);
        }
        if scheme_of(location) != "dbresource" {
            return Ok(Vec::new());
        }
        let Some(stat) = self.stat(location)? else {
            return Ok(Vec::new());
        };
        if stat.is_file {
            return Ok(self
                .version_history(location)?
                .into_iter()
                .take(limit)
                .map(|version| ChangeLogEntry {
                    version,
                    paths: Vec::new(),
                })
                .collect());
        }
        let facade = self.db_facade()?;
        let mut entries = Vec::new();
        for path in facade.db_resource_files_under(location, 500).map_err(Error::from)? {
            let file = join_location(location, &path);
            for version in self.version_history(&file)?.into_iter().take(limit) {
                entries.push(ChangeLogEntry {
                    version,
                    paths: vec![path.clone()],
                });
            }
        }
        entries.sort_by_key(|entry| std::cmp::Reverse(millis_of(&entry.version.version_date).unwrap_or(0)));
        entries.truncate(limit);
        Ok(entries)
    }

    /// Write a file as it was in `version_name` back as its newest version, with a note (a restore never rewrites
    /// history). On `git://` the entry of that commit is put back in one commit, so a directory deleted in a later
    /// commit comes back with everything under it. The new version's name.
    pub fn restore_version(
        &self,
        location: &str,
        version_name: &str,
        note: Option<&str>,
    ) -> Result<Option<String>, Error> {
        let location = strip_hash(location);
        let note = note.map(str::trim).filter(|note| !note.is_empty());
        super::check_lock(location, true)?;
        if let Some(provider) = self.provider(location) {
            return provider.restore_version(location, version_name, note);
        }
        let bytes = self
            .read_version(location, version_name)?
            .ok_or_else(|| Error::new(format!("version {version_name} not found: {location}")))?;
        self.write_bytes_noted(location, &bytes, note)?;
        Ok(self.current_version(location)?.map(|version| version.version_name))
    }

    /// The bytes of every file under a location (a file: its size), for a quota.
    pub fn tree_size(&self, location: &str) -> Result<u64, Error> {
        let location = strip_hash(location);
        if let Some(provider) = self.provider(location) {
            return provider.tree_size(location);
        }
        if scheme_of(location) == "dbresource" {
            return self.db_facade()?.db_resource_tree_size(location).map_err(Error::from);
        }
        let mut total = 0;
        let mut pending = vec![(location.to_string(), 0usize)];
        while let Some((location, depth)) = pending.pop() {
            let Some(stat) = self.stat(&location)? else {
                continue;
            };
            if stat.is_file {
                total += stat.size;
            } else if depth <= 64 {
                pending.extend(
                    self.list(&location)?
                        .into_iter()
                        .map(|entry| (entry.location, depth + 1)),
                );
            }
        }
        Ok(total)
    }
}
