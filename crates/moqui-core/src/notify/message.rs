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

//! Notification message builder. Recipient rows are one find per source, not one find per user.

use std::collections::{BTreeSet, HashMap};

use indexmap::IndexMap;
use moqui_lib::object::{basic_convert, to_plain_string, Value};

use super::topic::{self, TopicMessage, NOTIFICATION_MESSAGE};
use crate::context::{Error, ExecutionContext};
use crate::entity::{Condition, EntityFacade, EntityValue};
use crate::transaction::TransactionAttribute;

const TOPIC: &str = "moqui.security.user.NotificationTopic";
const TOPIC_USER: &str = "moqui.security.user.NotificationTopicUser";
const MESSAGE: &str = "moqui.security.user.NotificationMessage";
const MESSAGE_USER: &str = "moqui.security.user.NotificationMessageUser";
const USER_ACCOUNT: &str = "moqui.security.UserAccount";
const GROUP_MEMBER: &str = "moqui.security.UserGroupMember";
const EMAIL_SERVICE: &str = "org.moqui.impl.EmailServices.send#EmailTemplate";

/// The fields of a notification that say why it came, what it is about, who did it and from which app (IronStone
/// design-drawers-commons §19, "Notification fields and approvals"). They are stored on the `NotificationMessage` row
/// and sent in the wrapped message.
#[derive(Debug, Default, Clone)]
struct RecordFields {
    reason_enum_id: Option<String>,
    record_entity_name: Option<String>,
    record_id: Option<String>,
    actor_user_id: Option<String>,
    app_name: Option<String>,
}

/// The `enumCode` of each `NotificationReason` and its `enumId`, so a sender may name the reason by its short name.
const REASON_CODES: [(&str, &str); 7] = [
    ("mention", "NrMention"),
    ("assignment", "NrAssignment"),
    ("approval", "NrApproval"),
    ("dateAlert", "NrDateAlert"),
    ("reminder", "NrReminder"),
    ("comment", "NrComment"),
    ("system", "NrSystem"),
];

