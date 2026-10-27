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

/// No more of a user's password checks run at once than the account has failures left, and a check that ends
/// frees its place; other users are not held by it.
#[test]
fn concurrent_checks_of_one_user_are_bounded_by_the_failures_left() {
    let ctx = ExecutionContextFactory::test_context();
    let _entered = ctx.enter();
    let first = LoginCheck::enter("ada", 2).expect("first");
    let second = LoginCheck::enter("ada", 2).expect("second");
    assert!(LoginCheck::enter("ada", 2).is_none());
    assert!(LoginCheck::enter("bob", 2).is_some());
    drop(first);
    let third = LoginCheck::enter("ada", 2).expect("a place came free");
    drop((second, third));
    // An account past its failures still gets one check, and an empty user id is not held.
    let one = LoginCheck::enter("ada", -3).expect("one");
    assert!(LoginCheck::enter("ada", -3).is_none());
    drop(one);
    assert!(LoginCheck::enter("", 0).is_some() && LoginCheck::enter("", 0).is_some());

    // A check past the bound waits for one to end, here on another thread.
    let held = LoginCheck::enter("ada", 1).expect("held");
    let factory = Arc::clone(ctx.factory());
    let started = std::time::Instant::now();
    let ender = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        drop(held);
        drop(factory);
    });
    LoginCheck::wait("ada");
    assert!(
        started.elapsed() < Duration::from_millis(900),
        "{:?}",
        started.elapsed()
    );
    assert!(LoginCheck::enter("ada", 1).is_some());
    ender.join().unwrap();
}

/// Concurrent logins of one user with the right password all succeed: those past the bound wait their turn.
/// Logins of one user at once write one history row: the minute is claimed before the worker counts the rows.
#[test]
fn concurrent_logins_of_one_user_leave_one_history_row() {
    let (ctx, facade) = open_ctx();
    create_user(&facade, "many", "Secret1!", |_| {});
    let factory = Arc::clone(ctx.factory());
    let start = Arc::new(std::sync::Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let factory = Arc::clone(&factory);
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                let context = factory.new_context();
                let _entered = context.enter();
                start.wait();
                login_user("many", "Secret1!").is_ok()
            })
        })
        .collect();
    for thread in threads {
        assert!(thread.join().unwrap());
    }
    let _guard = ctx.enter();
    let user_id = text(&load_user(&facade, "many"), "userId");
    let rows = facade
        .find(USER_LOGIN_HISTORY)
        .unwrap()
        .eq("userId", Value::string(&user_id))
        .list()
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn concurrent_logins_of_one_user_all_succeed() {
    let (ctx, facade) = open_ctx();
    create_user(&facade, "par", "Secret1!", |_| {});
    let factory = Arc::clone(ctx.factory());
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let factory = Arc::clone(&factory);
            std::thread::spawn(move || {
                let context = factory.new_context();
                let _entered = context.enter();
                login_user("par", "Secret1!").is_ok()
            })
        })
        .collect();
    for thread in threads {
        assert!(thread.join().unwrap());
    }
}

use std::time::{SystemTime, UNIX_EPOCH};

use moqui_lib::object::{basic_convert, Value};

use super::super::password::{passwords_match, simple_hash};
use super::super::session;

use super::*;
use crate::context::ExecutionContextFactory;
use crate::entity::EntityFacade;

#[test]
fn login_user_sets_current_username_and_logout_clears_it() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    session::add_flash_message("keep");
    assert_eq!(login_user("", "Secret1!").unwrap_err().to_string(), INVALID_LOGIN);
    assert_eq!(login_user("ada", "").unwrap_err().to_string(), INVALID_LOGIN);
    assert_eq!(
        login_user("missing", "Secret1!").unwrap_err().to_string(),
        INVALID_LOGIN
    );
    assert_eq!(session::flash_messages().unwrap(), vec!["keep".to_string()]);
    assert!(session::current_username().unwrap().is_none());

    login_anonymous().unwrap();
    assert!(session::current_username().unwrap().is_none());
    assert!(session::is_anonymous().unwrap());

    create_user(&facade, "ada", "Secret1!", |value| {
        value.set("locale", Value::string("en"));
        value.set("timeZone", Value::string("UTC"));
    });
    login_user("ada", "Secret1!").unwrap();
    assert_eq!(session::current_username().unwrap().as_deref(), Some("ada"));
    assert!(!session::is_anonymous().unwrap());
    assert_eq!(session::locale().unwrap().as_deref(), Some("en"));
    assert_eq!(session::time_zone().unwrap().as_deref(), Some("UTC"));
    assert_eq!(session::csrf_token().unwrap().len(), 40);
    assert_eq!(session::flash_messages().unwrap(), vec!["keep".to_string()]);

    logout().unwrap();
    assert!(session::current_username().unwrap().is_none());
    assert!(!session::is_anonymous().unwrap());
    let row = load_user(&facade, "ada");
    assert_eq!(text(&row, "hasLoggedOut"), "Y");
    session::reset_session();
}

