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

//! `ec.makeNotificationMessage()`: Java's `NotificationMessage` builder, backed by [`crate::notify::NotificationMessage`].
//! Each setter changes the builder and returns it. `reason`, `record`, `actorUserId` and `appName` are not in Java: the
//! IronStone notification fields (`NotificationMessage.reasonEnumId`, `recordEntityName`, `recordId`, `actorUserId`,
//! `appName`). Nor is `directOnly` (`NotificationMessage::direct_only`): the named users and group alone.

use std::sync::{Arc, Mutex, MutexGuard};

use moqui_lib::object::{to_plain_string, HostObject, Value};

use super::{method, Call, HostClass, HostError, Method, Ret};
use crate::notify::NotificationMessage;

pub(super) static MAKE: Method = method!("ec", "makeNotificationMessage", [], make);

pub(super) static NOTIFICATION: HostClass = HostClass {
    name: "host:NotificationMessage",
    methods: &[
        method!("ec.makeNotificationMessage()", "topic", [Value], topic),
        method!("ec.makeNotificationMessage()", "subTopic", [Value], sub_topic),
        method!("ec.makeNotificationMessage()", "userId", [Value], user_id),
        method!("ec.makeNotificationMessage()", "userIds", [Value], user_ids),
        method!("ec.makeNotificationMessage()", "userGroupId", [Value], user_group_id),
        method!("ec.makeNotificationMessage()", "directOnly", [OptValue], direct_only),
        method!("ec.makeNotificationMessage()", "message", [Value], message),
        method!("ec.makeNotificationMessage()", "title", [Value], title),
        method!("ec.makeNotificationMessage()", "link", [Value], link),
        method!("ec.makeNotificationMessage()", "type", [Value], type_name),
        method!("ec.makeNotificationMessage()", "showAlert", [Value], show_alert),
        method!(
            "ec.makeNotificationMessage()",
            "alertNoAutoHide",
            [Value],
            alert_no_auto_hide
        ),
        method!(
            "ec.makeNotificationMessage()",
            "persistOnSend",
            [Value],
            persist_on_send
        ),
        method!(
            "ec.makeNotificationMessage()",
            "emailTemplateId",
            [Value],
            email_template_id
        ),
        method!(
            "ec.makeNotificationMessage()",
            "emailMessageSave",
            [Value],
            email_message_save
        ),
        method!("ec.makeNotificationMessage()", "reason", [Value], reason),
        method!("ec.makeNotificationMessage()", "reasonEnumId", [Value], reason),
        method!("ec.makeNotificationMessage()", "record", [Value, OptValue], record),
        method!(
            "ec.makeNotificationMessage()",
            "recordEntityName",
            [Value],
            record_entity_name
        ),
        method!("ec.makeNotificationMessage()", "recordId", [Value], record_id),
        method!("ec.makeNotificationMessage()", "actorUserId", [Value], actor_user_id),
        method!("ec.makeNotificationMessage()", "appName", [Value], app_name),
        method!("ec.makeNotificationMessage()", "send", [OptValue], None, send),
        method!("ec.makeNotificationMessage()", "getTopic", [], get_topic),
        method!(
            "ec.makeNotificationMessage()",
            "getNotificationTopic",
            [],
            None,
            get_notification_topic
        ),
        method!("ec.makeNotificationMessage()", "getTitle", [], get_title),
        method!("ec.makeNotificationMessage()", "getLink", [], get_link),
        method!(
            "ec.makeNotificationMessage()",
            "getNotificationMessageId",
            [],
            get_notification_message_id
        ),
        method!(
            "ec.makeNotificationMessage()",
            "getNotifyUserIds",
            [],
            get_notify_user_ids
        ),
    ],
    getters: &[],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

type Builder = Mutex<NotificationMessage>;

fn make(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::Object(
        HostObject::new(Builder::new(NotificationMessage::new())),
        &NOTIFICATION,
    ))
}

/// The builder's message, locked. A notification being sent (a nested script during its send) is refused.
fn locked(builder: &Builder) -> Result<MutexGuard<'_, NotificationMessage>, HostError> {
    builder
        .try_lock()
        .map_err(|_| HostError::message("the notification is being sent"))
}

fn builder(call: &Call<'_>) -> Result<Arc<Builder>, HostError> {
    call.this::<Builder>()
}

/// Change the builder and return it.
fn changed(call: &Call<'_>, change: impl FnOnce(&mut NotificationMessage)) -> Result<Ret, HostError> {
    let builder = builder(call)?;
    change(&mut *locked(&builder)?);
    Ok(Ret::This)
}

fn text_of(call: &Call<'_>, index: usize) -> String {
    to_plain_string(&call.value_or_null(index))
}

/// A boolean argument; true for any other value, as `flag.as_bool().unwrap_or(true)`.
fn flag(call: &Call<'_>, index: usize) -> bool {
    match call.value(index) {
        Some(Value::Bool(flag)) => *flag,
        _ => true,
    }
}

fn topic(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let topic = text_of(call, 0);
    changed(call, |message| {
        message.topic(topic);
    })
}

fn sub_topic(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let sub_topic = text_of(call, 0);
    changed(call, |message| {
        message.sub_topic(sub_topic);
    })
}

fn user_id(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let user_id = text_of(call, 0);
    changed(call, |message| {
        message.user_id(user_id);
    })
}

