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

//! The secrets and environments of `org.moqui.git.GitCiServices` (docs/interfaces/Pipelines.md "Secrets"): a
//! repository's, which need the delete right on it, or an owner's, with authorization disabled. No service reads a
//! secret's value back.

use std::collections::BTreeMap;

use moqui_core::rust_service;

use super::{Out, Parameters};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use moqui_core::entity::{Condition, EntityFacade, EntityValue};

use super::{add_error, authz_disabled, current_user, flag, may, none, out, text};
use crate::ci::{field, field_value, is_yes, opt, yes_no, CI_ENVIRONMENT, CI_SECRET};
use crate::values::map;

pub(super) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    rust_service!(map, find_ci_secrets)?;
    rust_service!(map, store_ci_secret)?;
    rust_service!(map, delete_ci_secret)?;
    rust_service!(map, find_ci_environments)?;
    rust_service!(map, store_ci_environment)?;
    rust_service!(map, delete_ci_environment)?;
    Ok(())
}

/// Whose rows a secret or environment service is for: a repository's (the caller needs the delete right, a
/// maintainer's: a developer with the update right could otherwise point a secret at a branch or an environment it
/// pushes to and read it), or an owner's (the caller's authorization is disabled). `None` with the error recorded.
fn scope_of(parameters: &Parameters, what: &str) -> Option<(Option<String>, Option<String>)> {
    match (text(parameters, "repositoryId"), text(parameters, "ownerId")) {
        (Some(repository), None) => {
            if may(&repository, "delete") {
                Some((Some(repository), None))
            } else if may(&repository, "view") {
                add_error(format!(
                    "{what} needs the delete right on repository {repository} (a maintainer's)"
                ));
                None
            } else {
                add_error(format!("repository {repository} not found"));
                None
            }
        }
        (None, Some(owner)) => {
            if authz_disabled() {
                Some((None, Some(owner)))
            } else {
                add_error(format!(
                    "{what} of an owner is the application's (authorization disabled)"
                ));
                None
            }
        }
        _ => {
            add_error("one of repositoryId and ownerId is required");
            None
        }
    }
}

fn scoped(
    facade: &EntityFacade,
    entity_name: &str,
    scope: &(Option<String>, Option<String>),
) -> Result<moqui_core::entity::EntityFind, LibError> {
    let find = facade.find(entity_name)?;
    Ok(match scope {
        (Some(repository), _) => find.eq("repositoryId", Value::string(repository)),
        (None, Some(owner)) => find
            .eq("ownerId", Value::string(owner))
            .condition(Condition::is_null("repositoryId")),
        (None, None) => find,
    })
}

/// `find#CiSecrets`: the secrets of a repository or an owner, without their values. Out: `secretList`.
fn find_ci_secrets(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(scope) = scope_of(parameters, "reading the secrets") else {
        return none();
    };
    let _rows = crate::rows_guard();
    let environments: BTreeMap<String, String> = scoped(facade, CI_ENVIRONMENT, &scope)?
        .list()?
        .iter()
        .filter_map(|row| Some((field(row, "environmentId")?, field(row, "name")?)))
        .collect();
    let secrets: Vec<Value> = scoped(facade, CI_SECRET, &scope)?
        .order_by("name", false)
        .list()?
        .iter()
        .map(|secret| {
            let copy = |name: &'static str| (name, field_value(secret, name));
            map([
                copy("secretId"),
                copy("name"),
                copy("scope"),
                copy("environmentId"),
                (
                    "environmentName",
                    opt(field(secret, "environmentId").and_then(|id| environments.get(&id).cloned())),
                ),
                copy("createdByUserId"),
                copy("createdDate"),
                copy("lastUsedDate"),
            ])
        })
        .collect();
    Ok(out([("secretList", Value::List(secrets))]))
}

