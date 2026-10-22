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

//! The two `UserSecurityChecks` calls that `Security.eecas.xml` makes, as Rust: only a member of a
//! privileged group may change that group's membership, and only a holder of a sealed permission
//! may grant it. Both allow the write when authorization is disabled, as a data load runs.

use std::cell::Cell;
use std::collections::BTreeSet;

use moqui_lib::error::Error;
use moqui_lib::object::Value;

use super::eca::to_lib;
use super::util::now_stamp;
use crate::context::ExecutionContext;
use crate::entity::{EntityFacade, EntityValue};

const DEFAULT_PRIVILEGED_GROUPS: &str = "ADMIN,ADMIN_ADV";
const DEFAULT_SEALED_PERMISSIONS: &str =
    "GROOVY_SHELL_WEB,SQL_RUNNER_WEB,SSH_SHELL,SERVICE_LOAD_RUNNER,ADMIN_LOGIN_AS,ADMIN_PASSWORD,SearchRemote,DashboardRemote,REST_SCHEMA";

/// How many times `UserSecurityChecks` ran on this thread, for a load report to show the ECAs fired.
pub fn security_checks_run() -> u64 {
    SECURITY_CHECKS.with(Cell::get)
}

thread_local! {
    static SECURITY_CHECKS: Cell<u64> = const { Cell::new(0) };
}

#[cfg(test)]
thread_local! {
    /// Whether [`assert_sealed_permission_grant`] ran on this thread, for the tests that check the SECA fires.
    static SEALED_ENTERED: Cell<bool> = const { Cell::new(false) };
}

#[cfg(test)]
fn note_sealed_grant() {
    SEALED_ENTERED.with(|entered| entered.set(true));
}

#[cfg(test)]
pub(super) fn sealed_grant_entered() -> bool {
    SEALED_ENTERED.with(|cell| cell.get())
}

#[cfg(test)]
pub(super) fn reset_sealed_grant() {
    SEALED_ENTERED.with(|cell| cell.set(false));
}

pub(crate) fn assert_privileged_group_membership(facade: &EntityFacade, user_group_id: &str) -> Result<(), Error> {
    SECURITY_CHECKS.with(|count| count.set(count.get() + 1));
    if user_group_id.is_empty() {
        return Ok(());
    }
    if authz_disabled() {
        return Ok(());
    }
    if !privileged_groups().contains(user_group_id) {
        return Ok(());
    }
    if without_authz(|| user_in_group(facade, user_group_id))? {
        return Ok(());
    }
    Err(crate::context::Error::of_kind(
        crate::context::ErrorKind::Unauthorized,
        format!("User is not authorized to change membership of group {user_group_id}"),
    )
    .into())
}

pub(crate) fn assert_sealed_permission_grant(facade: &EntityFacade, user_permission_id: &str) -> Result<(), Error> {
    SECURITY_CHECKS.with(|count| count.set(count.get() + 1));
    #[cfg(test)]
    note_sealed_grant();
    if user_permission_id.is_empty() {
        return Ok(());
    }
    if authz_disabled() {
        return Ok(());
    }
    if !sealed_permissions().contains(user_permission_id) {
        return Ok(());
    }
    if without_authz(|| user_has_permission(facade, user_permission_id))? {
        return Ok(());
    }
    Err(crate::context::Error::of_kind(
        crate::context::ErrorKind::Unauthorized,
        format!("User is not authorized to grant permission {user_permission_id}"),
    )
    .into())
}

fn privileged_groups() -> BTreeSet<String> {
    csv_ids("privileged-groups", DEFAULT_PRIVILEGED_GROUPS)
}

fn sealed_permissions() -> BTreeSet<String> {
    csv_ids("sealed-permissions", DEFAULT_SEALED_PERMISSIONS)
}

/// The ids in the `user-facade` `authz` attribute `name` of the current factory's conf, or in `default_ids` when it is
/// empty or there is no current context.
fn csv_ids(name: &str, default_ids: &str) -> BTreeSet<String> {
    let loaded = ExecutionContext::with_current(|ctx| ctx.factory().user_facade_attr("authz", name))
        .flatten()
        .map(|raw| moqui_lib::env::expand(&raw).unwrap_or(raw))
        .unwrap_or_default();
    let source = if loaded.is_empty() {
        default_ids
    } else {
        loaded.as_str()
    };
    source
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect()
}

