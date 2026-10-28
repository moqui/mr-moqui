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

//! `org.moqui.devaide.ChangeSetServices`: an agent's change set (plans/design-dev-aide.md 13.3) as its owner reviews it.
//! `get#ChangeSet` gives the Workspace each file's text on disk and as proposed, for the inline merge of Review;
//! `complete#Review` resolves the hand-off with what the user did once their saves are in. The Preview of a change set
//! is a render of the page with its proposed text standing at the files' locations: the screen overlay's provider
//! ([`preview_overlay`]) answers `approval:<approvalId>` tokens of the current user's own pending change sets.

use std::sync::Arc;

use moqui_core::context::{Error, ExecutionContext, ExecutionContextFactory};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_core::screen::overlay::Overlay;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use moqui_llm::approval::{self, ApprovalStatus, TargetKind};
use serde_json::{json, Value as Json};

use crate::agent::propose::{prepare, ChangeSet};
use crate::tier::Tier;
use crate::workspace::{json_value, sha256, text, Context};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    moqui_core::screen::overlay::set_provider(Box::new(preview_overlay));
    registry.service_function(moqui_core::service_location!(get_change_set), get_change_set)?;
    registry.service_function(moqui_core::service_location!(complete_review), complete_review)
}

/// The approval `approval_id` when it is the current user's change set.
fn owned(context: &Context, approval_id: &str) -> Result<approval::ApprovalView, LibError> {
    let user_id = moqui_core::user::current_user_id()?.unwrap_or_default();
    let view = approval::load(&context.facade, approval_id)?
        .filter(|view| !user_id.is_empty() && view.user_id == user_id && view.target_kind == TargetKind::File)
        .ok_or_else(|| LibError::new(format!("no change set {approval_id} of yours")))?;
    Ok(view)
}

/// The overlay of a preview token: `approval:<approvalId>`, a pending or handed-off change set of the current user, or
/// `buffer:<id>`, a Design frame's canvas buffer.
fn preview_overlay(factory: &Arc<ExecutionContextFactory>, token: &str) -> Option<Overlay> {
    // the screen builder's Design frame: the user's unsaved texts (plans/design-dev-aide.md 11.2)
    if let Some(id) = token.strip_prefix("buffer:") {
        return crate::screens::buffer_overlay(factory, id);
    }
    let approval_id = token.strip_prefix("approval:")?;
    ExecutionContext::with_current(|ec| {
        let context = Context::of(ec, Tier::View).ok()?;
        let view = owned(&context, approval_id).ok()?;
        if !matches!(view.status, ApprovalStatus::Pending | ApprovalStatus::HandedOff) {
            return None;
        }
        let set: ChangeSet = serde_json::from_value(view.args.clone()).ok()?;
        let prepared = prepare(&context, &set).ok()?;
        let mut overlay = Overlay::new();
        for file in &prepared {
            overlay.insert_location(factory, &file.location, &file.new_text);
        }
        Some(overlay)
    })
    .flatten()
}

/// `get#ChangeSet`: a change set of the current user with each file's text on disk and as proposed, the card's rows,
/// and whether a file changed on disk since the proposal (its proposal then applies to the text now, or conflicts).
fn get_change_set(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let approval_id = text(parameters, "approvalId");
    let view = owned(&context, &approval_id)?;
    let set: ChangeSet = serde_json::from_value(view.args.clone())
        .map_err(|err| LibError::new(format!("the change set does not read: {err}")))?;
    let mut files: Vec<Json> = Vec::new();
    for change in &set.files {
        let one = ChangeSet {
            summary: String::new(),
            target: set.target.clone(),
            files: vec![change.clone()],
        };
        match prepare(&context, &one) {
            Ok(prepared) => {
                let file = &prepared[0];
                let disk = file
                    .current
                    .as_deref()
                    .map(|text| sha256(text.as_bytes()))
                    .unwrap_or_default();
                files.push(json!({
                    "file": file.named.shown, "rootId": file.named.root.id, "path": file.named.relative,
                    "diskText": file.current, "proposedText": file.new_text, "baseHash": change.base_hash,
                    "diskHash": disk, "changedOnDisk": disk != change.base_hash, "new": file.current.is_none(),
                }));
            }
            Err(error) => files.push(json!({"file": change.file, "conflict": error.instruction})),
        }
    }
    let mut out = Parameters::new();
    out.insert("approvalId".into(), Value::string(&approval_id));
    out.insert("status".into(), Value::string(view.status.as_str()));
    out.insert("summary".into(), Value::string(view.summary()));
    out.insert(
        "changeSet".into(),
        json_value(&view.snapshot.get("changeSet").cloned().unwrap_or(Json::Null)),
    );
    out.insert("files".into(), json_value(&files));
    Ok(out)
}

/// `complete#Review`: the user saved the change set from the editor's Review; the hand-off resolves as executed with
/// what they did, which the agent's run reads ("accepted 3 of 4 hunks; edited OrderServices.xml").
fn complete_review(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let approval_id = text(parameters, "approvalId");
    let view = owned(&context, &approval_id)?;
    let accepted = text(parameters, "accepted").parse::<u32>().unwrap_or(0);
    let total = text(parameters, "total").parse::<u32>().unwrap_or(0);
    let edited: Vec<String> = text(parameters, "edited")
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    let mut review = format!("accepted {accepted} of {total} hunks");
    if !edited.is_empty() {
        review.push_str(&format!("; edited {}", edited.join(", ")));
    }
    let saved: Vec<String> = text(parameters, "saved")
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    if !saved.is_empty() {
        review.push_str(&format!("; saved {}", saved.join(", ")));
    }
    // on a thread of its own: the hand-off ends in a transaction that commits before the run resumes on it, whatever
    // transaction this request is in
    let detail = json!({"accepted": accepted, "total": total, "edited": edited, "saved": saved});
    let done = std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let own = context.factory.new_context();
                let _entered = own.enter();
                let _authz = own.disable_authz();
                approval::handoff::complete(&context.factory, &view.approval_id, &view.user_id, &review, &detail)
            })
            .join()
            .unwrap_or_else(|_| Err(LibError::new("the review could not be completed")))
    })?;
    let mut out = Parameters::new();
    out.insert("completed".into(), Value::Bool(done));
    out.insert("review".into(), Value::string(review));
    Ok(out)
}
