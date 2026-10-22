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

//! Edit in screen (plans/LLM-A2A-Assist.md 4.8): the approval becomes a hand-off. The user's page gets `ui.navigate`
//! and `ui.fill` with the proposed values and `handoff.started`, the approval is HandedOff and an `AgentHandoff` watch
//! (Watching) waits for the user's own save; the run's pause is now the hand-off's.
//!
//! The [`HandoffObserver`] is the screen render's [`TransitionObserver`]: after a transition of a user with an active
//! watch it matches the watch's screen and transition, the held write's extra path and the record (the screen's
//! parameters and the key fields the agent did not change, [`same_record`]), and records the outcome, the values posted
//! and the messages. A save of the user's own (a browser or the terminal, never the agent's session or a request that
//! came by a header or in a session a header login made) that ran with no error is Saved: the approval is Executed, `handoff.done` is stored and the run resumes with "the
//! user saved …". A save refused with errors is Failed and the watch goes on, so the user may fix it and save again.
//! A watch that expires, or that the user leaves ([`abandon_handoff`]), is Abandoned and the run resumes with that. The
//! observer learns which users have a watch from the database every two seconds, so a save served by another node is
//! seen.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::{Duration, Instant};

use moqui_db::sql::Condition;
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;
use serde_json::{json, Map, Value as Json};

use super::{
    load, load_for_owner, move_status, ApprovalStatus, ApprovalView, DecideError, Decided, Decider, Resumed, HANDOFF,
};
use crate::context::ExecutionContextFactory;
use crate::entity::{fence, EntityFacade, EntityValue};
use crate::llm::run::{self, in_tx, quietly, EventSink, Hold, RunRow};
use crate::llm::tools::{Pause, PauseKind};
use crate::llm::{millis_of, text_of};
use crate::screen::{TransitionObserver, TransitionOutcome};

/// How often the observer reads which users have a watch.
const REFRESH: Duration = Duration::from_secs(2);

/// A hand-off watch's status: its `StatusItem` is [`HandoffStatus::status_id`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandoffStatus {
    Watching,
    Saved,
    /// The user's save was refused (a validation error); the watch goes on.
    Failed,
    Abandoned,
}

impl HandoffStatus {
    pub const ALL: [HandoffStatus; 4] = [
        HandoffStatus::Watching,
        HandoffStatus::Saved,
        HandoffStatus::Failed,
        HandoffStatus::Abandoned,
    ];

    pub fn status_id(self) -> &'static str {
        match self {
            HandoffStatus::Watching => "AghWatching",
            HandoffStatus::Saved => "AghSaved",
            HandoffStatus::Failed => "AghFailed",
            HandoffStatus::Abandoned => "AghAbandoned",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            HandoffStatus::Watching => "watching",
            HandoffStatus::Saved => "saved",
            HandoffStatus::Failed => "failed",
            HandoffStatus::Abandoned => "abandoned",
        }
    }

    pub fn from_status_id(status_id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.status_id() == status_id)
    }

    /// The watch still waits for a save.
    pub fn is_open(self) -> bool {
        matches!(self, HandoffStatus::Watching | HandoffStatus::Failed)
    }
}

/// Every allowed move of a watch, the `StatusFlowTransition` rows of `data/AgentTypeData.xml`.
pub const TRANSITIONS: &[(HandoffStatus, HandoffStatus)] = &[
    (HandoffStatus::Watching, HandoffStatus::Saved),
    (HandoffStatus::Watching, HandoffStatus::Failed),
    (HandoffStatus::Watching, HandoffStatus::Abandoned),
    (HandoffStatus::Failed, HandoffStatus::Watching),
    (HandoffStatus::Failed, HandoffStatus::Saved),
    (HandoffStatus::Failed, HandoffStatus::Abandoned),
];

