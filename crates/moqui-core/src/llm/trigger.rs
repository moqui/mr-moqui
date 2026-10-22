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

//! Triggered agents and the agent inbox (plans/LLM-A2A-Assist.md 4.14, after Twenty's agent triggers).
//!
//! An `AgentTrigger` starts a run of its agent on the job surface, as its run-as user (the trigger's, else the
//! agent's), with its instructions as the message:
//!
//! - **cron**: [`store_agent_trigger`] keeps the `ServiceJob` `agent_trigger_<triggerId>` that calls
//!   `fire#AgentTrigger` on the schedule (job locks and history from the service jobs), paused while the trigger is
//!   paused or its breaker is open;
//! - **document**: the trigger's `DataDocument` is linked to the `AgentTriggers` data feed, whose receive service
//!   ([`receive_trigger_documents`]) stores each delivered document as an `AgentTriggerEvent`. The **loop guard**: the
//!   data feed records the agent whose run made every write of a record in the transaction (the `ExecutionContext`'s
//!   agent origin, `DataFeedPending.originAgentId`, the document's `_originAgentId`), and a trigger drops a document
//!   its own agent changed, so a trigger's own writes (approved ones too, which run in the run's step) never fire it
//!   again. A chain through another agent's trigger is bounded by the run limits and the breaker.
//!
//! **Batching**: the events of a trigger wait for its batch window (`batchSeconds`, from the oldest); the scheduler's
//! tick ([`sweep`]) fires a trigger whose window closed, and one run takes up to `maxBatchEvents` (50) of them, claimed
//! with a conditional update so one node takes each. A window of 0 fires after the delivery commits.
//!
//! **Run limits**: `maxConcurrent` runs at once, `maxRunsPerHour` and `maxRunsPerDay`; a firing over a limit is skipped
//! (logged) and a document trigger's events wait for the next window. **The breaker**: a run that fails (or a firing
//! that cannot start one) counts against `failureLimit` (3) in a row, a completed run resets the count; at the limit
//! the breaker opens (`breakerState` open, the cron job paused), the admins and the run-as user are told in the
//! inbox, and nothing fires until an admin resets it ([`reset_trigger_breaker`]).
//!
//! **The inbox** (the `AgentInbox` notification topic, which the drawer's tasks pane also shows): a background run's
//! result or failure goes to its user, and so does an approval it waits for; a trigger with `approvalHold` Y lets a
//! write its policy does not approve wait for its run-as user's decision there (the run gets `approval.link`), else
//! the write is an error the model reads (plan 4.8). A background agent's policy rule approves by itself as before.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use moqui_db::sql::Condition;
use moqui_lib::error::Error;
use moqui_lib::object::{is_empty, to_plain_string, Value};
use serde_json::{json, Value as Json};

use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::EntityFacade;
use crate::llm::prompt::Surface;
use crate::llm::run::observe::RunEvents;
use crate::llm::run::{self, quietly, RunState};
use crate::llm::tools::caps;
use crate::llm::{flag_of, int_of, text_of};
use crate::service::rust_services::{rust_service, RustServiceFn};

pub(crate) const TRIGGER: &str = "moqui.agent.AgentTrigger";
pub(crate) const TRIGGER_EVENT: &str = "moqui.agent.AgentTriggerEvent";
/// The data feed of the document triggers (seed data).
pub const FEED_ID: &str = "AgentTriggers";
pub const FIRE_SERVICE: &str = "org.moqui.agent.AgentTriggerServices.fire#AgentTrigger";
pub const INBOX_TOPIC: &str = "AgentInbox";

/// Failures in a row that open the breaker when a trigger names none.
pub const FAILURE_LIMIT: i64 = 3;
/// Events one run takes when a trigger names no limit.
pub const MAX_BATCH_EVENTS: i64 = 50;
/// The run limits of a trigger that names none, so even a loop the guard does not see (a chain through another
/// agent, a write that lost its origin) stays bounded.
pub const MAX_RUNS_PER_HOUR: i64 = 60;
pub const MAX_CONCURRENT: i64 = 2;
/// The characters of one event, and of all of them, in the run's message.
const EVENT_CHARS: usize = 1500;
const EVENTS_CHARS: usize = 8000;

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, store_agent_trigger)?;
    rust_service!(map, fire_agent_trigger)?;
    rust_service!(map, receive_trigger_documents)?;
    rust_service!(map, delete_trigger_document)?;
    rust_service!(map, reset_trigger_breaker)?;
    Ok(())
}

type Parameters = BTreeMap<String, Value>;
type Out = Result<BTreeMap<String, Value>, Error>;

