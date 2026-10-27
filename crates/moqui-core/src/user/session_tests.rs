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

use std::collections::BTreeMap;
use std::sync::{mpsc, Arc, MutexGuard};
use std::time::Duration;

use base64::Engine;

use moqui_lib::object::Value;

use super::super::login;
use super::super::password::{hash_login_key, simple_hash};
use super::*;
use crate::context::ExecutionContextFactory;
use crate::entity::{EntityFacade, EntityValue};

#[test]
fn visit_urls_keep_their_parameters_but_credentials() {
    assert_eq!(
        without_secret_parameters("https://h/apps/Order?orderId=O1&password=pw&loginKey=K&api_key=A&pageIndex=2#top"),
        "https://h/apps/Order?orderId=O1&pageIndex=2#top"
    );
    // names compared decoded and without case
    assert_eq!(
        without_secret_parameters("/p?Pass%77ord=x&moquiSessionToken=T&a=1"),
        "/p?a=1"
    );
    assert_eq!(without_secret_parameters("/p?resetToken=x"), "/p");
    assert_eq!(without_secret_parameters("/p?primaryKeyId=1&&b"), "/p?primaryKeyId=1&b");
    assert_eq!(without_secret_parameters("/p#a?password=x"), "/p#a?password=x");
    assert_eq!(without_secret_parameters(""), "");
}

#[test]
fn a_session_belongs_to_its_context_and_never_leaks_to_the_next() {
    let first = ExecutionContextFactory::test_context();
    {
        let _entered = first.enter();
        set_user("U1", "ada");
        assert_eq!(current_username().unwrap().as_deref(), Some("ada"));
    }
    assert!(current_username().unwrap().is_none());
    let second = ExecutionContextFactory::test_context();
    let _entered = second.enter();
    assert!(
        current_username().unwrap().is_none(),
        "a new context starts with no user"
    );
}

#[test]
fn session_attribute_round_trip_stays_on_this_context() {
    let ctx = ExecutionContextFactory::test_context();
    let _entered = ctx.enter();
    set_session_attribute("saved", Some("yes"));
    assert_eq!(session_attribute("saved").as_deref(), Some("yes"));
    set_session_attribute("saved", None);
    assert_eq!(session_attribute("saved"), None);
}

#[test]
fn login_and_logout_rotate_the_csrf_token() {
    let ctx = ExecutionContextFactory::test_context();
    let _entered = ctx.enter();
    let anonymous = csrf_token().unwrap();
    set_user("U1", "ada");
    let after_login = csrf_token().unwrap();
    assert_ne!(anonymous, after_login);
    clear_login();
    assert_ne!(after_login, csrf_token().unwrap());
}

#[test]
fn current_username_is_none_until_a_user_is_set() {
    let ctx = ExecutionContextFactory::test_context();
    let _entered = ctx.enter();
    reset_session();
    assert!(current_username().unwrap().is_none());
    assert!(current_user_id().unwrap().is_none());
    assert!(!is_anonymous().unwrap());
    set_user("U1", "ada");
    set_locale(Some("en"));
    set_time_zone(Some("UTC"));
    set_visit("100000", "visitor-1");
    add_flash_message("saved-flash");
    save_parameter("q", "1");
    set_error_parameter("password", "required");
    push_screen_history("Login");
    assert_eq!(current_username().unwrap().as_deref(), Some("ada"));
    assert_eq!(current_user_id().unwrap().as_deref(), Some("U1"));
    assert_eq!(locale().unwrap().as_deref(), Some("en"));
    assert_eq!(time_zone().unwrap().as_deref(), Some("UTC"));
    assert_eq!(visit_id().unwrap().as_deref(), Some("100000"));
    assert_eq!(visitor_id().unwrap().as_deref(), Some("visitor-1"));
    assert_eq!(flash_messages().unwrap(), vec!["saved-flash".to_string()]);
    assert_eq!(saved_parameters().unwrap().get("q").map(String::as_str), Some("1"));
    assert_eq!(
        error_parameters().unwrap().get("password").map(String::as_str),
        Some("required")
    );
    assert_eq!(screen_history().unwrap(), vec!["Login".to_string()]);
    let token = csrf_token().unwrap();
    assert_eq!(token.len(), SESSION_TOKEN_BYTES * 2);
    assert_eq!(csrf_token().unwrap(), token);
    assert!(!is_anonymous().unwrap());
    clear_login();
    assert!(current_username().unwrap().is_none());
    assert!(!is_anonymous().unwrap());
    assert_eq!(visit_id().unwrap().as_deref(), Some("100000"));
    reset_session();
}

/// Set and get locale `en_GB` then `en_US`, and time zone `America/Los_Angeles` then `America/Chicago`, on a user
/// the test inserts. `US/Pacific` and `US/Central` are those zones. The zone id is what is kept.
#[test]
fn set_and_get_locale_and_time_zone_on_the_inserted_user() {
    let (_lock, ctx, facade, _key) = setup();
    let _entered = ctx.enter();
    create_user(&facade, "ada", "Secret1!");
    login::internal_login_user("ada").unwrap();

    set_locale(Some("en_GB"));
    assert_eq!(locale().unwrap().as_deref(), Some("en_GB"));
    assert_eq!(crate::l10n::locale(), "en_GB");
    set_locale(Some("en_US"));
    assert_eq!(locale().unwrap().as_deref(), Some("en_US"));
    assert_eq!(crate::l10n::locale(), "en_US");

    set_time_zone(Some("America/Los_Angeles"));
    assert_eq!(time_zone().unwrap().as_deref(), Some("America/Los_Angeles"));
    assert_eq!(crate::l10n::time_zone().iana_name(), Some("America/Los_Angeles"));
    set_time_zone(Some("America/Chicago"));
    assert_eq!(time_zone().unwrap().as_deref(), Some("America/Chicago"));
    assert_eq!(crate::l10n::time_zone().iana_name(), Some("America/Chicago"));
    reset_session();
}

