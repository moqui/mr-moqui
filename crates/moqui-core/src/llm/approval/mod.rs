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

//! Approvals (plans/LLM-A2A-Assist.md 4.8, D3 and D4): every write an agent makes is an `AgentApproval` the server
//! issues, bound to the user, the agent, the tool and the canonical hash of exactly what runs, single use and
//! expiring, and decided only by its user in an interactive session.
//!
//! A tool that writes (a held screen save, a service tool, a call to another system) does its precheck (the
//! arguments, the user's permission, the agent's role) and its snapshot, then [`request`]s the approval: the row is
//! written Pending, `approval.requested` is stored and the run pauses ([`Requested::Pause`]). A background agent whose
//! policy rule says `policy` is approved at once ([`Requested::Approved`], `decisionChannel` policy). The user decides
//! through [`decide`] with a [`Decider`] made from the request in progress ([`Decider::from_request`]: a browser
//! session with its token, or the terminal; never a header-authenticated request, an agent's screen session,
//! `agent-apps` or internal code): Approve, Reject with feedback, or Edit in screen, a hand-off to the user's own Save
//! ([`handoff`]). The decision is input for the run, which resumes the paused call: the tool reads it ([`for_call`],
//! [`resumed`]), checks for conflicts, moves the approval to Executing with the hash recomputed from the arguments it is
//! about to run ([`begin`]), runs it and records the result ([`finish`]). Status moves are conditional, so an approval
//! runs at most once. [`preview`] runs a write in the sim overlay on demand, or always for a tool configured so.

pub mod handoff;
pub mod preview;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_db::sql::Condition;
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};

use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::{fence, EntityFacade, EntityValue};
use crate::llm::prompt::Surface;
use crate::llm::run::context::TurnContext;
use crate::llm::run::{self, in_tx, quietly, EventSink, Hold};
use crate::llm::tools::{ApprovalMode, Classification, Pause, PauseKind};
use crate::llm::{millis_of, text_of};

pub(crate) const APPROVAL: &str = "moqui.agent.AgentApproval";
pub(crate) const HANDOFF: &str = "moqui.agent.AgentHandoff";
const TOOL_CALL: &str = "moqui.agent.AgentToolCall";

/// An approval's status: its `StatusItem` is [`ApprovalStatus::status_id`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Executing,
    Executed,
    Failed,
    Rejected,
    Expired,
    Conflict,
    HandedOff,
}

impl ApprovalStatus {
    pub const ALL: [ApprovalStatus; 9] = [
        ApprovalStatus::Pending,
        ApprovalStatus::Approved,
        ApprovalStatus::Executing,
        ApprovalStatus::Executed,
        ApprovalStatus::Failed,
        ApprovalStatus::Rejected,
        ApprovalStatus::Expired,
        ApprovalStatus::Conflict,
        ApprovalStatus::HandedOff,
    ];

    pub fn status_id(self) -> &'static str {
        match self {
            ApprovalStatus::Pending => "AgaPending",
            ApprovalStatus::Approved => "AgaApproved",
            ApprovalStatus::Executing => "AgaExecuting",
            ApprovalStatus::Executed => "AgaExecuted",
            ApprovalStatus::Failed => "AgaFailed",
            ApprovalStatus::Rejected => "AgaRejected",
            ApprovalStatus::Expired => "AgaExpired",
            ApprovalStatus::Conflict => "AgaConflict",
            ApprovalStatus::HandedOff => "AgaHandedOff",
        }
    }

    /// The `statusCode`, as the events and the cards name it.
    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalStatus::Pending => "pending",
            ApprovalStatus::Approved => "approved",
            ApprovalStatus::Executing => "executing",
            ApprovalStatus::Executed => "executed",
            ApprovalStatus::Failed => "failed",
            ApprovalStatus::Rejected => "rejected",
            ApprovalStatus::Expired => "expired",
            ApprovalStatus::Conflict => "conflict",
            ApprovalStatus::HandedOff => "handed-off",
        }
    }

    pub fn from_status_id(status_id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.status_id() == status_id)
    }

    pub fn can_move_to(self, to: ApprovalStatus) -> bool {
        TRANSITIONS.contains(&(self, to))
    }
}

/// Every allowed move, the `StatusFlowTransition` rows of `data/AgentTypeData.xml` (a test compares the two). Approved
/// goes to Failed when the arguments about to run do not hash to the approved ones, and to Expired when its run ends
/// before it runs.
pub const TRANSITIONS: &[(ApprovalStatus, ApprovalStatus)] = &[
    (ApprovalStatus::Pending, ApprovalStatus::Approved),
    (ApprovalStatus::Pending, ApprovalStatus::Rejected),
    (ApprovalStatus::Pending, ApprovalStatus::Expired),
    (ApprovalStatus::Pending, ApprovalStatus::Conflict),
    (ApprovalStatus::Pending, ApprovalStatus::HandedOff),
    (ApprovalStatus::Approved, ApprovalStatus::Executing),
    (ApprovalStatus::Approved, ApprovalStatus::Conflict),
    (ApprovalStatus::Approved, ApprovalStatus::Failed),
    (ApprovalStatus::Approved, ApprovalStatus::Expired),
    (ApprovalStatus::Executing, ApprovalStatus::Executed),
    (ApprovalStatus::Executing, ApprovalStatus::Failed),
    (ApprovalStatus::HandedOff, ApprovalStatus::Executed),
    (ApprovalStatus::HandedOff, ApprovalStatus::Expired),
];

/// What an approval runs (`targetKind`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TargetKind {
    /// A screen transition the gate held: the arguments are a [`crate::llm::screen::PendingWrite`].
    #[default]
    Screen,
    /// A service: the arguments are its parameters.
    Service,
    /// Any other tool call (another system's tool, an add-on tool): the arguments are the call's.
    Tool,
}

impl TargetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetKind::Screen => "screen",
            TargetKind::Service => "service",
            TargetKind::Tool => "tool",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [TargetKind::Screen, TargetKind::Service, TargetKind::Tool]
            .into_iter()
            .find(|kind| kind.as_str() == text.trim())
    }
}