#[test]
fn a_legacy_hash_verifies_and_the_stored_value_becomes_an_argon2_string() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    create_user(&facade, "ada", "Secret1!", |_| {});
    login_user("ada", "Secret1!").unwrap();
    let row = load_user(&facade, "ada");
    let stored = text(&row, "currentPassword");
    assert!(stored.starts_with("$argon2id$"), "{stored}");
    assert_eq!(text(&row, "passwordHashType"), "argon2");
    assert!(passwords_match("Secret1!", &stored, "argon2", "", false).unwrap());
    logout().unwrap();
    session::reset_session();
}

#[test]
fn a_count_the_driver_gives_as_a_decimal_is_its_number() {
    // Postgres reads a NUMERIC(20,0) column, such as successiveFailedLogins, as a decimal
    assert_eq!(value_i64(&Value::Decimal("3".parse().unwrap())), Some(3));
    assert_eq!(value_i64(&Value::Decimal("3.5".parse().unwrap())), None);
    assert_eq!(value_i64(&Value::I32(2)), Some(2));
}

#[test]
fn the_fourth_failed_login_disables_the_account_the_third_does_not_and_it_survives_rollback() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    session::add_flash_message("keep");
    create_user(&facade, "ada", "Secret1!", |_| {});
    crate::web::take_auth_failure();
    for _ in 0..3 {
        assert_eq!(login_user("ada", "wrong").unwrap_err().to_string(), INVALID_LOGIN);
        // noted for the web protection, once taken
        assert!(crate::web::take_auth_failure());
        assert!(!crate::web::take_auth_failure());
    }
    let before = load_user(&facade, "ada");
    assert_eq!(i64_field(&before, "successiveFailedLogins"), 3);
    assert_ne!(text(&before, "disabled"), "Y");

    facade.begin(USER_ACCOUNT, None).unwrap();
    let mut hinted = load_user(&facade, "ada");
    hinted.set("passwordHint", Value::string("changed"));
    hinted.update(&facade).unwrap();
    let failed = login_user("ada", "wrong");
    facade.rollback(USER_ACCOUNT).unwrap();
    assert_eq!(failed.unwrap_err().to_string(), INVALID_LOGIN);

    let after = load_user(&facade, "ada");
    assert_eq!(i64_field(&after, "successiveFailedLogins"), 4);
    assert_eq!(text(&after, "disabled"), "Y");
    assert!(!after.get("disabledDateTime").is_none_or(Value::is_null));
    assert_eq!(text(&after, "passwordHint"), "original");
    assert!(session::current_username().unwrap().is_none());
    assert_eq!(session::flash_messages().unwrap(), vec!["keep".to_string()]);
    session::reset_session();
}

#[test]
fn disabled_without_a_timestamp_stays_disabled_and_terminate_does_not_increment() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    create_user(&facade, "stuck", "Secret1!", |value| {
        value.set("disabled", Value::string("Y"));
    });
    assert_eq!(login_user("stuck", "Secret1!").unwrap_err().to_string(), INVALID_LOGIN);
    let stuck = load_user(&facade, "stuck");
    assert_eq!(text(&stuck, "disabled"), "Y");
    assert_eq!(i64_field(&stuck, "successiveFailedLogins"), 1);
    assert!(!text(&stuck, "currentPassword").starts_with("$argon2"));

    create_user(&facade, "gone", "Secret1!", |value| {
        value.set("terminateDate", stamp(1));
    });
    assert_eq!(login_user("gone", "Secret1!").unwrap_err().to_string(), INVALID_LOGIN);
    let gone = load_user(&facade, "gone");
    assert_eq!(i64_field(&gone, "successiveFailedLogins"), 0);
    assert!(!text(&gone, "currentPassword").starts_with("$argon2"));
    session::reset_session();
}

