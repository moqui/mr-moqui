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

//! The second factor through the `rest.xml` transitions (`login`, `sendOtp`, `verifyOtp`) and the
//! password reset of `UserServices.xml`. The tests that read the mail sent (the email code, the reset password) are in
//! moqui-mail.

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_lib::object::Value;

use super::{header_value, request, webroot, Env};
use crate::context::ExecutionContextFactory;
use crate::entity::EntityFacade;
use crate::screen::serve_at;
use crate::service::ServicePools;
use crate::web::WebResponse;

/// A factory with the framework services, two service workers for the async mail, an SMTP server
/// row, and the seed email templates.
struct Flow {
    _env: Env,
    factory: Arc<ExecutionContextFactory>,
    facade: EntityFacade,
}

impl Flow {
    fn open() -> Self {
        let env = Env::new();
        let context =
            ExecutionContextFactory::test_context_in("", ServicePools::bounded(2, 16), crate::context::framework_dir());
        let facade = crate::testing::open();
        context.factory().set_entity(facade.clone());
        let _entered = context.enter();
        let _authz = context.disable_authz();
        crate::service::load_definitions().unwrap();
        let flow = Self {
            _env: env,
            factory: Arc::clone(context.factory()),
            facade,
        };
        flow.row(
            "moqui.basic.email.EmailServer",
            &[
                ("emailServerId", "SYSTEM"),
                ("smtpHost", "smtp.example.com"),
                ("smtpPort", "25"),
            ],
        );
        for (id, screen, subject) in [
            ("PASSWORD_RESET", "PasswordReset.xml", "Password Reset"),
            ("SINGLE_USE_CODE", "SingleUseCode.xml", "Single Use Code"),
        ] {
            let location = format!("component://framework/screen/{screen}");
            flow.row(
                "moqui.basic.email.EmailTemplate",
                &[
                    ("emailTemplateId", id),
                    ("emailServerId", "SYSTEM"),
                    ("bodyScreenLocation", &location),
                    ("webappName", "webroot"),
                    ("fromAddress", "system@example.com"),
                    ("subject", subject),
                ],
            );
        }
        flow
    }

    fn row(&self, entity: &str, fields: &[(&str, &str)]) {
        let mut value = self.facade.make_value(entity).unwrap();
        for (name, field) in fields {
            value.set(*name, Value::string(*field));
        }
        value.create(&self.facade).unwrap();
    }

    fn user(&self, username: &str, email: &str) {
        let stored = crate::user::hash_password("Secret1!").unwrap();
        let mut account = self.facade.make_value("moqui.security.UserAccount").unwrap();
        account.set("userId", Value::string(username));
        account.set("username", Value::string(username));
        account.set("emailAddress", Value::string(email));
        account.set("currentPassword", Value::string(stored));
        account.set("passwordHashType", Value::string("argon2"));
        account.create(&self.facade).unwrap();
    }

    /// Run `body` in a context of this factory, with a user logged in when one is named.
    fn as_user<T>(&self, username: Option<&str>, body: impl FnOnce() -> T) -> T {
        let context = self.factory.new_context();
        let _entered = context.enter();
        if let Some(username) = username {
            crate::user::internal_login_user(username).unwrap();
        }
        let out = body();
        crate::user::session::reset_session();
        out
    }

    fn post(&self, path: &str, body: &str, session: &mut Session) -> WebResponse {
        let mut post = request("POST", path);
        post.set_header("content-type", "application/x-www-form-urlencoded");
        post.body = body.as_bytes().to_vec();
        post.session_cookie = session.cookie.clone();
        if let Some(token) = &session.token {
            post.set_header("x-csrf-token", token);
        }
        let response = serve_at(&self.factory, post, &webroot().to_string_lossy());
        session.follow(&response);
        response
    }

    fn row_text(&self, entity: &str, key: &str, id: &str, field: &str) -> String {
        let context = self.factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        self.facade
            .find(entity)
            .unwrap()
            .eq(key, Value::string(id))
            .one()
            .unwrap()
            .and_then(|row| row.get(field).map(moqui_lib::object::to_plain_string))
            .unwrap_or_default()
    }
}

/// The session cookie and CSRF token a browser would carry from one response to the next.
#[derive(Default)]
struct Session {
    cookie: Option<String>,
    token: Option<String>,
}

