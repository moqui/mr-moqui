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

//! Artifact authorization, ported from `ArtifactExecutionFacadeImpl.isPermitted`.
//!
//! The artifact stack carries each frame's [`AuthInfo`]. A new frame is decided against its
//! parent frame: an inheritable allow on the parent covers it, an always-allow skips every
//! check, and a deny anywhere on the stack stops a plain allow. The checks for a user's groups
//! are read once and kept for the current user, as Java's UserInfo caches them.

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use moqui_lib::object::Value;

use super::login::{entity_facade, now_millis};
use super::session::{current_user_id, with_session, AuthzCache, TarpitCheck};
use crate::context::{Error, ExecutionContext};
use crate::entity::{EntityFacade, EntityValue};

const AUTHZT_ALWAYS: &str = "AUTHZT_ALWAYS";
const AUTHZT_DENY: &str = "AUTHZT_DENY";
const AUTHZT_ALLOW: &str = "AUTHZT_ALLOW";
const AUTHZA_ALL: &str = "AUTHZA_ALL";
const AT_ENTITY: &str = "AT_ENTITY";

/// Bumped when authorization tables change, to refresh membership, tarpit and filter caches.
/// The authorization-check list itself follows Java's current-user lifetime.
static AUTHZ_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Entity writes call this. Only the authorization tables, the entity filter tables among them, change the generation.
///
/// The generation moves at once, and again when the write's transaction ends: a session that
/// reloaded in between read the rows from before the commit, or the writer's own rows that a
/// rollback then undid, and must not keep them.
pub(crate) fn note_entity_write(entity_name: &str) {
    if !matches!(
        entity_name,
        "moqui.security.ArtifactAuthz"
            | "moqui.security.ArtifactGroupMember"
            | "moqui.security.UserGroupMember"
            | "moqui.security.ArtifactTarpit"
            | "moqui.security.ArtifactAuthzFilter"
            | "moqui.security.EntityFilterSet"
            | "moqui.security.EntityFilter"
    ) {
        return;
    }
    bump_generation();
    let tx = ExecutionContext::with_current(|ec| ec.factory().entity())
        .flatten()
        .and_then(|facade| facade.transactions_for(entity_name).ok())
        .filter(crate::transaction::TransactionManager::in_transaction);
    if let Some(tx) = tx {
        if let Err(err) = tx.on_commit(bump_generation) {
            tracing::warn!(error = %err, "could not move the authorization generation on commit");
        }
        if let Err(err) = tx.on_rollback(bump_generation) {
            tracing::warn!(error = %err, "could not move the authorization generation on rollback");
        }
    }
}

fn bump_generation() {
    AUTHZ_GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// One row of `ArtifactAuthz` joined to a member of its artifact group.
#[derive(Debug)]
pub(crate) struct AuthzCheck {
    pub(crate) artifact_authz_id: String,
    pub(crate) authz_type: String,
    pub(crate) authz_action: String,
    pub(crate) authz_service_name: String,
    pub(crate) artifact_name: String,
    pub(crate) artifact_type: String,
    pub(crate) name_is_pattern: bool,
    pub(crate) inherit_authz: bool,
    pub(crate) filter_map: String,
}

/// What was authorized for an artifact frame, and whether a child frame may inherit it.
#[derive(Clone, Debug, Default)]
pub(crate) struct AuthInfo {
    pub(crate) user_id: String,
    pub(crate) authz_type: Option<String>,
    pub(crate) action: Option<String>,
    pub(crate) inheritable: bool,
    pub(crate) granted: Option<bool>,
    pub(crate) aacv: Option<Arc<AuthzCheck>>,
}

impl AuthInfo {
    fn from_check(check: &Arc<AuthzCheck>, user_id: &str, granted: bool) -> Self {
        Self {
            user_id: user_id.to_string(),
            authz_type: Some(check.authz_type.clone()),
            action: Some(check.authz_action.clone()),
            inheritable: check.inherit_authz,
            granted: Some(granted),
            aacv: Some(Arc::clone(check)),
        }
    }

    /// Mark the frame inheritable for any action, as `setAnonymousAuthorizedAll` does.
    pub(crate) fn anonymous_all(&mut self, user_id: &str) {
        self.inheritable = true;
        self.user_id = if user_id.is_empty() {
            "_NA_".to_string()
        } else {
            user_id.to_string()
        };
        if self.authz_type.as_deref() != Some(AUTHZT_ALWAYS) {
            self.authz_type = Some(AUTHZT_ALLOW.to_string());
        }
        self.action = Some(AUTHZA_ALL.to_string());
    }

    /// Mark the frame inheritable for view, as `setAnonymousAuthorizedView` does.
    pub(crate) fn anonymous_view(&mut self, user_id: &str) {
        self.inheritable = true;
        self.user_id = if user_id.is_empty() {
            "_NA_".to_string()
        } else {
            user_id.to_string()
        };
        if self.authz_type.as_deref() != Some(AUTHZT_ALWAYS) {
            self.authz_type = Some(AUTHZT_ALLOW.to_string());
        }
        if self.action.as_deref() != Some(AUTHZA_ALL) {
            self.action = Some("AUTHZA_VIEW".to_string());
        }
    }
}

/// The enum id for an action name. Ids pass through.
pub(crate) fn action_enum(action: &str) -> String {
    match action {
        "view" => "AUTHZA_VIEW",
        "create" => "AUTHZA_CREATE",
        "update" => "AUTHZA_UPDATE",
        "delete" => "AUTHZA_DELETE",
        "all" => AUTHZA_ALL,
        other => other,
    }
    .to_string()
}

/// True while an [`crate::context::ExecutionContext::enter`] guard is live on this thread.
pub fn context_entered() -> bool {
    ExecutionContext::is_entered()
}

/// Check one artifact without pushing it, against the top of the entered context's stack.
///
/// Allowed when no context is entered or authorization is disabled. This is the check a push makes,
/// for code that only asks.
///
/// # Errors
///
/// Returns an error naming the user, artifact, and action when the user is not authorized.
pub fn authorize(artifact_type: &str, name: &str, action: &str) -> Result<(), Error> {
    if !context_entered() {
        return Ok(());
    }
    let (last, ancestor_deny, disabled) =
        ExecutionContext::with_current(|ec| ec.stack_authz_state()).unwrap_or((None, false, true));
    let (permitted, info) = is_permitted(
        artifact_type,
        name,
        action,
        &Permit {
            requires_authz: true,
            authz_disabled: disabled,
            count_tarpit: false,
            parameters: None,
        },
        last.as_ref(),
        ancestor_deny,
    )?;
    if permitted {
        Ok(())
    } else {
        Err(denied(artifact_type, name, action, &info))
    }
}

/// What a push asks of [`is_permitted`].
pub(crate) struct Permit<'a> {
    pub(crate) requires_authz: bool,
    pub(crate) authz_disabled: bool,
    pub(crate) count_tarpit: bool,
    pub(crate) parameters: Option<&'a BTreeMap<String, Value>>,
}

