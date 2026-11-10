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

//! The `UserSecurityChecks` calls that `Security.eecas.xml` makes, as Rust: only a member of a
//! privileged group may change that group's membership, only a holder of a sealed permission may
//! grant it, and only a `DATABASE_ADMIN` holder may write a DataDocument field expression. Each
//! allows the write when authorization is disabled, as a data load runs.

use std::cell::Cell;
use std::collections::BTreeSet;

use moqui_lib::error::Error;
use moqui_lib::object::Value;

use super::eca::to_lib;
use super::util::now_stamp;
use crate::context::ExecutionContext;
use crate::entity::{EntityFacade, EntityValue};

const DEFAULT_PRIVILEGED_GROUPS: &str = "ADMIN,ADMIN_ADV,SYSMSG_RECEIVE";
const DEFAULT_SEALED_PERMISSIONS: &str =
    "GROOVY_SHELL_WEB,SQL_RUNNER_WEB,SSH_SHELL,LOAD_RUNNER,ADMIN_LOGIN_AS,ADMIN_PASSWORD,SearchRemote,DashboardRemote,REST_SCHEMA,LlmGatewayUse,COMPONENT_DEPLOY,CONF_EDIT,DATABASE_ADMIN,DATABASE_EXTERNAL,ADMIN_CONSTITUTION_GLOBAL,ADMIN_IDENTITY_GLOBAL,ADMIN_ACCESS_MANAGE,ADMIN_BACKUP_DOWNLOAD";

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

/// Refuses a membership in a group that grants a sealed permission unless the user holds each sealed permission the group
/// grants (one not ended: open or starting later), so a sealed grant is not passed on through a group that is not
/// privileged (impl-admin F-P3-3): a membership carries the group's grants as surely as the grant row does, and only a
/// holder may grant a sealed permission. A change that only ends a membership sooner (`ends_sooner`: an update whose end
/// is set and not later than the stored one) is allowed, as is any write while authorization is disabled, as a data load
/// runs. The converse, granting a sealed permission to a group that has members, is [`assert_sealed_permission_grant`].
pub(crate) fn assert_sealed_group_membership(
    facade: &EntityFacade,
    user_group_id: &str,
    ends_sooner: bool,
) -> Result<(), Error> {
    SECURITY_CHECKS.with(|count| count.set(count.get() + 1));
    if user_group_id.is_empty() || ends_sooner {
        return Ok(());
    }
    if authz_disabled() {
        return Ok(());
    }
    let sealed = sealed_permissions();
    let missing = without_authz(|| {
        let now = now_stamp();
        let grants = facade
            .find("moqui.security.UserGroupPermission")?
            .eq("userGroupId", Value::string(user_group_id))
            .list()?;
        let mut carried: BTreeSet<String> = BTreeSet::new();
        for grant in grants.values() {
            let permission_id = row_text(grant, "userPermissionId");
            if sealed.contains(&permission_id) && membership_open(None, grant.get("thruDate"), &now) {
                carried.insert(permission_id);
            }
        }
        for permission_id in carried {
            if !user_has_permission(facade, &permission_id)? {
                return Ok(Some(permission_id));
            }
        }
        Ok(None)
    })?;
    match missing {
        None => Ok(()),
        Some(permission_id) => Err(crate::context::Error::of_kind(
            crate::context::ErrorKind::Unauthorized,
            format!(
                "User is not authorized to change membership of group {user_group_id}, which grants the sealed permission {permission_id}"
            ),
        )
        .into()),
    }
}

/// Whether a membership write only ends the membership sooner: an update (`stored` the row before it) whose end `thru` is
/// set and not later than the stored end (an open stored end is the latest). A create, a stored row missing, an open or
/// later end, or dates that do not compare widen or may widen it.
pub(crate) fn membership_ends_sooner(thru: &Value, stored: Option<&Value>) -> bool {
    let Some(stored) = stored else {
        return false;
    };
    let Some(thru) = as_stamp(thru) else {
        return false;
    };
    if stored.is_null() {
        return true;
    }
    let Some(stored) = as_stamp(stored) else {
        return false;
    };
    moqui_lib::object::compare_obj(&thru, &stored).is_ok_and(std::cmp::Ordering::is_le)
}

/// A date value as a Timestamp, or None when it is empty or does not convert.
fn as_stamp(value: &Value) -> Option<Value> {
    if moqui_lib::object::is_empty(value) {
        return None;
    }
    let converted = moqui_lib::object::basic_convert(value, "Timestamp").value;
    matches!(converted, Value::Timestamp(_)).then_some(converted)
}

/// The permission that writing a DataDocument field expression needs. An expression is script that runs, as whoever
/// reads the document, on the server, which is data manipulation that ends in code running, as `DATABASE_ADMIN` allows.
const DOCUMENT_EXPRESSION_PERMISSION: &str = "DATABASE_ADMIN";

