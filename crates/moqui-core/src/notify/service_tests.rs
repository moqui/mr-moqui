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

//! `org.moqui.impl.NotificationServices`: a user's notifications by inbox and filter, the approvals a notification asks
//! for, marking them viewed, and the daily digest email.

use std::collections::BTreeMap;
use std::path::PathBuf;

use indexmap::IndexMap;
use moqui_lib::object::{to_plain_string, Value};

use super::NotificationMessage;
use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::EntityFacade;

const SERVICES: &str = "org.moqui.impl.NotificationServices";

/// The approve and refuse services a topic names: each keeps what it was given in an `Enumeration` row whose id is the
/// record id, so the test reads it back.
const DECIDERS: &str = r#"<services>
    <service verb="approve" noun="Thing">
        <in-parameters><parameter name="recordEntityName"/><parameter name="recordId"/><parameter name="message" type="Map"/>
            <parameter name="notificationMessageId"/></in-parameters>
        <out-parameters><parameter name="approved" type="Boolean"/></out-parameters>
        <actions><script><![CDATA[
            if (message.amount > 100) { ec.message.addError('Too much to approve'); return }
            const row = ec.entity.makeValue('moqui.basic.Enumeration')
            row.enumId = recordId
            row.description = 'approved ' + recordEntityName + ' ' + message.amount + ' ' + ec.user.userId
            row.create()
            approved = true
        ]]></script></actions>
    </service>
    <service verb="refuse" noun="Thing">
        <in-parameters><parameter name="recordEntityName"/><parameter name="recordId"/><parameter name="message" type="Map"/>
            <parameter name="notificationMessageId"/><parameter name="reason"/></in-parameters>
        <actions><script><![CDATA[
            const row = ec.entity.makeValue('moqui.basic.Enumeration')
            row.enumId = recordId
            row.description = 'refused ' + reason
            row.create()
        ]]></script></actions>
    </service>
</services>"#;

