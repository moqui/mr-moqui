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

//! `gc#GitRepositories` for `db` repositories, as git's `gc` and `repack` work: delete the objects no ref reaches,
//! and store the others as deltas of each other. A run holds the repository's lease (`gcLeaseOwner`), so it is the
//! only one that rewrites its rows, and goes:
//!
//! 1. Scan every row without its data.
//! 2. Mark what is reachable from the refs (a missing object there stops the run: a damaged repository is never
//!    swept), then from what may still be needed: the tips of a change whose events have not run, the old and new ids
//!    of the changes of the last `git-server` `gc-reflog-days` (a force push can be undone), and every row stamped in the last
//!    `git-server` `gc-prune-days` (a push or write in progress, whose refs have not moved yet). The bases of every kept delta
//!    are kept.
//! 3. Look again at the rows stamped since the scan began (a push that reused an old object freshens it), then
//!    delete the rest, deepest delta first, each `DELETE` only of rows still stamped before the cutoff.
//! 4. Repack, while time is left: the trees and blobs of each path, newest first, the newest whole and each older
//!    one a delta of a newer one when that saves a quarter; rows written before the zlib format compressed; chains too
//!    deep made shorter. Each delta is applied and checked against the object's id before its row is changed.
//! 5. Record the counts on the `GitRepository`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use gix::objs::Kind;
use gix::ObjectId;
use moqui_lib::object::{basic_convert, Value};

use super::db::{DbStore, Rewrite, ScanRow};
use super::store::{deflate, verify, GitStore, Storage};
use super::{events, ops, text, GitProvider, RepositoryKind, GIT_REPOSITORY};
use crate::context::Error;
use crate::entity::{Condition, EntityFacade};

const DAY_MILLIS: i64 = 24 * 60 * 60 * 1000;

/// How a run goes.
#[derive(Clone, Debug)]
pub struct GcOptions {
    /// Count what would be deleted, and change nothing.
    pub dry_run: bool,
    /// Unreachable rows stamped within this long are kept (`git-server` `gc-prune-days`, 14, at least 1).
    pub prune_millis: i64,
    /// The changes of this long ago are kept, so a force push can be undone (`git-server` `gc-reflog-days`, 30).
    pub reflog_millis: i64,
    /// When the run stops repacking.
    pub deadline: Instant,
    /// A repository with more rows is passed over (`git-server` `gc-max-objects`, 20,000,000): the scan is kept in memory.
    pub max_objects: usize,
    pub max_depth: u32,
    pub max_delta_size: u64,
}

impl GcOptions {
    /// The options from the conf's `git-server` element, with `timeout` for the run.
    pub fn from_conf(dry_run: bool, timeout: Duration) -> Self {
        let number = |name: &str, default: i64| super::number_setting(name, default);
        Self {
            dry_run,
            prune_millis: number("gc-prune-days", 14).max(1) * DAY_MILLIS,
            reflog_millis: number("gc-reflog-days", 30).max(0) * DAY_MILLIS,
            deadline: Instant::now() + timeout,
            max_objects: usize::try_from(number("gc-max-objects", 20_000_000)).unwrap_or(usize::MAX),
            max_depth: u32::try_from(number("db-delta-depth", 50)).unwrap_or(50).max(1),
            max_delta_size: u64::try_from(number("db-delta-max-size", 16 * 1024 * 1024)).unwrap_or(u64::MAX),
        }
    }
}

/// What a run did.
#[derive(Clone, Debug, Default)]
pub struct GcReport {
    /// Rows when the run began.
    pub scanned: usize,
    /// Rows kept: reachable, recent, or the base of a kept delta.
    pub kept: usize,
    /// Rows deleted (with a dry run: that would be).
    pub deleted: u64,
    pub deleted_bytes: u64,
    /// Rows rewritten, and the bytes that saved.
    pub repacked: usize,
    pub saved_bytes: i64,
    /// LFS objects the repository no longer refers to, whose association was removed (their content goes with
    /// `gc#ContentBlobs` when nothing else has it).
    pub lfs_deleted: u64,
    /// Rows and bytes stored after the run.
    pub objects: usize,
    pub stored_bytes: u64,
    /// Why the run stopped before it was through (its time ran out).
    pub stopped: Option<String>,
}

impl GcReport {
    pub fn summary(&self) -> String {
        let mut text = format!(
            "{} objects, {} bytes; deleted {} ({} bytes), repacked {} (saved {} bytes)",
            self.objects, self.stored_bytes, self.deleted, self.deleted_bytes, self.repacked, self.saved_bytes
        );
        if self.lfs_deleted > 0 {
            text.push_str(&format!("; {} LFS objects let go", self.lfs_deleted));
        }
        if let Some(stopped) = &self.stopped {
            text.push_str("; ");
            text.push_str(stopped);
        }
        text
    }
}

