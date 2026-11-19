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

use moqui_lib::object::Value;

use super::super::session;
use super::*;
use crate::context::ExecutionContextFactory;
use crate::entity::EntityFacade;

fn open_ctx() -> (crate::context::ExecutionContext, EntityFacade) {
    session::reset_session();
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    (ctx, facade)
}

fn row(facade: &EntityFacade, entity: &str, fields: &[(&str, Value)]) {
    let mut value = facade.make_value(entity).unwrap();
    for (name, field) in fields {
        value.set(*name, field.clone());
    }
    value.create(facade).unwrap();
}

fn group_preference(facade: &EntityFacade, group: &str, key: &str, value: &str, priority: i64) {
    row(
        facade,
        GROUP_PREFERENCE,
        &[
            ("userGroupId", Value::string(group)),
            ("preferenceKey", Value::string(key)),
            ("preferenceValue", Value::string(value)),
            ("groupPriority", Value::I64(priority)),
        ],
    );
}

fn seed(facade: &EntityFacade) {
    row(
        facade,
        "moqui.security.UserAccount",
        &[("userId", Value::string("ada")), ("username", Value::string("ada"))],
    );
    row(
        facade,
        GROUP_MEMBER,
        &[
            ("userGroupId", Value::string("EDITORS")),
            ("userId", Value::string("ada")),
            ("fromDate", crate::user::stamp_value(1)),
        ],
    );
    row(
        facade,
        GROUP_MEMBER,
        &[
            ("userGroupId", Value::string("FORMER")),
            ("userId", Value::string("ada")),
            ("fromDate", crate::user::stamp_value(1)),
            ("thruDate", crate::user::stamp_value(2)),
        ],
    );
    group_preference(facade, ALL_USERS, "theme.dark", "false", 9);
    group_preference(facade, ALL_USERS, "page.size", "20", 9);
    group_preference(facade, "EDITORS", "page.size", "50", 1);
    group_preference(facade, "FORMER", "page.size", "99", 0);
}

/// As Java's `filterByDate`, both ends are inclusive: a row from or thru now is in date.
#[test]
fn a_row_from_or_thru_now_is_in_date() {
    let facade = crate::testing::open();
    let now = crate::user::stamp_value(5);
    let member = |from: i64, thru: Option<i64>| {
        let mut value = facade.make_value(GROUP_MEMBER).unwrap();
        value.set("fromDate", crate::user::stamp_value(from));
        if let Some(thru) = thru {
            value.set("thruDate", crate::user::stamp_value(thru));
        }
        value
    };
    assert!(date_open(&member(5, None), "fromDate", "thruDate", &now));
    assert!(date_open(&member(1, Some(5)), "fromDate", "thruDate", &now));
    assert!(!date_open(&member(1, Some(4)), "fromDate", "thruDate", &now));
    assert!(!date_open(&member(6, None), "fromDate", "thruDate", &now));
}

#[test]
fn set_and_get_preference_with_group_and_default_fallback() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    seed(&facade);
    // Nobody logged in: the ALL_USERS rows are the defaults, and setting needs a user.
    assert_eq!(get_preference("page.size").unwrap().as_deref(), Some("20"));
    assert!(set_preference("page.size", Some("10")).is_err());

    crate::user::internal_login_user("ada").unwrap();
    assert_eq!(
        user_group_ids(Some("ada")).unwrap().into_iter().collect::<Vec<_>>(),
        ["ALL_USERS", "EDITORS"]
    );
    // The lower groupPriority wins, and the ended membership does not count.
    assert_eq!(get_preference("page.size").unwrap().as_deref(), Some("50"));
    assert_eq!(get_preference("theme.dark").unwrap().as_deref(), Some("false"));
    assert_eq!(get_preference("missing").unwrap(), None);

    set_preference("page.size", Some("10")).unwrap();
    assert_eq!(get_preference("page.size").unwrap().as_deref(), Some("10"));
    set_preference("page.size", Some("12")).unwrap();
    assert_eq!(get_preference("page.size").unwrap().as_deref(), Some("12"));
    let all = get_preferences().unwrap();
    assert_eq!(all.get("page.size").map(String::as_str), Some("12"));
    assert_eq!(all.get("theme.dark").map(String::as_str), Some("false"));

    // A process property overrides the database, also under the key with `_` for `.`.
    moqui_lib::env::set_property("theme_dark", "true");
    assert_eq!(get_preference("theme.dark").unwrap().as_deref(), Some("true"));
    moqui_lib::env::clear_property("theme_dark");
    session::reset_session();
}