#[test]
fn a_correct_password_logs_in_after_the_disable_window() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    let opened = now_millis() - 6 * 60_000;
    create_user(&facade, "ada", "Secret1!", |value| {
        value.set("disabled", Value::string("Y"));
        value.set("disabledDateTime", stamp(opened));
        value.set("successiveFailedLogins", Value::I64(4));
    });
    login_user("ada", "Secret1!").unwrap();
    assert_eq!(session::current_username().unwrap().as_deref(), Some("ada"));
    let row = load_user(&facade, "ada");
    assert_eq!(text(&row, "disabled"), "N");
    assert!(row.get("disabledDateTime").is_none_or(Value::is_null));
    assert_eq!(i64_field(&row, "successiveFailedLogins"), 0);
    assert!(text(&row, "currentPassword").starts_with("$argon2id$"));
    logout().unwrap();
    session::reset_session();
}

#[test]
fn require_password_change_and_an_old_password_fail_without_rehash() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    create_user(&facade, "must", "Secret1!", |value| {
        value.set("requirePasswordChange", Value::string("Y"));
    });
    let err = login_user("must", "Secret1!").unwrap_err();
    assert!(err.to_string().contains("password change"), "{err}");
    assert_eq!(i64_field(&load_user(&facade, "must"), "successiveFailedLogins"), 0);
    assert!(!text(&load_user(&facade, "must"), "currentPassword").starts_with("$argon2"));

    let old = now_millis() - 13 * WEEK_MILLIS;
    create_user(&facade, "stale", "Secret1!", |value| {
        value.set("passwordSetDate", stamp(old));
    });
    let err = login_user("stale", "Secret1!").unwrap_err();
    assert!(err.to_string().contains("must be changed"), "{err}");
    assert!(!text(&load_user(&facade, "stale"), "currentPassword").starts_with("$argon2"));
    assert!(session::current_username().unwrap().is_none());
    session::reset_session();
}

#[test]
fn a_login_once_the_account_is_known_leaves_one_history_row_a_minute() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    let history = |user: &str| {
        facade
            .find(USER_LOGIN_HISTORY)
            .unwrap()
            .eq("userId", Value::string(user))
            .list()
            .unwrap()
    };
    create_user(&facade, "ada", "Secret1!", |_| {});
    assert_eq!(login_user("ada", "wrong").unwrap_err().to_string(), INVALID_LOGIN);
    let rows = history("ada");
    let row = rows.iter().next().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(text(row, "successfulLogin"), "N");
    // The password tried is never stored: the entity has no field for it.
    assert!(row.get("passwordUsed").is_none());
    // A second login within the minute is not kept.
    login_user("ada", "Secret1!").unwrap();
    assert_eq!(history("ada").len(), 1);
    logout().unwrap();

    create_user(&facade, "bob", "Secret1!", |_| {});
    login_user("bob", "Secret1!").unwrap();
    let rows = history("bob");
    let row = rows.iter().next().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(text(row, "successfulLogin"), "Y");
    logout().unwrap();

    // A right password that fails after the check is a failure.
    create_user(&facade, "cy", "Secret1!", |value| {
        value.set("requirePasswordChange", Value::string("Y"));
    });
    assert!(login_user("cy", "Secret1!").is_err());
    let rows = history("cy");
    let row = rows.iter().next().unwrap();
    assert_eq!(text(row, "successfulLogin"), "N");

    // An unknown or disabled account has no user to keep a row for.
    create_user(&facade, "stuck", "Secret1!", |value| {
        value.set("disabled", Value::string("Y"));
    });
    assert!(login_user("stuck", "Secret1!").is_err());
    assert!(login_user("nobody", "Secret1!").is_err());
    assert!(history("stuck").is_empty());
    assert!(history("nobody").is_empty());
    session::reset_session();
}