/// One row of the card: a field's current value against the proposed one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeRow {
    /// The request or parameter name.
    pub field: String,
    pub label: String,
    pub current: String,
    pub proposed: String,
}

/// Where an Edit in screen takes the user: the screen and the form the write came from, and the transition whose save
/// the hand-off watches for ([`handoff`]). The values filled are the request's changes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffTarget {
    /// The screen path the form is on, and its parameters.
    pub path: String,
    #[serde(default)]
    pub parameters: BTreeMap<String, String>,
    /// The qvt `domId` of the form.
    #[serde(default)]
    pub form: String,
    /// The transition's screen location and name, as the observer sees them.
    pub screen_location: String,
    pub transition: String,
    /// The transition's extra path (`…/updateOrder/ORD1`): a save of another record is not this one.
    #[serde(default)]
    pub extra_path: Vec<String>,
    /// The request names the outcome reports, the values the user saved.
    #[serde(default)]
    pub fields: Vec<String>,
}

/// Whether the write's effects are run in the sim before the user decides (`AgentToolExposure.previewMode`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreviewMode {
    /// Never: the card has no Preview effects.
    None,
    /// On the card's Preview effects button.
    #[default]
    OnDemand,
    /// Before the card is shown (a destructive or bulk tool).
    Always,
}

impl PreviewMode {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "none" => Some(PreviewMode::None),
            "on-demand" | "" => Some(PreviewMode::OnDemand),
            "always" => Some(PreviewMode::Always),
            _ => None,
        }
    }
}

/// What a tool asks the user to approve, after its precheck.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApprovalRequest {
    pub target_kind: TargetKind,
    /// The service name, or `<screen location>#<transition>`, or the tool's own target.
    pub target_name: String,
    /// WRITE, DESTRUCTIVE or EXTERNAL; a READ tool's held write is a WRITE.
    pub classification: Classification,
    /// Exactly what runs: the arguments the hash covers and the executor runs.
    pub args: Json,
    /// One line for the card and the model: what it does to what.
    pub summary: String,
    /// The snapshot rows: each changed field, current against proposed.
    pub changes: Vec<ChangeRow>,
    /// The rest of the snapshot the conflict check compares: a record, its key and `lastUpdatedStamp`.
    pub snapshot: Json,
    /// The records it changes, for the card's chips.
    pub record_refs: Json,
    pub preview: PreviewMode,
    /// Edit in screen is offered: where it takes the user.
    pub handoff: Option<HandoffTarget>,
}

/// What [`request`] came to.
#[derive(Clone, Debug, PartialEq)]
pub enum Requested {
    /// The run pauses for the user's decision: the tool returns it as [`crate::llm::tools::ToolOutcome::Pause`].
    Pause(Pause),
    /// A background agent's policy approved it: the tool runs it now, through [`begin`] with the grant's id.
    Approved(Grant),
    /// It cannot be approved here (the agent may not write, nobody can be asked, the row could not be written): the
    /// model reads why, and nothing was changed.
    Refused(String),
}

/// An approval that may run: its id and the hash of what was approved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grant {
    pub approval_id: String,
    pub args_hash: String,
}

/// An approval as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct ApprovalView {
    pub approval_id: String,
    pub run_id: String,
    pub tool_call_id: String,
    pub user_id: String,
    pub agent_id: String,
    pub classification: Classification,
    pub target_kind: TargetKind,
    pub target_name: String,
    pub args: Json,
    pub args_hash: String,
    /// `{summary, changes, record, handoff?, edited?}`.
    pub snapshot: Json,
    pub record_refs: Json,
    pub preview: Option<Json>,
    pub status: ApprovalStatus,
    pub decision_user_id: Option<String>,
    pub decision_channel: Option<String>,
    pub feedback: Option<String>,
    /// Epoch milliseconds.
    pub expires: Option<i64>,
    pub created: Option<i64>,
    pub result: Option<Json>,
    pub policy: Option<String>,
}

impl ApprovalView {
    pub fn of(row: &EntityValue) -> Self {
        let json_of = |field: &str| text_of(row, field).and_then(|text| serde_json::from_str::<Json>(&text).ok());
        Self {
            approval_id: text_of(row, "approvalId").unwrap_or_default(),
            run_id: text_of(row, "runId").unwrap_or_default(),
            tool_call_id: text_of(row, "toolCallId").unwrap_or_default(),
            user_id: text_of(row, "userId").unwrap_or_default(),
            agent_id: text_of(row, "agentId").unwrap_or_default(),
            classification: text_of(row, "classificationEnumId")
                .and_then(|class| Classification::parse(&class))
                .unwrap_or(Classification::Write),
            target_kind: text_of(row, "targetKind")
                .and_then(|kind| TargetKind::parse(&kind))
                .unwrap_or_default(),
            target_name: text_of(row, "targetName").unwrap_or_default(),
            args: json_of("argumentsJson").unwrap_or(Json::Null),
            args_hash: text_of(row, "argumentsHash").unwrap_or_default(),
            snapshot: json_of("snapshotJson").unwrap_or_else(|| json!({})),
            record_refs: json_of("recordRefsJson").unwrap_or(Json::Null),
            preview: json_of("previewJson"),
            status: text_of(row, "statusId")
                .and_then(|status| ApprovalStatus::from_status_id(&status))
                .unwrap_or(ApprovalStatus::Pending),
            decision_user_id: text_of(row, "decisionUserId"),
            decision_channel: text_of(row, "decisionChannel"),
            feedback: text_of(row, "feedback"),
            expires: row.get("expireDate").and_then(millis_of),
            created: row.get("createdDate").and_then(millis_of),
            result: json_of("resultJson"),
            policy: text_of(row, "policy"),
        }
    }

    /// The snapshot rows.
    pub fn changes(&self) -> Vec<ChangeRow> {
        self.snapshot
            .get("changes")
            .and_then(|changes| serde_json::from_value(changes.clone()).ok())
            .unwrap_or_default()
    }

