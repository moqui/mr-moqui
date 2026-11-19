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

/// SEC-WEB-87: a second factor's code that counts as a login is compared behind the password's gate. With one failure
/// left and that place held, even the right code is counted and not compared, and locks the account. Codes sent at
/// once lock the account.
#[test]
fn codes_sent_at_once_are_gated_and_each_counted() {
    session::reset_session();
    let ctx = ExecutionContextFactory::test_context_in(
        "",
        crate::service::ServicePools::bounded(2, 16),
        crate::context::framework_dir(),
    );
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    {
        let _authz = ctx.disable_authz();
        crate::service::load_definitions().unwrap();
    }
    create_user(&facade, "codes", "Secret1!", |value| {
        value.set("successiveFailedLogins", Value::I64(policy::max_failures()));
    });
    let mut factor = facade.make_value("moqui.security.UserAuthcFactor").unwrap();
    factor.set("factorId", Value::string("CODES_1"));
    factor.set("userId", Value::string("codes"));
    factor.set("factorTypeEnumId", Value::string("UafSingleUse"));
    factor.set(
        "factorOption",
        Value::string(super::super::factor::single_use_hash("70316001").unwrap()),
    );
    factor.set("fromDate", stamp(now_millis() - 60_000));
    factor.create(&facade).unwrap();
    let factory = Arc::clone(ctx.factory());
    // a code of the pre-authenticated user, as `/rest/verifyOtp` and `Login/login` send it
    let send = move |code: &'static str| {
        let factory = Arc::clone(&factory);
        std::thread::spawn(move || {
            let context = factory.new_context();
            let _entered = context.enter();
            let _web = crate::web::install(BTreeMap::new());
            session::set_session_attribute(PRE_AUTHC_USERNAME, Some("codes"));
            let parameters = BTreeMap::from([("code".to_string(), Value::string(code))]);
            let out = crate::service::sync(super::super::factor::VALIDATE_CODE, &parameters).unwrap();
            let errors = context.message().errors().join("; ");
            assert!(errors.is_empty(), "{errors}");
            out.get("verified") == Some(&Value::Bool(true))
        })
    };

    // the one place held by a code being checked: the right code past it is counted and not compared, which locks
    let held = LoginCheck::enter("codes", 1).expect("the one place");
    assert!(
        !send("70316001").join().unwrap(),
        "a code past the failures left is not compared"
    );
    drop(held);
    let account = load_user(&facade, "codes");
    assert_eq!(
        i64_field(&account, "successiveFailedLogins"),
        policy::max_failures() + 1
    );
    assert_eq!(
        text(&account, "disabled"),
        "Y",
        "the code past the failures left locked the account"
    );

    // unlocked, twelve codes at once, the right one among them: each refused one counted once, and the account locked
    update_user(
        &facade,
        "codes",
        &[
            ("successiveFailedLogins", Value::I64(0)),
            ("disabled", Value::string("N")),
            ("disabledDateTime", Value::Null),
        ],
    )
    .unwrap();
    let mut burst: Vec<_> = (0..11).map(|_| send("22222222")).collect();
    burst.push(send("70316001"));
    let verified = burst
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .filter(|verified| *verified)
        .count();
    let account = load_user(&facade, "codes");
    let counted = i64_field(&account, "successiveFailedLogins");
    // The test database's transactions each work on a snapshot with no row lock, so two counts committed together can
    // still keep one; on Postgres the update waits for the other and counts again (write_failed_increment), which the
    // security proof V6Authentication 6.6.3 shows. Here: no more than the failures left were compared, and it locked.
    assert_eq!(
        text(&account, "disabled"),
        "Y",
        "counted {counted}, verified {verified}"
    );
    assert!(counted > policy::max_failures(), "counted {counted}");
}

