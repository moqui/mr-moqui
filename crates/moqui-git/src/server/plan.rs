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

//! What a fetch sends, as git's upload-pack and pack-objects work it out: the shallow boundary of a shallow fetch
//! (`deepen`, `deepen-relative`, `deepen-since`, `deepen-not`), then the commits reachable from the wants and not from
//! what the client has, and the trees and blobs of those commits that are not under the trees of the commits at the
//! edge (the client's commits the sent ones have as parents). The client's shallow commits, and the new boundary, are
//! walked as commits with no parents, so a hidden walk does not reach past what the client really has.

use std::collections::{HashMap, HashSet, VecDeque};

use gix::objs::Kind;
use gix::ObjectId;

use crate::ops;
use crate::revwalk::{peel_to_commit, Walk};
use crate::store::GitStore;
use moqui_core::context::Error;

/// `deepen` of this many commits (git's `INFINITE_DEPTH`, what `--unshallow` sends) asks for all of the history.
pub(crate) const INFINITE_DEPTH: u32 = 0x7fff_ffff;

/// A shallow fetch's request.
#[derive(Clone, Debug, Default)]
pub(crate) struct Deepen {
    /// The client's shallow commits (`shallow` lines).
    pub client_shallow: Vec<ObjectId>,
    /// `deepen <depth>`.
    pub depth: Option<u32>,
    /// `deepen-relative`: the depth counts from the client's shallow commits.
    pub relative: bool,
    /// `deepen-since <seconds>`.
    pub since: Option<i64>,
    /// `deepen-not <ref>`, as the commits the refs name.
    pub not: Vec<ObjectId>,
}

impl Deepen {
    pub(crate) fn deepens(&self) -> bool {
        self.depth.is_some() || self.since.is_some() || !self.not.is_empty()
    }
}

/// The shallow lines of a response, and the grafts the walk uses.
#[derive(Clone, Debug, Default)]
pub(crate) struct ShallowInfo {
    /// New boundary commits: `shallow <id>`.
    pub shallow: Vec<ObjectId>,
    /// Client shallow commits whose parents are now sent: `unshallow <id>`.
    pub unshallow: Vec<ObjectId>,
}

/// The commits each of `heads` reaches within `depth` commits (a head is at depth 1): those at `depth` are the
/// boundary, the others are not shallow. By breadth, so each commit has its least depth.
fn by_depth(
    store: &dyn GitStore,
    heads: &[ObjectId],
    depth: u32,
) -> Result<(HashSet<ObjectId>, HashSet<ObjectId>), Error> {
    let (mut boundary, mut inside) = (HashSet::new(), HashSet::new());
    let mut seen = HashSet::new();
    let mut queue: VecDeque<(ObjectId, u32)> = VecDeque::new();
    for head in heads {
        if let Some(commit) = peel_to_commit(store, head)? {
            if seen.insert(commit) {
                queue.push_back((commit, 1));
            }
        }
    }
    while let Some((id, at)) = queue.pop_front() {
        if at >= depth {
            boundary.insert(id);
            continue;
        }
        inside.insert(id);
        for parent in ops::read_commit(store, &id)?.parents {
            if seen.insert(parent) && store.find(&parent)?.is_some() {
                queue.push_back((parent, at + 1));
            }
        }
    }
    boundary.retain(|id| !inside.contains(id));
    Ok((boundary, inside))
}

/// The commits from `wants` newer than `since` and not reachable from `not`: those with a parent outside are the
/// boundary.
fn by_rev_list(
    store: &dyn GitStore,
    wants: &[ObjectId],
    since: Option<i64>,
    not: &[ObjectId],
) -> Result<(HashSet<ObjectId>, HashSet<ObjectId>), Error> {
    let mut walk = Walk::new(store);
    for id in not {
        walk.push(*id, true)?;
    }
    for id in wants {
        walk.push(*id, false)?;
    }
    let included: Vec<ops::CommitInfo> = walk
        .limited()?
        .into_iter()
        .filter(|commit| since.is_none_or(|since| commit.seconds >= since))
        .collect();
    let inside: HashSet<ObjectId> = included.iter().map(|commit| commit.id).collect();
    let boundary = included
        .iter()
        .filter(|commit| commit.parents.iter().any(|parent| !inside.contains(parent)))
        .map(|commit| commit.id)
        .collect();
    Ok((boundary, inside))
}

