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

//! The mail capture, for specs (no Java counterpart; docs/Build-Test.md, "Mail"). With the conf property
//! `mail_capture_sends=true` (`mail_capture_sends=true ./moq test ...`) or the tool enabled by a spec, a send of
//! `send#EmailMessage` (and so of `send#EmailTemplate` and the mail client's sends) does everything a real send does,
//! the `EmailMessage` row and its `ES_SENT` status included, except connect: the message is kept in memory, where
//! `ec.factory.getTool('MailCapture')` reads and clears it. The most recent [`MAX_CAPTURED`] are kept.
//!
//! The tool: `isEnabled()`, `enable()`, `disable()` (this factory, until `reset()`, which goes back to the conf
//! property), `getMessages()` (oldest first, each `{subject, fromAddress, fromName, toList, ccList, bccList,
//! replyToList, body, bodyText, messageId, inReplyTo, references, attachmentNames, headers, capturedDate}`), `count()`,
//! `last()` and `clear()`.

use std::sync::{Arc, Mutex};

use indexmap::IndexMap;
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;

use crate::context::{Error, ExecutionContext, ExecutionContextFactory};
use crate::native::{NativeRegistry, Tool, ToolFactory};

/// The class `<tool-factory>` names for the mail capture tool.
pub const MAIL_CAPTURE_CLASS: &str = "moqui_core::service::mail_capture::MailCaptureToolFactory";
/// The conf property that turns the capture on for a factory with no `enable()`/`disable()` of its own.
pub const MAIL_CAPTURE_PROPERTY: &str = "mail_capture_sends";
/// The captured messages a factory keeps; past it the oldest go.
pub const MAX_CAPTURED: usize = 500;

/// One message a send would have sent.
#[derive(Clone, Debug, Default)]
pub struct CapturedMail {
    pub subject: String,
    pub from_address: String,
    pub from_name: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub reply_to: Vec<String>,
    pub body: String,
    pub body_text: String,
    pub message_id: String,
    pub in_reply_to: String,
    pub references: String,
    pub attachment_names: Vec<String>,
    /// The headers the send added beside its own fields (a list email's List-Unsubscribe), in order.
    pub headers: Vec<(String, String)>,
}

/// The capture of a factory: whether a script turned it on or off, and the messages.
#[derive(Default)]
pub struct MailCapture {
    enabled: Mutex<Option<bool>>,
    messages: Mutex<Vec<(i64, CapturedMail)>>,
}

impl MailCapture {
    /// The capture of `factory`, made on first use.
    pub fn of(factory: &ExecutionContextFactory) -> Arc<Self> {
        factory.extension(|_| Self::default())
    }

    /// Whether sends are captured: the factory's `enable()`/`disable()`, else the conf property.
    pub fn is_enabled(&self) -> bool {
        decide(*lock(&self.enabled), || {
            moqui_lib::env::get_prop_or_env(MAIL_CAPTURE_PROPERTY)
        })
    }

    /// Turn the capture on or off for this factory; `None` goes back to the conf property.
    pub fn set_enabled(&self, enabled: Option<bool>) {
        *lock(&self.enabled) = enabled;
    }

    pub fn record(&self, mail: CapturedMail) {
        let mut messages = lock(&self.messages);
        if messages.len() >= MAX_CAPTURED {
            let over = messages.len() + 1 - MAX_CAPTURED;
            messages.drain(..over);
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX));
        messages.push((now, mail));
    }

    pub fn messages(&self) -> Vec<CapturedMail> {
        lock(&self.messages).iter().map(|(_, mail)| mail.clone()).collect()
    }

    pub fn clear(&self) {
        lock(&self.messages).clear();
    }

    fn messages_value(&self) -> Vec<Value> {
        lock(&self.messages)
            .iter()
            .map(|(at, mail)| mail_value(*at, mail))
            .collect()
    }
}

/// A factory's own switch, else the conf property's text.
fn decide(own: Option<bool>, property: impl FnOnce() -> String) -> bool {
    own.unwrap_or_else(|| property().trim() == "true")
}

/// Whether a send now is captured: the capture of the current execution context's factory is on.
pub fn enabled() -> bool {
    ExecutionContext::with_current(|ec| MailCapture::of(ec.factory()).is_enabled()).unwrap_or(false)
}

/// Keep `mail` in the capture of the current execution context's factory.
pub fn record(mail: CapturedMail) {
    ExecutionContext::with_current(|ec| MailCapture::of(ec.factory()).record(mail));
}

fn list(items: &[String]) -> Value {
    Value::List(items.iter().map(Value::string).collect())
}

fn mail_value(at: i64, mail: &CapturedMail) -> Value {
    let mut out = IndexMap::new();
    for (key, value) in [
        ("subject", Value::string(&mail.subject)),
        ("fromAddress", Value::string(&mail.from_address)),
        ("fromName", Value::string(&mail.from_name)),
        ("toList", list(&mail.to)),
        ("ccList", list(&mail.cc)),
        ("bccList", list(&mail.bcc)),
        ("replyToList", list(&mail.reply_to)),
        ("body", Value::string(&mail.body)),
        ("bodyText", Value::string(&mail.body_text)),
        ("messageId", Value::string(&mail.message_id)),
        ("inReplyTo", Value::string(&mail.in_reply_to)),
        ("references", Value::string(&mail.references)),
        ("attachmentNames", list(&mail.attachment_names)),
        (
            "headers",
            Value::Map(
                mail.headers
                    .iter()
                    .map(|(name, value)| (name.clone(), Value::string(value)))
                    .collect(),
            ),
        ),
        ("capturedDate", Value::I64(at)),
    ] {
        out.insert(key.to_string(), value);
    }
    Value::Map(out)
}