/// SEC-WEB-87: a factor's code is taken by one conditional update: a single-use code while it has not ended, an
/// authenticator factor's step only after the last one it took. Two right submissions at once are the host test
/// `second_factor_codes` on Postgres, as this test database has no row locks.
#[test]
fn a_code_is_taken_by_a_conditional_update() {
    session::reset_session();
    let ctx = ExecutionContextFactory::test_context_in(
        "",
        crate::service::ServicePools::bounded(2, 16),
        crate::context::framework_dir(),
    );
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    {
        let _authz = ctx.disable_authz();
        crate::service::load_definitions().unwrap();
    }
    create_user(&facade, "twice", "Secret1!", |_| {});
    for (factor_id, kind) in [("TWICE_1", "UafSingleUse"), ("TWICE_T", "UafTotp")] {
        let mut factor = facade.make_value("moqui.security.UserAuthcFactor").unwrap();
        factor.set("factorId", Value::string(factor_id));
        factor.set("userId", Value::string("twice"));
        factor.set("factorTypeEnumId", Value::string(kind));
        factor.set(
            "factorOption",
            Value::string(super::super::factor::single_use_hash("70316001").unwrap()),
        );
        factor.set("fromDate", stamp(now_millis() - 60_000));
        // a printed code is in date for a year
        factor.set("thruDate", stamp(now_millis() + 365 * 86_400_000));
        factor.create(&facade).unwrap();
    }
    assert_eq!(
        check_counted_code("twice", "70316001").unwrap().as_deref(),
        Some("TWICE_1")
    );
    assert_eq!(
        check_counted_code("twice", "70316001").unwrap(),
        None,
        "the code was taken"
    );
    // two takes that read the same thruDate, the second with an earlier now (a request that began first and was held
    // on the row lock): the second matches nothing once the first has ended the code
    let read = facade
        .find(USER_AUTHC_FACTOR)
        .unwrap()
        .eq("factorId", Value::string("TWICE_T"))
        .one()
        .unwrap()
        .and_then(|row| row.get("thruDate").cloned())
        .expect("a thruDate a year ahead");
    let tx = facade.transactions_for(USER_AUTHC_FACTOR).unwrap();
    let now = now_millis();
    assert!(take_single_use(&facade, &tx, "TWICE_T", &read, now).unwrap());
    assert!(
        !take_single_use(&facade, &tx, "TWICE_T", &read, now - 5_000).unwrap(),
        "the later take of the same read"
    );
    let ended = facade
        .find(USER_AUTHC_FACTOR)
        .unwrap()
        .eq("factorId", Value::string("TWICE_T"))
        .one()
        .unwrap()
        .and_then(|row| row.get("thruDate").and_then(value_millis))
        .expect("ended");
    assert_eq!(ended, now);
    assert!(
        !take_factor_code("TWICE_1", None).unwrap(),
        "a single-use code ended is not taken again"
    );
    assert!(take_factor_code("TWICE_T", Some(5)).unwrap());
    assert!(!take_factor_code("TWICE_T", Some(5)).unwrap(), "the same step");
    assert!(!take_factor_code("TWICE_T", Some(4)).unwrap(), "an earlier step");
    assert!(take_factor_code("TWICE_T", Some(6)).unwrap());
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

/// A job's borrowed sign-in (internal login, then `logout_local`) writes nothing to the account: a person's
/// `hasLoggedOut` stays as it was, set or not, so a job neither ends nor brings back the person's sessions; a person's
/// own sign-in clears it, and `logout` sets it.
#[test]
fn a_jobs_login_and_logout_local_leave_has_logged_out_alone() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    create_user(&facade, "job.user", "Secret1!", |_| {});
    let flag = || text(&load_user(&facade, "job.user"), "hasLoggedOut");
    let before = flag();
    internal_login_user("job.user").unwrap();
    assert_eq!(session::current_username().unwrap().as_deref(), Some("job.user"));
    logout_local().unwrap();
    assert!(session::current_username().unwrap().is_none());
    assert_eq!(flag(), before, "not marked logged out");

    login_user("job.user", "Secret1!").unwrap();
    logout().unwrap();
    assert_eq!(flag(), "Y");
    internal_login_user("job.user").unwrap();
    logout_local().unwrap();
    assert_eq!(flag(), "Y", "a job's login does not clear it");
    login_user("job.user", "Secret1!").unwrap();
    assert_eq!(flag(), "N", "a person's sign-in clears it");
    logout_local().unwrap();
    session::reset_session();
}

/// A person's sign-in by an SSH key the host checked clears `hasLoggedOut`, as a password does; the session's
/// operations after it (an internal login each, ended by `logout_local`) leave it as it is.
#[test]
fn an_ssh_key_sign_in_is_a_persons_and_clears_has_logged_out() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    create_user(&facade, "key.user", "Secret1!", |_| {});
    let flag = || text(&load_user(&facade, "key.user"), "hasLoggedOut");
    login_user("key.user", "Secret1!").unwrap();
    logout().unwrap();
    assert_eq!(flag(), "Y");
    login_ssh_key_user("key.user", "").unwrap();
    assert_eq!(session::current_username().unwrap().as_deref(), Some("key.user"));
    assert_eq!(flag(), "N", "a key sign-in is a person's");
    logout_local().unwrap();
    assert_eq!(flag(), "N", "an operation's borrowed login ends without a sign-out");
    session::reset_session();
}

/// SEC-FIL-89: an SSH key sign-in checks the account's ipAllowed against the SSH peer, as the SSH password login does,
/// and is recorded in the login history.
#[test]
fn an_ssh_key_sign_in_checks_ip_allowed_and_is_recorded() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    create_user(&facade, "ssh.key", "Secret1!", |value| {
        value.set("ipAllowed", Value::string("10.1.*.*"));
    });
    assert!(login_ssh_key_user("ssh.key", "192.0.2.10").is_err());
    assert!(session::current_username().unwrap().is_none());
    login_ssh_key_user("ssh.key", "10.1.2.3").unwrap();
    assert_eq!(session::current_username().unwrap().as_deref(), Some("ssh.key"));
    logout_local().unwrap();
    let history = facade
        .find(USER_LOGIN_HISTORY)
        .unwrap()
        .eq("userId", Value::string("ssh.key"))
        .list()
        .unwrap();
    assert!(!history.is_empty(), "a key sign-in leaves a history row");
    session::reset_session();
}