#[test]
fn scripts_reach_preferences_groups_permissions_and_session_attributes() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    seed(&facade);
    row(
        &facade,
        GROUP_PERMISSION,
        &[
            ("userGroupId", Value::string("EDITORS")),
            ("userPermissionId", Value::string("EDIT_PAGES")),
            ("fromDate", crate::user::stamp_value(1)),
        ],
    );
    crate::user::internal_login_user("ada").unwrap();
    crate::script::clear_context_stack();
    crate::script::run_actions(
        r#"<actions><script><![CDATA[
                ec.user.setPreference("qz.print.enabled", "true")
                ec.user.setPreference("qz.printer", "Front Desk")
                const prefs = ec.user.getPreferences("qz\\..*")
                prefKeys = Object.keys(prefs).sort().join(",")
                printer = ec.user.getPreference("qz.printer")
                pageSize = ec.user.getPreference("page.size")
                canEdit = ec.user.hasPermission("EDIT_PAGES")
                canDelete = ec.user.hasPermission("DELETE_PAGES")
                inEditors = ec.user.isInGroup("EDITORS")
                groups = ec.user.getUserGroupIdSet().join(",")
                ec.web.sessionAttributes.moquiKept = "kept"
                ec.web.sessionAttributes.put("moquiGone", "gone")
                removed = ec.web.sessionAttributes.remove("moquiGone")
            ]]></script></actions>"#,
    )
    .unwrap();
    let text = |name: &str| crate::script::context_value(name).map(|value| to_plain_string(&value));
    assert_eq!(text("prefKeys").as_deref(), Some("qz.print.enabled,qz.printer"));
    assert_eq!(text("printer").as_deref(), Some("Front Desk"));
    assert_eq!(text("pageSize").as_deref(), Some("50"));
    assert_eq!(crate::script::context_value("canEdit"), Some(Value::Bool(true)));
    assert_eq!(crate::script::context_value("canDelete"), Some(Value::Bool(false)));
    assert_eq!(crate::script::context_value("inEditors"), Some(Value::Bool(true)));
    assert_eq!(text("groups").as_deref(), Some("ALL_USERS,EDITORS"));
    // `ec.web.sessionAttributes` writes through, with the Java map's put and remove.
    assert_eq!(text("removed").as_deref(), Some("gone"));
    assert_eq!(session::session_attribute("moquiKept").as_deref(), Some("kept"));
    assert_eq!(session::session_attribute("moquiGone"), None);
    session::reset_session();
}

/// The inserted user is in `ALL_USERS` and `ADMIN` after those memberships, and `is_in_group` says so.
#[test]
fn the_inserted_user_is_in_all_users_and_admin() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    row(
        &facade,
        "moqui.security.UserAccount",
        &[("userId", Value::string("ada")), ("username", Value::string("ada"))],
    );
    for group in ["ALL_USERS", "ADMIN"] {
        row(
            &facade,
            GROUP_MEMBER,
            &[
                ("userGroupId", Value::string(group)),
                ("userId", Value::string("ada")),
                ("fromDate", crate::user::stamp_value(1)),
            ],
        );
    }
    crate::user::internal_login_user("ada").unwrap();
    let groups = user_group_ids(Some("ada")).unwrap();
    assert!(groups.contains("ALL_USERS"));
    assert!(groups.contains("ADMIN"));
    assert!(is_in_group("ALL_USERS").unwrap());
    assert!(is_in_group("ADMIN").unwrap());
    session::reset_session();
}

