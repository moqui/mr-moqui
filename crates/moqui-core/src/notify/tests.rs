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

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use indexmap::IndexMap;
use moqui_lib::object::{basic_convert, Value};
use moqui_lib::sync::lock;

use super::{
    install, mark_sent, mark_viewed, origin_id, publish, subscribe, NotificationMessage, TopicMessage,
    ENTITY_CACHE_INVALIDATE, NOTIFICATION_MESSAGE,
};
use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::EntityFacade;

const TOPIC: &str = "moqui.security.user.NotificationTopic";
const TOPIC_USER: &str = "moqui.security.user.NotificationTopicUser";
const MESSAGE: &str = "moqui.security.user.NotificationMessage";
const MESSAGE_USER: &str = "moqui.security.user.NotificationMessageUser";
const USER_ACCOUNT: &str = "moqui.security.UserAccount";
const GROUP_MEMBER: &str = "moqui.security.UserGroupMember";

fn unique(prefix: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!(
        "{prefix}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn stamp(millis: i64) -> Value {
    basic_convert(&Value::I64(millis), "java.sql.Timestamp").value
}

fn open() -> (ExecutionContext, EntityFacade) {
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    (ctx, facade)
}

fn open_taped() -> (ExecutionContext, EntityFacade, Arc<Mutex<Vec<String>>>) {
    let ctx = ExecutionContextFactory::test_context();
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = crate::testing::open_with_tape(Arc::clone(&tape));
    ctx.factory().set_entity(facade.clone());
    (ctx, facade, tape)
}

fn account(facade: &EntityFacade, user_id: &str, email: Option<&str>, terminate: Option<Value>) {
    let mut row = facade.make_value(USER_ACCOUNT).unwrap();
    row.set("userId", Value::string(user_id));
    row.set("username", Value::string(user_id));
    if let Some(email) = email {
        row.set("emailAddress", Value::string(email));
    }
    if let Some(terminate) = terminate {
        row.set("terminateDate", terminate);
    }
    row.create(facade).unwrap();
}

fn member(facade: &EntityFacade, group: &str, user_id: &str, from: Value, thru: Option<Value>) {
    let mut row = facade.make_value(GROUP_MEMBER).unwrap();
    row.set("userGroupId", Value::string(group));
    row.set("userId", Value::string(user_id));
    row.set("fromDate", from);
    if let Some(thru) = thru {
        row.set("thruDate", thru);
    }
    row.create(facade).unwrap();
}

fn topic_row(facade: &EntityFacade, name: &str, fields: &[(&str, &str)]) {
    let mut row = facade.make_value(TOPIC).unwrap();
    row.set("topic", Value::string(name));
    for (field, value) in fields {
        row.set(*field, Value::string(*value));
    }
    row.create(facade).unwrap();
}

fn topic_user(facade: &EntityFacade, topic: &str, user_id: &str, fields: &[(&str, &str)]) {
    let mut row = facade.make_value(TOPIC_USER).unwrap();
    row.set("topic", Value::string(topic));
    row.set("userId", Value::string(user_id));
    for (field, value) in fields {
        row.set(*field, Value::string(*value));
    }
    row.create(facade).unwrap();
}

fn text(row: &crate::entity::EntityValue, field: &str) -> String {
    row.get(field).and_then(Value::as_str).unwrap_or("").to_string()
}

fn select_count(sqls: &[String], table: &str) -> usize {
    let quoted = format!("\"{table}\"");
    sqls.iter()
        .filter(|sql| sql.to_ascii_uppercase().starts_with("SELECT") && sql.contains(&quoted))
        .count()
}

fn kept(body_title: &str) -> (super::Subscription, Arc<Mutex<Vec<TopicMessage>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let keep = Arc::clone(&seen);
    let title = body_title.to_string();
    let sub = subscribe(
        NOTIFICATION_MESSAGE,
        Arc::new(move |message| {
            if message.body.get("title").and_then(|value| value.as_str()) == Some(title.as_str()) {
                lock(&keep).push(message.clone());
            }
        }),
    )
    .unwrap();
    (sub, seen)
}

#[test]
fn recipients_dedup_group_explicit_terminated_and_all() {
    let (ctx, facade, tape) = open_taped();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let topic = "Desk";
    topic_row(&facade, topic, &[("description", "Desk notes")]);
    for user in ["alice", "bob", "carol", "eve", "quiet"] {
        account(&facade, user, None, None);
    }
    account(&facade, "dave", None, Some(stamp(1_000)));
    let past = stamp(1_000);
    member(&facade, "OPS", "bob", past.clone(), None);
    member(&facade, "OPS", "carol", past.clone(), None);
    member(&facade, "OPS", "old", past, Some(stamp(2_000)));
    let future = jiff::Timestamp::now()
        .as_millisecond()
        .saturating_add(30 * 24 * 60 * 60 * 1_000);
    member(&facade, "OPS", "future", stamp(future), None);
    topic_user(&facade, topic, "quiet", &[("receiveNotifications", "N")]);
    topic_user(
        &facade,
        topic,
        "eve",
        &[("allNotifications", "Y"), ("receiveNotifications", "N")],
    );
    let before = lock(&tape).len();
    let mut note = NotificationMessage::new();
    note.topic(topic)
        .user_group_id("OPS")
        .user_id("alice")
        .user_id("carol")
        .user_id("dave")
        .user_id("quiet")
        .user_id("ghost")
        .persist(false)
        .send()
        .unwrap();
    assert_eq!(
        note.notify_user_ids(),
        ["alice", "bob", "carol", "eve", "ghost"].as_slice()
    );
    let sqls = lock(&tape)[before..].to_vec();
    assert_eq!(select_count(&sqls, "notification_topic"), 1, "{sqls:?}");
    assert_eq!(select_count(&sqls, "notification_topic_user"), 1, "{sqls:?}");
    assert_eq!(select_count(&sqls, "user_group_member"), 1, "{sqls:?}");
    assert_eq!(select_count(&sqls, "user_account"), 1, "{sqls:?}");
}

#[test]
fn persist_survives_caller_rollback() {
    let (ctx, facade) = open();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    account(&facade, "alice", None, None);
    facade.begin(MESSAGE, None).unwrap();
    let mut body = IndexMap::new();
    body.insert("note".to_string(), Value::string("kept-body"));
    let mut note = NotificationMessage::new();
    note.topic("PersistTopic")
        .user_id("alice")
        .persist(true)
        .title("kept")
        .message_map(body)
        .send()
        .unwrap();
    let id = note.notification_message_id().unwrap().to_string();
    facade.rollback(MESSAGE).unwrap();
    let row = facade
        .find(MESSAGE)
        .unwrap()
        .eq("notificationMessageId", Value::string(&id))
        .one()
        .unwrap()
        .expect("persisted row");
    assert_eq!(text(&row, "titleText"), "kept");
    assert!(text(&row, "messageJson").contains("kept-body"));
    assert!(row.get("sentDate").is_some_and(|value| !value.is_null()));
    let users = facade
        .find(MESSAGE_USER)
        .unwrap()
        .eq("notificationMessageId", Value::string(&id))
        .list()
        .unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(text(&users.values()[0], "userId"), "alice");
    mark_sent("", "alice").unwrap();
    mark_sent(&id, "alice").unwrap();
    let sent = facade
        .find(MESSAGE_USER)
        .unwrap()
        .eq("notificationMessageId", Value::string(&id))
        .eq("userId", Value::string("alice"))
        .one()
        .unwrap()
        .unwrap();
    assert!(sent.get("sentDate").is_some_and(|value| !value.is_null()));
    mark_viewed(&id, "alice").unwrap();
    let viewed = facade
        .find(MESSAGE_USER)
        .unwrap()
        .eq("notificationMessageId", Value::string(&id))
        .eq("userId", Value::string("alice"))
        .one()
        .unwrap()
        .unwrap();
    assert!(viewed.get("viewedDate").is_some_and(|value| !value.is_null()));
    assert!(mark_viewed("", "alice").is_err());
    assert!(mark_viewed(&id, "").is_err());
    assert!(mark_sent(&id, "").is_err());
}

/// The stored message keeps its own showAlert, N unless show_alert was called; the topic's default is for the message
/// sent now. (What the sockets get is not checked here: a persisted message is published under its sequenced id, which
/// the in-memory databases of parallel tests repeat, so another test may have taken it.)
#[test]
fn a_stored_message_keeps_its_own_show_alert() {
    let (ctx, facade) = open();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    account(&facade, "alice", None, None);
    topic_row(&facade, "AlertTopic", &[("showAlert", "Y"), ("persistOnSend", "Y")]);
    let stored = |show: Option<bool>| {
        let mut note = NotificationMessage::new();
        note.topic("AlertTopic").user_id("alice").title("alerting");
        if let Some(show) = show {
            note.show_alert(show);
        }
        note.send().unwrap();
        let id = note.notification_message_id().unwrap().to_string();
        let row = facade
            .find(MESSAGE)
            .unwrap()
            .eq("notificationMessageId", Value::string(&id))
            .one()
            .unwrap()
            .expect("persisted row");
        text(&row, "showAlert")
    };
    assert_eq!(stored(None), "N");
    assert_eq!(stored(Some(true)), "Y");
    assert_eq!(stored(Some(false)), "N");
}

#[test]
fn subscriber_receives_one_message() {
    let (ctx, facade) = open();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let label = unique("Hello");
    let title = format!("Hello {label}");
    topic_row(
        &facade,
        "HelloTopic",
        &[
            ("titleTemplate", "ignored ${label}"),
            ("linkTemplate", "go/${label}"),
            ("errorTitleTemplate", "bad ${label}"),
        ],
    );
    let (sub, seen) = kept(&title);
    let mut message = IndexMap::new();
    message.insert("label".to_string(), Value::string(&label));
    let mut note = NotificationMessage::new();
    note.topic("HelloTopic")
        .title("Hello ${label}")
        .message_map(message)
        .persist(false)
        .send()
        .unwrap();
    assert_eq!(note.title_text(), Some(title.as_str()));
    assert_eq!(note.link_text(), Some(format!("go/{label}").as_str()));
    crate::script::set_context_name("label", Value::string("changed")).unwrap();
    assert_eq!(note.title_text(), Some(title.as_str()));
    assert_eq!(note.link_text(), Some(format!("go/{label}").as_str()));
    let got = lock(&seen);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].origin_id, origin_id());
    assert!(!got[0].message_id.is_empty());
    assert!(got[0]
        .body
        .get("notificationMessageId")
        .is_some_and(|value| value.is_null()));
    drop(got);
    drop(sub);

    let bad = unique("bad");
    topic_row(
        &facade,
        "DangerTopic",
        &[
            ("titleTemplate", "ok ${label}"),
            ("errorTitleTemplate", "bad ${label}"),
            ("typeString", "danger"),
        ],
    );
    let mut message = IndexMap::new();
    message.insert("label".to_string(), Value::string(&bad));
    let mut note = NotificationMessage::new();
    note.topic("DangerTopic")
        .message_map(message)
        .persist(false)
        .send()
        .unwrap();
    assert_eq!(note.title_text(), Some(format!("bad {bad}").as_str()));
}