pub(crate) fn denied(artifact_type: &str, name: &str, action: &str, info: &AuthInfo) -> Error {
    let user = current_user_id().ok().flatten().unwrap_or_default();
    let user = if user.is_empty() { "[No User]".to_string() } else { user };
    let because = match &info.aacv {
        Some(_) if info.granted == Some(false) => " because of a deny record",
        _ => "",
    };
    Error::of_kind(
        crate::context::ErrorKind::Unauthorized,
        format!("User {user} is not authorized for {artifact_type} [{name}] action {action}{because}"),
    )
    .with_denied(crate::context::DeniedArtifact {
        artifact_type: artifact_type.to_string(),
        name: name.to_string(),
        action: action.to_string(),
    })
}

/// Decide a new frame, as Java `isPermitted` does. Returns the decision and the frame's auth info.
///
/// `last` is the info of the frame below, `ancestor_deny` is true when any frame carries a deny.
pub(crate) fn is_permitted(
    artifact_type: &str,
    name: &str,
    action: &str,
    permit: &Permit<'_>,
    last: Option<&AuthInfo>,
    ancestor_deny: bool,
) -> Result<(bool, AuthInfo), Error> {
    let is_entity = artifact_type == AT_ENTITY;
    let inherited = |info: &mut AuthInfo| {
        if let Some(last) = last {
            *info = last.clone();
        }
    };
    let mut info = AuthInfo::default();
    // Entities are checked while the checks themselves read entities, so this must not recurse.
    if (!permit.requires_authz || permit.authz_disabled) && is_entity {
        if last.is_some_and(|last| last.inheritable) {
            inherited(&mut info);
        }
        return Ok((true, info));
    }
    if !context_entered() {
        return Ok((true, info));
    }
    // As Java, a frame that is not an entity is still counted for the tarpit and matched against the user's checks
    // while authorization is disabled: nothing below refuses it then, but it records what was authorized.
    let action_id = action_enum(action);
    if !is_entity
        && permit.count_tarpit
        && artifact_flag(artifact_type, "tarpit-enabled")
        && (permit.requires_authz || (artifact_type != "AT_XML_SCREEN" && artifact_type != "AT_REST_PATH"))
    {
        // Nothing is refused while authorization is disabled, so checks that cannot be read (a run with no
        // database) are passed over.
        match check_tarpit(artifact_type, name) {
            Err(err) if permit.authz_disabled && err.kind() != crate::context::ErrorKind::Tarpit => {
                tracing::debug!(error = %err, "tarpit checks not read while authorization is disabled");
            }
            outcome => outcome?,
        }
    }
    if let Some(last) = last {
        if last.inheritable
            && last.authz_type.as_deref() == Some(AUTHZT_ALWAYS)
            && (last.action.as_deref() == Some(AUTHZA_ALL) || last.action.as_deref() == Some(action_id.as_str()))
        {
            return Ok((true, last.clone()));
        }
    }
    if !artifact_flag(artifact_type, "authz-enabled") {
        inherited(&mut info);
        return Ok((true, info));
    }
    let user_id = current_user_id()?.unwrap_or_default();
    let mut deny: Option<Arc<AuthzCheck>> = None;
    let mut allow: Option<Arc<AuthzCheck>> = None;
    {
        let _guard = disable_authz_guard()?;
        let facade = entity_facade()?;
        let checks = match user_checks(&facade) {
            Err(err) if permit.authz_disabled => {
                tracing::debug!(error = %err, "authz checks not read while authorization is disabled");
                Arc::default()
            }
            checks => checks?,
        };
        for check in checks.iter() {
            if check.artifact_type != artifact_type
                || !(check.authz_action == AUTHZA_ALL || check.authz_action == action_id)
                || !(check.name_is_pattern || check.artifact_name == name)
            {
                continue;
            }
            match record_applies(check, name, permit.parameters) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(err) => {
                    // A record that cannot be evaluated fails closed: a deny applies, anything else does not.
                    tracing::error!(
                        artifact_authz_id = %check.artifact_authz_id,
                        authz_type = %check.authz_type,
                        error = %err,
                        "authz record did not evaluate; a deny applies and an allow does not"
                    );
                    if check.authz_type == AUTHZT_DENY {
                        deny = Some(Arc::clone(check));
                    }
                    continue;
                }
            }
            let mut authz_type = check.authz_type.clone();
            if !check.authz_service_name.is_empty() {
                if let Some(overridden) =
                    authz_service_type(&check.authz_service_name, &user_id, &action_id, artifact_type, name)?
                {
                    authz_type = overridden;
                }
            }
            match authz_type.as_str() {
                AUTHZT_DENY => deny = Some(Arc::clone(check)),
                AUTHZT_ALWAYS => return Ok((true, AuthInfo::from_check(check, &user_id, true))),
                AUTHZT_ALLOW if deny.is_none() && !ancestor_deny => allow = Some(Arc::clone(check)),
                _ => {}
            }
        }
        if let Some(check) = deny {
            info = AuthInfo::from_check(&check, &user_id, false);
            if !permit.requires_authz || permit.authz_disabled {
                return Ok((true, info));
            }
            record_failure(&facade, artifact_type, name, &action_id, &user_id, true);
            return Ok((false, info));
        }
        if let Some(check) = allow {
            return Ok((true, AuthInfo::from_check(&check, &user_id, true)));
        }
        if let Some(last) = last {
            if last.inheritable
                && (last.user_id == "_NA_" || last.user_id == user_id)
                && (last.action.as_deref() == Some(AUTHZA_ALL) || last.action.as_deref() == Some(action_id.as_str()))
                && last.authz_type.as_deref() != Some(AUTHZT_DENY)
            {
                return Ok((true, last.clone()));
            }
        }
        if !permit.requires_authz || permit.authz_disabled {
            if last.is_some_and(|last| last.inheritable) {
                inherited(&mut info);
            }
            return Ok((true, info));
        }
        record_failure(&facade, artifact_type, name, &action_id, &user_id, false);
    }
    info.granted = Some(false);
    Ok((false, info))
}

