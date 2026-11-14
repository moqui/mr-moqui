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

//! `fetch#GitRefs`: named refs of another server fetched into a repository of any store (plans/Dogfood.md DG8, the
//! GitHub importer's pull request heads; docs/interfaces/Git-Hosting.md "Fetching refs"), over smart HTTP
//! ([`crate::server::smart_http`]). The objects come in as a push's do (one pack into quarantine, each object held to
//! `git-server` `max-object-size`, checked to be whole, then accepted under the repository's lock and lease), and for
//! a caller with authorization on they are held to a push's reachability rules: the haves are the refs the caller
//! reads, a ref's new value the pack did not bring is the caller's own, and the pack builds on and names only what the
//! caller reads, so objects of a hidden merge request or of another fork of the store are neither told to the remote
//! nor made readable. A local ref is a branch, a tag or a merge request's head only: a branch or tag moves as a push's
//! does (the branch rules, its pre events first, a `GitUpdate` after); a merge request's head moves with no event, with
//! authorization disabled (the importer) or, for a caller with it on, only as the first head of an imported merge
//! request, which the merge request then records. The application's outbound check is asked first.

use std::collections::HashSet;

use gix::bstr::ByteSlice;
use gix::ObjectId;
use moqui_core::context::{Error, ExecutionContext};
use moqui_lib::object::Value;

use crate::server::smart_http::{self, Remote};
use crate::{events, lease, ops, rows_guard, rules, server, store, GitProvider, Outbound};

/// The local refs `fetch#GitRefs` writes.
pub(crate) const LOCAL_PREFIXES: [&str; 3] = ["refs/merge-requests/", "refs/heads/", "refs/tags/"];

/// The most commits a fetch tells the server it has.
const MAX_HAVES: usize = 256;

/// What a fetch did: the local refs written (or already at the remote's value), and why the others were not.
#[derive(Debug, Default)]
pub struct FetchedRefs {
    pub fetched: Vec<String>,
    pub problems: Vec<String>,
}

/// Why `name` may not be a local ref of a fetch: not a valid ref name, or not under [`LOCAL_PREFIXES`] (the framework's
/// own `refs/moqui-*` among those refused).
pub(crate) fn local_ref_refused(name: &str) -> Option<String> {
    if gix::validate::reference::name(name.as_bytes().as_bstr()).is_err() {
        return Some(format!("{name} is not a ref name"));
    }
    if !LOCAL_PREFIXES.iter().any(|prefix| name.starts_with(prefix)) || name.contains("/moqui-") {
        return Some(format!(
            "{name} is refused: a fetch writes refs/heads/, refs/tags/ and refs/merge-requests/ only"
        ));
    }
    None
}

fn authz_disabled() -> bool {
    ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(false)
}

/// What a caller with authorization on is told for a merge request ref it may not set.
fn head_refused(name: &str) -> String {
    format!("{name} is refused: a fetch sets only the first head of an imported merge request")
}

impl GitProvider {
    /// The commits a fetch into `repo` tells the server it has: the tips of the refs the repository's readers read
    /// (branches, tags, the heads of merge requests that are not hidden), at most [`MAX_HAVES`]. Never a hidden merge
    /// request's head or the framework's own refs, whose ids are not the caller's to tell another server.
    pub(crate) fn fetch_haves(&self, repo: &crate::Repo) -> Result<Vec<ObjectId>, Error> {
        let mut haves: Vec<ObjectId> = Vec::new();
        for (_, id) in self.readable_refs(repo)?.readable {
            if haves.len() >= MAX_HAVES {
                break;
            }
            if !haves.contains(&id) {
                haves.push(id);
            }
        }
        Ok(haves)
    }

    /// The merge request whose head `name` is, when a caller with authorization on may set it by a fetch: an
    /// imported one (`importedByUserId`) that has no head yet. Its id.
    fn headless_import(&self, repository_id: &str, name: &str) -> Result<Option<String>, Error> {
        let Some(num) = name
            .strip_prefix(crate::reach::MERGE_REQUEST_REFS)
            .and_then(|rest| rest.strip_suffix("/head"))
            .and_then(|num| num.parse::<i64>().ok())
        else {
            return Ok(None);
        };
        let _rows = rows_guard();
        let row = self
            .entity()?
            .find("moqui.resource.GitMergeRequest")?
            .eq("repositoryId", Value::string(repository_id))
            .eq("mergeRequestNum", Value::I64(num))
            .use_cache(false)
            .one()?;
        Ok(row
            .filter(|row| crate::text(row.get("importedByUserId")).is_some())
            .filter(|row| crate::text(row.get("headCommitId")).is_none())
            .and_then(|row| crate::text(row.get("mergeRequestId"))))
    }

