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

//! What a commit changed: the files added, modified and deleted between two trees (a commit's and its first
//! parent's), and the line diff of one changed file through `resource::textdiff` (stats, unified text, the escaped
//! `moqui-diff` HTML table). A file is binary when either side has a NUL in its first 8000 bytes or is not UTF-8, as
//! git's own check; a side over [`MAX_DIFF_BYTES`] is not diffed.

use std::collections::BTreeMap;

use gix::bstr::ByteSlice;
use gix::objs::tree::EntryMode;
use gix::objs::Kind;
use gix::ObjectId;

use super::ops;
use super::store::GitStore;
use moqui_core::context::Error;
use moqui_core::resource::textdiff::{self, TextDiff};

/// The largest side of a file that is diffed line by line.
pub(crate) const MAX_DIFF_BYTES: usize = 1024 * 1024;

/// How a file changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

impl ChangeKind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
        }
    }
}

/// One changed file (a blob, link or submodule entry; directories are walked into).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileChange {
    pub path: String,
    pub kind: ChangeKind,
    pub old: Option<(EntryMode, ObjectId)>,
    pub new: Option<(EntryMode, ObjectId)>,
}

/// The line diff of one changed file, or why there is none.
#[derive(Clone, Debug)]
pub(crate) struct FileDiff {
    pub binary: bool,
    /// A side is over [`MAX_DIFF_BYTES`].
    pub too_large: bool,
    pub diff: Option<TextDiff>,
}

type Entries = BTreeMap<Vec<u8>, (EntryMode, ObjectId)>;

fn entries(store: &dyn GitStore, tree: Option<&ObjectId>) -> Result<Entries, Error> {
    let Some(tree) = tree else {
        return Ok(Entries::new());
    };
    Ok(ops::read_tree(store, tree)?
        .entries
        .into_iter()
        .map(|entry| (entry.filename.to_vec(), (entry.mode, entry.oid)))
        .collect())
}

fn join(prefix: &str, name: &[u8]) -> String {
    let name = name.to_str_lossy();
    if prefix.is_empty() {
        name.into_owned()
    } else {
        format!("{prefix}/{name}")
    }
}

fn walk(
    store: &dyn GitStore,
    prefix: &str,
    old: Option<&ObjectId>,
    new: Option<&ObjectId>,
    changes: &mut Vec<FileChange>,
) -> Result<(), Error> {
    let (old, new) = (entries(store, old)?, entries(store, new)?);
    let mut names: Vec<&Vec<u8>> = old.keys().chain(new.keys()).collect();
    names.sort();
    names.dedup();
    for name in names {
        let path = join(prefix, name);
        let (before, after) = (old.get(name).copied(), new.get(name).copied());
        if before == after {
            continue;
        }
        let tree_before = before.filter(|(mode, _)| mode.is_tree());
        let tree_after = after.filter(|(mode, _)| mode.is_tree());
        if tree_before.is_some() || tree_after.is_some() {
            walk(
                store,
                &path,
                tree_before.map(|(_, id)| id).as_ref(),
                tree_after.map(|(_, id)| id).as_ref(),
                changes,
            )?;
        }
        let file_before = before.filter(|(mode, _)| !mode.is_tree());
        let file_after = after.filter(|(mode, _)| !mode.is_tree());
        let kind = match (file_before, file_after) {
            (None, None) => continue,
            (None, Some(_)) => ChangeKind::Added,
            (Some(_), None) => ChangeKind::Deleted,
            (Some(_), Some(_)) => ChangeKind::Modified,
        };
        changes.push(FileChange {
            path,
            kind,
            old: file_before,
            new: file_after,
        });
    }
    Ok(())
}

/// The files that differ from tree `old` (`None`: an empty tree, as for a root commit) to tree `new`, by path.
pub(crate) fn tree_changes(
    store: &dyn GitStore,
    old: Option<&ObjectId>,
    new: Option<&ObjectId>,
) -> Result<Vec<FileChange>, Error> {
    let mut changes = Vec::new();
    walk(store, "", old, new, &mut changes)?;
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}

/// The files commit `commit` changed against its first parent (all of its files for a root commit).
pub(crate) fn commit_changes(store: &dyn GitStore, commit: &ops::CommitInfo) -> Result<Vec<FileChange>, Error> {
    let parent_tree = match commit.parents.first() {
        Some(parent) => Some(ops::read_commit(store, parent)?.tree),
        None => None,
    };
    tree_changes(store, parent_tree.as_ref(), Some(&commit.tree))
}

