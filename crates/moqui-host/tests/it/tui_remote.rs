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

//! `moqui tui --server` (plans/archive/Screen-TUI-Impl.md 4.3): the HTTP link against a served runtime. A screen asks for the
//! login (401, not the browser's redirect to /Login), the login keeps a key and never the password, a transition posts
//! with the key alone and brings its message, and a key the server does not know is a login again.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum", feature = "tui"))]

use std::collections::BTreeMap;

use moqui_host::tui_remote::HttpLink;
use moqui_tui::{LinkError, ScreenLink};

const USERS: &str = "/apps/system/Security/UserAccount/UserAccountList";

#[test]
fn the_remote_link_logs_in_with_a_key_and_reads_screens() {
    let gate = crate::common::start("tui_remote");
    let server = format!("http://{}", gate.server.address());
    let key_file = gate.runtime.join("tui-key.toml");
    let none = BTreeMap::new();

    let link = HttpLink::connect(&server, Some(key_file.clone())).unwrap();
    assert!(
        matches!(link.level(USERS, &none), Err(LinkError::Auth(_))),
        "a screen asks for the login"
    );
    assert!(link.login("john.doe", "wrong", "").is_err());
    link.login("john.doe", "moqui", "").unwrap();
    let stored = std::fs::read_to_string(&key_file).unwrap();
    assert!(stored.contains("loginKey") && !stored.contains("moqui\""), "{stored}");

    let model = link.level(USERS, &none).unwrap();
    assert!(model.screen.path.ends_with("UserAccountList"), "{}", model.screen.path);
    let shell = link.shell(USERS).unwrap();
    assert_eq!(shell.user, "john.doe");
    assert!(!shell.menu.is_empty(), "the drawer of the app");

    // A posted transition, with the key and no session token.
    let posted = link
        .transition(
            "/apps/system/ThreadList/interruptThread",
            "POST",
            &BTreeMap::from([("threadName".to_string(), "nobody".to_string())]),
        )
        .unwrap();
    assert!(
        posted.messages.info.iter().any(|text| text.contains("not interrupted")),
        "{posted:?}"
    );

    // A notification for john.doe comes over /notws, with the key.
    let (tx, rx) = std::sync::mpsc::channel();
    let subscription = link
        .subscribe(std::sync::Arc::new(move |push| {
            let _ = tx.send(push);
        }))
        .expect("the remote link pushes");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let pushed = loop {
        send_note(gate.server.factory(), "EX_JOHN_DOE", "TuiRemote", "For the terminal");
        if let Ok(push) = rx.recv_timeout(std::time::Duration::from_millis(500)) {
            break push;
        }
        assert!(std::time::Instant::now() < deadline, "no push came");
    };
    assert_eq!(pushed.topic, "TuiRemote");
    assert_eq!(pushed.title, "For the terminal");
    drop(subscription);

    // The stored key serves the next run with no login.
    let again = HttpLink::connect(&server, Some(key_file.clone())).unwrap();
    assert!(again.level(USERS, &none).is_ok());
    // A key the server does not know is a login again.
    std::fs::write(&key_file, "user = \"john.doe\"\nloginKey = \"not-a-key\"\n").unwrap();
    let stale = HttpLink::connect(&server, Some(key_file)).unwrap();
    assert!(matches!(stale.level(USERS, &none), Err(LinkError::Auth(_))));

    // An account with a second factor: the code it wants, a wrong one, the right one; and a code sent in the session
    // the password started (a factor of another user is refused by name, so the pre-authentication held).
    add_code_user(gate.server.factory());
    let coded = HttpLink::connect(&server, None).unwrap();
    match coded.login("code.user", "moqui", "") {
        Err(LinkError::Code(ask)) => assert!(ask.text.starts_with("Enter the authentication code"), "{ask:?}"),
        other => panic!("a code is wanted: {other:?}"),
    }
    match coded.login("code.user", "moqui", "111111") {
        Err(LinkError::Code(ask)) => assert_eq!(ask.text, "Authentication code is not valid"),
        other => panic!("a wrong code: {other:?}"),
    }
    match coded.send_code("code.user", "moqui", "OTHER_FACTOR") {
        Err(LinkError::Remote(text)) => assert!(text.contains("OTHER_FACTOR"), "{text}"),
        other => panic!("another user's factor is refused: {other:?}"),
    }
    coded.login("code.user", "moqui", "246810").unwrap();
    // Logged in: a screen it may not view is refused, not a login again.
    assert!(!matches!(coded.level(USERS, &none), Err(LinkError::Auth(_))));
}

/// `code.user`, john.doe's password and a single-use code `246810`; `OTHER_FACTOR` is john.doe's.
fn add_code_user(factory: &std::sync::Arc<moqui_core::context::ExecutionContextFactory>) {
    use moqui_lib::object::Value;
    let ctx = factory.new_context();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let facade = factory.entity().unwrap();
    let john = facade
        .find("moqui.security.UserAccount")
        .unwrap()
        .eq("username", Value::string("john.doe"))
        .one()
        .unwrap()
        .unwrap();
    let mut account = facade.make_value("moqui.security.UserAccount").unwrap();
    for field in ["currentPassword", "passwordHashType", "passwordSalt", "passwordBase64"] {
        if let Some(value) = john.get(field) {
            account.set(field, value.clone());
        }
    }
    account.set("userId", Value::string("CODE_USER"));
    account.set("username", Value::string("code.user"));
    account.create(&facade).unwrap();
    for (factor_id, user_id, kind, option) in [
        (
            "CODE_USER_1",
            "CODE_USER",
            "UafSingleUse",
            moqui_core::user::single_use_hash("246810").unwrap(),
        ),
        (
            "OTHER_FACTOR",
            "EX_JOHN_DOE",
            "UafEmail",
            "john.doe@example.com".to_string(),
        ),
    ] {
        let mut factor = facade.make_value("moqui.security.UserAuthcFactor").unwrap();
        factor.set("factorId", Value::string(factor_id));
        factor.set("userId", Value::string(user_id));
        factor.set("factorTypeEnumId", Value::string(kind));
        factor.set("factorOption", Value::string(option));
        factor.set("fromDate", Value::I64(0));
        factor.create(&facade).unwrap();
    }
}

fn send_note(
    factory: &std::sync::Arc<moqui_core::context::ExecutionContextFactory>,
    user_id: &str,
    topic: &str,
    title: &str,
) {
    let ctx = factory.new_context();
    let entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let mut note = moqui_core::notify::NotificationMessage::new();
    note.topic(topic)
        .user_id(user_id)
        .title(title)
        .persist(false)
        .message_json("{\"n\":1}")
        .send()
        .unwrap();
    drop(entered);
}