impl Session {
    fn follow(&mut self, response: &WebResponse) {
        if let Some(set_cookie) = header_value(response, "Set-Cookie") {
            if let Some(value) = set_cookie
                .split(';')
                .next()
                .and_then(|pair| pair.strip_prefix("moqui.session="))
            {
                self.cookie = Some(value.to_string());
            }
        }
        if let Some(token) = header_value(response, "X-CSRF-Token") {
            self.token = Some(token);
        }
    }
}

fn json(response: &WebResponse) -> serde_json::Value {
    serde_json::from_str(&response.body_text()).unwrap_or_else(|err| panic!("{err}: {}", response.body_text()))
}

fn current_totp(secret: &str) -> String {
    let bytes = totp_rs::Secret::try_from_base32(secret).unwrap();
    totp_rs::Builder::new()
        .with_secret(bytes)
        .build()
        .unwrap()
        .generate_current()
        .to_string()
}

#[test]
fn rest_login_with_totp_needs_the_code_and_verify_otp_finishes_it() {
    let flow = Flow::open();
    flow.user("ada", "ada@example.com");
    let (secret, qr_uri) = flow.as_user(Some("ada"), || {
        let mut parameters = BTreeMap::new();
        parameters.insert("userId".to_string(), Value::string("ada"));
        let setup = crate::service::sync("org.moqui.impl.UserServices.setup#UserAuthcFactorTotp", &parameters).unwrap();
        let errors = crate::context::ExecutionContext::with_current(|ec| ec.message().errors().to_vec()).unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        let secret = moqui_lib::object::to_plain_string(setup.get("qrSecret").expect("qrSecret"));
        let qr_uri = moqui_lib::object::to_plain_string(setup.get("qrUri").expect("qrUri"));
        let factor = crate::service::sync(
            "org.moqui.impl.UserServices.get#UserAuthcFactorInfo",
            &BTreeMap::from([("userId".to_string(), Value::string("ada"))]),
        )
        .unwrap();
        assert_eq!(factor.get("secondFactorRequired"), Some(&Value::Bool(true)));
        (secret, qr_uri)
    });
    assert_eq!(secret.len(), 32, "{secret}");
    assert!(qr_uri.starts_with("otpauth://totp/ada?secret="), "{qr_uri}");
    assert_eq!(
        flow.row_text("moqui.security.UserAuthcFactor", "userId", "ada", "factorTypeEnumId"),
        "UafTotp"
    );

    let mut session = Session::default();
    let response = flow.post("/rest/login", "username=ada&password=Secret1!", &mut session);
    assert_eq!(response.status, 200, "{}", response.body_text());
    let info = json(&response);
    assert_eq!(info["secondFactorRequired"], true, "{info}");
    assert_eq!(info["factorTypeEnumIds"], serde_json::json!(["UafTotp"]), "{info}");
    assert_eq!(info["sendableFactors"], serde_json::json!([]), "{info}");
    assert!(info.get("loggedIn").is_none(), "{info}");

    let response = flow.post("/rest/verifyOtp", "code=000000", &mut session);
    assert_eq!(response.status, 403, "{}", response.body_text());
    assert!(
        response.body_text().contains("Authentication code is not valid"),
        "{}",
        response.body_text()
    );

    let code = current_totp(&secret);
    let response = flow.post("/rest/verifyOtp", &format!("code={code}"), &mut session);
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert_eq!(json(&response)["loggedIn"], true);
    assert_eq!(
        flow.row_text("moqui.security.UserAuthcFactor", "userId", "ada", "needsValidation"),
        "N"
    );
    let response = flow.post("/rest/logout", "", &mut session);
    assert_eq!(response.body_text(), "successful");

    // The code can also come with the password, in one request.
    let mut session = Session::default();
    let code = current_totp(&secret);
    let response = flow.post(
        "/rest/login",
        &format!("username=ada&password=Secret1!&code={code}"),
        &mut session,
    );
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert_eq!(json(&response)["loggedIn"], true);
    // Without a pre-authenticated user there is nothing to verify.
    let response = flow.post("/rest/verifyOtp", "code=123456", &mut Session::default());
    assert_eq!(response.status, 401, "{}", response.body_text());
}

