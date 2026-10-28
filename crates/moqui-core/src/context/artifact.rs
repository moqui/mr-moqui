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

//! Artifact stack. Each frame is a telemetry span, and its pop records the artifact duration metric. A checked push
//! decides the frame against the one below it.

use std::borrow::Cow;
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Instant, SystemTime};

use moqui_lib::object::Value;

use super::error::Error;
use crate::user::AuthInfo;

struct Frame {
    artifact_type: Cow<'static, str>,
    name: String,
    action: Cow<'static, str>,
    info: AuthInfo,
    telemetry_span: crate::telemetry::Span,
    telemetry_context: crate::telemetry::ContextGuard,
    telemetry_suppression: Option<crate::telemetry::SuppressionGuard>,
    legacy_slow: Option<Box<LegacySlow>>,
    started: Instant,
    hit: HitDetail,
    /// The context's errors when the frame was pushed: an error added since marks the frame's span.
    errors: ErrorsAtPush,
}

/// What a frame needs to tell whether an error was added while it was on the stack, and which one came first.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ErrorsAtPush {
    /// [`super::MessageFacade::errors_added`] at the push.
    pub(super) added: u64,
    /// How many errors the message facade held at the push.
    pub(super) held: usize,
}

/// A deferred legacy hit keeps only bounded frame metadata, without starting or buffering
/// spans for fast frames. Descendants remain children of the nearest recorded parent.
struct LegacySlow {
    parent: Option<crate::telemetry::TraceContext>,
    started: SystemTime,
    error: Option<String>,
    user: Option<String>,
    visit: Option<String>,
}

/// Per-frame suppression and artifact subtype; request/service parameters are not retained.
pub(super) struct HitDetail {
    pub(super) track: bool,
    pub(super) sub_type: Option<Cow<'static, str>>,
}

impl Default for HitDetail {
    fn default() -> Self {
        Self {
            track: true,
            sub_type: None,
        }
    }
}

#[derive(Default)]
pub(super) struct ArtifactStack {
    frames: Vec<Frame>,
    authz_depth: Rc<Cell<u32>>,
    /// Java's `entityEcaDisabled` of the `ArtifactExecutionFacade`: entity ECA rules do not run for this context.
    entity_eca_disabled: Cell<bool>,
    /// Java's `tarpitDisabled` of the `ArtifactExecutionFacade`: no frame of this context is counted against a tarpit.
    tarpit_disabled: Cell<bool>,
    /// The first frame an authorization decision sees: frames below it belong to the render an isolated check runs in.
    check_base: usize,
    /// How many isolated checks are open: their frames ask whether the user may, they are not a use.
    checks: usize,
}

impl Drop for ArtifactStack {
    fn drop(&mut self) {
        // Context guards are nested. Vec's forward element destruction would restore a child
        // after its parent when an exception leaves frames unpopped.
        while let Some(frame) = self.frames.pop() {
            drop(frame);
        }
    }
}

impl ArtifactStack {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Push a frame that has already been decided. `info` is what the decision recorded.
    pub(super) fn push_with(
        &mut self,
        artifact_type: &str,
        name: &str,
        action: &str,
        info: AuthInfo,
        errors: ErrorsAtPush,
    ) {
        let artifact_type = known(artifact_type);
        let name = name.to_string();
        let action = known(action);
        // the calls observed on a dev instance, for the artifact index (plans/design-dev-aide.md 7.1)
        if crate::devtools::index::calls::enabled() {
            if let Some(parent) = self.frames.last() {
                crate::devtools::index::calls::observe(&parent.artifact_type, &parent.name, &artifact_type, &name);
            }
        }
        let telemetry = crate::telemetry::current();
        let legacy_slow = telemetry
            .as_ref()
            .is_some_and(|telemetry| telemetry.legacy_slow_enabled(&artifact_type))
            .then(|| {
                Box::new(LegacySlow {
                    parent: crate::telemetry::capture(),
                    started: SystemTime::now(),
                    error: None,
                    user: None,
                    visit: None,
                })
            });
        let mut telemetry_span = telemetry
            .filter(|telemetry| telemetry.artifact_enabled(&artifact_type).0)
            .map(|telemetry| telemetry.start_artifact_span(&artifact_type, &name, crate::telemetry::SpanKind::Internal))
            .unwrap_or_else(crate::telemetry::Span::noop);
        if telemetry_span.is_recording() {
            telemetry_span.set_attribute("moqui.artifact.type", Value::string(artifact_type.as_ref()));
            telemetry_span.set_attribute("moqui.artifact.name", Value::string(&name));
            telemetry_span.set_attribute("moqui.artifact.action", Value::string(action.as_ref()));
        }
        let telemetry_context = crate::telemetry::attach(telemetry_span.context().or_else(crate::telemetry::capture));
        self.frames.push(Frame {
            artifact_type,
            name,
            action,
            info,
            telemetry_span,
            telemetry_context,
            telemetry_suppression: None,
            legacy_slow,
            started: Instant::now(),
            hit: HitDetail::default(),
            errors,
        });
    }

