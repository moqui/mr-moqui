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

use std::sync::Arc;

use moqui_lib::object::Value;

use super::super::login;
use super::super::session::{bind, cookie_property_lock, current_user_id, live_session_user_id, reset_session};
use super::*;
use crate::context::ExecutionContextFactory;
use crate::entity::EntityFacade;

const HOUR: i64 = 60 * MINUTE;

struct Key;

impl Drop for Key {
    fn drop(&mut self) {
        moqui_lib::env::clear_property("session_cookie_key");
        reset_session();
    }
}

fn setup() -> (
    std::sync::MutexGuard<'static, ()>,
    crate::context::ExecutionContext,
    EntityFacade,
    Key,
) {
    use base64::Engine;
    let lock = cookie_property_lock();
    reset_session();
    moqui_lib::env::set_property(
        "session_cookie_key",
        base64::engine::general_purpose::STANDARD.encode([7_u8; 32]),
    );
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    (lock, ctx, facade, Key)
}

fn create(facade: &EntityFacade, entity: &str, fields: &[(&str, Value)]) {
    let mut row = facade.make_value(entity).unwrap();
    for (name, value) in fields {
        row.set(*name, value.clone());
    }
    row.create(facade).unwrap();
}

fn set_device(facade: &EntityFacade, fields: &[(&str, Value)]) {
    write_device(
        facade,
        "D1",
        None,
        fields.iter().map(|(name, value)| (*name, value.clone())).collect(),
    )
    .unwrap();
}

fn device(facade: &EntityFacade) -> EntityValue {
    find_device(facade, "D1").unwrap().unwrap()
}

/// A display account, a route group and a registration of device D1; the registration code.
fn register(facade: &EntityFacade) -> String {
    create(
        facade,
        USER_ACCOUNT,
        &[
            ("userId", Value::string("WALL")),
            ("username", Value::string("display.wall")),
            ("displayAccount", Value::string("Y")),
        ],
    );
    create(
        facade,
        ROUTE_GROUP,
        &[
            ("routeGroupId", Value::string("WALLS")),
            ("artifactGroupId", Value::string("WALL_SCREENS")),
            ("parameterName", Value::string("playlistId")),
        ],
    );
    let (code, hash) = new_registration_code().unwrap();
    let now = now_millis();
    create(
        facade,
        DEVICE,
        &[
            ("deviceId", Value::string("D1")),
            ("deviceName", Value::string("Plant wall")),
            ("routeGroupId", Value::string("WALLS")),
            ("parameterValue", Value::string("OPS")),
            ("displayUserId", Value::string("WALL")),
            ("registrationCodeHash", Value::string(hash)),
            ("codeExpireDate", stamp_value(now + 15 * MINUTE)),
            ("expireDate", stamp_value(now + 24 * HOUR * 365)),
        ],
    );
    code
}

fn cookie_of_header(header: &str) -> String {
    header.split(';').next().unwrap().split_once('=').unwrap().1.to_string()
}

fn active(cookie: &str) -> Option<DeviceGuard> {
    match begin_with_cookie(cookie, "203.0.113.9", "Mozilla/5.0 Chrome/1", false).unwrap() {
        DeviceRequest::Active(guard) => Some(guard),
        _ => None,
    }
}

fn refusal(cookie: &str) -> Option<Refusal> {
    match begin_with_cookie(cookie, "203.0.113.9", "Mozilla/5.0 Chrome/1", false).unwrap() {
        DeviceRequest::Refused(refusal) => Some(refusal),
        _ => None,
    }
}