/// Seed `resetPassword` and `resetPasswordSetDate` so a later service call does not send mail.
fn seed_reset_password(flow: &Flow, user_id: &str, plain: &str, set_at_ms: i64) -> String {
    let hashed = crate::user::hash_password(plain).unwrap();
    let mut account = flow
        .facade
        .find("moqui.security.UserAccount")
        .unwrap()
        .eq("userId", Value::string(user_id))
        .one()
        .unwrap()
        .expect("account");
    account.set("resetPassword", Value::string(&hashed));
    account.set("resetPasswordSetDate", crate::user::stamp_value(set_at_ms));
    account.update(&flow.facade).unwrap();
    hashed
}

#[test]
fn a_second_reset_password_inside_the_window_leaves_the_stored_reset_password() {
    let flow = Flow::open();
    flow.user("rst", "rst@example.com");
    // Inside `login/@disable-minutes` (5): `reset#Password` returns before it replaces the hash or sends mail.
    let hashed = seed_reset_password(&flow, "rst", "ResetUse1!!", crate::user::now_millis());
    let messages = flow.as_user(None, || {
        let parameters = BTreeMap::from([("username".to_string(), Value::string("rst"))]);
        crate::service::sync("org.moqui.impl.UserServices.reset#Password", &parameters).unwrap();
        crate::context::ExecutionContext::with_current(|ec| {
            assert!(ec.message().errors().is_empty(), "{:?}", ec.message().errors());
            ec.message().messages().to_vec()
        })
        .unwrap()
    });
    assert!(
        messages
            .iter()
            .any(|message| message.contains("reset password was sent")),
        "{messages:?}"
    );
    assert_eq!(
        flow.row_text("moqui.security.UserAccount", "userId", "rst", "resetPassword"),
        hashed
    );
}

#[test]
fn an_expired_reset_password_is_rejected() {
    let flow = Flow::open();
    flow.user("old", "old-reset@example.com");
    let reset = "ResetExp1!!";
    // Older than `password/@email-expire-hours` (48). The hash matches; only the set date is past.
    let hashed = seed_reset_password(&flow, "old", reset, crate::user::now_millis() - 50 * 3_600_000);
    let (out, messages) = flow.as_user(None, || {
        let parameters = BTreeMap::from([
            ("username".to_string(), Value::string("old")),
            ("oldPassword".to_string(), Value::string(reset)),
            ("newPassword".to_string(), Value::string("SecNew8!!")),
            ("newPasswordVerify".to_string(), Value::string("SecNew8!!")),
        ]);
        let out = crate::service::sync("org.moqui.impl.UserServices.update#Password", &parameters).unwrap();
        let messages = crate::context::ExecutionContext::with_current(|ec| {
            let mut all = ec.message().errors().to_vec();
            all.extend(ec.message().messages().iter().cloned());
            all
        })
        .unwrap();
        (out, messages)
    });
    assert_ne!(out.get("updateSuccessful"), Some(&Value::Bool(true)), "{out:?}");
    assert!(
        messages
            .iter()
            .any(|message| message.contains("Password could not be updated")),
        "{messages:?}"
    );
    assert_eq!(
        flow.row_text("moqui.security.UserAccount", "userId", "old", "resetPassword"),
        hashed
    );
    flow.as_user(None, || {
        crate::user::login_user("old", "SecNew8!!").unwrap_err();
        crate::user::login_user("old", "Secret1!").unwrap();
    });
}

/// A TOTP factor for `username`, as the user would set it up. Returns the secret.
fn totp_factor(flow: &Flow, username: &str) -> String {
    flow.as_user(Some(username), || {
        let parameters = BTreeMap::from([("userId".to_string(), Value::string(username))]);
        let setup = crate::service::sync("org.moqui.impl.UserServices.setup#UserAuthcFactorTotp", &parameters).unwrap();
        moqui_lib::object::to_plain_string(setup.get("qrSecret").expect("qrSecret"))
    })
}

