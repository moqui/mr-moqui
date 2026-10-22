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

//! Merging two commits of a repository, over any [`GitStore`]: their merge base, a three-way merge of trees through
//! gix-merge (git's `merge-ort` rules: renames, content merges, the conflicts it cannot resolve), and the four merge
//! methods of `merge#GitRefs`: a merge commit, a fast-forward, a squash (one commit of the source's changes) and a
//! rebase (the source's commits replayed on the target). Nothing here moves a ref: the caller does, under the
//! repository's lock, with the rights, branch rules and gitECA events of a push. New objects are made in an
//! [`Overlay`] and written to the store only when the merge is made, so a trial merge leaves nothing behind.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use gix::bstr::{BStr, ByteSlice};
use gix::objs::{Kind, WriteTo};
use gix::ObjectId;

use super::ops::{self, Author, CommitDetails};
use super::revwalk::{self, new_commits};
use super::store::{object_id, GitStore, RefEdit, RefOutcome};
use crate::context::Error;

/// How a source is merged into a target: the values of `GitMergeRequest.mergeMethodEnumId`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Method {
    /// A merge commit with the target and the source as its parents (also when a fast-forward was possible).
    Merge,
    /// One commit on the target with the merged tree.
    Squash,
    /// The target moved to the source; refused when the target has commits the source lacks.
    FastForward,
    /// Each commit of the source not in the target replayed on the target, oldest first.
    Rebase,
}

impl Method {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        Some(match text.trim() {
            "" | "GmmMerge" | "merge" => Self::Merge,
            "GmmSquash" | "squash" => Self::Squash,
            "GmmFastForward" | "fast-forward" | "ff" => Self::FastForward,
            "GmmRebase" | "rebase" => Self::Rebase,
            _ => return None,
        })
    }

    pub(crate) fn enum_id(self) -> &'static str {
        match self {
            Self::Merge => "GmmMerge",
            Self::Squash => "GmmSquash",
            Self::FastForward => "GmmFastForward",
            Self::Rebase => "GmmRebase",
        }
    }
}

/// Objects made by a merge over a store's own: read through to the store, written here until [`Overlay::flush`].
#[derive(Debug)]
pub(crate) struct Overlay<'a> {
    base: &'a dyn GitStore,
    written: Mutex<HashMap<ObjectId, (Kind, Vec<u8>)>>,
    /// The order they were written in, so a flush writes a tree's entries before the tree.
    order: Mutex<Vec<ObjectId>>,
}

impl<'a> Overlay<'a> {
    pub(crate) fn new(base: &'a dyn GitStore) -> Self {
        Self {
            base,
            written: Mutex::new(HashMap::new()),
            order: Mutex::new(Vec::new()),
        }
    }

    /// Write every object made here to the store. How many.
    pub(crate) fn flush(&self) -> Result<usize, Error> {
        let order = std::mem::take(&mut *moqui_lib::sync::lock(&self.order));
        let mut written = moqui_lib::sync::lock(&self.written);
        let mut count = 0;
        for id in order {
            if let Some((kind, data)) = written.remove(&id) {
                self.base.write(kind, &data)?;
                count += 1;
            }
        }
        Ok(count)
    }
}