/// Refuses a create or update of a `DataDocumentField` whose `fieldPath` is an expression (it has a `(`; the
/// document reads its last element as script) unless the user has `DATABASE_ADMIN`. A path with no `(` and a delete
/// (`field_path` empty) are allowed, as is any write while authorization is disabled, as a data load runs.
pub(crate) fn assert_document_expression(facade: &EntityFacade, field_path: &str) -> Result<(), Error> {
    SECURITY_CHECKS.with(|count| count.set(count.get() + 1));
    if !field_path.contains('(') {
        return Ok(());
    }
    if authz_disabled() {
        return Ok(());
    }
    if without_authz(|| user_has_permission(facade, DOCUMENT_EXPRESSION_PERMISSION))? {
        return Ok(());
    }
    Err(crate::context::Error::of_kind(
        crate::context::ErrorKind::Unauthorized,
        format!("A DataDocument field expression needs the {DOCUMENT_EXPRESSION_PERMISSION} permission"),
    )
    .into())
}

/// The effective privileged groups: the `user-facade` `authz` `privileged-groups` list (or the framework's default) joined with
/// every component's `privileged-group` element. Only a member of one changes its membership. Admin's access services read it to
/// mark a group and to answer a refusal with its reason before the EECA refuses the write.
pub(crate) fn privileged_groups() -> BTreeSet<String> {
    conf_ids("privileged-groups", "privileged-group", DEFAULT_PRIVILEGED_GROUPS)
}

/// The effective sealed permissions, as [`privileged_groups`]: only a holder of one grants it.
pub(crate) fn sealed_permissions() -> BTreeSet<String> {
    conf_ids("sealed-permissions", "sealed-permission", DEFAULT_SEALED_PERMISSIONS)
}

/// The ids of the current factory's conf: the `user-facade` `authz` attribute `attribute` (or `default_ids` when it is
/// empty or there is no current context) joined with the `name` of every `authz` child named `element`, which the
/// components add.
fn conf_ids(attribute: &str, element: &str, default_ids: &str) -> BTreeSet<String> {
    let (loaded, added) = ExecutionContext::with_current(|ctx| {
        let factory = ctx.factory();
        (
            factory.user_facade_attr("authz", attribute),
            factory.authz_names(element),
        )
    })
    .unwrap_or_default();
    let loaded = loaded.map(|raw| moqui_lib::env::expand(&raw).unwrap_or(raw));
    join_ids(loaded.as_deref(), default_ids, &added)
}

/// The comma separated `listed` ids, or `default_ids` when `listed` is missing or empty, with every id of `added`.
fn join_ids(listed: Option<&str>, default_ids: &str, added: &[String]) -> BTreeSet<String> {
    let source = listed.filter(|text| !text.trim().is_empty()).unwrap_or(default_ids);
    source
        .split(',')
        .chain(added.iter().map(String::as_str))
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

    use super::{join_ids, membership_open, DEFAULT_SEALED_PERMISSIONS};

    #[test]
    fn the_listed_ids_or_the_defaults_are_joined_with_every_added_id() {
        let added = ["APP_ADMIN".to_string(), " APP_BACKUP ".to_string()];
        let ids = join_ids(None, "ADMIN,ADMIN_ADV", &added);
        assert_eq!(
            ids.into_iter().collect::<Vec<_>>(),
            ["ADMIN", "ADMIN_ADV", "APP_ADMIN", "APP_BACKUP"]
        );
        // an attribute a component still sets replaces the defaults, as before; the added ids still apply
        let ids = join_ids(Some("X"), "ADMIN,ADMIN_ADV", &added);
        assert_eq!(ids.into_iter().collect::<Vec<_>>(), ["APP_ADMIN", "APP_BACKUP", "X"]);
        assert_eq!(
            join_ids(Some(" "), "ADMIN", &[]).into_iter().collect::<Vec<_>>(),
            ["ADMIN"]
        );
    }

    #[test]
    fn the_framework_seals_the_admin_grants_that_widen_access() {
        let sealed = join_ids(None, DEFAULT_SEALED_PERMISSIONS, &[]);
        for permission in [
            "ADMIN_IDENTITY_GLOBAL",
            "ADMIN_ACCESS_MANAGE",
            "ADMIN_BACKUP_DOWNLOAD",
            "ADMIN_CONSTITUTION_GLOBAL",
        ] {
            assert!(sealed.contains(permission), "{permission}");
        }
    }

    #[test]
    fn a_missing_date_leaves_that_end_open_as_filter_by_date() {
        let at = |millis: i64| moqui_lib::object::basic_convert(&Value::I64(millis), "Timestamp").value;
        let now = at(2_000);
        assert!(membership_open(None, None, &now));
        assert!(membership_open(Some(&Value::Null), Some(&Value::Null), &now));
        assert!(membership_open(Some(&at(1_000)), Some(&at(3_000)), &now));
        assert!(!membership_open(Some(&at(3_000)), None, &now), "not started");
        assert!(!membership_open(None, Some(&at(1_000)), &now), "ended");
        assert!(membership_open(Some(&now), Some(&now), &now), "both ends are inclusive");
    }
}