    pub fn summary(&self) -> String {
        self.snapshot
            .get("summary")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// Where an Edit in screen takes the user, when it is offered.
    pub fn handoff_target(&self) -> Option<HandoffTarget> {
        self.snapshot
            .get("handoff")
            .and_then(|target| serde_json::from_value(target.clone()).ok())
    }

    /// What a card shows: never the hash.
    pub fn to_json(&self) -> Json {
        json!({
            "approvalId": self.approval_id,
            "runId": self.run_id,
            "toolCallId": self.tool_call_id,
            "classification": self.classification.as_str(),
            "targetKind": self.target_kind.as_str(),
            "targetName": self.target_name,
            "summary": self.summary(),
            "changes": self.changes(),
            "record": self.snapshot.get("record").cloned().unwrap_or(Json::Null),
            "recordRefs": self.record_refs,
            "status": self.status.as_str(),
            "decisionChannel": self.decision_channel,
            "feedback": self.feedback,
            "expiresAt": self.expires,
            "canEdit": self.snapshot.get("handoff").is_some_and(|target| !target.is_null()),
            "preview": self.preview,
            "result": self.result,
        })
    }
}

/// The state a run pauses in for `kind` on `surface`: over A2A and MCP an approval (or its hand-off) waits for the
/// human owner in Moqui, which is AuthRequired (plan 4.8, J21); every other pause, and every pause elsewhere, is the
/// kind's own state.
pub fn pause_state(kind: PauseKind, surface: Surface) -> run::RunState {
    match kind {
        PauseKind::Approval | PauseKind::Handoff if matches!(surface, Surface::A2a | Surface::Mcp) => {
            run::RunState::AuthRequired
        }
        _ => kind.state(),
    }
}

/// The page where an approval's owner decides it when the surface has no card of its own (A2A, MCP): the drawer opened
/// on the run's conversation, which shows the card, at `base_url`.
pub fn link(base_url: &str, conversation_id: &str) -> String {
    // a conversation id is a sequenced id: nothing in it needs encoding
    format!("{}/qapps?assist={conversation_id}", base_url.trim_end_matches('/'))
}

/// The data part of an approval decided by link (plan 4.8): `{type: "moqui.approval", approvalId, url, expiresAt,
/// summary, status}`, the time as RFC 3339. Never the arguments' hash.
pub fn link_data(view: &ApprovalView, url: &str) -> Json {
    let expires = view
        .expires
        .and_then(|at| jiff::Timestamp::from_millisecond(at).ok())
        .map(|at| at.to_string());
    json!({"type": "moqui.approval", "approvalId": view.approval_id, "url": url, "expiresAt": expires,
        "summary": view.summary(), "status": view.status.as_str()})
}

/// The hash an approval is bound by: SHA-256 over the RFC 8785 canonical JSON of `{v: 1, userId, agentId, toolName,
/// target: {kind, name}, args}`. The executor computes it again from the arguments it is about to run.
pub fn approval_hash(
    user_id: &str,
    agent_id: &str,
    tool_name: &str,
    kind: TargetKind,
    target_name: &str,
    args: &Json,
) -> String {
    moqui_lib::json::canonical_sha256(&json!({
        "v": 1,
        "userId": user_id,
        "agentId": agent_id,
        "toolName": tool_name,
        "target": {"kind": kind.as_str(), "name": target_name},
        "args": args,
    }))
}

/// A new approval id: 128 random bits as hex.
fn new_id() -> Result<String, Error> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| Error::new(format!("random source failed: {error}")))?;
    Ok(moqui_lib::codec::hex_encode(&bytes))
}

/// A background agent: its run is a job, and every surface it serves is (plan 4.8: policy approves only for them).
fn is_background(ctx: &TurnContext<'_>) -> bool {
    ctx.surface == Surface::Job
        && !ctx.agent.surfaces.is_empty()
        && ctx.agent.surfaces.iter().all(|surface| *surface == Surface::Job)
}

fn factory_and_facade() -> Option<(Arc<ExecutionContextFactory>, EntityFacade)> {
    let factory = ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))?;
    let facade = factory.entity()?;
    Some((factory, facade))
}