impl GitStore for Overlay<'_> {
    fn find(&self, id: &ObjectId) -> Result<Option<(Kind, Vec<u8>)>, Error> {
        if let Some((kind, data)) = moqui_lib::sync::lock(&self.written).get(id) {
            return Ok(Some((*kind, data.clone())));
        }
        self.base.find(id)
    }

    fn object_size(&self, id: &ObjectId) -> Result<Option<u64>, Error> {
        if let Some((_, data)) = moqui_lib::sync::lock(&self.written).get(id) {
            return Ok(Some(data.len() as u64));
        }
        self.base.object_size(id)
    }

    fn find_prefix(&self, prefix: &str, limit: usize) -> Result<Vec<ObjectId>, Error> {
        self.base.find_prefix(prefix, limit)
    }

    fn write(&self, kind: Kind, data: &[u8]) -> Result<ObjectId, Error> {
        let id = object_id(kind, data)?;
        let mut written = moqui_lib::sync::lock(&self.written);
        if let std::collections::hash_map::Entry::Vacant(slot) = written.entry(id) {
            slot.insert((kind, data.to_vec()));
            moqui_lib::sync::lock(&self.order).push(id);
        }
        Ok(id)
    }

    fn read_ref(&self, name: &str) -> Result<Option<ObjectId>, Error> {
        self.base.read_ref(name)
    }

    fn list_refs(&self, prefix: &str) -> Result<Vec<(String, ObjectId)>, Error> {
        self.base.list_refs(prefix)
    }

    fn update_refs(&self, _edits: &[RefEdit], _atomic: bool) -> Result<Vec<RefOutcome>, Error> {
        Err(Error::new("a merge overlay does not move refs"))
    }
}

/// [`GitStore`] as gix's object traits, for gix-merge.
struct Objects<'a> {
    store: &'a dyn GitStore,
}

fn gix_error(err: Error) -> gix::Error {
    gix::Error::from_error(std::io::Error::other(err.message().to_string()))
}

impl gix::objs::Find for Objects<'_> {
    fn try_find<'b>(&self, id: &gix::hash::oid, buffer: &'b mut Vec<u8>) -> gix::Result<Option<gix::objs::Data<'b>>> {
        match self.store.find(&id.to_owned()).map_err(gix_error)? {
            Some((kind, data)) => {
                *buffer = data;
                Ok(Some(gix::objs::Data::new(buffer, kind, gix::hash::Kind::Sha1)))
            }
            None => Ok(None),
        }
    }
}

impl gix::objs::FindHeader for Objects<'_> {
    fn try_header(&self, id: &gix::hash::oid) -> gix::Result<Option<gix::objs::Header>> {
        Ok(self
            .store
            .find(&id.to_owned())
            .map_err(gix_error)?
            .map(|(kind, data)| gix::objs::Header {
                kind,
                size: data.len() as u64,
            }))
    }
}

/// An attribute stack with no attributes: merges read no `.gitattributes` (every file is merged as text unless it
/// is binary, as git does without attributes).
fn attribute_stack() -> gix::worktree::Stack {
    use gix::worktree::stack::state::{attributes::Source, Attributes};
    gix::worktree::Stack::new(
        std::path::PathBuf::new(),
        gix::worktree::stack::State::AttributesStack(Attributes::new(
            Default::default(),
            None,
            Source::IdMapping,
            Default::default(),
        )),
        gix::glob::pattern::Case::Sensitive,
        Vec::new(),
        Vec::new(),
    )
}

fn filter_pipeline() -> gix::filter::plumbing::Pipeline {
    gix::filter::plumbing::Pipeline::new(Default::default(), gix::hash::Kind::Sha1, Default::default())
}

/// What a three-way merge of trees gave: the merged tree when nothing conflicted, else the conflicting paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TreeMerge {
    Clean(ObjectId),
    Conflicts(Vec<String>),
}