#[test]
fn a_bad_mac_is_an_unknown_session() {
    let (_lock, ctx, _facade, _key) = setup();
    let _entered = ctx.enter();
    let factory = Arc::clone(ctx.factory());
    let id = "ab".repeat(SESSION_ID_BYTES);
    let header = format_session_cookie(&id, false).unwrap();
    assert!(header.starts_with("moqui.session="));
    assert!(header.contains("HttpOnly"));
    assert!(header.contains("SameSite=Lax"));
    assert!(header.contains("Path=/"));
    assert!(!header.contains("Secure"));
    let secure = format_session_cookie(&id, true).unwrap();
    assert!(secure.contains("Secure"));
    let visitor = format_visitor_cookie("visitor-1", false);
    assert!(visitor.starts_with("moqui.visitor=visitor-1;"));
    assert!(visitor.contains("Max-Age=31536000"));
    assert!(visitor.contains("HttpOnly"));
    assert!(!visitor.contains("Secure"));
    assert!(format_visitor_cookie("visitor-1", true).contains("Secure"));

    let value = cookie_value(&header);
    assert_eq!(verify_session_cookie(value).unwrap().as_deref(), Some(id.as_str()));
    let bad = tamper_mac(value);
    assert!(verify_session_cookie(&bad).unwrap().is_none());
    assert!(verify_session_cookie("nope").unwrap().is_none());
    assert!(verify_session_cookie("abc.def.ghi").unwrap().is_none());

    let guard = bind(&factory, Some(&bad), None, false).unwrap();
    assert_ne!(guard.session_id(), id);
    drop(guard);
    let ghost = format_session_cookie(&id, false).unwrap();
    let guard = bind(&factory, Some(cookie_value(&ghost)), None, false).unwrap();
    assert_ne!(guard.session_id(), id);
}

/// A visit records the request that starts it, as Java's; a user ends another session of theirs by its visit (its
/// row goes, its live copy is retired, so its cookie starts over signed out, and the Visit gets its thruDate), but
/// not the current one and not another user's.
#[test]
fn a_user_ends_another_session_of_theirs_by_its_visit() {
    let (_lock, ctx, facade, _key) = setup();
    let _entered = ctx.enter();
    let factory = Arc::clone(ctx.factory());
    let other_browser = |user: &'static str| {
        let factory = Arc::clone(&factory);
        std::thread::spawn(move || {
            let ctx = factory.new_context();
            let _entered = ctx.enter();
            let _request = VisitRequestGuard::set(VisitRequest {
                url: format!("https://erp.example.com/qapps?{}", "x".repeat(300)),
                referrer: "https://example.com/start".to_string(),
                user_agent: "Firefox/140.0".to_string(),
                locale: "de_DE".to_string(),
                client_ip: "203.0.113.9".to_string(),
            });
            let guard = bind(&factory, None, None, false).unwrap();
            set_user(user, user);
            start_deferred_visit().unwrap();
            flush_dirty(&factory).unwrap();
            (
                guard.session_cookie_header().unwrap(),
                guard.session_id(),
                visit_id().unwrap().unwrap(),
            )
        })
        .join()
        .unwrap()
    };
    let (b_cookie, b_id, b_visit) = other_browser("ada");
    let (_, _, bob_visit) = other_browser("bob");
    let visit = facade
        .find(VISIT)
        .unwrap()
        .eq("visitId", Value::string(&b_visit))
        .one()
        .unwrap()
        .unwrap();
    for (field, expected) in [
        ("userId", "ada"),
        ("sessionId", b_id.as_str()),
        ("webappName", "webroot"),
        ("initialLocale", "de_DE"),
        ("initialReferrer", "https://example.com/start"),
        ("initialUserAgent", "Firefox/140.0"),
        ("clientIpAddress", "203.0.113.9"),
        ("clientHostName", "203.0.113.9"),
    ] {
        assert_eq!(text(&visit, field), expected, "{field}");
    }
    assert_eq!(
        text(&visit, "initialRequest").chars().count(),
        255,
        "cut to 255 as Java"
    );
    assert!(!text(&visit, "serverHostName").is_empty() && !text(&visit, "serverIpAddress").is_empty());

    // this browser: Ada again
    let guard = bind(&factory, None, None, false).unwrap();
    set_user("ada", "ada");
    start_deferred_visit().unwrap();
    let a_visit = visit_id().unwrap().unwrap();
    assert!(end_user_visit(&a_visit)
        .unwrap_err()
        .to_string()
        .contains("current session"));
    assert!(end_user_visit(&bob_visit)
        .unwrap_err()
        .to_string()
        .contains("not one of yours"));
    assert!(end_user_visit("NoSuchVisit").is_err());
    assert_eq!(end_user_visit(&b_visit).unwrap(), 1);
    let ended = facade
        .find(VISIT)
        .unwrap()
        .eq("visitId", Value::string(&b_visit))
        .one()
        .unwrap()
        .unwrap();
    assert!(ended.get("thruDate").is_some_and(|value| !value.is_null()));
    assert!(
        load_session_optional(&facade, &b_id).is_none(),
        "the session row is gone"
    );
    assert!(lock_sessions(&factory)
        .sessions
        .get(&b_id)
        .is_some_and(|live| live.retired));
    assert_eq!(end_user_visit(&b_visit).unwrap(), 0, "nothing left to end");
    drop(guard);
    // the other browser's next request starts over, signed out
    let factory_b = Arc::clone(&factory);
    let (id_after, user_after) = std::thread::spawn(move || {
        let ctx = factory_b.new_context();
        let _entered = ctx.enter();
        let guard = bind(&factory_b, Some(cookie_value(&b_cookie)), None, false).unwrap();
        (guard.session_id(), current_user_id().unwrap())
    })
    .join()
    .unwrap();
    assert_ne!(id_after, b_id);
    assert_eq!(user_after, None);
}