/// Ask for the approval of a write, after the tool's precheck: refused when the agent's policy does not let it write
/// (its write rule, or a DESTRUCTIVE call without `allowDestructive`), when the call has no run, or when nobody can be
/// asked and no policy decides; approved at once by a policy rule of a background agent; else a Pending row, the
/// `approval.requested` event and a pause. A tool with `previewMode` always (or a DESTRUCTIVE one that has not turned
/// it off) is previewed first when the sim is available.
pub fn request(ctx: &mut TurnContext<'_>, request: &ApprovalRequest) -> Requested {
    let rule = ctx.approvals.write_rule;
    let class = request.classification;
    if !rule.offered || (class == Classification::Destructive && !rule.allow_destructive) {
        return Requested::Refused(format!(
            "this agent may not make {} calls with {}, so nothing was changed",
            class.as_str(),
            ctx.call.name
        ));
    }
    let Some(run) = ctx.run else {
        return Requested::Refused(
            "a write needs a run that can wait for the user's approval, so nothing was changed".to_string(),
        );
    };
    let policy = ctx.approvals.mode == ApprovalMode::Policy && is_background(ctx);
    let what = if request.summary.is_empty() {
        request.target_name.clone()
    } else {
        request.summary.clone()
    };
    if !policy && !ctx.approvals.can_ask {
        return Requested::Refused(format!(
            "{what} needs the user's approval, and nobody can give it here, so nothing was changed"
        ));
    }
    let Some((factory, facade)) = factory_and_facade() else {
        return Requested::Refused("approvals are not available here, so nothing was changed".to_string());
    };
    let approval_id = match new_id() {
        Ok(id) => id,
        Err(error) => {
            tracing::error!("no approval id for agent run {}: {error}", run.run_id);
            return Requested::Refused("the approval could not be asked now, so nothing was changed".to_string());
        }
    };
    let now = run::now_millis();
    let expires = now + i64::from(ctx.approvals.ttl_minutes.max(1)) * 60_000;
    let hash = approval_hash(
        &ctx.user.user_id,
        &ctx.agent.agent_id,
        &ctx.call.name,
        request.target_kind,
        &request.target_name,
        &request.args,
    );
    let mut snapshot = json!({"summary": what, "changes": request.changes, "record": request.snapshot});
    if let Some(target) = &request.handoff {
        snapshot["handoff"] = serde_json::to_value(target).unwrap_or(Json::Null);
    }
    let auto_preview = match request.preview {
        PreviewMode::Always => true,
        PreviewMode::OnDemand => class == Classification::Destructive,
        PreviewMode::None => false,
    };
    let previewed = (!policy && auto_preview && preview::available(&facade)).then(|| {
        preview::run_target(
            &factory,
            &preview::PreviewTarget {
                username: &ctx.user.username,
                agent_id: &ctx.agent.agent_id,
                run_id: Some(&run.run_id),
                approval_id: &approval_id,
                kind: request.target_kind,
                name: &request.target_name,
                args: &request.args,
            },
        )
    });
    let policy_name = policy.then(|| match rule.sequence {
        Some(sequence) => format!("AgentToolPolicy {}#{sequence}", ctx.agent.agent_id),
        None => format!("AgentToolPolicy {}", ctx.agent.agent_id),
    });
    let node = ctx.events.node().to_string();
    let tool_call_id = ctx.call.tool_call_id.clone();
    let stored = {
        // run bookkeeping the runtime writes, under the run's lease, so a read tool's fence lifts for it
        let _lifted = fence::lift();
        quietly(|| {
            in_tx(&facade, || {
                run::held(&facade, &run.run_id, &node, Hold::Lease)?;
                let mut row = facade.make_value(APPROVAL)?;
                row.set("approvalId", Value::string(&approval_id));
                row.set("runId", Value::string(&run.run_id));
                row.set("toolCallId", Value::string(&tool_call_id));
                row.set("userId", Value::string(&ctx.user.user_id));
                row.set("agentId", Value::string(&ctx.agent.agent_id));
                row.set("classificationEnumId", Value::string(class.enum_id()));
                row.set("targetKind", Value::string(request.target_kind.as_str()));
                row.set("targetName", Value::string(&request.target_name));
                row.set("argumentsJson", Value::string(request.args.to_string()));
                row.set("argumentsHash", Value::string(&hash));
                row.set("snapshotJson", Value::string(snapshot.to_string()));
                if !request.record_refs.is_null() {
                    row.set("recordRefsJson", Value::string(request.record_refs.to_string()));
                }
                if let Some(preview) = &previewed {
                    row.set("previewJson", Value::string(preview.to_string()));
                }
                row.set("createdDate", run::stamp(now));
                row.set("expireDate", run::stamp(expires));
                match &policy_name {
                    Some(name) => {
                        row.set("statusId", Value::string(ApprovalStatus::Approved.status_id()));
                        row.set("decisionUserId", Value::string(&ctx.user.user_id));
                        row.set("decisionDate", run::stamp(now));
                        row.set("decisionChannel", Value::string(Channel::Policy.as_str()));
                        row.set("policy", Value::string(name));
                    }
                    None => row.set("statusId", Value::string(ApprovalStatus::Pending.status_id())),
                }
                row.create(&facade)?;
                facade
                    .find(TOOL_CALL)?
                    .eq("toolCallId", Value::string(&tool_call_id))
                    .for_update(true)
                    .disable_authz()
                    .update_all([("approvalId".to_string(), Value::string(&approval_id))])?;
                Ok(ApprovalView::of(&row))
            })
        })
    };
    let view = match stored {
        Ok(view) => view,
        Err(error) => {
            tracing::error!("the approval of agent run {} could not be stored: {error}", run.run_id);
            return Requested::Refused("the approval could not be asked now, so nothing was changed".to_string());
        }
    };
    if let Some(name) = policy_name {
        ctx.events.emit(
            "approval.decided",
            json!({"approvalId": approval_id, "toolCallId": tool_call_id, "decision": "approve",
                "status": ApprovalStatus::Approved.as_str(), "channel": Channel::Policy.as_str(), "policy": name}),
        );
        return Requested::Approved(Grant {
            approval_id,
            args_hash: hash,
        });
    }
    let mut card = view.to_json();
    card["tool"] = json!(ctx.call.name);
    // the risk a loaded skill says the write has: shown on the card, it never skips the approval
    if let Some((skill, risk)) = crate::llm::skills::risk_of_write(ctx, &request.target_name) {
        card["risk"] = json!({"skill": skill, "risk": risk.as_str()});
    }
    // a tool call's write (another system's, a canvas Mutation) has no sim preview
    card["canPreview"] = json!(request.target_kind != TargetKind::Tool && preview::available(&facade));
    ctx.events.signal(json!({"runId": run.run_id, "conversationId": run.conversation_id,
        "approvalId": approval_id, "event": "approval.requested", "summary": what}));
    Requested::Pause(Pause {
        kind: PauseKind::Approval,
        reference: approval_id,
        expires: Some(expires),
        event: Some(("approval.requested".to_string(), card)),
    })
}

/// The approval `approval_id`, if there is one.
///
/// # Errors
///
/// Returns an error when the row cannot be read.
pub fn load(facade: &EntityFacade, approval_id: &str) -> Result<Option<ApprovalView>, Error> {
    Ok(facade
        .find(APPROVAL)?
        .eq("approvalId", Value::string(approval_id))
        .disable_authz()
        .one()?
        .map(|row| ApprovalView::of(&row)))
}

/// The approval `approval_id` when it is `user_id`'s; another user's is `None`, as for none (owner-scoped lookups).
///
/// # Errors
///
/// Returns an error when the row cannot be read.
pub fn load_for_owner(facade: &EntityFacade, approval_id: &str, user_id: &str) -> Result<Option<ApprovalView>, Error> {
    Ok(load(facade, approval_id)?.filter(|view| !user_id.is_empty() && view.user_id == user_id))
}