#[test]
fn a_code_changes_only_the_password_of_the_pre_authenticated_user() {
    let flow = Flow::open();
    flow.user("mal", "mal@example.com");
    flow.user("vic", "vic@example.com");
    totp_factor(&flow, "mal");
    let vic_secret = totp_factor(&flow, "vic");
    let change = |pre_authc: &str| {
        flow.as_user(None, || {
            let _web = crate::web::install(BTreeMap::new());
            crate::user::session::set_session_attribute(crate::user::PRE_AUTHC_USERNAME, Some(pre_authc));
            let parameters = BTreeMap::from([
                ("username".to_string(), Value::string("vic")),
                // Required by the service, and not vic's password: the code is meant to stand in for it.
                ("oldPassword".to_string(), Value::string("not-known")),
                ("code".to_string(), Value::string(current_totp(&vic_secret))),
                ("newPassword".to_string(), Value::string("Taken3!x")),
                ("newPasswordVerify".to_string(), Value::string("Taken3!x")),
            ]);
            let out = crate::service::sync("org.moqui.impl.UserServices.update#Password", &parameters).unwrap();
            let messages = crate::context::ExecutionContext::with_current(|ec| {
                let mut all = ec.message().errors().to_vec();
                all.extend(ec.message().messages().iter().cloned());
                ec.message().clear();
                all
            })
            .unwrap();
            (out, messages)
        })
    };
    // Pre-authenticated as mal, vic's code does not stand in for vic's old password, which is then checked.
    let (out, messages) = change("mal");
    assert_ne!(out.get("updateSuccessful"), Some(&Value::Bool(true)), "{out:?}");
    assert!(
        messages
            .iter()
            .any(|message| message.contains("Password could not be updated")),
        "{messages:?}"
    );
    flow.as_user(None, || {
        crate::user::login_user("vic", "Taken3!x").unwrap_err();
    });
    // Pre-authenticated as vic, the same code does.
    let (out, messages) = change("vic");
    assert_eq!(
        out.get("updateSuccessful"),
        Some(&Value::Bool(true)),
        "{out:?} {messages:?}"
    );
}

#[test]
fn wrong_codes_count_as_failed_logins_and_disable_the_account() {
    let flow = Flow::open();
    flow.user("ada", "ada@example.com");
    let secret = totp_factor(&flow, "ada");
    let failures = || {
        flow.row_text("moqui.security.UserAccount", "userId", "ada", "successiveFailedLogins")
            .parse::<i64>()
            .unwrap_or(0)
    };

    let mut session = Session::default();
    // A wrong code with the password, then wrong codes at verifyOtp: each counts.
    let response = flow.post(
        "/rest/login",
        "username=ada&password=Secret1!&code=000000",
        &mut session,
    );
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert_eq!(failures(), 1);
    for count in 2..=3 {
        let response = flow.post("/rest/verifyOtp", "code=000000", &mut session);
        assert_eq!(response.status, 403, "{}", response.body_text());
        assert_eq!(failures(), count);
    }
    assert_ne!(
        flow.row_text("moqui.security.UserAccount", "userId", "ada", "disabled"),
        "Y"
    );
    // The fourth failure disables the account, as the fourth wrong password does.
    let response = flow.post("/rest/verifyOtp", "code=000000", &mut session);
    assert_eq!(response.status, 403, "{}", response.body_text());
    assert_eq!(
        flow.row_text("moqui.security.UserAccount", "userId", "ada", "disabled"),
        "Y"
    );
    // While disabled, the right code is refused and still counted, and the password does not log in.
    let code = current_totp(&secret);
    let response = flow.post("/rest/verifyOtp", &format!("code={code}"), &mut session);
    assert_eq!(response.status, 403, "{}", response.body_text());
    assert_eq!(failures(), 5);
    let response = flow.post("/rest/login", "username=ada&password=Secret1!", &mut Session::default());
    assert_eq!(response.status, 401, "{}", response.body_text());
}

