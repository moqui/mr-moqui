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

//! The push mirror services of `org.moqui.git.GitServices` ([`crate::mirror`]) and `fetch#GitRefs`
//! ([`crate::fetch_refs`]): store, find, delete and push a `GitRepositoryMirror`, the queue's run, and what the
//! post-receive rule asks for. Each needs the delete right on the repository (as its webhooks and branch rules do)
//! unless authorization is disabled; the credential is written, never read back (`hasCredential`), and is given again
//! when where it goes changes. Guide: docs/interfaces/Git-Hosting.md, "Push mirrors" and "Fetching refs".

use std::collections::BTreeMap;

use moqui_core::context::ExecutionContext;
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::rust_service;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use super::hosting::{none, Out, Parameters};
use super::{add_error, count, out, provider_and_id, text};
use crate::mirror::{self, GIT_REPOSITORY_MIRROR};
use crate::{rows_guard, GitProvider};

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    rust_service!(map, store_mirror)?;
    rust_service!(map, delete_mirror)?;
    rust_service!(map, find_mirrors)?;
    rust_service!(map, push_mirror_now)?;
    rust_service!(map, run_mirrors)?;
    rust_service!(map, queue_mirror_pushes)?;
    rust_service!(map, fetch_refs)?;
    Ok(())
}

fn authz_disabled() -> bool {
    ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(false)
}

/// The provider when the current user administers repository `id` (the delete right, as for its webhooks and branch
/// rules: a mirror's credential and a fetch's reach out of the server) or authorization is disabled; otherwise the
/// error recorded, as one not found to a user who may not view it.
fn administered(id: &str) -> Option<GitProvider> {
    let parameters: Parameters = [("repositoryId".to_string(), Value::string(id))].into_iter().collect();
    let (provider, id) = provider_and_id(&parameters, true)?;
    if provider.definition(&id).is_err() {
        add_error(format!("repository {id} not found"));
        return None;
    }
    if authz_disabled() || super::hosting::may(&id, mirror::RIGHT) {
        return Some(provider);
    }
    if super::hosting::may(&id, "view") {
        add_error(format!("you may not change the push mirrors or fetch the refs of {id}"));
    } else {
        add_error(format!("repository {id} not found"));
    }
    None
}

fn field(row: &EntityValue, name: &str) -> Option<String> {
    row.get(name)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .filter(|text| !text.trim().is_empty())
}

fn mirror_row(facade: &EntityFacade, mirror_id: &str) -> Result<Option<EntityValue>, moqui_lib::error::Error> {
    let _rows = rows_guard();
    facade
        .find(GIT_REPOSITORY_MIRROR)?
        .eq("mirrorId", Value::string(mirror_id))
        .use_cache(false)
        .one()
}

/// A Y/N parameter: Y, true; N, false; `None` when not given.
fn yes_no(parameters: &Parameters, key: &str) -> Result<Option<&'static str>, String> {
    match text(parameters, key).as_deref() {
        None => Ok(None),
        Some("Y" | "y" | "true") => Ok(Some("Y")),
        Some("N" | "n" | "false") => Ok(Some("N")),
        Some(other) => Err(format!("{key} is Y or N, not {other}")),
    }
}