/// Another server ended a session this one holds live (its `end#UserVisit` deleted the row and published
/// `session-end`): the live copy is dropped, so its cookie starts over signed out and no flush writes the row back.
#[test]
fn a_session_ended_on_another_server_is_dropped_here() {
    let (_lock, ctx, facade, _key) = setup();
    let _entered = ctx.enter();
    let factory = Arc::clone(ctx.factory());
    let browser = Arc::clone(&factory);
    let (cookie, id) = std::thread::spawn(move || {
        let ctx = browser.new_context();
        let _entered = ctx.enter();
        let guard = bind(&browser, None, None, false).unwrap();
        set_user("ada", "ada");
        flush_dirty(&browser).unwrap();
        (guard.session_cookie_header().unwrap(), guard.session_id())
    })
    .join()
    .unwrap();
    assert!(load_session_optional(&facade, &id).is_some());
    // the other server's end#UserVisit: its row goes, then the message arrives here
    delete_session_row(&facade, &id).unwrap();
    let message = crate::notify::TopicMessage {
        origin_id: "another-server".to_string(),
        message_id: crate::entity::uuid_v4().unwrap(),
        body: serde_json::json!({ "sessionIds": [id] }),
    };
    crate::cluster::apply_remote(&facade, crate::notify::SESSION_END, &message).unwrap();
    assert!(lock_sessions(&factory)
        .sessions
        .get(&id)
        .is_some_and(|live| live.retired));
    assert!(!adopt_presented(&factory, &id).unwrap());
    let browser = Arc::clone(&factory);
    let (id_after, user_after) = std::thread::spawn(move || {
        let ctx = browser.new_context();
        let _entered = ctx.enter();
        let guard = bind(&browser, Some(cookie_value(&cookie)), None, false).unwrap();
        (guard.session_id(), current_user_id().unwrap())
    })
    .join()
    .unwrap();
    assert_ne!(id_after, id);
    assert_eq!(user_after, None);
    flush_dirty(&factory).unwrap();
    assert!(load_session_optional(&facade, &id).is_none(), "not written back");
}

/// A flush of a session whose row was deleted elsewhere (before this server heard of it) does not insert the row
/// again: the session is retired. A session never written still gets its row.
#[test]
fn a_flush_after_the_row_was_deleted_elsewhere_does_not_recreate_it() {
    let (_lock, ctx, facade, _key) = setup();
    let _entered = ctx.enter();
    let factory = Arc::clone(ctx.factory());
    let browser = Arc::clone(&factory);
    let reader = facade.clone();
    let id = std::thread::spawn(move || {
        let ctx = browser.new_context();
        let _entered = ctx.enter();
        let guard = bind(&browser, None, None, false).unwrap();
        set_user("ada", "ada");
        flush_dirty(&browser).unwrap();
        let id = guard.session_id();
        assert!(
            load_session_optional(&reader, &id).is_some(),
            "a new session gets its row"
        );
        delete_session_row(&reader, &id).unwrap();
        set_session_attribute("after.delete", Some("1"));
        flush_dirty(&browser).unwrap();
        id
    })
    .join()
    .unwrap();
    assert!(
        load_session_optional(&facade, &id).is_none(),
        "the deleted row stays deleted"
    );
    assert!(lock_sessions(&factory)
        .sessions
        .get(&id)
        .is_some_and(|live| live.retired));
}

#[test]
fn flush_then_drop_the_map_reloads_the_signed_cookie() {
    let (_lock, ctx, _facade, _key) = setup();
    let _entered = ctx.enter();
    let factory = Arc::clone(ctx.factory());
    let guard = bind(&factory, None, None, false).unwrap();
    save_parameter("color", "blue");
    flush_dirty(&factory).unwrap();
    let id = guard.session_id();
    let cookie = guard.session_cookie_header().unwrap();
    drop(guard);
    clear_sessions(&factory);
    let guard = bind(&factory, Some(cookie_value(&cookie)), None, false).unwrap();
    assert_eq!(guard.session_id(), id);
    assert_eq!(
        saved_parameters().unwrap().get("color").map(String::as_str),
        Some("blue")
    );
}