/// A watch as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct HandoffView {
    pub handoff_id: String,
    pub run_id: String,
    pub approval_id: String,
    pub user_id: String,
    pub status: HandoffStatus,
    pub outcome: Json,
    pub expires: Option<i64>,
}

impl HandoffView {
    fn of(row: &EntityValue) -> Self {
        Self {
            handoff_id: text_of(row, "handoffId").unwrap_or_default(),
            run_id: text_of(row, "runId").unwrap_or_default(),
            approval_id: text_of(row, "approvalId").unwrap_or_default(),
            user_id: text_of(row, "userId").unwrap_or_default(),
            status: text_of(row, "statusId")
                .and_then(|status| HandoffStatus::from_status_id(&status))
                .unwrap_or(HandoffStatus::Watching),
            outcome: text_of(row, "outcomeJson")
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or(Json::Null),
            expires: row.get("expireDate").and_then(millis_of),
        }
    }

    /// What the paused call of `approval` resumes with.
    pub(super) fn resumed(&self, approval: &ApprovalView) -> Resumed {
        match self.status {
            HandoffStatus::Watching | HandoffStatus::Failed => Resumed::Waiting(Pause {
                kind: PauseKind::Handoff,
                reference: self.handoff_id.clone(),
                expires: self.expires,
                event: None,
            }),
            HandoffStatus::Saved => Resumed::HandedOff(saved_text(&self.outcome, approval)),
            HandoffStatus::Abandoned => Resumed::HandedOff(format!(
                "The user took this write to the screen and did not save it ({}), so nothing was saved.",
                self.outcome
                    .get("abandoned")
                    .and_then(Json::as_str)
                    .unwrap_or("abandoned")
            )),
        }
    }
}

/// What the model reads of the user's own save: the values saved, each marked when it is not the one proposed, and the
/// messages.
fn saved_text(outcome: &Json, approval: &ApprovalView) -> String {
    let changes = approval.changes();
    let mut values: Vec<String> = Vec::new();
    if let Some(saved) = outcome.get("values").and_then(Json::as_object) {
        for (field, value) in saved {
            let value = value.as_str().map_or_else(|| value.to_string(), str::to_string);
            let change = changes.iter().find(|change| change.field == *field);
            let label = change.map_or(field.as_str(), |change| {
                if change.label.is_empty() {
                    &change.field
                } else {
                    &change.label
                }
            });
            match change {
                Some(change) if change.proposed != value => {
                    values.push(format!("{label}: {value} (you proposed {})", change.proposed));
                }
                Some(_) => values.push(format!("{label}: {value}")),
                // a field the agent did not change is named only when the user did
                None => {}
            }
        }
    }
    let mut text = "The user saved this write themselves in the screen, so it is done".to_string();
    if !values.is_empty() {
        text.push_str(&format!(": {}", values.join(", ")));
    }
    text.push('.');
    let messages: Vec<&str> = outcome
        .get("messages")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .collect();
    if !messages.is_empty() {
        text.push_str(&format!(" Messages: {}.", messages.join("; ")));
    }
    text
}

/// The newest watch of an approval, if it was handed off.
///
/// # Errors
///
/// Returns an error when the rows cannot be read.
pub fn of_approval(facade: &EntityFacade, approval_id: &str) -> Result<Option<HandoffView>, Error> {
    quietly(|| {
        Ok(facade
            .find(HANDOFF)?
            .eq("approvalId", Value::string(approval_id))
            .order_by("createdDate", true)
            .limit(1)
            .disable_authz()
            .list()?
            .values()
            .first()
            .map(HandoffView::of))
    })
}

