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

//! What a run may have (plans/Dogfood.md §8.4, docs/interfaces/Pipelines.md "Trust and approval"). A run is trusted
//! only when it is for a ref of the repository itself, pushed or asked for by an account with the update right. A
//! run for a fork's or a pushed-for-review merge request, or by an account without update, is untrusted: no secrets,
//! caches read and not written, only a pool that takes untrusted runs, and it waits for approval. A secret is given
//! only to a job of a trusted run on a protected branch or tag, or in an environment, and is masked in the log.

use moqui_lib::object::Value;

use moqui_core::context::Error;
use moqui_core::entity::{EntityFacade, EntityValue};

use super::file::Event;
use super::{field, is_yes, CI_ENVIRONMENT, CI_SECRET};

/// Who and what a run is for.
#[derive(Clone, Debug, Default)]
pub struct Origin<'a> {
    pub repository_id: &'a str,
    pub user_id: Option<&'a str>,
    /// The repository a merge request's commits come from, when it is another one (a fork).
    pub source_repository_id: Option<&'a str>,
    pub pushed_for_review: bool,
}

/// Whether `user_id` has the update right on the repository, decided as that user with authorization on.
pub(crate) fn has_update(repository_id: &str, user_id: &str) -> bool {
    let id = repository_id.to_string();
    moqui_core::user::as_user(user_id, move || crate::server::permitted(&id, "update")).unwrap_or(false)
}

/// Whether a run of `event` from `origin` is trusted. A schedule run is: it is the repository's own branch, by its
/// own file. Every other run needs an account with the update right, and a merge request's commits must be the
/// repository's own.
pub fn is_trusted(event: Event, origin: &Origin<'_>) -> bool {
    if origin.pushed_for_review
        || origin
            .source_repository_id
            .is_some_and(|source| source != origin.repository_id)
    {
        return false;
    }
    match (event, origin.user_id) {
        (Event::Schedule, _) => true,
        (_, Some(user)) => has_update(origin.repository_id, user),
        (_, None) => false,
    }
}

/// Whether a branch rule names `ref_name`: a secret of the `protected` or `tags` scope is given only then. Never for
/// a merge request's run, whose commits are not the protected branch's yet.
pub fn is_protected(entity: &EntityFacade, repository_id: &str, event: Event, ref_name: &str) -> Result<bool, Error> {
    if event == Event::MergeRequest {
        return Ok(false);
    }
    Ok(!crate::rules::rules_for(entity, repository_id, ref_name)?.is_empty())
}

/// A secret a job is given: the variable and the value.
#[derive(Clone, PartialEq, Eq)]
pub struct Given {
    pub env: String,
    pub value: String,
}

impl std::fmt::Debug for Given {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // never the value
        f.debug_struct("Given").field("env", &self.env).finish_non_exhaustive()
    }
}

fn scope_allows(secret: &EntityValue, run: &EntityValue, environment_id: Option<&str>) -> bool {
    let protected = is_yes(run, "protectedRef");
    let is_tag = field(run, "refName").is_some_and(|name| name.starts_with("refs/tags/"));
    match field(secret, "scope").as_deref() {
        Some("protected") => protected,
        Some("tags") => protected && is_tag,
        Some("environment") => {
            let wanted = field(secret, "environmentId");
            wanted.is_some() && wanted.as_deref() == environment_id
        }
        _ => false,
    }
}

/// The environment a job names, by its name, of the run's repository or owner.
fn environment_id(entity: &EntityFacade, run: &EntityValue, name: &str) -> Result<Option<String>, Error> {
    let repository = field(run, "repositoryId").unwrap_or_default();
    let found = entity
        .find(CI_ENVIRONMENT)?
        .eq("repositoryId", Value::string(&repository))
        .eq("name", Value::string(name))
        .one()?;
    if let Some(row) = found {
        return Ok(field(&row, "environmentId"));
    }
    let Some(owner) = field(run, "ownerId") else {
        return Ok(None);
    };
    Ok(entity
        .find(CI_ENVIRONMENT)?
        .eq("ownerId", Value::string(owner))
        .eq("name", Value::string(name))
        .list()?
        .iter()
        .find(|row| field(row, "repositoryId").is_none())
        .and_then(|row| field(row, "environmentId")))
}