/// The newest approval of the call being made (its resume reads it).
///
/// # Errors
///
/// Returns an error when the rows cannot be read.
pub fn for_call(ctx: &TurnContext<'_>) -> Result<Option<ApprovalView>, Error> {
    let Some(run) = ctx.run else {
        return Ok(None);
    };
    let Some((_, facade)) = factory_and_facade() else {
        return Ok(None);
    };
    quietly(|| {
        Ok(facade
            .find(APPROVAL)?
            .eq("runId", Value::string(&run.run_id))
            .eq("toolCallId", Value::string(&ctx.call.tool_call_id))
            .order_by("createdDate", true)
            .limit(1)
            .disable_authz()
            .list()?
            .values()
            .first()
            .map(ApprovalView::of))
    })
}

/// What a paused call resumes with, from its approval.
#[derive(Clone, Debug, PartialEq)]
pub enum Resumed {
    /// The user approved it (or a policy did): check for conflicts, then [`begin`] and run it.
    Approved,
    /// It will not run (rejected, expired, a conflict, already done): what the model reads.
    Done(String),
    /// The user saved it themselves in the screen (Edit in screen), or left it: what the model reads.
    HandedOff(String),
    /// Nothing is decided yet: the call pauses again.
    Waiting(Pause),
}

/// What the call of `view` resumes with: its status, and its hand-off when the user took it to the screen.
///
/// # Errors
///
/// Returns an error when the hand-off cannot be read.
pub fn resumed(view: &ApprovalView) -> Result<Resumed, Error> {
    if let Some((_, facade)) = factory_and_facade() {
        if let Some(found) = handoff::of_approval(&facade, &view.approval_id)? {
            return Ok(found.resumed(view));
        }
    }
    Ok(match view.status {
        ApprovalStatus::Approved => Resumed::Approved,
        ApprovalStatus::Pending => Resumed::Waiting(Pause {
            kind: PauseKind::Approval,
            reference: view.approval_id.clone(),
            expires: view.expires,
            event: None,
        }),
        // a room member's rejection is no instruction of the requester's (answered question 12)
        ApprovalStatus::Rejected => match view.decision_user_id.as_deref().filter(|decider| *decider != view.user_id) {
            Some(member) => Resumed::Done(rejected_by_member_text(member)),
            None => Resumed::Done(rejected_text(view.feedback.as_deref())),
        },
        ApprovalStatus::Expired => {
            Resumed::Done("The approval expired before the user decided, so nothing was saved.".to_string())
        }
        ApprovalStatus::Conflict => Resumed::Done(
            "The record changed after this write was proposed, so nothing was saved; read it again and propose the \
             write anew."
                .to_string(),
        ),
        ApprovalStatus::Executing | ApprovalStatus::Executed | ApprovalStatus::Failed | ApprovalStatus::HandedOff => {
            Resumed::Done(format!(
                "This approval is {} already, so it was not run again.",
                view.status.as_str()
            ))
        }
    })
}

/// What the model reads of a rejection: the feedback is its only task now (plan 7.2).
pub fn rejected_text(feedback: Option<&str>) -> String {
    match feedback.map(str::trim).filter(|text| !text.is_empty()) {
        Some(feedback) => format!(
            "The user rejected this write, so nothing was saved. Their feedback is now your only task: {feedback}"
        ),
        None => "The user rejected this write and said nothing more, so nothing was saved. Ask what they want instead."
            .to_string(),
    }
}

/// What the model reads of a rejection by a chat room member who is not the requester: nothing they said, and no task.
pub fn rejected_by_member_text(member: &str) -> String {
    format!(
        "Another member of the chat room ({}) rejected this write, so nothing was saved. This is not an instruction \
         of the person who asked you: ask them what they want instead.",
        crate::llm::prompt::one_line(member, 60)
    )
}

/// Move the approval from one of `from` to `to` with `fields`, in the transaction in place: `None` when it is in
/// none of them (another decision came first), else the row as it was before.
fn move_status(
    facade: &EntityFacade,
    approval_id: &str,
    from: &[ApprovalStatus],
    to: ApprovalStatus,
    fields: Vec<(&str, Value)>,
) -> Result<Option<ApprovalView>, Error> {
    let Some(mut row) = facade
        .find(APPROVAL)?
        .eq("approvalId", Value::string(approval_id))
        .for_update(true)
        .disable_authz()
        .one()?
    else {
        return Ok(None);
    };
    let before = ApprovalView::of(&row);
    if !from.contains(&before.status) {
        return Ok(None);
    }
    if !before.status.can_move_to(to) {
        return Err(Error::new(format!(
            "an approval cannot go from {} to {}",
            before.status.as_str(),
            to.as_str()
        )));
    }
    row.set("statusId", Value::string(to.status_id()));
    for (field, value) in fields {
        row.set(field, value);
    }
    row.update(facade)?;
    Ok(Some(before))
}

