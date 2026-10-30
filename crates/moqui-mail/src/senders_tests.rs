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

//! The framework's mail senders, which moved here with the send path (plans/Crate-Split.md): an email ECA that sends a
//! template, a notification topic's email, the daily digest, the login code and the password reset mailed from the
//! service worker, and the read fence on a send.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, MutexGuard};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use moqui_lib::object::{to_plain_string, Value};

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::notify::NotificationMessage;
use moqui_core::service::ServicePools;
use moqui_core::web::{WebRequest, WebResponse};

use crate::SharedSinkGuard;

const NOTIFICATIONS: &str = "org.moqui.impl.NotificationServices";
const MESSAGE_USER: &str = "moqui.security.user.NotificationMessageUser";
const MAIL_WAIT: Duration = Duration::from_secs(10);

/// A factory on the framework component with the in-memory entity facade.
fn open(pools: ServicePools) -> (ExecutionContext, EntityFacade) {
    let ctx = ExecutionContextFactory::test_context_in("", pools, moqui_core::context::framework_dir());
    let facade = moqui_core::testing::open();
    ctx.factory().set_entity(facade.clone());
    (ctx, facade)
}

fn unique(prefix: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!(
        "{prefix}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn row(facade: &EntityFacade, entity: &str, fields: &[(&str, &str)]) {
    let mut row = facade.make_value(entity).unwrap();
    for (field, value) in fields {
        row.set(*field, Value::string(*value));
    }
    row.create(facade).unwrap();
}

fn seed(facade: &EntityFacade, context: &ExecutionContext, entity: &str, fields: &[(&str, &str)]) {
    let _authz = context.disable_authz();
    row(facade, entity, fields);
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

fn topic_row(facade: &EntityFacade, name: &str, fields: &[(&str, &str)]) {
    let mut all = vec![("topic", name)];
    all.extend_from_slice(fields);
    row(facade, "moqui.security.user.NotificationTopic", &all);
}

fn text(row: &EntityValue, field: &str) -> String {
    row.get(field).and_then(Value::as_str).unwrap_or("").to_string()
}

/// Send a persisted notification to `user_id` and return its id.
fn notify(topic: &str, user_id: &str, title: &str, fill: impl FnOnce(&mut NotificationMessage)) -> String {
    let mut note = NotificationMessage::new();
    note.topic(topic).user_id(user_id).title(title).persist(true);
    fill(&mut note);
    note.send().unwrap();
    note.notification_message_id().unwrap().to_string()
}

/// A `NotificationServices` service.
fn call(name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    moqui_core::service::sync(&format!("{NOTIFICATIONS}.{name}"), &parameters)
        .unwrap_or_else(|err| panic!("{name}: {err}"))
}

/// The errors of the current context, cleared for the next call.
fn errors() -> String {
    ExecutionContext::with_current(|ec| {
        let text = ec.message().errors().join("\n");
        ec.message().clear_errors();
        text
    })
    .unwrap_or_default()
}

fn count(out: &BTreeMap<String, Value>, key: &str) -> i64 {
    out.get(key)
        .map(to_plain_string)
        .and_then(|text| text.parse().ok())
        .unwrap_or(-1)
}

#[test]
fn email_eca_runs_its_action_and_sends_the_screen_mail() {
    let _lock = moqui_core::user::session::cookie_property_lock();
    let facade = moqui_core::testing::open();
    let context = ExecutionContextFactory::test_context_in(
        "",
        moqui_core::service::ServicePools::idle(),
        moqui_core::context::framework_dir(),
    );
    moqui_core::service::install_eca_rules(
        context.factory(),
        "",
        r#"<emecas>
                <emeca rule-name="SendNotice">
                    <condition><expression>true</expression></condition>
                    <actions>
                        <script>ec.message.addMessage("emailEcaRan");</script>
                        <service-call name="org.moqui.impl.EmailServices.send#EmailTemplate">
                            <field-map field-name="emailTemplateId" value="T1"/>
                            <field-map field-name="toAddresses" value="a@example.com"/>
                        </service-call>
                    </actions>
                </emeca>
            </emecas>"#,
    );
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    moqui_core::service::load_definitions().unwrap();
    // No template yet: the send adds an error and the rule's generated code returns, as in Java.
    moqui_core::service::run_email_rules(&BTreeMap::new()).unwrap();
    let errors = context.message().errors().join("\n");
    assert!(errors.contains("No EmailTemplate record found"), "{errors}");
    assert!(context.message().messages().iter().any(|text| text == "emailEcaRan"));
    context.message().clear();

    // With the template, its body screen renders in html and text and the mail goes out.
    let body = format!("{}/testdata/MailBody.xml", env!("CARGO_MANIFEST_DIR"));
    seed(
        &facade,
        &context,
        "moqui.basic.email.EmailServer",
        &[("emailServerId", "ECA_SMTP"), ("smtpHost", "smtp.example.com")],
    );
    seed(
        &facade,
        &context,
        "moqui.basic.email.EmailTemplate",
        &[
            ("emailTemplateId", "T1"),
            ("emailServerId", "ECA_SMTP"),
            ("fromAddress", "from@example.com"),
            ("subject", "Noticed"),
            ("bodyScreenLocation", &body),
        ],
    );
    let sink = crate::SinkGuard::install();
    moqui_core::service::run_email_rules(&BTreeMap::new()).unwrap();
    let errors = context.message().errors().join("\n");
    assert!(errors.is_empty(), "{errors}");
    let sent = sink.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].to, ["a@example.com"]);
    assert!(sent[0].body.contains("Hello nobody"), "{}", sent[0].body);
    assert!(sent[0].body_text.contains("Subject: Noticed"), "{}", sent[0].body_text);
}