fn disable_authz_guard() -> Result<crate::context::AuthzGuard, Error> {
    ExecutionContext::with_current(|ec| ec.disable_authz())
        .ok_or_else(|| Error::new("User is not authorized (no execution context)"))
}

/// Whether the record's name pattern and `filterMap` match. An error means the record could not
/// be evaluated: an invalid pattern, or a `filterMap` that does not evaluate.
fn record_applies(
    check: &AuthzCheck,
    name: &str,
    parameters: Option<&BTreeMap<String, Value>>,
) -> Result<bool, String> {
    if check.name_is_pattern && !pattern_matches(&check.artifact_name, name)? {
        return Ok(false);
    }
    if !check.filter_map.is_empty() && !filter_map_matches(&check.filter_map, parameters)? {
        return Ok(false);
    }
    Ok(true)
}

fn pattern_matches(pattern: &str, name: &str) -> Result<bool, String> {
    moqui_lib::object::compare(&Value::string(name), "matches", &Value::string(pattern), "String")
        .map_err(|err| format!("name pattern [{pattern}]: {err}"))
}

/// Every entry of the check's `filterMap` must equal the same parameter. Without parameters the
/// filter does not apply, as in Java.
fn filter_map_matches(expression: &str, parameters: Option<&BTreeMap<String, Value>>) -> Result<bool, String> {
    let Some(parameters) = parameters else {
        return Ok(true);
    };
    let wanted = crate::script::eval_map(expression).map_err(|err| format!("filterMap [{expression}]: {err}"))?;
    Ok(wanted
        .iter()
        .all(|(key, value)| parameters.get(key).is_some_and(|actual| actual == value)))
}

/// The service named on an authz record may change the record's type. A call that leaves errors fails the check, where
/// Java goes on with the record's type: a failing authz service must not authorize (accepted, P6b-16, the AGENTS.md
/// rule to stay strict where it protects security).
fn authz_service_type(
    service: &str,
    user_id: &str,
    action_id: &str,
    artifact_type: &str,
    name: &str,
) -> Result<Option<String>, Error> {
    let mut parameters = BTreeMap::new();
    parameters.insert("userId".to_string(), Value::string(user_id));
    parameters.insert("authzActionEnumId".to_string(), Value::string(action_id));
    parameters.insert("artifactTypeEnumId".to_string(), Value::string(artifact_type));
    parameters.insert("artifactName".to_string(), Value::string(name));
    let result = crate::service::call::sync_checked(service, &parameters, &Default::default())?;
    Ok(result
        .get("authzTypeEnumId")
        .map(moqui_lib::object::to_plain_string)
        .filter(|text| !text.is_empty()))
}

/// Write the failure row. It is advisory, so a failure here is logged and the decision stands.
fn record_failure(
    facade: &EntityFacade,
    artifact_type: &str,
    name: &str,
    action_id: &str,
    user_id: &str,
    is_deny: bool,
) {
    let outcome = (|| -> Result<(), Error> {
        let mut row = facade.make_value("moqui.security.ArtifactAuthzFailure")?;
        row.set_sequenced_id_primary(facade)?;
        row.set("artifactName", Value::string(name));
        row.set("artifactTypeEnumId", Value::string(artifact_type));
        row.set("authzActionEnumId", Value::string(action_id));
        row.set("userId", Value::string(user_id));
        row.set("failureDate", super::login::now_value());
        row.set("isDeny", Value::string(if is_deny { "Y" } else { "N" }));
        row.create(facade)?;
        Ok(())
    })();
    if let Err(err) = outcome {
        tracing::warn!(error = %err, "could not record an artifact authorization failure");
    }
}

/// Velocity limit. Counts hits per user and artifact and locks the artifact when a rule is exceeded.
fn check_tarpit(artifact_type: &str, name: &str) -> Result<(), Error> {
    let _guard = disable_authz_guard()?;
    let facade = entity_facade()?;
    let checks = tarpit_checks(&facade, artifact_type)?;
    if checks.is_empty() {
        return Ok(());
    }
    let user_id = current_user_id()?.unwrap_or_default();
    let key = format!("{user_id}@{artifact_type}:{name}");
    let now = now_millis();
    let times = ExecutionContext::with_current(|ec| ec.factory().tarpit_times(&key)).unwrap_or_default();
    let mut record = false;
    let mut lock_for = 0_i64;
    let mut longest = 0_i64;
    for check in checks.iter() {
        let matched = if check.name_is_pattern {
            pattern_matches(&check.artifact_name, name).unwrap_or_else(|err| {
                tracing::error!(error = %err, "tarpit record did not evaluate; it does not apply");
                false
            })
        } else {
            check.artifact_name == name
        };
        if !matched {
            continue;
        }
        record = true;
        let window_ms = check.max_hits_duration.saturating_mul(1000);
        longest = longest.max(window_ms);
        let recent = times
            .iter()
            .filter(|time| now - **time >= 0 && now - **time < window_ms)
            .count();
        // The current hit counts too.
        let hits = i64::try_from(recent).unwrap_or(i64::MAX).saturating_add(1);
        if hits > check.max_hits_count && check.tarpit_duration > lock_for {
            lock_for = check.tarpit_duration;
            tracing::warn!(
                user = %user_id,
                artifact = %key,
                "tarpit limit exceeded; locking for {lock_for} seconds"
            );
        }
    }
    if record {
        ExecutionContext::with_current(|ec| {
            ec.factory().record_tarpit_hit(&key, now, longest.max(60_000));
        });
        let locks = facade
            .find("moqui.security.ArtifactTarpitLock")?
            .eq("userId", Value::string(&user_id))
            .eq("artifactName", Value::string(name))
            .eq("artifactTypeEnumId", Value::string(artifact_type))
            .list()?;
        let now_value = super::login::now_value();
        for lock in locks.values() {
            let Some(release) = lock.get("releaseDateTime").filter(|value| !value.is_null()) else {
                continue;
            };
            if moqui_lib::object::compare_obj(release, &now_value).ok() == Some(std::cmp::Ordering::Greater) {
                let seconds = match release {
                    Value::Timestamp(stamp) => (stamp.as_millisecond() - now) / 1000,
                    _ => 0,
                };
                return Err(Error::of_kind(
                    crate::context::ErrorKind::Tarpit,
                    format!(
                        "User {user_id} has accessed {artifact_type} {name} too many times; retry after {seconds} seconds"
                    ),
                ));
            }
        }
    }
    if lock_for > 0 {
        let release = moqui_lib::object::basic_convert(
            &Value::I64(now.saturating_add(lock_for.saturating_mul(1000))),
            "java.sql.Timestamp",
        )
        .value;
        let mut row = facade.make_value("moqui.security.ArtifactTarpitLock")?;
        row.set_sequenced_id_primary(&facade)?;
        row.set("userId", Value::string(&user_id));
        row.set("artifactName", Value::string(name));
        row.set("artifactTypeEnumId", Value::string(artifact_type));
        row.set("releaseDateTime", release);
        row.create(&facade)?;
        ExecutionContext::with_current(|ec| ec.factory().clear_tarpit_hits(&key));
    }
    Ok(())
}