fn setup(name: &str) -> (ExecutionContext, EntityFacade, PathBuf) {
    let ctx = ExecutionContextFactory::test_context_in(
        "",
        crate::service::ServicePools::idle(),
        crate::context::framework_dir(),
    );
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    let dir = std::env::temp_dir().join(format!("moqui-notify-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("service/test")).unwrap();
    std::fs::write(dir.join("service/test/Decide.xml"), DECIDERS).unwrap();
    (ctx, facade, dir)
}

fn install(dir: &std::path::Path) {
    let framework = crate::context::framework_dir().join("service/org/moqui");
    let paths = vec![
        framework.join("EmailServices.xml"),
        framework.join("impl/EmailServices.xml"),
        framework.join("impl/NotificationServices.xml"),
        dir.join("service/test/Decide.xml"),
    ];
    let failures = crate::service::lint(&paths).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
}

fn row(facade: &EntityFacade, entity: &str, fields: &[(&str, &str)]) {
    let mut row = facade.make_value(entity).unwrap();
    for (field, value) in fields {
        row.set(*field, Value::string(*value));
    }
    row.create(facade).unwrap();
}

fn account(facade: &EntityFacade, user_id: &str, email: &str) {
    row(
        facade,
        "moqui.security.UserAccount",
        &[
            ("userId", user_id),
            ("username", user_id),
            ("emailAddress", email),
            ("timeZone", "America/Chicago"),
        ],
    );
}

fn login(user_id: &str) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    crate::user::internal_login_user(user_id).unwrap();
}

/// Send a persisted notification to `user_id` and return its id.
fn notify(topic: &str, user_id: &str, title: &str, fill: impl FnOnce(&mut NotificationMessage)) -> String {
    let mut note = NotificationMessage::new();
    note.topic(topic).user_id(user_id).title(title).persist(true);
    fill(&mut note);
    note.send().unwrap();
    note.notification_message_id().unwrap().to_string()
}

fn call(name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    crate::service::sync(&format!("{SERVICES}.{name}"), &parameters).unwrap_or_else(|err| panic!("{name}: {err}"))
}

fn errors() -> String {
    ExecutionContext::with_current(|ec| {
        let text = ec.message().errors().join("\n");
        ec.message().clear_errors();
        text
    })
    .unwrap_or_default()
}

fn titles(out: &BTreeMap<String, Value>) -> Vec<String> {
    match out.get("notificationList") {
        Some(Value::List(items)) => items
            .iter()
            .map(|item| match item {
                Value::Map(map) => map.get("titleText").map(to_plain_string).unwrap_or_default(),
                _ => String::new(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn count(out: &BTreeMap<String, Value>, key: &str) -> i64 {
    out.get(key)
        .map(to_plain_string)
        .and_then(|text| text.parse().ok())
        .unwrap_or(-1)
}

fn map_count(out: &BTreeMap<String, Value>, key: &str, entry: &str) -> i64 {
    match out.get(key) {
        Some(Value::Map(map)) => map
            .get(entry)
            .map(to_plain_string)
            .and_then(|text| text.parse().ok())
            .unwrap_or(0),
        _ => -1,
    }
}

fn viewed(facade: &EntityFacade, id: &str, user_id: &str) -> bool {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    facade
        .find("moqui.security.user.NotificationMessageUser")
        .unwrap()
        .eq("notificationMessageId", Value::string(id))
        .eq("userId", Value::string(user_id))
        .one()
        .unwrap()
        .and_then(|row| row.get("viewedDate").cloned())
        .is_some_and(|value| !value.is_null())
}

fn description(facade: &EntityFacade, enum_id: &str) -> Option<String> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    facade
        .find("moqui.basic.Enumeration")
        .unwrap()
        .eq("enumId", Value::string(enum_id))
        .one()
        .unwrap()
        .and_then(|row| row.get("description").map(to_plain_string))
}

/// The screens' view of a user's notifications: the inbox by its topic, the filters, the counts beside the list, and
/// never another user's rows; then the approvals the notifications ask for, and marking them viewed.
#[test]
fn a_user_lists_decides_and_marks_only_their_own_notifications() {
    let (ctx, facade, dir) = setup("inbox");
    let _entered = ctx.enter();
    install(&dir);
    {
        let _authz = ctx.disable_authz();
        account(&facade, "alice", "alice@example.com");
        account(&facade, "bob", "bob@example.com");
        row(
            &facade,
            "moqui.security.user.NotificationTopic",
            &[
                ("topic", "Approve"),
                ("description", "Approvals"),
                ("approveServiceName", "test.Decide.approve#Thing"),
                ("refuseServiceName", "test.Decide.refuse#Thing"),
                ("refuseReasonRequired", "Y"),
                // an approval topic is in Notifications whatever it says
                ("inboxEnumId", "NinUpdates"),
            ],
        );
        row(
            &facade,
            "moqui.security.user.NotificationTopic",
            &[
                ("topic", "Feed"),
                ("description", "Record updates"),
                ("inboxEnumId", "NinUpdates"),
            ],
        );
    }
    login("bob");
    let approve_one = notify("Approve", "alice", "Approve PO1", |note| {
        let mut message = IndexMap::new();
        message.insert("amount".to_string(), Value::I64(40));
        note.record("moqui.test.Order", "PO1")
            .message_map(message)
            .reason("approval")
            .app_name("Purchasing");
    });
    let approve_big = notify("Approve", "alice", "Approve PO2", |note| {
        let mut message = IndexMap::new();
        message.insert("amount".to_string(), Value::I64(400));
        note.record("moqui.test.Order", "PO2")
            .message_map(message)
            .reason("approval");
    });
    let refuse_one = notify("Approve", "alice", "Approve PO3", |note| {
        note.record("moqui.test.Order", "PO3").reason("approval");
    });
    let mention = notify("Mentions", "alice", "Bob mentioned you", |note| {
        note.reason("mention").app_name("Hub");
    });
    let update = notify("Feed", "alice", "Order changed", |note| {
        note.reason("system");
    });
    let bobs = notify("Mentions", "bob", "For Bob", |_| {});

    login("alice");
    let all = call("get#UserNotifications", &[]);
    assert_eq!(
        titles(&all),
        ["Bob mentioned you", "Approve PO3", "Approve PO2", "Approve PO1"],
        "the Notifications inbox, newest first, only Alice's"
    );
    assert_eq!(count(&all, "notificationListCount"), 4);
    assert_eq!(map_count(&all, "unreadCounts", "NinNotifications"), 4);
    assert_eq!(map_count(&all, "unreadCounts", "NinUpdates"), 1);
    assert_eq!(count(&all, "approvalCount"), 3);
    assert_eq!(map_count(&all, "reasonCounts", "NrApproval"), 3);
    assert_eq!(map_count(&all, "appCounts", "Hub"), 1);
    let Some(Value::List(items)) = all.get("notificationList") else {
        panic!("{all:?}")
    };
    let Value::Map(first_approval) = &items[3] else {
        panic!()
    };
    for (key, expected) in [
        ("recordId", "PO1"),
        ("actorUserId", "bob"),
        ("actorName", "bob"),
        ("appName", "Purchasing"),
        ("canApprove", "true"),
        ("canRefuse", "true"),
        ("refuseReasonRequired", "true"),
        ("inboxEnumId", "NinNotifications"),
        ("topicDescription", "Approvals"),
    ] {
        assert_eq!(
            first_approval.get(key).map(to_plain_string).as_deref(),
            Some(expected),
            "{key}"
        );
    }
    assert!(matches!(first_approval.get("message"), Some(Value::Map(message)) if message.get("amount").is_some()));

    let updates = call("get#UserNotifications", &[("inboxEnumId", Value::string("NinUpdates"))]);
    assert_eq!(titles(&updates), ["Order changed"]);
    let by_app = call("get#UserNotifications", &[("appName", Value::string("Hub"))]);
    assert_eq!(titles(&by_app), ["Bob mentioned you"]);
    let by_reason = call("get#UserNotifications", &[("reasonEnumId", Value::string("NrMention"))]);
    assert_eq!(titles(&by_reason), ["Bob mentioned you"]);
    let by_text = call("get#UserNotifications", &[("text", Value::string("po2"))]);
    assert_eq!(titles(&by_text), ["Approve PO2"]);
    let paged = call(
        "get#UserNotifications",
        &[("pageSize", Value::I64(3)), ("pageIndex", Value::I64(1))],
    );
    assert_eq!(titles(&paged), ["Approve PO1"]);
    assert_eq!(count(&paged, "notificationListPageMaxIndex"), 1);
    let waiting = call("get#UserNotifications", &[("approvals", Value::Bool(true))]);
    assert_eq!(titles(&waiting), ["Approve PO3", "Approve PO2", "Approve PO1"]);
    let read = call("get#UserNotifications", &[("viewed", Value::string("Y"))]);
    assert!(titles(&read).is_empty());

    // a decision only by a recipient, on a topic with that service, and a refuse with the reason the topic requires
    login("bob");
    call(
        "approve#NotificationMessage",
        &[("notificationMessageId", Value::string(&approve_one))],
    );
    assert!(errors().contains("not addressed to you"));
    call(
        "approve#NotificationMessage",
        &[("notificationMessageId", Value::string(&bobs))],
    );
    assert!(errors().contains("has no approve service"));
    login("alice");
    let out = call(
        "approve#NotificationMessage",
        &[("notificationMessageId", Value::string(&approve_one))],
    );
    assert_eq!(errors(), "");
    assert!(
        matches!(out.get("results"), Some(Value::Map(results)) if results.get("approved") == Some(&Value::Bool(true)))
    );
    assert_eq!(
        description(&facade, "PO1").as_deref(),
        Some("approved moqui.test.Order 40 alice"),
        "the service gets the record, the message map and runs as the user"
    );
    assert!(viewed(&facade, &approve_one, "alice"));
    // the service's error is the caller's, and nothing is marked
    call(
        "approve#NotificationMessage",
        &[("notificationMessageId", Value::string(&approve_big))],
    );
    assert!(errors().contains("Too much to approve"));
    assert!(!viewed(&facade, &approve_big, "alice"));
    call(
        "refuse#NotificationMessage",
        &[("notificationMessageId", Value::string(&refuse_one))],
    );
    assert!(errors().contains("A reason is required"));
    assert!(!viewed(&facade, &refuse_one, "alice"));
    call(
        "refuse#NotificationMessage",
        &[
            ("notificationMessageId", Value::string(&refuse_one)),
            ("reason", Value::string("over budget")),
        ],
    );
    assert_eq!(errors(), "");
    assert_eq!(description(&facade, "PO3").as_deref(), Some("refused over budget"));
    assert!(viewed(&facade, &refuse_one, "alice"));
    let after = call("get#UserNotifications", &[]);
    assert_eq!(count(&after, "approvalCount"), 1);

    // mark viewed: one inbox, then everything else of Alice's, never Bob's
    let marked = call(
        "mark#NotificationsViewed",
        &[("inboxEnumId", Value::string("NinUpdates"))],
    );
    assert_eq!(count(&marked, "viewedCount"), 1);
    assert!(viewed(&facade, &update, "alice"));
    assert!(!viewed(&facade, &mention, "alice"));
    let marked = call(
        "mark#NotificationsViewed",
        &[(
            "notificationMessageIdList",
            Value::List(vec![Value::string(&mention), Value::string(&bobs)]),
        )],
    );
    assert_eq!(count(&marked, "viewedCount"), 1, "Bob's id is not Alice's to mark");
    assert!(!viewed(&facade, &bobs, "bob"));
    // an approval is decided, not read: all of an inbox, or all, leaves the one still waiting; its id marks it
    let marked = call(
        "mark#NotificationsViewed",
        &[("inboxEnumId", Value::string("NinNotifications"))],
    );
    assert_eq!(count(&marked, "viewedCount"), 0, "the approval still waiting");
    let marked = call("mark#NotificationsViewed", &[]);
    assert_eq!(count(&marked, "viewedCount"), 0, "the approval still waiting");
    assert!(!viewed(&facade, &approve_big, "alice"));
    let none = call(
        "get#UserNotifications",
        &[("viewed", Value::string("N")), ("inboxEnumId", Value::string("all"))],
    );
    assert_eq!(titles(&none), ["Approve PO2"]);
    let marked = call(
        "mark#NotificationsViewed",
        &[("notificationMessageId", Value::string(&approve_big))],
    );
    assert_eq!(count(&marked, "viewedCount"), 1, "an explicit id is marked");

    // one approval to two recipients: the first decision is final for the message, so the other no longer sees it waiting
    {
        let _authz = ctx.disable_authz();
        account(&facade, "carol", "carol@example.com");
    }
    login("bob");
    let shared = {
        let mut note = NotificationMessage::new();
        note.topic("Approve")
            .user_ids(["alice", "carol"])
            .title("Approve PO4")
            .persist(true)
            .record("moqui.test.Order", "PO4")
            .reason("approval");
        note.send().unwrap();
        note.notification_message_id().unwrap().to_string()
    };
    login("carol");
    assert_eq!(count(&call("get#UserNotifications", &[]), "approvalCount"), 1);
    login("alice");
    call(
        "approve#NotificationMessage",
        &[("notificationMessageId", Value::string(&shared))],
    );
    assert_eq!(errors(), "");
    assert!(viewed(&facade, &shared, "alice") && viewed(&facade, &shared, "carol"));
    login("carol");
    assert_eq!(count(&call("get#UserNotifications", &[]), "approvalCount"), 0);
    let _ = std::fs::remove_dir_all(&dir);
}