/// SEC-WEB-06: a request by a login key or an app password after a sign-out elsewhere signs the request in but leaves
/// `hasLoggedOut` set, so a session that made no request since the sign-out still ends at its next one; only an
/// interactive sign-in (a password, an identity provider) clears it.
#[test]
fn a_key_or_app_password_login_leaves_has_logged_out_set() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    create_user(&facade, "race.user", "Secret1!", |_| {});
    let user_id = text(&load_user(&facade, "race.user"), "userId");
    let flag = || text(&load_user(&facade, "race.user"), "hasLoggedOut");
    internal_login_user("race.user").unwrap();
    let key = get_login_key(None).unwrap();
    logout_local().unwrap();
    let password = new_app_password().unwrap();
    let mut row = facade.make_value(USER_APP_PASSWORD).unwrap();
    row.set("appPasswordId", Value::string("AP_race"));
    row.set("userId", Value::string(&user_id));
    row.set(
        "passwordHash",
        Value::string(password::hash_login_key(&app_password_key(&password)).unwrap()),
    );
    row.create(&facade).unwrap();

    login_user("race.user", "Secret1!").unwrap();
    logout().unwrap();
    assert_eq!(flag(), "Y");
    login_user_key(&key).unwrap();
    assert_eq!(session::current_username().unwrap().as_deref(), Some("race.user"));
    logout_local().unwrap();
    assert_eq!(flag(), "Y", "a login key does not undo the sign-out");
    login_app_password("race.user", &password, "dav").unwrap();
    logout_local().unwrap();
    assert_eq!(flag(), "Y", "an app password does not undo the sign-out");
    login_external_user("race.user", None).unwrap();
    assert_eq!(flag(), "N", "an identity provider's sign-in is a person's");
    logout().unwrap();
    login_user("race.user", "Secret1!").unwrap();
    assert_eq!(flag(), "N", "a password sign-in clears it");
    logout_local().unwrap();
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

/// SEC-WEB-01: failed logins on an account disabled for good never give it a lock time, which would end by itself.
#[test]
fn failed_logins_do_not_turn_a_permanent_disable_into_a_timed_lock() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    create_user(&facade, "stuck", "Secret1!", |value| {
        value.set("disabled", Value::string("Y"));
    });
    for _ in 0..6 {
        assert_eq!(login_user("stuck", "wrong").unwrap_err().to_string(), INVALID_LOGIN);
    }
    let stuck = load_user(&facade, "stuck");
    assert_eq!(text(&stuck, "disabled"), "Y");
    assert!(stuck.get("disabledDateTime").is_none_or(Value::is_null));
    assert_eq!(i64_field(&stuck, "successiveFailedLogins"), 6);
    assert_eq!(login_user("stuck", "Secret1!").unwrap_err().to_string(), INVALID_LOGIN);
    session::reset_session();
}

/// SEC-ADM-01: a password change ends a tarpit lock but leaves an account disabled for good disabled.
#[test]
fn a_password_change_keeps_a_permanent_disable_and_ends_a_timed_lock() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    create_user(&facade, "stuck", "Secret1!", |value| {
        value.set("disabled", Value::string("Y"));
    });
    change_password("stuck", "Secret2!").unwrap();
    let stuck = load_user(&facade, "stuck");
    assert_eq!(text(&stuck, "disabled"), "Y");
    assert_eq!(login_user("stuck", "Secret2!").unwrap_err().to_string(), INVALID_LOGIN);

    create_user(&facade, "locked", "Secret1!", |value| {
        value.set("disabled", Value::string("Y"));
        value.set("disabledDateTime", stamp(now_millis()));
    });
    change_password("locked", "Secret2!").unwrap();
    assert_eq!(text(&load_user(&facade, "locked"), "disabled"), "N");
    session::reset_session();
}

/// SEC-WEB-03, SEC-COM-12: a password tried during a lock window counts but does not restart the window, and an
/// internal login (an XMPP service call, queued work) or a valid key tried no password and counts nothing, nor is it
/// noted for the web protection.
#[test]
fn attempts_during_a_lock_window_do_not_restart_it_and_internal_logins_do_not_count() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    let locked_at = now_millis() - 2 * 60_000;
    create_user(&facade, "ada", "Secret1!", |_| {});
    internal_login_user("ada").unwrap();
    let key = get_login_key(None).unwrap();
    logout().unwrap();
    update_user(
        &facade,
        "ada",
        &[
            ("disabled", Value::string("Y")),
            ("disabledDateTime", stamp(locked_at)),
            ("successiveFailedLogins", Value::I64(4)),
        ],
    )
    .unwrap();
    crate::web::take_auth_failure();

    assert_eq!(login_user("ada", "wrong").unwrap_err().to_string(), INVALID_LOGIN);
    assert!(crate::web::take_auth_failure());
    let after_password = load_user(&facade, "ada");
    assert_eq!(i64_field(&after_password, "successiveFailedLogins"), 5);
    assert_eq!(field_millis(&after_password, "disabledDateTime"), Some(locked_at));

    for _ in 0..3 {
        assert_eq!(internal_login_user("ada").unwrap_err().to_string(), INVALID_LOGIN);
        assert_eq!(internal_login_queued_user("ada").unwrap_err().to_string(), INVALID_LOGIN);
        assert_eq!(login_user_key(&key).unwrap_err().to_string(), INVALID_LOGIN);
    }
    assert!(!crate::web::take_auth_failure(), "a valid key is not a failed credential");
    let after = load_user(&facade, "ada");
    assert_eq!(i64_field(&after, "successiveFailedLogins"), 5);
    assert_eq!(field_millis(&after, "disabledDateTime"), Some(locked_at));
    assert!(session::current_username().unwrap().is_none());

    // once the window is over, the next wrong password starts a new one
    update_user(&facade, "ada", &[("disabledDateTime", stamp(now_millis() - 6 * 60_000))]).unwrap();
    assert_eq!(login_user("ada", "wrong").unwrap_err().to_string(), INVALID_LOGIN);
    let relocked = field_millis(&load_user(&facade, "ada"), "disabledDateTime").unwrap();
    assert!(relocked > now_millis() - 60_000);
    session::reset_session();
}