fn artifact_flag(artifact_type: &str, flag: &str) -> bool {
    ExecutionContext::with_current(|ec| ec.factory().artifact_flag(artifact_type, flag)).unwrap_or(false)
}

/// True when `artifact-execution` for this type has `authz-enabled="true"`.
pub(crate) fn authz_enabled_for(artifact_type: &str) -> bool {
    artifact_flag(artifact_type, "authz-enabled")
}

pub(super) fn generation() -> u64 {
    AUTHZ_GENERATION.load(Ordering::Relaxed)
}

/// Refresh generation-bound caches, retaining Java's current-user authorization-check list.
fn with_cache<T>(body: impl FnOnce(&mut AuthzCache) -> T) -> T {
    let now = generation();
    with_session(|session| {
        if session.authz.generation != now {
            session.authz = AuthzCache {
                generation: now,
                // Java retains the current UserInfo's authorization list across entity writes.
                // Login/logout reset it with the rest of the user's state.
                checks: session.authz.checks.take(),
                ..AuthzCache::default()
            };
        }
        body(&mut session.authz)
    })
}

/// What `read` finds in the session's cache, or else what `load` returns, kept by `store` only when the
/// generation is still the one the load began under. A load that overlapped an authorization write is
/// returned but not kept.
fn cached<T: Clone>(
    read: impl FnOnce(&AuthzCache) -> Option<T>,
    load: impl FnOnce() -> Result<T, Error>,
    store: impl FnOnce(&mut AuthzCache, T),
) -> Result<T, Error> {
    let (loaded_at, found) = with_cache(|cache| (cache.generation, read(cache)));
    if let Some(found) = found {
        return Ok(found);
    }
    let value = load()?;
    with_cache(|cache| {
        if cache.generation == loaded_at {
            store(cache, value.clone());
        }
    });
    Ok(value)
}