/// Merge `theirs` into `ours`, both changed from `base` (`None`: no common history, the empty tree). The merged blobs
/// and trees are written to `store` (an [`Overlay`], for a merge that may not be kept).
pub(crate) fn merge_trees(
    store: &dyn GitStore,
    base: Option<&ObjectId>,
    ours: &ObjectId,
    theirs: &ObjectId,
    labels: (&str, &str),
) -> Result<TreeMerge, Error> {
    let empty;
    let base = match base {
        Some(base) => base,
        None => {
            empty = store.write(Kind::Tree, b"")?;
            &empty
        }
    };
    let objects = Objects { store };
    let mut diff_cache = gix::diff::blob::Platform::new(
        gix::diff::blob::platform::Options {
            algorithm: Some(gix::diff::blob::Algorithm::Histogram),
            skip_internal_diff_if_external_is_configured: false,
        },
        gix::diff::blob::Pipeline::new(Default::default(), filter_pipeline(), Vec::new(), Default::default()),
        gix::diff::blob::pipeline::Mode::ToGit,
        attribute_stack(),
    );
    let mut blob_merge = gix::merge::blob::Platform::new(
        gix::merge::blob::Pipeline::new(Default::default(), filter_pipeline(), Default::default()),
        gix::merge::blob::pipeline::Mode::ToGit,
        attribute_stack(),
        Vec::new(),
        Default::default(),
    );
    let options = gix::merge::plumbing::tree::Options {
        rewrites: Some(Default::default()),
        blob_merge: gix::merge::blob::platform::merge::Options {
            is_virtual_ancestor: false,
            resolve_binary_with: None,
            text: gix::merge::blob::builtin_driver::text::Options {
                diff_algorithm: gix::diff::blob::Algorithm::Histogram,
                conflict: gix::merge::blob::builtin_driver::text::Conflict::Keep {
                    style: Default::default(),
                    marker_size: gix::merge::blob::builtin_driver::text::Conflict::DEFAULT_MARKER_SIZE
                        .try_into()
                        .unwrap_or(std::num::NonZeroU8::MIN),
                },
            },
        },
        blob_merge_command_ctx: Default::default(),
        fail_on_conflict: None,
        marker_size_multiplier: 0,
        symlink_conflicts: None,
        tree_conflicts: None,
    };
    let labels = gix::merge::blob::builtin_driver::text::Labels {
        ancestor: Some(BStr::new("base")),
        current: Some(labels.0.as_bytes().as_bstr()),
        other: Some(labels.1.as_bytes().as_bstr()),
    };
    let mut outcome = gix::merge::plumbing::tree(
        base,
        ours,
        theirs,
        labels,
        &objects,
        |data: &[u8]| store.write(Kind::Blob, data).map_err(gix_error),
        &mut Default::default(),
        &mut diff_cache,
        &mut blob_merge,
        options,
    )
    .map_err(|err| Error::new(format!("merging trees: {err}")))?;
    let how = gix::merge::plumbing::tree::TreatAsUnresolved::git();
    if outcome.has_unresolved_conflicts(how) {
        let mut paths: Vec<String> = outcome
            .conflicts
            .iter()
            .filter(|conflict| conflict.is_unresolved(how))
            .flat_map(|conflict| [conflict.ours.location(), conflict.theirs.location()])
            .map(|location| location.to_str_lossy().into_owned())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        paths.sort();
        return Ok(TreeMerge::Conflicts(paths));
    }
    let tree = outcome
        .tree
        .write(|tree: &gix::objs::Tree| -> Result<ObjectId, Error> {
            let mut buf = Vec::with_capacity(64 * tree.entries.len());
            tree.write_to(&mut buf)
                .map_err(|err| Error::new(format!("encoding a git tree: {err}")))?;
            store.write(Kind::Tree, &buf)
        })
        .map_err(|err| Error::new(format!("writing a merged tree: {err}")))?;
    Ok(TreeMerge::Clean(tree))
}

