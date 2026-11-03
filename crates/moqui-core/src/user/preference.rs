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

//! User preferences, group membership, and permissions, as `UserFacadeImpl` has them.
//!
//! A preference is a process property first, then the user's `UserPreference`, then the
//! `UserGroupPreference` of a group the user is in. `ALL_USERS` is every user's group, so its
//! rows are the defaults.

use std::collections::{BTreeMap, BTreeSet};

use moqui_db::sql::Condition;
use moqui_lib::object::{compare_obj, to_plain_string, Value};

use super::login::{entity_facade, now_value};
use super::session;
use crate::context::{Error, ExecutionContext};
use crate::entity::EntityValue;
use crate::transaction::TransactionAttribute;

const USER_PREFERENCE: &str = "moqui.security.UserPreference";
const GROUP_PREFERENCE: &str = "moqui.security.UserGroupPreference";
const GROUP_MEMBER: &str = "moqui.security.UserGroupMember";
const GROUP_PERMISSION: &str = "moqui.security.UserGroupPermission";
const ALL_USERS: &str = "ALL_USERS";

/// The preference for the logged-in user, or the defaults when nobody is logged in.
///
/// # Errors
///
/// Returns an error when the preference rows cannot be read.
pub fn get_preference(key: &str) -> Result<Option<String>, Error> {
    let user_id = session::current_user_id()?;
    get_preference_for(key, user_id.as_deref())
}

/// [`get_preference`] for `user_id`. A process property named `key`, or `key` with `.` as `_`,
/// overrides the database as in Java.
///
/// # Errors
///
/// Returns an error when the preference rows cannot be read.
pub fn get_preference_for(key: &str, user_id: Option<&str>) -> Result<Option<String>, Error> {
    if key.is_empty() {
        return Ok(None);
    }
    if let Some(value) = property(key) {
        return Ok(Some(value));
    }
    let _authz = without_authz();
    let facade = entity_facade()?;
    if let Some(user_id) = user_id.filter(|id| !id.is_empty()) {
        let row = facade
            .find(USER_PREFERENCE)?
            .eq("userId", Value::string(user_id))
            .eq("preferenceKey", Value::string(key))
            .one()?;
        if let Some(row) = row {
            return Ok(text(&row, "preferenceValue"));
        }
    }
    let groups = group_values(user_id)?;
    let rows = facade
        .find(GROUP_PREFERENCE)?
        .condition(Condition::is_in("userGroupId", groups))
        .eq("preferenceKey", Value::string(key))
        .order_by("groupPriority", false)
        .order_by("userGroupId", true)
        .list()?;
    Ok(rows.get(0).and_then(|row| text(row, "preferenceValue")))
}

/// Every preference the logged-in user sees: group values, with the user's own rows over them.
/// An empty value is left out. The caller filters by key, as Java's `keyRegexp` does.
///
/// # Errors
///
/// Returns an error when the preference rows cannot be read.
pub fn get_preferences() -> Result<BTreeMap<String, String>, Error> {
    let user_id = session::current_user_id()?;
    let _authz = without_authz();
    let facade = entity_facade()?;
    let mut out = BTreeMap::new();
    // Reverse of the single-key order, so the value that wins there is written last here.
    let groups = facade
        .find(GROUP_PREFERENCE)?
        .condition(Condition::is_in("userGroupId", group_values(user_id.as_deref())?))
        .order_by("groupPriority", true)
        .order_by("userGroupId", false)
        .list()?;
    let own = match user_id.as_deref().filter(|id| !id.is_empty()) {
        Some(user_id) => facade
            .find(USER_PREFERENCE)?
            .eq("userId", Value::string(user_id))
            .list()?
            .values()
            .to_vec(),
        None => Vec::new(),
    };
    for row in groups.iter().chain(own.iter()) {
        let (Some(key), Some(value)) = (text(row, "preferenceKey"), text(row, "preferenceValue")) else {
            continue;
        };
        if !value.is_empty() {
            out.insert(key, value);
        }
    }
    Ok(out)
}