/// Move a watch from one of `from` to `to` with `fields`, in the transaction in place: `None` when it is in none.
fn move_handoff(
    facade: &EntityFacade,
    handoff_id: &str,
    from: &[HandoffStatus],
    to: HandoffStatus,
    fields: Vec<(&str, Value)>,
) -> Result<Option<HandoffView>, Error> {
    let Some(mut row) = facade
        .find(HANDOFF)?
        .eq("handoffId", Value::string(handoff_id))
        .for_update(true)
        .disable_authz()
        .one()?
    else {
        return Ok(None);
    };
    let before = HandoffView::of(&row);
    if !from.contains(&before.status) || !TRANSITIONS.contains(&(before.status, to)) {
        return Ok(None);
    }
    row.set("statusId", Value::string(to.status_id()));
    for (field, value) in fields {
        row.set(field, value);
    }
    row.update(facade)?;
    Ok(Some(before))
}

/// Edit in screen: the approval is HandedOff, a watch is written and the run's pause becomes the watch's; the user's
/// page gets `handoff.started`, `ui.navigate` and a `ui.fill` for each proposed value.
pub(super) fn start(
    factory: &Arc<ExecutionContextFactory>,
    facade: &EntityFacade,
    view: &ApprovalView,
    run: &RunRow,
    decider: &Decider,
) -> Result<Decided, DecideError> {
    let target = view.handoff_target().ok_or_else(|| {
        DecideError::Refused("this write cannot be edited in a screen; approve or reject it".to_string())
    })?;
    let now = run::now_millis();
    let ttl = i64::from(crate::llm::facade(factory).conf().limits.approval_ttl_minutes.max(1)) * 60_000;
    let expires = now + ttl;
    let mut sink = EventSink::new(factory, facade, &view.run_id, &view.user_id);
    let changes = view.changes();
    // the page events go to the conversation's follow owner (the deciding tab claims it), for its page as it is now
    let stamp = crate::llm::assist::follow_stamp(facade, &run.conversation_id);
    let stamped = |mut payload: Json| {
        stamp.apply(&mut payload);
        payload
    };
    let handoff_id = in_tx(facade, || {
        let decided = vec![
            ("decisionUserId", Value::string(decider.user_id())),
            ("decisionDate", run::stamp(now)),
            ("decisionChannel", Value::string(decider.channel().as_str())),
        ];
        if move_status(
            facade,
            &view.approval_id,
            &[ApprovalStatus::Pending],
            ApprovalStatus::HandedOff,
            decided,
        )?
        .is_none()
        {
            return Ok(None);
        }
        let mut row = facade.make_value(HANDOFF)?;
        row.set_sequenced_id_primary(facade)?;
        row.set("runId", Value::string(&view.run_id));
        row.set("approvalId", Value::string(&view.approval_id));
        row.set("userId", Value::string(&view.user_id));
        row.set("screenPath", Value::string(&target.screen_location));
        row.set("transitionName", Value::string(&target.transition));
        row.set("statusId", Value::string(HandoffStatus::Watching.status_id()));
        row.set("createdDate", run::stamp(now));
        row.set("expireDate", run::stamp(expires));
        row.create(facade)?;
        let handoff_id = text_of(&row, "handoffId").unwrap_or_default();
        // the run waits for the user's save now
        let mut paused = run::held(facade, &run.run_id, sink.node(), Hold::LeaseOrNone)?;
        paused.set("pauseKindEnumId", Value::string(PauseKind::Handoff.enum_id()));
        paused.set("pauseRef", Value::string(&handoff_id));
        paused.set("pauseExpireDate", run::stamp(expires));
        paused.update(facade)?;
        sink.emit(
            "approval.decided",
            json!({"approvalId": view.approval_id, "toolCallId": view.tool_call_id, "decision": "edit",
                "status": ApprovalStatus::HandedOff.as_str(), "channel": decider.channel().as_str(),
                "handoffId": handoff_id}),
        );
        let values: Vec<Json> = changes
            .iter()
            .map(|change| json!({"field": change.field, "label": change.label, "value": change.proposed}))
            .collect();
        sink.emit(
            "handoff.started",
            stamped(json!({"handoffId": handoff_id, "approvalId": view.approval_id, "path": target.path,
                "parameters": target.parameters, "form": target.form, "transition": target.transition,
                "values": values, "expiresAt": expires})),
        );
        sink.emit(
            "ui.navigate",
            stamped(json!({"path": target.path, "parameters": target.parameters, "title": view.summary(),
                "handoffId": handoff_id})),
        );
        for change in &changes {
            sink.emit(
                "ui.fill",
                stamped(json!({"form": target.form, "domId": target.form, "occurrence": 0, "field": change.field,
                    "value": change.proposed, "label": change.label, "handoffId": handoff_id})),
            );
        }
        sink.signal(json!({"runId": view.run_id, "approvalId": view.approval_id, "handoffId": handoff_id,
            "event": "handoff.started"}));
        sink.flush()?;
        Ok(Some(handoff_id))
    })?
    .ok_or_else(|| DecideError::Refused("this approval was decided already".to_string()))?;
    note_watch(&view.user_id);
    Ok(Decided {
        approval_id: view.approval_id.clone(),
        status: ApprovalStatus::HandedOff,
        handoff_id: Some(handoff_id),
    })
}