fn non_empty(text: String) -> Option<String> {
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// A notification to send. Title and link templates expand once, in [`Self::send`].
#[derive(Debug, Default)]
pub struct NotificationMessage {
    topic: String,
    sub_topic: Option<String>,
    user_ids: Vec<String>,
    user_group_id: Option<String>,
    message_map: Option<IndexMap<String, Value>>,
    message_json: Option<String>,
    title_template: Option<String>,
    link_template: Option<String>,
    title_text: Option<String>,
    link_text: Option<String>,
    type_name: Option<String>,
    show_alert: Option<bool>,
    alert_no_auto_hide: Option<bool>,
    persist: Option<bool>,
    email_template_id: Option<String>,
    email_message_save: Option<bool>,
    record: RecordFields,
    notification_message_id: Option<String>,
    notify_user_ids: Vec<String>,
    email_message_id_by_user_id: HashMap<String, String>,
}

impl NotificationMessage {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn topic(&mut self, topic: impl Into<String>) -> &mut Self {
        self.topic = topic.into();
        self
    }

    pub fn sub_topic(&mut self, sub_topic: impl Into<String>) -> &mut Self {
        self.sub_topic = Some(sub_topic.into());
        self
    }

    pub fn user_id(&mut self, user_id: impl Into<String>) -> &mut Self {
        let user_id = user_id.into();
        if !user_id.is_empty() && !self.user_ids.iter().any(|id| id == &user_id) {
            self.user_ids.push(user_id);
        }
        self
    }

    pub fn user_ids<I, S>(&mut self, user_ids: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for user_id in user_ids {
            self.user_id(user_id);
        }
        self
    }

    pub fn user_group_id(&mut self, user_group_id: impl Into<String>) -> &mut Self {
        self.user_group_id = Some(user_group_id.into());
        self
    }

    pub fn message_map(&mut self, message: IndexMap<String, Value>) -> &mut Self {
        self.message_json = None;
        self.message_map = Some(message);
        self
    }

    pub fn message_json(&mut self, message: impl Into<String>) -> &mut Self {
        self.message_map = None;
        self.message_json = Some(message.into());
        self
    }

    pub fn title(&mut self, title: impl Into<String>) -> &mut Self {
        self.title_template = Some(title.into());
        self
    }

    pub fn link(&mut self, link: impl Into<String>) -> &mut Self {
        self.link_template = Some(link.into());
        self
    }

    /// `info`, `success`, `warning`, or `danger`. An unknown value fails in [`Self::send`].
    pub fn notification_type(&mut self, type_name: impl Into<String>) -> &mut Self {
        self.type_name = Some(type_name.into());
        self
    }

    pub fn show_alert(&mut self, show: bool) -> &mut Self {
        self.show_alert = Some(show);
        self
    }

    pub fn alert_no_auto_hide(&mut self, no_auto_hide: bool) -> &mut Self {
        self.alert_no_auto_hide = Some(no_auto_hide);
        self
    }

    /// When unset, [`Self::send`] uses the topic's `persistOnSend`. A missing topic does not persist.
    pub fn persist(&mut self, persist: bool) -> &mut Self {
        self.persist = Some(persist);
        self
    }

    pub fn email_template_id(&mut self, email_template_id: impl Into<String>) -> &mut Self {
        let email_template_id = email_template_id.into();
        self.email_template_id = if email_template_id.is_empty() {
            None
        } else {
            Some(email_template_id)
        };
        self
    }

    pub fn email_message_save(&mut self, save: bool) -> &mut Self {
        self.email_message_save = Some(save);
        self
    }

    /// The `NotificationReason` of the notification: an `enumId` (`NrMention`), or the short name of one of the seeded
    /// reasons (`mention`, `assignment`, `approval`, `dateAlert`, `reminder`, `comment`, `system`). Empty clears it.
    pub fn reason(&mut self, reason: impl Into<String>) -> &mut Self {
        let reason = reason.into();
        let enum_id = REASON_CODES
            .iter()
            .find(|(code, _)| *code == reason)
            .map(|(_, enum_id)| (*enum_id).to_string())
            .unwrap_or(reason);
        self.record.reason_enum_id = non_empty(enum_id);
        self
    }

    /// The record the notification is about. Empty clears it.
    pub fn record(&mut self, entity_name: impl Into<String>, record_id: impl Into<String>) -> &mut Self {
        self.record_entity_name(entity_name).record_id(record_id)
    }

    pub fn record_entity_name(&mut self, entity_name: impl Into<String>) -> &mut Self {
        self.record.record_entity_name = non_empty(entity_name.into());
        self
    }

    pub fn record_id(&mut self, record_id: impl Into<String>) -> &mut Self {
        self.record.record_id = non_empty(record_id.into());
        self
    }

    /// Who did what the notification tells of. When unset, [`Self::send`] uses the current user, if any.
    pub fn actor_user_id(&mut self, actor_user_id: impl Into<String>) -> &mut Self {
        self.record.actor_user_id = non_empty(actor_user_id.into());
        self
    }

    /// The app the notification comes from: an app root's name, as in its `/qapps` path.
    pub fn app_name(&mut self, app_name: impl Into<String>) -> &mut Self {
        self.record.app_name = non_empty(app_name.into());
        self
    }

    pub fn reason_enum_id(&self) -> Option<&str> {
        self.record.reason_enum_id.as_deref()
    }

    pub fn topic_name(&self) -> &str {
        &self.topic
    }

    /// Java's `getNotificationTopic`: the `NotificationTopic` row of the topic, `None` when the topic has none.
    ///
    /// # Errors
    ///
    /// Returns an error when the entity facade is not installed or the find fails.
    pub fn notification_topic(&self) -> Result<Option<EntityValue>, Error> {
        if self.topic.is_empty() {
            return Ok(None);
        }
        let facade = current_facade()?;
        let _authz = ExecutionContext::with_current(|ctx| ctx.disable_authz());
        Ok(facade.find(TOPIC)?.eq("topic", Value::string(&self.topic)).one()?)
    }

    /// Expanded title from the last [`Self::send`]. Not expanded again.
    pub fn title_text(&self) -> Option<&str> {
        self.title_text.as_deref()
    }

    /// Expanded link from the last [`Self::send`]. Not expanded again.
    pub fn link_text(&self) -> Option<&str> {
        self.link_text.as_deref()
    }

    pub fn notification_message_id(&self) -> Option<&str> {
        self.notification_message_id.as_deref()
    }

    /// Recipients from the last [`Self::send`].
    pub fn notify_user_ids(&self) -> &[String] {
        &self.notify_user_ids
    }

    /// The `EmailMessage` id each recipient's email saved in the last [`Self::send`], when the send
    /// created one (`emailMessageSave`).
    pub fn email_message_id_by_user_id(&self) -> &HashMap<String, String> {
        &self.email_message_id_by_user_id
    }

    /// Resolve recipients, persist when required, publish, then try email.
    ///
    /// Email failure is logged. It does not fail this call.
    ///
    /// # Errors
    ///
    /// Returns an error when the topic name or type is invalid, expansion fails, or the rows cannot be written.
    pub fn send(&mut self) -> Result<(), Error> {
        if self.topic.is_empty() {
            return Err(Error::new("notification topic is required"));
        }
        // under the read fence or in a sim preview nothing is published
        crate::entity::fence::check_effect(crate::entity::fence::Effect::Notify, &self.topic)?;
        let facade = current_facade()?;
        let _authz = ExecutionContext::with_current(|ctx| ctx.disable_authz());
        let topic = facade.find(TOPIC)?.eq("topic", Value::string(&self.topic)).one()?;
        let type_name = resolve_type(self.type_name.as_deref(), topic.as_ref())?;
        let persist = resolve_persist(self.persist, topic.as_ref());
        let show_alert = resolve_bool(self.show_alert, topic.as_ref(), "showAlert");
        let alert_no_auto_hide = resolve_bool(self.alert_no_auto_hide, topic.as_ref(), "alertNoAutoHide");
        let message = self.resolved_message()?;
        if self.record.actor_user_id.is_none() {
            self.record.actor_user_id = crate::user::current_user_id()
                .ok()
                .flatten()
                .filter(|id| !id.is_empty());
        }
        let title_template = self
            .title_template
            .clone()
            .filter(|text| !text.is_empty())
            .or_else(|| topic_title(topic.as_ref(), &type_name));
        let link_template = self
            .link_template
            .clone()
            .filter(|text| !text.is_empty())
            .or_else(|| topic.as_ref().and_then(|row| field_str(row, "linkTemplate")));
        self.title_text = Some(expand_template(title_template.as_deref().unwrap_or(""), &message)?);
        self.link_text = Some(expand_template(link_template.as_deref().unwrap_or(""), &message)?);
        let resolved = resolve_users(
            &facade,
            &self.topic,
            &self.user_ids,
            self.user_group_id.as_deref(),
            topic.as_ref(),
        )?;
        self.notify_user_ids = resolved.users;
        let sent = if persist {
            let sent = now_value();
            let id = persist_rows(
                &facade,
                &self.topic,
                self.sub_topic.as_deref(),
                self.user_group_id.as_deref(),
                &sent,
                stored_json(&message, self.message_json.as_deref()),
                self.title_text.as_deref().unwrap_or(""),
                self.link_text.as_deref().unwrap_or(""),
                &type_name,
                show_alert,
                &self.record,
                &self.notify_user_ids,
            )?;
            self.notification_message_id = Some(id);
            Some(sent)
        } else {
            None
        };
        let wrapped = wrapped_message(
            self,
            topic.as_ref(),
            &message,
            &type_name,
            persist,
            show_alert,
            alert_no_auto_hide,
            sent.as_ref(),
        );
        let message_id = match self.notification_message_id.clone() {
            Some(id) => id,
            None => new_id()?,
        };
        // The topic carries who the message is for, beside Java's wrapped message, so a socket (here or on a cluster
        // peer) can pick its users. The socket sends the client the wrapped message without it.
        let mut routed = wrapped.clone();
        routed.insert(
            ROUTE_USER_IDS.to_string(),
            Value::List(self.notify_user_ids.iter().map(Value::string).collect()),
        );
        crate::cluster::publish(
            &facade,
            NOTIFICATION_MESSAGE,
            &TopicMessage {
                origin_id: topic::origin_id().to_string(),
                message_id,
                body: moqui_lib::json::to_json(&Value::Map(routed)),
            },
        )?;
        self.email_message_id_by_user_id = send_emails(
            &facade,
            topic.as_ref(),
            &resolved.topic_users,
            &resolved.emails,
            &self.notify_user_ids,
            self.email_template_id.as_deref(),
            self.email_message_save,
            &wrapped,
            self.notification_message_id.as_deref(),
        );
        Ok(())
    }

    fn resolved_message(&self) -> Result<IndexMap<String, Value>, Error> {
        if let Some(message) = &self.message_map {
            return Ok(message.clone());
        }
        let Some(json) = &self.message_json else {
            return Ok(IndexMap::new());
        };
        if json.is_empty() {
            return Ok(IndexMap::new());
        }
        let parsed = serde_json::from_str::<serde_json::Value>(json)
            .map_err(|err| Error::new(format!("notification message is not JSON: {err}")))?;
        match moqui_lib::json::from_json(&parsed) {
            Value::Map(message) => Ok(message),
            _ => Err(Error::new("notification message JSON must be an object")),
        }
    }
}

struct Resolved {
    users: Vec<String>,
    emails: HashMap<String, String>,
    topic_users: Vec<EntityValue>,
}

fn resolve_users(
    facade: &EntityFacade,
    topic_name: &str,
    explicit: &[String],
    user_group_id: Option<&str>,
    topic: Option<&EntityValue>,
) -> Result<Resolved, Error> {
    let topic_users = facade
        .find(TOPIC_USER)?
        .eq("topic", Value::string(topic_name))
        .list()?
        .values()
        .to_vec();
    let now = jiff::Timestamp::now();
    let mut recipients = BTreeSet::new();
    let mut checked = BTreeSet::new();
    for user_id in explicit {
        checked.insert(user_id.clone());
        if receives(user_id, &topic_users, topic) {
            recipients.insert(user_id.clone());
        }
    }
    if let Some(user_group_id) = user_group_id.filter(|id| !id.is_empty()) {
        let members = facade
            .find(GROUP_MEMBER)?
            .eq("userGroupId", Value::string(user_group_id))
            .list()?;
        for member in members.values() {
            if !member_active(member, now) {
                continue;
            }
            let user_id = field_str(member, "userId").unwrap_or_default();
            if user_id.is_empty() || !checked.insert(user_id.clone()) {
                continue;
            }
            if receives(&user_id, &topic_users, topic) {
                recipients.insert(user_id);
            }
        }
    }
    for row in &topic_users {
        if yn(row, "allNotifications") == Some(true) {
            if let Some(user_id) = field_str(row, "userId") {
                recipients.insert(user_id);
            }
        }
    }
    let mut emails = HashMap::new();
    if !recipients.is_empty() {
        let ids: Vec<Value> = recipients.iter().cloned().map(Value::string).collect();
        let accounts = facade
            .find(USER_ACCOUNT)?
            .condition(Condition::is_in("userId", ids))
            .list()?;
        for account in accounts.values() {
            let Some(user_id) = field_str(account, "userId") else {
                continue;
            };
            if terminated(account, now) {
                recipients.remove(&user_id);
                continue;
            }
            if let Some(email) = field_str(account, "emailAddress") {
                emails.insert(user_id, email);
            }
        }
    }
    Ok(Resolved {
        users: recipients.into_iter().collect(),
        emails,
        topic_users,
    })
}

fn receives(user_id: &str, topic_users: &[EntityValue], topic: Option<&EntityValue>) -> bool {
    if let Some(row) = topic_user(topic_users, user_id) {
        if let Some(flag) = yn(row, "receiveNotifications") {
            return flag;
        }
    }
    if let Some(flag) = topic.and_then(|row| yn(row, "receiveNotifications")) {
        return flag;
    }
    true
}

fn topic_user<'a>(topic_users: &'a [EntityValue], user_id: &str) -> Option<&'a EntityValue> {
    topic_users
        .iter()
        .find(|row| field_str(row, "userId").as_deref() == Some(user_id))
}