/// SEC-RTE-07: a request's own credentials name their user before the body is read, with no login: Basic with the
/// account password (only where the caller takes it), and nothing for wrong or missing ones.
#[test]
fn credentials_name_their_user_without_a_login() {
    use base64::Engine as _;
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    create_user(&facade, "ada", "Secret1!", |_| {});
    let basic = |password: &str| {
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("ada:{password}"))
        )
    };
    let right = basic("Secret1!");
    assert_eq!(credential_user(None, None, Some(&right), true, None).unwrap().as_deref(), Some("ada"));
    assert_eq!(credential_user(None, None, Some(&right), false, None).unwrap(), None);
    assert_eq!(credential_user(None, None, Some(&basic("wrong")), true, None).unwrap(), None);
    assert!(crate::web::take_auth_failure(), "a wrong password counts, as a login with it would");
    assert_eq!(credential_user(None, None, None, true, None).unwrap(), None);
    assert_eq!(credential_user(Some("no-such-key"), None, None, true, None).unwrap(), None);
    assert!(session::current_username().unwrap().is_none(), "nobody is logged in");
}

/// SEC-WEB-02, SEC-ADM-02: the old password of a password change is behind the login gate. A wrong one counts and is
/// noted; during the lock window even the right one is refused, counted and noted; an unknown user is refused; a reset
/// password in date is taken, an ended one is not.
#[test]
fn the_old_password_of_a_change_is_behind_the_login_gate() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    create_user(&facade, "ada", "Secret1!", |_| {});
    crate::web::take_auth_failure();
    assert!(check_old_password("ada", "Secret1!").unwrap());
    assert!(!crate::web::take_auth_failure());
    for _ in 0..4 {
        assert!(!check_old_password("ada", "wrong").unwrap());
        assert!(crate::web::take_auth_failure());
    }
    let locked = load_user(&facade, "ada");
    assert_eq!(text(&locked, "disabled"), "Y");
    assert_eq!(i64_field(&locked, "successiveFailedLogins"), 4);
    assert!(!check_old_password("ada", "Secret1!").unwrap(), "the lock window refuses the right one");
    assert!(crate::web::take_auth_failure());
    assert_eq!(i64_field(&load_user(&facade, "ada"), "successiveFailedLogins"), 5);

    assert!(!check_old_password("nobody", "Secret1!").unwrap());
    assert!(!check_old_password("", "Secret1!").unwrap());
    assert!(crate::web::take_auth_failure());

    let reset_hash = hash_password("Reset-1234").unwrap();
    create_user(&facade, "bob", "Secret1!", |value| {
        value.set("resetPassword", Value::string(&reset_hash));
        value.set("resetPasswordSetDate", stamp(now_millis()));
    });
    assert!(check_old_password("bob", "Reset-1234").unwrap());
    update_user(&facade, "bob", &[("resetPasswordSetDate", stamp(now_millis() - 49 * 3_600_000))]).unwrap();
    assert!(!check_old_password("bob", "Reset-1234").unwrap());
    assert_eq!(i64_field(&load_user(&facade, "bob"), "successiveFailedLogins"), 1);
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
    let mut row = facade
        .find(USER_LOGIN_KEY)
        .unwrap()
        .eq("loginKey", Value::string(&hash))
        .one()
        .unwrap()
        .unwrap();
    row.set("thruDate", stamp(now_millis() - 1000));
    row.update(&facade).unwrap();
    assert!(login_key_user_by_hash(&hash).unwrap().is_none());
    row.set("thruDate", Value::Null);
    row.update(&facade).unwrap();
    assert_eq!(login_key_user_by_hash(&hash).unwrap(), Some(user));
    create_user(&facade, "socket-other", "Secret1!", |_| {});
    let other = text(&load_user(&facade, "socket-other"), "userId");
    row.set("userId", Value::string(&other));
    row.update(&facade).unwrap();
    assert_eq!(login_key_user_by_hash(&hash).unwrap(), Some(other));
    let mut account = load_user(&facade, "socket-other");
    account.set("disabled", Value::string("Y"));
    account.update(&facade).unwrap();
    assert!(login_key_user_by_hash(&hash).unwrap().is_none());
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
    login_app_password("dav", &good, "dav").unwrap();
    logout().unwrap();
    assert_eq!(history(&user_id("dav")), ["Y"]);
    assert!(login_app_password("old", &revoked, "dav").is_err());
    assert_eq!(history(&user_id("old")), ["N"]);

    // an identity provider login
    create_user(&facade, "sso", "Secret1!", |_| {});
    login_external_user("sso", None).unwrap();
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