/// `store#CiSecret`: make a secret, or change one (`secretId`): its `value` is written and never read back; left
/// out on a change, the value stays. `scope` is protected (the default), tags or environment (with
/// `environmentId`). Out: `secretId`.
fn store_ci_secret(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(scope) = scope_of(parameters, "writing a secret") else {
        return none();
    };
    let _rows = crate::rows_guard();
    let existing = match text(parameters, "secretId") {
        Some(id) => {
            let found = scoped(facade, CI_SECRET, &scope)?
                .eq("secretId", Value::string(&id))
                .one()?;
            if found.is_none() {
                add_error(format!("no secret {id}"));
                return none();
            }
            found
        }
        None => None,
    };
    let name = text(parameters, "name").or_else(|| existing.as_ref().and_then(|row| field(row, "name")));
    let Some(name) = name.filter(|name| {
        name.len() <= 128
            && name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            && !name.starts_with(|ch: char| ch.is_ascii_digit())
    }) else {
        add_error("name is required: letters, digits and underscore, as an environment variable's");
        return none();
    };
    let secret_scope = text(parameters, "scope")
        .or_else(|| existing.as_ref().and_then(|row| field(row, "scope")))
        .unwrap_or_else(|| "protected".to_string());
    if !matches!(secret_scope.as_str(), "protected" | "tags" | "environment") {
        add_error("scope is protected, tags or environment");
        return none();
    }
    let environment = if secret_scope == "environment" {
        let id =
            text(parameters, "environmentId").or_else(|| existing.as_ref().and_then(|row| field(row, "environmentId")));
        let known = match &id {
            Some(id) => scoped(facade, CI_ENVIRONMENT, &scope)?
                .eq("environmentId", Value::string(id))
                .one()?
                .is_some(),
            None => false,
        };
        if !known {
            add_error("a secret of the environment scope needs an environmentId of the same repository or owner");
            return none();
        }
        id
    } else {
        None
    };
    // a secret moved to another scope or environment takes a new value: its old one was given for the old one only
    let moved = existing.as_ref().is_some_and(|row| {
        field(row, "scope").as_deref() != Some(secret_scope.as_str()) || field(row, "environmentId") != environment
    });
    if moved && text(parameters, "value").is_none() {
        add_error("a secret moved to another scope or environment needs a new value");
        return none();
    }
    let own_id = existing.as_ref().and_then(|row| field(row, "secretId"));
    let taken = scoped(facade, CI_SECRET, &scope)?
        .eq("name", Value::string(&name))
        .list()?
        .iter()
        .any(|row| field(row, "secretId") != own_id && field(row, "environmentId") == environment);
    if taken {
        add_error(format!("a secret named {name} is already there"));
        return none();
    }
    let creating = existing.is_none();
    let mut row = match existing {
        Some(row) => row,
        None => {
            let Some(_) = text(parameters, "value") else {
                add_error("value is required");
                return none();
            };
            let mut row = EntityValue::make(facade, CI_SECRET)?;
            row.set_sequenced_id_primary(facade)?;
            row.set("repositoryId", opt(scope.0.clone()));
            row.set("ownerId", opt(scope.1.clone()));
            row.set("createdByUserId", opt(current_user()));
            row.set("createdDate", moqui_core::user::now_value());
            row
        }
    };
    row.set("name", Value::string(&name));
    row.set("scope", Value::string(&secret_scope));
    row.set("environmentId", opt(environment));
    if let Some(value) = parameters
        .get("value")
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
    {
        if value.len() > 64 * 1024 {
            add_error("a secret's value is at most 64 KB");
            return none();
        }
        row.set("secretValue", Value::string(value));
    }
    if creating {
        row.create(facade)?;
    } else {
        row.update(facade)?;
    }
    Ok(out([("secretId", field_value(&row, "secretId"))]))
}

/// `delete#CiSecret`. Out: `deleted`.
fn delete_ci_secret(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(scope) = scope_of(parameters, "deleting a secret") else {
        return none();
    };
    let Some(id) = text(parameters, "secretId") else {
        add_error("secretId is required");
        return none();
    };
    let _rows = crate::rows_guard();
    let deleted = scoped(facade, CI_SECRET, &scope)?
        .eq("secretId", Value::string(id))
        .delete_all()?;
    Ok(out([("deleted", Value::Bool(deleted > 0))]))
}

/// `find#CiEnvironments`: the environments of a repository (the caller views it) or an owner. Out:
/// `environmentList`, without the target's token.
fn find_ci_environments(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let scope = match (text(parameters, "repositoryId"), text(parameters, "ownerId")) {
        (Some(repository), None) if may(&repository, "view") => (Some(repository), None),
        (Some(repository), None) => {
            add_error(format!("repository {repository} not found"));
            return none();
        }
        _ => match scope_of(parameters, "reading the environments") {
            Some(scope) => scope,
            None => return none(),
        },
    };
    let _rows = crate::rows_guard();
    let environments: Vec<Value> = scoped(facade, CI_ENVIRONMENT, &scope)?
        .order_by("name", false)
        .list()?
        .iter()
        .map(|row| {
            let copy = |name: &'static str| (name, field_value(row, name));
            map([
                copy("environmentId"),
                copy("name"),
                copy("targetUrl"),
                copy("cloneUrl"),
                copy("requiredPipeline"),
                ("approvalRequired", Value::Bool(is_yes(row, "approvalRequired"))),
                copy("approverGroupId"),
                copy("allowedRefs"),
                ("hasToken", Value::Bool(crate::ci::deploy::target_token(row).is_some())),
                copy("lastCommitId"),
                copy("lastDeploymentId"),
                copy("lastDeployDate"),
            ])
        })
        .collect();
    Ok(out([("environmentList", Value::List(environments))]))
}