fn member_active(row: &EntityValue, now: jiff::Timestamp) -> bool {
    if let Some(from) = row.get("fromDate").filter(|value| !value.is_null()) {
        match as_timestamp(from) {
            Some(stamp) if stamp <= now => {}
            _ => return false,
        }
    }
    if let Some(thru) = row.get("thruDate").filter(|value| !value.is_null()) {
        match as_timestamp(thru) {
            Some(stamp) if stamp > now => {}
            _ => return false,
        }
    }
    true
}

fn terminated(row: &EntityValue, now: jiff::Timestamp) -> bool {
    row.get("terminateDate")
        .filter(|value| !value.is_null())
        .and_then(as_timestamp)
        .is_some_and(|stamp| now > stamp)
}

fn as_timestamp(value: &Value) -> Option<jiff::Timestamp> {
    match &crate::l10n::basic_convert(value, "Timestamp").value {
        Value::Timestamp(stamp) => Some(*stamp),
        _ => None,
    }
}

/// ForceNew commits before the caller can roll back.
#[allow(clippy::too_many_arguments)]
fn persist_rows(
    facade: &EntityFacade,
    topic: &str,
    sub_topic: Option<&str>,
    user_group_id: Option<&str>,
    sent: &Value,
    message_json: Option<String>,
    title: &str,
    link: &str,
    type_name: &str,
    show_alert: bool,
    record: &RecordFields,
    users: &[String],
) -> Result<String, Error> {
    let tx = facade.transactions_for(MESSAGE)?;
    tx.run(TransactionAttribute::ForceNew, None, || {
        let mut row = facade.make_value(MESSAGE)?;
        row.set_sequenced_id_primary(facade)?;
        let id = field_str(&row, "notificationMessageId")
            .ok_or_else(|| moqui_lib::error::Error::new("notification message id was not sequenced"))?;
        row.set("topic", Value::string(topic));
        if let Some(sub_topic) = sub_topic.filter(|text| !text.is_empty()) {
            row.set("subTopic", Value::string(sub_topic));
        }
        if let Some(user_group_id) = user_group_id.filter(|text| !text.is_empty()) {
            row.set("userGroupId", Value::string(user_group_id));
        }
        row.set("sentDate", sent.clone());
        if let Some(message_json) = &message_json {
            row.set("messageJson", Value::string(message_json));
        }
        if !title.is_empty() {
            row.set("titleText", Value::string(title));
        }
        if !link.is_empty() {
            row.set("linkText", Value::string(link));
        }
        row.set("typeString", Value::string(type_name));
        row.set("showAlert", Value::string(if show_alert { "Y" } else { "N" }));
        for (field, value) in record.fields() {
            if let Some(value) = value {
                row.set(field, Value::string(value));
            }
        }
        row.create(facade)?;
        for user_id in users {
            let mut user = facade.make_value(MESSAGE_USER)?;
            user.set("notificationMessageId", Value::string(&id));
            user.set("userId", Value::string(user_id));
            user.create(facade)?;
        }
        Ok(id)
    })
    .map_err(Error::from)
}