/// A desktop or mobile client (plans/Mobile-Desktop.md, MD3): the REST login with `createLoginKey` and `deviceName`
/// makes that device's key, the user's device list shows it, `logout` with `revokeLoginKey` ends the key it signed in
/// with, and `revoke#UserDeviceKey` signs another device out.
#[test]
fn a_device_signs_in_by_key_is_listed_and_signs_out_by_revoking_its_key() {
    let flow = Flow::open();
    flow.user("ada", "ada@example.com");
    let device_key = |name: &str| {
        let body = format!("username=ada&password=Secret1%21&createLoginKey=true&deviceName={name}");
        let response = flow.post("/rest/login", &body, &mut Session::default());
        assert_eq!(response.status, 200, "{}", response.body_text());
        json(&response)["loginKey"].as_str().expect("a device key").to_string()
    };
    let laptop = device_key("Ada%27s%20laptop");
    let phone = device_key("Phone");
    let devices = || {
        flow.as_user(Some("ada"), || {
            let out = crate::service::sync("org.moqui.impl.UserServices.get#UserDeviceKeys", &BTreeMap::new()).unwrap();
            let Some(Value::List(list)) = out.get("deviceList") else {
                panic!("{out:?}")
            };
            list.iter()
                .map(|row| {
                    let Value::Map(row) = row else { panic!("{row:?}") };
                    let name = row.get("deviceName").map(moqui_lib::object::to_plain_string);
                    let key = row.get("loginKey").map(moqui_lib::object::to_plain_string);
                    (name.unwrap_or_default(), key.unwrap_or_default())
                })
                .collect::<Vec<_>>()
        })
    };
    let listed = devices();
    let names: Vec<&str> = listed.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(listed.len(), 2, "{listed:?}");
    assert!(names.contains(&"Ada's laptop") && names.contains(&"Phone"), "{names:?}");

    // the laptop signs out: its key ends with the session, by the header alone (no cookie, no CSRF token)
    let mut logout = request("POST", "/rest/logout");
    logout.set_header("content-type", "application/x-www-form-urlencoded");
    logout.set_header("login_key", &laptop);
    logout.body = b"revokeLoginKey=true".to_vec();
    let response = serve_at(&flow.factory, logout, &webroot().to_string_lossy());
    assert_eq!(response.body_text(), "successful");
    // a key ends at its thruDate, now: it is refused from the next millisecond
    std::thread::sleep(std::time::Duration::from_millis(5));
    flow.as_user(None, || {
        assert!(
            crate::user::login_user_key(&laptop).is_err(),
            "the laptop's key is ended"
        );
        crate::user::login_user_key(&phone).expect("the phone's key still signs in");
    });
    let listed = devices();
    assert_eq!(listed.len(), 1, "{listed:?}");

    // the phone is signed out from another device; another user's key is not theirs to revoke
    flow.user("bob", "bob@example.com");
    let phone_hash = listed[0].1.clone();
    let refused = flow.as_user(Some("bob"), || {
        let parameters = BTreeMap::from([("loginKey".to_string(), Value::string(&phone_hash))]);
        let _ = crate::service::sync("org.moqui.impl.UserServices.revoke#UserDeviceKey", &parameters);
        crate::context::ExecutionContext::with_current(|ec| ec.message().errors().to_vec()).unwrap()
    });
    assert!(!refused.is_empty(), "bob may not revoke ada's device");
    flow.as_user(Some("ada"), || {
        let parameters = BTreeMap::from([("loginKey".to_string(), Value::string(&phone_hash))]);
        crate::service::sync("org.moqui.impl.UserServices.revoke#UserDeviceKey", &parameters).unwrap();
    });
    std::thread::sleep(std::time::Duration::from_millis(5));
    assert!(devices().is_empty());
    flow.as_user(None, || assert!(crate::user::login_user_key(&phone).is_err()));
}