/// The best common ancestor of two commits: of the commits both reach, one no other of them reaches (the newest
/// when there are several, as in a criss-cross history). `None` when they share no history.
pub(crate) fn merge_base(store: &dyn GitStore, one: &ObjectId, other: &ObjectId) -> Result<Option<ObjectId>, Error> {
    if revwalk::is_ancestor(store, one, other)? {
        return Ok(Some(*one));
    }
    if revwalk::is_ancestor(store, other, one)? {
        return Ok(Some(*other));
    }
    // the commits only `other` reaches; their parents outside that set are the common ancestors at its edge
    let only: Vec<ops::CommitInfo> = new_commits(store, &[*other], &[*one])?;
    let inside: HashSet<ObjectId> = only.iter().map(|commit| commit.id).collect();
    let mut edge: Vec<ObjectId> = Vec::new();
    for commit in &only {
        for parent in &commit.parents {
            if !inside.contains(parent) && !edge.contains(parent) {
                edge.push(*parent);
            }
        }
    }
    let mut best: Vec<ObjectId> = Vec::new();
    for candidate in &edge {
        let mut reached = false;
        for other_candidate in &edge {
            if other_candidate != candidate && revwalk::is_ancestor(store, candidate, other_candidate)? {
                reached = true;
                break;
            }
        }
        if !reached {
            best.push(*candidate);
        }
    }
    let mut dated = Vec::with_capacity(best.len());
    for id in best {
        dated.push((ops::read_commit(store, &id)?.seconds, id));
    }
    dated.sort();
    Ok(dated.last().map(|(_, id)| *id))
}

/// How a source stands against a target before a merge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Mergeability {
    pub base: Option<ObjectId>,
    /// The target already has every commit of the source.
    pub merged: bool,
    /// The target is an ancestor of the source.
    pub fast_forward: bool,
    /// The paths a merge cannot resolve; empty when it can merge.
    pub conflicts: Vec<String>,
}

/// Whether `source` merges into `target`, by a trial merge in an overlay that is dropped.
pub(crate) fn check(store: &dyn GitStore, source: &ObjectId, target: &ObjectId) -> Result<Mergeability, Error> {
    if revwalk::is_ancestor(store, source, target)? {
        return Ok(Mergeability {
            base: Some(*source),
            merged: true,
            fast_forward: false,
            conflicts: Vec::new(),
        });
    }
    if revwalk::is_ancestor(store, target, source)? {
        return Ok(Mergeability {
            base: Some(*target),
            merged: false,
            fast_forward: true,
            conflicts: Vec::new(),
        });
    }
    let base = merge_base(store, source, target)?;
    let overlay = Overlay::new(store);
    let base_tree = match &base {
        Some(base) => Some(ops::read_commit(store, base)?.tree),
        None => None,
    };
    let ours = ops::read_commit(store, target)?.tree;
    let theirs = ops::read_commit(store, source)?.tree;
    let conflicts = match merge_trees(&overlay, base_tree.as_ref(), &ours, &theirs, ("target", "source"))? {
        TreeMerge::Clean(_) => Vec::new(),
        TreeMerge::Conflicts(paths) => paths,
    };
    Ok(Mergeability {
        base,
        merged: false,
        fast_forward: false,
        conflicts,
    })
}

/// A signature as it is in a commit, with its time zone.
fn signature_of(signature: &ops::Signature) -> gix::actor::Signature {
    gix::actor::Signature {
        name: signature.name.as_str().into(),
        email: signature.email.as_str().into(),
        time: gix::date::Time::new(signature.seconds, signature.offset),
    }
}

fn author_signature(author: &Author) -> gix::actor::Signature {
    gix::actor::Signature {
        name: author.name.as_str().into(),
        email: author.email.as_str().into(),
        time: match author.seconds {
            Some(seconds) => gix::date::Time::new(seconds, 0),
            None => gix::date::Time::now_local_or_utc(),
        },
    }
}

/// A message with the `Moqui-User-Id` trailer of `author`, as a commit made through Moqui has.
fn with_user_trailer(message: &str, author: &Author) -> String {
    let mut message = message.trim_end().to_string();
    message.push('\n');
    if let Some(user_id) = &author.user_id {
        if ops::user_of(&message).is_none() {
            if ops::trailers(&message).is_empty() {
                message.push('\n');
            }
            message.push_str(ops::USER_TRAILER);
            message.push_str(user_id);
            message.push('\n');
        }
    }
    message
}