    /// Info of the top frame, and whether any frame carries a deny, of the frames above the base of an isolated check.
    pub(super) fn authz_state(&self) -> (Option<AuthInfo>, bool) {
        let frames = &self.frames[self.check_base.min(self.frames.len())..];
        let last = frames.last().map(|frame| frame.info.clone());
        let deny = frames
            .iter()
            .any(|frame| frame.info.authz_type.as_deref() == Some("AUTHZT_DENY"));
        (last, deny)
    }

    /// Start an isolated check at the top of the stack, returning the base it replaces.
    pub(super) fn begin_isolated_check(&mut self) -> usize {
        self.checks += 1;
        std::mem::replace(&mut self.check_base, self.frames.len())
    }

    pub(super) fn end_isolated_check(&mut self, previous: usize) {
        self.checks = self.checks.saturating_sub(1);
        self.check_base = previous;
    }

    pub(super) fn in_isolated_check(&self) -> bool {
        self.checks > 0
    }

    /// The stack as `type:name:action`, top first, for a failure message.
    pub(super) fn names(&self) -> String {
        self.frames
            .iter()
            .rev()
            .map(|frame| format!("{}:{}:{}", frame.artifact_type, frame.name, frame.action))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The artifact names, top first, joined with ", ": Java's `getStackNameString`.
    pub(super) fn name_string(&self) -> String {
        self.frames
            .iter()
            .rev()
            .map(|frame| frame.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub(super) fn top_info_mut(&mut self) -> Option<&mut AuthInfo> {
        self.frames.last_mut().map(|frame| &mut frame.info)
    }

    pub(super) fn top_hit_mut(&mut self) -> Option<&mut HitDetail> {
        self.frames.last_mut().map(|frame| &mut frame.hit)
    }

    /// The top frame is not an artifact hit: no span or metric of its own. What runs under it records as children of
    /// its parent.
    pub(super) fn untrack_top(&mut self) {
        if let Some(frame) = self.frames.last_mut() {
            frame.hit.track = false;
            frame.telemetry_span.suppress();
            frame.telemetry_context.restore_parent();
        }
    }

    /// Nothing under the top frame records artifact spans or metrics, until it is popped.
    pub(super) fn suppress_top_descendants(&mut self) {
        if let Some(frame) = self.frames.last_mut() {
            if frame.telemetry_suppression.is_none() {
                frame.telemetry_suppression = Some(crate::telemetry::suppress_artifacts());
            }
        }
    }

    /// The top frame's errors at its push, when its pop records telemetry; `None` when there is nothing to finish.
    pub(super) fn top_telemetry_errors(&self) -> Option<ErrorsAtPush> {
        self.frames
            .last()
            .filter(|frame| frame.hit.track && (frame.telemetry_span.is_recording() || frame.legacy_slow.is_some()))
            .map(|frame| frame.errors)
    }

    pub(super) fn finish_top_telemetry(&mut self, error: Option<&str>, user_id: Option<&str>, visit_id: Option<&str>) {
        if let Some(frame) = self.frames.last_mut() {
            if let Some(legacy) = &mut frame.legacy_slow {
                legacy.error = error.map(|error| error.chars().take(4096).collect());
                legacy.user = user_id.map(str::to_string);
                legacy.visit = visit_id.map(str::to_string);
            }
            if let Some(error) = error {
                frame.telemetry_span.set_error(error);
            }
            if let Some(user_id) = user_id {
                frame.telemetry_span.set_attribute("user.id", Value::string(user_id));
            }
            if let Some(visit_id) = visit_id {
                frame
                    .telemetry_span
                    .set_attribute("moqui.visit.id", Value::string(visit_id));
            }
            if let Some(sub_type) = &frame.hit.sub_type {
                frame
                    .telemetry_span
                    .set_attribute("moqui.artifact.sub_type", Value::string(sub_type.as_ref()));
            }
        }
    }

    /// Pop the top frame and close its telemetry span and metric.
    pub(super) fn pop(&mut self) -> Result<(), Error> {
        let frame = self.frames.pop().ok_or_else(|| Error::new("artifact stack is empty"))?;
        let elapsed = frame.started.elapsed();
        let mut frame = frame;
        if !frame.hit.track {
            frame.telemetry_span.suppress();
        }
        if frame.hit.track {
            if let Some(telemetry) = crate::telemetry::current() {
                let slow = telemetry.record_artifact(
                    &frame.artifact_type,
                    &frame.name,
                    &frame.action,
                    frame.hit.sub_type.as_deref(),
                    elapsed.as_secs_f64(),
                );
                if slow && elapsed > std::time::Duration::from_secs(1) {
                    if let Some(legacy) = frame.legacy_slow.take() {
                        frame.telemetry_span =
                            telemetry.start_legacy_slow_span(&frame.name, legacy.parent, legacy.started);
                        frame
                            .telemetry_span
                            .set_attribute("moqui.artifact.type", Value::string(frame.artifact_type.as_ref()));
                        frame
                            .telemetry_span
                            .set_attribute("moqui.artifact.name", Value::string(&frame.name));
                        frame
                            .telemetry_span
                            .set_attribute("moqui.artifact.action", Value::string(frame.action.as_ref()));
                        if let Some(sub_type) = &frame.hit.sub_type {
                            frame
                                .telemetry_span
                                .set_attribute("moqui.artifact.sub_type", Value::string(sub_type.as_ref()));
                        }
                        if let Some(error) = legacy.error {
                            frame.telemetry_span.set_error(&error);
                        }
                        if let Some(user) = legacy.user {
                            frame.telemetry_span.set_attribute("user.id", Value::string(user));
                        }
                        if let Some(visit) = legacy.visit {
                            frame
                                .telemetry_span
                                .set_attribute("moqui.visit.id", Value::string(visit));
                        }
                    }
                }
                if slow {
                    frame
                        .telemetry_span
                        .set_attribute("moqui.artifact.slow", Value::Bool(true));
                }
                if crate::web::attribute("moqui.telemetry.artifact.name")
                    .as_ref()
                    .and_then(Value::as_str)
                    == Some(frame.name.as_str())
                {
                    crate::web::set_attribute("moqui.telemetry.artifact.slow", Some(Value::Bool(slow)));
                }
            }
        }
        frame.telemetry_span.finish();
        drop(frame.telemetry_context);
        drop(frame.telemetry_suppression);
        Ok(())
    }

    pub(super) fn disable_authz(&self) -> AuthzGuard {
        self.authz_depth.set(self.authz_depth.get().saturating_add(1));
        AuthzGuard {
            depth: Rc::clone(&self.authz_depth),
        }
    }

    /// Disable entity ECA rules; whether they were already disabled, as Java's `disableEntityEca`.
    pub(super) fn disable_entity_eca(&self) -> bool {
        self.entity_eca_disabled.replace(true)
    }

    pub(super) fn enable_entity_eca(&self) {
        self.entity_eca_disabled.set(false);
    }

    pub(super) fn entity_eca_disabled(&self) -> bool {
        self.entity_eca_disabled.get()
    }

    /// Disable tarpit counting; whether it was already disabled, as Java's `disableTarpit`.
    pub(super) fn disable_tarpit(&self) -> bool {
        self.tarpit_disabled.replace(true)
    }

    pub(super) fn enable_tarpit(&self) {
        self.tarpit_disabled.set(false);
    }

    pub(super) fn tarpit_disabled(&self) -> bool {
        self.tarpit_disabled.get()
    }

    pub(super) fn authz_disabled(&self) -> bool {
        self.authz_depth.get() > 0
    }
}

/// Decrements the authorization-disabled depth when dropped.
#[must_use = "dropping the guard re-enables authorization checks"]
pub struct AuthzGuard {
    depth: Rc<Cell<u32>>,
}

impl Drop for AuthzGuard {
    fn drop(&mut self) {
        self.depth.set(self.depth.get().saturating_sub(1));
    }
}

/// The artifact types, authorization actions and hit sub-types the framework pushes, kept as they are rather than copied
/// per frame.
const KNOWN: &[&str] = &[
    "inline",
    "entity-implicit",
    "entity-auto",
    "script",
    "java",
    "one",
    "list",
    "count",
    "iterator",
    "AT_ENTITY",
    "AT_SERVICE",
    "AT_XML_SCREEN",
    "AT_XML_SCREEN_TRANS",
    "AT_XML_SCREEN_CONTENT",
    "AT_REST_PATH",
    "AT_OTHER",
    "AT_LLM",
    "view",
    "create",
    "update",
    "delete",
    "all",
    "AUTHZA_VIEW",
    "AUTHZA_CREATE",
    "AUTHZA_UPDATE",
    "AUTHZA_DELETE",
    "AUTHZA_ALL",
];

pub(super) fn known(text: &str) -> Cow<'static, str> {
    match KNOWN.iter().find(|known| **known == text) {
        Some(known) => Cow::Borrowed(known),
        None => Cow::Owned(text.to_string()),
    }
}

#[cfg(all(test, feature = "telemetry"))]
mod telemetry_tests {
    use super::*;
    #[test]
    fn abandoned_frames_restore_the_context_in_stack_order() {
        let context = crate::context::ExecutionContextFactory::test_context();
        let _entered = context.enter();
        let parent =
            crate::telemetry::TraceContext::extract("00-0123456789abcdef0123456789abcdef-0123456789abcdef-01", "")
                .unwrap();
        let _trace = crate::telemetry::attach(Some(parent.clone()));
        {
            let mut stack = ArtifactStack::new();
            stack.push_with(
                "AT_OTHER",
                "outer",
                "view",
                AuthInfo::default(),
                ErrorsAtPush::default(),
            );
            stack.push_with(
                "AT_OTHER",
                "inner",
                "view",
                AuthInfo::default(),
                ErrorsAtPush::default(),
            );
            assert_ne!(crate::telemetry::capture(), Some(parent.clone()));
        }
        assert_eq!(crate::telemetry::capture(), Some(parent));
    }
    #[test]
    fn deprecated_bin_only_types_retain_only_impactful_slow_evidence() {
        use crate::{
            entity::document::Document,
            telemetry::{Exporter, SpanKind},
        };
        use std::sync::{Arc, Mutex};
        use std::time::Duration;
        #[derive(Default)]
        struct Memory(Mutex<Vec<Document>>);
        impl Exporter for Memory {
            fn export(&self, index: &str, documents: Vec<Document>) -> Result<usize, moqui_lib::error::Error> {
                let count = documents.len();
                if index == "otel_spans" {
                    self.0.lock().unwrap().extend(documents);
                }
                Ok(count)
            }
        }
        for explicit_disable in [false, true] {
            let override_xml = if explicit_disable {
                r#"<artifact type="AT_SERVICE" spans="false" metrics="true"/>"#
            } else {
                ""
            };
            let conf = format!(
                r#"<moqui-conf><server-stats><artifact-stats type="AT_SERVICE" persist-hit="false" persist-bin="true"/></server-stats><telemetry sample-ratio="0">{override_xml}</telemetry></moqui-conf>"#
            );
            let context = crate::context::ExecutionContextFactory::test_context_with_conf(&conf);
            let _entered = context.enter();
            let telemetry = crate::telemetry::for_factory(context.factory());
            let memory = Arc::new(Memory::default());
            telemetry.set_exporter(memory.clone());
            let root = telemetry.start_span("legacy.request", SpanKind::Server, None);
            let parent = root.context().unwrap();
            let _trace = crate::telemetry::attach(Some(parent.clone()));
            let mut stack = ArtifactStack::new();
            let mut record = |duration: Duration| {
                stack.push_with(
                    "AT_SERVICE",
                    "legacy.service",
                    "view",
                    AuthInfo::default(),
                    ErrorsAtPush::default(),
                );
                assert_eq!(
                    crate::telemetry::capture(),
                    Some(parent.clone()),
                    "fast legacy candidates must preserve their parent's context"
                );
                let frame = stack.frames.last_mut().unwrap();
                frame.started = Instant::now().checked_sub(duration).unwrap();
                frame.hit.sub_type = Some(known("inline"));
                if let Some(legacy) = &mut frame.legacy_slow {
                    legacy.started = SystemTime::now().checked_sub(duration).unwrap();
                }
                stack.finish_top_telemetry(None, Some("legacy-user"), Some("legacy-visit"));
                stack.pop().unwrap();
            };
            for _ in 0..100 {
                record(Duration::from_millis(100));
            }
            record(Duration::from_millis(1500));
            record(Duration::from_millis(900)); // statistically slow, below the legacy impact minimum
            record(Duration::from_millis(100));
            assert_eq!(crate::telemetry::capture(), Some(parent.clone()));
            root.finish();
            telemetry.flush();
            let documents = memory.0.lock().unwrap();
            let slow: Vec<_> = documents
                .iter()
                .filter(|document| {
                    document.get("moqui.artifact.name").and_then(Value::as_str) == Some("legacy.service")
                })
                .collect();
            assert_eq!(
                slow.len(),
                usize::from(!explicit_disable),
                "explicit telemetry span disabling must override the deprecated fallback"
            );
            if let Some(slow) = slow.first() {
                assert_eq!(
                    slow.get("parent_span_id").and_then(Value::as_str),
                    Some(parent.span_id.as_str())
                );
                assert_eq!(slow.get("moqui.artifact.slow"), Some(&Value::Bool(true)));
                assert_eq!(
                    slow.get("moqui.artifact.sub_type").and_then(Value::as_str),
                    Some("inline")
                );
                assert_eq!(slow.get("user.id").and_then(Value::as_str), Some("legacy-user"));
                assert_eq!(slow.get("moqui.visit.id").and_then(Value::as_str), Some("legacy-visit"));
                assert!(matches!(slow.get("duration_nano"),Some(Value::I64(duration))if *duration>=1_500_000_000));
            }
            drop(documents);
            telemetry.shutdown();
        }
    }
}