/// SEC-COM-11, SEC-COM-22: an XMPP SCRAM or FAST sign-in by an app password the session verified checks the account's
/// ipAllowed against the session's address, and is recorded in the login history with the app password's last use;
/// another user's or a revoked app password is refused.
/// SEC-COM-21: an app password made for one protocol is taken by that protocol's login only, the check before a body
/// too; one made before scopes is taken by each; an XMPP sign-in the protocol verified itself takes one made for XMPP
/// or for none.
#[test]
fn an_app_password_is_taken_only_by_the_protocol_it_was_made_for() {
    use base64::Engine as _;
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    create_user(&facade, "pat", "Secret1!", |_| {});
    let made = |id: &str, scope: &str| {
        let password = new_app_password().unwrap();
        let mut row = facade.make_value(USER_APP_PASSWORD).unwrap();
        row.set("appPasswordId", Value::string(id));
        row.set("userId", Value::string("pat"));
        row.set(
            "passwordHash",
            Value::string(password::hash_login_key(&app_password_key(&password)).unwrap()),
        );
        if !scope.is_empty() {
            row.set("scope", Value::string(scope));
        }
        row.create(&facade).unwrap();
        password
    };
    let dav = made("AP_DAV", "dav");
    let any = made("AP_ANY", "");
    for scope in super::APP_PASSWORD_SCOPES {
        assert_eq!(
            login_app_password("pat", &dav, scope).is_ok(),
            scope == "dav",
            "{scope}"
        );
        logout().unwrap();
        assert!(
            login_app_password("pat", &any, scope).is_ok(),
            "one with no scope, by {scope}"
        );
        logout().unwrap();
    }
    let basic = |password: &str| {
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("pat:{password}"))
        )
    };
    let pat = Some("pat".to_string());
    assert_eq!(
        credential_user(None, None, Some(&basic(&dav)), false, Some("dav")).unwrap(),
        pat
    );
    assert_eq!(
        credential_user(None, None, Some(&basic(&dav)), false, Some("ssh")).unwrap(),
        None
    );
    assert_eq!(
        credential_user(None, None, Some(&basic(&dav)), false, None).unwrap(),
        pat,
        "any, for a check before the login"
    );
    assert!(
        login_verified_app_password("pat", Some("AP_DAV")).is_err(),
        "not made for XMPP"
    );
    logout().unwrap();
    login_verified_app_password("pat", Some("AP_ANY")).unwrap();
    logout().unwrap();
}

#[test]
fn a_verified_app_password_sign_in_checks_ip_allowed_and_is_recorded() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    create_user(&facade, "xmpp", "Secret1!", |value| {
        value.set("ipAllowed", Value::string("10.1.*.*"));
    });
    create_user(&facade, "other", "Secret1!", |_| {});
    let mut row = facade.make_value(USER_APP_PASSWORD).unwrap();
    row.set("appPasswordId", Value::string("AP_XMPP"));
    row.set("userId", Value::string("xmpp"));
    row.set("passwordHash", Value::string("unused"));
    row.create(&facade).unwrap();
    let last_used = || {
        facade
            .find(USER_APP_PASSWORD)
            .unwrap()
            .eq("appPasswordId", Value::string("AP_XMPP"))
            .one()
            .unwrap()
            .and_then(|row| field_millis(&row, "lastUsedDate"))
    };

    with_client_ip("10.1.2.3", || login_verified_app_password("xmpp", Some("AP_XMPP"))).unwrap();
    assert_eq!(session::current_username().unwrap().as_deref(), Some("xmpp"));
    logout().unwrap();
    assert!(last_used().is_some());
    let history = facade
        .find(USER_LOGIN_HISTORY)
        .unwrap()
        .eq("userId", Value::string("xmpp"))
        .list()
        .unwrap();
    assert!(history.iter().any(|row| text(row, "successfulLogin") == "Y"));

    let refused = with_client_ip("192.0.2.10", || login_verified_app_password("xmpp", Some("AP_XMPP")));
    assert!(refused.unwrap_err().to_string().contains("not in allowed list"));
    assert!(session::current_username().unwrap().is_none());

    assert_eq!(
        with_client_ip("10.1.2.3", || login_verified_app_password("other", Some("AP_XMPP")))
            .unwrap_err()
            .to_string(),
        INVALID_LOGIN
    );
    update_user(&facade, "xmpp", &[("ipAllowed", Value::Null)]).unwrap();
    let mut revoked = facade.make_value(USER_APP_PASSWORD).unwrap();
    revoked.set("appPasswordId", Value::string("AP_XMPP"));
    revoked.set("revokedDate", stamp(now_millis() - 1000));
    revoked.update(&facade).unwrap();
    assert!(login_verified_app_password("xmpp", Some("AP_XMPP")).is_err());
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