/// An array or a set of user IDs, added to those set.
fn user_ids(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let ids = super::host_texts(call.value_or_null(0));
    changed(call, |message| {
        message.user_ids(ids);
    })
}

fn user_group_id(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let group_id = text_of(call, 0);
    changed(call, |message| {
        message.user_group_id(group_id);
    })
}

/// `directOnly()` or `directOnly(flag)`: the named users and group alone receive it; a user subscribed to every
/// message of the topic (`NotificationTopicUser.allNotifications`) does not.
fn direct_only(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let on = flag(call, 0);
    changed(call, |message| {
        message.direct_only(on);
    })
}

/// `message(map)` or `message(jsonText)`.
fn message(call: &mut Call<'_>) -> Result<Ret, HostError> {
    match call.value_or_null(0) {
        Value::Map(map) => changed(call, |message| {
            message.message_map(map);
        }),
        Value::Null => changed(call, |message| {
            message.message_map(indexmap::IndexMap::default());
        }),
        other => {
            let json = to_plain_string(&other);
            changed(call, |message| {
                message.message_json(json);
            })
        }
    }
}

fn title(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let title = text_of(call, 0);
    changed(call, |message| {
        message.title(title);
    })
}

fn link(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let link = text_of(call, 0);
    changed(call, |message| {
        message.link(link);
    })
}

/// `type(name)`: `info`, `success`, `warning`, or `danger`.
fn type_name(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let type_name = text_of(call, 0);
    changed(call, |message| {
        message.notification_type(type_name);
    })
}

fn show_alert(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let on = flag(call, 0);
    changed(call, |message| {
        message.show_alert(on);
    })
}

fn alert_no_auto_hide(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let on = flag(call, 0);
    changed(call, |message| {
        message.alert_no_auto_hide(on);
    })
}

fn persist_on_send(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let on = flag(call, 0);
    changed(call, |message| {
        message.persist(on);
    })
}

fn email_template_id(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let id = text_of(call, 0);
    changed(call, |message| {
        message.email_template_id(id);
    })
}

fn email_message_save(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let on = flag(call, 0);
    changed(call, |message| {
        message.email_message_save(on);
    })
}

/// `reason(name)`: a `NotificationReason` enumId (`NrMention`) or the short name of a seeded one (`mention`,
/// `assignment`, `approval`, `dateAlert`, `reminder`, `comment`, `system`). `reasonEnumId` is the same.
fn reason(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let reason = text_of(call, 0);
    changed(call, |message| {
        message.reason(reason);
    })
}

/// `record(entityName, recordId)`: the record the notification is about.
fn record(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let entity_name = text_of(call, 0);
    let record_id = if call.passed(1) {
        text_of(call, 1)
    } else {
        String::new()
    };
    changed(call, |message| {
        message.record(entity_name, record_id);
    })
}

fn record_entity_name(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let entity_name = text_of(call, 0);
    changed(call, |message| {
        message.record_entity_name(entity_name);
    })
}

fn record_id(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let record_id = text_of(call, 0);
    changed(call, |message| {
        message.record_id(record_id);
    })
}

/// `actorUserId(userId)`: who did what the notification tells of; the current user when not set.
fn actor_user_id(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let user_id = text_of(call, 0);
    changed(call, |message| {
        message.actor_user_id(user_id);
    })
}

/// `appName(name)`: the app root the notification comes from, as in its `/qapps` path.
fn app_name(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let app_name = text_of(call, 0);
    changed(call, |message| {
        message.app_name(app_name);
    })
}

/// `send()` or `send(persist)`: returns the notification, so `getNotificationMessageId()` follows.
fn send(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let builder = builder(call)?;
    let mut message = locked(&builder)?;
    if let Some(Value::Bool(on)) = call.value(0) {
        message.persist(*on);
    }
    message.send().map_err(|err| HostError::message(err.message()))?;
    Ok(Ret::This)
}

fn get_topic(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let builder = builder(call)?;
    let topic = locked(&builder)?.topic_name().to_string();
    Ok(topic.into())
}

/// The `NotificationTopic` value of the topic, or null when the topic has none.
fn get_notification_topic(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let builder = builder(call)?;
    let found = locked(&builder)?
        .notification_topic()
        .map_err(|err| HostError::message(err.message()))?;
    Ok(Ret::Value(match found {
        Some(value) => Value::Object(HostObject(Arc::new(Mutex::new(value)))),
        None => Value::Null,
    }))
}

fn optional(text: Option<&str>) -> Ret {
    Ret::Value(text.map_or(Value::Null, Value::string))
}

/// The title as expanded by the last send, null before.
fn get_title(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let builder = builder(call)?;
    let message = locked(&builder)?;
    Ok(optional(message.title_text()))
}

fn get_link(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let builder = builder(call)?;
    let message = locked(&builder)?;
    Ok(optional(message.link_text()))
}

fn get_notification_message_id(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let builder = builder(call)?;
    let message = locked(&builder)?;
    Ok(optional(message.notification_message_id()))
}

fn get_notify_user_ids(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let builder = builder(call)?;
    let ids: Vec<Value> = locked(&builder)?.notify_user_ids().iter().map(Value::string).collect();
    Ok(Ret::Value(Value::List(ids)))
}