/// Start running an approved write: the hash is computed again from the arguments about to run (`args`) with the
/// user, agent and tool of this call, and only a match moves the approval from Approved to Executing, once. A mismatch
/// fails the approval; the caller runs nothing and asks again.
///
/// # Errors
///
/// What the model reads: the approval is not Approved (decided otherwise, run already), or the arguments differ.
pub fn begin(ctx: &mut TurnContext<'_>, approval_id: &str, args: &Json) -> Result<Grant, String> {
    let Some((_, facade)) = factory_and_facade() else {
        return Err("approvals are not available here, so nothing was run".to_string());
    };
    let Some(view) = quietly(|| load(&facade, approval_id)).ok().flatten() else {
        return Err("there is no such approval, so nothing was run".to_string());
    };
    let hash = approval_hash(
        &ctx.user.user_id,
        &ctx.agent.agent_id,
        &ctx.call.name,
        view.target_kind,
        &view.target_name,
        args,
    );
    let matches = view.user_id == ctx.user.user_id
        && view.tool_call_id == ctx.call.tool_call_id
        && constant_time_eq(hash.as_bytes(), view.args_hash.as_bytes());
    let (to, fields) = if matches {
        (ApprovalStatus::Executing, Vec::new())
    } else {
        tracing::warn!(
            "the arguments of approval {approval_id} do not hash to the approved ones, so it fails and nothing runs"
        );
        (
            ApprovalStatus::Failed,
            vec![(
                "resultJson",
                Value::string(json!({"error": "arguments_mismatch"}).to_string()),
            )],
        )
    };
    let node = ctx.events.node().to_string();
    let run_id = ctx.run.map(|run| run.run_id.clone());
    let moved = {
        let _lifted = fence::lift();
        quietly(|| {
            in_tx(&facade, || {
                if let Some(run_id) = &run_id {
                    run::held(&facade, run_id, &node, Hold::Lease)?;
                }
                move_status(&facade, approval_id, &[ApprovalStatus::Approved], to, fields)
            })
        })
    };
    match moved {
        Ok(Some(_)) if matches => Ok(Grant {
            approval_id: approval_id.to_string(),
            args_hash: hash,
        }),
        Ok(Some(_)) => Err(
            "These are not the arguments the user approved, so nothing was run; propose the write again for a new \
             approval."
                .to_string(),
        ),
        Ok(None) => Err(format!(
            "This approval is {}, not approved, so nothing was run.",
            view.status.as_str()
        )),
        Err(error) => {
            tracing::error!("approval {approval_id} could not start: {error}");
            Err("the approval could not be used now, so nothing was run".to_string())
        }
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && left.iter().zip(right).fold(0_u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

/// Record what an approved write came to: Executed or Failed, with its result.
pub fn finish(ctx: &mut TurnContext<'_>, approval_id: &str, ok: bool, result: &Json) {
    let to = if ok {
        ApprovalStatus::Executed
    } else {
        ApprovalStatus::Failed
    };
    settle(ctx, approval_id, &[ApprovalStatus::Executing], to, result);
}

/// Record a conflict found before an approved write ran: the record changed since the snapshot, so nothing ran.
pub fn conflict(ctx: &mut TurnContext<'_>, approval_id: &str, conflicts: &Json) {
    settle(
        ctx,
        approval_id,
        &[ApprovalStatus::Approved, ApprovalStatus::Pending],
        ApprovalStatus::Conflict,
        &json!({"conflicts": conflicts}),
    );
}

fn settle(ctx: &mut TurnContext<'_>, approval_id: &str, from: &[ApprovalStatus], to: ApprovalStatus, result: &Json) {
    let Some((_, facade)) = factory_and_facade() else {
        return;
    };
    let node = ctx.events.node().to_string();
    let run_id = ctx.run.map(|run| run.run_id.clone());
    let _lifted = fence::lift();
    let settled = quietly(|| {
        in_tx(&facade, || {
            if let Some(run_id) = &run_id {
                run::held(&facade, run_id, &node, Hold::Lease)?;
            }
            move_status(
                &facade,
                approval_id,
                from,
                to,
                vec![("resultJson", Value::string(result.to_string()))],
            )
        })
    });
    match settled {
        Ok(Some(_)) => {}
        Ok(None) => tracing::warn!("approval {approval_id} was not in a state to become {}", to.as_str()),
        Err(error) => tracing::error!("approval {approval_id} could not become {}: {error}", to.as_str()),
    }
}

/// Where a decision came from (`decisionChannel`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// The Assist drawer of a browser session.
    Drawer,
    /// A chat room's card.
    Chat,
    /// The terminal.
    Tui,
    /// A link to the user's own session (A2A, MCP).
    Link,
    /// A background agent's policy rule.
    Policy,
    /// The user's click on a Button a canvas drew (open question 13: the click is the approval).
    Canvas,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Drawer => "drawer",
            Channel::Chat => "chat",
            Channel::Tui => "tui",
            Channel::Link => "link",
            Channel::Policy => "policy",
            Channel::Canvas => "canvas",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [Channel::Drawer, Channel::Chat, Channel::Tui, Channel::Link]
            .into_iter()
            .find(|channel| channel.as_str() == text.trim())
    }
}

/// Who decides an approval, and through what. Made only from the request in progress ([`Decider::from_request`]), or
/// by the evaluation harness and the tests for a scripted user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decider {
    user_id: String,
    channel: Channel,
}

impl Decider {
    /// The current user deciding through the request in progress: a browser render authenticated by its session
    /// cookie whose transition presented a valid session token (the drawer, a chat card, a link; `channel` says
    /// which, the drawer by default), or the terminal client. Refused for a header-authenticated request (Basic,
    /// `api_key`, `login_key`), an agent's own screen session, `agent-apps`, internal code (a job, a service called
    /// from Rust) and any context an agent's run works in, so an A2A, MCP or REST caller, or the agent itself, can
    /// never decide.
    ///
    /// # Errors
    ///
    /// Why the request may not decide.
    pub fn from_request(channel: Option<Channel>) -> Result<Self, String> {
        let refused = || {
            "an approval is decided only by its user in the app: a browser session or the terminal".to_string()
        };
        if ExecutionContext::with_current(|ec| ec.agent_origin().is_some()).unwrap_or(true) {
            return Err(refused());
        }
        let user_id = crate::user::current_user_id()
            .ok()
            .flatten()
            .filter(|id| !id.is_empty())
            .ok_or_else(refused)?;
        let channel = match crate::screen::interactive_request() {
            Some(crate::screen::InteractiveRequest::Terminal) => Channel::Tui,
            Some(crate::screen::InteractiveRequest::Browser) => match channel {
                Some(channel @ (Channel::Drawer | Channel::Chat | Channel::Link)) => channel,
                _ => Channel::Drawer,
            },
            None => return Err(refused()),
        };
        Ok(Self { user_id, channel })
    }

    /// A scripted user's decision: the evaluation harness and the tests, never a request.
    pub(crate) fn scripted(user_id: &str, channel: Channel) -> Self {
        Self {
            user_id: user_id.to_string(),
            channel,
        }
    }

    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    pub fn channel(&self) -> Channel {
        self.channel
    }
}

/// A decision of the user.
#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    /// Run it. `arguments` are the user's edits of a service or tool call's arguments (the card's argument editor):
    /// they replace the proposed ones, and the hash is bound to them; the tool checks them again before it runs them.
    /// A screen write is edited in the screen instead.
    Approve { arguments: Option<Json> },
    /// Do not run it; the feedback is the agent's next task.
    Reject { feedback: String },
    /// Edit in screen: the user's page gets the proposed values and the user saves it themselves.
    Edit,
}