/// A password login is a recent sign-in, an internal login is not, and the code that finishes a second factor is; the
/// sign-in is the user's own (another user's session does not take it), and a logout drops it.
#[test]
fn a_password_login_and_a_finished_second_factor_are_recent_sign_ins_an_internal_login_is_not() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    create_user(&facade, "ada", "Secret1!", |_| {});
    create_user(&facade, "bob", "Secret1!", |_| {});
    internal_login_user("ada").unwrap();
    assert!(!recent_auth(300_000));
    assert_eq!(last_auth_millis(), None);
    login_user("ada", "Secret1!").unwrap();
    assert!(recent_auth(300_000));
    let at = last_auth_millis().unwrap();
    assert!((now_millis() - at).abs() < 5_000);
    assert!(!recent_auth(-1));
    // a local login (a service run with credentials) leaves the session user's sign-in as it was
    drop(login_user_local("bob", "Secret1!").unwrap());
    assert!(recent_auth(300_000));
    // another user's internal login in the same session does not take ada's sign-in, and drops it: ada coming back
    // without signing in has none
    internal_login_user("bob").unwrap();
    assert!(!recent_auth(300_000));
    internal_login_user("ada").unwrap();
    assert!(!recent_auth(300_000));
    logout().unwrap();
    internal_login_user("ada").unwrap();
    assert!(!recent_auth(300_000));
    // the internal login that finishes a second factor (the password was checked, then the code)
    logout().unwrap();
    session::set_session_attribute(PRE_AUTHC_USERNAME, Some("bob"));
    session::set_session_attribute(AUTHC_FACTOR_REQUIRED, Some("true"));
    internal_login_user("bob").unwrap();
    assert!(recent_auth(300_000));
    session::reset_session();
}

/// A re-authentication checks the signed-in user's password without changing the login: a wrong one fails and counts,
/// a right one is a recent sign-in.
#[test]
fn a_reauthentication_checks_the_password_and_counts_a_wrong_one() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    create_user(&facade, "ada", "Secret1!", |_| {});
    internal_login_user("ada").unwrap();
    assert_eq!(
        check_reauth_password("ada", "wrong").unwrap_err().to_string(),
        INVALID_LOGIN
    );
    assert_eq!(i64_field(&load_user(&facade, "ada"), "successiveFailedLogins"), 1);
    assert!(!reauth_password_checked("ada"));
    check_reauth_password("ada", "Secret1!").unwrap();
    assert!(reauth_password_checked("ada"));
    assert_eq!(i64_field(&load_user(&facade, "ada"), "successiveFailedLogins"), 0);
    assert_eq!(session::current_username().unwrap().as_deref(), Some("ada"));
    note_recent_auth("ada");
    assert!(recent_auth(300_000));
    assert!(!reauth_password_checked("ada"));
    session::reset_session();
}

/// SEC-CLI-107: the apps sign every request in by the device's key, each a session of its own, so a re-authentication
/// made in one request is kept on the key and the key's next requests have it: the checked password while it waits for
/// the code, then the recent sign-in. Another key of the user, and a session's key, carry none, and a stamp is as old
/// as it was.
#[test]
fn a_device_keys_reauthentication_counts_in_its_next_requests_and_no_others() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    create_user(&facade, "ada", "Secret1!", |_| {});
    internal_login_user("ada").unwrap();
    let device = get_device_key("Ada's laptop").unwrap();
    let other = get_device_key("Ada's phone").unwrap();
    let session_key = get_login_key(None).unwrap();
    session::reset_session();

    // a request of the device: its password checked; the next request has it, waiting for the code
    login_user_key(&device).unwrap();
    assert!(!recent_auth(300_000));
    check_reauth_password("ada", "Secret1!").unwrap();
    session::reset_session();
    login_user_key(&device).unwrap();
    assert!(reauth_password_checked("ada"));
    assert!(!recent_auth(300_000));
    // the code finishes it: the next request is a recent sign-in, and the checked password is spent
    note_recent_auth("ada");
    session::reset_session();
    login_user_key(&device).unwrap();
    assert!(recent_auth(300_000));
    assert!(!reauth_password_checked("ada"));
    session::reset_session();

    // another key of the user, and a session's key (whose re-authentication is kept nowhere), have none
    login_user_key(&other).unwrap();
    assert!(!recent_auth(300_000));
    session::reset_session();
    login_user_key(&session_key).unwrap();
    note_recent_auth("ada");
    session::reset_session();
    login_user_key(&session_key).unwrap();
    assert!(!recent_auth(300_000));
    session::reset_session();

    // a stamp an hour old is an hour old
    let hashed = password::hash_login_key(&device).unwrap();
    facade
        .find(USER_LOGIN_KEY)
        .unwrap()
        .eq("loginKey", Value::string(hashed))
        .update_all([("lastAuthDate".to_string(), stamp_value(now_millis() - 3_600_000))])
        .unwrap();
    login_user_key(&device).unwrap();
    assert!(!recent_auth(300_000));
    assert!(recent_auth(2 * 3_600_000));
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

    apply_header_keys(Some(&key), None, None).unwrap();
    assert_eq!(session::current_username().unwrap().as_deref(), Some("ada"));
    logout().unwrap();
    assert_eq!(login_user_key(&short).unwrap_err().to_string(), INVALID_LOGIN);

    // Revoke: the admin deletes the key's row.
    row.delete(&facade).unwrap();
    assert_eq!(
        apply_header_keys(Some(&key), None, None).unwrap_err().to_string(),
        INVALID_LOGIN
    );
    assert!(session::current_username().unwrap().is_none());
    session::reset_session();
}