/// The users with an open watch, as last read, and the watches begun on this node since (kept two reads long, so a
/// read that began before one was written does not hide it).
struct Watchers {
    users: HashSet<String>,
    noted: HashMap<String, Instant>,
    loaded: Option<Instant>,
}

impl Watchers {
    fn has(&self, user_id: &str) -> bool {
        self.users.contains(user_id)
            || self
                .noted
                .get(user_id)
                .is_some_and(|at| at.elapsed() < REFRESH * 2)
    }
}

static WATCHERS: LazyLock<Mutex<Watchers>> = LazyLock::new(|| {
    Mutex::new(Watchers {
        users: HashSet::new(),
        noted: HashMap::new(),
        loaded: None,
    })
});

/// A watch of `user_id` began on this node: the observer sees their next save without waiting for its next read.
fn note_watch(user_id: &str) {
    lock(&WATCHERS).noted.insert(user_id.to_string(), Instant::now());
}

/// Whether `user_id` has an open watch: the users read from the database at most every [`REFRESH`], the lock let go
/// while they are read.
fn watching(facade: &EntityFacade, user_id: &str) -> bool {
    {
        let watchers = lock(&WATCHERS);
        if watchers.loaded.is_some_and(|at| at.elapsed() < REFRESH) {
            return watchers.has(user_id);
        }
    }
    let users = quietly(|| -> Result<HashSet<String>, Error> {
        Ok(facade
            .find(HANDOFF)?
            .condition(Condition::is_in(
                "statusId",
                vec![
                    Value::string(HandoffStatus::Watching.status_id()),
                    Value::string(HandoffStatus::Failed.status_id()),
                ],
            ))
            .disable_authz()
            .list()?
            .values()
            .iter()
            .filter_map(|row| text_of(row, "userId"))
            .collect())
    });
    let mut watchers = lock(&WATCHERS);
    match users {
        Ok(users) => {
            watchers.users = users;
            watchers.noted.retain(|_, at| at.elapsed() < REFRESH * 2);
            watchers.loaded = Some(Instant::now());
        }
        Err(error) => tracing::error!("the users with a hand-off watch could not be read: {error}"),
    }
    watchers.has(user_id)
}

/// The screen render's observer of the hand-off watches.
pub struct HandoffObserver {
    factory: Weak<ExecutionContextFactory>,
}

impl TransitionObserver for HandoffObserver {
    fn wants(&self, user_id: Option<&str>) -> bool {
        let (Some(user_id), Some(factory)) = (user_id, self.factory.upgrade()) else {
            return false;
        };
        factory.entity().is_some_and(|facade| watching(&facade, user_id))
    }

    fn observed(&self, outcome: &TransitionOutcome) {
        let Some(factory) = self.factory.upgrade() else {
            return;
        };
        if let Err(error) = observe(&factory, outcome) {
            tracing::error!(
                "the outcome of {}#{} could not be given to its hand-off: {error}",
                outcome.screen_location,
                outcome.transition
            );
        }
    }
}