#[test]
fn a_socket_key_check_is_read_only_and_observes_expiry_revocation_and_user_changes() {
    let (ctx, facade) = open_ctx();
    let _entered = ctx.enter();
    create_user(&facade, "socket", "Secret1!", |_| {});
    internal_login_user("socket").unwrap();
    let user = super::session::current_user_id().unwrap().unwrap();
    let key = get_login_key(None).unwrap();
    let hash = password::hash_login_key(&key).unwrap();
    logout().unwrap();
    for _ in 0..3 {
        assert_eq!(login_key_user_by_hash(&hash).unwrap(), Some(user.clone()));
        assert!(super::session::current_user_id().unwrap().is_none());
    }
    assert!(facade.find(USER_LOGIN_HISTORY).unwrap().list().unwrap().is_empty());
    let mut row = facade.find(USER_LOGIN_KEY).unwrap().eq("loginKey", Value::string(&hash)).one().unwrap().unwrap();
    row.set("thruDate", stamp(now_millis() - 1000));
    row.update(&facade).unwrap();
    assert!(login_key_user_by_hash(&hash).unwrap().is_none());
    row.set("thruDate", Value::Null);
    row.update(&facade).unwrap();
    assert_eq!(login_key_user_by_hash(&hash).unwrap(), Some(user));
    row.delete(&facade).unwrap();
    assert!(login_key_user_by_hash(&hash).unwrap().is_none());
    session::reset_session();
}

#[test]
fn key_app_password_and_external_logins_leave_history_rows() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    let history = |user: &str| -> Vec<String> {
        facade
            .find(USER_LOGIN_HISTORY)
            .unwrap()
            .eq("userId", Value::string(user))
            .list()
            .unwrap()
            .iter()
            .map(|row| text(row, "successfulLogin"))
            .collect()
    };
    let user_id = |username: &str| text(&load_user(&facade, username), "userId");

    // a login key, good and expired
    create_user(&facade, "kim", "Secret1!", |_| {});
    create_user(&facade, "kit", "Secret1!", |_| {});
    internal_login_user("kim").unwrap();
    let key = get_login_key(None).unwrap();
    logout().unwrap();
    internal_login_user("kit").unwrap();
    let expired = get_login_key(Some(0.0)).unwrap();
    logout().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    assert!(history(&user_id("kim")).is_empty(), "an internal login is not recorded");
    login_user_key(&key).unwrap();
    logout().unwrap();
    assert_eq!(history(&user_id("kim")), ["Y"]);
    assert!(login_user_key(&expired).is_err());
    assert_eq!(history(&user_id("kit")), ["N"]);

    // an app password, good and revoked
    let app_password = |username: &str, revoked: bool| {
        let password = new_app_password().unwrap();
        let mut row = facade.make_value(USER_APP_PASSWORD).unwrap();
        row.set("appPasswordId", Value::string(format!("AP_{username}")));
        row.set("userId", Value::string(user_id(username)));
        row.set(
            "passwordHash",
            Value::string(password::hash_login_key(&app_password_key(&password)).unwrap()),
        );
        if revoked {
            row.set("revokedDate", stamp(now_millis() - 1000));
        }
        row.create(&facade).unwrap();
        password
    };
    create_user(&facade, "dav", "Secret1!", |_| {});
    create_user(&facade, "old", "Secret1!", |_| {});
    let good = app_password("dav", false);
    let revoked = app_password("old", true);
    login_app_password("dav", &good).unwrap();
    logout().unwrap();
    assert_eq!(history(&user_id("dav")), ["Y"]);
    assert!(login_app_password("old", &revoked).is_err());
    assert_eq!(history(&user_id("old")), ["N"]);

    // an identity provider login
    create_user(&facade, "sso", "Secret1!", |_| {});
    login_external_user("sso").unwrap();
    logout().unwrap();
    assert_eq!(history(&user_id("sso")), ["Y"]);

    // a disabled account the gate stops leaves no row, as for a password login
    create_user(&facade, "off", "Secret1!", |_| {});
    internal_login_user("off").unwrap();
    let off_key = get_login_key(None).unwrap();
    logout().unwrap();
    update_user(&facade, &user_id("off"), &[("disabled", Value::string("Y"))]).unwrap();
    assert!(login_user_key(&off_key).is_err());
    assert!(history(&user_id("off")).is_empty());
    session::reset_session();
}

