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

//! A fetch with a CI job token (plans/Dogfood.md §8.3, docs/interfaces/Pipelines.md "Job tokens"): the token of a running
//! job run reads its repository at its commit, and nothing else. No user is logged in. The request must be an
//! upload-pack of the job's repository; one ref is advertised, `refs/heads/ci`, at the job's commit; every `want` must
//! be that commit, and the fetch must be shallow, no deeper than the conf's `git-ci` `max-fetch-depth`. A push, another
//! repository, another commit and a token whose job has ended are refused.
//!
//! The commit must still be the repository's to give (docs/interfaces/Git-Hosting.md, Reachability): a merge request's
//! run reads the head its merge request has in the run's repository (`refs/merge-requests/<n>/head`, a fork's and a
//! pushed one's alike, and a hidden one's: the token is the only thing that fetches that), any other run a commit a
//! ref the repository's readers read reaches. A commit its ref has left (a forced branch, a replaced or released
//! head) is refused where the store may hold what its readers were never to read: in a fork network, and in a
//! repository that has had a hidden merge request.

use std::collections::HashSet;
use std::io::Read;
use std::sync::Arc;

use gix::ObjectId;

use super::{
    content_type, factory, io_error, no_cache, pack_threads, path, pktline, provider, refuse_http, upload, wants_v2,
    Advertised, AdvertisedRef, GitHttpRequest, GitService, HttpResponder, Refusal,
};
use crate::ci::token;
use crate::{GitProvider, Repo};
use moqui_core::context::Error;
use moqui_core::entity::EntityFacade;

/// The ref a job token's fetch sees.
const JOB_REF: &str = "refs/heads/ci";
/// A fetch request is small: wants, shallow lines and haves.
const MAX_REQUEST: u64 = 1024 * 1024;

/// The job token of a git request, when it carries one.
pub(super) fn token_of(request: &GitHttpRequest) -> Option<String> {
    token::from_authorization(request.authorization.as_deref()).filter(|token| token.starts_with(token::JOB_PREFIX))
}

/// Check a fetch request's lines: each `want` is `commit`, and a request with wants deepens by a depth of at most
/// `max_depth`.
fn check_request(body: &[u8], commit: &ObjectId, max_depth: u32) -> Result<(), Error> {
    let mut input = body;
    let (mut wants, mut depth) = (0usize, None);
    let hex = commit.to_string();
    while let Some(packet) = pktline::read(&mut input).map_err(io_error)? {
        let Some(line) = packet.text() else { continue };
        if let Some(rest) = line.strip_prefix("want ") {
            if rest.split_whitespace().next() != Some(hex.as_str()) {
                return Err(Error::new("a job token fetches its job's commit and nothing else"));
            }
            wants += 1;
        } else if line.starts_with("want-ref ") || line.starts_with("deepen-since ") || line.starts_with("deepen-not ")
        {
            return Err(Error::new(
                "a job token fetches its job's commit by depth and nothing else",
            ));
        } else if let Some(rest) = line.strip_prefix("deepen ") {
            depth = rest.trim().parse::<u32>().ok();
        }
    }
    if wants > 0 && depth.is_none_or(|depth| depth == 0 || depth > max_depth) {
        return Err(Error::new(format!(
            "a job token's fetch is shallow: --depth of at most {max_depth}"
        )));
    }
    Ok(())
}

/// Whether the job of `auth` still reads `commit`: a merge request's run when the head ref its merge request has in
/// the run's repository reaches it (hidden or not), any other run when a ref every reader of the repository reads
/// does. Where the commits the refs have left are still read by who works in the repository
/// ([`GitProvider::leftovers_readable`]: no fork network, no merge request ever hidden), a commit of the store is
/// enough, as it is for the engine's own read of the pipeline file: every commit there was once a reader's.
fn still_readable(
    provider: &GitProvider,
    entity: &EntityFacade,
    repo: &Repo,
    auth: &token::JobAuth,
    commit: &ObjectId,
) -> Result<bool, Error> {
    let Some(merge_request_id) = &auth.merge_request_id else {
        // what every reader of the repository fetches: the heads of the merge requests that are not hidden too
        return Ok(provider.reachable(repo, commit)? || left_readable(provider, repo, commit)?);
    };
    let num = {
        let _rows = crate::rows_guard();
        crate::ci::queue::merge_request(entity, merge_request_id)?
            .filter(|row| crate::text(row.get("repositoryId")).as_deref() == Some(auth.repository_id.as_str()))
            .and_then(|row| crate::long(row.get("mergeRequestNum")))
    };
    let head = match num {
        Some(num) => repo.store.read_ref(&crate::reach::merge_request_ref(num))?,
        None => None,
    };
    let Some(head) = head else {
        // no such merge request in this repository, or its ref was released
        return left_readable(provider, repo, commit);
    };
    if crate::revwalk::is_ancestor(repo.store.as_ref(), commit, &head)? {
        return Ok(true);
    }
    left_readable(provider, repo, commit)
}