fn param(parameters: &Parameters, key: &str) -> Option<String> {
    parameters
        .get(key)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn add_error(text: impl Into<String>) {
    let text = text.into();
    ExecutionContext::with_current(|ctx| ctx.message().add_error(text));
}

fn factory() -> Option<Arc<ExecutionContextFactory>> {
    ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
}

fn admin() -> bool {
    match crate::user::has_permission("AgentAdmin") {
        Ok(true) => true,
        _ => {
            add_error("this needs the AgentAdmin permission");
            false
        }
    }
}

/// The job of a cron trigger.
pub fn job_name(trigger_id: &str) -> String {
    format!("agent_trigger_{trigger_id}")
}

/// A trigger as the engine reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct Trigger {
    pub trigger_id: String,
    pub agent_id: String,
    /// `cron` or `document`.
    pub kind: String,
    pub cron: Option<String>,
    pub data_document_id: Option<String>,
    pub instructions: String,
    pub run_as_user_id: Option<String>,
    pub batch_seconds: i64,
    pub max_batch_events: i64,
    pub max_runs_per_hour: Option<i64>,
    pub max_runs_per_day: Option<i64>,
    pub max_concurrent: Option<i64>,
    pub approval_hold: bool,
    pub failure_limit: i64,
    pub failure_count: i64,
    pub breaker_open: bool,
    pub paused: bool,
}

impl Trigger {
    pub fn of(row: &crate::entity::EntityValue) -> Self {
        let positive = |field: &str| int_of(row, field).filter(|n| *n > 0);
        Self {
            trigger_id: text_of(row, "triggerId").unwrap_or_default(),
            agent_id: text_of(row, "agentId").unwrap_or_default(),
            kind: match text_of(row, "triggerTypeEnumId").as_deref() {
                Some("AgttDocument" | "document") => "document".to_string(),
                _ => "cron".to_string(),
            },
            cron: text_of(row, "cronExpression"),
            data_document_id: text_of(row, "dataDocumentId"),
            instructions: text_of(row, "instructions").unwrap_or_default(),
            run_as_user_id: text_of(row, "runAsUserId"),
            batch_seconds: int_of(row, "batchSeconds").unwrap_or(0).max(0),
            max_batch_events: positive("maxBatchEvents").unwrap_or(MAX_BATCH_EVENTS),
            max_runs_per_hour: positive("maxRunsPerHour").or(Some(MAX_RUNS_PER_HOUR)),
            max_runs_per_day: positive("maxRunsPerDay"),
            max_concurrent: positive("maxConcurrent").or(Some(MAX_CONCURRENT)),
            approval_hold: flag_of(row, "approvalHold") == Some(true),
            failure_limit: positive("failureLimit").unwrap_or(FAILURE_LIMIT),
            failure_count: int_of(row, "failureCount").unwrap_or(0),
            breaker_open: text_of(row, "breakerState").as_deref() == Some("open"),
            paused: flag_of(row, "paused") == Some(true),
        }
    }

    pub fn load(facade: &EntityFacade, trigger_id: &str) -> Result<Option<Self>, Error> {
        Ok(facade
            .find(TRIGGER)?
            .eq("triggerId", Value::string(trigger_id))
            .use_cache(false)
            .disable_authz()
            .one()?
            .map(|row| Self::of(&row)))
    }
}

// ---- configuration ----

/// `store#AgentTrigger`: create or change a trigger (AgentAdmin) and keep what fires it: the cron job (paused while the
/// trigger is paused, its breaker open, or it is no cron trigger) and the link of its DataDocument to the trigger feed.
fn store_agent_trigger(parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !admin() {
        return Ok(BTreeMap::new());
    }
    const FIELDS: &[&str] = &[
        "agentId",
        "triggerTypeEnumId",
        "description",
        "cronExpression",
        "dataDocumentId",
        "instructions",
        "runAsUserId",
        "batchSeconds",
        "maxBatchEvents",
        "maxRunsPerHour",
        "maxRunsPerDay",
        "maxConcurrent",
        "approvalHold",
        "failureLimit",
        "paused",
    ];
    let stored = quietly(|| {
        run::in_tx(facade, || {
            let trigger_id = param(parameters, "triggerId");
            let existing = match &trigger_id {
                Some(id) => facade
                    .find(TRIGGER)?
                    .eq("triggerId", Value::string(id))
                    .for_update(true)
                    .disable_authz()
                    .one()?,
                None => None,
            };
            let creating = existing.is_none();
            let mut row = match existing {
                Some(row) => row,
                None => {
                    let mut row = facade.make_value(TRIGGER)?;
                    match &trigger_id {
                        Some(id) => row.set("triggerId", Value::string(id)),
                        None => row.set_sequenced_id_primary(facade)?,
                    }
                    row.set("breakerState", Value::string("closed"));
                    row.set("failureCount", Value::I64(0));
                    row
                }
            };
            for field in FIELDS {
                if let Some(value) = parameters.get(*field) {
                    row.set(*field, if is_empty(value) { Value::Null } else { value.clone() });
                }
            }
            if text_of(&row, "agentId").is_none() {
                return Err(Error::new("a trigger needs an agent"));
            }
            // D12: a trigger runs as an agent's own service user or as the admin who stores it, never as another person
            if let Some(run_as) = text_of(&row, "runAsUserId") {
                let me = crate::user::current_user_id().ok().flatten().unwrap_or_default();
                let agent_user = ["runAsUserId", "chatUserId"].into_iter().any(|field| {
                    facade
                        .find("moqui.agent.AgentDefinition")
                        .and_then(|find| find.eq(field, Value::string(&run_as)).disable_authz().count())
                        .is_ok_and(|count| count > 0)
                });
                if run_as != me && !agent_user {
                    return Err(Error::new(format!(
                        "a trigger runs as an agent's service user or as yourself, not as {run_as}"
                    )));
                }
            }
            if creating {
                row.create(facade)?;
            } else {
                row.update(facade)?;
            }
            let trigger = Trigger::of(&row);
            sync_firing(facade, &trigger)?;
            Ok(trigger.trigger_id)
        })
    });
    match stored {
        Ok(trigger_id) => Ok(BTreeMap::from([("triggerId".to_string(), Value::string(trigger_id))])),
        Err(error) => {
            add_error(error.to_string());
            Ok(BTreeMap::new())
        }
    }
}