/// The shallow lines for a fetch of `wants` with `deepen`, as git's `deepen()`, `send_shallow()` and
/// `send_unshallow()`. Wants of the parents of each unshallowed commit are added to `wants`.
pub(crate) fn shallow(store: &dyn GitStore, wants: &mut Vec<ObjectId>, deepen: &Deepen) -> Result<ShallowInfo, Error> {
    let client: Vec<ObjectId> = deepen
        .client_shallow
        .iter()
        .copied()
        .filter(|id| store.object_size(id).ok().flatten().is_some())
        .collect();
    let (boundary, inside) = if deepen.depth == Some(INFINITE_DEPTH) {
        // all of the history: every shallow commit of the client is unshallowed, and nothing is a boundary
        (HashSet::new(), client.iter().copied().collect())
    } else if let Some(depth) = deepen.depth {
        if deepen.relative {
            by_depth(store, &client, depth.saturating_add(1))?
        } else {
            by_depth(store, wants, depth)?
        }
    } else if deepen.deepens() {
        by_rev_list(store, wants, deepen.since, &deepen.not)?
    } else {
        (HashSet::new(), HashSet::new())
    };
    let client_set: HashSet<ObjectId> = client.iter().copied().collect();
    let mut info = ShallowInfo::default();
    let mut new_shallow: Vec<ObjectId> = boundary.into_iter().filter(|id| !client_set.contains(id)).collect();
    new_shallow.sort();
    info.shallow = new_shallow;
    for id in &client {
        if inside.contains(id) {
            info.unshallow.push(*id);
            for parent in ops::read_commit(store, id)?.parents {
                if store.object_size(&parent)?.is_some() && !wants.contains(&parent) {
                    wants.push(parent);
                }
            }
        }
    }
    Ok(info)
}

/// What to send.
#[derive(Clone, Debug, Default)]
pub(crate) struct Plan {
    /// Every object, commits first.
    pub objects: Vec<ObjectId>,
    pub commits: usize,
    /// The trees and blobs the client has (those under the trees of the commits at the edge), which a thin pack may
    /// send deltas against.
    pub excluded: HashSet<ObjectId>,
}