#[test]
fn same_message_id_delivers_once() {
    install();
    install();
    let id = unique("msg");
    let seen = Arc::new(Mutex::new(0_u32));
    let count = Arc::clone(&seen);
    let watch = id.clone();
    let _sub = subscribe(
        NOTIFICATION_MESSAGE,
        Arc::new(move |message| {
            if message.message_id == watch {
                *lock(&count) += 1;
            }
        }),
    )
    .unwrap();
    let message = TopicMessage {
        origin_id: origin_id().to_string(),
        message_id: id,
        body: serde_json::Value::Null,
    };
    publish(NOTIFICATION_MESSAGE, &message).unwrap();
    publish(NOTIFICATION_MESSAGE, &message).unwrap();
    assert_eq!(*lock(&seen), 1);

    let cache_id = unique("cache");
    let cache_seen = Arc::new(Mutex::new(0_u32));
    let cache_count = Arc::clone(&cache_seen);
    let watch = cache_id.clone();
    let _cache = subscribe(
        ENTITY_CACHE_INVALIDATE,
        Arc::new(move |message| {
            if message.message_id == watch {
                *lock(&cache_count) += 1;
            }
        }),
    )
    .unwrap();
    let cache = TopicMessage {
        origin_id: origin_id().to_string(),
        message_id: cache_id,
        body: serde_json::Value::Null,
    };
    publish(ENTITY_CACHE_INVALIDATE, &cache).unwrap();
    publish(ENTITY_CACHE_INVALIDATE, &cache).unwrap();
    assert_eq!(*lock(&cache_seen), 2);
    let err = publish(
        "no-such",
        &TopicMessage {
            origin_id: origin_id().to_string(),
            message_id: unique("nope"),
            body: serde_json::Value::Null,
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("unknown topic"), "{err}");
}

#[test]
fn email_failure_does_not_fail_send() {
    let (ctx, facade) = open();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let title = unique("mail");
    topic_row(
        &facade,
        "MailTopic",
        &[("emailNotifications", "Y"), ("emailTemplateId", "NOTE")],
    );
    account(&facade, "alice", Some("alice@example.com"), None);
    ctx.message().add_error("keep-me");
    let (sub, seen) = kept(&title);
    let mut note = NotificationMessage::new();
    note.topic("MailTopic")
        .user_id("alice")
        .title(&title)
        .persist(false)
        .send()
        .unwrap();
    assert_eq!(ctx.message().errors(), ["keep-me".to_string()]);
    assert_eq!(lock(&seen).len(), 1);
    drop(sub);
}

#[test]
fn job_error_publishes_danger_with_job_name() {
    let (ctx, facade) = open();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let job = unique("job");
    let error = unique("broke");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let keep = Arc::clone(&seen);
    let watch = job.clone();
    let _sub = subscribe(
        NOTIFICATION_MESSAGE,
        Arc::new(move |message| {
            let name = message
                .body
                .get("message")
                .and_then(|value| value.get("jobName"))
                .and_then(|value| value.as_str());
            if name == Some(watch.as_str()) {
                lock(&keep).push(message.clone());
            }
        }),
    )
    .unwrap();
    crate::service::send_job_error_notification(&job, "run-1", &error);
    let got = lock(&seen);
    assert_eq!(got.len(), 1);
    assert_eq!(
        got[0].body.get("topic").and_then(|value| value.as_str()),
        Some("ServiceJobError")
    );
    assert_eq!(got[0].body.get("type").and_then(|value| value.as_str()), Some("danger"));
    assert_eq!(
        got[0]
            .body
            .get("message")
            .and_then(|value| value.get("errors"))
            .and_then(|value| value.as_str()),
        Some(error.as_str())
    );
    assert_eq!(
        got[0]
            .body
            .get("message")
            .and_then(|value| value.get("message"))
            .and_then(|value| value.as_str()),
        Some(error.as_str())
    );
    let title = got[0].body.get("title").and_then(|value| value.as_str()).unwrap_or("");
    assert!(title.contains(&job), "{title}");
    drop(got);
    let rows = facade
        .find(MESSAGE)
        .unwrap()
        .eq("topic", Value::string("ServiceJobError"))
        .list()
        .unwrap();
    assert!(rows.is_empty());
}

#[test]
fn job_error_notification_failure_does_not_panic() {
    crate::service::send_job_error_notification("orphan-job", "run-9", "no facade");
}

/// `emailNotifications` Y: the notification goes out by email with the `NotificationEmail.xml` body in html
/// and text, and the saved `EmailMessage` id is stored on the recipient's `NotificationMessageUser` row.
#[test]
#[cfg(feature = "smtp")]
fn email_notification_sends_the_screen_body_and_stores_the_email_message_id() {
    let _lock = crate::user::session::cookie_property_lock();
    let ctx = ExecutionContextFactory::test_context_in(
        "",
        crate::service::ServicePools::idle(),
        crate::context::framework_dir(),
    );
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    crate::service::load_definitions().unwrap();
    let _authz = ctx.disable_authz();
    let mut server = facade.make_value("moqui.basic.email.EmailServer").unwrap();
    server.set("emailServerId", Value::string("NOTE_SMTP"));
    server.set("smtpHost", Value::string("smtp.example.com"));
    server.create(&facade).unwrap();
    let mut template = facade.make_value("moqui.basic.email.EmailTemplate").unwrap();
    for (field, value) in [
        ("emailTemplateId", "NOTE_MAIL"),
        ("emailServerId", "NOTE_SMTP"),
        (
            "bodyScreenLocation",
            "component://framework/screen/NotificationEmail.xml",
        ),
        ("webappName", "webroot"),
        ("fromAddress", "notify@example.com"),
        ("subject", "${title}"),
    ] {
        template.set(field, Value::string(value));
    }
    template.create(&facade).unwrap();
    let topic = unique("MailOut");
    topic_row(
        &facade,
        &topic,
        &[
            ("description", "Mail Out"),
            ("emailNotifications", "Y"),
            ("emailTemplateId", "NOTE_MAIL"),
            ("emailMessageSave", "Y"),
        ],
    );
    let user = unique("bob");
    account(&facade, &user, Some("bob@example.com"), None);
    let title = unique("Disk full");
    let sink = crate::service::SinkGuard::install();
    ctx.message().add_error("caller-error");
    let mut note = NotificationMessage::new();
    note.topic(&topic)
        .user_id(&user)
        .title(&title)
        .link("/apps/System")
        .persist(true)
        .send()
        .unwrap();
    assert_eq!(
        ctx.message().errors(),
        ["caller-error"],
        "the caller's errors survive independent mail delivery"
    );
    let sent = sink.sent();
    assert_eq!(sent.len(), 1);
    let mail = &sent[0];
    assert_eq!(mail.subject, title);
    assert_eq!(mail.to, ["bob@example.com"]);
    assert!(mail.body.contains(&format!("<h4>{title}</h4>")), "{}", mail.body);
    assert!(mail.body.contains("Mail Out Notification (info)"), "{}", mail.body);
    assert!(mail.body_text.contains(&title), "{}", mail.body_text);
    assert!(mail.body_text.contains("/apps/System"), "{}", mail.body_text);
    assert!(mail.mime.contains("multipart/alternative"), "{}", mail.mime);

    let email_message_id = note
        .email_message_id_by_user_id()
        .get(&user)
        .cloned()
        .expect("email message id");
    let notification_message_id = note.notification_message_id().unwrap().to_string();
    let row = facade
        .find(MESSAGE_USER)
        .unwrap()
        .eq("notificationMessageId", Value::string(&notification_message_id))
        .eq("userId", Value::string(&user))
        .one()
        .unwrap()
        .expect("NotificationMessageUser");
    assert_eq!(text(&row, "emailMessageId"), email_message_id);
    assert!(row.get("sentDate").is_some_and(|value| !value.is_null()));

    // A draft id survives a skipped SMTP send, but is not evidence of delivery.
    server.set("smtpHost", Value::string(""));
    server.update(&facade).unwrap();
    let mut skipped = NotificationMessage::new();
    skipped
        .topic(&topic)
        .user_id(&user)
        .title("SMTP disabled")
        .persist(true)
        .send()
        .unwrap();
    assert!(!skipped.email_message_id_by_user_id().get(&user).unwrap().is_empty());
    let row = facade
        .find(MESSAGE_USER)
        .unwrap()
        .eq(
            "notificationMessageId",
            Value::string(skipped.notification_message_id().unwrap()),
        )
        .eq("userId", Value::string(&user))
        .one()
        .unwrap()
        .unwrap();
    assert!(!text(&row, "emailMessageId").is_empty());
    assert!(row.get("sentDate").is_none_or(Value::is_null));
    assert_eq!(sink.sent().len(), 1);
}

/// The app fields: a reason by its short name or id, the record, the actor and the app are stored on the
/// `NotificationMessage` row and sent in the wrapped message, beside the topic's inbox; the actor is the user who sent
/// it unless set.
#[test]
fn reason_record_actor_and_app_are_stored_and_sent() {
    let (ctx, facade) = open();
    let _entered = ctx.enter();
    let sender = unique("sender");
    account(&facade, &sender, None, None);
    let recipient = unique("recipient");
    account(&facade, &recipient, None, None);
    {
        let _authz = ctx.disable_authz();
        crate::user::internal_login_user(&sender).unwrap();
    }
    let topic = unique("Fields");
    topic_row(
        &facade,
        &topic,
        &[("description", "Fields"), ("inboxEnumId", "NinUpdates")],
    );
    let title = unique("Assigned");
    let send = |persist: bool| {
        let mut note = NotificationMessage::new();
        note.topic(&topic)
            .user_id(&recipient)
            .title(&title)
            .reason("assignment")
            .record("moqui.test.Task", "TASK1")
            .app_name("Hub")
            .persist(persist)
            .send()
            .unwrap();
        note
    };
    let note = send(true);
    assert_eq!(note.reason_enum_id(), Some("NrAssignment"));
    let id = note.notification_message_id().unwrap().to_string();
    let _authz = ctx.disable_authz();
    let row = facade
        .find(MESSAGE)
        .unwrap()
        .eq("notificationMessageId", Value::string(&id))
        .one()
        .unwrap()
        .unwrap();
    for (field, expected) in [
        ("reasonEnumId", "NrAssignment"),
        ("recordEntityName", "moqui.test.Task"),
        ("recordId", "TASK1"),
        ("actorUserId", sender.as_str()),
        ("appName", "Hub"),
    ] {
        assert_eq!(text(&row, field), expected, "{field}");
    }
    // (a persisted message is published under its sequenced id, which the in-memory databases of the tests repeat)
    let (sub, seen) = kept(&title);
    send(false);
    let got = lock(&seen);
    assert_eq!(got.len(), 1);
    let body = &got[0].body;
    assert_eq!(
        body.get("reasonEnumId").and_then(|value| value.as_str()),
        Some("NrAssignment")
    );
    assert_eq!(body.get("recordId").and_then(|value| value.as_str()), Some("TASK1"));
    assert_eq!(
        body.get("actorUserId").and_then(|value| value.as_str()),
        Some(sender.as_str())
    );
    assert_eq!(body.get("appName").and_then(|value| value.as_str()), Some("Hub"));
    assert_eq!(
        body.get("inboxEnumId").and_then(|value| value.as_str()),
        Some("NinUpdates")
    );
    // the fields Java sends are still there
    assert_eq!(body.get("title").and_then(|value| value.as_str()), Some(title.as_str()));
    assert_eq!(body.get("topic").and_then(|value| value.as_str()), Some(topic.as_str()));
    drop(got);
    drop(sub);

    // an actor set wins over the sender, an enumId is kept as it is, and a topic without a record is in Notifications
    let title = unique("Mentioned");
    let (sub, seen) = kept(&title);
    let mut note = NotificationMessage::new();
    note.topic(unique("NoRow"))
        .user_id(&recipient)
        .title(&title)
        .reason("NrMention")
        .actor_user_id("someone")
        .persist(false)
        .send()
        .unwrap();
    let got = lock(&seen);
    let body = &got[0].body;
    assert_eq!(
        body.get("reasonEnumId").and_then(|value| value.as_str()),
        Some("NrMention")
    );
    assert_eq!(
        body.get("actorUserId").and_then(|value| value.as_str()),
        Some("someone")
    );
    assert_eq!(
        body.get("inboxEnumId").and_then(|value| value.as_str()),
        Some("NinNotifications")
    );
    assert!(body.get("recordEntityName").is_some_and(serde_json::Value::is_null));
    drop(got);
    drop(sub);
}