/// The cron job and the document link of a trigger, as its row says.
fn sync_firing(facade: &EntityFacade, trigger: &Trigger) -> Result<(), Error> {
    let name = job_name(&trigger.trigger_id);
    let job = facade
        .find("moqui.service.job.ServiceJob")?
        .eq("jobName", Value::string(&name))
        .disable_authz()
        .one()?;
    let wants_job = trigger.kind == "cron" && trigger.cron.is_some();
    let paused = trigger.paused || trigger.breaker_open || !wants_job;
    match job {
        Some(mut job) => {
            job.set("paused", Value::string(if paused { "Y" } else { "N" }));
            if let Some(cron) = &trigger.cron {
                job.set("cronExpression", Value::string(cron));
            }
            job.update(facade)?;
        }
        None if wants_job => {
            let mut job = facade.make_value("moqui.service.job.ServiceJob")?;
            job.set("jobName", Value::string(&name));
            job.set(
                "description",
                Value::string(format!("Agent trigger {}", trigger.trigger_id)),
            );
            job.set("serviceName", Value::string(FIRE_SERVICE));
            job.set(
                "cronExpression",
                Value::string(trigger.cron.clone().unwrap_or_default()),
            );
            job.set("paused", Value::string(if paused { "Y" } else { "N" }));
            job.create(facade)?;
            let mut parameter = facade.make_value("moqui.service.job.ServiceJobParameter")?;
            parameter.set("jobName", Value::string(&name));
            parameter.set("parameterName", Value::string("triggerId"));
            parameter.set("parameterValue", Value::string(&trigger.trigger_id));
            parameter.create(facade)?;
        }
        None => {}
    }
    if trigger.kind == "document" {
        if let Some(document_id) = &trigger.data_document_id {
            let linked = facade
                .find("moqui.entity.feed.DataFeedDocument")?
                .eq("dataFeedId", Value::string(FEED_ID))
                .eq("dataDocumentId", Value::string(document_id))
                .disable_authz()
                .one()?;
            if linked.is_none() {
                let mut link = facade.make_value("moqui.entity.feed.DataFeedDocument")?;
                link.set("dataFeedId", Value::string(FEED_ID));
                link.set("dataDocumentId", Value::string(document_id));
                link.create(facade)?;
            }
        }
    }
    Ok(())
}

/// `reset#AgentTriggerBreaker`: an admin closes a trigger's breaker; its count starts again and its job runs again.
fn reset_trigger_breaker(parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !admin() {
        return Ok(BTreeMap::new());
    }
    let Some(trigger_id) = param(parameters, "triggerId") else {
        add_error("reset#AgentTriggerBreaker needs a triggerId");
        return Ok(BTreeMap::new());
    };
    let reset = quietly(|| reset_breaker(facade, &trigger_id));
    if let Err(error) = reset {
        add_error(error.to_string());
    }
    Ok(BTreeMap::new())
}

/// Close the breaker of `trigger_id`.
///
/// # Errors
///
/// Returns an error when there is no such trigger or it cannot be written.
pub fn reset_breaker(facade: &EntityFacade, trigger_id: &str) -> Result<(), Error> {
    run::in_tx(facade, || {
        let mut row = facade
            .find(TRIGGER)?
            .eq("triggerId", Value::string(trigger_id))
            .for_update(true)
            .disable_authz()
            .one()?
            .ok_or_else(|| Error::new(format!("there is no agent trigger {trigger_id}")))?;
        row.set("breakerState", Value::string("closed"));
        row.set("failureCount", Value::I64(0));
        row.set("breakerOpenedDate", Value::Null);
        row.update(facade)?;
        sync_firing(facade, &Trigger::of(&row))
    })
}

// ---- firing ----

/// What a firing did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fired {
    Started {
        run_id: String,
        events: usize,
    },
    /// Nothing ran: why (paused, the breaker, a limit, no events).
    Skipped(String),
}