/// Each repository's outcome, by id.
pub type GcOutcomes = Vec<(String, Result<GcReport, Error>)>;

/// One row of the scan.
struct Obj {
    id: ObjectId,
    kind: Kind,
    size: u64,
    storage: Storage,
    base: Option<ObjectId>,
    stored: u64,
    stamp: i64,
    depth: u32,
}

/// Where a reachable tree or blob was first found: for grouping the versions of a path, newest first.
#[derive(Clone, Copy)]
struct Mark {
    /// The newest reachable commit it is in, by date (0 is the newest).
    rank: u32,
    /// A hash of its path.
    path: u64,
}

fn path_hash(path: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    hasher.finish()
}

/// The scan, with what is marked.
struct Table {
    objs: Vec<Obj>,
    index: HashMap<ObjectId, usize>,
    marks: Vec<Option<Mark>>,
    /// Commits by id: tree, parents, committer seconds.
    commits: HashMap<ObjectId, (ObjectId, Vec<ObjectId>, i64)>,
    /// The rank the next marked commit gets: a later pass ranks its commits after an earlier one's.
    next_rank: u32,
}

impl Table {
    fn get(&self, id: &ObjectId) -> Option<&Obj> {
        self.index.get(id).map(|at| &self.objs[*at])
    }

    fn mark(&mut self, id: &ObjectId, mark: Mark) -> bool {
        let Some(at) = self.index.get(id).copied() else {
            return false;
        };
        match &mut self.marks[at] {
            Some(old) => {
                old.rank = old.rank.min(mark.rank);
                false
            }
            slot => {
                *slot = Some(mark);
                true
            }
        }
    }

    fn is_marked(&self, id: &ObjectId) -> bool {
        self.index.get(id).is_some_and(|at| self.marks[*at].is_some())
    }

    /// How many deltas from each row to a whole one, by the bases the scan read: `None` for a row in a cycle or on a
    /// missing base.
    fn depths(&self) -> Vec<Option<u32>> {
        let mut depths: Vec<Option<u32>> = vec![None; self.objs.len()];
        let mut dependents: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut queue = VecDeque::new();
        for (at, obj) in self.objs.iter().enumerate() {
            match obj.base.and_then(|base| self.index.get(&base)) {
                Some(base) if obj.storage == Storage::Delta => dependents.entry(*base).or_default().push(at),
                _ if obj.storage == Storage::Delta => {}
                _ => {
                    depths[at] = Some(0);
                    queue.push_back(at);
                }
            }
        }
        while let Some(at) = queue.pop_front() {
            let depth = depths[at].unwrap_or(0);
            for dependent in dependents.get(&at).into_iter().flatten() {
                if depths[*dependent].is_none() {
                    depths[*dependent] = Some(depth + 1);
                    queue.push_back(*dependent);
                }
            }
        }
        depths
    }
}

/// The ids the run starts marking from.
struct Roots {
    /// What the refs point to: each must be there.
    refs: Vec<ObjectId>,
    /// What may still be needed: changes in progress and of the reflog window.
    others: Vec<ObjectId>,
}

impl GitProvider {
    /// `gc#GitRepositories`: collect `repository_id`, or every `db` repository. Each repository's outcome, by id: one
    /// that fails is recorded on its row and the others still run.
    pub fn gc(&self, repository_id: Option<&str>, options: &GcOptions) -> Result<GcOutcomes, Error> {
        let _rows = super::rows_guard();
        let entity = self.entity()?;
        let ids: Vec<String> = match repository_id {
            Some(id) => vec![id.to_string()],
            None => entity
                .find(GIT_REPOSITORY)?
                .eq("repositoryKind", Value::string("db"))
                .order_by("repositoryId", false)
                .list()?
                .iter()
                .filter_map(|row| text(row.get("repositoryId")))
                .collect(),
        };
        let mut outcomes = Vec::with_capacity(ids.len());
        for id in ids {
            let outcome = self.gc_one(&entity, &id, options);
            match &outcome {
                Ok(report) => tracing::info!(repository = %id, "git gc: {}", report.summary()),
                Err(err) => tracing::error!(repository = %id, "git gc failed: {}", err.message()),
            }
            if !options.dry_run {
                let (result, report) = match &outcome {
                    Ok(report) => (report.summary(), Some(report.clone())),
                    Err(err) => (format!("failed: {}", err.message()), None),
                };
                self.write_row(&id, |row| {
                    row.set(
                        "lastGcDate",
                        basic_convert(&Value::I64(now_millis()), "java.sql.Timestamp").value,
                    );
                    row.set("lastGcResult", Value::string(result));
                    if let Some(report) = report {
                        row.set(
                            "objectCount",
                            Value::I64(i64::try_from(report.objects).unwrap_or(i64::MAX)),
                        );
                        row.set(
                            "storedBytes",
                            Value::I64(i64::try_from(report.stored_bytes).unwrap_or(i64::MAX)),
                        );
                    }
                });
            }
            outcomes.push((id, outcome));
        }
        Ok(outcomes)
    }