/// `store#GitRepositoryMirror`: create a mirror (no `mirrorId`) or change one; an empty credential keeps the one
/// stored. `clearRefPattern` empties the patterns (every branch and tag) and `clearCredential` the username and the
/// credential, as a null parameter is not told from one left out. A change of where the credential goes (the URL's
/// scheme, host and port, its user or `username`, the host key) needs the credential again in the same call, or
/// `clearCredential`: a user who may edit the mirror does not send the stored credential to a host of their own. Who
/// gives a credential (or makes the mirror) is its `createdByUserId`, whose right its pushes are held to unless it was
/// given with authorization disabled (`storedAuthzDisabled`). A new or re-enabled mirror is pushed soon (its
/// `nextAttemptDate` is now). Out: `mirrorId`.
fn store_mirror(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(repository_id) = text(parameters, "repositoryId") else {
        add_error("repositoryId is required");
        return none();
    };
    let Some(_provider) = administered(&repository_id) else {
        return none();
    };
    let existing = match text(parameters, "mirrorId") {
        Some(id) => match mirror_row(facade, &id)? {
            Some(row) if field(&row, "repositoryId").as_deref() == Some(repository_id.as_str()) => Some(row),
            _ => {
                add_error(format!("no push mirror {id} of repository {repository_id}"));
                return none();
            }
        },
        None => None,
    };
    let url = text(parameters, "url")
        .map(|url| url.trim().to_string())
        .or_else(|| existing.as_ref().and_then(|row| field(row, "url")));
    let Some(url) = url else {
        add_error("url is required");
        return none();
    };
    let target = match mirror::parse_mirror_url(&url, mirror::private_network()) {
        Ok(target) => target,
        Err(why) => {
            add_error(format!("url: {why}"));
            return none();
        }
    };
    let fingerprint = text(parameters, "hostKeyFingerprint")
        .map(|value| value.trim().to_string())
        .or_else(|| existing.as_ref().and_then(|row| field(row, "hostKeyFingerprint")));
    if matches!(target, mirror::MirrorUrl::Ssh(_)) && fingerprint.is_none() {
        add_error("hostKeyFingerprint is required for an ssh URL (SHA256:...)");
        return none();
    }
    // a null parameter is not told from an absent one, so a clear is asked for by its flag
    let pattern = match text(parameters, "refPattern") {
        _ if super::flag(parameters, "clearRefPattern") => None,
        Some(pattern) => Some(pattern.trim().to_string()),
        None => existing.as_ref().and_then(|row| field(row, "refPattern")),
    };
    let clear_credential = super::flag(parameters, "clearCredential");
    if let Err(why) = mirror::mirror_maps(pattern.as_deref().unwrap_or_default()) {
        add_error(format!("refPattern: {why}"));
        return none();
    }
    let (delete_refs, enabled) = match (yes_no(parameters, "deleteRefs"), yes_no(parameters, "enabled")) {
        (Ok(delete_refs), Ok(enabled)) => (delete_refs, enabled),
        (Err(why), _) | (_, Err(why)) => {
            add_error(why);
            return none();
        }
    };
    let credential = parameters
        .get("credential")
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .filter(|value| !value.is_empty());
    if credential.as_ref().is_some_and(|value| value.len() > 16 * 1024) {
        add_error("credential is at most 16 KB");
        return none();
    }
    if let Some(row) = &existing {
        let username_now = if parameters.contains_key("username") {
            text(parameters, "username")
        } else {
            field(row, "username")
        };
        let moved = mirror::credential_target(&field(row, "url").unwrap_or_default())
            != mirror::credential_target(&url)
            || username_now != field(row, "username")
            || fingerprint != field(row, "hostKeyFingerprint");
        if moved && field(row, "credential").is_some() && credential.is_none() && !clear_credential {
            add_error(
                "the mirror's credential goes only where it was given for: with a new host, user or host key give the \
                 credential again (credential) or clear it (clearCredential)",
            );
            return none();
        }
    }
    // read before the rows' guard turns authorization off
    let by_application = authz_disabled();
    let _rows = rows_guard();
    let was_enabled = existing
        .as_ref()
        .is_none_or(|row| field(row, "enabled").as_deref() != Some("N"));
    let creating = existing.is_none();
    let mut row = match existing {
        Some(row) => row,
        None => {
            let mut row = EntityValue::make(facade, GIT_REPOSITORY_MIRROR)?;
            row.set_sequenced_id_primary(facade)?;
            row.set("repositoryId", Value::string(&repository_id));
            row.set("createdDate", moqui_core::user::now_value());
            row.set("attemptCount", Value::I64(0));
            row
        }
    };
    let opt = |value: Option<String>| value.map_or(Value::Null, Value::string);
    row.set("url", Value::string(&url));
    if clear_credential {
        row.set("username", Value::Null);
        row.set("credential", Value::Null);
    } else if parameters.contains_key("username") {
        row.set("username", opt(text(parameters, "username")));
    }
    row.set("hostKeyFingerprint", opt(fingerprint));
    row.set("refPattern", opt(pattern.filter(|pattern| !pattern.is_empty())));
    if let Some(value) = delete_refs.or(creating.then_some("N")) {
        row.set("deleteRefs", Value::string(value));
    }
    if let Some(value) = enabled.or(creating.then_some("Y")) {
        row.set("enabled", Value::string(value));
    }
    let given = credential.is_some() && !clear_credential;
    if let Some(credential) = credential.filter(|_| !clear_credential) {
        row.set("credential", Value::string(credential));
    }
    // the credential rides on the right of who gave it: checked again at each push (crate::mirror)
    if creating || given {
        row.set(
            "createdByUserId",
            moqui_core::user::current_user_id()
                .ok()
                .flatten()
                .filter(|user| !user.is_empty())
                .map_or(Value::Null, Value::string),
        );
        row.set(
            "storedAuthzDisabled",
            Value::string(if by_application { "Y" } else { "N" }),
        );
    }
    let now_enabled = enabled.map_or(was_enabled, |value| value == "Y");
    if now_enabled && (creating || !was_enabled) {
        row.set("nextAttemptDate", moqui_core::user::now_value());
    }
    if creating {
        row.create(facade)?;
    } else {
        row.update(facade)?;
    }
    Ok(out([("mirrorId", row.get("mirrorId").cloned().unwrap_or(Value::Null))]))
}