/// `fire#AgentTrigger`: the cron job (or an admin's Fire now, or a closed batch window) fires a trigger.
fn fire_agent_trigger(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let (Some(trigger_id), Some(factory)) = (param(parameters, "triggerId"), factory()) else {
        add_error("fire#AgentTrigger needs a triggerId");
        return Ok(BTreeMap::new());
    };
    let cause = param(parameters, "cause").unwrap_or_else(|| "schedule".to_string());
    match fire(&factory, &trigger_id, &cause) {
        Ok(Fired::Started { run_id, events }) => Ok(BTreeMap::from([
            ("runId".to_string(), Value::string(run_id)),
            ("eventCount".to_string(), Value::I64(i64::try_from(events).unwrap_or(0))),
        ])),
        Ok(Fired::Skipped(why)) => {
            tracing::info!("agent trigger {trigger_id} did not fire: {why}");
            Ok(BTreeMap::from([("skipped".to_string(), Value::string(why))]))
        }
        Err(error) => {
            tracing::error!("agent trigger {trigger_id} failed to fire: {error}");
            add_error(error.to_string());
            Ok(BTreeMap::new())
        }
    }
}

fn count_runs(facade: &EntityFacade, trigger_id: &str, since: Option<i64>, running: bool) -> Result<i64, Error> {
    let mut find = facade
        .find(run::RUN)?
        .eq("triggerId", Value::string(trigger_id))
        .disable_authz();
    if let Some(since) = since {
        find = find.condition(Condition::greater_equal("submittedDate", run::stamp(since)));
    }
    if running {
        find = find.condition(Condition::is_in(
            "statusId",
            [
                RunState::Submitted,
                RunState::Working,
                RunState::InputRequired,
                RunState::AuthRequired,
            ]
            .into_iter()
            .map(|state| Value::string(state.status_id()))
            .collect(),
        ));
    }
    find.count()
}

/// Why a trigger may not fire now, by its limits; `None` when it may.
fn over_limit(facade: &EntityFacade, trigger: &Trigger, now: i64) -> Result<Option<String>, Error> {
    if let Some(limit) = trigger.max_concurrent {
        if count_runs(facade, &trigger.trigger_id, None, true)? >= limit {
            return Ok(Some(format!("{limit} of its runs are not done (maxConcurrent)")));
        }
    }
    if let Some(limit) = trigger.max_runs_per_hour {
        if count_runs(facade, &trigger.trigger_id, Some(now - 3_600_000), false)? >= limit {
            return Ok(Some(format!("it ran {limit} times in the last hour (maxRunsPerHour)")));
        }
    }
    if let Some(limit) = trigger.max_runs_per_day {
        if count_runs(facade, &trigger.trigger_id, Some(now - 86_400_000), false)? >= limit {
            return Ok(Some(format!("it ran {limit} times in the last day (maxRunsPerDay)")));
        }
    }
    Ok(None)
}

/// Claim up to `limit` of the trigger's waiting events for `token`, oldest first: each with a conditional update, so
/// another node's claim of the same event wins once.
fn claim_events(facade: &EntityFacade, trigger_id: &str, limit: i64, token: &str) -> Result<Vec<Json>, Error> {
    let waiting = facade
        .find(TRIGGER_EVENT)?
        .eq("triggerId", Value::string(trigger_id))
        .condition(Condition::is_null("runId"))
        .order_by("receivedDate", false)
        .order_by("triggerEventId", false)
        .limit(limit)
        .disable_authz()
        .list()?;
    let mut claimed = Vec::new();
    for row in waiting.values() {
        let event_id = text_of(row, "triggerEventId").unwrap_or_default();
        let taken = run::in_tx(facade, || {
            facade
                .find(TRIGGER_EVENT)?
                .eq("triggerEventId", Value::string(&event_id))
                .condition(Condition::is_null("runId"))
                .disable_authz()
                .update_all([("runId".to_string(), Value::string(token))])
        })?;
        if taken > 0 {
            claimed.push(
                text_of(row, "eventJson")
                    .and_then(|text| serde_json::from_str(&text).ok())
                    .unwrap_or(Json::Null),
            );
        }
    }
    Ok(claimed)
}

/// Set the claimed events of `token` to `run_id` (or back to waiting with `None`).
fn settle_claim(facade: &EntityFacade, token: &str, run_id: Option<&str>) -> Result<(), Error> {
    run::in_tx(facade, || {
        facade
            .find(TRIGGER_EVENT)?
            .eq("runId", Value::string(token))
            .disable_authz()
            .update_all([("runId".to_string(), run_id.map_or(Value::Null, Value::string))])
            .map(|_| ())
    })
}

/// The run's message: the trigger's instructions, then the records that changed, each as untrusted text.
fn message_of(trigger: &Trigger, events: &[Json], cause: &str) -> String {
    let mut text = if trigger.instructions.trim().is_empty() {
        "Do the work this trigger is for.".to_string()
    } else {
        trigger.instructions.trim().to_string()
    };
    if trigger.kind == "document" {
        let mut block = String::new();
        let mut left = events.len();
        for event in events {
            let mut shown: Json = event.clone();
            if let Some(object) = shown.as_object_mut() {
                object.retain(|key, _| key != "_originAgentId" && key != "_index" && key != "_timestamp");
            }
            let line = crate::llm::tools::format::cut(&shown.to_string(), EVENT_CHARS);
            if block.chars().count() + line.chars().count() > EVENTS_CHARS {
                break;
            }
            block.push_str(&line);
            block.push('\n');
            left -= 1;
        }
        if left > 0 {
            block.push_str(&format!("(and {left} more not shown)\n"));
        }
        text.push_str(&format!(
            "\n\nRecords that changed ({}):\n{}",
            events.len(),
            crate::llm::prompt::untrusted("record", block.trim_end())
        ));
    } else {
        text.push_str(&format!("\n\n(The {cause} run of trigger {}.)", trigger.trigger_id));
    }
    text
}