    fn gc_one(&self, entity: &EntityFacade, repository_id: &str, options: &GcOptions) -> Result<GcReport, Error> {
        let definition = self.definition(repository_id)?;
        if definition.kind != RepositoryKind::Db {
            return Err(Error::new(format!(
                "git repository {repository_id} is not a db repository"
            )));
        }
        let store = DbStore::new(
            entity.clone(),
            repository_id,
            self.objects(),
        );
        if options.dry_run {
            return collect(&store, &self.gc_roots(entity, &store, options)?, options);
        }
        let owner = format!("{}-{}", std::process::id(), now_millis());
        let lease = options.deadline.saturating_duration_since(Instant::now()) + Duration::from_secs(600);
        if !lease_repository(
            entity,
            repository_id,
            Some(&owner),
            now_millis() + i64::try_from(lease.as_millis()).unwrap_or(i64::MAX),
        )? {
            return Err(Error::new(format!(
                "git repository {repository_id} is being collected by another run"
            )));
        }
        let outcome = self
            .gc_roots(entity, &store, options)
            .and_then(|roots| collect(&store, &roots, options));
        if let Err(err) = lease_repository(entity, repository_id, None, 0) {
            tracing::warn!(repository = %repository_id, "the gc lease was not released: {}", err.message());
        }
        outcome
    }