    /// Fetch `ref_map` (remote ref, local ref) from `remote_url` into the repository, each local ref forced to the
    /// remote's value. A ref the remote does not have or did not send, a local ref refused, and a branch or tag a rule
    /// refused are problems; the others are fetched.
    pub fn fetch_refs(
        &self,
        repository_id: &str,
        remote_url: &str,
        username: Option<&str>,
        password: Option<&str>,
        ref_map: &[(String, String)],
    ) -> Result<FetchedRefs, Error> {
        let checked = !authz_disabled();
        let mut out = FetchedRefs::default();
        let mut asked: Vec<(String, String)> = Vec::new();
        // the merge requests whose first head a caller with authorization on sets, by local ref
        let mut imports: Vec<(String, String)> = Vec::new();
        for (remote_ref, local_ref) in ref_map {
            match local_ref_refused(local_ref) {
                Some(why) => out.problems.push(why),
                None if asked.iter().any(|(_, local)| local == local_ref) => {
                    out.problems.push(format!("{local_ref} is named twice"));
                }
                None if checked && local_ref.starts_with(crate::reach::MERGE_REQUEST_REFS) => {
                    match self.headless_import(repository_id, local_ref)? {
                        Some(request_id) => {
                            imports.push((local_ref.clone(), request_id));
                            asked.push((remote_ref.clone(), local_ref.clone()));
                        }
                        None => out.problems.push(head_refused(local_ref)),
                    }
                }
                None => asked.push((remote_ref.clone(), local_ref.clone())),
            }
        }
        if asked.is_empty() {
            return Ok(out);
        }
        let remote = Remote {
            url: remote_url,
            username,
            password,
            private_network: crate::mirror::private_network(),
        };
        let shown = remote.shown();
        if self.entity()?.sim_active() {
            return Err(Error::new(format!(
                "fetch of {shown} denied because sim mode is active"
            )));
        }
        moqui_core::entity::fence::check_effect(
            moqui_core::entity::fence::Effect::RemoteCall,
            &format!("fetch of {shown}"),
        )?;
        let repo = self.repo(repository_id)?;
        self.still_writable(repository_id)?;
        // as before a push mirror's push: the application may hold what a repository reaches out for
        crate::outbound_allowed(repository_id, Outbound::Fetch, "")
            .map_err(|why| Error::new(format!("not fetched: {why}")))?;
        let store = repo.store.as_ref();
        let advertised = smart_http::advertisement(&remote, "git-upload-pack")?;
        let mut pairs: Vec<(String, ObjectId)> = Vec::new();
        for (remote_ref, local_ref) in asked {
            match advertised.refs.get(&remote_ref) {
                Some(id) => pairs.push((local_ref, *id)),
                None => out.problems.push(format!("{shown} has no {remote_ref}")),
            }
        }
        let ids: Vec<ObjectId> = pairs
            .iter()
            .map(|(_, id)| *id)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        // an id the store holds is not wanted only when it is the caller's (as a push's new value): another is asked
        // of the server as one the store does not hold, so the answer does not tell the two apart
        let mut own: HashSet<ObjectId> = store.contains_many(&ids)?.into_iter().collect();
        if checked {
            let mut kept = HashSet::new();
            for id in own {
                if self.own_value_in(&repo, &id)? {
                    kept.insert(id);
                }
            }
            own = kept;
        }
        let wants: Vec<ObjectId> = ids.into_iter().filter(|id| !own.contains(id)).collect();
        let haves = self.fetch_haves(&repo)?;
        let scratch = self.scratch("fetch", repository_id);
        let quarantine = server::quarantine_in(store, &scratch);
        let result = (|| -> Result<(), Error> {
            let mut received = smart_http::fetch_pack(
                store,
                &remote,
                &advertised,
                &wants,
                &haves,
                &quarantine,
                server::max_object_size(),
            )?;
            let mut brought: HashSet<ObjectId> = HashSet::new();
            if let Some(received) = &received {
                received
                    .check_closure(&HashSet::new())
                    .map_err(|err| Error::new(format!("fetching from {shown}: {}", err.message())))?;
                if checked {
                    if let Some(foreign) = self.foreign_in_pack(&repo, received)? {
                        tracing::warn!(
                            repository = repository_id,
                            "a fetched pack builds on {foreign}, which no ref here reaches"
                        );
                        // said as an object the store does not hold is (pack_in), so neither tells the two apart
                        return Err(Error::new(format!(
                            "fetching from {shown}: {}{foreign}, which a pushed object refers to",
                            server::pack_in::MISSING_OBJECT
                        )));
                    }
                }
                brought.extend(pairs.iter().map(|(_, id)| *id).filter(|id| received.holds(id)));
            }
            let origin = events::UpdateOrigin {
                source: "fetch",
                transport: "internal",
                user_id: moqui_core::user::current_user_id().ok().flatten(),
                ..Default::default()
            };
            let entity = self.entity()?;
            // the lease before this process's lock, so no writer holds the lock while it waits on another member
            let shared = self.shared_turn(repository_id, repo.definition.kind)?;
            let lock = self.lock_of(repository_id);
            let turn = moqui_lib::sync::lock(&lock);
            self.still_writable(repository_id)?;
            lease::check(shared.as_ref())?;
            if let Some(received) = &mut received {
                received.accept()?;
            }
            drop(received);
            let prior = store.list_refs("refs/")?;
            let mut changes = Vec::new();
            for (name, id) in &pairs {
                // what the pack brought, or what the store held that is the caller's: never an id the server named
                // and did not send
                if !brought.contains(id) && !own.contains(id) {
                    out.problems.push(format!("{name}: {shown} did not send {id}"));
                    continue;
                }
                let current = store.read_ref(name)?;
                let import = imports.iter().find(|(local, _)| local == name).map(|(_, id)| id);
                if import.is_some() && current.is_some() {
                    out.problems.push(head_refused(name));
                    continue;
                }
                if current == Some(*id) {
                    out.fetched.push(name.clone());
                    continue;
                }
                let branch_or_tag = name.starts_with("refs/heads/") || name.starts_with("refs/tags/");
                if name.starts_with("refs/heads/") {
                    let kind = store.find(id)?.map(|(kind, _)| kind);
                    if kind != Some(gix::objs::Kind::Commit) {
                        out.problems.push(format!("{name}: {id} is not a commit"));
                        continue;
                    }
                }
                if branch_or_tag {
                    let reference = events::RefUpdate {
                        name: name.clone(),
                        old: current,
                        new: Some(*id),
                        change_type: events::change_type(store, current, Some(*id)).to_string(),
                    };
                    // the branch rules, as for a push: a fetch is no way around a protected branch
                    if checked {
                        let found = rules::rules_for(&entity, repository_id, name)?;
                        let change = rules::RefChangeKind::parse(&reference.change_type);
                        if let Err(reason) = rules::check_direct(&found, name, change, &rules::current_user_in) {
                            out.problems.push(format!("{name}: GitProtectedBranch: {reason}"));
                            continue;
                        }
                    }
                    if let Err(err) = events::pre_check(
                        store,
                        repository_id,
                        &repo.definition.branch,
                        &origin,
                        &prior,
                        &reference,
                    ) {
                        out.problems.push(format!("{name}: {}", err.message()));
                        continue;
                    }
                }
                lease::check(shared.as_ref())?;
                if !store.compare_and_set_ref(name, current, *id)? {
                    out.problems.push(format!("{name} moved during the fetch"));
                    continue;
                }
                if let Some(request_id) = import {
                    // the merge request's head is the one its ref has: what its approvals are counted for
                    let _rows = rows_guard();
                    entity
                        .find("moqui.resource.GitMergeRequest")?
                        .eq("mergeRequestId", Value::string(request_id))
                        .condition(moqui_core::entity::Condition::is_null("headCommitId"))
                        .update_all([("headCommitId".to_string(), Value::string(id.to_string()))])?;
                }
                if branch_or_tag {
                    changes.push(store::RefChange {
                        name: name.clone(),
                        old: current,
                        new: Some(*id),
                    });
                }
                out.fetched.push(name.clone());
            }
            drop(turn);
            if changes.is_empty() {
                return Ok(());
            }
            let head_ref = ops::branch_ref(&repo.definition.branch);
            if let Some(change) = changes.iter().find(|change| change.name == head_ref) {
                self.remember_head(repository_id, change.new, None);
            }
            self.record_update(repository_id, &origin, store, &prior, &changes);
            Ok(())
        })();
        if let Err(err) = std::fs::remove_dir_all(&scratch) {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("cannot remove {}: {err}", scratch.display());
            }
        }
        self.refresh(repository_id);
        result.map(|()| out)
    }
}

#[cfg(test)]
#[path = "fetch_refs_tests.rs"]
mod tests;
