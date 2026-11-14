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

//! Blame (`get#GitBlame`; plans/Dogfood.md, N-BLAME): for each line of a file at a commit, the commit that last
//! changed it. The lines start at the commit asked for; each commit passes the lines it did not change to the parent
//! that has them (the first parent whose file is the same takes them all, as git's history simplification does), and
//! keeps the rest. Renames are not followed: a file is blamed as far back as it has its path.
//!
//! What a blame costs is bounded by [`Limits`], and it is refused past any of them: the file's bytes and lines, the
//! commits read (a commit reached again is counted again), the comparisons of two versions (each parent a merge is
//! compared with is one), and the time, which every comparison is cut off at. Within them a blame of a large file
//! with a long history is still seconds of one service thread: the limits are the conf's to set.

use std::collections::{BinaryHeap, HashMap};
use std::time::{Duration, Instant};

use gix::objs::Kind;
use gix::ObjectId;

use super::ops;
use super::store::GitStore;
use moqui_core::context::Error;
use moqui_core::resource::textdiff;

/// What a blame may read.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    /// The largest file (`git-server` `blame-max-bytes`).
    pub max_bytes: u64,
    /// The most lines of the file, and of an older version compared with it (`git-server` `blame-max-lines`).
    pub max_lines: usize,
    /// The most commits read (`git-server` `blame-max-commits`).
    pub max_commits: usize,
    /// The most comparisons of two versions of the file (`git-server` `blame-max-diffs`).
    pub max_diffs: usize,
    /// The longest a blame runs (`git-server` `blame-max-seconds`).
    pub max_time: Duration,
}

/// `git-server` `blame-max-bytes` when the conf has none: 1 MiB, as the largest file diffed.
pub(crate) const DEFAULT_MAX_BYTES: u64 = 1024 * 1024;
/// `git-server` `blame-max-lines` when the conf has none.
pub(crate) const DEFAULT_MAX_LINES: usize = 50_000;
/// `git-server` `blame-max-commits` when the conf has none.
pub(crate) const DEFAULT_MAX_COMMITS: usize = 10_000;
/// `git-server` `blame-max-diffs` when the conf has none.
pub(crate) const DEFAULT_MAX_DIFFS: usize = 2_000;
/// `git-server` `blame-max-seconds` when the conf has none.
pub(crate) const DEFAULT_MAX_SECONDS: u64 = 10;

/// A run of lines one commit last changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BlameHunk {
    /// The first line, from 1.
    pub start: usize,
    pub lines: usize,
    pub commit: ObjectId,
}

/// The blame of a file.
#[derive(Clone, Debug)]
pub(crate) struct Blame {
    pub text: String,
    pub hunks: Vec<BlameHunk>,
}

/// A line of a suspect commit's file that is a line of the file blamed.
#[derive(Clone, Copy, Debug)]
struct Pending {
    /// The line in the suspect's file, from 0.
    at: usize,
    /// The line in the file blamed, from 0.
    line: usize,
}

/// How many lines `text` has, as [`textdiff::split_lines`] splits it.
fn line_count(text: &str) -> usize {
    text.bytes().filter(|byte| *byte == b'\n').count() + usize::from(!text.is_empty() && !text.ends_with('\n'))
}

/// The text of the file `blob`, or `None` when it is over the limits, binary or not UTF-8.
fn text_of(store: &dyn GitStore, blob: &ObjectId, limits: Limits) -> Result<Option<String>, Error> {
    if store.object_size(blob)?.is_none_or(|size| size > limits.max_bytes) {
        return Ok(None);
    }
    let data = ops::object(store, blob, Kind::Blob)?;
    if data.iter().take(8000).any(|byte| *byte == 0) {
        return Ok(None);
    }
    Ok(String::from_utf8(data)
        .ok()
        .filter(|text| line_count(text) <= limits.max_lines))
}

/// The file's blob at `path` in `tree`; `None` when the path is not a file there (a directory, a submodule, nothing).
fn file_at(store: &dyn GitStore, tree: &ObjectId, path: &[String]) -> Result<Option<ObjectId>, Error> {
    Ok(ops::resolve(store, tree, path)?
        .filter(|(mode, _)| !mode.is_tree() && !mode.is_commit())
        .map(|(_, id)| id))
}