#[test]
fn a_password_below_the_current_policy_still_logs_in() {
    // The policy applies when a password is set. Java does not check it at login, so a user
    // whose password predates the policy is not locked out.
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    create_user(&facade, "ada", "secret", |_| {});
    login_user("ada", "secret").unwrap();
    assert_eq!(session::current_username().unwrap().as_deref(), Some("ada"));
    assert!(text(&load_user(&facade, "ada"), "currentPassword").starts_with("$argon2"));
    session::reset_session();
}

#[test]
fn internal_login_skips_the_password_and_anonymous_does_not_set_a_username() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    create_user(&facade, "ada", "Secret1!", |_| {});
    internal_login_user("ada").unwrap();
    assert_eq!(session::current_username().unwrap().as_deref(), Some("ada"));
    assert!(!text(&load_user(&facade, "ada"), "currentPassword").starts_with("$argon2"));
    login_anonymous().unwrap();
    assert_eq!(session::current_username().unwrap().as_deref(), Some("ada"));
    logout().unwrap();
    login_anonymous().unwrap();
    assert!(session::current_username().unwrap().is_none());
    assert!(session::is_anonymous().unwrap());
    assert_eq!(
        internal_login_user("").unwrap_err().to_string(),
        "No username specified"
    );
    session::reset_session();
}

#[test]
fn change_password_stores_argon2_and_rejects_history() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    create_user(&facade, "ada", "Secret1!", |_| {});
    for millis in 1..=5 {
        let mut row = facade.make_value(PASSWORD_HISTORY).unwrap();
        row.set("userId", Value::string("ada"));
        row.set("fromDate", stamp(millis));
        row.set("password", Value::string("not-a-real-hash"));
        row.set("passwordSalt", Value::string("s4lt"));
        row.set("passwordHashType", Value::string("SHA-256"));
        row.create(&facade).unwrap();
    }
    change_password("ada", "Secret2!").unwrap();
    let account = load_user(&facade, "ada");
    let stored = text(&account, "currentPassword");
    assert!(stored.starts_with("$argon2id$"), "{stored}");
    assert_eq!(text(&account, "passwordHashType"), "argon2");
    let history = facade
        .find(PASSWORD_HISTORY)
        .unwrap()
        .eq("userId", Value::string("ada"))
        .order_by("fromDate", false)
        .list()
        .unwrap();
    assert_eq!(history.len(), 5);
    assert!(history.iter().all(|row| field_millis(row, "fromDate") != Some(1)));
    let current = change_password("ada", "Secret2!").unwrap_err();
    assert!(current.to_string().contains("current"), "{current}");
    let reused = change_password("ada", "Secret1!").unwrap_err();
    assert!(reused.to_string().contains("last"), "{reused}");
    session::reset_session();
}

#[test]
fn a_user_with_a_factor_or_a_requiring_group_is_pre_authenticated_not_logged_in() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    create_user(&facade, "ada", "Secret1!", |_| {});
    let mut factor = facade.make_value("moqui.security.UserAuthcFactor").unwrap();
    factor.set("factorId", Value::string("ADA_TOTP"));
    factor.set("userId", Value::string("ada"));
    factor.set("factorTypeEnumId", Value::string("UafTotp"));
    factor.set("factorOption", Value::string(super::super::factor::totp_secret()));
    factor.set("fromDate", stamp(1));
    factor.create(&facade).unwrap();
    login_user("ada", "Secret1!").unwrap();
    assert!(session::current_username().unwrap().is_none());
    assert_eq!(session::session_attribute(PRE_AUTHC_USERNAME).as_deref(), Some("ada"));
    assert!(session::session_attribute_flag(AUTHC_FACTOR_REQUIRED));
    // The password was right, so a legacy hash is replaced as on a full login.
    assert!(text(&load_user(&facade, "ada"), "currentPassword").starts_with("$argon2id$"));
    // A wrong password does not pre-authenticate, and clears the earlier attempt.
    assert_eq!(login_user("ada", "wrong").unwrap_err().to_string(), INVALID_LOGIN);
    assert_eq!(session::session_attribute(PRE_AUTHC_USERNAME), None);
    // The factor is the second step: the internal login after a verified code sets the user.
    internal_login_user("ada").unwrap();
    assert_eq!(session::current_username().unwrap().as_deref(), Some("ada"));
    logout().unwrap();

    create_user(&facade, "bob", "Secret1!", |_| {});
    let mut group = facade.make_value("moqui.security.UserGroup").unwrap();
    group.set("userGroupId", Value::string("STRICT"));
    group.set("requireAuthcFactor", Value::string("Y"));
    group.create(&facade).unwrap();
    assert!(!super::super::factor::second_factor_required("bob").unwrap());
    let mut member = facade.make_value("moqui.security.UserGroupMember").unwrap();
    member.set("userGroupId", Value::string("STRICT"));
    member.set("userId", Value::string("bob"));
    member.set("fromDate", stamp(1));
    member.create(&facade).unwrap();
    assert!(super::super::factor::second_factor_required("bob").unwrap());
    login_user("bob", "Secret1!").unwrap();
    assert!(session::current_username().unwrap().is_none());
    assert_eq!(session::session_attribute(PRE_AUTHC_USERNAME).as_deref(), Some("bob"));
    session::reset_session();
}