/// `emailNotifications` Y: the notification goes out by email with the `NotificationEmail.xml` body in html
/// and text, and the saved `EmailMessage` id is stored on the recipient's `NotificationMessageUser` row.
#[test]
fn email_notification_sends_the_screen_body_and_stores_the_email_message_id() {
    let _lock = moqui_core::user::session::cookie_property_lock();
    let ctx = ExecutionContextFactory::test_context_in(
        "",
        moqui_core::service::ServicePools::idle(),
        moqui_core::context::framework_dir(),
    );
    let facade = moqui_core::testing::open();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    moqui_core::service::load_definitions().unwrap();
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
    account(&facade, &user, "bob@example.com");
    let title = unique("Disk full");
    let sink = crate::SinkGuard::install();
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

/// The daily digest: one email per user of the unviewed notifications on the topics they get as a digest (the topic's
/// default or their own choice), not those a topic emailed at once or an earlier digest carried; each row is marked.
#[test]
fn the_daily_digest_sends_each_user_one_email_of_their_digest_topics() {
    let _lock = moqui_core::user::session::cookie_property_lock();
    let (ctx, facade) = open(ServicePools::idle());
    let _entered = ctx.enter();
    moqui_core::service::load_definitions().unwrap();
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
        note.link("/qapps/Hub/Tasks").app_name("Warehouse");
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
        moqui_core::notify::mark_viewed(&viewed_one, "carol").unwrap();
    }
    notify("Chosen", "dave", "Not Dave's choice", |_| {});
    notify("Daily", "dave", "Dave said no", |_| {});
    notify("Daily", "erin", "No address", |_| {});
    let sink = crate::SinkGuard::install();
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
    for text in ["Daily one", "Daily later", "Chosen one", "/qapps/Hub/Tasks"] {
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
}

/// Each user is sent only in their hour (NOTIFICATION_DIGEST_HOUR) in their time zone. Carol's hour is now in
/// America/Chicago; Dave's is three hours off, so only Carol is sent and Dave's row stays unmarked.
#[test]
fn the_digest_sends_a_user_only_in_their_hour() {
    let _lock = moqui_core::user::session::cookie_property_lock();
    let (ctx, facade) = open(ServicePools::idle());
    let _entered = ctx.enter();
    moqui_core::service::load_definitions().unwrap();
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
    let sink = crate::SinkGuard::install();
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
}

/// Under the read fence (plans/LLM-A2A-Assist.md 4.6) a send is refused before it reaches a server.
#[test]
fn a_send_is_refused_under_the_read_fence() {
    let (ctx, _facade) = open(ServicePools::inline());
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    moqui_core::service::load_definitions().unwrap();
    let guard = moqui_core::entity::fence::enter("tool run_tool");
    let message = BTreeMap::from([("emailMessageId".to_string(), Value::string("NO_SUCH_MESSAGE"))]);
    let _ = moqui_core::service::sync("org.moqui.impl.EmailServices.send#EmailMessage", &message);
    let errors = errors();
    assert!(
        errors.contains(moqui_core::entity::fence::FENCE_ERROR) && errors.contains("mail (send#EmailMessage)"),
        "{errors}"
    );
    drop(guard);
}

// The second factor and the password reset through the webroot's `rest.xml` transitions, with mail from the service
// worker kept by the shared sink (moqui-core's screen/tests/authc.rs has the rest of these).

fn webroot() -> std::path::PathBuf {
    moqui_core::context::framework_dir().join("../webroot/screen/webroot.xml")
}

/// The session cookie key a request needs, for the life of a test, under the lock of the process properties.
struct Env {
    _lock: MutexGuard<'static, ()>,
}

impl Env {
    fn new() -> Self {
        let lock = moqui_core::user::session::cookie_property_lock();
        moqui_lib::env::set_property("session_cookie_key", STANDARD.encode([0x09_u8; 32]));
        Self { _lock: lock }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        moqui_lib::env::clear_property("session_cookie_key");
    }
}

fn request(method: &str, path: &str) -> WebRequest {
    let mut request = WebRequest::new(method, path);
    request.peer_ip = "127.0.0.1".to_string();
    request
}

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
        let context = ExecutionContextFactory::test_context_in(
            "",
            ServicePools::bounded(2, 16),
            moqui_core::context::framework_dir(),
        );
        let facade = moqui_core::testing::open();
        context.factory().set_entity(facade.clone());
        let _entered = context.enter();
        let _authz = context.disable_authz();
        moqui_core::service::load_definitions().unwrap();
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
        let stored = moqui_core::user::hash_password("Secret1!").unwrap();
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
            moqui_core::user::internal_login_user(username).unwrap();
        }
        let out = body();
        moqui_core::user::session::reset_session();
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
        let response = moqui_core::screen::serve_at(&self.factory, post, &webroot().to_string_lossy());
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
        if let Some(set_cookie) = response.header("Set-Cookie").map(str::to_string) {
            if let Some(value) = set_cookie
                .split(';')
                .next()
                .and_then(|pair| pair.strip_prefix("moqui.session="))
            {
                self.cookie = Some(value.to_string());
            }
        }
        if let Some(token) = response.header("X-CSRF-Token").map(str::to_string) {
            self.token = Some(token);
        }
    }
}