#[test]
fn concurrent_requests_build_on_each_others_attribute_updates() {
    let (_lock, ctx, _facade, _key) = setup();
    let _entered = ctx.enter();
    let factory = Arc::clone(ctx.factory());
    let append = |item: &'static str| {
        move |current: Option<String>| Some(current.map_or_else(|| item.to_string(), |text| format!("{text},{item}")))
    };
    let guard = bind(&factory, None, None, false).unwrap();
    let id = guard.session_id();
    let cookie = guard.session_cookie_header().unwrap();
    update_session_attribute("ids", append("a"));
    // A second request of the session, while this one is still bound.
    std::thread::spawn({
        let factory = Arc::clone(&factory);
        move || {
            let ctx = factory.new_context();
            let _entered = ctx.enter();
            let _guard = bind(&factory, Some(cookie_value(&cookie)), None, false).unwrap();
            update_session_attribute("ids", append("b"));
        }
    })
    .join()
    .unwrap();
    update_session_attribute("ids", append("c"));
    assert_eq!(session_attribute("ids").as_deref(), Some("a,b,c"));
    drop(guard);
    let live = lock_sessions(&factory)
        .sessions
        .get(&id)
        .unwrap()
        .attributes
        .get("ids")
        .cloned();
    assert_eq!(live, Some(Json::String("a,b,c".into())));
}

#[test]
fn an_idle_session_expires_and_its_row_is_deleted() {
    let (_lock, ctx, facade, _key) = setup();
    let _entered = ctx.enter();
    let factory = Arc::clone(ctx.factory());
    let guard = bind(&factory, None, None, false).unwrap();
    save_parameter("color", "blue");
    flush_dirty(&factory).unwrap();
    let id = guard.session_id();
    let cookie = guard.session_cookie_header().unwrap();
    drop(guard);
    assert!(read_row(&facade, &id).unwrap().is_some());

    // Two hours idle against the 60 minute default.
    lock_sessions(&factory).sessions.get_mut(&id).unwrap().last_access = now_millis() - 2 * 60 * 60_000;
    let guard = bind(&factory, Some(cookie_value(&cookie)), None, false).unwrap();
    assert_ne!(guard.session_id(), id);
    assert!(saved_parameters().unwrap().is_empty());
    assert!(read_row(&facade, &id).unwrap().is_none());
}

#[test]
fn the_sweep_drops_idle_sessions_and_keeps_active_ones() {
    let (_lock, ctx, _facade, _key) = setup();
    let _entered = ctx.enter();
    let factory = Arc::clone(ctx.factory());
    let guard = bind(&factory, None, None, false).unwrap();
    let active = guard.session_id();
    drop(guard);
    let stale = random_hex(SESSION_ID_BYTES).unwrap();
    {
        let mut table = lock_sessions(&factory);
        let mut live = table.sessions.get(&active).unwrap().clone();
        live.last_access = now_millis() - 3 * 60 * 60_000;
        table.sessions.insert(stale.clone(), live);
    }
    expire_idle(&factory).unwrap();
    let table = lock_sessions(&factory);
    assert!(table.sessions.contains_key(&active));
    assert!(!table.sessions.contains_key(&stale));
}

#[test]
fn dirty_merge_keeps_a_clean_json_key() {
    let (_lock, ctx, facade, _key) = setup();
    let _entered = ctx.enter();
    let factory = Arc::clone(ctx.factory());
    let guard = bind(&factory, None, None, false).unwrap();
    save_parameter("a", "one");
    set_error_parameter("b", "two");
    flush_dirty(&factory).unwrap();
    let id = guard.session_id();
    invalidate(&id);
    let replacement = attributes_json("from-db", "two");
    exec_update(
        &facade,
        &["attributes"],
        &[Value::string(&replacement), Value::string(&id)],
    );
    set_error_parameter("b", "new");
    flush_dirty(&factory).unwrap();
    let row = load_session(&facade, &id);
    let map = parse_attributes(&text(&row, "attributes")).unwrap();
    assert_eq!(map.get(SAVED), Some(&serde_json::json!({"a": "from-db"})));
    assert_eq!(map.get(ERROR_PARAMETERS), Some(&serde_json::json!({"b": "new"})));
    assert_eq!(map.get("moqui.second.factor"), Some(&Json::String("kept".to_string())));
}

#[test]
fn revision_conflict_retries_and_keeps_the_clean_column() {
    let (_lock, ctx, facade, _key) = setup();
    let _entered = ctx.enter();
    let factory = Arc::clone(ctx.factory());
    let guard = bind(&factory, None, None, false).unwrap();
    set_locale(Some("en"));
    save_parameter("q", "1");
    flush_dirty(&factory).unwrap();
    let id = guard.session_id();
    exec_update(
        &facade,
        &["locale", "sessionRevision"],
        &[Value::string("fr"), Value::I64(7), Value::string(&id)],
    );
    save_parameter("q", "2");
    flush_dirty(&factory).unwrap();
    let row = load_session(&facade, &id);
    assert_eq!(text(&row, "locale"), "fr");
    assert_eq!(i64_field(&row, "sessionRevision"), 8);
    let map = parse_attributes(&text(&row, "attributes")).unwrap();
    assert_eq!(map.get(SAVED), Some(&serde_json::json!({"q": "2"})));
}

