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

//! The notification capture, for specs (no Java counterpart; docs/Build-Test.md, "Notifications"). While a spec has it
//! on (`ec.factory.getTool('NotificationCapture').enable()`), it listens on the [`NOTIFICATION_MESSAGE`] topic as the
//! host's notification socket does, so what is sent only when someone listens (the shell counts) is sent, and keeps
//! each message in memory, where the tool reads and clears it. A `moqui test` run has no socket otherwise. The most
//! recent [`MAX_CAPTURED`] are kept.
//!
//! The tool: `isEnabled()`, `enable()`, `disable()`, `getMessages(topic?)` (oldest first, each `{topic, subTopic,
//! userIds, title, link, message, capturedDate}`), `count(topic?)`, `last(topic?)` and `clear()`.

use std::sync::{Arc, Mutex};

use indexmap::IndexMap;
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;

use super::topic::{subscribe, Subscription, TopicMessage, NOTIFICATION_MESSAGE};
use super::ROUTE_USER_IDS;
use crate::context::{Error, ExecutionContext, ExecutionContextFactory};
use crate::native::{NativeRegistry, Tool, ToolFactory};

/// The class `<tool-factory>` names for the notification capture tool.
pub const NOTIFICATION_CAPTURE_CLASS: &str = "moqui_core::notify::capture::NotificationCaptureToolFactory";
/// The captured messages a factory keeps; past it the oldest go.
pub const MAX_CAPTURED: usize = 500;

type Captured = Arc<Mutex<Vec<(i64, Value)>>>;

/// The capture of a factory: its subscription while on, and the messages.
#[derive(Default)]
pub struct NotificationCapture {
    subscription: Mutex<Option<Subscription>>,
    messages: Captured,
}

impl NotificationCapture {
    /// The capture of `factory`, made on first use.
    pub fn of(factory: &ExecutionContextFactory) -> Arc<Self> {
        factory.extension(|_| Self::default())
    }

    pub fn is_enabled(&self) -> bool {
        lock(&self.subscription).is_some()
    }

    /// Listen on the notification topic; a second enable keeps the one listener.
    ///
    /// # Errors
    ///
    /// Returns an error when the topic cannot be subscribed.
    pub fn enable(&self) -> Result<(), Error> {
        let mut subscription = lock(&self.subscription);
        if subscription.is_none() {
            let messages = Arc::clone(&self.messages);
            *subscription = Some(subscribe(
                NOTIFICATION_MESSAGE,
                Arc::new(move |message: &TopicMessage| record(&messages, message)),
            )?);
        }
        Ok(())
    }

    /// Stop listening (dropping the subscription removes the listener); the messages stay until `clear()`.
    pub fn disable(&self) {
        lock(&self.subscription).take();
    }

    pub fn clear(&self) {
        lock(&self.messages).clear();
    }

    /// The captured messages of `topic` (all with `None`), oldest first.
    pub fn messages(&self, topic: Option<&str>) -> Vec<Value> {
        lock(&self.messages)
            .iter()
            .filter(|(_, message)| topic.is_none_or(|name| topic_of(message) == name))
            .map(|(at, message)| with_date(message, *at))
            .collect()
    }
}

fn record(messages: &Captured, message: &TopicMessage) {
    let Value::Map(body) = moqui_lib::json::from_json(&message.body) else {
        return;
    };
    let mut kept = IndexMap::new();
    for key in ["topic", "subTopic", ROUTE_USER_IDS, "title", "link", "message"] {
        kept.insert(key.to_string(), body.get(key).cloned().unwrap_or(Value::Null));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX));
    let mut messages = lock(messages);
    if messages.len() >= MAX_CAPTURED {
        let over = messages.len() + 1 - MAX_CAPTURED;
        messages.drain(..over);
    }
    messages.push((now, Value::Map(kept)));
}

fn topic_of(message: &Value) -> String {
    match message {
        Value::Map(map) => map.get("topic").map(to_plain_string).unwrap_or_default(),
        _ => String::new(),
    }
}

fn with_date(message: &Value, at: i64) -> Value {
    let mut out = match message {
        Value::Map(map) => map.clone(),
        _ => IndexMap::new(),
    };
    out.insert("capturedDate".to_string(), Value::I64(at));
    Value::Map(out)
}

/// The framework's notification capture tool factory, registered with the mail capture.
pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.tool(NOTIFICATION_CAPTURE_CLASS, || Arc::new(NotificationCaptureToolFactory))
}