/// A device renews its key before it runs out (plans/Mobile-Desktop.md §4): the new key has the device's name and a
/// full lifetime, the old one still signs in for the grace minutes, and a key that is not a device's is not renewed.
#[test]
fn a_device_renews_its_key_with_the_same_name_and_the_old_key_ends_soon() {
    let flow = Flow::open();
    flow.user("ada", "ada@example.com");
    let response = flow.post(
        "/rest/login",
        "username=ada&password=Secret1%21&createLoginKey=true&deviceName=Laptop",
        &mut Session::default(),
    );
    assert_eq!(response.status, 200, "{}", response.body_text());
    let old = json(&response)["loginKey"].as_str().expect("a device key").to_string();
    let now = crate::user::now_millis();
    let (renewed, thru) = flow
        .as_user(Some("ada"), || crate::user::renew_device_key(&old))
        .unwrap()
        .expect("the device's key is renewed");
    assert_ne!(renewed, old);
    assert!(thru - now > 89 * 24 * 3_600_000, "a full lifetime: {}", thru - now);
    flow.as_user(None, || {
        crate::user::login_user_key(&renewed).expect("the new key signs in");
        crate::user::login_user_key(&old).expect("the old key still signs in, for a few minutes");
    });
    let rows = flow
        .facade
        .find("moqui.security.UserLoginKey")
        .unwrap()
        .condition(moqui_db::sql::Condition::is_not_null("deviceName"))
        .list()
        .unwrap();
    let rows: Vec<(String, i64)> = rows
        .iter()
        .map(|row| {
            let name = row
                .get("deviceName")
                .map(moqui_lib::object::to_plain_string)
                .unwrap_or_default();
            let thru = row.get("thruDate").and_then(crate::user::value_millis).unwrap_or(0);
            (name, thru)
        })
        .collect();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows.iter().all(|(name, _)| name == "Laptop"), "{rows:?}");
    let ends = rows.iter().map(|(_, thru)| *thru).min().unwrap();
    assert!(ends - now <= 11 * 60_000, "the old key ends in minutes: {}", ends - now);

    // a session's own key, with no device, is not a device's to renew; nor is another user's
    let plain = flow.as_user(Some("ada"), || crate::user::get_login_key(None)).unwrap();
    assert!(flow
        .as_user(Some("ada"), || crate::user::renew_device_key(&plain))
        .unwrap()
        .is_none());
    flow.user("bob", "bob@example.com");
    assert!(flow
        .as_user(Some("bob"), || crate::user::renew_device_key(&renewed))
        .unwrap()
        .is_none());
}

/// An installed app's SSO sign-in ends at `/sso/appToken` (crates/moqui-core/src/service/sso_app.rs): the single-use
/// code `issue#AppCode` made, with the verifier of its challenge, is a device key once; a wrong verifier, a used code
/// and an expired one are refused.
#[test]
fn an_app_code_with_its_verifier_is_a_device_key_once() {
    let flow = Flow::open();
    flow.user("ada", "ada@example.com");
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let code_row = |code: &str, expire_offset: i64| {
        let context = flow.factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let mut row = flow.facade.make_value("moqui.security.UserAppCode").unwrap();
        row.set("codeHash", Value::string(crate::user::single_use_hash(code).unwrap()));
        row.set("userId", Value::string("ada"));
        row.set(
            "codeChallenge",
            Value::string("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"),
        );
        row.set("redirectUri", Value::string("org.moqui.desktop:/sso"));
        row.set(
            "expireDate",
            crate::user::stamp_value(crate::user::now_millis() + expire_offset),
        );
        row.create(&flow.facade).unwrap();
    };
    let exchange = |code: &str, verifier: &str| {
        let body = format!(
            "code={code}&codeVerifier={verifier}&redirectUri=org.moqui.desktop%3A%2Fsso&deviceName=Ada%27s%20laptop"
        );
        flow.post("/sso/appToken", &body, &mut Session::default())
    };
    code_row("good-code", 60_000);
    let wrong = exchange("good-code", &"x".repeat(43));
    assert_eq!(wrong.status, 400, "{}", wrong.body_text());
    // the wrong verifier used the code: the right one is too late
    assert_eq!(exchange("good-code", verifier).status, 400);

    code_row("second-code", 60_000);
    let answer = exchange("second-code", verifier);
    assert_eq!(answer.status, 200, "{}", answer.body_text());
    let key = json(&answer)["loginKey"].as_str().expect("a device key").to_string();
    flow.as_user(None, || crate::user::login_user_key(&key).expect("the key signs in"));
    let device = flow.row_text(
        "moqui.security.UserLoginKey",
        "loginKey",
        &crate::user::hash_login_key(&key).unwrap(),
        "deviceName",
    );
    assert_eq!(device, "Ada's laptop");
    assert_eq!(exchange("second-code", verifier).status, 400, "once");

    code_row("other-redirect", 60_000);
    let body = format!("code=other-redirect&codeVerifier={verifier}&redirectUri=org.evil%3A%2Fsso");
    assert_eq!(
        flow.post("/sso/appToken", &body, &mut Session::default()).status,
        400,
        "another redirect URI"
    );

    code_row("old-code", -1_000);
    assert_eq!(exchange("old-code", verifier).status, 400, "expired");
    assert_eq!(exchange("no-such-code", verifier).status, 400);
}