/// Create or update the logged-in user's `UserPreference`.
///
/// A failed write is rolled back and logged, and is not the caller's error, as in Java.
///
/// # Errors
///
/// Returns an error when nobody is logged in, or when `key` is an appearance preference the user's Quasar theme does
/// not let the user change (`STRT_APPEARANCE_POLICY`, docs/ui/Qvt-Client.md).
pub fn set_preference(key: &str, value: Option<&str>) -> Result<(), Error> {
    let Some(user_id) = session::current_user_id()?.filter(|id| !id.is_empty()) else {
        return Err(Error::new(format!(
            "Cannot set preference with key {key}, no user logged in."
        )));
    };
    if crate::screen::sri::appearance::is_appearance_key(key) {
        let policy = crate::screen::sri::appearance::appearance_policy_for_user(Some(&user_id));
        if let Some(reason) = policy.denial(key) {
            return Err(Error::new(format!("Cannot set preference {key}: {reason}")));
        }
    }
    let _authz = without_authz();
    let facade = entity_facade()?;
    let mut row = facade.make_value(USER_PREFERENCE)?;
    row.set("userId", Value::string(&user_id));
    row.set("preferenceKey", Value::string(key));
    row.set("preferenceValue", value.map_or(Value::Null, Value::string));
    let tx = facade.transactions_for(USER_PREFERENCE)?;
    let written = tx.run(TransactionAttribute::UseOrBegin, None, || row.create_or_update(&facade));
    match written {
        Err(err) => tracing::error!(error = %err, user_id = %user_id, key = %key, "Error saving UserPreference"),
        // the response of a web request tells the page, which applies what the shell reads (moqui.applyPreference)
        Ok(_) => crate::web::note_changed_preference(key, value),
    }
    Ok(())
}

/// `ALL_USERS` and each group `user_id` has an open membership in.
///
/// # Errors
///
/// Returns an error when the membership rows cannot be read.
pub fn user_group_ids(user_id: Option<&str>) -> Result<BTreeSet<String>, Error> {
    let mut groups = BTreeSet::from([ALL_USERS.to_string()]);
    let Some(user_id) = user_id.filter(|id| !id.is_empty()) else {
        return Ok(groups);
    };
    let _authz = without_authz();
    let facade = entity_facade()?;
    let now = now_value();
    let rows = facade.find(GROUP_MEMBER)?.eq("userId", Value::string(user_id)).list()?;
    for row in rows.iter().filter(|row| date_open(row, "fromDate", "thruDate", &now)) {
        if let Some(group) = text(row, "userGroupId").filter(|id| !id.is_empty()) {
            groups.insert(group);
        }
    }
    Ok(groups)
}

/// True when the logged-in user is in `user_group_id`. Everyone is in `ALL_USERS`.
///
/// # Errors
///
/// Returns an error when the membership rows cannot be read.
pub fn is_in_group(user_group_id: &str) -> Result<bool, Error> {
    if user_group_id == ALL_USERS {
        return Ok(true);
    }
    let Some(user_id) = session::current_user_id()? else {
        return Ok(false);
    };
    Ok(user_group_ids(Some(&user_id))?.contains(user_group_id))
}

/// True when a group the logged-in user is in grants `user_permission_id`, both rows in date.
///
/// Java reads the `UserPermissionCheck` view. This reads its two member entities.
///
/// # Errors
///
/// Returns an error when the rows cannot be read.
pub fn has_permission(user_permission_id: &str) -> Result<bool, Error> {
    let Some(user_id) = session::current_user_id()?.filter(|id| !id.is_empty()) else {
        return Ok(false);
    };
    user_has_permission(&user_id, user_permission_id)
}

/// [`has_permission`] for `user_id`, who need not be logged in: internal work for another user (a chat agent that runs as
/// a message's sender).
///
/// # Errors
///
/// Returns an error when the rows cannot be read.
pub fn user_has_permission(user_id: &str, user_permission_id: &str) -> Result<bool, Error> {
    if !user_granted(user_id, user_permission_id)? {
        return Ok(false);
    }
    // an external user never holds the agent permissions, whatever group grants them
    Ok(!EXTERNAL_WITHHELD.contains(&user_permission_id) || !is_external_user(user_id))
}