fn json(response: &WebResponse) -> serde_json::Value {
    serde_json::from_str(&response.body_text()).unwrap_or_else(|err| panic!("{err}: {}", response.body_text()))
}

#[test]
fn rest_login_with_an_email_code_sends_it_and_verify_otp_takes_it_once() {
    let flow = Flow::open();
    flow.user("bob", "bob-otp@example.com");
    flow.row(
        "moqui.security.UserGroup",
        &[("userGroupId", "STRICT"), ("requireAuthcFactor", "Y")],
    );
    let mut member = flow.facade.make_value("moqui.security.UserGroupMember").unwrap();
    member.set("userGroupId", Value::string("STRICT"));
    member.set("userId", Value::string("bob"));
    member.set("fromDate", moqui_core::user::stamp_value(1));
    member.create(&flow.facade).unwrap();

    let mut session = Session::default();
    let response = flow.post("/rest/login", "username=bob&password=Secret1!", &mut session);
    assert_eq!(response.status, 200, "{}", response.body_text());
    let info = json(&response);
    assert_eq!(info["secondFactorRequired"], true, "{info}");
    // No factor of his own, so the code goes to the account email.
    let sendable = &info["sendableFactors"][0];
    assert_eq!(sendable["factorId"], "UserAccountEmail", "{info}");
    assert_eq!(sendable["factorOption"], "bob-otp@example.com", "{info}");

    let sink = SharedSinkGuard::install();
    let response = flow.post("/rest/sendOtp", "factorId=UserAccountEmail", &mut session);
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert!(
        json(&response)["singleUseFactorId"].is_string(),
        "{}",
        response.body_text()
    );
    assert!(!response.body_text().contains("\"code\""), "{}", response.body_text());
    let sent = sink.wait_for("bob-otp@example.com", MAIL_WAIT);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].subject, "Single Use Code");
    let code = after(&sent[0].body, "Your single use code is: ");
    assert_eq!(code.len(), 6, "{}", sent[0].body);
    drop(sink);

    let response = flow.post("/rest/verifyOtp", &format!("code={code}"), &mut session);
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert_eq!(json(&response)["loggedIn"], true);
    let response = flow.post("/rest/logout", "", &mut session);
    assert_eq!(response.body_text(), "successful");

    // The code is single use.
    let mut session = Session::default();
    flow.post("/rest/login", "username=bob&password=Secret1!", &mut session);
    let response = flow.post("/rest/verifyOtp", &format!("code={code}"), &mut session);
    assert_eq!(response.status, 403, "{}", response.body_text());
}