#[test]
fn ip_allowed_on_the_account_or_a_group_refuses_another_client_ip() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    let _web = crate::web::install(std::collections::BTreeMap::new());
    let from = |address: &str| {
        let mut request = crate::web::WebRequest::new("POST", "/Login/login");
        request.peer_ip = address.to_string();
        crate::web::set_request_info(&request);
    };
    create_user(&facade, "ada", "Secret1!", |value| {
        value.set("ipAllowed", Value::string("10.1.*.*"));
    });
    from("10.1.2.3");
    login_user("ada", "Secret1!").unwrap();
    assert_eq!(session::current_username().unwrap().as_deref(), Some("ada"));
    logout().unwrap();
    from("192.168.0.4");
    let refused = login_user("ada", "Secret1!").unwrap_err().to_string();
    assert!(refused.contains("not in allowed list"), "{refused}");
    assert!(session::current_username().unwrap().is_none());
    // Not a failed password: the count is not raised.
    assert_eq!(i64_field(&load_user(&facade, "ada"), "successiveFailedLogins"), 0);
    // The internal login after a second-factor code is checked as well.
    assert!(internal_login_user("ada").is_err());

    // A group the user is in adds its list; any one matching allows.
    create_user(&facade, "bob", "Secret1!", |_| {});
    let mut group = facade.make_value("moqui.security.UserGroup").unwrap();
    group.set("userGroupId", Value::string("OFFICE"));
    group.set("ipAllowed", Value::string("192.168.0.1-5"));
    group.create(&facade).unwrap();
    let mut member = facade.make_value("moqui.security.UserGroupMember").unwrap();
    member.set("userGroupId", Value::string("OFFICE"));
    member.set("userId", Value::string("bob"));
    member.set("fromDate", stamp(1));
    member.create(&facade).unwrap();
    login_user("bob", "Secret1!").unwrap();
    logout().unwrap();
    from("192.168.0.9");
    assert!(login_user("bob", "Secret1!").is_err());
    // The account's own list matches although the group's does not.
    let mut bob = load_user(&facade, "bob");
    bob.set("ipAllowed", Value::string("192.168.0.9"));
    bob.update(&facade).unwrap();
    login_user("bob", "Secret1!").unwrap();
    logout().unwrap();
    session::reset_session();
}