#[test]
fn password_login_rotates_and_a_failed_password_keeps_the_id() {
    let (_lock, ctx, facade, _key) = setup();
    let _entered = ctx.enter();
    let factory = Arc::clone(ctx.factory());
    create_user(&facade, "ada", "Secret1!");
    let guard = bind(&factory, None, None, false).unwrap();
    flush_dirty(&factory).unwrap();
    let old_id = guard.session_id();
    let old_csrf = csrf_token().unwrap();
    assert!(login::login_user("ada", "wrong").is_err());
    assert_eq!(guard.session_id(), old_id);
    assert!(current_username().unwrap().is_none());
    assert!(load_session_optional(&facade, &old_id).is_some());

    assert!(visit_id().unwrap().is_none());
    login::login_user("ada", "Secret1!").unwrap();
    let new_id = guard.session_id();
    assert_ne!(new_id, old_id);
    assert_eq!(current_username().unwrap().as_deref(), Some("ada"));
    let visit = visit_id()
        .unwrap()
        .expect("a login starts the visit the session put off");
    let visit = facade
        .find(VISIT)
        .unwrap()
        .eq("visitId", Value::string(&visit))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(text(&visit, "userId"), "ada");
    let new_csrf = csrf_token().unwrap();
    assert_ne!(new_csrf, old_csrf);
    assert_eq!(new_csrf.len(), SESSION_TOKEN_BYTES * 2);
    assert!(load_session_optional(&facade, &old_id).is_none());
    flush_dirty(&factory).unwrap();
    let row = load_session(&facade, &new_id);
    assert_eq!(text(&row, "userId"), "ada");

    login::logout().unwrap();
    let stayed = guard.session_id();
    login::internal_login_user("ada").unwrap();
    assert_eq!(guard.session_id(), stayed);
    login::logout().unwrap();
}

#[test]
fn guarantee_flush_survives_clearing_the_factory_map() {
    let (_lock, ctx, _facade, _key) = setup();
    let _entered = ctx.enter();
    let factory = Arc::clone(ctx.factory());

    let guard = bind(&factory, None, None, false).unwrap();
    save_parameter("k", "v");
    end_request().unwrap();
    let cookie = guard.session_cookie_header().unwrap();
    drop(guard);
    clear_sessions(&factory);
    let guard = bind(&factory, Some(cookie_value(&cookie)), None, false).unwrap();
    assert!(!saved_parameters().unwrap().contains_key("k"));
    drop(guard);

    lock_sessions(&factory).mode = FlushMode::Guarantee;
    let guard = bind(&factory, None, None, false).unwrap();
    save_parameter("k", "v");
    end_request().unwrap();
    let id = guard.session_id();
    let cookie = guard.session_cookie_header().unwrap();
    drop(guard);
    clear_sessions(&factory);
    let guard = bind(&factory, Some(cookie_value(&cookie)), None, false).unwrap();
    assert_eq!(guard.session_id(), id);
    assert_eq!(saved_parameters().unwrap().get("k").map(String::as_str), Some("v"));
}

#[test]
fn login_key_header_logs_in_and_an_expired_key_does_not() {
    let (_lock, ctx, facade, _key) = setup();
    let _entered = ctx.enter();
    let factory = Arc::clone(ctx.factory());
    create_user(&facade, "ada", "Secret1!");
    create_login_key(&facade, "temp-key", "ada", None);
    create_login_key(&facade, "old-key", "ada", Some(1));
    let guard = bind(&factory, None, None, false).unwrap();
    let id = guard.session_id();
    login::apply_header_keys(Some("temp-key"), None).unwrap();
    assert_eq!(current_username().unwrap().as_deref(), Some("ada"));
    assert_eq!(guard.session_id(), id);
    login::logout().unwrap();
    assert!(login::apply_header_keys(None, Some("old-key")).is_err());
    assert!(current_username().unwrap().is_none());
    assert_eq!(guard.session_id(), id);
    login::login_user("ada", "Secret1!").unwrap();
    let rotated = guard.session_id();
    login::apply_header_keys(Some("not-a-key"), None).unwrap();
    assert_eq!(current_username().unwrap().as_deref(), Some("ada"));
    assert_eq!(guard.session_id(), rotated);
}

#[test]
fn a_query_string_api_key_is_not_consulted() {
    let (_lock, ctx, facade, _key) = setup();
    let _entered = ctx.enter();
    create_user(&facade, "ada", "Secret1!");
    create_login_key(&facade, "temp-key", "ada", None);
    save_parameter("api_key", "temp-key");
    let mut other = BTreeMap::new();
    other.insert("api_key".to_string(), "temp-key".to_string());
    login::apply_header_keys(None, None).unwrap();
    assert!(current_username().unwrap().is_none());
    assert_eq!(other.get("api_key").map(String::as_str), Some("temp-key"));
}

#[test]
fn one_visit_per_bound_session_reuses_a_known_visitor() {
    let (_lock, ctx, facade, _key) = setup();
    let _entered = ctx.enter();
    let factory = Arc::clone(ctx.factory());
    let guard = bind(&factory, None, None, false).unwrap();
    let session_id = guard.session_id();
    assert!(
        visit_id().unwrap().is_none(),
        "a request with no cookie starts no visit"
    );
    assert!(guard.visitor_cookie_header().is_none());
    assert_eq!(count_entity(&facade, VISITOR), 0);
    let session_cookie = guard.session_cookie_header().unwrap();
    drop(guard);

    let guard = bind(&factory, Some(cookie_value(&session_cookie)), None, false).unwrap();
    assert_eq!(guard.session_id(), session_id);
    let visit = visit_id().unwrap().unwrap();
    let visitor = guard.visitor_id().unwrap();
    let visitor_cookie = guard.visitor_cookie_header().unwrap();
    drop(guard);
    let guard = bind(
        &factory,
        Some(cookie_value(&session_cookie)),
        Some(cookie_value(&visitor_cookie)),
        false,
    )
    .unwrap();
    assert_eq!(guard.session_id(), session_id);
    assert_eq!(visit_id().unwrap().as_deref(), Some(visit.as_str()));
    assert_eq!(guard.visitor_id().as_deref(), Some(visitor.as_str()));
    assert_eq!(count_entity(&facade, VISIT), 1);
    drop(guard);

    let guard = bind(&factory, None, Some(&visitor), false).unwrap();
    assert_ne!(guard.session_id(), session_id);
    assert_eq!(guard.visitor_id().as_deref(), Some(visitor.as_str()));
    assert_ne!(visit_id().unwrap().as_deref(), Some(visit.as_str()));
    assert_eq!(count_entity(&facade, VISIT), 2);
    assert_eq!(count_entity(&facade, VISITOR), 1);
    drop(guard);

    let guard = bind(&factory, None, Some("missing-visitor"), false).unwrap();
    assert_ne!(guard.visitor_id().as_deref(), Some("missing-visitor"));
    assert_eq!(count_entity(&facade, VISITOR), 2);
}