/// Fire trigger `trigger_id` now: a run of its agent with its waiting events (a document trigger with none does not
/// run), unless it is paused, its breaker is open or a limit is reached. A firing that cannot start a run counts as a
/// failure of the trigger.
///
/// # Errors
///
/// Returns an error when the rows cannot be read or written.
pub fn fire(factory: &Arc<ExecutionContextFactory>, trigger_id: &str, cause: &str) -> Result<Fired, Error> {
    /// What the locked part of a firing came to.
    enum Step {
        Started(run::Started, usize),
        Skipped(String),
        /// Nothing started: a failure of the trigger, counted once the lock is given up.
        Failed(Box<Trigger>, String),
    }
    let facade = run::facade_of(factory)?;
    quietly(|| {
        // the trigger row is locked from the limit check to the run tagged with it, so two firings (two nodes, the job
        // and the sweep) never both pass a limit
        let step = run::in_tx(&facade, || {
            let Some(row) = facade
                .find(TRIGGER)?
                .eq("triggerId", Value::string(trigger_id))
                .for_update(true)
                .use_cache(false)
                .disable_authz()
                .one()?
            else {
                return Ok(Step::Skipped(format!("there is no agent trigger {trigger_id}")));
            };
            let trigger = Trigger::of(&row);
            if trigger.paused {
                return Ok(Step::Skipped("it is paused".to_string()));
            }
            if trigger.breaker_open {
                return Ok(Step::Skipped("its breaker is open".to_string()));
            }
            let now = run::now_millis();
            if let Some(why) = over_limit(&facade, &trigger, now)? {
                return Ok(Step::Skipped(why));
            }
            // a claim's mark in runId (an id field, 40 characters) until the run is there
            let token = format!("c{}", crate::entity::uuid_v4()?.replace('-', ""));
            let events = if trigger.kind == "document" {
                let events = claim_events(&facade, &trigger.trigger_id, trigger.max_batch_events, &token)?;
                if events.is_empty() {
                    return Ok(Step::Skipped("no record changed".to_string()));
                }
                events
            } else {
                Vec::new()
            };
            let agent_run_as = facade
                .find("moqui.agent.AgentDefinition")?
                .eq("agentId", Value::string(&trigger.agent_id))
                .disable_authz()
                .one()?
                .and_then(|row| text_of(&row, "runAsUserId"));
            let Some(user_id) = trigger.run_as_user_id.clone().or(agent_run_as) else {
                settle_claim(&facade, &token, None)?;
                return Ok(Step::Failed(Box::new(trigger), "the trigger and its agent name no run-as user".to_string()));
            };
            #[cfg(test)]
            let mut extra_caps = crate::llm::chat::test_caps(&trigger.agent_id);
            #[cfg(not(test))]
            let mut extra_caps = Vec::new();
            if trigger.approval_hold {
                // a write its policy does not approve waits for the run-as user in the inbox
                extra_caps.push(caps::APPROVAL_LINK.to_string());
            }
            let request = run::StartRequest {
                user_id,
                agent_id: trigger.agent_id.clone(),
                surface: Surface::Job,
                text: message_of(&trigger, &events, cause),
                client_request_id: Some(format!("trigger:{}:{token}", trigger.trigger_id)),
                extra_caps,
                defer_submit: true,
                ..run::StartRequest::default()
            };
            let started = match run::start_run(factory, &request) {
                Ok(started) => started,
                Err(error) => {
                    settle_claim(&facade, &token, None)?;
                    return Ok(Step::Failed(Box::new(trigger), format!("no run started: {error}")));
                }
            };
            facade
                .find(run::RUN)?
                .eq("runId", Value::string(&started.run_id))
                .disable_authz()
                .update_all([
                    ("triggerId".to_string(), Value::string(&trigger.trigger_id)),
                    (
                        "triggerRef".to_string(),
                        Value::string(if events.is_empty() {
                            cause.to_string()
                        } else {
                            format!("{cause}: {} events", events.len())
                        }),
                    ),
                ])?;
            facade
                .find(TRIGGER)?
                .eq("triggerId", Value::string(&trigger.trigger_id))
                .disable_authz()
                .update_all([
                    ("lastRunId".to_string(), Value::string(&started.run_id)),
                    ("lastRunDate".to_string(), run::stamp(now)),
                ])?;
            settle_claim(&facade, &token, Some(&started.run_id))?;
            if started.state == RunState::Rejected {
                return Ok(Step::Failed(Box::new(trigger), "the run was rejected (the quota is used up)".to_string()));
            }
            Ok(Step::Started(started, events.len()))
        })?;
        match step {
            Step::Started(started, events) => {
                run::submit_started(factory, &started)?;
                Ok(Fired::Started {
                    run_id: started.run_id,
                    events,
                })
            }
            Step::Skipped(why) => Ok(Fired::Skipped(why)),
            Step::Failed(trigger, why) => {
                failed(factory, &facade, &trigger, None, &why)?;
                Ok(Fired::Skipped(why))
            }
        }
    })
}