/// Install the hand-off observer for the screen renders of this process.
pub fn install(factory: &Arc<ExecutionContextFactory>) {
    crate::screen::install_transition_observer(Arc::new(HandoffObserver {
        factory: Arc::downgrade(factory),
    }));
}

/// Give a transition's outcome to the watch it matches: the user's own save (a browser or the terminal) of the watched
/// screen, transition and extra path. Out: the watch it went to.
///
/// # Errors
///
/// Returns an error when the rows cannot be read or written.
pub fn observe(factory: &Arc<ExecutionContextFactory>, outcome: &TransitionOutcome) -> Result<Option<String>, Error> {
    // the user's own save: a person at a browser or the terminal, never a program with a key or the agent
    if outcome.held || !outcome.origin.is_interactive() || outcome.header_authenticated {
        return Ok(None);
    }
    let Some(user_id) = outcome.user_id.as_deref().filter(|id| !id.is_empty()) else {
        return Ok(None);
    };
    let facade = run::facade_of(factory)?;
    let now = run::now_millis();
    let watches: Vec<HandoffView> = quietly(|| -> Result<Vec<HandoffView>, Error> {
        Ok(facade
            .find(HANDOFF)?
            .eq("userId", Value::string(user_id))
            .eq("screenPath", Value::string(&outcome.screen_location))
            .eq("transitionName", Value::string(&outcome.transition))
            .condition(Condition::is_in(
                "statusId",
                vec![
                    Value::string(HandoffStatus::Watching.status_id()),
                    Value::string(HandoffStatus::Failed.status_id()),
                ],
            ))
            .order_by("createdDate", true)
            .disable_authz()
            .list()?
            .values()
            .iter()
            .map(HandoffView::of)
            .filter(|watch| watch.expires.is_none_or(|at| at > now))
            .collect())
    })?;
    for watch in watches {
        let Some(approval) = quietly(|| load(&facade, &watch.approval_id))? else {
            continue;
        };
        let Some(target) = approval.handoff_target() else {
            continue;
        };
        if !target.extra_path.is_empty() && target.extra_path != outcome.extra_path {
            continue;
        }
        if !same_record(&approval, &target, outcome) {
            continue;
        }
        let saved = outcome.error.is_none()
            && outcome.errors.is_empty()
            && outcome.validation_errors.is_empty()
            && outcome.status < 400;
        let values: Map<String, Json> = target
            .fields
            .iter()
            .filter_map(|field| outcome.parameters.get(field).map(|value| (field.clone(), value.clone())))
            .collect();
        let mut errors = outcome.errors.clone();
        errors.extend(outcome.error.clone());
        let recorded = json!({"saved": saved, "values": values, "messages": outcome.messages, "errors": errors,
            "validationErrors": outcome.validation_errors, "status": outcome.status});
        if saved {
            finish(factory, &facade, &watch, &approval, HandoffStatus::Saved, &recorded)?;
        } else {
            // refused: the watch goes on for the user's next try, the last outcome kept
            quietly(|| {
                in_tx(&facade, || {
                    let mut fields = vec![("outcomeJson".to_string(), Value::string(recorded.to_string()))];
                    if watch.status == HandoffStatus::Watching {
                        fields.push(("statusId".to_string(), Value::string(HandoffStatus::Failed.status_id())));
                    }
                    facade
                        .find(HANDOFF)?
                        .eq("handoffId", Value::string(&watch.handoff_id))
                        .eq("statusId", Value::string(watch.status.status_id()))
                        .for_update(true)
                        .disable_authz()
                        .update_all(fields)
                })
            })?;
        }
        return Ok(Some(watch.handoff_id));
    }
    Ok(None)
}