impl Decision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Decision::Approve { .. } => "approve",
            Decision::Reject { .. } => "reject",
            Decision::Edit => "edit",
        }
    }
}

/// What [`decide`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decided {
    pub approval_id: String,
    pub status: ApprovalStatus,
    /// The hand-off watch an Edit in screen made.
    pub handoff_id: Option<String>,
}

/// Why a decision was not taken.
#[derive(Clone, Debug, PartialEq)]
pub enum DecideError {
    /// No approval of that id for this user: someone else's is not named.
    NotFound,
    /// It may not be decided (already decided, expired, its run ended, a decision it does not take): why.
    Refused(String),
    Failed(String),
}

impl std::fmt::Display for DecideError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecideError::NotFound => f.write_str("there is no such approval"),
            DecideError::Refused(text) | DecideError::Failed(text) => f.write_str(text),
        }
    }
}

impl From<Error> for DecideError {
    fn from(error: Error) -> Self {
        DecideError::Failed(error.to_string())
    }
}

/// Decide an approval as `decider`: only its user, only while it is Pending, not expired and its run waits for it.
/// Approve and Reject are moved conditionally (a second decision is refused) and given to the run as input, which
/// resumes it; Edit hands the write to the user's screen ([`handoff`]) and the run waits for the save.
///
/// # Errors
///
/// [`DecideError::NotFound`] for another user's approval or none, [`DecideError::Refused`] when it cannot be decided.
pub fn decide(
    factory: &Arc<ExecutionContextFactory>,
    approval_id: &str,
    decider: &Decider,
    decision: &Decision,
) -> Result<Decided, DecideError> {
    let facade = run::facade_of(factory)?;
    suspended(&facade, || quietly(|| decide_in(factory, &facade, approval_id, decider, decision)))
}

/// `body` with any transaction in place suspended, so what it writes commits before the run it resumes reads it (a
/// decision from a transition's actions, inside the transition's transaction).
pub(crate) fn suspended<T>(
    facade: &EntityFacade,
    body: impl FnOnce() -> Result<T, DecideError>,
) -> Result<T, DecideError> {
    let tx = facade.transactions_for(APPROVAL)?;
    let mut out = None;
    tx.run(crate::transaction::TransactionAttribute::IgnoreSuspend, None, || {
        out = Some(body());
        Ok(())
    })?;
    out.unwrap_or_else(|| Err(DecideError::Failed("the decision did not run".to_string())))
}

fn decide_in(
    factory: &Arc<ExecutionContextFactory>,
    facade: &EntityFacade,
    approval_id: &str,
    decider: &Decider,
    decision: &Decision,
) -> Result<Decided, DecideError> {
    let view = match load_for_owner(facade, approval_id, &decider.user_id)? {
        Some(view) => view,
        // answered question 12: a chat run's card is decided by any human who may post in its room, never an agent
        None if decider.channel == Channel::Chat => {
            crate::llm::chat::room_approval(facade, approval_id, &decider.user_id)?.ok_or(DecideError::NotFound)?
        }
        None => return Err(DecideError::NotFound),
    };
    if view.status != ApprovalStatus::Pending {
        return Err(DecideError::Refused(format!(
            "this approval was decided already: it is {}",
            view.status.as_str()
        )));
    }
    let now = run::now_millis();
    let run = run::load_run(facade, &view.run_id)?;
    let waiting = run.as_ref().is_some_and(|run| {
        run.state.is_paused()
            && run.pause_kind == Some(PauseKind::Approval)
            && run.pause_ref.as_deref() == Some(approval_id)
    });
    let expired = view.expires.is_some_and(|at| at <= now);
    if expired || !waiting {
        expire_one(factory, facade, &view)?;
        return Err(DecideError::Refused(if expired {
            "this approval expired, so nothing was saved".to_string()
        } else {
            "the agent no longer waits for this approval, so nothing was saved".to_string()
        }));
    }
    let run = run.expect("a waiting run");
    let mut fields: Vec<(&str, Value)> = vec![
        ("decisionUserId", Value::string(&decider.user_id)),
        ("decisionDate", run::stamp(now)),
        ("decisionChannel", Value::string(decider.channel.as_str())),
    ];
    // a room member who is not the requester (answered question 12) approves the write as it stands or rejects it:
    // no edited arguments, no hand-off to their own page, and their rejection gives the agent no task
    let member = view.user_id != decider.user_id;
    if member {
        let edits = match decision {
            Decision::Approve { arguments } => arguments.as_ref().is_some_and(|edited| *edited != view.args),
            Decision::Edit => true,
            Decision::Reject { .. } => false,
        };
        if edits {
            return Err(DecideError::Refused(
                "only the person who asked the agent may change what it writes (edited arguments, Edit in screen)"
                    .to_string(),
            ));
        }
    }
    let (to, text) = match decision {
        Decision::Edit => return handoff::start(factory, facade, &view, &run, decider),
        Decision::Approve { arguments } => {
            if let Some(edited) = arguments.as_ref().filter(|edited| **edited != view.args) {
                if view.target_kind == TargetKind::Screen {
                    return Err(DecideError::Refused(
                        "a screen write is edited in the screen: use Edit in screen".to_string(),
                    ));
                }
                let tool_name = tool_name_of(facade, &view.tool_call_id)?;
                let hash = approval_hash(
                    &view.user_id,
                    &view.agent_id,
                    &tool_name,
                    view.target_kind,
                    &view.target_name,
                    edited,
                );
                let mut snapshot = view.snapshot.clone();
                snapshot["edited"] = json!(true);
                snapshot["proposedArgs"] = view.args.clone();
                fields.push(("argumentsJson", Value::string(edited.to_string())));
                fields.push(("argumentsHash", Value::string(hash)));
                fields.push(("snapshotJson", Value::string(snapshot.to_string())));
            }
            (ApprovalStatus::Approved, "approved")
        }
        Decision::Reject { feedback } => {
            if !member && !feedback.trim().is_empty() {
                fields.push(("feedback", Value::string(feedback.trim())));
            }
            (ApprovalStatus::Rejected, "rejected")
        }
    };
    let mut sink = EventSink::new(factory, facade, &view.run_id, &view.user_id);
    let feedback = match decision {
        Decision::Reject { .. } if member => None,
        Decision::Reject { feedback } => Some(feedback.trim().to_string()).filter(|text| !text.is_empty()),
        _ => None,
    };
    in_tx(facade, || {
        if move_status(facade, approval_id, &[ApprovalStatus::Pending], to, fields)?.is_none() {
            return Ok(false);
        }
        run::add_input(
            facade,
            &view.run_id,
            &view.user_id,
            "AgikDecision",
            text,
            Some(&json!({"approvalId": approval_id, "decision": decision.as_str()})),
            None,
        )?;
        sink.emit(
            "approval.decided",
            json!({"approvalId": approval_id, "toolCallId": view.tool_call_id, "decision": decision.as_str(),
                "status": to.as_str(), "channel": decider.channel.as_str(), "feedback": feedback}),
        );
        sink.signal(json!({"runId": view.run_id, "approvalId": approval_id, "event": "approval.decided",
            "status": to.as_str()}));
        sink.flush()?;
        Ok(true)
    })
    .map_err(DecideError::from)
    .and_then(|moved| {
        if moved {
            Ok(())
        } else {
            Err(DecideError::Refused("this approval was decided already".to_string()))
        }
    })?;
    if let Some(feedback) = &feedback {
        propose_lesson_of(factory, facade, &view, &run.conversation_id, feedback);
    }
    run::resume_paused(factory, facade, &view.run_id)?;
    Ok(Decided {
        approval_id: approval_id.to_string(),
        status: to,
        handoff_id: None,
    })
}