/// The scheduler's tick: fire each document trigger whose batch window has closed (from its oldest waiting event).
/// Failure tolerant: a trigger that fails to fire is logged and the others go on.
pub fn sweep(factory: &Arc<ExecutionContextFactory>) {
    let Some(facade) = factory.entity() else {
        return;
    };
    let waiting = quietly(|| {
        facade
            .find(TRIGGER_EVENT)?
            .condition(Condition::is_null("runId"))
            .order_by("receivedDate", false)
            .limit(1000)
            .disable_authz()
            .list()
    });
    let waiting = match waiting {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!("the waiting agent trigger events could not be read: {error}");
            return;
        }
    };
    let mut oldest: HashMap<String, i64> = HashMap::new();
    for row in waiting.values() {
        let (Some(trigger_id), Some(received)) = (
            text_of(row, "triggerId"),
            row.get("receivedDate").and_then(crate::llm::millis_of),
        ) else {
            continue;
        };
        oldest
            .entry(trigger_id)
            .and_modify(|at| *at = (*at).min(received))
            .or_insert(received);
    }
    let now = run::now_millis();
    for (trigger_id, received) in oldest {
        let due = quietly(|| Trigger::load(&facade, &trigger_id))
            .ok()
            .flatten()
            .is_some_and(|trigger| received + trigger.batch_seconds * 1000 <= now);
        if !due {
            continue;
        }
        if let Err(error) = fire(factory, &trigger_id, "batch") {
            tracing::error!("agent trigger {trigger_id} failed to fire its batch: {error}");
        }
    }
}

/// Start the batch sweep on the scheduler's tick (once per factory).
pub fn start(factory: &Arc<ExecutionContextFactory>) {
    struct Started;
    let mut first = false;
    factory.extension(|_| {
        first = true;
        Started
    });
    if !first {
        return;
    }
    let weak = Arc::downgrade(factory);
    factory.service_pools().add_tick_hook(move || {
        if let Some(factory) = weak.upgrade() {
            let context = factory.new_context();
            let _entered = context.enter();
            let _authz = context.disable_authz();
            sweep(&factory);
        }
    });
}

// ---- the trigger feed ----

/// The triggers on a document type, which are not paused.
fn document_triggers(facade: &EntityFacade, document_id: &str) -> Result<Vec<Trigger>, Error> {
    Ok(facade
        .find(TRIGGER)?
        .eq("dataDocumentId", Value::string(document_id))
        .disable_authz()
        .list()?
        .values()
        .iter()
        .map(Trigger::of)
        .filter(|trigger| trigger.kind == "document" && !trigger.paused)
        .collect())
}

/// Store `event` for each trigger on its document type whose agent did not make the change (the loop guard); the
/// triggers with no batch window are fired once the delivery commits.
fn store_events(
    facade: &EntityFacade,
    document_id: &str,
    record_id: &str,
    origin: Option<&str>,
    event: &Json,
) -> Result<Vec<String>, Error> {
    let mut now_fire = Vec::new();
    for trigger in document_triggers(facade, document_id)? {
        if origin == Some(trigger.agent_id.as_str()) {
            tracing::debug!(
                "agent trigger {} leaves out {document_id} {record_id}: its own agent changed it",
                trigger.trigger_id
            );
            continue;
        }
        let mut row = facade.make_value(TRIGGER_EVENT)?;
        row.set_sequenced_id_primary(facade)?;
        row.set("triggerId", Value::string(&trigger.trigger_id));
        row.set("dataDocumentId", Value::string(document_id));
        row.set("documentId", Value::string(record_id));
        row.set("eventJson", Value::string(event.to_string()));
        row.set("receivedDate", run::stamp(run::now_millis()));
        row.create(facade)?;
        if trigger.batch_seconds == 0 && !now_fire.contains(&trigger.trigger_id) {
            now_fire.push(trigger.trigger_id.clone());
        }
    }
    Ok(now_fire)
}

