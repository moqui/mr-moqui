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

//! Artifact stack. Push and pop are tracing spans. A checked push decides the frame against the one below it.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Instant;

use moqui_lib::object::Value;

use super::error::Error;
use crate::user::AuthInfo;

struct Frame {
    artifact_type: String,
    name: String,
    action: String,
    info: AuthInfo,
    entered: tracing::span::EnteredSpan,
    started: Instant,
    /// Wall-clock start in epoch milliseconds, the hit's `startDateTime`.
    started_ms: i64,
    hit: HitDetail,
}

/// What the hit of a frame records beyond the span fields. Java's `trackArtifactHit`, the
/// `actionDetail` that is the hit sub-type, and the remembered parameters.
pub(super) struct HitDetail {
    pub(super) track: bool,
    pub(super) sub_type: Option<String>,
    pub(super) parameters: Option<BTreeMap<String, Value>>,
}

impl Default for HitDetail {
    fn default() -> Self {
        Self {
            track: true,
            sub_type: None,
            parameters: None,
        }
    }
}

/// A frame that was popped, for the hit counter.
pub(super) struct Popped {
    pub(super) artifact_type: String,
    pub(super) name: String,
    pub(super) started_ms: i64,
    pub(super) running_ms: f64,
    pub(super) hit: HitDetail,
}

#[derive(Default)]
pub(super) struct ArtifactStack {
    frames: Vec<Frame>,
    authz_depth: Rc<Cell<u32>>,
    /// Java's `entityEcaDisabled` of the `ArtifactExecutionFacade`: entity ECA rules do not run for this context.
    entity_eca_disabled: Cell<bool>,
}

impl ArtifactStack {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Push a frame that has already been decided. `info` is what the decision recorded.
    pub(super) fn push_with(&mut self, artifact_type: &str, name: &str, action: &str, info: AuthInfo) {
        let artifact_type = artifact_type.to_string();
        let name = name.to_string();
        let action = action.to_string();
        crate::deterministic::trace(&artifact_type, &action, &name, "enter")
            .expect("deterministic artifact trace failed");
        let entered = tracing::info_span!(
            "artifact",
            artifact_type = %artifact_type,
            name = %name,
            action = %action,
            duration_ms = tracing::field::Empty,
        )
        .entered();
        self.frames.push(Frame {
            artifact_type,
            name,
            action,
            info,
            entered,
            started: Instant::now(),
            started_ms: crate::user::now_millis(),
            hit: HitDetail::default(),
        });
    }

    /// Info of the top frame, and whether any frame carries a deny.
    pub(super) fn authz_state(&self) -> (Option<AuthInfo>, bool) {
        let last = self.frames.last().map(|frame| frame.info.clone());
        let deny = self
            .frames
            .iter()
            .any(|frame| frame.info.authz_type.as_deref() == Some("AUTHZT_DENY"));
        (last, deny)
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

    pub(super) fn deterministic_metadata(&self) -> Vec<serde_json::Value> {
        self.frames
            .iter()
            .rev()
            .map(|frame| {
                serde_json::json!({
                    "type": frame.artifact_type,
                    "sub_type": frame.hit.sub_type.as_deref().unwrap_or(""),
                    "name": frame.name,
                })
            })
            .collect()
    }

    pub(super) fn top_type(&self) -> Option<String> {
        self.frames.last().map(|frame| frame.artifact_type.clone())
    }

    pub(super) fn top_hit_mut(&mut self) -> Option<&mut HitDetail> {
        self.frames.last_mut().map(|frame| &mut frame.hit)
    }

    /// Pop the top frame and close its span. What the span recorded is returned for the hit counter.
    pub(super) fn pop(&mut self) -> Result<Popped, Error> {
        let frame = self.frames.pop().ok_or_else(|| Error::new("artifact stack is empty"))?;
        crate::deterministic::trace(
            &frame.artifact_type,
            frame.hit.sub_type.as_deref().unwrap_or(&frame.action),
            &frame.name,
            "exit",
        )?;
        let elapsed = frame.started.elapsed();
        let duration_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
        frame.entered.record("duration_ms", duration_ms);
        let Frame {
            artifact_type,
            name,
            entered,
            started_ms,
            hit,
            ..
        } = frame;
        drop(entered);
        Ok(Popped {
            artifact_type,
            name,
            started_ms,
            running_ms: elapsed.as_secs_f64() * 1000.0,
            hit,
        })
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