/// Whether a save is of the record the write was for: the screen's parameters the save posts are the write's, and so
/// is every key field the write posted and the agent did not change (a field named `…Id`, or one of the screen's
/// parameters, such as a hidden id). A save of another record through the same screen and transition (another tab, a
/// hidden key changed) is not this hand-off's. The user may change the other fields of the form as they like.
fn same_record(approval: &ApprovalView, target: &super::HandoffTarget, outcome: &TransitionOutcome) -> bool {
    let posted = |name: &str| {
        outcome
            .parameters
            .get(name)
            .map(|value| value.as_str().map_or_else(|| value.to_string(), str::to_string))
    };
    for (name, value) in &target.parameters {
        if posted(name).is_some_and(|posted| posted != *value) {
            return false;
        }
    }
    let Ok(write) = serde_json::from_value::<crate::llm::screen::PendingWrite>(approval.args.clone()) else {
        return true;
    };
    let changed: Vec<&str> = write.changes.iter().map(|change| change.field.as_str()).collect();
    write
        .fields
        .iter()
        .filter(|(name, _)| !changed.contains(&name.as_str()))
        .filter(|(name, _)| name.ends_with("Id") || target.parameters.contains_key(*name))
        .all(|(name, value)| posted(name).as_deref() == Some(value.as_str()))
}

/// End a watch (saved or abandoned): the approval Executed or Expired, `handoff.done`, and the run resumes with it.
fn finish(
    factory: &Arc<ExecutionContextFactory>,
    facade: &EntityFacade,
    watch: &HandoffView,
    approval: &ApprovalView,
    to: HandoffStatus,
    recorded: &Json,
) -> Result<bool, Error> {
    let approval_to = if to == HandoffStatus::Saved {
        ApprovalStatus::Executed
    } else {
        ApprovalStatus::Expired
    };
    let mut sink = EventSink::new(factory, facade, &watch.run_id, &watch.user_id);
    let ended = quietly(|| {
        let _lifted = fence::lift();
        in_tx(facade, || {
            if move_handoff(
                facade,
                &watch.handoff_id,
                &[HandoffStatus::Watching, HandoffStatus::Failed],
                to,
                vec![
                    ("outcomeJson", Value::string(recorded.to_string())),
                    ("endedDate", run::stamp(run::now_millis())),
                ],
            )?
            .is_none()
            {
                return Ok(false);
            }
            move_status(
                facade,
                &approval.approval_id,
                &[ApprovalStatus::HandedOff],
                approval_to,
                vec![("resultJson", Value::string(recorded.to_string()))],
            )?;
            run::add_input(
                facade,
                &watch.run_id,
                &watch.user_id,
                "AgikDecision",
                to.as_str(),
                Some(&json!({"handoffId": watch.handoff_id, "approvalId": approval.approval_id,
                    "outcome": to.as_str()})),
                None,
            )?;
            sink.emit(
                "handoff.done",
                json!({"handoffId": watch.handoff_id, "approvalId": approval.approval_id, "outcome": to.as_str(),
                    "values": recorded.get("values"), "messages": recorded.get("messages")}),
            );
            sink.signal(json!({"runId": watch.run_id, "handoffId": watch.handoff_id, "event": "handoff.done",
                "outcome": to.as_str()}));
            sink.flush()?;
            Ok(true)
        })
    })?;
    if ended {
        quietly(|| run::resume_paused(factory, facade, &watch.run_id))?;
    }
    Ok(ended)
}