#[test]
fn a_device_key_lasts_ninety_days_notes_its_use_and_signs_out_alone() {
    let (ctx, facade) = open_ctx();
    let _guard = ctx.enter();
    create_user(&facade, "ada", "Secret1!", |_| {});
    internal_login_user("ada").unwrap();
    let device = get_device_key("  Ada's laptop  ").unwrap();
    let other = get_device_key("Ada's phone").unwrap();
    let session = get_login_key(None).unwrap();
    let row_of = |key: &str| {
        let hashed = password::hash_login_key(key).unwrap();
        facade
            .find(USER_LOGIN_KEY)
            .unwrap()
            .eq("loginKey", Value::string(hashed))
            .one()
            .unwrap()
            .expect("the key's row")
    };
    let row = row_of(&device);
    assert_eq!(text(&row, "deviceName"), "Ada's laptop");
    let thru = field_millis(&row, "thruDate").unwrap() - field_millis(&row, "fromDate").unwrap();
    assert_eq!(thru, 2160 * 60 * 60 * 1000, "the conf default is 90 days");
    assert!(field_millis(&row, "lastUsedDate").is_none());
    assert_eq!(text(&row_of(&session), "deviceName"), "");
    logout().unwrap();

    // a sign-in by the device's key notes its use; a session's key is not written to
    login_user_key(&device).unwrap();
    let used = field_millis(&row_of(&device), "lastUsedDate").expect("the use is noted");
    logout().unwrap();
    login_user_key(&device).unwrap();
    assert_eq!(
        field_millis(&row_of(&device), "lastUsedDate"),
        Some(used),
        "at most once a minute"
    );
    logout().unwrap();
    login_user_key(&session).unwrap();
    assert!(field_millis(&row_of(&session), "lastUsedDate").is_none());

    // signing the device out ends its key only
    assert!(revoke_login_key(&device).unwrap());
    assert!(!revoke_login_key("not a key").unwrap());
    logout().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    assert_eq!(login_user_key(&device).unwrap_err().to_string(), INVALID_LOGIN);
    login_user_key(&other).unwrap();
    logout().unwrap();
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
    basic_convert(&Value::I64(millis), "Timestamp").value
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

/// A login key's thruDate is an exclusive end, as every effective-dated row's: one ended at this millisecond is ended.
/// revoke#UserHeldKey sets it to now, and a login in the same millisecond took the revoked key (seen as an
/// order-dependent failure of a_scoped_key_opens_only_its_scope_and_its_holder_ends_it).
#[test]
fn a_key_ended_this_millisecond_is_ended() {
    assert!(super::key_ended(1_000, 1_000));
    assert!(super::key_ended(1_000, 1_001));
    assert!(!super::key_ended(1_001, 1_000));
}

/// The security proof of OWASP ASVS 5.0.0 v5.0.0-6.5.3 (docs/security/proofs/V6-Authentication.md), in-process: the
/// seeds and codes of the second factors are drawn from the operating system's random source (`getrandom`, in
/// `factor.rs` `totp_secret` and `random_code`), never a seeded or time-based generator. A TOTP seed
/// (`create#UserAuthcFactorTotp`, `ec.user.totpSecret()`) is 20 random bytes in base32, 160 bits; a code sent by email
/// or SMS (`create#SingleUseAuthcFactor`, `ec.user.randomCode(6)`) is six digits and a printed recovery code
/// (`create#SingleUseAuthcCodes`, `ec.user.randomCode(8)`) eight, each a uniform draw (the top of the `u32` range that
/// would bias the remainder is drawn again). Two thousand seeds are all different and their bytes take every value; two
/// thousand codes of each width are of that width, take every digit in every place, and spread their leading digit
/// evenly. The scripts' calls give the same, and `UserServices.xml` makes its seeds and codes with them and has no
/// `Math.random`. It does not judge the operating system's generator.
#[test]
fn factor_seeds_and_codes_come_from_the_operating_systems_random_source() {
    let mut seeds = std::collections::HashSet::new();
    let mut byte_values = [false; 256];
    for _ in 0..2000 {
        let seed = crate::user::totp_secret();
        assert_eq!(seed.len(), 32, "{seed}");
        let bytes = super::super::totp::key(&seed).expect("base32");
        assert_eq!(bytes.len(), 20, "{seed}");
        for byte in bytes {
            byte_values[usize::from(byte)] = true;
        }
        seeds.insert(seed);
    }
    assert_eq!(seeds.len(), 2000, "every seed is new");
    assert!(
        byte_values.iter().all(|seen| *seen),
        "the seeds' bytes take every value"
    );

    for width in [6, 8] {
        let mut codes = std::collections::HashSet::new();
        let mut digits = vec![[0_u32; 10]; width];
        for _ in 0..2000 {
            let code = crate::user::random_code(width).unwrap();
            assert_eq!(code.len(), width, "{code}");
            for (place, digit) in code.chars().enumerate() {
                let digit = digit.to_digit(10).expect("a digit") as usize;
                digits[place][digit] += 1;
            }
            codes.insert(code);
        }
        // 2000 draws of 10^6 or more values: a repeat is possible, many are not
        assert!(codes.len() >= 1990, "width {width}: {} distinct", codes.len());
        for (place, counts) in digits.iter().enumerate() {
            assert!(
                counts.iter().all(|count| *count > 0),
                "width {width}, place {place}: {counts:?}"
            );
        }
        // about 200 each; a generator stuck on a few values, or one biased to low digits, falls outside
        assert!(
            digits[0].iter().all(|count| (100..=320).contains(count)),
            "width {width}, leading digit: {:?}",
            digits[0]
        );
    }

    let ctx = ExecutionContextFactory::test_context();
    let _entered = ctx.enter();
    let script = |expression: &str| moqui_lib::object::to_plain_string(&crate::script::eval_value(expression).unwrap());
    for width in [6, 8] {
        let code = script(&format!("ec.user.randomCode({width})"));
        assert!(
            code.len() == width && code.chars().all(|ch| ch.is_ascii_digit()),
            "{code}"
        );
    }
    let seed = script("ec.user.totpSecret()");
    assert_eq!(
        super::super::totp::key(&seed).map(|bytes| bytes.len()),
        Some(20),
        "{seed}"
    );
    assert_ne!(seed, script("ec.user.totpSecret()"));
    crate::script::clear_context_stack();

    let services = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/components/base/framework/service/org/moqui/impl/UserServices.xml"
    ))
    .unwrap();
    let body = |noun: &str| {
        let start = services
            .find(&format!("noun=\"{noun}\""))
            .unwrap_or_else(|| panic!("{noun}"));
        let end = services[start..].find("</service>").expect("its end") + start;
        services[start..end].to_string()
    };
    assert!(body("SingleUseAuthcCodes").contains("ec.user.randomCode(8)"));
    assert!(body("SingleUseAuthcFactor").contains("ec.user.randomCode(6)"));
    assert!(body("UserAuthcFactorTotp").contains("ec.user.totpSecret()"));
    assert!(
        !services.contains("Math.random"),
        "no code from the script engine's own generator"
    );
}