/// The secrets the job of `spec` (its `jobSpec`) is given in `run`: none for an untrusted run; otherwise each one it
/// asks for whose scope the run meets, the repository's before its owner's. A secret that is not given leaves its
/// variable unset. `touch` records the use (`lastUsedDate`).
pub fn secrets_for(
    entity: &EntityFacade,
    run: &EntityValue,
    spec: &serde_json::Value,
    touch: bool,
) -> Result<Vec<Given>, Error> {
    if !is_yes(run, "trusted") {
        return Ok(Vec::new());
    }
    let asks: Vec<(String, String)> = spec["secrets"]
        .as_array()
        .map(|asks| {
            asks.iter()
                .filter_map(|ask| Some((ask["name"].as_str()?.to_string(), ask["env"].as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    if asks.is_empty() {
        return Ok(Vec::new());
    }
    let environment = match spec["environment"].as_str() {
        Some(name) => environment_id(entity, run, name)?,
        None => None,
    };
    let repository = field(run, "repositoryId").unwrap_or_default();
    let owner = field(run, "ownerId");
    let mut given = Vec::new();
    for (name, env) in asks {
        let mut candidates: Vec<EntityValue> = entity
            .find(CI_SECRET)?
            .eq("repositoryId", Value::string(&repository))
            .eq("name", Value::string(&name))
            .list()?
            .iter()
            .cloned()
            .collect();
        if let Some(owner) = &owner {
            let of_owner = entity
                .find(CI_SECRET)?
                .eq("ownerId", Value::string(owner))
                .eq("name", Value::string(&name))
                .list()?;
            candidates.extend(
                of_owner
                    .iter()
                    .filter(|row| field(row, "repositoryId").is_none())
                    .cloned(),
            );
        }
        // an environment's secret before a general one
        candidates.sort_by_key(|row| field(row, "scope").as_deref() != Some("environment"));
        let Some(secret) = candidates
            .iter()
            .find(|secret| scope_allows(secret, run, environment.as_deref()))
        else {
            continue;
        };
        let Some(value) = field(secret, "secretValue") else {
            continue;
        };
        if touch {
            if let Some(id) = field(secret, "secretId") {
                super::set_fields(
                    entity,
                    CI_SECRET,
                    "secretId",
                    &id,
                    None,
                    vec![("lastUsedDate", moqui_core::user::now_value())],
                )?;
            }
        }
        given.push(Given { env, value });
    }
    Ok(given)
}

/// What stands for a secret in a log.
pub const MASK: &str = "***";

/// The shortest value that is masked: a shorter one would blank ordinary text.
const MASK_MIN: usize = 4;

/// What to look for in a log to hide `values`: each value, and each line of one with several lines, longest first so
/// a value that holds another is hidden whole.
pub fn mask_patterns(values: &[String]) -> Vec<String> {
    let mut patterns: Vec<String> = Vec::new();
    for value in values {
        for part in std::iter::once(value.as_str()).chain(value.lines()) {
            let part = part.trim();
            if part.len() >= MASK_MIN && !patterns.iter().any(|known| known == part) {
                patterns.push(part.to_string());
            }
        }
    }
    patterns.sort_by_key(|pattern| std::cmp::Reverse(pattern.len()));
    patterns
}

/// `text` with every pattern replaced by [`MASK`].
pub fn mask(text: &str, patterns: &[String]) -> String {
    let mut out = std::borrow::Cow::Borrowed(text);
    for pattern in patterns {
        if out.contains(pattern.as_str()) {
            out = std::borrow::Cow::Owned(out.replace(pattern.as_str(), MASK));
        }
    }
    out.into_owned()
}

#[cfg(test)]
#[path = "trust_tests.rs"]
mod tests;