/// Write a commit of `tree` with `parents`.
pub(crate) fn write_commit(
    store: &dyn GitStore,
    tree: ObjectId,
    parents: &[ObjectId],
    author: gix::actor::Signature,
    committer: gix::actor::Signature,
    message: &str,
) -> Result<ObjectId, Error> {
    let commit = gix::objs::Commit {
        tree,
        parents: parents.iter().copied().collect(),
        author,
        committer,
        encoding: None,
        message: message.into(),
        extra_headers: Vec::new(),
    };
    let mut buf = Vec::with_capacity(256);
    commit
        .write_to(&mut buf)
        .map_err(|err| Error::new(format!("encoding a git commit: {err}")))?;
    store.write(Kind::Commit, &buf)
}

/// What a merge asks for.
#[derive(Clone, Debug)]
pub(crate) struct MergeRequest<'a> {
    pub method: Method,
    pub source: ObjectId,
    pub target: ObjectId,
    /// The names in a default message and conflict labels: the source and target branches.
    pub source_name: &'a str,
    pub target_name: &'a str,
    /// The message of a merge commit or squash; a default when empty.
    pub message: Option<&'a str>,
    /// Who the merge is by (the committer of every commit it makes).
    pub committer: &'a Author,
    /// The author of a squash commit (the merge request's), else the committer.
    pub squash_author: Option<&'a Author>,
}

/// What a merge made: the target's new commit and the commits it gained that the merge made (oldest first; none for
/// a fast-forward).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Merged {
    pub head: ObjectId,
    pub made: Vec<ObjectId>,
}

/// Why a merge was not made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// The target already has the source.
    AlreadyMerged,
    /// A fast-forward was asked and the target has commits the source lacks.
    NotFastForward,
    /// These paths conflict (for a rebase, at the commit named).
    Conflicts { paths: Vec<String>, at: Option<ObjectId> },
}

impl Refused {
    pub(crate) fn message(&self, source: &str, target: &str) -> String {
        match self {
            Self::AlreadyMerged => format!("{target} already has every commit of {source}"),
            Self::NotFastForward => {
                format!("{target} cannot be fast-forwarded to {source}: it has commits {source} lacks; rebase first")
            }
            Self::Conflicts { paths, at } => {
                let at = at
                    .map(|id| format!(" at commit {}", &id.to_string()[..10]))
                    .unwrap_or_default();
                format!(
                    "{source} does not merge into {target}{at}: conflicts in {}",
                    paths.join(", ")
                )
            }
        }
    }
}