/// The run's watch expired (the pause sweep): Abandoned, and the run resumes with that. `false` when the run's pause
/// names no open watch, for the sweep to end the run as for any expired pause.
///
/// # Errors
///
/// Returns an error when the rows cannot be written.
pub(crate) fn abandon_paused(
    factory: &Arc<ExecutionContextFactory>,
    facade: &EntityFacade,
    run: &RunRow,
    reason: &str,
) -> Result<bool, Error> {
    let Some(handoff_id) = &run.pause_ref else {
        return Ok(false);
    };
    let Some(watch) = quietly(|| -> Result<Option<HandoffView>, Error> {
        Ok(facade
            .find(HANDOFF)?
            .eq("handoffId", Value::string(handoff_id))
            .disable_authz()
            .one()?
            .map(|row| HandoffView::of(&row)))
    })?
    else {
        return Ok(false);
    };
    let Some(approval) = quietly(|| load(facade, &watch.approval_id))? else {
        return Ok(false);
    };
    if !watch.status.is_open() {
        return Ok(false);
    }
    finish(
        factory,
        facade,
        &watch,
        &approval,
        HandoffStatus::Abandoned,
        &json!({"saved": false, "abandoned": reason}),
    )
}

/// The user left a hand-off without saving (the drawer's "navigated away"): its watch is Abandoned and the run goes on.
/// Only the watch's user may; anyone else gets [`DecideError::NotFound`].
///
/// # Errors
///
/// [`DecideError::NotFound`] for another user's watch or none, [`DecideError::Refused`] when it ended already.
pub fn abandon_handoff(
    factory: &Arc<ExecutionContextFactory>,
    handoff_id: &str,
    decider: &Decider,
    reason: &str,
) -> Result<(), DecideError> {
    let facade = run::facade_of(factory)?;
    super::suspended(&facade, || abandon_in(factory, &facade, handoff_id, decider, reason))
}

fn abandon_in(
    factory: &Arc<ExecutionContextFactory>,
    facade: &EntityFacade,
    handoff_id: &str,
    decider: &Decider,
    reason: &str,
) -> Result<(), DecideError> {
    let watch = quietly(|| -> Result<Option<HandoffView>, Error> {
        Ok(facade
            .find(HANDOFF)?
            .eq("handoffId", Value::string(handoff_id))
            .disable_authz()
            .one()?
            .map(|row| HandoffView::of(&row)))
    })?
    .filter(|watch| watch.user_id == decider.user_id())
    .ok_or(DecideError::NotFound)?;
    if !watch.status.is_open() {
        return Err(DecideError::Refused(format!(
            "this hand-off ended already: it is {}",
            watch.status.as_str()
        )));
    }
    let approval =
        quietly(|| load_for_owner(facade, &watch.approval_id, decider.user_id()))?.ok_or(DecideError::NotFound)?;
    if finish(
        factory,
        facade,
        &watch,
        &approval,
        HandoffStatus::Abandoned,
        &json!({"saved": false, "abandoned": reason}),
    )? {
        Ok(())
    } else {
        Err(DecideError::Refused("this hand-off ended already".to_string()))
    }
}

/// The run ended: its open watches are Abandoned, with `handoff.done`, in the transaction of its end.
///
/// # Errors
///
/// Returns an error when the rows cannot be written.
pub(crate) fn abandon_for_run(facade: &EntityFacade, sink: &mut EventSink, run_id: &str, reason: &str) -> Result<(), Error> {
    let open = facade
        .find(HANDOFF)?
        .eq("runId", Value::string(run_id))
        .condition(Condition::is_in(
            "statusId",
            vec![
                Value::string(HandoffStatus::Watching.status_id()),
                Value::string(HandoffStatus::Failed.status_id()),
            ],
        ))
        .disable_authz()
        .list()?;
    for row in open.values() {
        let watch = HandoffView::of(row);
        if move_handoff(
            facade,
            &watch.handoff_id,
            &[watch.status],
            HandoffStatus::Abandoned,
            vec![
                ("outcomeJson", Value::string(json!({"saved": false, "abandoned": reason}).to_string())),
                ("endedDate", run::stamp(run::now_millis())),
            ],
        )?
        .is_some()
        {
            sink.emit(
                "handoff.done",
                json!({"handoffId": watch.handoff_id, "approvalId": watch.approval_id, "outcome": "abandoned"}),
            );
        }
    }
    Ok(())
}
