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

//! The lock rules on an in-memory database, and a lock taken through one factory holding a write through another
//! factory on the same database, as two members of a cluster (HL-036).

use super::*;
use crate::context::{ExecutionContext, ExecutionContextFactory};

fn facade() -> EntityFacade {
    let facade = crate::testing::open();
    facade.assume_db_tables();
    facade
}

fn lock(user_id: &str, depth_infinity: bool) -> Lock {
    Lock {
        token: format!("opaquelocktoken:{user_id}"),
        user_id: user_id.to_string(),
        owner: String::new(),
        depth_infinity,
        seconds: 60,
        expires_millis: Lock::expiry(60),
    }
}

#[test]
fn a_lock_holds_its_own_location_and_nothing_beside_it() {
    let facade = facade();
    let root = "dbresource://locks-test-one";
    put(&facade, &format!("{root}/docs/a.txt"), &lock("alice", false)).unwrap();
    put(&facade, &format!("{root}/shared/"), &lock("alice", true)).unwrap();
    let check = |path: &str, user: Option<&str>, tree: bool| check(&facade, &format!("{root}/{path}"), user, tree);
    assert!(check("docs/a.txt", Some("bob"), false).is_err());
    assert!(check("docs/a.txt#v2", None, false).is_err(), "no user is another user");
    assert!(check("docs/a.txt", Some("alice"), true).is_ok(), "the owner writes");
    assert!(check("docs/b.txt", Some("bob"), false).is_ok(), "the file beside it");
    assert!(check("docs", Some("bob"), false).is_ok(), "a write in the folder");
    assert!(
        check("docs", Some("bob"), true).is_err(),
        "a move of the folder takes the file"
    );
    assert!(check("docs/a.txt.bak", Some("bob"), true).is_ok());
    assert!(
        check("shared/deep/c.txt", Some("bob"), false).is_err(),
        "depth infinity"
    );
    assert!(check("shared-not", Some("bob"), false).is_ok());
    assert!(holding(&facade, &format!("{root}/shared/deep/c.txt"))
        .unwrap()
        .is_some());
    release(&facade, &format!("{root}/docs"));
    assert!(
        check("docs/a.txt", Some("bob"), false).is_ok(),
        "released with its folder"
    );
    release(&facade, &format!("{root}/shared"));
    assert!(holding(&facade, &format!("{root}/shared/deep/c.txt"))
        .unwrap()
        .is_none());
}

#[test]
fn an_expired_lock_holds_nothing_and_goes_when_a_lock_is_taken() {
    let facade = facade();
    let location = "dbresource://locks-test-two/a.txt";
    let mut expired = lock("alice", false);
    expired.expires_millis = now_millis() - 1000;
    put(&facade, location, &expired).unwrap();
    assert!(check(&facade, location, Some("bob"), false).is_ok());
    assert!(get(&facade, location).unwrap().is_none());
    put(&facade, "dbresource://locks-test-two/b.txt", &lock("alice", false)).unwrap();
    let rows = facade.find(LOCK_ENTITY).unwrap().disable_authz().list().unwrap();
    assert_eq!(rows.values().len(), 1, "the expired row was removed");
}

#[test]
fn a_key_drops_the_version_and_trailing_slashes() {
    assert_eq!(key("dbresource://é/a.txt#v2#v3"), "dbresource://é/a.txt");
    assert_eq!(key("dbresource://docs//"), "dbresource://docs");
    assert_eq!(key("#v2/"), "#v2");
    assert_eq!(key("dbresource://a.txt"), "dbresource://a.txt");
    assert_eq!(
        with_ancestors("dbresource://drives/d1/a/b.txt"),
        [
            "dbresource://drives/d1/a/b.txt",
            "dbresource://drives/d1/a",
            "dbresource://drives/d1",
            "dbresource://drives"
        ]
    );
    assert_eq!(row_key("dbresource://a").len(), 43, "fits text-short");
}

fn user(facade: &EntityFacade, user_id: &str) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut row = facade.make_value("moqui.security.UserAccount").unwrap();
    row.set("userId", Value::string(user_id));
    row.set("username", Value::string(user_id));
    row.create(facade).unwrap();
}

/// A lock taken through one factory refuses another user's write through a second factory on the same database (a
/// second member of a cluster), until it is removed or has expired.
#[test]
fn a_lock_through_one_factory_holds_a_write_through_another() {
    let facade = facade();
    let member = |facade: &EntityFacade| {
        let context = ExecutionContextFactory::test_context_in(
            "",
            crate::service::ServicePools::idle(),
            crate::context::framework_dir(),
        );
        context.factory().set_entity(facade.clone());
        context
    };
    let first = member(&facade);
    let second = member(&facade);
    let location = "dbresource://locks-test-cluster/report.txt";
    {
        let _entered = first.enter();
        user(&facade, "alice");
        user(&facade, "bob");
        put(&facade, location, &lock("alice", false)).unwrap();
    }
    let _entered = second.enter();
    crate::user::internal_login_user("bob").unwrap();
    let resources = second.factory().resource();
    let refused = resources.write_bytes(location, b"bob's").unwrap_err();
    assert!(refused.message().contains(LOCKED_ERROR), "{refused}");
    remove(&facade, location).unwrap();
    resources.write_bytes(location, b"bob's").unwrap();
    let mut expired = lock("alice", false);
    expired.expires_millis = now_millis() - 1;
    put(&facade, location, &expired).unwrap();
    resources.write_bytes(location, b"bob's again").unwrap();
    // a lock of a folder with depth infinity, and the owner writing without its token
    put(&facade, "dbresource://locks-test-cluster", &lock("bob", true)).unwrap();
    resources.write_bytes(location, b"bob's, locked").unwrap();
    crate::user::internal_login_user("alice").unwrap();
    assert!(resources.write_bytes(location, b"alice's").is_err());
    assert!(resources.delete(location).is_err());
}