/// `delete#GitRepositoryMirror`. Out: `deleted`.
fn delete_mirror(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(mirror_id) = text(parameters, "mirrorId") else {
        add_error("mirrorId is required");
        return none();
    };
    let Some(row) = mirror_row(facade, &mirror_id)? else {
        return Ok(out([("deleted", Value::Bool(false))]));
    };
    if administered(&field(&row, "repositoryId").unwrap_or_default()).is_none() {
        return none();
    }
    let _rows = rows_guard();
    let deleted = facade
        .find(GIT_REPOSITORY_MIRROR)?
        .eq("mirrorId", Value::string(&mirror_id))
        .delete_all()?;
    Ok(out([("deleted", Value::Bool(deleted > 0))]))
}

/// `find#GitRepositoryMirrors`: a repository's mirrors with `hasCredential`, never the credential. Out: `mirrorList`.
fn find_mirrors(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(repository_id) = text(parameters, "repositoryId") else {
        add_error("repositoryId is required");
        return none();
    };
    if administered(&repository_id).is_none() {
        return none();
    }
    let _rows = rows_guard();
    let rows = facade
        .find(GIT_REPOSITORY_MIRROR)?
        .eq("repositoryId", Value::string(&repository_id))
        .order_by("mirrorId", false)
        .use_cache(false)
        .list()?;
    let list = rows
        .iter()
        .map(|row| {
            let mut entry = BTreeMap::new();
            for name in [
                "mirrorId",
                "repositoryId",
                "url",
                "username",
                "hostKeyFingerprint",
                "refPattern",
                "deleteRefs",
                "enabled",
                "lastPushDate",
                "lastResult",
                "lastError",
                "nextAttemptDate",
                "attemptCount",
                "createdByUserId",
                "storedAuthzDisabled",
                "createdDate",
            ] {
                entry.insert(name.to_string(), row.get(name).cloned().unwrap_or(Value::Null));
            }
            entry.insert(
                "hasCredential".to_string(),
                Value::Bool(field(row, "credential").is_some()),
            );
            Value::Map(entry.into_iter().collect())
        })
        .collect();
    Ok(out([("mirrorList", Value::List(list))]))
}