/// `ec.factory.getTool('NotificationCapture')`.
pub struct NotificationCaptureToolFactory;

impl ToolFactory for NotificationCaptureToolFactory {
    fn name(&self) -> &str {
        "NotificationCapture"
    }

    fn get_instance(&self, _parameters: &[Value]) -> Result<Arc<dyn Tool>, Error> {
        Ok(Arc::new(NotificationCaptureTool))
    }

    fn type_declaration(&self) -> Option<&'static str> {
        Some(
            "{ isEnabled(): boolean; enable(): void; disable(): void; \
             getMessages(topic?: string): { topic: string; subTopic: string | null; userIds: string[]; title: string; \
             link: string; message: Record<string, unknown>; capturedDate: number }[]; count(topic?: string): number; \
             last(topic?: string): Record<string, unknown> | null; clear(): void }",
        )
    }
}

pub struct NotificationCaptureTool;

impl Tool for NotificationCaptureTool {
    fn methods(&self) -> &'static [&'static str] {
        &[
            "isEnabled",
            "enable",
            "disable",
            "getMessages",
            "count",
            "last",
            "clear",
        ]
    }

    fn call(&self, ec: &ExecutionContext, method: &str, args: &[Value]) -> Result<Value, Error> {
        let capture = NotificationCapture::of(ec.factory());
        let topic = args.first().filter(|value| !value.is_null()).map(to_plain_string);
        match method {
            "isEnabled" => Ok(Value::Bool(capture.is_enabled())),
            "enable" => capture.enable().map(|()| Value::Null),
            "disable" => {
                capture.disable();
                Ok(Value::Null)
            }
            "getMessages" => Ok(Value::List(capture.messages(topic.as_deref()))),
            "count" => Ok(Value::I64(
                i64::try_from(capture.messages(topic.as_deref()).len()).unwrap_or(i64::MAX),
            )),
            "last" => Ok(capture.messages(topic.as_deref()).pop().unwrap_or(Value::Null)),
            "clear" => {
                capture.clear();
                Ok(Value::Null)
            }
            other => Err(Error::new(format!(
                "NotificationCapture has no method {other} ({} arguments: {})",
                args.len(),
                args.iter().map(to_plain_string).collect::<Vec<_>>().join(", ")
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::{has_listeners, publish};

    fn message(id: &str, topic: &str) -> TopicMessage {
        TopicMessage {
            origin_id: "test".to_string(),
            message_id: id.to_string(),
            body: serde_json::json!({"topic": topic, "title": "T", "link": "/x", "message": {"n": 1}, "userIds": ["u1"]}),
        }
    }

    #[test]
    fn the_capture_listens_while_on_and_keeps_the_messages_by_topic() {
        let ctx = ExecutionContextFactory::test_context();
        let _entered = ctx.enter();
        let capture = NotificationCapture::of(ctx.factory());
        assert!(!capture.is_enabled());
        capture.enable().unwrap();
        capture.enable().unwrap();
        assert!(capture.is_enabled() && has_listeners(NOTIFICATION_MESSAGE));
        // the bus is the process's: other tests publish too, so this one counts only its own topics
        let unique = crate::entity::uuid_v4().unwrap();
        let (counts_topic, mention_topic) = (format!("counts-{unique}"), format!("mention-{unique}"));
        let mine = |capture: &NotificationCapture| {
            capture.messages(Some(&counts_topic)).len() + capture.messages(Some(&mention_topic)).len()
        };
        publish(NOTIFICATION_MESSAGE, &message(&format!("{unique}-1"), &counts_topic)).unwrap();
        publish(NOTIFICATION_MESSAGE, &message(&format!("{unique}-2"), &mention_topic)).unwrap();
        let counts = capture.messages(Some(&counts_topic));
        assert_eq!(counts.len(), 1);
        let Value::Map(first) = &counts[0] else {
            panic!("not a map")
        };
        assert_eq!(first.get("userIds"), Some(&Value::List(vec![Value::string("u1")])));
        assert_eq!(first.get("title").map(to_plain_string).as_deref(), Some("T"));
        assert!(matches!(first.get("capturedDate"), Some(Value::I64(_))));
        assert_eq!(mine(&capture), 2);
        // off: nothing more is kept, what was kept stays until clear
        capture.disable();
        publish(NOTIFICATION_MESSAGE, &message(&format!("{unique}-3"), &counts_topic)).unwrap();
        assert_eq!(mine(&capture), 2);
        capture.clear();
        assert_eq!(mine(&capture), 0);
    }
}