/// The key of the recipients in a `notification-message` topic body. It is for routing and is not sent to a client.
pub const ROUTE_USER_IDS: &str = "userIds";

#[allow(clippy::too_many_arguments)]
fn wrapped_message(
    message: &NotificationMessage,
    topic: Option<&EntityValue>,
    body: &IndexMap<String, Value>,
    type_name: &str,
    persist: bool,
    show_alert: bool,
    alert_no_auto_hide: bool,
    sent: Option<&Value>,
) -> IndexMap<String, Value> {
    let mut wrapped = IndexMap::new();
    wrapped.insert("topic".to_string(), Value::string(&message.topic));
    wrapped.insert(
        "subTopic".to_string(),
        message
            .sub_topic
            .as_ref()
            .filter(|text| !text.is_empty())
            .map(Value::string)
            .unwrap_or(Value::Null),
    );
    wrapped.insert("sentDate".to_string(), sent.cloned().unwrap_or(Value::Null));
    wrapped.insert(
        "notificationMessageId".to_string(),
        message
            .notification_message_id
            .as_ref()
            .map(Value::string)
            .unwrap_or(Value::Null),
    );
    wrapped.insert(
        "topicDescription".to_string(),
        topic
            .and_then(|row| field_str(row, "description"))
            .map(Value::string)
            .unwrap_or(Value::Null),
    );
    wrapped.insert("message".to_string(), Value::Map(body.clone()));
    wrapped.insert(
        "title".to_string(),
        Value::string(message.title_text.as_deref().unwrap_or("")),
    );
    wrapped.insert(
        "link".to_string(),
        Value::string(message.link_text.as_deref().unwrap_or("")),
    );
    wrapped.insert("type".to_string(), Value::string(type_name));
    wrapped.insert("persistOnSend".to_string(), Value::Bool(persist));
    wrapped.insert("showAlert".to_string(), Value::Bool(show_alert));
    wrapped.insert("alertNoAutoHide".to_string(), Value::Bool(alert_no_auto_hide));
    for (field, value) in message.record.fields() {
        wrapped.insert(field.to_string(), value.map(Value::string).unwrap_or(Value::Null));
    }
    // The inbox is the topic's, so the client tells the actionable notifications from the automatic updates.
    wrapped.insert(
        "inboxEnumId".to_string(),
        Value::string(
            topic
                .and_then(|row| field_str(row, "inboxEnumId"))
                .unwrap_or_else(|| DEFAULT_INBOX.to_string()),
        ),
    );
    wrapped
}