/// The requester rejected a write with feedback (plan 7.5): the feedback becomes a lesson of theirs on the write's screen
/// path or service, Proposed (theirs at once, the site's once an admin approves it). A tool call has no anchor; a
/// failure is logged and the decision stands. Only with the requester's memory gate open (their preference, or the
/// conversation's own): a lesson is something kept for them, as a memory is.
fn propose_lesson_of(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    view: &ApprovalView,
    conversation_id: &str,
    feedback: &str,
) {
    let default = crate::llm::facade(factory).conf().assist.memory_default;
    match crate::llm::memory::gate(facade, &view.user_id, Some(conversation_id), default) {
        Ok(gate) if gate.effective() => {}
        Ok(_) => return,
        Err(error) => {
            tracing::warn!("the memory gate of {} could not be read, so no lesson was kept: {error}", view.user_id);
            return;
        }
    }
    let anchor = match view.target_kind {
        TargetKind::Screen => view
            .args
            .get("path")
            .and_then(Json::as_str)
            .and_then(crate::llm::memory::Anchor::parse),
        TargetKind::Service => crate::llm::memory::Anchor::parse(&view.target_name),
        TargetKind::Tool => None,
    };
    let Some(anchor) = anchor else {
        return;
    };
    let summary = view.snapshot.get("summary").and_then(Json::as_str).unwrap_or(&view.target_name);
    let trigger = format!("Rejected: {summary}");
    let lesson: String = feedback.chars().take(crate::llm::memory::LESSON_CHARS).collect();
    if let Err(error) =
        crate::llm::memory::propose_lesson(facade, &view.user_id, &anchor, &lesson, Some(&trigger), Some(conversation_id))
    {
        tracing::warn!("the feedback on approval {} was not kept as a lesson: {error}", view.approval_id);
    }
}

fn tool_name_of(facade: &EntityFacade, tool_call_id: &str) -> Result<String, Error> {
    Ok(facade
        .find(TOOL_CALL)?
        .eq("toolCallId", Value::string(tool_call_id))
        .disable_authz()
        .one()?
        .and_then(|row| text_of(&row, "toolName"))
        .unwrap_or_default())
}

/// Expire one Pending approval that can no longer be decided, with its event.
fn expire_one(factory: &Arc<ExecutionContextFactory>, facade: &EntityFacade, view: &ApprovalView) -> Result<(), Error> {
    let mut sink = EventSink::new(factory, facade, &view.run_id, &view.user_id);
    in_tx(facade, || {
        if move_status(
            facade,
            &view.approval_id,
            &[ApprovalStatus::Pending],
            ApprovalStatus::Expired,
            Vec::new(),
        )?
        .is_some()
        {
            sink.emit(
                "approval.decided",
                json!({"approvalId": view.approval_id, "toolCallId": view.tool_call_id, "decision": "expire",
                    "status": ApprovalStatus::Expired.as_str()}),
            );
        }
        // an ended run's events are still its own; a run on another node is that node's to tell
        match sink.flush() {
            Err(error) if run::is_lease_lost(&error) => Ok(()),
            other => other,
        }
    })
}

/// The run ended: its approvals that did not run expire, and its hand-off watches are abandoned, each with its event.
/// In the transaction of the run's end, which gave its lease up.
///
/// # Errors
///
/// Returns an error when the rows cannot be written.
pub(crate) fn expire_for_run(facade: &EntityFacade, sink: &mut EventSink, run_id: &str) -> Result<(), Error> {
    let _lifted = fence::lift();
    let open = facade
        .find(APPROVAL)?
        .eq("runId", Value::string(run_id))
        .condition(Condition::is_in(
            "statusId",
            [ApprovalStatus::Pending, ApprovalStatus::Approved, ApprovalStatus::HandedOff]
                .into_iter()
                .map(|status| Value::string(status.status_id()))
                .collect(),
        ))
        .disable_authz()
        .list()?;
    for row in open.values() {
        let view = ApprovalView::of(row);
        if move_status(
            facade,
            &view.approval_id,
            &[view.status],
            ApprovalStatus::Expired,
            Vec::new(),
        )?
        .is_some()
        {
            sink.emit(
                "approval.decided",
                json!({"approvalId": view.approval_id, "toolCallId": view.tool_call_id, "decision": "expire",
                    "status": ApprovalStatus::Expired.as_str()}),
            );
        }
    }
    handoff::abandon_for_run(facade, sink, run_id, "the run ended")
}