/// The framework's mail capture tool factory, registered with the VCard and ICalendar ones.
pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.tool(MAIL_CAPTURE_CLASS, || Arc::new(MailCaptureToolFactory))
}

/// `ec.factory.getTool('MailCapture')`.
pub struct MailCaptureToolFactory;

impl ToolFactory for MailCaptureToolFactory {
    fn name(&self) -> &str {
        "MailCapture"
    }

    fn get_instance(&self, _parameters: &[Value]) -> Result<Arc<dyn Tool>, Error> {
        Ok(Arc::new(MailCaptureTool))
    }

    fn type_declaration(&self) -> Option<&'static str> {
        Some(
            "{ isEnabled(): boolean; enable(): void; disable(): void; reset(): void; \
             getMessages(): { subject: string; fromAddress: string; fromName: string; toList: string[]; ccList: string[]; \
             bccList: string[]; replyToList: string[]; body: string; bodyText: string; messageId: string; inReplyTo: string; \
             references: string; attachmentNames: string[]; headers: Record<string, string>; capturedDate: number }[]; \
             count(): number; \
             last(): Record<string, unknown> | null; clear(): void }",
        )
    }
}

pub struct MailCaptureTool;

impl Tool for MailCaptureTool {
    fn methods(&self) -> &'static [&'static str] {
        &[
            "isEnabled",
            "enable",
            "disable",
            "reset",
            "getMessages",
            "count",
            "last",
            "clear",
        ]
    }

    fn call(&self, ec: &ExecutionContext, method: &str, args: &[Value]) -> Result<Value, Error> {
        let capture = MailCapture::of(ec.factory());
        match method {
            "isEnabled" => Ok(Value::Bool(capture.is_enabled())),
            "enable" => {
                capture.set_enabled(Some(true));
                Ok(Value::Null)
            }
            "disable" => {
                capture.set_enabled(Some(false));
                Ok(Value::Null)
            }
            "reset" => {
                capture.set_enabled(None);
                Ok(Value::Null)
            }
            "getMessages" => Ok(Value::List(capture.messages_value())),
            "count" => Ok(Value::I64(i64::try_from(capture.messages().len()).unwrap_or(i64::MAX))),
            "last" => Ok(capture.messages_value().pop().unwrap_or(Value::Null)),
            "clear" => {
                capture.clear();
                Ok(Value::Null)
            }
            other => Err(Error::new(format!(
                "MailCapture has no method {other} ({} arguments: {})",
                args.len(),
                args.iter().map(to_plain_string).collect::<Vec<_>>().join(", ")
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail(subject: &str) -> CapturedMail {
        CapturedMail {
            subject: subject.to_string(),
            to: vec!["a@example.com".to_string()],
            ..CapturedMail::default()
        }
    }

    #[test]
    fn the_capture_follows_its_switch_and_keeps_the_latest() {
        let ctx = ExecutionContextFactory::test_context();
        let _entered = ctx.enter();
        let capture = MailCapture::of(ctx.factory());
        // the conf property, unless the factory has its own switch (the property is not set here: another test's
        // send would be captured)
        assert!(decide(None, || "true".to_string()) && !decide(None, String::new) && !decide(None, || "Y".to_string()));
        assert!(!decide(Some(false), || "true".to_string()) && decide(Some(true), String::new));
        capture.set_enabled(Some(true));
        assert!(enabled());
        capture.set_enabled(Some(false));
        assert!(!enabled());
        capture.set_enabled(None);
        for index in 0..MAX_CAPTURED + 2 {
            record(mail(&format!("m{index}")));
        }
        let kept = capture.messages();
        assert_eq!(kept.len(), MAX_CAPTURED);
        assert_eq!(kept[0].subject, "m2");
        let tool = MailCaptureTool;
        let Value::Map(last) = tool.call(&ctx, "last", &[]).unwrap() else {
            panic!()
        };
        assert_eq!(
            last.get("subject"),
            Some(&Value::string(format!("m{}", MAX_CAPTURED + 1)))
        );
        assert_eq!(
            last.get("toList"),
            Some(&Value::List(vec![Value::string("a@example.com")]))
        );
        tool.call(&ctx, "clear", &[]).unwrap();
        assert_eq!(tool.call(&ctx, "count", &[]).unwrap(), Value::I64(0));
        tool.call(&ctx, "enable", &[]).unwrap();
        assert_eq!(tool.call(&ctx, "isEnabled", &[]).unwrap(), Value::Bool(true));
        tool.call(&ctx, "disable", &[]).unwrap();
        assert_eq!(tool.call(&ctx, "isEnabled", &[]).unwrap(), Value::Bool(false));
        tool.call(&ctx, "reset", &[]).unwrap();
    }
}
