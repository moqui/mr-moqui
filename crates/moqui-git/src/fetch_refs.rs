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
//! `git-server` `max-object-size`, checked to be whole, then accepted under the repository's lock and lease); the
//! reachability rules of hidden merge requests do not hold them back, as the caller (the application, or a user with
//! the update right) brings them. A local ref is a branch, a tag or a merge request's ref only: a branch or tag moves
//! as a fetch's does (its pre events first, a `GitUpdate` after); a merge request's ref moves with no event, as the
//! review services move it.

use std::collections::HashSet;

use gix::bstr::ByteSlice;
use gix::ObjectId;
use moqui_core::context::Error;

use crate::server::smart_http::{self, Remote};
use crate::{events, lease, ops, server, store, GitProvider};

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

impl GitProvider {
    /// Fetch `ref_map` (remote ref, local ref) from `remote_url` into the repository, each local ref forced to the
    /// remote's value. A ref the remote does not have, a local ref refused, and a branch or tag a rule refused are
    /// problems; the others are fetched.
    pub fn fetch_refs(
        &self,
        repository_id: &str,
        remote_url: &str,
        username: Option<&str>,
        password: Option<&str>,
        ref_map: &[(String, String)],
    ) -> Result<FetchedRefs, Error> {
        let mut out = FetchedRefs::default();
        let mut asked: Vec<(String, String)> = Vec::new();
        for (remote_ref, local_ref) in ref_map {
            match local_ref_refused(local_ref) {
                Some(why) => out.problems.push(why),
                None if asked.iter().any(|(_, local)| local == local_ref) => {
                    out.problems.push(format!("{local_ref} is named twice"));
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
        let present = store.contains_many(&ids)?;
        let wants: Vec<ObjectId> = ids.into_iter().filter(|id| !present.contains(id)).collect();
        let mut haves: Vec<ObjectId> = Vec::new();
        for (_, id) in store.list_refs("refs/")? {
            if haves.len() >= MAX_HAVES {
                break;
            }
            if !haves.contains(&id) {
                haves.push(id);
            }
        }
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
            if let Some(received) = &received {
                received.check_closure(&HashSet::new())?;
            }
            let origin = events::UpdateOrigin {
                source: "fetch",
                transport: "internal",
                user_id: moqui_core::user::current_user_id().ok().flatten(),
                ..Default::default()
            };
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
                let current = store.read_ref(name)?;
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