    /// The refs, and the ids of the changes in progress or in the reflog window.
    fn gc_roots(&self, entity: &EntityFacade, store: &DbStore, options: &GcOptions) -> Result<Roots, Error> {
        let refs: Vec<ObjectId> = store.list_refs("refs/")?.into_iter().map(|(_, id)| id).collect();
        let mut others = Vec::new();
        let parse = |hex: &str| ObjectId::from_hex(hex.trim().as_bytes()).ok();
        if let Some(head) = entity
            .find(GIT_REPOSITORY)?
            .eq("repositoryId", Value::string(store.repository_id()))
            .one()?
            .and_then(|row| text(row.get("headCommitId")))
        {
            others.extend(parse(&head));
        }
        let since = basic_convert(&Value::I64(now_millis() - options.reflog_millis), "java.sql.Timestamp").value;
        let updates = entity
            .find(events::GIT_UPDATE)?
            .eq("repositoryId", Value::string(store.repository_id()))
            .condition(Condition::or(vec![
                Condition::is_in(
                    "ecaStatus",
                    ["recording", "pending", "running"]
                        .into_iter()
                        .map(Value::string)
                        .collect(),
                ),
                Condition::greater_equal("updateDate", since),
            ]))
            .list()?;
        let mut update_ids = Vec::new();
        for update in updates.iter() {
            if let Some(tips) = text(update.get("priorTips")) {
                others.extend(tips.split_whitespace().filter_map(parse));
            }
            if let Some(id) = text(update.get("updateId")) {
                update_ids.push(Value::string(id));
            }
        }
        for chunk in update_ids.chunks(500) {
            for row in entity
                .find(events::GIT_UPDATE_REF)?
                .condition(Condition::is_in("updateId", chunk.to_vec()))
                .list()?
                .iter()
            {
                for field in ["oldObjectId", "newObjectId"] {
                    others.extend(text(row.get(field)).as_deref().and_then(parse));
                }
            }
        }
        Ok(Roots { refs, others })
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

/// Take the repository's lease for `owner` until `until` (when no other run holds one that has not expired), or with
/// `None` give it back. Whether it was taken.
fn lease_repository(
    entity: &EntityFacade,
    repository_id: &str,
    owner: Option<&str>,
    until: i64,
) -> Result<bool, Error> {
    let stamp = |millis: i64| basic_convert(&Value::I64(millis), "java.sql.Timestamp").value;
    let condition = match owner {
        Some(owner) => Condition::and(vec![
            Condition::equals("repositoryId", Value::string(repository_id)),
            Condition::or(vec![
                Condition::is_null("gcLeaseOwner"),
                Condition::less("gcLeaseUntil", stamp(now_millis())),
                Condition::equals("gcLeaseOwner", Value::string(owner)),
            ]),
        ]),
        None => Condition::equals("repositoryId", Value::string(repository_id)),
    };
    let statement = entity.with_entity(GIT_REPOSITORY, |definition, dialect, _| {
        moqui_db::sql::update(definition, dialect, &["gcLeaseOwner", "gcLeaseUntil"], Some(&condition))
    })?;
    let mut params = match owner {
        Some(owner) => vec![Value::string(owner), stamp(until)],
        None => vec![Value::Null, Value::Null],
    };
    params.extend(statement.params);
    let tx = entity.transactions_for(GIT_REPOSITORY)?;
    let scope = tx.scope(None)?;
    let updated = crate::entity::exec::execute_tx(&tx, &statement.sql, &params)?;
    scope.commit()?;
    Ok(updated > 0)
}

/// One run over `store`.
fn collect(store: &DbStore, roots: &Roots, options: &GcOptions) -> Result<GcReport, Error> {
    let began = now_millis();
    let cutoff = began - options.prune_millis;
    let mut report = GcReport::default();
    let mut table = Table {
        objs: Vec::new(),
        index: HashMap::new(),
        marks: Vec::new(),
        commits: HashMap::new(),
        next_rank: 0,
    };
    store.scan(&mut |row: ScanRow| {
        if table.objs.len() >= options.max_objects {
            return Err(Error::new(format!(
                "the repository has more than git-server gc-max-objects ({}) objects",
                options.max_objects
            )));
        }
        table.index.insert(row.id, table.objs.len());
        table.objs.push(Obj {
            id: row.id,
            kind: row.meta.kind,
            size: row.meta.size,
            storage: row.meta.storage,
            base: row.meta.base,
            stored: row.meta.stored_bytes,
            stamp: row.stamp,
            depth: row.meta.depth,
        });
        Ok(())
    })?;
    table.marks = vec![None; table.objs.len()];
    report.scanned = table.objs.len();
    store.scan_commits(&mut |id, data| {
        let commit = ops::parse_commit(&id, data)?;
        table.commits.insert(id, (commit.tree, commit.parents, commit.seconds));
        Ok(true)
    })?;

    // what is reachable: strictly from the refs, then from the rest
    mark_from(store, &mut table, &roots.refs, true)?;
    let mut others = roots.others.clone();
    others.extend(
        table
            .objs
            .iter()
            .filter(|obj| obj.stamp >= cutoff && obj.kind != Kind::Blob)
            .map(|obj| obj.id),
    );
    mark_from(store, &mut table, &others, false)?;

    // what is kept: reachable, recent, and the bases of what is kept
    let mut kept = vec![false; table.objs.len()];
    for (at, obj) in table.objs.iter().enumerate() {
        kept[at] = table.marks[at].is_some() || obj.stamp >= cutoff;
    }
    keep_bases(&table, &mut kept);

    // what was stamped since the scan began is kept too, with what it reaches
    let touched: Vec<ObjectId> = store
        .touched_since(began - 1000)?
        .into_iter()
        .map(|row| row.id)
        .collect();
    if !touched.is_empty() {
        mark_from(store, &mut table, &touched, false)?;
        for id in &touched {
            if let Some(at) = table.index.get(id) {
                kept[*at] = true;
            }
        }
        for (at, mark) in table.marks.iter().enumerate() {
            if mark.is_some() {
                kept[at] = true;
            }
        }
        keep_bases(&table, &mut kept);
    }
    report.kept = kept.iter().filter(|kept| **kept).count();

    // delete the rest, the deepest deltas first (a base after what is made from it)
    let depths = table.depths();
    let mut garbage: Vec<usize> = (0..table.objs.len()).filter(|at| !kept[*at]).collect();
    garbage.sort_by_key(|at| std::cmp::Reverse(depths[*at].unwrap_or(u32::MAX)));
    sweep_lfs(store, &table, &kept, cutoff, options.dry_run, &mut report)?;
    if options.dry_run {
        report.deleted = garbage.len() as u64;
        report.deleted_bytes = garbage.iter().map(|at| table.objs[*at].stored).sum();
        report.objects = report.scanned - garbage.len();
        report.stored_bytes = table.objs.iter().map(|obj| obj.stored).sum::<u64>() - report.deleted_bytes;
        return Ok(report);
    }
    let mut deleted: HashSet<usize> = HashSet::new();
    for batch in garbage.chunks(500) {
        let ids: Vec<ObjectId> = batch.iter().map(|at| table.objs[*at].id).collect();
        match store.delete_unreferenced(&ids, cutoff) {
            Ok(count) => {
                report.deleted += count;
                if count == ids.len() as u64 {
                    deleted.extend(batch.iter().copied());
                } else {
                    // some were freshened meanwhile: which ones is read back
                    let still: HashSet<ObjectId> = store.contains_many(&ids)?;
                    deleted.extend(batch.iter().filter(|at| !still.contains(&table.objs[**at].id)).copied());
                }
            }
            Err(err) => tracing::error!(
                repository = %store.repository_id(),
                "git gc could not delete {} unreachable objects: {}",
                ids.len(),
                err.message()
            ),
        }
    }
    report.deleted_bytes = deleted.iter().map(|at| table.objs[*at].stored).sum();

    let (repacked, saved) = repack(store, &mut table, &kept, &deleted, options, &mut report)?;
    report.repacked = repacked;
    report.saved_bytes = saved;
    fix_depths(store, &mut table, &deleted, options, &mut report)?;
    report.objects = table.objs.len() - deleted.len();
    report.stored_bytes = table
        .objs
        .iter()
        .enumerate()
        .filter(|(at, _)| !deleted.contains(at))
        .map(|(_, obj)| obj.stored)
        .sum();
    Ok(report)
}

/// Let go of the LFS objects of the repository uploaded before the cutoff that no kept pointer file names. Only a
/// repository with such objects reads its small blobs for pointers.
fn sweep_lfs(
    store: &DbStore,
    table: &Table,
    kept: &[bool],
    cutoff: i64,
    dry_run: bool,
    report: &mut GcReport,
) -> Result<(), Error> {
    let entity = store.entity();
    if !entity.table_ready(super::server::lfs::GIT_LFS_OBJECT) {
        return Ok(());
    }
    let old = entity
        .find(super::server::lfs::GIT_LFS_OBJECT)?
        .eq("repositoryId", Value::string(store.repository_id()))
        .condition(Condition::less(
            "uploadedDate",
            basic_convert(&Value::I64(cutoff), "java.sql.Timestamp").value,
        ))
        .list()?;
    if old.is_empty() {
        return Ok(());
    }
    let small: Vec<ObjectId> = table
        .objs
        .iter()
        .enumerate()
        .filter(|(at, obj)| kept[*at] && obj.kind == Kind::Blob && obj.size <= super::server::lfs::MAX_POINTER)
        .map(|(_, obj)| obj.id)
        .collect();
    let mut named: HashSet<String> = HashSet::new();
    store.find_many(&small, &mut |_, _, data| {
        if let Some((oid, _)) = super::server::lfs::pointer(data) {
            named.insert(oid);
        }
        Ok(())
    })?;
    for row in old.iter() {
        let oid = text(row.get("oid")).unwrap_or_default();
        if named.contains(&oid) {
            continue;
        }
        if !dry_run {
            if let Err(err) = row.delete(entity) {
                tracing::error!(repository = %store.repository_id(), "LFS object {oid} was not let go: {}", err.message());
                continue;
            }
        }
        report.lfs_deleted += 1;
    }
    Ok(())
}

/// Keep the delta bases of what is kept, down each chain.
fn keep_bases(table: &Table, kept: &mut [bool]) {
    let mut pending: Vec<usize> = (0..kept.len()).filter(|at| kept[*at]).collect();
    while let Some(at) = pending.pop() {
        if let Some(base) = table.objs[at].base.and_then(|base| table.index.get(&base)) {
            if !kept[*base] {
                kept[*base] = true;
                pending.push(*base);
            }
        }
    }
}

/// Mark what `roots` reach: commits through their parents, then the trees of the commits (newest first) and what is
/// in them, a level of trees at a time. With `strict`, an object that is not there is an error, except a parent (a
/// shallow push leaves its parents out) and a submodule's commit.
fn mark_from(store: &DbStore, table: &mut Table, roots: &[ObjectId], strict: bool) -> Result<(), Error> {
    let missing = |id: &ObjectId, from: &str| {
        Error::new(format!(
            "git repository {} is missing object {id}, which {from} refers to; it is not collected (run fsck#GitRepository)",
            store.repository_id()
        ))
    };
    let mut commits: Vec<ObjectId> = Vec::new();
    let mut trees: Vec<(ObjectId, Mark, String)> = Vec::new();
    let unranked = Mark {
        rank: u32::MAX,
        path: 0,
    };
    for root in roots {
        let mut current = *root;
        loop {
            let Some(obj) = table.get(&current) else {
                if strict {
                    return Err(missing(&current, "a ref"));
                }
                break;
            };
            match obj.kind {
                Kind::Commit => {
                    commits.push(current);
                    break;
                }
                Kind::Tag => {
                    table.mark(&current, unranked);
                    let (_, data) = store.find(&current)?.ok_or_else(|| missing(&current, "a ref"))?;
                    current = gix::objs::TagRef::from_bytes(&data, ops::HASH)
                        .map_err(|err| Error::new(format!("git tag {current}: {err}")))?
                        .target();
                }
                Kind::Tree => {
                    trees.push((current, unranked, String::new()));
                    break;
                }
                Kind::Blob => {
                    table.mark(&current, unranked);
                    break;
                }
            }
        }
    }
    // the commits, by their parents; one this walk has marked before is not walked again
    let mut reached: Vec<(i64, ObjectId)> = Vec::new();
    while let Some(id) = commits.pop() {
        if table.is_marked(&id) {
            continue;
        }
        if !table.mark(&id, unranked) {
            continue;
        }
        let commit = match table.commits.get(&id) {
            Some(commit) => commit.clone(),
            None => match store.find(&id)? {
                Some((Kind::Commit, data)) => {
                    let commit = ops::parse_commit(&id, &data)?;
                    (commit.tree, commit.parents, commit.seconds)
                }
                _ => return Err(missing(&id, "a commit")),
            },
        };
        reached.push((commit.2, id));
        for parent in &commit.1 {
            if table.get(parent).is_some() {
                commits.push(*parent);
            }
        }
    }
    reached.sort_by_key(|commit| std::cmp::Reverse(commit.0));
    let first_rank = table.next_rank;
    table.next_rank = first_rank.saturating_add(u32::try_from(reached.len()).unwrap_or(u32::MAX));
    for (rank, (_, id)) in reached.iter().enumerate() {
        let rank = first_rank.saturating_add(u32::try_from(rank).unwrap_or(u32::MAX));
        let tree = table.commits.get(id).map(|commit| commit.0);
        let tree = match tree {
            Some(tree) => tree,
            None => ops::read_commit(store, id)?.tree,
        };
        trees.push((
            tree,
            Mark {
                rank,
                path: path_hash(""),
            },
            String::new(),
        ));
    }
    // the trees, a level at a time
    let mut level: Vec<(ObjectId, Mark, String)> = Vec::new();
    for (id, mark, path) in trees {
        if table.get(&id).is_none() {
            if strict {
                return Err(missing(&id, "a commit"));
            }
            continue;
        }
        if table.mark(&id, mark) {
            level.push((id, mark, path));
        }
    }
    while !level.is_empty() {
        let mut next = Vec::new();
        for batch in level.chunks(256) {
            let ids: Vec<ObjectId> = batch.iter().map(|(id, _, _)| *id).collect();
            let mut data: HashMap<ObjectId, Vec<u8>> = HashMap::with_capacity(ids.len());
            store.find_many(&ids, &mut |id, _, content| {
                data.insert(id, content.to_vec());
                Ok(())
            })?;
            for (id, mark, path) in batch {
                let Some(content) = data.get(id) else {
                    if strict {
                        return Err(missing(id, "a tree"));
                    }
                    continue;
                };
                let tree = gix::objs::TreeRef::from_bytes(content, ops::HASH)
                    .map_err(|err| Error::new(format!("git tree {id}: {err}")))?;
                for entry in tree.entries {
                    if entry.mode.is_commit() {
                        continue;
                    }
                    let oid = entry.oid.to_owned();
                    let child_path = format!("{path}/{}", entry.filename);
                    let child = Mark {
                        rank: mark.rank,
                        path: path_hash(&child_path),
                    };
                    if table.get(&oid).is_none() {
                        if strict {
                            return Err(missing(&oid, &format!("tree {id}")));
                        }
                        continue;
                    }
                    if table.mark(&oid, child) && entry.mode.is_tree() {
                        next.push((oid, child, child_path));
                    }
                }
            }
        }
        level = next;
    }
    Ok(())
}

/// The trees and blobs of each path, newest first: the newest whole, each older one a delta of one of the two newer
/// ones before it when that saves a quarter of its bytes (and does not make a chain too deep or a cycle). Rows that
/// are not compressed, and deltas against what is not reachable, are rewritten too. (rows rewritten, bytes saved)
fn repack(
    store: &DbStore,
    table: &mut Table,
    kept: &[bool],
    deleted: &HashSet<usize>,
    options: &GcOptions,
    report: &mut GcReport,
) -> Result<(usize, i64), Error> {
    let mut groups: HashMap<(u8, u64), Vec<usize>> = HashMap::new();
    for (at, mark) in table.marks.iter().enumerate() {
        let obj = &table.objs[at];
        if let (Some(mark), Kind::Tree | Kind::Blob) = (mark, obj.kind) {
            if kept[at] && !deleted.contains(&at) && mark.rank != u32::MAX {
                let kind = u8::from(obj.kind == Kind::Tree);
                groups.entry((kind, mark.path)).or_default().push(at);
            }
        }
    }
    let mut groups: Vec<Vec<usize>> = groups.into_values().collect();
    for group in &mut groups {
        group.sort_by_key(|at| {
            let rank = table.marks[*at].map_or(u32::MAX, |mark| mark.rank);
            (rank, std::cmp::Reverse(table.objs[*at].size))
        });
    }
    groups.sort_by_key(|group| std::cmp::Reverse(group.iter().map(|at| table.objs[*at].stored).sum::<u64>()));
    let heights = heights(table);
    let mut depths: Vec<u32> = table
        .depths()
        .into_iter()
        .map(|depth| depth.unwrap_or(u32::MAX))
        .collect();
    let (mut repacked, mut saved) = (0usize, 0i64);
    'groups: for group in groups {
        for (position, at) in group.iter().enumerate() {
            if Instant::now() >= options.deadline {
                report.stopped = Some("the time ran out while repacking".to_string());
                break 'groups;
            }
            let (id, storage, size, stored, base) = {
                let obj = &table.objs[*at];
                (obj.id, obj.storage, obj.size, obj.stored, obj.base)
            };
            if storage == Storage::Blob {
                // a content blob stays one
                continue;
            }
            let base_kept = base.is_some_and(|base| table.is_marked(&base));
            let fine_delta = storage == Storage::Delta && base_kept && depths[*at] <= options.max_depth;
            if position == 0 || size > options.max_delta_size || fine_delta || size < 64 {
                // the newest stays (or becomes) whole; a fine delta stays; an uncompressed row is compressed
                if storage == Storage::Raw
                    || (position == 0 && storage == Storage::Delta && size <= options.max_delta_size)
                {
                    let saved_now = rewrite_whole(store, table, *at)?;
                    depths[*at] = 0;
                    repacked += 1;
                    saved += saved_now;
                }
                continue;
            }
            let candidates: Vec<usize> = group[position.saturating_sub(2)..position]
                .iter()
                .copied()
                .filter(|candidate| table.objs[*candidate].storage != Storage::Blob)
                .collect();
            let mut ids = vec![id];
            ids.extend(candidates.iter().map(|candidate| table.objs[*candidate].id));
            let mut contents: HashMap<ObjectId, Vec<u8>> = HashMap::new();
            store.find_many(&ids, &mut |found, _, data| {
                contents.insert(found, data.to_vec());
                Ok(())
            })?;
            let Some(target) = contents.get(&id) else {
                continue;
            };
            let mut best: Option<(usize, Vec<u8>, Vec<u8>)> = None;
            for candidate in &candidates {
                let candidate_id = table.objs[*candidate].id;
                let Some(source) = contents.get(&candidate_id) else {
                    continue;
                };
                if depths[*candidate].saturating_add(1).saturating_add(heights[*at]) > options.max_depth
                    || chain_contains(table, candidate_id, id)
                {
                    continue;
                }
                let limit = usize::try_from(stored * 3 / 4).unwrap_or(usize::MAX).max(16);
                let Some(delta) = super::delta::encode(source, target, limit * 4) else {
                    continue;
                };
                let compressed = deflate(&delta);
                if compressed.len() as u64 * 4 > stored * 3 {
                    continue;
                }
                if best.as_ref().is_none_or(|(_, _, old)| compressed.len() < old.len()) {
                    best = Some((*candidate, delta, compressed));
                }
            }
            match best {
                Some((candidate, delta, compressed)) => {
                    let candidate_id = table.objs[candidate].id;
                    let source = &contents[&candidate_id];
                    let made = super::delta::apply(source, &delta, Some(size))?;
                    verify(id, table.objs[*at].kind, &made)?;
                    let depth = depths[candidate] + 1;
                    let bytes = compressed.len() as u64;
                    store.rewrite(
                        id,
                        Rewrite::Delta {
                            base: candidate_id,
                            depth,
                            delta_size: delta.len() as u64,
                            data: compressed,
                        },
                    )?;
                    let obj = &mut table.objs[*at];
                    obj.storage = Storage::Delta;
                    obj.base = Some(candidate_id);
                    obj.stored = bytes;
                    obj.depth = depth;
                    depths[*at] = depth;
                    repacked += 1;
                    saved += i64::try_from(stored).unwrap_or(i64::MAX) - i64::try_from(bytes).unwrap_or(i64::MAX);
                }
                None if storage != Storage::Zlib => {
                    saved += rewrite_whole(store, table, *at)?;
                    depths[*at] = 0;
                    repacked += 1;
                }
                None => {}
            }
        }
    }
    // the rest of the rows written before the zlib format (commits, tags, what is only kept as recent)
    for (at, kept) in kept.iter().enumerate() {
        if Instant::now() >= options.deadline {
            report.stopped = Some("the time ran out while repacking".to_string());
            break;
        }
        if table.objs[at].storage == Storage::Raw && *kept && !deleted.contains(&at) {
            saved += rewrite_whole(store, table, at)?;
            repacked += 1;
        }
    }
    Ok((repacked, saved))
}

/// Whether the chain of bases from `from` reaches `id`.
fn chain_contains(table: &Table, from: ObjectId, id: ObjectId) -> bool {
    let mut current = Some(from);
    let mut hops = 0;
    while let Some(at) = current {
        if at == id {
            return true;
        }
        hops += 1;
        if hops > 1000 {
            return true;
        }
        current = table
            .get(&at)
            .filter(|obj| obj.storage == Storage::Delta)
            .and_then(|obj| obj.base);
    }
    false
}

/// For each row, the longest chain of deltas made from it.
fn heights(table: &Table) -> Vec<u32> {
    let mut heights = vec![0u32; table.objs.len()];
    let depths = table.depths();
    let mut order: Vec<usize> = (0..table.objs.len()).filter(|at| depths[*at].is_some()).collect();
    order.sort_by_key(|at| std::cmp::Reverse(depths[*at]));
    for at in order {
        if let Some(base) = table.objs[at]
            .base
            .filter(|_| table.objs[at].storage == Storage::Delta)
            .and_then(|base| table.index.get(&base))
        {
            heights[*base] = heights[*base].max(heights[at] + 1);
        }
    }
    heights
}

/// Rewrite a row as its whole content, compressed. The bytes saved.
fn rewrite_whole(store: &DbStore, table: &mut Table, at: usize) -> Result<i64, Error> {
    let id = table.objs[at].id;
    let (_, content) = store
        .find(&id)?
        .ok_or_else(|| Error::new(format!("git object {id} is missing")))?;
    let data = deflate(&content);
    let bytes = data.len() as u64;
    store.rewrite(id, Rewrite::Whole { data })?;
    let obj = &mut table.objs[at];
    let saved = i64::try_from(obj.stored).unwrap_or(i64::MAX) - i64::try_from(bytes).unwrap_or(i64::MAX);
    obj.storage = Storage::Zlib;
    obj.base = None;
    obj.stored = bytes;
    obj.depth = 0;
    Ok(saved)
}

/// Record each row's true delta depth, and make a chain deeper than `git-server` `db-delta-depth` shorter by storing the row
/// at the limit whole: from the whole rows down the chains, a level at a time.
fn fix_depths(
    store: &DbStore,
    table: &mut Table,
    deleted: &HashSet<usize>,
    options: &GcOptions,
    report: &mut GcReport,
) -> Result<(), Error> {
    let mut dependents: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut depths: Vec<Option<u32>> = vec![None; table.objs.len()];
    let mut queue = VecDeque::new();
    for (at, obj) in table.objs.iter().enumerate() {
        if deleted.contains(&at) {
            continue;
        }
        match obj.base.and_then(|base| table.index.get(&base)) {
            Some(base) if obj.storage == Storage::Delta => dependents.entry(*base).or_default().push(at),
            _ if obj.storage == Storage::Delta => {}
            _ => {
                depths[at] = Some(0);
                queue.push_back(at);
            }
        }
    }
    while let Some(at) = queue.pop_front() {
        let depth = depths[at].unwrap_or(0);
        for dependent in dependents.get(&at).cloned().unwrap_or_default() {
            if depths[dependent].is_some() {
                continue;
            }
            if depth + 1 > options.max_depth && Instant::now() < options.deadline {
                rewrite_whole(store, table, dependent)?;
                report.repacked += 1;
                depths[dependent] = Some(0);
            } else {
                depths[dependent] = Some(depth + 1);
            }
            queue.push_back(dependent);
        }
    }
    let mut by_depth: HashMap<u32, Vec<ObjectId>> = HashMap::new();
    for (at, obj) in table.objs.iter().enumerate() {
        if deleted.contains(&at) {
            continue;
        }
        match depths[at] {
            Some(depth) if depth != obj.depth => by_depth.entry(depth).or_default().push(obj.id),
            Some(_) => {}
            None => tracing::error!(
                repository = %store.repository_id(),
                "git object {} is a delta on a base that is missing or in a cycle; run fsck#GitRepository",
                obj.id
            ),
        }
    }
    for (depth, ids) in by_depth {
        store.set_depth(&ids, depth)?;
        for id in ids {
            if let Some(at) = table.index.get(&id).copied() {
                table.objs[at].depth = depth;
            }
        }
    }
    Ok(())
}

/// What `fsck#GitRepository` found.
#[derive(Clone, Debug, Default)]
pub struct FsckReport {
    pub checked: usize,
    pub errors: Vec<String>,
    pub stopped: Option<String>,
}

impl GitProvider {
    /// `fsck#GitRepository`: decode every row of a `db` repository and check it against its id, until `deadline`.
    pub fn fsck(&self, repository_id: &str, deadline: Instant) -> Result<FsckReport, Error> {
        let _rows = super::rows_guard();
        let definition = self.definition(repository_id)?;
        if definition.kind != RepositoryKind::Db {
            return Err(Error::new(format!(
                "git repository {repository_id} is not a db repository"
            )));
        }
        let store = DbStore::new(
            self.entity()?,
            repository_id,
            self.objects(),
        );
        let mut ids = Vec::new();
        store.scan(&mut |row| {
            ids.push(row.id);
            Ok(())
        })?;
        let mut report = FsckReport::default();
        for batch in ids.chunks(256) {
            if Instant::now() >= deadline {
                report.stopped = Some(format!("the time ran out after {} objects", report.checked));
                break;
            }
            for (id, why) in store.check(batch) {
                report.errors.push(format!("{id}: {why}"));
            }
            report.checked += batch.len();
        }
        for (name, id) in store.list_refs("refs/")? {
            if store.object_size(&id)?.is_none() {
                report
                    .errors
                    .push(format!("ref {name} points to {id}, which is missing"));
            }
        }
        Ok(report)
    }
}