/// The security proof of OWASP ASVS 5.0.0 v5.0.0-6.2.10 (docs/security/proofs/V6-Authentication.md), in-process, of
/// a Configuration row: a password stays valid until it is changed when `user-facade/password@change-weeks` is 0, a
/// password set ten years ago signing in as one set today does; under the value `MoquiDefaultConf.xml` ships (104, read
/// from the file) the same account is refused once its password is older than that many weeks, and signs in while it
/// is not. So the deployer must set `change-weeks` to 0 (a `moqui_conf__user_facade__password__change_weeks` of 0, or
/// the attribute in the runtime conf) for no periodic rotation.
#[test]
fn a_password_stays_valid_with_change_weeks_zero_and_the_shipped_default_rotates_it() {
    let shipped_conf = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/components/base/framework/conf/MoquiDefaultConf.xml"
    ))
    .unwrap();
    let (_, after) = shipped_conf
        .split_once("change-weeks=\"")
        .expect("the shipped change-weeks");
    let shipped: i64 = after.split('"').next().unwrap().parse().unwrap();
    assert!(shipped > 0, "the shipped default expires a password: {shipped}");

    let sign_in = |change_weeks: i64, set_weeks_ago: i64| {
        session::reset_session();
        let ctx = ExecutionContextFactory::test_context_with_conf(&format!(
            r#"<moqui-conf><user-facade><password change-weeks="{change_weeks}"/></user-facade></moqui-conf>"#
        ));
        let facade = crate::testing::open();
        ctx.factory().set_entity(facade.clone());
        let _guard = ctx.enter();
        assert_eq!(policy::change_weeks(), change_weeks);
        create_user(&facade, "rotate", "Secret1!", |value| {
            value.set("passwordSetDate", stamp(now_millis() - set_weeks_ago * WEEK_MILLIS));
        });
        let outcome = login_user("rotate", "Secret1!").map_err(|err| err.to_string());
        if outcome.is_ok() {
            logout().unwrap();
        }
        session::reset_session();
        outcome
    };
    assert_eq!(sign_in(0, 520), Ok(()), "ten years old, with change-weeks 0");
    assert_eq!(sign_in(0, 0), Ok(()));
    let expired = sign_in(shipped, shipped + 1).expect_err("older than the shipped default");
    assert!(expired.contains("must be changed"), "{expired}");
    assert_eq!(
        sign_in(shipped, shipped - 1),
        Ok(()),
        "younger than the shipped default"
    );
}