/// The visit gets the client address once: a later request from the same address of the session writes nothing,
/// and one from a new address writes it.
#[test]
fn the_client_address_is_written_to_the_visit_only_when_it_changes() {
    let (_lock, ctx, facade, _key) = setup();
    let _entered = ctx.enter();
    let factory = Arc::clone(ctx.factory());
    let session_cookie = bind(&factory, None, None, false)
        .unwrap()
        .session_cookie_header()
        .unwrap();
    let visit_row = |visit: &str| {
        facade
            .find(VISIT)
            .unwrap()
            .eq("visitId", Value::string(visit))
            .use_cache(false)
            .one()
            .unwrap()
            .unwrap()
    };
    let request = |address: &str| {
        let guard = bind(&factory, Some(cookie_value(&session_cookie)), None, false).unwrap();
        remember_client_ip(address).unwrap();
        let visit = visit_id().unwrap().unwrap();
        drop(guard);
        visit
    };
    let visit = request("10.0.0.1");
    let first = visit_row(&visit);
    assert_eq!(first.get("clientIpAddress"), Some(&Value::string("10.0.0.1")));
    std::thread::sleep(std::time::Duration::from_millis(5));
    request("10.0.0.1");
    assert_eq!(
        visit_row(&visit).get("lastUpdatedStamp"),
        first.get("lastUpdatedStamp"),
        "the same address wrote the visit again"
    );
    request("10.0.0.2");
    assert_eq!(
        visit_row(&visit).get("clientIpAddress"),
        Some(&Value::string("10.0.0.2"))
    );
}

#[test]
fn requests_bind_and_touch_while_a_flush_waits_on_sql() {
    let (_lock, ctx, facade, _key) = setup();
    let _entered = ctx.enter();
    let factory = Arc::clone(ctx.factory());
    let guard = bind(&factory, None, None, false).unwrap();
    save_parameter("color", "blue");
    let id = guard.session_id();
    let cookie = guard.session_cookie_header().unwrap();
    drop(guard);

    let blocked = BlockedFlush::new(&id);
    let flusher = {
        let factory = Arc::clone(&factory);
        let id = id.clone();
        std::thread::spawn(move || flush_id(&factory, &id))
    };
    blocked.entered.recv().unwrap();
    let (done, finished) = mpsc::channel();
    let request = {
        let factory = Arc::clone(&factory);
        std::thread::spawn(move || {
            let ctx = factory.new_context();
            let _entered = ctx.enter();
            let touched = bind(&factory, Some(cookie_value(&cookie)), None, false).map(|guard| guard.session_id());
            let fresh = bind(&factory, None, None, false).map(|guard| guard.session_id());
            done.send((touched, fresh)).unwrap();
        })
    };
    // The deadline only turns a regression into a failure instead of a hang.
    let (touched, fresh) = finished
        .recv_timeout(Duration::from_secs(60))
        .expect("bind waited for the flush");
    assert_eq!(touched.unwrap(), id);
    assert_ne!(fresh.unwrap(), id);
    assert!(read_row(&facade, &id).unwrap().is_none(), "the flush is still blocked");
    blocked.release();
    flusher.join().unwrap().unwrap();
    request.join().unwrap();
    let row = load_session(&facade, &id);
    let saved = parse_attributes(&text(&row, "attributes")).unwrap();
    assert_eq!(saved.get(SAVED), Some(&serde_json::json!({"color": "blue"})));
}

#[test]
fn a_write_during_a_flush_stays_dirty_and_is_written_next() {
    let (_lock, ctx, facade, _key) = setup();
    let _entered = ctx.enter();
    let factory = Arc::clone(ctx.factory());
    let guard = bind(&factory, None, None, false).unwrap();
    save_parameter("color", "blue");
    let id = guard.session_id();
    let cookie = guard.session_cookie_header().unwrap();
    drop(guard);

    let blocked = BlockedFlush::new(&id);
    let flusher = {
        let factory = Arc::clone(&factory);
        let id = id.clone();
        std::thread::spawn(move || flush_id(&factory, &id))
    };
    blocked.entered.recv().unwrap();
    let guard = bind(&factory, Some(cookie_value(&cookie)), None, false).unwrap();
    save_parameter("color", "red");
    drop(guard);
    blocked.release();
    flusher.join().unwrap().unwrap();

    let saved = |facade: &EntityFacade| {
        let row = load_session(facade, &id);
        parse_attributes(&text(&row, "attributes")).unwrap().remove(SAVED)
    };
    assert_eq!(saved(&facade), Some(serde_json::json!({"color": "blue"})));
    {
        let table = lock_sessions(&factory);
        let live = table.sessions.get(&id).unwrap();
        assert!(needs_flush(live));
        assert!(live.dirty_json.contains(SAVED));
    }
    flush_dirty(&factory).unwrap();
    assert_eq!(saved(&facade), Some(serde_json::json!({"color": "red"})));
    assert!(!needs_flush(lock_sessions(&factory).sessions.get(&id).unwrap()));
}