/// Make the commits of a merge in `overlay` (written to the store by the caller's [`Overlay::flush`] once it moves the
/// ref). Refused on a conflict, with nothing kept.
pub(crate) fn merge(overlay: &Overlay<'_>, request: &MergeRequest<'_>) -> Result<Result<Merged, Refused>, Error> {
    let store: &dyn GitStore = overlay;
    let (source, target) = (request.source, request.target);
    if revwalk::is_ancestor(store, &source, &target)? {
        return Ok(Err(Refused::AlreadyMerged));
    }
    let fast_forward = revwalk::is_ancestor(store, &target, &source)?;
    let committer = author_signature(request.committer);
    let merged_tree = |labels: (&str, &str)| -> Result<Result<ObjectId, Refused>, Error> {
        if fast_forward {
            return Ok(Ok(ops::read_commit(store, &source)?.tree));
        }
        let base = merge_base(store, &source, &target)?;
        let base_tree = match &base {
            Some(base) => Some(ops::read_commit(store, base)?.tree),
            None => None,
        };
        let ours = ops::read_commit(store, &target)?.tree;
        let theirs = ops::read_commit(store, &source)?.tree;
        Ok(match merge_trees(store, base_tree.as_ref(), &ours, &theirs, labels)? {
            TreeMerge::Clean(tree) => Ok(tree),
            TreeMerge::Conflicts(paths) => Err(Refused::Conflicts { paths, at: None }),
        })
    };
    let labels = (request.target_name, request.source_name);
    match request.method {
        Method::FastForward => {
            if !fast_forward {
                return Ok(Err(Refused::NotFastForward));
            }
            Ok(Ok(Merged {
                head: source,
                made: Vec::new(),
            }))
        }
        Method::Merge => {
            let tree = match merged_tree(labels)? {
                Ok(tree) => tree,
                Err(refused) => return Ok(Err(refused)),
            };
            let message = request
                .message
                .map(str::trim)
                .filter(|message| !message.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("Merge branch '{}' into '{}'", request.source_name, request.target_name));
            let message = with_user_trailer(&message, request.committer);
            let head = write_commit(store, tree, &[target, source], committer.clone(), committer, &message)?;
            Ok(Ok(Merged { head, made: vec![head] }))
        }
        Method::Squash => {
            let tree = match merged_tree(labels)? {
                Ok(tree) => tree,
                Err(refused) => return Ok(Err(refused)),
            };
            let author = request.squash_author.unwrap_or(request.committer);
            let message = match request.message.map(str::trim).filter(|message| !message.is_empty()) {
                Some(message) => message.to_string(),
                None => {
                    // the source's commit messages, oldest first, as git merge --squash lists them
                    let mut commits = new_commits(store, &[source], &[target])?;
                    commits.reverse();
                    let summaries: Vec<String> = commits
                        .iter()
                        .map(|commit| format!("* {}", commit.message.lines().next().unwrap_or("").trim()))
                        .collect();
                    format!(
                        "Squashed '{}' into '{}'\n\n{}",
                        request.source_name,
                        request.target_name,
                        summaries.join("\n")
                    )
                }
            };
            let message = with_user_trailer(&message, author);
            let head = write_commit(store, tree, &[target], author_signature(author), committer, &message)?;
            Ok(Ok(Merged { head, made: vec![head] }))
        }
        Method::Rebase => {
            if fast_forward {
                return Ok(Ok(Merged {
                    head: source,
                    made: Vec::new(),
                }));
            }
            let mut commits = new_commits(store, &[source], &[target])?;
            commits.reverse();
            let mut tip = target;
            let mut made = Vec::new();
            for commit in commits {
                // a merge commit of the source is not replayed: its changes come with the commits it merged
                if commit.parents.len() > 1 {
                    continue;
                }
                let details: CommitDetails = ops::commit_details(store, &commit.id)?;
                let base_tree = match commit.parents.first() {
                    Some(parent) => Some(ops::read_commit(store, parent)?.tree),
                    None => None,
                };
                let tip_tree = ops::read_commit(store, &tip)?.tree;
                let tree = match merge_trees(store, base_tree.as_ref(), &tip_tree, &commit.tree, labels)? {
                    TreeMerge::Clean(tree) => tree,
                    TreeMerge::Conflicts(paths) => {
                        return Ok(Err(Refused::Conflicts {
                            paths,
                            at: Some(commit.id),
                        }))
                    }
                };
                if tree == tip_tree {
                    // its changes are already on the target
                    continue;
                }
                tip = write_commit(
                    store,
                    tree,
                    &[tip],
                    signature_of(&details.author),
                    committer.clone(),
                    &details.message,
                )?;
                made.push(tip);
            }
            if made.is_empty() {
                return Ok(Err(Refused::AlreadyMerged));
            }
            Ok(Ok(Merged { head: tip, made }))
        }
    }
}

#[cfg(test)]
mod tests {
    use gix::objs::tree::EntryKind;

    use super::super::ops::{self, Edit};
    use super::super::store::MemoryStore;
    use super::*;

    fn author(name: &str) -> Author {
        Author {
            name: name.to_string(),
            email: format!("{name}@example.com"),
            user_id: Some(name.to_uppercase()),
            seconds: None,
        }
    }

    fn path(text: &str) -> Vec<String> {
        text.split('/').map(str::to_string).collect()
    }