/// The code works once and neither it nor the secret is stored; the device's request runs as its display account
/// with its registered parameter, and nothing of that identity reaches the web session; a revoked device's next
/// request is refused.
#[test]
fn a_code_redeems_once_and_the_device_is_its_display_account_for_a_request_until_revoked() {
    let (_lock, ctx, facade, _key) = setup();
    let _entered = ctx.enter();
    let factory = Arc::clone(ctx.factory());
    let code = register(&facade);
    assert_eq!(
        redeem("aaaa-bbbb-cccc-dddd-eeee", "203.0.113.9", "", false)
            .unwrap()
            .unwrap_err(),
        RedeemRefusal::Unknown
    );
    let redeemed = redeem(&code.to_uppercase(), "203.0.113.9", "Chrome", true)
        .unwrap()
        .unwrap();
    assert_eq!(redeemed.device_id, "D1");
    assert!(redeemed.set_cookie.contains("; HttpOnly; SameSite=Strict; Path=/"));
    assert!(redeemed.set_cookie.ends_with("; Secure"));
    let cookie = cookie_of_header(&redeemed.set_cookie);
    let secret = cookie.split_once('.').unwrap().1.to_string();
    let stored = device(&facade);
    for value in stored.fields().values() {
        let held = moqui_lib::object::to_plain_string(value);
        assert!(
            !held.contains(&secret) && !held.contains(&code),
            "no code or secret is stored"
        );
    }
    assert_eq!(
        redeem(&code, "203.0.113.9", "", false).unwrap().unwrap_err(),
        RedeemRefusal::Used,
        "a code works once"
    );

    let session = bind(&factory, None, None, false).unwrap();
    let session_id = session.session_id();
    {
        let guard = active(&cookie).expect("the device's request");
        assert_eq!(current_user_id().unwrap().as_deref(), Some("WALL"));
        assert_eq!(guard.scope().artifact_group_id, "WALL_SCREENS");
        assert_eq!(
            guard.scope().parameter,
            Some(("playlistId".to_string(), "OPS".to_string()))
        );
        assert!(
            login::internal_login_user("display.wall").is_err(),
            "no login in a device's request"
        );
    }
    assert_eq!(current_user_id().unwrap(), None, "the identity ends with the request");
    drop(session);
    assert_eq!(
        live_session_user_id(&factory, &session_id),
        Some(None),
        "the web session never holds the display account"
    );
    assert_eq!(text(&device(&facade), "lastSeenAddress"), "203.0.113.9");

    // not its secret, then revoked
    let forged = format!("D1.{}", "x".repeat(SECRET_LENGTH));
    assert_eq!(refusal(&forged), Some(Refusal::Unknown));
    set_device(&facade, &[("revokedDate", login::now_value())]);
    assert_eq!(refusal(&cookie), Some(Refusal::Revoked));
    assert!(Refusal::Revoked.ends_cookie());
}

/// The secret is replaced each day; the one before it works for the grace and then stops the device, since only a
/// copy still presents it. A device not seen for its idle timeout stops too.
#[test]
fn a_replaced_secret_presented_after_its_grace_stops_the_device() {
    let (_lock, ctx, facade, _key) = setup();
    let _entered = ctx.enter();
    let code = register(&facade);
    let first = cookie_of_header(&redeem(&code, "", "", false).unwrap().unwrap().set_cookie);
    let now = now_millis();
    set_device(&facade, &[("rotatedDate", stamp_value(now - ROTATE_MILLIS - MINUTE))]);
    // due, but a request whose response cannot carry the new secret (an error, a streamed or cacheable answer) makes none
    drop(active(&first).unwrap());
    assert!(text(&device(&facade), "previousHash").is_empty());
    let second = {
        let mut guard = active(&first).unwrap();
        cookie_of_header(&guard.rotate().unwrap().expect("a new secret"))
    };
    assert_ne!(first, second);
    assert!(
        active(&first).is_some_and(|mut guard| guard.rotate().unwrap().is_none()),
        "the grace"
    );
    assert!(active(&second).is_some());
    set_device(
        &facade,
        &[("rotatedDate", stamp_value(now - ROTATE_GRACE_MILLIS - MINUTE))],
    );
    assert_eq!(refusal(&first), Some(Refusal::Stopped));
    assert_eq!(text(&device(&facade), "stopReason"), "copied");
    assert_eq!(
        refusal(&second),
        Some(Refusal::Stopped),
        "the device is stopped for every holder"
    );

    // idle: another device row state, not seen for longer than its timeout
    set_device(
        &facade,
        &[
            ("stoppedDate", Value::Null),
            ("stopReason", Value::Null),
            ("rotatedDate", stamp_value(now)),
            ("idleTimeoutMinutes", Value::I64(30)),
            ("lastSeenDate", stamp_value(now - 31 * MINUTE)),
        ],
    );
    assert_eq!(refusal(&second), Some(Refusal::Stopped));
    assert_eq!(text(&device(&facade), "stopReason"), "idle");
}