/// The compatibility rule of reads by id, for a commit its ref has left.
fn left_readable(provider: &GitProvider, repo: &Repo, commit: &ObjectId) -> Result<bool, Error> {
    Ok(provider.leftovers_readable(&repo.definition.id)?
        && matches!(repo.store.find(commit)?, Some((gix::objs::Kind::Commit, _))))
}

/// Serve a git HTTP request that carries the job token `job_token`.
pub(super) fn serve(request: &GitHttpRequest, job_token: &str, body: &mut dyn Read, out: &mut dyn HttpResponder) {
    let started = (|| -> Result<(Arc<Repo>, ObjectId), Refusal> {
        let provider = provider()?;
        if !request.secure {
            return Err(Refusal::Forbidden(
                "credentials over plain HTTP are refused; use HTTPS".into(),
            ));
        }
        let entity = factory()?
            .entity()
            .ok_or_else(|| Refusal::Unavailable("no entity facade".into()))?;
        let auth = token::job_of(&entity, job_token)
            .map_err(|err| Refusal::Unavailable(err.message().to_string()))?
            .ok_or_else(|| {
                moqui_core::web::note_auth_failure();
                Refusal::Unauthenticated("the job token is not valid: its job has ended".into())
            })?;
        if request.service != GitService::UploadPack {
            return Err(Refusal::Forbidden("a job token does not push".into()));
        }
        if path::resolve(&request.repository).as_deref() != Some(auth.repository_id.as_str()) {
            return Err(Refusal::Forbidden("the job token is for another repository".into()));
        }
        let repo = provider
            .repo(&auth.repository_id)
            .map_err(|err| Refusal::Unavailable(err.message().to_string()))?;
        let commit = ObjectId::from_hex(auth.commit_id.as_bytes())
            .map_err(|_| Refusal::Unavailable("the job's commit id does not read".into()))?;
        let readable = still_readable(&provider, &entity, &repo, &auth, &commit)
            .map_err(|err| Refusal::Unavailable(err.message().to_string()))?;
        if !readable {
            return Err(Refusal::Forbidden(
                "the job's commit is no longer its repository's to give".into(),
            ));
        }
        Ok((repo, commit))
    })();
    let (repo, commit) = match started {
        Ok(started) => started,
        Err(refusal) => {
            tracing::info!(repository = %request.repository, "git request with a job token refused: {}", refusal.message());
            refuse_http(out, &refusal);
            return;
        }
    };
    let mut headers = vec![("Content-Type", content_type(request.service, request.advertise))];
    headers.extend(no_cache());
    if out.head(200, &headers).is_err() {
        return;
    }
    let advertised = Advertised {
        head: Some(commit),
        branch_ref: JOB_REF.to_string(),
        refs: vec![AdvertisedRef {
            name: JOB_REF.to_string(),
            id: commit,
            peeled: None,
        }],
        review: Vec::new(),
    };
    // nothing is wanted by id but the one ref's commit, and a `have` is acknowledged for that commit alone: what
    // else the store holds is not this token's to learn
    let only = |ids: &[ObjectId]| -> Result<HashSet<ObjectId>, Error> {
        Ok(ids.iter().filter(|id| **id == commit).copied().collect())
    };
    let v2 = wants_v2(request.git_protocol.as_deref());
    let outcome = (|| -> Result<(), Error> {
        let out = out.body();
        if request.advertise {
            if v2 {
                return upload::advertise_v2(out);
            }
            pktline::text(out, &format!("# service={}", request.service.name())).map_err(io_error)?;
            pktline::flush(out).map_err(io_error)?;
            return upload::advertise_v0(out, &advertised);
        }
        let mut bytes = Vec::new();
        body.take(MAX_REQUEST + 1).read_to_end(&mut bytes).map_err(io_error)?;
        if bytes.len() as u64 > MAX_REQUEST {
            return Err(Error::new("the fetch request is too large"));
        }
        check_request(&bytes, &commit, crate::ci::limits().max_fetch_depth)?;
        let mut input: &[u8] = &bytes;
        if v2 {
            upload::serve_v2(
                repo.store.as_ref(),
                &advertised,
                &only,
                &mut input,
                out,
                true,
                pack_threads(),
            )
        } else {
            upload::upload_v0(
                repo.store.as_ref(),
                &advertised,
                &only,
                &mut input,
                out,
                true,
                pack_threads(),
            )
        }
    })();
    if let Err(err) = outcome {
        tracing::warn!(repository = %request.repository, "git fetch with a job token failed: {err}");
        let out = out.body();
        let _ = pktline::text(out, &format!("ERR {}", err.message()));
        let _ = std::io::Write::flush(out);
    }
}
