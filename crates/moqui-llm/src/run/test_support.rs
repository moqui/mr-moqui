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

//! What the crate's unit tests that run agents share: the rows a run's user needs.

use moqui_core::entity::EntityFacade;
use moqui_lib::object::Value;

/// Grant `user_id` `permission` through a group of its own, in a test's database.
pub(crate) fn grant(facade: &EntityFacade, user_id: &str, permission: &str) {
    let group = format!("TEST_{permission}");
    let rows: [(&str, Vec<(&str, &str)>); 3] = [
        (
            "moqui.security.UserGroup",
            vec![("userGroupId", group.as_str()), ("description", group.as_str())],
        ),
        (
            "moqui.security.UserGroupPermission",
            vec![
                ("userGroupId", group.as_str()),
                ("userPermissionId", permission),
                ("fromDate", "1265184000000"),
            ],
        ),
        (
            "moqui.security.UserGroupMember",
            vec![
                ("userGroupId", group.as_str()),
                ("userId", user_id),
                ("fromDate", "1265184000000"),
            ],
        ),
    ];
    for (entity, fields) in rows {
        let mut value = facade.make_value(entity).expect("a security entity");
        for (field, text) in fields {
            value.set(field, Value::string(text));
        }
        // the group and its permission are there from an earlier user
        let _ = value.create(facade);
    }
}

/// Grant `user_id` the `AgentUse` permission, which each step of a run checks, in a test's database.
pub(crate) fn grant_agent_use(facade: &EntityFacade, user_id: &str) {
    grant(facade, user_id, "AgentUse");
}