    /// A commit of `files` on `parent`'s tree, made on branch `branch`.
    fn commit_on(store: &MemoryStore, branch: &str, files: &[(&str, &str)], message: &str) -> ObjectId {
        let edits: Vec<Edit> = files
            .iter()
            .map(|(at, text)| {
                Edit::Put(
                    path(at),
                    EntryKind::Blob,
                    store.write(Kind::Blob, text.as_bytes()).unwrap(),
                )
            })
            .collect();
        ops::commit(store, branch, &author("dev"), |_| {
            Ok(Some((edits.clone(), message.to_string())))
        })
        .unwrap()
        .unwrap()
    }

    fn branch_from(store: &MemoryStore, name: &str, at: ObjectId) {
        assert!(store.compare_and_set_ref(&ops::branch_ref(name), None, at).unwrap());
    }

    fn file(store: &dyn GitStore, commit: &ObjectId, at: &str) -> Option<String> {
        ops::read_at(store, commit, &path(at))
            .unwrap()
            .map(|bytes| String::from_utf8(bytes).unwrap())
    }

    const BASE: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\n";

    /// main with a base file; `feature` from it changes the first line, main the last.
    fn diverged(store: &MemoryStore) -> (ObjectId, ObjectId, ObjectId) {
        let base = commit_on(store, "main", &[("a.txt", BASE)], "base");
        branch_from(store, "feature", base);
        commit_on(
            store,
            "feature",
            &[("a.txt", &BASE.replace("one", "ONE"))],
            "feature: first line",
        );
        let feature = commit_on(store, "feature", &[("b.txt", "new file\n")], "feature: b");
        let main = commit_on(
            store,
            "main",
            &[("a.txt", &BASE.replace("seven", "SEVEN"))],
            "main: last line",
        );
        (base, feature, main)
    }

