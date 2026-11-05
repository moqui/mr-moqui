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

use super::{AnonymousAccess, AnonymousLogin};
use crate::context::{Authz, ExecutionContextFactory, Tarpit};

#[test]
fn modes_name_their_access() {
    assert_eq!(AnonymousAccess::from_mode("anonymous-all"), Some(AnonymousAccess::All));
    assert_eq!(
        AnonymousAccess::from_mode("anonymous-view"),
        Some(AnonymousAccess::View)
    );
    for other in ["", "true", "false", "anonymous"] {
        assert_eq!(AnonymousAccess::from_mode(other), None, "{other}");
    }
}

/// A login is ended only by the holder that started it: a nested holder and a logged-in user start none.
#[test]
fn only_the_login_a_holder_started_is_ended() {
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let mut account = facade.make_value("moqui.security.UserAccount").unwrap();
    account.set("userId", moqui_lib::object::Value::string("someone"));
    account.set("username", moqui_lib::object::Value::string("someone"));
    account.create(&facade).unwrap();
    let mut outer = AnonymousLogin::default();
    outer.begin().unwrap();
    assert!(crate::user::is_anonymous().unwrap());
    {
        let mut inner = AnonymousLogin::default();
        inner.begin().unwrap();
        inner.end();
        assert!(
            crate::user::is_anonymous().unwrap(),
            "the inner holder ended the outer login"
        );
    }
    // A second begin on the same holder starts nothing more; one end ends it, and a second end does nothing.
    outer.begin().unwrap();
    outer.end();
    assert!(!crate::user::is_anonymous().unwrap());
    outer.end();

    crate::user::internal_login_user("someone").unwrap();
    let mut user = AnonymousLogin::default();
    user.begin().unwrap();
    assert!(!crate::user::is_anonymous().unwrap());
    drop(user);
    assert_eq!(crate::user::current_username().unwrap().as_deref(), Some("someone"));
}

/// Dropping the holder ends its login.
#[test]
fn a_dropped_holder_ends_its_login() {
    let ctx = ExecutionContextFactory::test_context();
    let _entered = ctx.enter();
    {
        let mut login = AnonymousLogin::default();
        login.begin().unwrap();
        assert!(crate::user::is_anonymous().unwrap());
    }
    assert!(!crate::user::is_anonymous().unwrap());
}

/// The grant marks the top frame inheritable for the identity given, `_NA_` for anyone; view keeps an all-actions
/// grant all.
#[test]
fn the_grant_marks_the_top_frame_for_its_identity() {
    let ctx = ExecutionContextFactory::test_context();
    let _entered = ctx.enter();
    let push = |name: &str| {
        ctx.push_checked("AT_OTHER", name, "view", Authz::NotRequired, Tarpit::Skip, None)
            .unwrap();
    };
    push("anyone");
    ctx.grant_anonymous(AnonymousAccess::All, None);
    let info = ctx.top_authz_info().unwrap();
    assert!(info.inheritable);
    assert_eq!(info.user_id, "_NA_");
    assert_eq!(info.action.as_deref(), Some("AUTHZA_ALL"));
    push("viewer");
    ctx.grant_anonymous(AnonymousAccess::View, Some("someone"));
    let info = ctx.top_authz_info().unwrap();
    assert_eq!(info.user_id, "someone");
    assert_eq!(
        info.action.as_deref(),
        Some("AUTHZA_ALL"),
        "view below an all-actions grant keeps all"
    );
    ctx.pop().unwrap();
    ctx.pop().unwrap();
    push("alone");
    ctx.grant_anonymous(AnonymousAccess::View, None);
    assert_eq!(ctx.top_authz_info().unwrap().action.as_deref(), Some("AUTHZA_VIEW"));
    ctx.pop().unwrap();
}