/// Fire `triggers` on a service worker after the transaction in place commits (now, with none).
fn fire_after_commit(facade: &EntityFacade, triggers: Vec<String>) {
    if triggers.is_empty() {
        return;
    }
    let submit = move || {
        for trigger_id in &triggers {
            let parameters = BTreeMap::from([
                ("triggerId".to_string(), Value::string(trigger_id)),
                ("cause".to_string(), Value::string("change")),
            ]);
            if let Err(error) = crate::service::call::async_call(FIRE_SERVICE, &parameters) {
                tracing::error!(
                    "agent trigger {trigger_id} could not be fired, its batch waits for the sweep: {error}"
                );
            }
        }
    };
    match facade.transactions_for(TRIGGER_EVENT) {
        Ok(tx) if tx.in_transaction() => {
            let held = Arc::new(std::sync::Mutex::new(Some(submit)));
            let later = Arc::clone(&held);
            if tx
                .on_commit(move || {
                    if let Some(submit) = moqui_lib::sync::lock(&later).take() {
                        submit();
                    }
                })
                .is_err()
            {
                tracing::warn!("the agent triggers wait for the sweep: the commit hook could not be registered");
            }
        }
        _ => submit(),
    }
}

/// `receive#TriggerDocuments`: the trigger feed's documents (`documentList`), stored as events of the triggers on
/// their type; a document its trigger's own agent changed (`_originAgentId`) is left out for that trigger.
fn receive_trigger_documents(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let documents = match parameters.get("documentList") {
        Some(Value::List(list)) => list.clone(),
        _ => Vec::new(),
    };
    let mut fire_now: Vec<String> = Vec::new();
    for document in &documents {
        let json = moqui_lib::json::to_json(document);
        let document_id = json.get("_type").and_then(Json::as_str).unwrap_or_default().to_string();
        let record_id = json
            .get("_id")
            .map(|id| id.as_str().map_or_else(|| id.to_string(), str::to_string))
            .unwrap_or_default();
        if document_id.is_empty() {
            continue;
        }
        let origin = json.get("_originAgentId").and_then(Json::as_str).map(str::to_string);
        match quietly(|| store_events(facade, &document_id, &record_id, origin.as_deref(), &json)) {
            Ok(triggers) => {
                for trigger_id in triggers {
                    if !fire_now.contains(&trigger_id) {
                        fire_now.push(trigger_id);
                    }
                }
            }
            // one document that cannot be stored does not stop the others
            Err(error) => {
                tracing::error!("the agent trigger event of {document_id} {record_id} was not stored: {error}")
            }
        }
    }
    fire_after_commit(facade, fire_now);
    Ok(BTreeMap::new())
}

/// `delete#TriggerDocument`: a record of a trigger's document type is gone: an event `{_type, _id, _deleted}`.
fn delete_trigger_document(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let (Some(document_id), Some(record_id)) = (param(parameters, "dataDocumentId"), param(parameters, "documentId"))
    else {
        return Ok(BTreeMap::new());
    };
    let origin = param(parameters, "originAgentId");
    let event = json!({"_type": document_id, "_id": record_id, "_deleted": true});
    match quietly(|| store_events(facade, &document_id, &record_id, origin.as_deref(), &event)) {
        Ok(triggers) => fire_after_commit(facade, triggers),
        Err(error) => {
            tracing::error!("the agent trigger event of deleted {document_id} {record_id} was not stored: {error}")
        }
    }
    Ok(BTreeMap::new())
}

// ---- the breaker and the inbox ----

/// A failure of `trigger` (its run failed, or no run started): the count goes up, and at the limit the breaker opens,
/// its job pauses and the admins and the run-as user are told.
fn failed(
    factory: &Arc<ExecutionContextFactory>,
    facade: &EntityFacade,
    trigger: &Trigger,
    run_id: Option<&str>,
    why: &str,
) -> Result<(), Error> {
    let opened = run::in_tx(facade, || {
        let Some(mut row) = facade
            .find(TRIGGER)?
            .eq("triggerId", Value::string(&trigger.trigger_id))
            .for_update(true)
            .disable_authz()
            .one()?
        else {
            return Ok(false);
        };
        let now_trigger = Trigger::of(&row);
        let count = now_trigger.failure_count + 1;
        row.set("failureCount", Value::I64(count));
        row.set(
            "lastFailureText",
            Value::string(crate::llm::tools::format::cut(why, 1000)),
        );
        let opens = !now_trigger.breaker_open && count >= now_trigger.failure_limit;
        if opens {
            row.set("breakerState", Value::string("open"));
            row.set("breakerOpenedDate", run::stamp(run::now_millis()));
        }
        row.update(facade)?;
        if opens {
            sync_firing(facade, &Trigger::of(&row))?;
        }
        Ok(opens)
    })?;
    if opened {
        tracing::error!(
            "agent trigger {} failed {} times in a row, so its breaker is open: {why}",
            trigger.trigger_id,
            trigger.failure_count + 1
        );
        let mut users: Vec<String> = admin_users(facade).unwrap_or_default();
        if let Some(run_as) = &trigger.run_as_user_id {
            if !users.contains(run_as) {
                users.push(run_as.clone());
            }
        }
        inbox(
            factory,
            &users,
            &format!("Agent trigger {} stopped", trigger.trigger_id),
            json!({"kind": "breaker", "triggerId": trigger.trigger_id, "runId": run_id, "reason": why}),
        );
    }
    Ok(())
}