/// The framework permission of a user from outside the organization that runs the server: a client contact, a vendor's
/// or partner's user. Such a user reaches no room bound to a record (`org.moqui.impl.ChatServices`: not as a member
/// either, and `check#RecordVisible` answers false for them whatever the app's hook says), no public room but as a
/// member, no record folder of a records drive (`FileServices.get#DriveAccess`, over the screens, WebDAV and SFTP),
/// sees no other user's presence, and never holds the agent permissions ([`EXTERNAL_WITHHELD`]: no Assist, chat agents,
/// AI tasks, A2A, MCP or the LLM gateway). The framework grants it to no group; an app grants it to its group of outside
/// users (IronStone BOSS: `BOSS_PROJECT_CLIENT`).
pub const EXTERNAL_USER: &str = "EXTERNAL_USER";

/// The permissions a user with [`EXTERNAL_USER`] never holds.
pub const EXTERNAL_WITHHELD: &[&str] = &[
    "AgentUse",
    "AgentAdmin",
    "AgentAudit",
    "LlmGatewayUse",
    "A2aServerUse",
    "McpServerUse",
];

/// Whether `user_id` holds [`EXTERNAL_USER`]. A failure to read the rows counts as external (logged): what an external
/// user is refused, an internal user can do without for the moment.
pub fn is_external_user(user_id: &str) -> bool {
    if user_id.is_empty() {
        return false;
    }
    match user_granted(user_id, EXTERNAL_USER) {
        Ok(external) => external,
        Err(err) => {
            tracing::error!("Could not read whether user {user_id} is external, treating them as external: {err}");
            true
        }
    }
}

/// Whether the logged-in user holds [`EXTERNAL_USER`]; false without a user (system work).
pub fn current_user_external() -> bool {
    match session::current_user_id() {
        Ok(Some(user_id)) => is_external_user(&user_id),
        Ok(None) => false,
        Err(err) => {
            tracing::error!("Could not read the current user, treating them as external: {err}");
            true
        }
    }
}

/// Whether a group `user_id` is in grants `user_permission_id`, both rows in date.
fn user_granted(user_id: &str, user_permission_id: &str) -> Result<bool, Error> {
    let _authz = without_authz();
    let facade = entity_facade()?;
    let now = now_value();
    let members = facade.find(GROUP_MEMBER)?.eq("userId", Value::string(user_id)).list()?;
    for member in members
        .iter()
        .filter(|row| date_open(row, "fromDate", "thruDate", &now))
    {
        let Some(group) = text(member, "userGroupId") else {
            continue;
        };
        let grants = facade
            .find(GROUP_PERMISSION)?
            .eq("userGroupId", Value::string(group))
            .eq("userPermissionId", Value::string(user_permission_id))
            .list()?;
        if grants.iter().any(|row| date_open(row, "fromDate", "thruDate", &now)) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn group_values(user_id: Option<&str>) -> Result<Vec<Value>, Error> {
    Ok(user_group_ids(user_id)?.into_iter().map(Value::String).collect())
}

/// `EntityList.filterByDate`: a missing from date has started and a missing thru date is open. Both ends are inclusive,
/// so a row whose thru date is now is still open.
fn date_open(row: &EntityValue, from: &str, thru: &str, now: &Value) -> bool {
    let started = match row.get(from) {
        None | Some(Value::Null) => true,
        Some(from) => compare_obj(from, now).is_ok_and(|order| order.is_le()),
    };
    let open = match row.get(thru) {
        None | Some(Value::Null) => true,
        Some(thru) => compare_obj(thru, now).is_ok_and(|order| order.is_ge()),
    };
    started && open
}

fn property(key: &str) -> Option<String> {
    let value = moqui_lib::env::get_prop_or_env(key);
    if !value.is_empty() {
        return Some(value);
    }
    let underscore = key.replace('.', "_");
    if underscore == key {
        return None;
    }
    Some(moqui_lib::env::get_prop_or_env(&underscore)).filter(|value| !value.is_empty())
}

fn text(row: &EntityValue, field: &str) -> Option<String> {
    match row.get(field) {
        None | Some(Value::Null) => None,
        Some(value) => Some(to_plain_string(value)),
    }
}

fn without_authz() -> Option<crate::context::AuthzGuard> {
    ExecutionContext::with_current(|ctx| ctx.disable_authz())
}

#[cfg(test)]
#[path = "preference_tests.rs"]
mod tests;