/// Blame the file at `path` in `commit`. `None` when the path is not a file there.
///
/// # Errors
///
/// A file over `limits.max_bytes` or `limits.max_lines`, a binary file, a file kept in Git LFS (its lines are not in
/// the repository), and a history that needs more than `limits.max_commits` commits read, more than
/// `limits.max_diffs` versions compared, or longer than `limits.max_time`.
pub(crate) fn blame(
    store: &dyn GitStore,
    commit: &ObjectId,
    path: &[String],
    limits: Limits,
) -> Result<Option<Blame>, Error> {
    let shown = path.join("/");
    let start = ops::read_commit(store, commit)?;
    let Some(blob) = file_at(store, &start.tree, path)? else {
        return Ok(None);
    };
    if store.object_size(&blob)?.is_some_and(|size| size > limits.max_bytes) {
        return Err(Error::new(format!(
            "{shown} is over {} bytes: too large to blame",
            limits.max_bytes
        )));
    }
    let deadline = Instant::now() + limits.max_time;
    let late = || {
        Error::new(format!(
            "the blame of {shown} takes longer than {} seconds",
            limits.max_time.as_secs()
        ))
    };
    let any_length = Limits {
        max_lines: usize::MAX,
        ..limits
    };
    let Some(text) = text_of(store, &blob, any_length)? else {
        return Err(Error::new(format!(
            "{shown} is a binary file: it has no lines to blame"
        )));
    };
    if line_count(&text) > limits.max_lines {
        return Err(Error::new(format!(
            "{shown} has more than {} lines: too long to blame",
            limits.max_lines
        )));
    }
    if super::server::lfs::pointer(text.as_bytes()).is_some() {
        return Err(Error::new(format!(
            "{shown} is kept in Git LFS: it has no line history"
        )));
    }
    let line_count = line_count(&text);
    let mut blamed: Vec<Option<ObjectId>> = vec![None; line_count];
    // the commits that still hold lines, newest first, each with its lines
    let mut pending: HashMap<ObjectId, Vec<Pending>> = HashMap::new();
    let mut queue: BinaryHeap<(i64, ObjectId)> = BinaryHeap::new();
    pending.insert(
        *commit,
        (0..line_count).map(|line| Pending { at: line, line }).collect(),
    );
    queue.push((start.seconds, *commit));
    let mut read = 0usize;
    let mut diffs = 0usize;
    while let Some((_, id)) = queue.pop() {
        // a commit reached by two children is queued twice; its lines are taken once
        let Some(mut lines) = pending.remove(&id) else {
            continue;
        };
        read += 1;
        if read > limits.max_commits {
            return Err(Error::new(format!(
                "the history of {shown} is longer than {} commits: too long to blame",
                limits.max_commits
            )));
        }
        if Instant::now() > deadline {
            return Err(late());
        }
        let info = ops::read_commit(store, &id)?;
        let own = file_at(store, &info.tree, path)?;
        let mut own_text: Option<Option<String>> = None;
        for parent_id in &info.parents {
            if lines.is_empty() {
                break;
            }
            let parent = ops::read_commit(store, parent_id)?;
            let Some(theirs) = file_at(store, &parent.tree, path)? else {
                continue;
            };
            let passed: Vec<Pending> = if Some(theirs) == own {
                std::mem::take(&mut lines)
            } else {
                let mine = match &own_text {
                    Some(mine) => mine,
                    None => {
                        let read = match own {
                            Some(own) => text_of(store, &own, limits)?,
                            None => None,
                        };
                        own_text.insert(read)
                    }
                };
                // a side that is not text (binary, over a limit) shares no lines: this commit keeps them
                let (Some(mine), Some(theirs)) = (mine.as_deref(), text_of(store, &theirs, limits)?) else {
                    continue;
                };
                // every comparison counts, each parent of a merge and a commit reached again alike
                diffs += 1;
                if diffs > limits.max_diffs {
                    return Err(Error::new(format!(
                        "the blame of {shown} compares more than {} versions: too many changes to blame",
                        limits.max_diffs
                    )));
                }
                let origins = textdiff::line_origins(&theirs, mine, Some(deadline));
                // a comparison cut off at the deadline is not the answer: the blame is refused, not made worse
                if Instant::now() > deadline {
                    return Err(late());
                }
                let mut passed = Vec::new();
                lines.retain(|pending| match origins.get(pending.at).copied().flatten() {
                    Some(at) => {
                        passed.push(Pending { at, line: pending.line });
                        false
                    }
                    None => true,
                });
                passed
            };
            if passed.is_empty() {
                continue;
            }
            let waiting = pending.entry(*parent_id).or_default();
            if waiting.is_empty() {
                queue.push((parent.seconds, *parent_id));
            }
            waiting.extend(passed);
        }
        for kept in lines {
            if let Some(slot) = blamed.get_mut(kept.line) {
                *slot = Some(id);
            }
        }
    }
    let mut hunks: Vec<BlameHunk> = Vec::new();
    for (line, by) in blamed.iter().enumerate() {
        // every line ends at a commit: one with no parent keeps what it has
        let by = by.unwrap_or(*commit);
        match hunks.last_mut() {
            Some(last) if last.commit == by => last.lines += 1,
            _ => hunks.push(BlameHunk {
                start: line + 1,
                lines: 1,
                commit: by,
            }),
        }
    }
    Ok(Some(Blame { text, hunks }))
}

/// A file's blame with the commits it names.
#[derive(Clone, Debug)]
pub(crate) struct Blamed {
    /// The commit the revision named.
    pub commit: ObjectId,
    pub blame: Blame,
    /// Each commit of the hunks, once.
    pub commits: Vec<ops::CommitDetails>,
}

impl super::GitProvider {
    /// The blame of the file at `path` in the commit `revision` names ([`blame`]); `None` when the revision names
    /// nothing or the path is not a file there.
    pub(crate) fn blame(
        &self,
        repository_id: &str,
        revision: &str,
        path: &[String],
        limits: Limits,
    ) -> Result<Option<Blamed>, Error> {
        let repo = self.repo(repository_id)?;
        let store = repo.store.as_ref();
        let Some(commit) = self.resolve_commit(repository_id, revision)? else {
            return Ok(None);
        };
        let Some(found) = blame(store, &commit, path, limits)? else {
            return Ok(None);
        };
        let mut seen = std::collections::HashSet::new();
        let mut commits = Vec::new();
        for hunk in &found.hunks {
            if seen.insert(hunk.commit) {
                commits.push(ops::commit_details(store, &hunk.commit)?);
            }
        }
        Ok(Some(Blamed {
            commit,
            blame: found,
            commits,
        }))
    }
}

#[cfg(test)]
#[path = "blame_tests.rs"]
mod tests;
