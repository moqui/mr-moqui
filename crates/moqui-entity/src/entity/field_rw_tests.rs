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

use super::{encrypt_in, raw_read, RawReadGuard};
use moqui_db::sql::Condition;
use moqui_lib::object::Value;

use crate::entity::{EntityFacade, EntityList};

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
fn audit_stack_without_an_entered_context_is_empty_not_null() {
    let facade = audit_facade();
    write_username(&facade, "alice", true);
    write_username(&facade, "alice2", false);
    let audits = username_audits(&facade);
    assert_eq!(audits.len(), 2);
    for row in audits.iter() {
        assert_eq!(row.get("artifactStack").and_then(Value::as_str), Some(""));
    }
}

/// As Java's `FieldInfo`, every bound value of an encrypted field is sealed, whatever the operator.
#[test]
fn every_bound_value_of_an_encrypted_field_is_sealed() {
    let config = crate::entity::crypto::CryptConfig {
        pass: "MoquiDefaultPassword:CHANGEME".to_string(),
        salt: "20201202".to_string(),
        iterations: 10,
        algorithm: "PBEWithHmacSHA256AndAES_128".to_string(),
    };
    let sealed = Value::string(crate::entity::crypto::encrypt("v", &config).unwrap());
    let plain = || Value::string("v");
    let condition = Condition::and(vec![
        Condition::Like("secret".into(), plain()),
        Condition::Between("secret".into(), plain(), plain()),
        Condition::IgnoreCase(Box::new(Condition::Greater("secret".into(), plain()))),
        Condition::Equals("other".into(), plain()),
    ]);
    let Condition::And(list) = encrypt_in(&["secret"], &config, condition).unwrap() else {
        panic!("an and");
    };
    assert_eq!(list[0], Condition::Like("secret".into(), sealed.clone()));
    assert_eq!(
        list[1],
        Condition::Between("secret".into(), sealed.clone(), sealed.clone())
    );
    assert_eq!(
        list[2],
        Condition::IgnoreCase(Box::new(Condition::Greater("secret".into(), sealed)))
    );
    assert_eq!(list[3], Condition::Equals("other".into(), plain()));
}

#[test]
fn a_nested_raw_read_restores_the_outer_flag() {
    assert!(!raw_read());
    {
        let _outer = RawReadGuard::enter();
        {
            let _inner = RawReadGuard::enter();
            assert!(raw_read());
        }
        assert!(raw_read());
    }
    assert!(!raw_read());
}