#[test]
fn cookieless_requests_keep_the_table_within_its_cap() {
    let (_lock, ctx, facade, _key) = setup();
    let _entered = ctx.enter();
    let factory = Arc::clone(ctx.factory());
    lock_sessions(&factory).max_live = Some(5);
    for _ in 0..20 {
        drop(bind(&factory, None, None, false).unwrap());
        assert!(lock_sessions(&factory).sessions.len() <= 5);
    }
    assert_eq!(count_entity(&facade, VISITOR), 0);
    assert_eq!(count_entity(&facade, VISIT), 0);
}

#[test]
fn eviction_writes_a_dirty_session_before_dropping_it() {
    let (_lock, ctx, facade, _key) = setup();
    let _entered = ctx.enter();
    let factory = Arc::clone(ctx.factory());
    lock_sessions(&factory).max_live = Some(3);
    let guard = bind(&factory, None, None, false).unwrap();
    save_parameter("color", "blue");
    let id = guard.session_id();
    let cookie = guard.session_cookie_header().unwrap();
    drop(guard);
    assert!(read_row(&facade, &id).unwrap().is_none());
    // Sessions made in the same millisecond tie, so this one is made the oldest.
    lock_sessions(&factory).sessions.get_mut(&id).unwrap().last_access -= 60_000;
    for _ in 0..5 {
        drop(bind(&factory, None, None, false).unwrap());
    }
    assert!(!lock_sessions(&factory).sessions.contains_key(&id));
    let guard = bind(&factory, Some(cookie_value(&cookie)), None, false).unwrap();
    assert_eq!(guard.session_id(), id);
    assert_eq!(
        saved_parameters().unwrap().get("color").map(String::as_str),
        Some("blue")
    );
}

#[test]
fn stored_attributes_keep_their_text_and_bad_ones_are_errors() {
    let stored = r#"{"a":{"b":[1.50,true,null],"z":"x\"y"},"emoji":"😀","n":12345678901234567890123}"#;
    let parsed = parse_attributes(stored).unwrap();
    assert_eq!(parsed.get("emoji"), Some(&Json::String("\u{1F600}".to_string())));
    assert_eq!(
        attribute_text(parsed.get("n")).as_deref(),
        Some("12345678901234567890123")
    );
    assert_eq!(
        encode_attributes(&parsed),
        "{\"a\":{\"b\":[1.50,true,null],\"z\":\"x\\\"y\"},\"emoji\":\"\u{1F600}\",\"n\":12345678901234567890123}"
    );
    assert!(parse_attributes("[1]").is_err());
    let deep = format!("{{\"a\":{}{}}}", "[".repeat(100_000), "]".repeat(100_000));
    assert!(parse_attributes(&deep).is_err());
}

type FlushHook = Arc<dyn Fn() + Send + Sync>;

static FLUSH_HOOK: std::sync::Mutex<Option<(String, FlushHook)>> = std::sync::Mutex::new(None);

/// Called by [`flush_id`] after it copied what it writes and before its SQL.
pub(super) fn flush_hook(id: &str) {
    let hook = lock(&FLUSH_HOOK)
        .as_ref()
        .filter(|(hook_id, _)| hook_id == id)
        .map(|(_, hook)| Arc::clone(hook));
    if let Some(hook) = hook {
        hook();
    }
}

/// Holds the flushes of one session id at [`flush_hook`] until released.
struct BlockedFlush {
    entered: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
}

impl BlockedFlush {
    fn new(id: &str) -> Self {
        let (entered_tx, entered) = mpsc::channel();
        let (release, release_rx) = mpsc::channel::<()>();
        let release_rx = std::sync::Mutex::new(release_rx);
        let hook: FlushHook = Arc::new(move || {
            let _ = entered_tx.send(());
            let _ = lock(&release_rx).recv();
        });
        *lock(&FLUSH_HOOK) = Some((id.to_string(), hook));
        Self { entered, release }
    }

    fn release(self) {
        *lock(&FLUSH_HOOK) = None;
        let _ = self.release.send(());
    }
}

struct KeyGuard;

impl KeyGuard {
    fn install() -> Self {
        moqui_lib::env::set_property("session_cookie_key", test_cookie_key());
        Self
    }
}

impl Drop for KeyGuard {
    fn drop(&mut self) {
        moqui_lib::env::clear_property("session_cookie_key");
        reset_session();
    }
}

fn setup() -> (
    MutexGuard<'static, ()>,
    crate::context::ExecutionContext,
    EntityFacade,
    KeyGuard,
) {
    let lock = cookie_tests_lock();
    reset_session();
    let key = KeyGuard::install();
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    (lock, ctx, facade, key)
}

fn cookie_tests_lock() -> MutexGuard<'static, ()> {
    super::cookie_property_lock()
}

fn test_cookie_key() -> String {
    STANDARD.encode([9_u8; 32])
}

fn clear_sessions(factory: &ExecutionContextFactory) {
    lock_sessions(factory).sessions.clear();
}

fn cookie_value(header: &str) -> &str {
    header.split(';').next().unwrap().split_once('=').unwrap().1
}