/// The text of one side of a change, or why it is not diffed.
enum Side {
    Text(String),
    Binary,
    TooLarge,
}

fn side(store: &dyn GitStore, entry: Option<(EntryMode, ObjectId)>) -> Result<Side, Error> {
    let Some((mode, id)) = entry else {
        return Ok(Side::Text(String::new()));
    };
    if mode.is_commit() {
        // a submodule entry: show the commit it names, as git does
        return Ok(Side::Text(format!("Subproject commit {id}\n")));
    }
    if store.object_size(&id)?.is_some_and(|size| size > MAX_DIFF_BYTES as u64) {
        return Ok(Side::TooLarge);
    }
    let data = ops::object(store, &id, Kind::Blob)?;
    if data.iter().take(8000).any(|byte| *byte == 0) {
        return Ok(Side::Binary);
    }
    match String::from_utf8(data) {
        Ok(text) => Ok(Side::Text(text)),
        Err(_) => Ok(Side::Binary),
    }
}

/// The line diff of one change, with `a/<path>` and `b/<path>` as its names.
pub(crate) fn file_diff(store: &dyn GitStore, change: &FileChange) -> Result<FileDiff, Error> {
    let (old, new) = (side(store, change.old)?, side(store, change.new)?);
    Ok(match (old, new) {
        (Side::Text(old), Side::Text(new)) => FileDiff {
            binary: false,
            too_large: false,
            diff: Some(textdiff::diff(
                &old,
                &new,
                &format!("a/{}", change.path),
                &format!("b/{}", change.path),
            )),
        },
        (Side::TooLarge, _) | (_, Side::TooLarge) => FileDiff {
            binary: false,
            too_large: true,
            diff: None,
        },
        _ => FileDiff {
            binary: true,
            too_large: false,
            diff: None,
        },
    })
}

#[cfg(test)]
mod tests {
    use gix::objs::tree::EntryKind;

    use super::super::ops::{commit, head, Author, Edit};
    use super::super::store::MemoryStore;
    use super::*;

    fn path(text: &str) -> Vec<String> {
        text.split('/').map(str::to_string).collect()
    }

    fn change(store: &MemoryStore, edits: Vec<Edit>) -> ops::CommitInfo {
        let author = Author {
            name: "Test".into(),
            email: "test@example.com".into(),
            user_id: None,
            seconds: None,
        };
        commit(store, "main", &author, |_| Ok(Some((edits.clone(), "change".into()))))
            .unwrap()
            .unwrap();
        head(store, "main").unwrap().unwrap()
    }

    fn put(store: &MemoryStore, at: &str, data: &[u8]) -> Edit {
        Edit::Put(path(at), EntryKind::Blob, store.write(Kind::Blob, data).unwrap())
    }

    #[test]
    fn changes_walk_into_directories_and_diff_text() {
        let store = MemoryStore::default();
        let root = change(
            &store,
            vec![put(&store, "a.txt", b"one\ntwo\n"), put(&store, "dir/b.txt", b"b\n")],
        );
        let found: Vec<(String, ChangeKind)> = commit_changes(&store, &root)
            .unwrap()
            .into_iter()
            .map(|change| (change.path, change.kind))
            .collect();
        assert_eq!(
            found,
            [
                ("a.txt".to_string(), ChangeKind::Added),
                ("dir/b.txt".to_string(), ChangeKind::Added)
            ]
        );
        let next = change(
            &store,
            vec![
                put(&store, "a.txt", b"one\n2\n"),
                Edit::Remove(path("dir/b.txt")),
                put(&store, "dir", b"now a file\n"),
                put(&store, "bin.dat", b"\0\x01\x02"),
            ],
        );
        let changes = commit_changes(&store, &next).unwrap();
        let found: Vec<(&str, ChangeKind)> = changes
            .iter()
            .map(|change| (change.path.as_str(), change.kind))
            .collect();
        assert_eq!(
            found,
            [
                ("a.txt", ChangeKind::Modified),
                ("bin.dat", ChangeKind::Added),
                ("dir", ChangeKind::Added),
                ("dir/b.txt", ChangeKind::Deleted)
            ]
        );
        let text = file_diff(&store, &changes[0]).unwrap();
        let diff = text.diff.expect("a text diff");
        assert_eq!((diff.stats.insertions, diff.stats.deletions), (1, 1));
        assert!(diff.unified().contains("--- a/a.txt"), "{}", diff.unified());
        assert!(file_diff(&store, &changes[1]).unwrap().binary);
    }
}