/// The users of the groups that have AgentAdmin.
fn admin_users(facade: &EntityFacade) -> Result<Vec<String>, Error> {
    let groups: Vec<Value> = facade
        .find("moqui.security.UserGroupPermission")?
        .eq("userPermissionId", Value::string("AgentAdmin"))
        .disable_authz()
        .list()?
        .values()
        .iter()
        .filter_map(|row| text_of(row, "userGroupId"))
        .map(Value::string)
        .collect();
    if groups.is_empty() {
        return Ok(Vec::new());
    }
    let mut users: Vec<String> = facade
        .find("moqui.security.UserGroupMember")?
        .condition(Condition::is_in("userGroupId", groups))
        .disable_authz()
        .list()?
        .values()
        .iter()
        .filter_map(|row| text_of(row, "userId"))
        .collect();
    users.sort();
    users.dedup();
    Ok(users)
}

/// A message to users' agent inbox (the `AgentInbox` topic, kept); a failure is logged.
pub fn inbox(_factory: &Arc<ExecutionContextFactory>, user_ids: &[String], title: &str, message: Json) {
    if user_ids.is_empty() {
        return;
    }
    let map = match moqui_lib::json::from_json(&message) {
        Value::Map(map) => map,
        _ => indexmap::IndexMap::new(),
    };
    let sent = crate::notify::NotificationMessage::new()
        .topic(INBOX_TOPIC)
        .user_ids(user_ids.iter().cloned())
        .title(title)
        .message_map(map)
        .send();
    if let Err(error) = sent {
        tracing::error!("the agent inbox message \"{title}\" could not be sent: {error}");
    }
}

/// The observer of background runs: the breaker of a trigger's run, and the inbox of a job-surface run (its result or
/// failure, and an approval it waits for).
pub(crate) fn observe(factory: &Arc<ExecutionContextFactory>, batch: &RunEvents) {
    let Some(facade) = factory.entity() else {
        return;
    };
    let relevant = batch
        .events
        .iter()
        .any(|(_, event_type, _)| event_type == "run.done" || event_type == "approval.requested");
    if !relevant {
        return;
    }
    let row = match facade
        .find(run::RUN)
        .and_then(|find| find.eq("runId", Value::string(&batch.run_id)).disable_authz().one())
    {
        Ok(Some(row)) => row,
        Ok(None) => return,
        Err(error) => {
            tracing::error!("trigger: agent run {} could not be read: {error}", batch.run_id);
            return;
        }
    };
    let run = run::RunRow::of(&row);
    if run.surface != Surface::Job {
        return;
    }
    let trigger_id = text_of(&row, "triggerId");
    for (_, event_type, payload) in &batch.events {
        match event_type.as_str() {
            "approval.requested" => {
                let summary = payload.get("summary").and_then(Json::as_str).unwrap_or_default();
                inbox(
                    factory,
                    std::slice::from_ref(&run.user_id),
                    &format!("Approval waiting: {}", crate::llm::tools::format::cut(summary, 120)),
                    json!({"kind": "approval", "runId": run.run_id, "conversationId": run.conversation_id,
                        "approvalId": payload.get("approvalId"), "triggerId": trigger_id, "summary": summary}),
                );
            }
            "run.done" => {
                let state = payload
                    .get("state")
                    .and_then(Json::as_str)
                    .and_then(RunState::from_wire);
                let text = payload
                    .get("result")
                    .and_then(|result| result.get("text"))
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_string();
                let error = text_of(&row, "errorText")
                    .or_else(|| text_of(&row, "statusReason"))
                    .unwrap_or_default();
                if let Some(trigger_id) = &trigger_id {
                    let outcome = quietly(|| -> Result<(), Error> {
                        let Some(trigger) = Trigger::load(&facade, trigger_id)? else {
                            return Ok(());
                        };
                        match state {
                            Some(RunState::Completed) => run::in_tx(&facade, || {
                                facade
                                    .find(TRIGGER)?
                                    .eq("triggerId", Value::string(trigger_id))
                                    .disable_authz()
                                    .update_all([
                                        ("failureCount".to_string(), Value::I64(0)),
                                        ("lastFailureText".to_string(), Value::Null),
                                    ])
                                    .map(|_| ())
                            }),
                            // a Rejected run is counted where it is started (fire)
                            Some(RunState::Failed) => failed(factory, &facade, &trigger, Some(&run.run_id), &error),
                            _ => Ok(()),
                        }
                    });
                    if let Err(error) = outcome {
                        tracing::error!("the breaker of agent trigger {trigger_id} was not updated: {error}");
                    }
                }
                let (title, summary) = match state {
                    Some(RunState::Completed) => ("finished", text),
                    Some(RunState::Canceled) => ("was canceled", String::new()),
                    _ => ("failed", error),
                };
                inbox(
                    factory,
                    std::slice::from_ref(&run.user_id),
                    &format!("Agent {} {title}", run.agent_id),
                    json!({"kind": "result", "runId": run.run_id, "conversationId": run.conversation_id,
                        "triggerId": trigger_id, "state": state.map(RunState::wire),
                        "summary": crate::llm::tools::format::cut(&summary, 600)}),
                );
            }
            _ => {}
        }
    }
}