/// A display account is never signed in, by anything; a device stops serving while its account holds what no device
/// may (a privileged group), is disabled for good, or is given a password, and from an address it is not allowed from.
#[test]
fn a_display_account_never_signs_in_and_must_stay_fit_for_its_device() {
    let (_lock, ctx, facade, _key) = setup();
    let _entered = ctx.enter();
    let code = register(&facade);
    let cookie = cookie_of_header(&redeem(&code, "", "", false).unwrap().unwrap().set_cookie);
    create(
        &facade,
        "moqui.security.UserLoginKey",
        &[
            (
                "loginKey",
                Value::string(super::super::password::hash_login_key("K1").unwrap()),
            ),
            ("userId", Value::string("WALL")),
            ("fromDate", stamp_value(1)),
        ],
    );
    assert!(
        login::login_user_key("K1").is_err(),
        "a key does not sign a display account in"
    );
    assert!(login::internal_login_user("display.wall").is_err());
    assert!(login::login_external_user("display.wall", None).is_err());
    assert_eq!(current_user_id().unwrap(), None);
    // the work a device's request queued runs as it
    login::internal_login_queued_user("display.wall").unwrap();
    login::logout().unwrap();

    assert!(active(&cookie).is_some());
    create(
        &facade,
        "moqui.security.UserGroupMember",
        &[
            ("userGroupId", Value::string("ADMIN")),
            ("userId", Value::string("WALL")),
            ("fromDate", stamp_value(1)),
        ],
    );
    assert_eq!(refusal(&cookie), Some(Refusal::Account), "a privileged group");
    assert!(!Refusal::Account.ends_cookie());
    facade
        .find("moqui.security.UserGroupMember")
        .unwrap()
        .eq("userId", Value::string("WALL"))
        .delete_all()
        .unwrap();
    assert!(active(&cookie).is_some(), "and again once it is out of the group");

    set_device(&facade, &[("ipAllowed", Value::string("10.0.*.*"))]);
    assert_eq!(refusal(&cookie), Some(Refusal::Address));
    assert!(
        matches!(
            begin_with_cookie(&cookie, "", "", false).unwrap(),
            DeviceRequest::Refused(Refusal::Address)
        ),
        "an address that is not known matches no pattern"
    );
    set_device(&facade, &[("ipAllowed", Value::Null)]);

    let mut account = facade
        .find(USER_ACCOUNT)
        .unwrap()
        .eq("userId", Value::string("WALL"))
        .one()
        .unwrap()
        .unwrap();
    account.set("disabled", Value::string("Y"));
    account.update(&facade).unwrap();
    assert_eq!(refusal(&cookie), Some(Refusal::Account), "disabled for good");
}

#[test]
fn a_refused_display_gets_a_plain_page_with_nothing_of_the_instance() {
    for refusal in [
        Refusal::Unknown,
        Refusal::Revoked,
        Refusal::Stopped,
        Refusal::Address,
        Refusal::Account,
    ] {
        let page = refusal_page(refusal.page_text());
        assert!(page.starts_with("<!DOCTYPE html>"));
        assert!(page.contains(&crate::screen::encode_html(refusal.page_text())));
        assert!(page.contains("href=\"/Device\""));
        // no script, style sheet, image or other link of the instance's; no route group, device or account
        for absent in ["<script", "<link", "<img", "INSIGHTS", "moqui"] {
            assert!(!page.contains(absent), "{absent} in {page}");
        }
    }
    assert!(refusal_page("<b>").contains("&lt;b&gt;"));
}