#[test]
fn an_api_key_logs_in_until_it_is_revoked_or_expires() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    assert!(get_login_key(None).is_err(), "a key needs a logged-in user");
    create_user(&facade, "ada", "Secret1!", |_| {});
    internal_login_user("ada").unwrap();
    let key = get_login_key(None).unwrap();
    assert_eq!(key.len(), 40);
    // A key that ends now is kept, and is expired a moment later.
    let short = get_login_key(Some(0.0)).unwrap();
    logout().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));

    let stored = facade
        .find(USER_LOGIN_KEY)
        .unwrap()
        .eq("userId", Value::string("ada"))
        .list()
        .unwrap();
    assert_eq!(stored.len(), 2);
    let hashed = password::hash_login_key(&key).unwrap();
    let row = stored
        .iter()
        .find(|row| text(row, "loginKey") == hashed)
        .expect("only the hash is stored");
    let thru = field_millis(row, "thruDate").unwrap() - field_millis(row, "fromDate").unwrap();
    assert_eq!(thru, 144 * 60 * 60 * 1000, "the conf default is six days");

    apply_header_keys(Some(&key), None).unwrap();
    assert_eq!(session::current_username().unwrap().as_deref(), Some("ada"));
    logout().unwrap();
    assert_eq!(login_user_key(&short).unwrap_err().to_string(), INVALID_LOGIN);

    // Revoke: the admin deletes the key's row.
    row.delete(&facade).unwrap();
    assert_eq!(
        apply_header_keys(Some(&key), None).unwrap_err().to_string(),
        INVALID_LOGIN
    );
    assert!(session::current_username().unwrap().is_none());
    session::reset_session();
}

#[test]
fn a_service_call_with_credentials_puts_the_callers_user_back() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    create_user(&facade, "ada", "Secret1!", |value| {
        value.set("locale", Value::string("en"));
    });
    create_user(&facade, "bob", "Secret2!", |value| {
        value.set("locale", Value::string("fr"));
    });
    let path = std::env::temp_dir().join(format!("moqui-login-local-{}.xml", std::process::id()));
    std::fs::write(
        &path,
        r#"<services>
                <service verb="who" noun="Am" authenticate="true" transaction="ignore">
                    <out-parameters><parameter name="who"/></out-parameters>
                    <actions><set field="who" from="ec.user.username"/></actions>
                </service>
            </services>"#,
    )
    .unwrap();
    let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    login_user("ada", "Secret1!").unwrap();
    let token = session::csrf_token().unwrap();
    let mut parameters = std::collections::BTreeMap::new();
    parameters.insert("authUsername".to_string(), Value::string("bob"));
    parameters.insert("authPassword".to_string(), Value::string("Secret2!"));
    let out = crate::service::sync("who#Am", &parameters).unwrap();
    assert_eq!(out.get("who").and_then(Value::as_str), Some("bob"));
    assert_eq!(session::current_username().unwrap().as_deref(), Some("ada"));
    assert_eq!(session::current_user_id().unwrap().as_deref(), Some("ada"));
    assert_eq!(session::locale().unwrap().as_deref(), Some("en"));
    assert_eq!(session::csrf_token().unwrap(), token);

    parameters.insert("authPassword".to_string(), Value::string("wrong"));
    assert!(crate::service::sync("who#Am", &parameters).unwrap().is_empty());
    assert_eq!(session::current_username().unwrap().as_deref(), Some("ada"));
}

fn open_ctx() -> (crate::context::ExecutionContext, EntityFacade) {
    session::reset_session();
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    (ctx, facade)
}

fn create_user(facade: &EntityFacade, username: &str, password: &str, fill: impl FnOnce(&mut EntityValue)) {
    let stored = simple_hash(password, "s4lt", "SHA-256", false).unwrap();
    let mut value = facade.make_value(USER_ACCOUNT).unwrap();
    value.set("userId", Value::string(username));
    value.set("username", Value::string(username));
    value.set("currentPassword", Value::string(stored));
    value.set("passwordSalt", Value::string("s4lt"));
    value.set("passwordHashType", Value::string("SHA-256"));
    value.set("passwordBase64", Value::string("N"));
    value.set("passwordHint", Value::string("original"));
    fill(&mut value);
    value.create(facade).unwrap();
}

fn load_user(facade: &EntityFacade, username: &str) -> EntityValue {
    facade
        .find(USER_ACCOUNT)
        .unwrap()
        .eq("username", Value::string(username))
        .one()
        .unwrap()
        .unwrap_or_else(|| panic!("missing {username}"))
}

fn text(value: &EntityValue, field: &str) -> String {
    value.get(field).and_then(Value::as_str).unwrap_or("").to_string()
}

fn i64_field(value: &EntityValue, field: &str) -> i64 {
    match value.get(field) {
        Some(Value::I64(number)) => *number,
        Some(Value::I32(number)) => i64::from(*number),
        _ => 0,
    }
}

fn stamp(millis: i64) -> Value {
    basic_convert(&Value::I64(millis), "java.sql.Timestamp").value
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}