fn authz_disabled() -> bool {
    ExecutionContext::with_current(|ctx| ctx.authz_disabled()).unwrap_or(false)
}

fn without_authz<T>(body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    let guard = ExecutionContext::with_current(|ctx| ctx.disable_authz());
    let result = body();
    drop(guard);
    result
}

/// As Java's `isInGroupById`: everyone is in `ALL_USERS`, with no lookup and no user needed.
pub(super) fn user_in_group(facade: &EntityFacade, group_id: &str) -> Result<bool, Error> {
    if group_id == "ALL_USERS" {
        return Ok(true);
    }
    let Some(user_id) = current_account_id(facade)? else {
        return Ok(false);
    };
    let now = now_stamp();
    let rows = facade
        .find("moqui.security.UserGroupMember")?
        .eq("userId", Value::string(&user_id))
        .eq("userGroupId", Value::string(group_id))
        .list()?;
    Ok(rows
        .values()
        .iter()
        .any(|row| membership_open(row.get("fromDate"), row.get("thruDate"), &now)))
}

/// As Java's `hasPermissionById`, which reads the `UserPermissionCheck` view of the user's own memberships: a permission
/// granted to `ALL_USERS` counts only for a user with a `UserGroupMember` row for it.
pub(super) fn user_has_permission(facade: &EntityFacade, permission_id: &str) -> Result<bool, Error> {
    let Some(user_id) = current_account_id(facade)? else {
        return Ok(false);
    };
    let now = now_stamp();
    let members = facade
        .find("moqui.security.UserGroupMember")?
        .eq("userId", Value::string(&user_id))
        .list()?;
    for member in members.values() {
        if !membership_open(member.get("fromDate"), member.get("thruDate"), &now) {
            continue;
        }
        let group_id = row_text(member, "userGroupId");
        if group_id.is_empty() {
            continue;
        }
        let grants = facade
            .find("moqui.security.UserGroupPermission")?
            .eq("userGroupId", Value::string(&group_id))
            .eq("userPermissionId", Value::string(permission_id))
            .list()?;
        if grants
            .values()
            .iter()
            .any(|row| membership_open(row.get("fromDate"), row.get("thruDate"), &now))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn current_account_id(facade: &EntityFacade) -> Result<Option<String>, Error> {
    let Some(username) = crate::user::current_username().map_err(to_lib)? else {
        return Ok(None);
    };
    if username.is_empty() {
        return Ok(None);
    }
    let Some(account) = facade
        .find("moqui.security.UserAccount")?
        .eq("username", Value::string(username))
        .one()?
    else {
        return Ok(None);
    };
    let user_id = row_text(&account, "userId");
    if user_id.is_empty() {
        Ok(None)
    } else {
        Ok(Some(user_id))
    }
}

/// As Java's `filterByDate`: a missing date leaves that end open, and both ends are inclusive. A date that does not
/// compare is not open.
fn membership_open(from: Option<&Value>, thru: Option<&Value>, now: &Value) -> bool {
    if let Some(from) = from.filter(|from| !from.is_null()) {
        let Ok(order) = moqui_lib::object::compare_obj(from, now) else {
            return false;
        };
        if order == std::cmp::Ordering::Greater {
            return false;
        }
    }
    match thru {
        None | Some(Value::Null) => true,
        Some(thru) => moqui_lib::object::compare_obj(thru, now).is_ok_and(std::cmp::Ordering::is_ge),
    }
}

fn row_text(row: &EntityValue, field: &str) -> String {
    match row.get(field) {
        Some(value) if !value.is_null() => moqui_lib::object::to_plain_string(value),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use moqui_lib::object::Value;

    use super::membership_open;

    #[test]
    fn a_missing_date_leaves_that_end_open_as_filter_by_date() {
        let at = |millis: i64| moqui_lib::object::basic_convert(&Value::I64(millis), "java.sql.Timestamp").value;
        let now = at(2_000);
        assert!(membership_open(None, None, &now));
        assert!(membership_open(Some(&Value::Null), Some(&Value::Null), &now));
        assert!(membership_open(Some(&at(1_000)), Some(&at(3_000)), &now));
        assert!(!membership_open(Some(&at(3_000)), None, &now), "not started");
        assert!(!membership_open(None, Some(&at(1_000)), &now), "ended");
        assert!(membership_open(Some(&now), Some(&now), &now), "both ends are inclusive");
    }
}