/// `store#CiEnvironment`: make or change an environment. The target's `token` is written and never read back. Out:
/// `environmentId`.
fn store_ci_environment(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(scope) = scope_of(parameters, "writing an environment") else {
        return none();
    };
    let _rows = crate::rows_guard();
    let existing = match text(parameters, "environmentId") {
        Some(id) => {
            let found = scoped(facade, CI_ENVIRONMENT, &scope)?
                .eq("environmentId", Value::string(&id))
                .one()?;
            if found.is_none() {
                add_error(format!("no environment {id}"));
                return none();
            }
            found
        }
        None => None,
    };
    let name = text(parameters, "name").or_else(|| existing.as_ref().and_then(|row| field(row, "name")));
    let Some(name) = name.filter(|name| crate::ci::file::valid_name(name)) else {
        add_error("name is required: letters, digits, dot, dash and underscore");
        return none();
    };
    let own_id = existing.as_ref().and_then(|row| field(row, "environmentId"));
    let taken = scoped(facade, CI_ENVIRONMENT, &scope)?
        .eq("name", Value::string(&name))
        .list()?
        .iter()
        .any(|row| field(row, "environmentId") != own_id);
    if taken {
        add_error(format!("an environment named {name} is already there"));
        return none();
    }
    let creating = existing.is_none();
    let mut row = match existing {
        Some(row) => row,
        None => {
            let mut row = EntityValue::make(facade, CI_ENVIRONMENT)?;
            row.set_sequenced_id_primary(facade)?;
            row.set("repositoryId", opt(scope.0.clone()));
            row.set("ownerId", opt(scope.1.clone()));
            row
        }
    };
    row.set("name", Value::string(&name));
    for field_name in [
        "targetUrl",
        "cloneUrl",
        "requiredPipeline",
        "allowedRefs",
        "approverGroupId",
    ] {
        if parameters.contains_key(field_name) {
            row.set(field_name, opt(text(parameters, field_name)));
        }
    }
    // the target and the clone URL are checked when stored, and again when a deployment calls the target
    for field_name in ["targetUrl", "cloneUrl"] {
        if let Some(url) = field(&row, field_name) {
            let checked = if field_name == "targetUrl" {
                crate::ci::deploy::endpoint(&url).map(|_| ())
            } else if url.starts_with("https://") || url.starts_with("ssh://") {
                Ok(())
            } else {
                Err("the clone URL must be https:// or ssh://".to_string())
            };
            if let Err(why) = checked {
                add_error(why);
                return none();
            }
        }
    }
    if let Some(group) = field(&row, "approverGroupId") {
        let known = facade
            .find("moqui.security.UserGroup")?
            .eq("userGroupId", Value::string(&group))
            .count()?
            > 0;
        if !known {
            add_error(format!("no user group {group}"));
            return none();
        }
    }
    if text(parameters, "token").is_some_and(|token| token.len() < crate::ci::attest::MIN_SECRET) {
        add_error(format!(
            "the target's token is at least {} characters",
            crate::ci::attest::MIN_SECRET
        ));
        return none();
    }
    if parameters.contains_key("approvalRequired") {
        row.set("approvalRequired", yes_no(flag(parameters, "approvalRequired")));
    }
    if let Some(token) = text(parameters, "token") {
        row.set("targetToken", Value::string(token));
    }
    if creating {
        row.create(facade)?;
    } else {
        row.update(facade)?;
    }
    Ok(out([("environmentId", field_value(&row, "environmentId"))]))
}

/// `delete#CiEnvironment`: refused while a secret is scoped to it. Out: `deleted`.
fn delete_ci_environment(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(scope) = scope_of(parameters, "deleting an environment") else {
        return none();
    };
    let Some(id) = text(parameters, "environmentId") else {
        add_error("environmentId is required");
        return none();
    };
    let _rows = crate::rows_guard();
    let secrets = facade
        .find(CI_SECRET)?
        .eq("environmentId", Value::string(&id))
        .count()?;
    if secrets > 0 {
        add_error(format!("environment {id} has {secrets} secrets scoped to it"));
        return none();
    }
    let deleted = scoped(facade, CI_ENVIRONMENT, &scope)?
        .eq("environmentId", Value::string(id))
        .delete_all()?;
    Ok(out([("deleted", Value::Bool(deleted > 0))]))
}