/// `push#GitRepositoryMirror`: push a mirror soon (now, on the service pool), whatever its back-off. A push of it
/// running answers an error starting `GitMirrorBusy: `. Out: `queued`.
fn push_mirror_now(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(mirror_id) = text(parameters, "mirrorId") else {
        add_error("mirrorId is required");
        return none();
    };
    let Some(row) = mirror_row(facade, &mirror_id)? else {
        add_error(format!("no push mirror {mirror_id}"));
        return none();
    };
    let Some(provider) = administered(&field(&row, "repositoryId").unwrap_or_default()) else {
        return none();
    };
    match provider.mirror_busy(&mirror_id) {
        Ok(true) => {
            add_error(format!("{}: a push of mirror {mirror_id} is running", mirror::BUSY));
            return none();
        }
        Ok(false) => {}
        Err(err) => {
            add_error(err.message());
            return none();
        }
    }
    if let Err(err) = provider.request_mirror_push(&mirror_id) {
        add_error(err.message());
        return none();
    }
    Ok(out([("queued", Value::Bool(true))]))
}

/// `run#GitRepositoryMirrors`: the queue (the `run_GitRepositoryMirrors_frequent` job, and what a request starts):
/// push the mirrors that are due, or the one named now. Out: `pushedIds`, `failedIds`.
fn run_mirrors(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let Some((provider, _)) = provider_and_id(parameters, false) else {
        return none();
    };
    let only = text(parameters, "mirrorId");
    match provider.push_due_mirrors(only.as_deref()) {
        Ok((pushed, failed)) => Ok(out([
            (
                "pushedIds",
                Value::List(pushed.into_iter().map(Value::string).collect()),
            ),
            (
                "failedIds",
                Value::List(failed.into_iter().map(Value::string).collect()),
            ),
        ])),
        Err(err) => {
            add_error(err.message());
            none()
        }
    }
}

/// `queue#GitRepositoryMirrorPushes`: for the framework's post-receive rule: a push of each enabled mirror of the
/// repository, soon. Out: `queuedCount`.
fn queue_mirror_pushes(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let Some((provider, id)) = provider_and_id(parameters, true) else {
        return none();
    };
    match provider.request_mirror_pushes(&id) {
        Ok(queued) => Ok(out([("queuedCount", count(queued))])),
        Err(err) => {
            add_error(err.message());
            none()
        }
    }
}

/// `fetch#GitRefs`: fetch refs of another server (`refMap`, remote ref to local ref) into a repository over HTTPS.
/// Out: `fetched` (the local refs written or already there), `problems`. The password is never logged and is taken
/// out of any error.
fn fetch_refs(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let Some(repository_id) = text(parameters, "repositoryId") else {
        add_error("repositoryId is required");
        return none();
    };
    let Some(provider) = administered(&repository_id) else {
        return none();
    };
    let Some(remote_url) = text(parameters, "remoteUrl") else {
        add_error("remoteUrl is required");
        return none();
    };
    let password = text(parameters, "password");
    let ref_map: Vec<(String, String)> = match parameters.get("refMap") {
        Some(Value::Map(map)) => map
            .iter()
            .map(|(remote, local)| (remote.trim().to_string(), to_plain_string(local).trim().to_string()))
            .filter(|(remote, local)| !remote.is_empty() && !local.is_empty())
            .collect(),
        _ => Vec::new(),
    };
    if ref_map.is_empty() {
        add_error("refMap is required: a Map of remote ref to local ref");
        return none();
    }
    match provider.fetch_refs(
        &repository_id,
        &remote_url,
        text(parameters, "username").as_deref(),
        password.as_deref(),
        &ref_map,
    ) {
        Ok(done) => Ok(out([
            (
                "fetched",
                Value::List(done.fetched.into_iter().map(Value::string).collect()),
            ),
            (
                "problems",
                Value::List(
                    done.problems
                        .into_iter()
                        .map(|problem| Value::string(crate::server::smart_http::redact(&problem, password.as_deref())))
                        .collect(),
                ),
            ),
        ])),
        Err(err) => {
            add_error(crate::server::smart_http::redact(err.message(), password.as_deref()));
            none()
        }
    }
}
