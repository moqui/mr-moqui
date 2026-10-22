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
        note.record("mantle.order.OrderHeader", "PO1")
            .message_map(message)
            .reason("approval")
            .app_name("Purchasing");
    });
    let approve_big = notify("Approve", "alice", "Approve PO2", |note| {
        let mut message = IndexMap::new();
        message.insert("amount".to_string(), Value::I64(400));
        note.record("mantle.order.OrderHeader", "PO2")
            .message_map(message)
            .reason("approval");
    });
    let refuse_one = notify("Approve", "alice", "Approve PO3", |note| {
        note.record("mantle.order.OrderHeader", "PO3").reason("approval");
    });
    let mention = notify("Mentions", "alice", "Bob mentioned you", |note| {
        note.reason("mention").app_name("Commons");
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
    assert_eq!(map_count(&all, "appCounts", "Commons"), 1);
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
    let by_app = call("get#UserNotifications", &[("appName", Value::string("Commons"))]);
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
        Some("approved mantle.order.OrderHeader 40 alice"),
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
            .record("mantle.order.OrderHeader", "PO4")
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

/// The daily digest: one email per user of the unviewed notifications on the topics they get as a digest (the topic's
/// default or their own choice), not those a topic emailed at once or an earlier digest carried; each row is marked.
#[test]
#[cfg(feature = "smtp")]
fn the_daily_digest_sends_each_user_one_email_of_their_digest_topics() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade, dir) = setup("digest");
    let _entered = ctx.enter();
    install(&dir);
    {
        let _authz = ctx.disable_authz();
        row(
            &facade,
            "moqui.basic.email.EmailServer",
            &[("emailServerId", "DIGEST_SMTP"), ("smtpHost", "smtp.example.com")],
        );
        row(
            &facade,
            "moqui.basic.email.EmailTemplate",
            &[
                ("emailTemplateId", "TEST_DIGEST"),
                ("emailServerId", "DIGEST_SMTP"),
                (
                    "bodyScreenLocation",
                    "component://framework/screen/NotificationDigestEmail.xml",
                ),
                ("webappName", "webroot"),
                ("fromAddress", "notify@example.com"),
                ("subject", "Your notifications (${notificationCount})"),
            ],
        );
        account(&facade, "carol", "carol@example.com");
        account(&facade, "dave", "dave@example.com");
        account(&facade, "erin", "");
        let topic = |name: &str, fields: &[(&str, &str)]| {
            let mut all = vec![("topic", name), ("description", name)];
            all.extend_from_slice(fields);
            row(&facade, "moqui.security.user.NotificationTopic", &all);
        };
        topic("Daily", &[("digestDefault", "Y"), ("description", "Daily feed")]);
        topic("Chosen", &[("description", "Chosen feed")]);
        topic(
            "Mentions",
            &[("emailNotifications", "Y"), ("emailTemplateId", "TEST_DIGEST")],
        );
        let choice = |topic: &str, user: &str, fields: &[(&str, &str)]| {
            let mut all = vec![("topic", topic), ("userId", user)];
            all.extend_from_slice(fields);
            row(&facade, "moqui.security.user.NotificationTopicUser", &all);
        };
        choice("Chosen", "carol", &[("digest", "Y")]);
        // Dave turned the default off, and wants mentions as a digest though they are emailed at once
        choice("Daily", "dave", &[("digest", "N")]);
        choice("Mentions", "dave", &[("digest", "Y"), ("emailNotifications", "Y")]);
    }
    let daily = notify("Daily", "carol", "Daily one", |note| {
        note.link("/qapps/Commons/Tasks").app_name("Warehouse");
    });
    let daily_later = notify("Daily", "carol", "Daily later", |note| {
        note.app_name("Warehouse");
    });
    let chosen = notify("Chosen", "carol", "Chosen one", |note| {
        note.app_name("Purchasing");
    });
    let viewed_one = notify("Daily", "carol", "Seen already", |_| {});
    {
        let _authz = ctx.disable_authz();
        crate::notify::mark_viewed(&viewed_one, "carol").unwrap();
    }
    notify("Chosen", "dave", "Not Dave's choice", |_| {});
    notify("Daily", "dave", "Dave said no", |_| {});
    notify("Daily", "erin", "No address", |_| {});
    let sink = crate::service::SinkGuard::install();
    notify("Mentions", "dave", "Mentioned at once", |_| {});
    assert_eq!(sink.sent().len(), 1, "the mention went out at once");

    let out = call(
        "send#NotificationDigests",
        &[
            ("emailTemplateId", Value::string("TEST_DIGEST")),
            ("ignoreHour", Value::Bool(true)),
        ],
    );
    assert_eq!(errors(), "");
    assert_eq!(count(&out, "userCount"), 1, "{out:?}");
    assert_eq!(count(&out, "notificationCount"), 3);
    let sent = sink.sent();
    assert_eq!(sent.len(), 2, "one digest");
    let digest = &sent[1];
    assert_eq!(digest.to, ["carol@example.com"]);
    assert_eq!(digest.subject, "Your notifications (3)");
    for text in ["Daily one", "Daily later", "Chosen one", "/qapps/Commons/Tasks"] {
        assert!(digest.body_text.contains(text), "{text} in {}", digest.body_text);
    }
    assert!(!digest.body_text.contains("Seen already"), "{}", digest.body_text);
    // grouped by app, then topic; first seen (oldest) first, so the later daily title follows the first
    let warehouse = digest.body.find("<h3>Warehouse</h3>").expect(&digest.body);
    let purchasing = digest.body.find("<h3>Purchasing</h3>").expect(&digest.body);
    assert!(warehouse < purchasing, "{}", digest.body);
    assert!(digest.body.contains("<h4>Daily feed</h4>"), "{}", digest.body);
    assert!(digest.body.contains("<h4>Chosen feed</h4>"), "{}", digest.body);
    let first = digest.body.find("Daily one").expect(&digest.body);
    let later = digest.body.find("Daily later").expect(&digest.body);
    assert!(first < later, "{}", digest.body);
    assert!(digest.body_text.contains("Warehouse\n"), "{}", digest.body_text);
    assert!(digest.body_text.contains("  Daily feed"), "{}", digest.body_text);
    assert!(digest.body_text.contains("  Chosen feed"), "{}", digest.body_text);
    // the dates are in the user's time zone
    let chicago = jiff::Timestamp::now()
        .to_zoned(jiff::tz::TimeZone::get("America/Chicago").unwrap())
        .strftime("%Y-%m-%d")
        .to_string();
    assert!(digest.body_text.contains(&chicago), "{chicago} in {}", digest.body_text);
    {
        let _authz = ctx.disable_authz();
        for id in [&daily, &daily_later, &chosen] {
            let row = facade
                .find("moqui.security.user.NotificationMessageUser")
                .unwrap()
                .eq("notificationMessageId", Value::string(id))
                .eq("userId", Value::string("carol"))
                .one()
                .unwrap()
                .unwrap();
            assert!(row.get("digestDate").is_some_and(|value| !value.is_null()));
        }
    }
    // a notification goes in one digest
    let again = call(
        "send#NotificationDigests",
        &[
            ("emailTemplateId", Value::string("TEST_DIGEST")),
            ("ignoreHour", Value::Bool(true)),
        ],
    );
    assert_eq!(count(&again, "userCount"), 0);
    assert_eq!(sink.sent().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Each user is sent only in their hour (NOTIFICATION_DIGEST_HOUR) in their time zone. Carol's hour is now in
/// America/Chicago; Dave's is three hours off, so only Carol is sent and Dave's row stays unmarked.
#[test]
#[cfg(feature = "smtp")]
fn the_digest_sends_a_user_only_in_their_hour() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade, dir) = setup("digest-hour");
    let _entered = ctx.enter();
    install(&dir);
    let chicago = jiff::tz::TimeZone::get("America/Chicago").unwrap();
    let hour = i32::from(jiff::Timestamp::now().to_zoned(chicago).hour());
    let carol_hour = hour.to_string();
    let dave_hour = (hour + 3).rem_euclid(24).to_string();
    {
        let _authz = ctx.disable_authz();
        row(
            &facade,
            "moqui.basic.email.EmailServer",
            &[("emailServerId", "DIGEST_SMTP"), ("smtpHost", "smtp.example.com")],
        );
        row(
            &facade,
            "moqui.basic.email.EmailTemplate",
            &[
                ("emailTemplateId", "TEST_DIGEST"),
                ("emailServerId", "DIGEST_SMTP"),
                (
                    "bodyScreenLocation",
                    "component://framework/screen/NotificationDigestEmail.xml",
                ),
                ("webappName", "webroot"),
                ("fromAddress", "notify@example.com"),
                ("subject", "Your notifications (${notificationCount})"),
            ],
        );
        account(&facade, "carol", "carol@example.com");
        account(&facade, "dave", "dave@example.com");
        row(
            &facade,
            "moqui.security.user.NotificationTopic",
            &[
                ("topic", "Daily"),
                ("description", "Daily feed"),
                ("digestDefault", "Y"),
            ],
        );
        let pref = |user: &str, value: &str| {
            row(
                &facade,
                "moqui.security.UserPreference",
                &[
                    ("userId", user),
                    ("preferenceKey", "NOTIFICATION_DIGEST_HOUR"),
                    ("preferenceValue", value),
                ],
            );
        };
        pref("carol", &carol_hour);
        pref("dave", &dave_hour);
    }
    let carol_id = notify("Daily", "carol", "Carol now", |note| {
        note.app_name("Warehouse");
    });
    let dave_id = notify("Daily", "dave", "Dave later", |_| {});
    let sink = crate::service::SinkGuard::install();
    let out = call(
        "send#NotificationDigests",
        &[("emailTemplateId", Value::string("TEST_DIGEST"))],
    );
    assert_eq!(errors(), "");
    assert_eq!(count(&out, "userCount"), 1, "{out:?}");
    let sent = sink.sent();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].to, ["carol@example.com"]);
    assert!(sent[0].body_text.contains("Carol now"), "{}", sent[0].body_text);
    assert!(!sent[0].body_text.contains("Dave later"), "{}", sent[0].body_text);
    {
        let _authz = ctx.disable_authz();
        let marked = |id: &str, user: &str| {
            facade
                .find("moqui.security.user.NotificationMessageUser")
                .unwrap()
                .eq("notificationMessageId", Value::string(id))
                .eq("userId", Value::string(user))
                .one()
                .unwrap()
                .unwrap()
                .get("digestDate")
                .is_some_and(|value| !value.is_null())
        };
        assert!(marked(&carol_id, "carol"));
        assert!(!marked(&dave_id, "dave"));
    }
    let _ = std::fs::remove_dir_all(&dir);
}