#[test]
fn password_reset_mails_a_reset_password_that_changes_the_password() {
    let flow = Flow::open();
    flow.user("cy", "cy-reset@example.com");
    let sink = SharedSinkGuard::install();
    let messages = flow.as_user(None, || {
        let parameters = BTreeMap::from([("username".to_string(), Value::string("cy"))]);
        moqui_core::service::sync("org.moqui.impl.UserServices.reset#Password", &parameters).unwrap();
        let unknown = BTreeMap::from([("username".to_string(), Value::string("nobody"))]);
        moqui_core::service::sync("org.moqui.impl.UserServices.reset#Password", &unknown).unwrap();
        moqui_core::context::ExecutionContext::with_current(|ec| {
            assert!(ec.message().errors().is_empty(), "{:?}", ec.message().errors());
            ec.message().messages().to_vec()
        })
        .unwrap()
    });
    // The same answer for a known and an unknown user.
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert_eq!(messages[0], messages[1]);
    let sent = sink.wait_for("cy-reset@example.com", MAIL_WAIT);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].subject, "Password Reset");
    let reset = after(&sent[0].body, "Your reset password has been set to: ");
    assert_eq!(reset.len(), 12, "{}", sent[0].body);
    drop(sink);
    let stored = flow.row_text("moqui.security.UserAccount", "userId", "cy", "resetPassword");
    assert!(stored.starts_with("$argon2id$"), "{stored}");

    // The current password still logs in; the reset password does not, it only changes it.
    flow.as_user(None, || {
        moqui_core::user::login_user("cy", &reset).unwrap_err();
        moqui_core::user::login_user("cy", "Secret1!").unwrap();
        moqui_core::user::logout().unwrap();
    });
    let out = flow.as_user(None, || {
        let parameters = BTreeMap::from([
            ("username".to_string(), Value::string("cy")),
            ("oldPassword".to_string(), Value::string(&reset)),
            ("newPassword".to_string(), Value::string("Changed2!")),
            ("newPasswordVerify".to_string(), Value::string("Changed2!")),
        ]);
        let out = moqui_core::service::sync("org.moqui.impl.UserServices.update#Password", &parameters).unwrap();
        let errors = moqui_core::context::ExecutionContext::with_current(|ec| ec.message().errors().to_vec()).unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        out
    });
    assert_eq!(out.get("updateSuccessful"), Some(&Value::Bool(true)), "{out:?}");
    assert_eq!(
        flow.row_text("moqui.security.UserAccount", "userId", "cy", "resetPassword"),
        ""
    );
    flow.as_user(None, || {
        moqui_core::user::login_user("cy", "Secret1!").unwrap_err();
        moqui_core::user::login_user("cy", "Changed2!").unwrap();
        assert_eq!(moqui_core::user::current_username().unwrap().as_deref(), Some("cy"));
    });
    // The old password went into the history.
    assert!(!flow
        .row_text("moqui.security.UserPasswordHistory", "userId", "cy", "password")
        .is_empty());
}

/// The text after `marker` in a mail body, up to the next tag or space.
fn after(body: &str, marker: &str) -> String {
    let start = body.find(marker).unwrap_or_else(|| panic!("no {marker:?} in {body}")) + marker.len();
    body[start..]
        .chars()
        .take_while(|ch| !ch.is_whitespace() && *ch != '<')
        .collect()
}