/// `hasPermission` and `isInGroup` read a grant's and a membership's dates at the effective time (`ec.user.nowTimestamp`),
/// as Java's do, and at the clock without one (found while fixing SEC-BOS-97). The group set authorization reads stays
/// on the clock, as Java's `getUserGroupIdSet`.
#[test]
fn has_permission_and_is_in_group_read_the_dates_at_the_effective_time() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    let stamp = crate::user::stamp_value;
    row(
        &facade,
        "moqui.security.UserAccount",
        &[("userId", Value::string("ada")), ("username", Value::string("ada"))],
    );
    // a membership of 1000 to 3000 seconds after the epoch, its group granting a permission from 1000 to 2000
    let member = [
        ("userGroupId", Value::string("DATED")),
        ("userId", Value::string("ada")),
    ];
    let membership = [("fromDate", stamp(1_000_000)), ("thruDate", stamp(3_000_000))];
    row(&facade, GROUP_MEMBER, &[&member[..], &membership[..]].concat());
    let grant = [
        ("userGroupId", Value::string("DATED")),
        ("userPermissionId", Value::string("DATED_PERM")),
    ];
    let window = [("fromDate", stamp(1_000_000)), ("thruDate", stamp(2_000_000))];
    row(&facade, GROUP_PERMISSION, &[&grant[..], &window[..]].concat());
    crate::user::internal_login_user("ada").unwrap();
    assert!(!has_permission("DATED_PERM").unwrap(), "the clock is after the grant");
    assert!(!is_in_group("DATED").unwrap(), "the clock is after the membership");
    ctx.set_effective_time(Some(1_500_000));
    assert!(
        has_permission("DATED_PERM").unwrap(),
        "in the grant at the effective time"
    );
    assert!(is_in_group("DATED").unwrap(), "in the membership at the effective time");
    assert!(
        !user_group_ids(Some("ada")).unwrap().contains("DATED"),
        "authorization reads the clock"
    );
    ctx.set_effective_time(Some(2_500_000));
    assert!(
        !has_permission("DATED_PERM").unwrap(),
        "after the grant at the effective time"
    );
    assert!(is_in_group("DATED").unwrap(), "still in the membership");
    ctx.set_effective_time(Some(500_000));
    assert!(
        !has_permission("DATED_PERM").unwrap(),
        "before the grant at the effective time"
    );
    assert!(
        !is_in_group("DATED").unwrap(),
        "before the membership at the effective time"
    );
    ctx.set_effective_time(None);
    session::reset_session();
}

/// `EXTERNAL_USER`, mr-moqui's own gate with no Java counterpart, is read on the clock: a user made external is external,
/// and never holds an agent permission, while code has set an earlier effective time.
#[test]
fn an_external_user_is_external_on_the_clock_whatever_the_effective_time() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    let stamp = crate::user::stamp_value;
    row(
        &facade,
        "moqui.security.UserAccount",
        &[("userId", Value::string("ada")), ("username", Value::string("ada"))],
    );
    // a member since the epoch of a group that grants AgentUse since then, and EXTERNAL_USER from 1000 seconds after it
    row(
        &facade,
        GROUP_MEMBER,
        &[
            ("userGroupId", Value::string("OUTSIDE")),
            ("userId", Value::string("ada")),
            ("fromDate", stamp(0)),
        ],
    );
    for (permission, from) in [("AgentUse", 0), (EXTERNAL_USER, 1_000_000)] {
        row(
            &facade,
            GROUP_PERMISSION,
            &[
                ("userGroupId", Value::string("OUTSIDE")),
                ("userPermissionId", Value::string(permission)),
                ("fromDate", stamp(from)),
            ],
        );
    }
    crate::user::internal_login_user("ada").unwrap();
    assert!(is_external_user("ada"));
    assert!(!user_has_permission("ada", "AgentUse").unwrap());
    ctx.set_effective_time(Some(500_000));
    assert!(
        is_external_user("ada"),
        "external on the clock before the effective time"
    );
    assert!(
        !user_has_permission("ada", "AgentUse").unwrap(),
        "an external user holds no agent permission at any effective time"
    );
    ctx.set_effective_time(None);
    session::reset_session();
}
