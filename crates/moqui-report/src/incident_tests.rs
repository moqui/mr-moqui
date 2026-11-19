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

use super::*;

/// SEC-TLS-107: an alert opens an incident only for an owner who holds MONITOR_INCIDENT by their own grants, an active
/// account; a severity label is taken only as minor, major or critical.
#[test]
fn an_alert_opens_an_incident_only_for_an_owner_who_holds_monitor_incident() {
    let facade = moqui_core::testing::open();
    facade.assume_db_tables();
    let ctx = moqui_core::context::ExecutionContextFactory::test_context_in(
        "",
        moqui_core::service::ServicePools::idle(),
        moqui_core::context::framework_dir(),
    );
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    {
        let _authz = ctx.disable_authz();
        let make = |entity: &str, fields: &[(&str, Value)]| {
            let mut row = facade.make_value(entity).unwrap();
            for (name, value) in fields {
                row.set(*name, value.clone());
            }
            row.create(&facade).unwrap();
        };
        for user in ["INC_HOLDER", "INC_PLAIN"] {
            make(
                "moqui.security.UserAccount",
                &[
                    ("userId", Value::string(user)),
                    ("username", Value::string(user.to_ascii_lowercase())),
                    ("disabled", Value::string("N")),
                ],
            );
        }
        make("moqui.security.UserGroup", &[("userGroupId", Value::string("INC_GROUP"))]);
        let from = Value::Timestamp(jiff::Timestamp::UNIX_EPOCH);
        make(
            "moqui.security.UserGroupMember",
            &[
                ("userGroupId", Value::string("INC_GROUP")),
                ("userId", Value::string("INC_HOLDER")),
                ("fromDate", from.clone()),
            ],
        );
        make(
            "moqui.security.UserGroupPermission",
            &[
                ("userGroupId", Value::string("INC_GROUP")),
                ("userPermissionId", Value::string("MONITOR_INCIDENT")),
                ("fromDate", from),
            ],
        );
    }
    let now = moqui_core::user::now_millis();
    assert!(may_open_incident(&ctx, "INC_HOLDER", now));
    assert!(!may_open_incident(&ctx, "INC_PLAIN", now));
    assert!(!may_open_incident(&ctx, "INC_NOBODY", now));
    assert_eq!(severity_of(&json!({"severity": "critical"})), "critical");
    assert_eq!(severity_of(&json!({"severity": "minor"})), "minor");
    assert_eq!(severity_of(&json!({"severity": "catastrophic <b>"})), "major");
    assert_eq!(severity_of(&json!({})), "major");
}