    fn request<'a>(method: Method, source: ObjectId, target: ObjectId, committer: &'a Author) -> MergeRequest<'a> {
        MergeRequest {
            method,
            source,
            target,
            source_name: "feature",
            target_name: "main",
            message: None,
            committer,
            squash_author: None,
        }
    }

    #[test]
    fn the_merge_base_is_the_newest_common_ancestor() {
        let store = MemoryStore::default();
        let (base, feature, main) = diverged(&store);
        assert_eq!(merge_base(&store, &feature, &main).unwrap(), Some(base));
        assert_eq!(merge_base(&store, &base, &main).unwrap(), Some(base));
        let checked = check(&store, &feature, &main).unwrap();
        assert!(
            !checked.merged && !checked.fast_forward && checked.conflicts.is_empty(),
            "{checked:?}"
        );
        assert!(check(&store, &base, &main).unwrap().merged);
        assert!(check(&store, &main, &base).unwrap().fast_forward);
    }

    #[test]
    fn a_merge_commit_has_both_parents_and_both_changes() {
        let store = MemoryStore::default();
        let (_, feature, main) = diverged(&store);
        let me = author("merger");
        let overlay = Overlay::new(&store);
        let merged = merge(&overlay, &request(Method::Merge, feature, main, &me))
            .unwrap()
            .unwrap();
        // nothing is in the store until the overlay is flushed
        assert!(store.find(&merged.head).unwrap().is_none());
        assert!(overlay.flush().unwrap() >= 3);
        let commit = ops::commit_details(&store, &merged.head).unwrap();
        assert_eq!(commit.parents, vec![main, feature]);
        assert!(
            commit.message.starts_with("Merge branch 'feature' into 'main'"),
            "{}",
            commit.message
        );
        assert_eq!(ops::user_of(&commit.message).as_deref(), Some("MERGER"));
        let text = file(&store, &merged.head, "a.txt").unwrap();
        assert!(text.starts_with("ONE\n") && text.ends_with("SEVEN\n"), "{text}");
        assert_eq!(file(&store, &merged.head, "b.txt").as_deref(), Some("new file\n"));
    }

    #[test]
    fn squash_and_rebase_results() {
        let store = MemoryStore::default();
        let (_, feature, main) = diverged(&store);
        let me = author("merger");
        let mr_author = author("writer");

        let overlay = Overlay::new(&store);
        let mut squash = request(Method::Squash, feature, main, &me);
        squash.squash_author = Some(&mr_author);
        squash.message = Some("Feature work (!1)");
        let squashed = merge(&overlay, &squash).unwrap().unwrap();
        overlay.flush().unwrap();
        let commit = ops::commit_details(&store, &squashed.head).unwrap();
        assert_eq!(commit.parents, vec![main], "a squash has the target as its only parent");
        assert_eq!(commit.author.name, "writer");
        assert_eq!(commit.committer.name, "merger");
        assert!(commit.message.starts_with("Feature work (!1)"));
        assert_eq!(squashed.made, vec![squashed.head]);
        let text = file(&store, &squashed.head, "a.txt").unwrap();
        assert!(text.starts_with("ONE\n") && text.ends_with("SEVEN\n"), "{text}");

        let overlay = Overlay::new(&store);
        let rebased = merge(&overlay, &request(Method::Rebase, feature, main, &me))
            .unwrap()
            .unwrap();
        overlay.flush().unwrap();
        assert_eq!(rebased.made.len(), 2, "each commit of the source replayed");
        let first = ops::commit_details(&store, &rebased.made[0]).unwrap();
        assert_eq!(first.parents, vec![main]);
        assert_eq!(first.message.trim(), "feature: first line\n\nMoqui-User-Id: DEV".trim());
        assert_eq!(first.author.name, "dev");
        assert_eq!(first.committer.name, "merger");
        let last = ops::commit_details(&store, &rebased.head).unwrap();
        assert_eq!(last.parents, vec![rebased.made[0]]);
        assert_eq!(file(&store, &rebased.head, "b.txt").as_deref(), Some("new file\n"));
        assert!(file(&store, &rebased.head, "a.txt").unwrap().ends_with("SEVEN\n"));

        // a fast-forward is refused on diverged branches and made when the target is behind
        let overlay = Overlay::new(&store);
        assert_eq!(
            merge(&overlay, &request(Method::FastForward, feature, main, &me)).unwrap(),
            Err(Refused::NotFastForward)
        );
        assert_eq!(
            merge(&overlay, &request(Method::FastForward, rebased.head, main, &me)).unwrap(),
            Ok(Merged {
                head: rebased.head,
                made: Vec::new()
            })
        );
        assert_eq!(
            merge(&overlay, &request(Method::Merge, main, rebased.head, &me)).unwrap(),
            Err(Refused::AlreadyMerged)
        );
    }

    #[test]
    fn a_conflict_refuses_the_merge_and_names_the_paths() {
        let store = MemoryStore::default();
        let base = commit_on(&store, "main", &[("a.txt", BASE), ("c.txt", "same\n")], "base");
        branch_from(&store, "feature", base);
        let feature = commit_on(
            &store,
            "feature",
            &[("a.txt", &BASE.replace("four", "FOUR from feature"))],
            "f",
        );
        let main = commit_on(
            &store,
            "main",
            &[("a.txt", &BASE.replace("four", "four from main"))],
            "m",
        );
        let checked = check(&store, &feature, &main).unwrap();
        assert_eq!(checked.conflicts, vec!["a.txt".to_string()]);
        let me = author("merger");
        for method in [Method::Merge, Method::Squash, Method::Rebase] {
            let overlay = Overlay::new(&store);
            match merge(&overlay, &request(method, feature, main, &me)).unwrap() {
                Err(Refused::Conflicts { paths, .. }) => assert_eq!(paths, vec!["a.txt".to_string()]),
                other => panic!("{method:?}: {other:?}"),
            }
        }
        assert_eq!(store.read_ref("refs/heads/main").unwrap(), Some(main));
    }
}
