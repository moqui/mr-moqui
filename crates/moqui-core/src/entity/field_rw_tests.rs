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

use crate::context::ExecutionContextFactory;
use crate::entity::{EntityFacade, EntityList};
use moqui_lib::object::Value;

fn audit_facade() -> EntityFacade {
    let facade = crate::testing::open();
    facade.set_disable_audit_log(false);
    facade
}

fn write_username(facade: &EntityFacade, username: &str, create: bool) {
    let mut user = facade.make_value("moqui.security.UserAccount").unwrap();
    user.set("userId", Value::string("AUDIT_STACK"));
    user.set("username", Value::string(username));
    if create {
        user.create(facade).unwrap();
    } else {
        user.update(facade).unwrap();
    }
}

fn username_audits(facade: &EntityFacade) -> EntityList {
    facade
        .find("moqui.entity.EntityAuditLog")
        .unwrap()
        .eq("changedEntityName", Value::string("moqui.security.UserAccount"))
        .eq("changedFieldName", Value::string("username"))
        .list()
        .unwrap()
}

#[test]
fn audit_stack_follows_create_update_and_scoped_contexts() {
    let facade = audit_facade();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    assert!(facade.begin("moqui.security.UserAccount", None).unwrap());
    context.push("AT_OTHER", "data load", "all");
    context.push("AT_SERVICE", "create#Account", "create");
    write_username(&facade, "alice", true);
    context.pop().unwrap();
    assert_eq!(context.artifact_stack_names(), "data load");

    // A nested execution context must not pick up the outer context's stack.
    let inner = ExecutionContextFactory::test_context();
    {
        let _entered = inner.enter();
        let _authz = inner.disable_authz();
        inner.push("AT_SERVICE", "update#Account", "update");
        write_username(&facade, "alice2", false);
        assert_eq!(inner.artifact_stack_names(), "update#Account");
    }
    write_username(&facade, "alice3", false);
    assert_eq!(context.artifact_stack_names(), "data load");
    facade.commit("moqui.security.UserAccount").unwrap();

    // Stack capture must not move audit writes out of the entity's transaction.
    assert!(facade.begin("moqui.security.UserAccount", None).unwrap());
    write_username(&facade, "rolled-back", false);
    facade.rollback("moqui.security.UserAccount").unwrap();
    let audits = username_audits(&facade);
    assert_eq!(audits.len(), 3);
    for (username, expected) in [
        ("alice", "moqui.security.UserAccount, create#Account, data load"),
        ("alice2", "moqui.security.UserAccount, update#Account"),
        ("alice3", "moqui.security.UserAccount, data load"),
    ] {
        let row = audits
            .iter()
            .find(|row| row.get("newValueText").and_then(Value::as_str) == Some(username))
            .unwrap();
        assert_eq!(row.get("artifactStack").and_then(Value::as_str), Some(expected));
    }
}

#[test]
fn audit_stack_of_an_entered_empty_context_is_empty() {
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    assert_eq!((moqui_entity::hooks::get().stack_names)(), "");
}

#[test]
fn audit_stack_truncates_to_java_utf16_length() {
    let facade = audit_facade();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let prefix = "moqui.security.UserAccount, ";
    // Non-BMP characters count as two Java string characters, not one Rust char or four bytes.
    let name = format!("{}😀tail", "é".repeat(3997 - prefix.len()));
    context.push("AT_OTHER", &name, "all");
    write_username(&facade, "alice", true);
    let audits = username_audits(&facade);
    let stack = audits
        .get(0)
        .unwrap()
        .get("artifactStack")
        .and_then(Value::as_str)
        .unwrap();
    assert_eq!(stack, format!("{prefix}{}😀t", "é".repeat(3997 - prefix.len())));
    assert_eq!(stack.encode_utf16().count(), 4000);
    context.pop().unwrap();

    // Java substring can split a surrogate pair. Rust strings represent that orphan lossily.
    context.push("AT_OTHER", &format!("{}😀", "a".repeat(3999 - prefix.len())), "all");
    write_username(&facade, "alice2", false);
    let audits = username_audits(&facade);
    let row = audits
        .iter()
        .find(|row| row.get("newValueText").and_then(Value::as_str) == Some("alice2"))
        .unwrap();
    let stack = row.get("artifactStack").and_then(Value::as_str).unwrap();
    assert_eq!(stack, format!("{prefix}{}\u{fffd}", "a".repeat(3999 - prefix.len())));
    assert_eq!(stack.encode_utf16().count(), 4000);
}