/// The current user's groups, `ALL_USERS` included, as `ec.user.userGroupIdSet`.
///
/// # Errors
///
/// Returns an error when the memberships cannot be read.
pub(crate) fn current_user_groups() -> Result<Arc<HashSet<String>>, Error> {
    let Some(facade) = ExecutionContext::with_current(|ec| ec.factory().entity()).flatten() else {
        return Ok(Arc::new(HashSet::from(["ALL_USERS".to_string()])));
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    user_groups(&facade)
}

/// The user's groups: `ALL_USERS` and each group with an open membership.
fn user_groups(facade: &EntityFacade) -> Result<Arc<HashSet<String>>, Error> {
    cached(
        |cache| cache.groups.clone(),
        || load_user_groups(facade),
        |cache, groups| cache.groups = Some(groups),
    )
}

fn load_user_groups(facade: &EntityFacade) -> Result<Arc<HashSet<String>>, Error> {
    let mut groups = HashSet::new();
    groups.insert("ALL_USERS".to_string());
    if let Some(user_id) = current_user_id()?.filter(|id| !id.is_empty()) {
        let now = super::login::now_value();
        let rows = facade
            .find("moqui.security.UserGroupMember")?
            .eq("userId", Value::string(&user_id))
            .list()?;
        for row in rows.values() {
            if !membership_open(row, &now) {
                continue;
            }
            let group = text(row, "userGroupId");
            if !group.is_empty() {
                groups.insert(group);
            }
        }
    }
    Ok(Arc::new(groups))
}

fn user_checks(facade: &EntityFacade) -> Result<Arc<Vec<Arc<AuthzCheck>>>, Error> {
    if let Some(checks) = with_session(|session| session.authz.checks.clone()) {
        return Ok(checks);
    }
    let checks = load_user_checks(facade)?;
    with_session(|session| session.authz.checks = Some(checks.clone()));
    Ok(checks)
}

/// One `ArtifactAuthzCheckView` read per group, as Java's `getArtifactAuthzCheckList`.
fn load_user_checks(facade: &EntityFacade) -> Result<Arc<Vec<Arc<AuthzCheck>>>, Error> {
    let groups = user_groups(facade)?;
    let mut ordered: Vec<&String> = groups.iter().collect();
    ordered.sort();
    let mut checks = Vec::new();
    for group in ordered {
        let rows = facade
            .find("moqui.security.ArtifactAuthzCheckView")?
            .eq("userGroupId", Value::string(group))
            .list()?;
        for row in rows.values() {
            let authz_id = text(row, "artifactAuthzId");
            let artifact_name = text(row, "artifactName");
            let artifact_type = text(row, "artifactTypeEnumId");
            if authz_id.is_empty() || artifact_name.is_empty() || artifact_type.is_empty() {
                continue;
            }
            checks.push(Arc::new(AuthzCheck {
                artifact_authz_id: authz_id,
                authz_type: text(row, "authzTypeEnumId"),
                authz_action: text(row, "authzActionEnumId"),
                authz_service_name: text(row, "authzServiceName"),
                artifact_name,
                artifact_type,
                name_is_pattern: text(row, "nameIsPattern") == "Y",
                inherit_authz: text(row, "inheritAuthz") == "Y",
                filter_map: text(row, "filterMap"),
            }));
        }
    }
    Ok(Arc::new(checks))
}

fn tarpit_checks(facade: &EntityFacade, artifact_type: &str) -> Result<Arc<Vec<TarpitCheck>>, Error> {
    cached(
        |cache| cache.tarpit.get(artifact_type).cloned(),
        || load_tarpit_checks(facade, artifact_type),
        |cache, checks| {
            cache.tarpit.insert(artifact_type.to_string(), checks);
        },
    )
}

/// One `ArtifactTarpitCheckView` read per group for the type, as Java's `getArtifactTarpitCheckList`.
fn load_tarpit_checks(facade: &EntityFacade, artifact_type: &str) -> Result<Arc<Vec<TarpitCheck>>, Error> {
    let groups = user_groups(facade)?;
    let mut ordered: Vec<&String> = groups.iter().collect();
    ordered.sort();
    let mut checks = Vec::new();
    for group in ordered {
        let rows = facade
            .find("moqui.security.ArtifactTarpitCheckView")?
            .eq("userGroupId", Value::string(group))
            .eq("artifactTypeEnumId", Value::string(artifact_type))
            .list()?;
        for row in rows.values() {
            checks.push(TarpitCheck {
                artifact_name: text(row, "artifactName"),
                name_is_pattern: text(row, "nameIsPattern") == "Y",
                max_hits_count: number(row, "maxHitsCount"),
                max_hits_duration: number(row, "maxHitsDuration"),
                tarpit_duration: number(row, "tarpitDuration"),
            });
        }
    }
    Ok(Arc::new(checks))
}

fn membership_open(row: &EntityValue, now: &Value) -> bool {
    let Some(from) = row.get("fromDate") else {
        return false;
    };
    if from.is_null() {
        return false;
    }
    let Ok(order) = moqui_lib::object::compare_obj(from, now) else {
        return false;
    };
    if order == std::cmp::Ordering::Greater {
        return false;
    }
    match row.get("thruDate") {
        None | Some(Value::Null) => true,
        Some(thru) => moqui_lib::object::compare_obj(thru, now).ok() == Some(std::cmp::Ordering::Greater),
    }
}

fn number(row: &EntityValue, field: &str) -> i64 {
    basic_long(row.get(field)).unwrap_or_default()
}

fn basic_long(value: Option<&Value>) -> Option<i64> {
    match moqui_lib::object::basic_convert(value?, "Long").value {
        Value::I64(number) => Some(number),
        _ => None,
    }
}

fn text(row: &EntityValue, field: &str) -> String {
    match row.get(field) {
        Some(Value::String(value)) => value.clone(),
        Some(other) if !other.is_null() => moqui_lib::object::to_plain_string(other),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use moqui_lib::object::Value;

    use super::super::session;
    use crate::context::{ExecutionContext, ExecutionContextFactory};
    use crate::entity::{EntityFacade, EntityValue};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn enter_marks_the_thread_until_the_guard_drops() {
        let context = ExecutionContextFactory::test_context();
        assert!(!super::context_entered());
        assert!(!ExecutionContext::is_entered());
        let outer = context.enter();
        assert!(super::context_entered());
        assert!(ExecutionContext::is_entered());
        let inner = context.enter();
        assert!(super::context_entered());
        drop(inner);
        assert!(super::context_entered());
        drop(outer);
        assert!(!super::context_entered());
        assert!(!ExecutionContext::is_entered());
    }

    /// A booted factory with the real conf, so artifact authorization is on, over an in-memory database.
    struct World {
        _runtime: TempDir,
        _factory: std::sync::Arc<ExecutionContextFactory>,
        context: ExecutionContext,
        facade: EntityFacade,
    }

    fn world() -> World {
        let runtime = temp_dir("authz-runtime");
        let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
        let factory = {
            let _lock = crate::user::session::cookie_property_lock();
            ExecutionContextFactory::boot_with_framework(runtime.path(), &framework).expect("boot")
        };
        let facade = crate::testing::open();
        factory.set_entity(facade.clone());
        let context = factory.new_context();
        World {
            _runtime: runtime,
            _factory: factory,
            context,
            facade,
        }
    }

    /// An authz record for `artifact_name`: the group member, the authz row, and the type and action.
    fn grant(world: &World, id: &str, artifact_type: &str, artifact_name: &str, authz_type: &str, inherit: &str) {
        grant_member(
            world,
            id,
            artifact_type,
            artifact_name,
            authz_type,
            &[("nameIsPattern", "N"), ("inheritAuthz", inherit)],
        );
    }

    /// [`grant`] with the group member's other fields, such as `filterMap` or `nameIsPattern`.
    fn grant_member(
        world: &World,
        id: &str,
        artifact_type: &str,
        artifact_name: &str,
        authz_type: &str,
        member: &[(&str, &str)],
    ) {
        let _guard = world.context.disable_authz();
        let mut fields = vec![
            ("artifactGroupId", id),
            ("artifactName", artifact_name),
            ("artifactTypeEnumId", artifact_type),
        ];
        fields.extend_from_slice(member);
        create(&world.facade, "moqui.security.ArtifactGroupMember", &fields);
        create(
            &world.facade,
            "moqui.security.ArtifactAuthz",
            &[
                ("artifactAuthzId", id),
                ("userGroupId", "ALL_USERS"),
                ("artifactGroupId", id),
                ("authzTypeEnumId", authz_type),
                ("authzActionEnumId", "AUTHZA_ALL"),
            ],
        );
    }

    fn create(facade: &EntityFacade, name: &str, fields: &[(&str, &str)]) {
        let mut value = facade.make_value(name).unwrap();
        for (field, text) in fields {
            value.set(*field, Value::string(*text));
        }
        value.create(facade).unwrap();
    }

    const ENTITY: &str = "moqui.test.TestEntity";

    #[test]
    fn a_second_context_entered_on_the_thread_is_the_current_one_until_it_drops() {
        let outer = ExecutionContextFactory::test_context();
        let inner = ExecutionContextFactory::test_context();
        let _outer = outer.enter();
        outer.message().add_message("outer");
        {
            let _inner = inner.enter();
            inner.message().add_message("inner");
            let seen = ExecutionContext::with_current(|ec| ec.message().messages().to_vec()).unwrap();
            assert_eq!(seen, ["inner"], "the innermost context answers");
        }
        let seen = ExecutionContext::with_current(|ec| ec.message().messages().to_vec()).unwrap();
        assert_eq!(seen, ["outer"], "the outer context is back");
    }

    #[test]
    fn groups_loaded_while_the_generation_moves_are_not_kept() {
        let context = ExecutionContextFactory::test_context();
        let _entered = context.enter();
        let loaded = super::cached(
            |cache| cache.groups.clone(),
            || {
                // An authorization write lands while the groups are read.
                super::note_entity_write("moqui.security.ArtifactAuthz");
                Ok(std::sync::Arc::new(std::collections::HashSet::new()))
            },
            |cache, groups| cache.groups = Some(groups),
        )
        .unwrap();
        assert!(loaded.is_empty());
        assert!(
            super::with_cache(|cache| cache.groups.is_none()),
            "groups read under the old generation are not stored as current"
        );
    }

    #[test]
    fn an_authorization_write_moves_the_generation_again_when_it_commits() {
        let world = world();
        let _entered = world.context.enter();
        let tx = world.facade.transactions_for("moqui.security.ArtifactAuthz").unwrap();
        assert!(tx.begin(None).unwrap());
        grant(&world, "GEN_COMMIT", "AT_ENTITY", ENTITY, "AUTHZT_ALLOW", "N");
        // Membership caches still invalidate on commit, unlike Java's user authorization list.
        super::with_cache(|cache| cache.groups = Some(std::sync::Arc::new(std::collections::HashSet::new())));
        let written = super::generation();
        tx.commit().unwrap();
        assert!(super::generation() > written);
        assert!(
            super::with_cache(|cache| cache.groups.is_none()),
            "the commit drops what was read before it"
        );
    }

    #[test]
    fn an_authorization_write_moves_the_generation_again_when_it_rolls_back() {
        let world = world();
        let _entered = world.context.enter();
        let tx = world.facade.transactions_for("moqui.security.ArtifactAuthz").unwrap();
        assert!(tx.begin(None).unwrap());
        grant(&world, "GEN_ROLLBACK", "AT_ENTITY", ENTITY, "AUTHZT_ALLOW", "N");
        // The writer reloads and reads its own row, which the rollback then undoes.
        super::with_cache(|cache| cache.groups = Some(std::sync::Arc::new(std::collections::HashSet::new())));
        let written = super::generation();
        tx.rollback().unwrap();
        assert!(super::generation() > written);
        assert!(
            super::with_cache(|cache| cache.groups.is_none()),
            "the rollback drops what was read before it"
        );
    }

    #[test]
    fn reads_and_writes_need_an_allow_record() {
        let world = world();
        let _entered = world.context.enter();
        session::set_user("nobody", "nobody");
        let row = test_row(&world.facade);
        let err = row.create(&world.facade).expect_err("create without an allow");
        assert_eq!(
            crate::context::ErrorKind::of(&err),
            crate::context::ErrorKind::Unauthorized,
            "{err}"
        );
        let err = world
            .facade
            .find(ENTITY)
            .unwrap()
            .list()
            .expect_err("read without an allow");
        assert!(err.to_string().contains("not authorized"), "{err}");
        assert!(world.facade.find(ENTITY).unwrap().count().is_err());

        grant(&world, "READ_TEST", "AT_ENTITY", ENTITY, "AUTHZT_ALLOW", "N");
        session::set_user("nobody", "nobody");
        world.facade.find(ENTITY).unwrap().list().expect("read after the allow");
        row.create(&world.facade).expect("the allow is for all actions");

        let guard = world.context.disable_authz();
        let found = world
            .facade
            .find(ENTITY)
            .unwrap()
            .eq("testId", Value::string("T-DENY"))
            .list()
            .unwrap();
        drop(guard);
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn an_iterator_is_authorized_as_a_list_is() {
        let world = world();
        let _entered = world.context.enter();
        session::set_user("nobody", "nobody");
        let err = world
            .facade
            .find(ENTITY)
            .unwrap()
            .iterator()
            .err()
            .expect("iterate without an allow");
        assert!(err.to_string().contains("not authorized"), "{err}");
        world
            .facade
            .find(ENTITY)
            .unwrap()
            .disable_authz()
            .iterator()
            .expect("a find that disables authorization")
            .close()
            .unwrap();
    }

    #[test]
    fn a_dynamic_view_needs_view_authorization_on_each_member() {
        let world = world();
        let _entered = world.context.enter();
        session::set_user("nobody", "nobody");
        let text = |text: &str| Value::string(text);
        let map = |pairs: Vec<(&str, Value)>| {
            Value::Map(pairs.into_iter().map(|(key, value)| (key.to_string(), value)).collect())
        };
        let members = Value::List(vec![
            map(vec![
                ("entity-alias", text("FOO")),
                ("entity-name", text("moqui.test.Foo")),
            ]),
            map(vec![
                ("entity-alias", text("BAR")),
                ("entity-name", text("moqui.test.Bar")),
                ("join-from-alias", text("FOO")),
                ("key-map", map(vec![("fooId", text("fooId"))])),
            ]),
        ]);
        let parameters: std::collections::BTreeMap<String, Value> = [
            ("member-entity".to_string(), members),
            ("alias-all".to_string(), map(vec![("entity-alias", text("FOO"))])),
        ]
        .into_iter()
        .collect();
        let view = || world.facade.dynamic_view("dynamic.FooBarAuthz", &parameters);
        let err = view().expect_err("no member is allowed");
        assert!(err.to_string().contains("not authorized"), "{err}");
        grant(&world, "DV_FOO", "AT_ENTITY", "moqui.test.Foo", "AUTHZT_ALLOW", "N");
        session::set_user("nobody", "nobody");
        let err = view().expect_err("the joined member is not allowed");
        assert!(err.to_string().contains("moqui.test.Bar"), "{err}");
        grant(&world, "DV_BAR", "AT_ENTITY", "moqui.test.Bar", "AUTHZT_ALLOW", "N");
        session::set_user("nobody", "nobody");
        assert!(view().expect("both members are allowed").is_empty());
    }

    #[test]
    fn deny_beats_allow_and_always_beats_deny() {
        let world = world();
        let _entered = world.context.enter();
        session::set_user("nobody", "nobody");
        grant(&world, "ALLOW_TEST", "AT_ENTITY", ENTITY, "AUTHZT_ALLOW", "N");
        grant(&world, "DENY_TEST", "AT_ENTITY", ENTITY, "AUTHZT_DENY", "N");
        let err = world.facade.find(ENTITY).unwrap().list().expect_err("deny wins");
        assert!(err.to_string().contains("deny record"), "{err}");
        grant(&world, "ALWAYS_TEST", "AT_ENTITY", ENTITY, "AUTHZT_ALWAYS", "N");
        session::set_user("nobody", "nobody");
        world.facade.find(ENTITY).unwrap().list().expect("always wins");
    }

    /// A filterMap that is not a valid expression, so it cannot be evaluated.
    const BAD_FILTER: &str = "[ this is not an expression";

    fn filter_parameters() -> std::collections::BTreeMap<String, Value> {
        [("orderId".to_string(), Value::string("O1"))].into_iter().collect()
    }

    #[test]
    fn a_deny_whose_filter_map_fails_denies_despite_an_allow() {
        let world = world();
        let _entered = world.context.enter();
        session::set_user("nobody", "nobody");
        let service = "test.Filtered#Row";
        grant(&world, "FILTER_ALLOW", "AT_SERVICE", service, "AUTHZT_ALLOW", "N");
        let parameters = filter_parameters();
        world
            .context
            .push_checked("AT_SERVICE", service, "all", true, false, Some(&parameters))
            .expect("allowed before the deny");
        world.context.pop().unwrap();
        grant_member(
            &world,
            "FILTER_DENY",
            "AT_SERVICE",
            service,
            "AUTHZT_DENY",
            &[("filterMap", BAD_FILTER)],
        );
        session::set_user("nobody", "nobody");
        let err = world
            .context
            .push_checked("AT_SERVICE", service, "all", true, false, Some(&parameters))
            .expect_err("the unevaluable deny applies");
        assert!(err.to_string().contains("deny record"), "{err}");
    }

    #[test]
    fn a_deny_whose_name_pattern_is_invalid_denies() {
        let world = world();
        let _entered = world.context.enter();
        session::set_user("nobody", "nobody");
        let service = "test.Pattern#Row";
        grant(&world, "PATTERN_ALLOW", "AT_SERVICE", service, "AUTHZT_ALLOW", "N");
        grant_member(
            &world,
            "PATTERN_DENY",
            "AT_SERVICE",
            "test.(",
            "AUTHZT_DENY",
            &[("nameIsPattern", "Y")],
        );
        let err = world
            .context
            .push_checked("AT_SERVICE", service, "all", true, false, None)
            .expect_err("the invalid deny pattern applies");
        assert!(err.to_string().contains("deny record"), "{err}");
    }

    #[test]
    fn an_allow_whose_filter_map_fails_does_not_allow() {
        let world = world();
        let _entered = world.context.enter();
        session::set_user("nobody", "nobody");
        let service = "test.FilteredAllow#Row";
        grant_member(
            &world,
            "FILTER_BAD_ALLOW",
            "AT_SERVICE",
            service,
            "AUTHZT_ALLOW",
            &[("filterMap", BAD_FILTER)],
        );
        let parameters = filter_parameters();
        let err = world
            .context
            .push_checked("AT_SERVICE", service, "all", true, false, Some(&parameters))
            .expect_err("the unevaluable allow does not apply");
        assert!(err.to_string().contains("not authorized"), "{err}");
        grant_member(
            &world,
            "FILTER_GOOD_ALLOW",
            "AT_SERVICE",
            service,
            "AUTHZT_ALLOW",
            &[("filterMap", "{orderId: 'O1'}")],
        );
        session::set_user("nobody", "nobody");
        world
            .context
            .push_checked("AT_SERVICE", service, "all", true, false, Some(&parameters))
            .expect("a matching filterMap allows");
        world.context.pop().unwrap();
    }

    #[test]
    fn an_inheritable_service_allow_covers_the_entity_operations_it_makes() {
        let world = world();
        let _entered = world.context.enter();
        session::set_user("nobody", "nobody");
        grant(&world, "SVC_TEST", "AT_SERVICE", "test.Make#Row", "AUTHZT_ALLOW", "Y");
        // Without the service frame the entity is not allowed.
        assert!(test_row(&world.facade).create(&world.facade).is_err());
        world
            .context
            .push_checked("AT_SERVICE", "test.Make#Row", "all", true, false, None)
            .expect("the service is allowed");
        test_row(&world.facade)
            .create(&world.facade)
            .expect("the entity write inherits the service's allow");
        world.facade.find(ENTITY).unwrap().list().expect("so does a read");
        world.context.pop().unwrap();
        assert!(test_row(&world.facade).create(&world.facade).is_err());

        // The same allow without inheritance covers only the service.
        grant(
            &world,
            "NOINH_TEST",
            "AT_SERVICE",
            "test.Other#Row",
            "AUTHZT_ALLOW",
            "N",
        );
        session::set_user("nobody", "nobody");
        world
            .context
            .push_checked("AT_SERVICE", "test.Other#Row", "all", true, false, None)
            .expect("the service is allowed");
        assert!(world.facade.find(ENTITY).unwrap().list().is_err());
        world.context.pop().unwrap();
        // A deny below the stack keeps a plain allow from applying.
    }

    #[test]
    fn a_deny_on_the_stack_stops_a_plain_allow_above_it() {
        let world = world();
        let _entered = world.context.enter();
        session::set_user("nobody", "nobody");
        grant(&world, "DENYSVC", "AT_SERVICE", "test.Denied#Row", "AUTHZT_DENY", "N");
        // A denied frame is not pushed, so the deny is seen as the explicit-deny error.
        let err = world
            .context
            .push_checked("AT_SERVICE", "test.Denied#Row", "all", true, false, None)
            .expect_err("denied");
        assert!(err.to_string().contains("deny record"), "{err}");
        // Not requiring authz pushes it anyway, carrying the deny.
        world
            .context
            .push_checked("AT_SERVICE", "test.Denied#Row", "all", false, false, None)
            .expect("pushed without needing authz");
        grant(&world, "ENT_ALLOW", "AT_ENTITY", ENTITY, "AUTHZT_ALLOW", "N");
        session::set_user("nobody", "nobody");
        assert!(
            world.facade.find(ENTITY).unwrap().list().is_err(),
            "the allow does not apply under a deny frame"
        );
        world.context.pop().unwrap();
        world
            .facade
            .find(ENTITY)
            .unwrap()
            .list()
            .expect("allowed once the deny frame is gone");
    }

    #[test]
    fn authorization_checks_stay_cached_until_the_user_changes_as_in_java() {
        let world = world();
        let _entered = world.context.enter();
        session::set_user("nobody", "nobody");
        let original = {
            let _guard = world.context.disable_authz();
            super::user_checks(&world.facade).unwrap()
        };
        assert!(original.is_empty());
        grant(&world, "LATE_DENY", "AT_SERVICE", "test.Cached#Row", "AUTHZT_DENY", "N");
        let retained = {
            let _guard = world.context.disable_authz();
            super::user_checks(&world.facade).unwrap()
        };
        assert!(std::sync::Arc::ptr_eq(&original, &retained), "seed writes must not reload the user's checks");
        session::set_user("other", "other");
        let err = world.context
            .push_checked("AT_SERVICE", "test.Cached#Row", "all", true, false, None)
            .expect_err("a new user loads and enforces the deny");
        assert!(err.to_string().contains("deny record"), "{err}");
    }

    #[test]
    fn a_new_allow_is_seen_after_refreshing_the_user() {
        let world = world();
        let _entered = world.context.enter();
        session::set_user("nobody", "nobody");
        assert!(world.facade.find(ENTITY).unwrap().list().is_err());
        grant(&world, "LATE_TEST", "AT_ENTITY", ENTITY, "AUTHZT_ALLOW", "N");
        assert!(world.facade.find(ENTITY).unwrap().list().is_err(), "the current user's checks stay cached");
        session::set_user("nobody", "nobody");
        world
            .facade
            .find(ENTITY)
            .unwrap()
            .list()
            .expect("the cached checks were reloaded");
    }

    #[test]
    fn a_failed_check_leaves_an_authz_failure_row() {
        let world = world();
        let _entered = world.context.enter();
        session::set_user("nobody", "nobody");
        assert!(world.facade.find(ENTITY).unwrap().list().is_err());
        let _guard = world.context.disable_authz();
        let failures = world
            .facade
            .find("moqui.security.ArtifactAuthzFailure")
            .unwrap()
            .eq("artifactName", Value::string(ENTITY))
            .list()
            .unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures.values()[0].get("isDeny").and_then(Value::as_str), Some("N"));
    }

    #[test]
    fn a_service_over_its_hit_limit_is_locked() {
        let world = world();
        let _entered = world.context.enter();
        session::set_user("nobody", "nobody");
        grant(&world, "TARPIT_SVC", "AT_SERVICE", "test.Busy#Row", "AUTHZT_ALLOW", "N");
        {
            let _guard = world.context.disable_authz();
            create(
                &world.facade,
                "moqui.security.ArtifactTarpit",
                &[
                    ("userGroupId", "ALL_USERS"),
                    ("artifactGroupId", "TARPIT_SVC"),
                    ("maxHitsCount", "2"),
                    ("maxHitsDuration", "60"),
                    ("tarpitDuration", "120"),
                ],
            );
        }
        for _ in 0..2 {
            world
                .context
                .push_checked("AT_SERVICE", "test.Busy#Row", "all", true, true, None)
                .expect("under the limit");
            world.context.pop().unwrap();
        }
        let err = world
            .context
            .push_checked("AT_SERVICE", "test.Busy#Row", "all", true, true, None);
        assert!(err.is_ok(), "the third hit exceeds the limit and sets the lock");
        world.context.pop().unwrap();
        let err = world
            .context
            .push_checked("AT_SERVICE", "test.Busy#Row", "all", true, true, None)
            .expect_err("locked");
        assert_eq!(err.kind(), crate::context::ErrorKind::Tarpit, "{err}");
    }

    #[test]
    fn login_works_with_authorization_on_and_no_entity_allows() {
        let world = world();
        let _entered = world.context.enter();
        {
            let _guard = world.context.disable_authz();
            let mut user = world.facade.make_value("moqui.security.UserAccount").unwrap();
            user.set("userId", Value::string("U1"));
            user.set("username", Value::string("ada"));
            user.set(
                "currentPassword",
                Value::string(super::super::password::hash_password("Secret1!").unwrap()),
            );
            user.set("passwordHashType", Value::string("argon2"));
            user.set("successiveFailedLogins", Value::I64(2));
            user.create(&world.facade).unwrap();
        }
        // Authorization is on and the caller is anonymous: the login must still read and update.
        super::super::login_user("ada", "Secret1!").expect("login");
        assert_eq!(session::current_username().unwrap().as_deref(), Some("ada"));
        let _guard = world.context.disable_authz();
        let user = world
            .facade
            .find("moqui.security.UserAccount")
            .unwrap()
            .eq("userId", Value::string("U1"))
            .one()
            .unwrap()
            .unwrap();
        assert_eq!(
            user.get("successiveFailedLogins")
                .map(moqui_lib::object::to_plain_string)
                .as_deref(),
            Some("0")
        );
    }

    fn test_row(facade: &EntityFacade) -> EntityValue {
        let mut row = facade.make_value(ENTITY).unwrap();
        row.set("testId", Value::string("T-DENY"));
        row
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_dir(label: &str) -> TempDir {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("moqui-{label}-{}-{nanos}-{seq}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }
}