/// The `NotificationInbox` of a topic that names none, and of a topic with no `NotificationTopic` record.
pub const DEFAULT_INBOX: &str = "NinNotifications";

impl RecordFields {
    /// Each field by its `NotificationMessage` field name, also its key in the wrapped message.
    fn fields(&self) -> [(&'static str, Option<&str>); 5] {
        [
            ("reasonEnumId", self.reason_enum_id.as_deref()),
            ("recordEntityName", self.record_entity_name.as_deref()),
            ("recordId", self.record_id.as_deref()),
            ("actorUserId", self.actor_user_id.as_deref()),
            ("appName", self.app_name.as_deref()),
        ]
    }
}

/// Java's `NotificationMessageImpl.send` email loop: `send#EmailTemplate` for each recipient who wants
/// email and has an address, with the wrapped message as `bodyParameters`. A saved `EmailMessage` id is
/// returned by user and, when the notification was persisted, stored on its `NotificationMessageUser`
/// row with the sent date. A failed email is logged; the messages and errors of the caller are restored,
/// so it never fails the in-app send.
#[allow(clippy::too_many_arguments)]
fn send_emails(
    facade: &EntityFacade,
    topic: Option<&EntityValue>,
    topic_users: &[EntityValue],
    emails: &HashMap<String, String>,
    users: &[String],
    caller_template: Option<&str>,
    caller_save: Option<bool>,
    wrapped: &IndexMap<String, Value>,
    notification_message_id: Option<&str>,
) -> HashMap<String, String> {
    let mut sent_ids = HashMap::new();
    let Some(template_id) = caller_template
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .or_else(|| topic.and_then(|row| field_str(row, "emailTemplateId")))
    else {
        return sent_ids;
    };
    let save = caller_save
        .or_else(|| topic.and_then(|row| yn(row, "emailMessageSave")))
        .unwrap_or(false);
    let saved = save_messages();
    for user_id in users {
        if !wants_email(user_id, topic_users, topic) {
            continue;
        }
        let Some(address) = emails.get(user_id).filter(|address| !address.is_empty()) else {
            continue;
        };
        let mut params = std::collections::BTreeMap::new();
        params.insert("emailTemplateId".to_string(), Value::string(&template_id));
        params.insert("toAddresses".to_string(), Value::string(address));
        params.insert("bodyParameters".to_string(), Value::Map(wrapped.clone()));
        params.insert("toUserId".to_string(), Value::string(user_id));
        params.insert("createEmailMessage".to_string(), Value::Bool(save));
        let out = match crate::service::sync(EMAIL_SERVICE, &params) {
            Ok(out) => out,
            Err(err) => {
                tracing::warn!(user_id = %user_id, error = %err, "notification email was not sent");
                continue;
            }
        };
        let email_message_id = out.get("emailMessageId").map(to_plain_string).unwrap_or_default();
        if email_message_id.is_empty() {
            continue;
        }
        if let Some(id) = notification_message_id.filter(|id| !id.is_empty()) {
            if let Err(err) = store_email_message_id(facade, id, user_id, &email_message_id) {
                tracing::warn!(user_id = %user_id, error = %err, "notification email id was not stored");
            }
        }
        sent_ids.insert(user_id.clone(), email_message_id);
    }
    log_new_email_errors(user_errors(&saved));
    restore_messages(saved);
    sent_ids
}

/// `store#NotificationMessageUser` with the email id and the sent date: an update of the row the
/// persist created, or a new row.
fn store_email_message_id(
    facade: &EntityFacade,
    notification_message_id: &str,
    user_id: &str,
    email_message_id: &str,
) -> Result<(), Error> {
    let _authz = ExecutionContext::with_current(|ctx| ctx.disable_authz());
    let existing = facade
        .find(MESSAGE_USER)?
        .eq("notificationMessageId", Value::string(notification_message_id))
        .eq("userId", Value::string(user_id))
        .one()?;
    let mut row = facade.make_value(MESSAGE_USER)?;
    row.set("notificationMessageId", Value::string(notification_message_id));
    row.set("userId", Value::string(user_id));
    row.set("emailMessageId", Value::string(email_message_id));
    row.set("sentDate", now_value());
    if existing.is_some() {
        row.update(facade)?;
    } else {
        row.create(facade)?;
    }
    Ok(())
}

fn wants_email(user_id: &str, topic_users: &[EntityValue], topic: Option<&EntityValue>) -> bool {
    if let Some(row) = topic_user(topic_users, user_id) {
        if let Some(flag) = yn(row, "emailNotifications") {
            return flag;
        }
    }
    topic.and_then(|row| yn(row, "emailNotifications")) == Some(true)
}

struct SavedMessages {
    messages: Vec<String>,
    errors: Vec<String>,
}

fn save_messages() -> SavedMessages {
    ExecutionContext::with_current(|ctx| {
        let messages = ctx.message();
        SavedMessages {
            messages: messages.messages().to_vec(),
            errors: messages.errors().to_vec(),
        }
    })
    .unwrap_or(SavedMessages {
        messages: Vec::new(),
        errors: Vec::new(),
    })
}

fn user_errors(saved: &SavedMessages) -> Vec<String> {
    ExecutionContext::with_current(|ctx| {
        ctx.message()
            .errors()
            .iter()
            .filter(|error| !saved.errors.iter().any(|prior| prior == *error))
            .cloned()
            .collect()
    })
    .unwrap_or_default()
}

fn log_new_email_errors(errors: Vec<String>) {
    for error in errors {
        tracing::warn!(error = %error, "notification email was not sent");
    }
}

fn restore_messages(saved: SavedMessages) {
    ExecutionContext::with_current(|ctx| {
        let mut messages = ctx.message();
        if messages.messages() == saved.messages.as_slice() && messages.errors() == saved.errors.as_slice() {
            return;
        }
        messages.clear();
        for message in saved.messages {
            messages.add_message(message);
        }
        for error in saved.errors {
            messages.add_error(error);
        }
    });
}

fn expand_template(template: &str, message: &IndexMap<String, Value>) -> Result<String, Error> {
    if !template.contains('$') || template.ends_with('$') {
        return Ok(template.to_string());
    }
    let _frame = crate::script::ContextFrame::push();
    for (key, value) in message {
        if key == "context" {
            continue;
        }
        crate::script::set_context_name(key, value.clone())?;
    }
    crate::script::expand(template)
}

fn topic_title(topic: Option<&EntityValue>, type_name: &str) -> Option<String> {
    let topic = topic?;
    if type_name == "danger" {
        if let Some(title) = field_str(topic, "errorTitleTemplate") {
            return Some(title);
        }
    }
    field_str(topic, "titleTemplate")
}

fn resolve_type(set: Option<&str>, topic: Option<&EntityValue>) -> Result<String, Error> {
    let chosen = set
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .or_else(|| topic.and_then(|row| field_str(row, "typeString")))
        .unwrap_or_else(|| "info".to_string());
    match chosen.as_str() {
        "info" | "success" | "warning" | "danger" => Ok(chosen),
        other => Err(Error::new(format!(
            "notification type {other} is not info, success, warning, or danger"
        ))),
    }
}

fn resolve_persist(set: Option<bool>, topic: Option<&EntityValue>) -> bool {
    if let Some(persist) = set {
        return persist;
    }
    topic.and_then(|row| yn(row, "persistOnSend")).unwrap_or(false)
}

fn resolve_bool(set: Option<bool>, topic: Option<&EntityValue>, field: &str) -> bool {
    if let Some(value) = set {
        return value;
    }
    topic.and_then(|row| yn(row, field)).unwrap_or(false)
}

fn stored_json(message: &IndexMap<String, Value>, raw: Option<&str>) -> Option<String> {
    if let Some(raw) = raw.filter(|text| !text.is_empty()) {
        return Some(raw.to_string());
    }
    if message.is_empty() {
        return None;
    }
    Some(moqui_lib::json::to_json(&Value::Map(message.clone())).to_string())
}

fn field_str(row: &EntityValue, name: &str) -> Option<String> {
    let value = row.get(name)?;
    if value.is_null() {
        return None;
    }
    let text = match value {
        Value::String(text) => text.clone(),
        other => to_plain_string(other),
    };
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn yn(row: &EntityValue, name: &str) -> Option<bool> {
    field_str(row, name).map(|text| text.eq_ignore_ascii_case("Y"))
}

fn now_value() -> Value {
    basic_convert(
        &Value::I64(jiff::Timestamp::now().as_millisecond()),
        "java.sql.Timestamp",
    )
    .value
}

fn new_id() -> Result<String, Error> {
    crate::entity::uuid_v4().map_err(Error::from)
}

fn current_facade() -> Result<EntityFacade, Error> {
    ExecutionContext::with_current(|ctx| ctx.factory().entity())
        .flatten()
        .ok_or_else(|| Error::new("no entity facade is installed"))
}

/// No persisted id does nothing. A missing user is an error.
///
/// # Errors
///
/// Returns an error when `user_id` is empty or the row cannot be updated.
pub fn mark_sent(notification_message_id: &str, user_id: &str) -> Result<(), Error> {
    if notification_message_id.is_empty() {
        return Ok(());
    }
    if user_id.is_empty() {
        return Err(Error::new("userId is required to mark a notification sent"));
    }
    touch_user(notification_message_id, user_id, "sentDate")
}

/// No id or no user is an error.
///
/// # Errors
///
/// Returns an error when either id is empty or the row cannot be updated.
pub fn mark_viewed(notification_message_id: &str, user_id: &str) -> Result<(), Error> {
    if notification_message_id.is_empty() {
        return Err(Error::new(
            "notificationMessageId is required to mark a notification viewed",
        ));
    }
    if user_id.is_empty() {
        return Err(Error::new("userId is required to mark a notification viewed"));
    }
    touch_user(notification_message_id, user_id, "viewedDate")
}

fn touch_user(notification_message_id: &str, user_id: &str, field: &str) -> Result<(), Error> {
    let facade = current_facade()?;
    let _authz = ExecutionContext::with_current(|ctx| ctx.disable_authz());
    let mut row = facade.make_value(MESSAGE_USER)?;
    row.set("notificationMessageId", Value::string(notification_message_id));
    row.set("userId", Value::string(user_id));
    row.set(field, now_value());
    row.update(&facade)?;
    Ok(())
}

/// Topic `ServiceJobError`, type `danger`, with the job name and the error text.
///
/// # Errors
///
/// Returns an error when the notification cannot be published.
pub(crate) fn service_job_error(
    job_name: &str,
    job_run_id: &str,
    errors: &str,
    user_ids: &[String],
    extra: IndexMap<String, Value>,
) -> Result<(), Error> {
    let mut message = IndexMap::new();
    message.insert("jobName".to_string(), Value::string(job_name));
    message.insert("errors".to_string(), Value::string(errors));
    message.insert("message".to_string(), Value::string(errors));
    // Java's `serviceCallRun`, `parameters` and `results`.
    message.extend(extra);
    let mut note = NotificationMessage::new();
    note.topic("ServiceJobError")
        .notification_type("danger")
        .title(format!("Job Error {job_name} [{job_run_id}] {errors}"))
        .message_map(message)
        .user_ids(user_ids.iter().cloned())
        .send()
}