/// The objects reachable from `wants` (commits, annotated tags, or with `allow-reachable-sha1-in-want` a tree or
/// blob) that the client does not have: `haves` are its commits, `edges` more of them (unshallowed commits), and
/// `grafts` the commits walked as if they had no parents. With `tags`, the annotated tags (by their tag object ids)
/// whose commit is sent are sent too (`include-tag`).
pub(crate) fn plan(
    store: &dyn GitStore,
    wants: &[ObjectId],
    haves: &[ObjectId],
    edges: &[ObjectId],
    grafts: &HashSet<ObjectId>,
    tags: &[ObjectId],
) -> Result<Plan, Error> {
    let mut sent: HashSet<ObjectId> = HashSet::new();
    let mut objects: Vec<ObjectId> = Vec::new();
    let mut extra_roots: Vec<(ObjectId, Kind)> = Vec::new();
    let mut tag_objects: Vec<ObjectId> = Vec::new();
    let mut walk = Walk::new(store).with_grafts(grafts);
    if haves.is_empty() && edges.is_empty() {
        // a clone walks all of the history
        walk.preload()?;
    }
    for id in haves.iter().chain(edges) {
        walk.push(*id, true)?;
    }
    for want in wants {
        // a tag object goes with the commit it names; a tree or blob is sent with what is under it
        let mut current = *want;
        loop {
            match store.find(&current)? {
                Some((Kind::Tag, data)) => {
                    if !sent.contains(&current) {
                        tag_objects.push(current);
                    }
                    current = gix::objs::TagRef::from_bytes(&data, ops::HASH)
                        .map_err(|err| Error::new(format!("git tag {current}: {err}")))?
                        .target();
                }
                Some((Kind::Commit, _)) => {
                    walk.push(current, false)?;
                    break;
                }
                Some((kind, _)) => {
                    extra_roots.push((current, kind));
                    break;
                }
                None => return Err(Error::new(format!("git object {current} is missing"))),
            }
        }
    }
    let commits = walk.limited()?;
    let commit_ids: HashSet<ObjectId> = commits.iter().map(|commit| commit.id).collect();
    // the client's commits that sent commits have as parents: what is under their trees is not sent
    let mut edge_commits: Vec<ObjectId> = edges.to_vec();
    for commit in &commits {
        if grafts.contains(&commit.id) {
            continue;
        }
        for parent in &commit.parents {
            if !commit_ids.contains(parent) && !edge_commits.contains(parent) {
                edge_commits.push(*parent);
            }
        }
    }
    let present = store.contains_many(&edge_commits)?;
    let mut edge_trees = Vec::new();
    for edge in edge_commits.iter().filter(|edge| present.contains(*edge)) {
        edge_trees.push(ops::read_commit(store, edge)?.tree);
    }
    let mut excluded: HashSet<ObjectId> = HashSet::new();
    walk_trees(store, &edge_trees, &mut excluded, &mut |_| {})?;
    for commit in &commits {
        sent.insert(commit.id);
        objects.push(commit.id);
    }
    let commit_count = objects.len();
    for id in tag_objects {
        if sent.insert(id) {
            objects.push(id);
        }
    }
    for tag in tags {
        let mut chain = Vec::new();
        let mut current = *tag;
        while let Some((Kind::Tag, data)) = store.find(&current)? {
            chain.push(current);
            current = gix::objs::TagRef::from_bytes(&data, ops::HASH)
                .map_err(|err| Error::new(format!("git tag {current}: {err}")))?
                .target();
        }
        if !chain.is_empty() && commit_ids.contains(&current) {
            for id in chain {
                if sent.insert(id) {
                    objects.push(id);
                }
            }
        }
    }
    let mut seen = excluded.clone();
    let mut roots: Vec<ObjectId> = commits.iter().map(|commit| commit.tree).collect();
    for (id, kind) in &extra_roots {
        if *kind == Kind::Tree {
            roots.push(*id);
        }
    }
    walk_trees(store, &roots, &mut seen, &mut |id| {
        if sent.insert(id) {
            objects.push(id);
        }
    })?;
    for (id, kind) in extra_roots {
        if kind != Kind::Tree && seen.insert(id) && sent.insert(id) {
            objects.push(id);
        }
    }
    Ok(Plan {
        objects,
        commits: commit_count,
        excluded,
    })
}

/// Trees read per batch of a walk.
const TREE_BATCH: usize = 256;

/// Every tree and blob under `roots` (themselves included) not in `seen`, which each is added to: a level of the
/// trees at a time, read in batches. A submodule entry names another repository's commit and is passed over.
pub(crate) fn walk_trees(
    store: &dyn GitStore,
    roots: &[ObjectId],
    seen: &mut HashSet<ObjectId>,
    each: &mut dyn FnMut(ObjectId),
) -> Result<(), Error> {
    let mut level: Vec<ObjectId> = Vec::new();
    for root in roots {
        if seen.insert(*root) {
            each(*root);
            level.push(*root);
        }
    }
    while !level.is_empty() {
        let mut next = Vec::new();
        for batch in level.chunks(TREE_BATCH) {
            let mut trees: HashMap<ObjectId, Vec<u8>> = HashMap::with_capacity(batch.len());
            store.find_many(batch, &mut |id, kind, data| {
                if kind != Kind::Tree {
                    return Err(Error::new(format!("git object {id} is a {kind}, not a tree")));
                }
                trees.insert(id, data.to_vec());
                Ok(())
            })?;
            for id in batch {
                let data = trees
                    .get(id)
                    .ok_or_else(|| Error::new(format!("git tree {id} is missing")))?;
                let tree = gix::objs::TreeRef::from_bytes(data, ops::HASH)
                    .map_err(|err| Error::new(format!("git tree {id}: {err}")))?;
                for entry in tree.entries {
                    if entry.mode.is_commit() {
                        continue;
                    }
                    let oid = entry.oid.to_owned();
                    if seen.insert(oid) {
                        each(oid);
                        if entry.mode.is_tree() {
                            next.push(oid);
                        }
                    }
                }
            }
        }
        level = next;
    }
    Ok(())
}