fn tamper_mac(value: &str) -> String {
    let (id, mac) = value.split_once('.').unwrap();
    let mut raw = URL_SAFE_NO_PAD.decode(mac).unwrap();
    raw[0] ^= 0x01;
    format!("{id}.{}", URL_SAFE_NO_PAD.encode(raw))
}

fn attributes_json(saved: &str, error: &str) -> String {
    let mut saved_map = BTreeMap::new();
    saved_map.insert("a".to_string(), saved.to_string());
    let mut error_map = BTreeMap::new();
    error_map.insert("b".to_string(), error.to_string());
    let mut root = BTreeMap::new();
    root.insert(SAVED.to_string(), json_string_map(&saved_map));
    root.insert(ERROR_PARAMETERS.to_string(), json_string_map(&error_map));
    root.insert("moqui.second.factor".to_string(), Json::String("kept".to_string()));
    encode_attributes(&root)
}

fn exec_update(facade: &EntityFacade, columns: &[&str], params: &[Value]) {
    let sql = facade
        .with_entity(HTTP_SESSION, |entity, dialect, _| {
            let condition = moqui_db::sql::Condition::bound("sessionId", moqui_db::sql::CompareOp::Equals);
            moqui_db::sql::update(entity, dialect, columns, Some(&condition)).map(|statement| statement.sql)
        })
        .unwrap();
    let tx = facade.transactions_for(HTTP_SESSION).unwrap();
    crate::entity::exec::with_reserve_tx(&tx, |conn| {
        conn.execute(&sql, params)?;
        Ok(())
    })
    .unwrap();
}

fn create_user(facade: &EntityFacade, username: &str, password: &str) {
    let stored = simple_hash(password, "s4lt", "SHA-256", false).unwrap();
    let mut value = facade.make_value(USER_ACCOUNT).unwrap();
    value.set("userId", Value::string(username));
    value.set("username", Value::string(username));
    value.set("currentPassword", Value::string(stored));
    value.set("passwordSalt", Value::string("s4lt"));
    value.set("passwordHashType", Value::string("SHA-256"));
    value.set("passwordBase64", Value::string("N"));
    value.create(facade).unwrap();
}

fn create_login_key(facade: &EntityFacade, key: &str, user_id: &str, thru_millis: Option<i64>) {
    let mut row = facade.make_value("moqui.security.UserLoginKey").unwrap();
    row.set("loginKey", Value::string(hash_login_key(key).unwrap()));
    row.set("userId", Value::string(user_id));
    row.set("fromDate", stamp(1));
    if let Some(millis) = thru_millis {
        row.set("thruDate", stamp(millis));
    }
    row.create(facade).unwrap();
}

fn load_session(facade: &EntityFacade, id: &str) -> EntityValue {
    load_session_optional(facade, id).unwrap_or_else(|| panic!("missing session {id}"))
}

fn load_session_optional(facade: &EntityFacade, id: &str) -> Option<EntityValue> {
    facade
        .find(HTTP_SESSION)
        .unwrap()
        .eq("sessionId", Value::string(id))
        .one()
        .unwrap()
}

fn count_entity(facade: &EntityFacade, name: &str) -> usize {
    facade.find(name).unwrap().list().unwrap().len()
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
    moqui_lib::object::basic_convert(&Value::I64(millis), "java.sql.Timestamp").value
}

#[test]
fn ending_a_visit_continues_after_one_session_delete_fails() {
    let (_lock, ctx, facade, _key) = setup();
    let _entered = ctx.enter();
    set_user("ada", "ada");
    let mut visit = facade.make_value(VISIT).unwrap();
    visit.set("visitId", Value::string("independent"));
    visit.set("userId", Value::string("ada"));
    visit.create(&facade).unwrap();
    for id in ["a", "b", "c"] {
        let mut row = facade.make_value(HTTP_SESSION).unwrap();
        row.set("sessionId", Value::string(id));
        row.set("visitId", Value::string("independent"));
        row.create(&facade).unwrap();
    }
    let published = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = Arc::clone(&published);
    let _subscription = crate::notify::subscribe(crate::notify::SESSION_END, Arc::new(move |message| {
        if message.body["sessionIds"].as_array().is_some_and(|ids| ids.iter().any(|id| id == "a")) {
            captured.lock().unwrap().push(message.body.clone());
        }
    })).unwrap();
    let attempted = std::cell::RefCell::new(Vec::new());
    let tx = facade.transactions_for(VISIT).unwrap();
    tx.run(crate::transaction::TransactionAttribute::UseOrBegin, None, || {
        let ended = end_user_visit_using("independent", |facade, id| {
            attempted.borrow_mut().push(id.to_string());
            if id == "b" {
                Err(Error::new("injected delete failure"))
            } else {
                delete_session_row(facade, id)
            }
        })
        .unwrap();
        assert_eq!(ended, 2);
        Ok(())
    })
    .unwrap();
    assert_eq!(*attempted.borrow(), ["a", "b", "c"]);
    assert_eq!(published.lock().unwrap().as_slice(), &[serde_json::json!({"sessionIds":["a","c"]})]);
    assert!(load_session_optional(&facade, "a").is_none());
    assert!(load_session_optional(&facade, "b").is_some());
    assert!(load_session_optional(&facade, "c").is_none());
    let visit = facade
        .find(VISIT)
        .unwrap()
        .eq("visitId", Value::string("independent"))
        .one()
        .unwrap()
        .unwrap();
    assert!(visit.get("thruDate").is_some_and(|value| !value.is_null()));
}
