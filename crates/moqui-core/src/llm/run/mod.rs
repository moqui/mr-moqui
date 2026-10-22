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

//! Runs (plan 4.4): one user request carried to an end, across pauses, as a durable server job.
//!
//! [`start_run`] stores the run (Submitted), its user message, and takes the conversation's one active run with a
//! conditional update of `AgentConversation.activeRunId`; a message sent while a run is active becomes an `AgentInput`
//! the run folds in at its next step (steering). The run is a chain of step jobs on the agent pool ([`pool`]), each in
//! a fresh execution context logged in as the user ([`step`]): one model call and its tool calls, with no transaction
//! open across the call. The node working a run holds its lease ([`lease`]), renewed every 15 seconds with the
//! `cancelRequested` flag read back, and a recovery sweep takes over a run whose lease expired. A tool that waits for
//! someone pauses the run (InputRequired or AuthRequired): it gives up its worker and its lease, and the input that
//! arrives ([`submit_input`]) resumes it. [`cancel_run`] asks it to stop; the partial answer is kept.
//!
//! Every change is an event ([`events`]): stored ones in `AgentRunEvent`, live ones in the node's hub ([`hub`]), and
//! the low-rate signals on the `AgentRun` notification topic. [`state`] is the state table.

pub mod context;
pub mod events;
pub mod hub;
pub mod lease;
pub mod observe;
pub mod pool;
pub mod state;
pub mod step;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_db::sql::Condition;
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;
use rust_decimal::Decimal;
use serde_json::{json, Value as Json};

use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::{EntityFacade, EntityValue};
use crate::llm::conf::Tier;
use crate::llm::prompt::Surface;
use crate::llm::provider::CancelToken;
use crate::llm::tools::{ClientCaps, Pause, PauseKind};
use crate::llm::{decimal_of, flag_of, int_of, text_of};
use crate::transaction::TransactionAttribute;

pub use context::{AgentDef, Budget, RunLimits, TurnContext};
pub use events::{EventSink, StoredEvent};
pub use hub::RunHub;
pub use state::RunState;

pub(crate) const CONVERSATION: &str = "moqui.agent.AgentConversation";
pub(crate) const RUN: &str = events::RUN;
pub(crate) const MESSAGE: &str = "moqui.agent.AgentMessage";
pub(crate) const PART: &str = "moqui.agent.AgentMessagePart";
pub(crate) const INPUT: &str = "moqui.agent.AgentInput";
pub(crate) const TOOL_CALL: &str = "moqui.agent.AgentToolCall";
pub(crate) const QUESTION: &str = "moqui.agent.AgentQuestion";

/// A run is taken over at most this many times in all (its first attempt included).
pub const MAX_ATTEMPTS: i64 = 3;

/// A run this node works: its cancel token, whether a step job of it runs now, and whether another node took its
/// lease (a lost run writes nothing more and leaves its state alone).
#[derive(Default)]
pub(crate) struct LocalRun {
    pub(crate) cancel: CancelToken,
    pub(crate) busy: AtomicBool,
    pub(crate) lost: AtomicBool,
}

impl LocalRun {
    pub(crate) fn is_lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
    }
}

/// The factory's run engine: this node's id (the lease owner), the agent pool, the runs it works and the live hub.
pub struct Engine {
    node: String,
    pool: pool::AgentPool,
    local: Mutex<HashMap<String, Arc<LocalRun>>>,
    hub: Arc<RunHub>,
    started: AtomicBool,
    parallel_reads: usize,
}

impl Engine {
    /// The lease owner this node writes: the cluster instance id, else `host:pid`.
    pub fn node(&self) -> &str {
        &self.node
    }

    pub fn hub(&self) -> Arc<RunHub> {
        Arc::clone(&self.hub)
    }

    /// How many independent reads of a step run at once: up to 4, fewer when the connections the request, service
    /// and agent workers leave do not stretch that far (1 on the default Postgres pool).
    pub fn parallel_reads(&self) -> usize {
        self.parallel_reads
    }

    /// The run's local state; one whose lease was lost is replaced, since this node holds the lease again.
    pub(crate) fn local(&self, run_id: &str) -> Arc<LocalRun> {
        let mut local = lock(&self.local);
        let entry = local.entry(run_id.to_string()).or_default();
        if entry.is_lost() {
            *entry = Arc::default();
        }
        Arc::clone(entry)
    }

    /// Forget the run only when its local state is still `which`.
    pub(crate) fn forget_if(&self, run_id: &str, which: &Arc<LocalRun>) {
        let mut local = lock(&self.local);
        if local.get(run_id).is_some_and(|known| Arc::ptr_eq(known, which)) {
            local.remove(run_id);
        }
    }

    pub(crate) fn local_if(&self, run_id: &str) -> Option<Arc<LocalRun>> {
        lock(&self.local).get(run_id).cloned()
    }

    pub(crate) fn forget(&self, run_id: &str) {
        lock(&self.local).remove(run_id);
    }

    /// The runs this node works now.
    pub fn local_ids(&self) -> Vec<String> {
        lock(&self.local).keys().cloned().collect()
    }

    /// Whether this node works the run now (holds its lease).
    pub fn is_local(&self, run_id: &str) -> bool {
        lock(&self.local).contains_key(run_id)
    }
}

/// The factory's engine, made on first use with its pool (`limits` `max-concurrent-runs`, 8, and `run-queue`, 64).
pub fn engine(factory: &ExecutionContextFactory) -> Arc<Engine> {
    factory.extension(|factory| {
        let limits = crate::llm::facade(factory).conf().limits.clone();
        let node = crate::cluster::instance_id(factory).unwrap_or_else(|| {
            let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "localhost".to_string());
            format!("{host}:{}", std::process::id())
        });
        let spare = crate::service::spare_connections(factory);
        let pool = pool::AgentPool::start(&limits, spare);
        // a parallel read holds a connection of its own: each worker gets an even share of what the workers leave
        let parallel_reads = spare.map_or(step::PARALLEL_READS, |spare| {
            let workers = pool.workers().max(1);
            let left = usize::try_from(spare).unwrap_or(0).saturating_sub(workers);
            (1 + left / workers).clamp(1, step::PARALLEL_READS)
        });
        Engine {
            node,
            pool,
            parallel_reads,
            local: Mutex::new(HashMap::new()),
            hub: Arc::new(RunHub::new()),
            started: AtomicBool::new(false),
        }
    })
}

/// Start the engine's background work on a booted server: the lease keeper, the recovery sweep on the scheduler's
/// tick, and one sweep now. A second call does nothing.
pub fn start(factory: &Arc<ExecutionContextFactory>) {
    let engine = engine(factory);
    if engine.started.swap(true, Ordering::SeqCst) {
        return;
    }
    lease::start_keeper(factory);
    let weak = Arc::downgrade(factory);
    factory.service_pools().add_tick_hook(move || {
        if let Some(factory) = weak.upgrade() {
            lease::sweep_logged(&factory);
        }
    });
    lease::sweep_logged(factory);
}

pub(crate) fn now_millis() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

pub(crate) fn stamp(millis: i64) -> Value {
    crate::user::stamp_value(millis)
}

/// The entity facade of the factory.
pub(crate) fn facade_of(factory: &ExecutionContextFactory) -> Result<EntityFacade, Error> {
    factory
        .entity()
        .ok_or_else(|| Error::new("the entity facade is not installed"))
}

/// Run `body` with authorization off: the engine's own bookkeeping on the `moqui.agent` entities, whose owner checks
/// are the engine's.
pub(crate) fn quietly<T>(body: impl FnOnce() -> T) -> T {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    body()
}

/// `body` in a transaction of its own, or the one in place.
pub(crate) fn in_tx<T>(facade: &EntityFacade, body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    facade
        .transactions_for(RUN)?
        .run(TransactionAttribute::UseOrBegin, None, body)
}

/// A run's row as the engine reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct RunRow {
    pub run_id: String,
    pub conversation_id: String,
    pub agent_id: String,
    pub user_id: String,
    pub run_as_user_id: Option<String>,
    pub surface: Surface,
    pub caps: ClientCaps,
    pub state: RunState,
    pub pause_kind: Option<PauseKind>,
    pub pause_ref: Option<String>,
    pub lease_owner: Option<String>,
    pub lease_expire: Option<i64>,
    pub attempt: i64,
    pub cancel_requested: bool,
    pub step_count: u32,
    pub tool_call_count: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost: Decimal,
    pub limits: RunLimits,
    pub active_seconds: i64,
    /// The tier the client chose; the agent's own tier wins.
    pub tier: Option<Tier>,
}

impl RunRow {
    pub fn of(row: &EntityValue) -> Self {
        let count = |field: &str| int_of(row, field).and_then(|n| u32::try_from(n).ok()).unwrap_or(0);
        let tokens = |field: &str| int_of(row, field).and_then(|n| u64::try_from(n).ok()).unwrap_or(0);
        let positive_u32 = |field: &str| {
            int_of(row, field)
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| *n > 0)
        };
        Self {
            run_id: text_of(row, "runId").unwrap_or_default(),
            conversation_id: text_of(row, "conversationId").unwrap_or_default(),
            agent_id: text_of(row, "agentId").unwrap_or_default(),
            user_id: text_of(row, "userId").unwrap_or_default(),
            run_as_user_id: text_of(row, "runAsUserId"),
            surface: text_of(row, "surfaceEnumId")
                .and_then(|surface| Surface::parse(&surface))
                .unwrap_or_default(),
            caps: ClientCaps::from_text(&text_of(row, "capsText").unwrap_or_default()),
            state: text_of(row, "statusId")
                .and_then(|status| RunState::from_status_id(&status))
                .unwrap_or(RunState::Submitted),
            pause_kind: text_of(row, "pauseKindEnumId").and_then(|kind| PauseKind::from_enum_id(&kind)),
            pause_ref: text_of(row, "pauseRef"),
            lease_owner: text_of(row, "leaseOwner"),
            lease_expire: row.get("leaseExpireDate").and_then(crate::llm::millis_of),
            attempt: int_of(row, "attempt").unwrap_or(0),
            cancel_requested: flag_of(row, "cancelRequested") == Some(true),
            step_count: count("stepCount"),
            tool_call_count: count("toolCallCount"),
            input_tokens: tokens("inputTokens"),
            output_tokens: tokens("outputTokens"),
            cost: decimal_of(row, "cost").unwrap_or_default(),
            limits: RunLimits {
                max_steps: positive_u32("maxSteps"),
                max_tool_calls: positive_u32("maxToolCalls"),
                max_tokens: int_of(row, "maxTokens")
                    .and_then(|n| u64::try_from(n).ok())
                    .filter(|n| *n > 0),
                max_cost: decimal_of(row, "maxCost").filter(|cost| *cost > Decimal::ZERO),
                max_run_seconds: int_of(row, "maxRunSeconds")
                    .and_then(|n| u64::try_from(n).ok())
                    .filter(|n| *n > 0),
            },
            active_seconds: int_of(row, "activeSeconds").unwrap_or(0),
            tier: text_of(row, "tier").and_then(|tier| Tier::parse(&tier)),
        }
    }

    /// The user the tools run as: the run-as user of a job, else the run's user.
    pub fn acting_user(&self) -> &str {
        self.run_as_user_id.as_deref().unwrap_or(&self.user_id)
    }
}

/// The run `run_id`.
///
/// # Errors
///
/// Returns an error when the row cannot be read.
pub fn load_run(facade: &EntityFacade, run_id: &str) -> Result<Option<RunRow>, Error> {
    Ok(facade
        .find(RUN)?
        .eq("runId", Value::string(run_id))
        .disable_authz()
        .one()?
        .map(|row| RunRow::of(&row)))
}

/// The run's user, for an owner check; `None` when there is no such run.
///
/// # Errors
///
/// Returns an error when the row cannot be read.
pub fn run_owner(facade: &EntityFacade, run_id: &str) -> Result<Option<String>, Error> {
    Ok(load_run(facade, run_id)?.map(|run| run.user_id))
}

/// What a new run is.
#[derive(Clone, Debug, Default)]
pub struct StartRequest {
    pub user_id: String,
    /// `assist` when empty.
    pub agent_id: String,
    /// A new conversation when `None`.
    pub conversation_id: Option<String>,
    pub surface: Surface,
    pub text: String,
    /// The volatile `<context>` block of this message ([`crate::llm::prompt::build_tail`]).
    pub tail: Option<String>,
    /// The caller's id of the request: the same request again gets the same run.
    pub client_request_id: Option<String>,
    pub client_message_id: Option<String>,
    /// What the client can do, which lowers the surface's capabilities; `None` keeps them all.
    pub client_caps: Option<Vec<String>>,
    /// Capabilities an internal caller adds (a test's tools, a surface adapter); never from a client.
    pub extra_caps: Vec<String>,
    /// Caps the caller asks for, which only lower the agent's.
    pub limits: RunLimits,
    /// The tools run as the agent's `runAsUserId` instead of the user: for internal callers only (a trigger or a
    /// scheduled job), never from a service a user calls.
    pub run_as_agent_user: bool,
    /// The tier the client chose (the drawer's Fast, Balanced, Thorough); an agent that names its own keeps it, and a
    /// message queued to an active run leaves that run's as it is.
    pub tier: Option<Tier>,
    /// The screen state of a new conversation (the drawer's page when it sent the first message), so its first screen
    /// session opens where the user is.
    pub screen_state: Option<crate::llm::screen::ScreenState>,
    /// The new run is made but not handed to the agent pool: the caller does what must come first (the drawer's
    /// follow claim and page sync, only once the run is there) and then calls [`submit_started`].
    pub defer_submit: bool,
    /// Files the user attached to the message (the drawer's `attachments`): each the data of a `file` part,
    /// `{attachmentId, ref, fileName, mediaType, size, location}`, checked by the caller as the user's.
    pub attachments: Vec<Json>,
}

/// What [`start_run`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Started {
    pub run_id: String,
    pub conversation_id: String,
    pub state: RunState,
    /// A run was active, so the message went to it as input.
    pub queued: bool,
    /// The client request id named a run that exists, which is returned as it is.
    pub existing: bool,
}

/// Start a run of `request.agent_id` for `request.user_id`, or hand the message to the conversation's active run.
/// The caller has checked that the user may use the agent (`AgentUse`); here the agent must exist, not be disabled and
/// serve the surface, the user must not be external (`EXTERNAL_USER`), and the conversation must be the user's. A user
/// over their daily quota gets a Rejected run.
///
/// # Errors
///
/// Returns an error when the agent, the user or the conversation is not found or not allowed, or the rows cannot be
/// written.
pub fn start_run(factory: &Arc<ExecutionContextFactory>, request: &StartRequest) -> Result<Started, Error> {
    let facade = facade_of(factory)?;
    let engine = engine(factory);
    let llm = crate::llm::facade(factory);
    if !llm.enabled() {
        return Err(Error::new("the llm-facade is disabled"));
    }
    let agent_id = if request.agent_id.is_empty() {
        "assist"
    } else {
        request.agent_id.as_str()
    };
    let started = quietly(|| -> Result<Started, Error> {
        let agent = AgentDef::load(&facade, agent_id)?
            .filter(|agent| !agent.disabled)
            .ok_or_else(|| Error::new(format!("there is no agent {agent_id}")))?;
        if !agent.serves(request.surface) {
            return Err(Error::new(format!(
                "agent {agent_id} does not serve the {} surface",
                request.surface.as_str()
            )));
        }
        if request.user_id.is_empty() {
            return Err(Error::new("a run needs a user"));
        }
        // an external user (EXTERNAL_USER) runs no agent, whatever the caller checked
        if crate::user::is_external_user(&request.user_id) {
            return Err(Error::new("the agents are not available to this user"));
        }
        if let Some(existing) = existing_message(&facade, &request.user_id, request.client_message_id.as_deref())? {
            return Ok(existing);
        }
        if let Some(client_request_id) = &request.client_request_id {
            if let Some(row) = facade
                .find(RUN)?
                .eq("userId", Value::string(&request.user_id))
                .eq("clientRequestId", Value::string(client_request_id))
                .disable_authz()
                .one()?
            {
                let run = RunRow::of(&row);
                return Ok(Started {
                    run_id: run.run_id,
                    conversation_id: run.conversation_id,
                    state: run.state,
                    queued: false,
                    existing: true,
                });
            }
        }
        let mut caps = ClientCaps::for_surface(request.surface);
        if let Some(client) = &request.client_caps {
            caps = caps.lowered(client.iter().map(String::as_str));
        }
        // the screen tools need the host's screen sessions; a client may lower this as any other
        let screen_cap = crate::llm::tools::caps::SCREEN;
        if crate::llm::screen::installed()
            && request
                .client_caps
                .as_ref()
                .is_none_or(|client| client.iter().any(|cap| cap == screen_cap))
        {
            caps = caps.with([screen_cap]);
        }
        // the canvas dialects the agent does not write are off whatever the surface offers (Vue SFC is off by default)
        let caps = caps.dialects_only(|dialect| agent.allows_dialect(dialect));
        let caps = caps.with(request.extra_caps.iter().map(String::as_str));
        let limits = RunLimits::of(&agent, &llm.conf().limits, &request.limits);
        let quota =
            crate::llm::usage::check_quota(&facade, &llm.conf().limits, &request.user_id, "agent", now_millis())?;
        // an @name of a skill loads it after the message (plan 7.5)
        let mentions = request.text.contains('@').then(|| {
            crate::llm::skills::SkillIndex::for_audience(
                factory,
                &facade,
                &crate::llm::skills::Audience {
                    user_id: Some(&request.user_id),
                    agent_id: Some(&agent.agent_id),
                    surface: Some(request.surface),
                },
            )
        });
        // one try, and one more when the active run the conversation names turns out to have ended
        for _ in 0..2 {
            let outcome = in_tx(&facade, || {
                begin_run(
                    &facade,
                    &engine,
                    &agent,
                    request,
                    &caps,
                    &limits,
                    quota.exceeded.as_deref(),
                    lease::lease_millis(factory),
                    mentions.as_ref(),
                )
            })?;
            match outcome {
                Begun::Started(started) => return Ok(started),
                Begun::Queued {
                    run_id,
                    conversation_id,
                    state,
                } => {
                    if state.is_paused() {
                        resume(factory, &facade, &run_id)?;
                    }
                    return Ok(Started {
                        run_id,
                        conversation_id,
                        state,
                        queued: true,
                        existing: false,
                    });
                }
                Begun::Stale => continue,
            }
        }
        Err(Error::new("the conversation's active run could not be settled"))
    })?;
    if !started.queued && !started.existing {
        let rejected = started.state == RunState::Rejected;
        let mut sink = EventSink::new(factory, &facade, &started.run_id, &request.user_id);
        let mut payload = json!({"runId": started.run_id, "conversationId": started.conversation_id,
            "state": started.state.wire(), "statusId": started.state.status_id()});
        if rejected {
            payload["errorCode"] = json!("quota_exhausted");
        }
        sink.emit("run.state", payload.clone());
        if rejected {
            sink.emit(
                "run.done",
                json!({"runId": started.run_id, "state": started.state.wire(), "errorCode": "quota_exhausted"}),
            );
        }
        sink.signal(payload);
        quietly(|| sink.flush_logged());
    }
    if !request.defer_submit {
        submit_started(factory, &started)?;
    }
    Ok(started)
}

/// Hand a run [`start_run`] made to the agent pool (once; a queued message, an existing run or a rejected one has
/// nothing to hand). A run that cannot be queued is released for the recovery sweep.
///
/// # Errors
///
/// Returns an error when the lease of a run the pool refused cannot be released.
pub fn submit_started(factory: &Arc<ExecutionContextFactory>, started: &Started) -> Result<(), Error> {
    if started.queued || started.existing || started.state != RunState::Submitted {
        return Ok(());
    }
    let facade = facade_of(factory)?;
    submit_when_committed(factory, &facade, &started.run_id)
}

/// Queue the run's next step on the agent pool once the rows written for it are committed: with a transaction in
/// place on this thread (a service that started the run, a chat agent's run under `deliver#AgentMessage`), after it
/// commits, since a worker that reads the run before then does not see it (or sees it still paused) and stops; else
/// now. A run the pool refuses is released for the recovery sweep.
///
/// # Errors
///
/// Returns an error when the commit hook cannot be registered, or (now) the lease of a refused run cannot be released.
fn submit_when_committed(
    factory: &Arc<ExecutionContextFactory>,
    facade: &EntityFacade,
    run_id: &str,
) -> Result<(), Error> {
    let submit = {
        let factory = Arc::clone(factory);
        let facade = facade.clone();
        let run_id = run_id.to_string();
        move || -> Result<(), Error> {
            let engine = engine(&factory);
            engine.local(&run_id);
            if let Err(error) = pool::submit(&factory, &run_id) {
                tracing::error!("agent run {run_id} could not be queued, so the recovery sweep will start it: {error}");
                quietly(|| lease::release(&facade, &run_id, engine.node()))?;
                engine.forget(&run_id);
            }
            Ok(())
        }
    };
    let tx = facade.transactions_for(RUN)?;
    if !tx.in_transaction() {
        return submit();
    }
    let run_id = run_id.to_string();
    tx.on_commit(move || {
        if let Err(error) = submit() {
            tracing::error!("the lease of agent run {run_id} the pool refused could not be given up: {error}");
        }
    })
}

enum Begun {
    Started(Started),
    Queued {
        run_id: String,
        conversation_id: String,
        state: RunState,
    },
    /// The conversation named an active run that had ended; it was cleared.
    Stale,
}

#[allow(clippy::too_many_arguments)]
fn begin_run(
    facade: &EntityFacade,
    engine: &Engine,
    agent: &AgentDef,
    request: &StartRequest,
    caps: &ClientCaps,
    limits: &RunLimits,
    over_quota: Option<&str>,
    lease: i64,
    mentions: Option<&crate::llm::skills::SkillIndex>,
) -> Result<Begun, Error> {
    let now = now_millis();
    let conversation_id = match &request.conversation_id {
        Some(conversation_id) => {
            let row = facade
                .find(CONVERSATION)?
                .eq("conversationId", Value::string(conversation_id))
                .for_update(true)
                .disable_authz()
                .one()?
                .filter(|row| text_of(row, "userId").as_deref() == Some(request.user_id.as_str()))
                .ok_or_else(|| Error::new(format!("there is no agent conversation {conversation_id}")))?;
            if let Some(active) = text_of(&row, "activeRunId") {
                let state = load_run(facade, &active)?.map(|run| run.state);
                match state {
                    Some(state) if !state.is_terminal() => {
                        let attached = (!request.attachments.is_empty())
                            .then(|| json!({"attachments": request.attachments}));
                        add_input_with(
                            facade,
                            &active,
                            &request.user_id,
                            "AgikMessage",
                            &request.text,
                            attached.as_ref(),
                            request.tail.as_deref(),
                            request.client_message_id.as_deref(),
                        )?;
                        return Ok(Begun::Queued {
                            run_id: active,
                            conversation_id: conversation_id.clone(),
                            state,
                        });
                    }
                    _ => {
                        clear_active(facade, conversation_id, &active)?;
                        return Ok(Begun::Stale);
                    }
                }
            }
            conversation_id.clone()
        }
        None => {
            let mut row = facade.make_value(CONVERSATION)?;
            row.set_sequenced_id_primary(facade)?;
            row.set("userId", Value::string(&request.user_id));
            row.set("agentId", Value::string(&agent.agent_id));
            row.set("surfaceEnumId", Value::string(request.surface.enum_id()));
            row.set("statusId", Value::string("AgcActive"));
            row.set("lastMessageSeq", Value::I64(0));
            row.set("createdDate", stamp(now));
            if let Some(state) = &request.screen_state {
                if let Ok(text) = serde_json::to_string(state) {
                    row.set("screenStateJson", Value::string(text));
                }
            }
            row.create(facade)?;
            text_of(&row, "conversationId").unwrap_or_default()
        }
    };
    let state = if over_quota.is_some() {
        RunState::Rejected
    } else {
        RunState::Submitted
    };
    let mut run = facade.make_value(RUN)?;
    run.set_sequenced_id_primary(facade)?;
    let run_id = text_of(&run, "runId").unwrap_or_default();
    for (field, value) in [
        ("conversationId", Value::string(&conversation_id)),
        ("agentId", Value::string(&agent.agent_id)),
        ("userId", Value::string(&request.user_id)),
        ("surfaceEnumId", Value::string(request.surface.enum_id())),
        ("capsText", Value::string(caps.to_text())),
        ("statusId", Value::string(state.status_id())),
        ("attempt", Value::I64(1)),
        ("cancelRequested", Value::string("N")),
        ("lastEventSeq", Value::I64(0)),
        ("stepCount", Value::I64(0)),
        ("toolCallCount", Value::I64(0)),
        ("submittedDate", stamp(now)),
    ] {
        run.set(field, value);
    }
    // a job's tools run as the agent's run-as user, only when an internal caller asks for it
    if request.run_as_agent_user {
        if let Some(run_as) = &agent.run_as_user_id {
            run.set("runAsUserId", Value::string(run_as));
        }
    }
    if let Some(client_request_id) = &request.client_request_id {
        run.set("clientRequestId", Value::string(client_request_id));
    }
    // the client's tier, unless the agent names its own
    if let Some(tier) = request.tier.filter(|_| agent.tier.is_none()) {
        run.set("tier", Value::string(tier.as_str()));
    }
    let set_limit = |run: &mut EntityValue, field: &str, value: Option<Value>| {
        if let Some(value) = value {
            run.set(field, value);
        }
    };
    set_limit(&mut run, "maxSteps", limits.max_steps.map(|n| Value::I64(i64::from(n))));
    set_limit(
        &mut run,
        "maxToolCalls",
        limits.max_tool_calls.map(|n| Value::I64(i64::from(n))),
    );
    set_limit(
        &mut run,
        "maxTokens",
        limits.max_tokens.and_then(|n| i64::try_from(n).ok()).map(Value::I64),
    );
    set_limit(&mut run, "maxCost", limits.max_cost.map(Value::Decimal));
    set_limit(
        &mut run,
        "maxRunSeconds",
        limits
            .max_run_seconds
            .and_then(|n| i64::try_from(n).ok())
            .map(Value::I64),
    );
    if let Some(reason) = over_quota {
        run.set("statusReason", Value::string(reason));
        run.set("errorCode", Value::string("quota_exhausted"));
        run.set("endedDate", stamp(now));
    } else {
        run.set("leaseOwner", Value::string(engine.node()));
        run.set("leaseExpireDate", stamp(now + lease));
    }
    run.create(facade)?;
    if state == RunState::Submitted && !take_active(facade, &conversation_id, &run_id)? {
        // another start took the conversation between the read and now
        run.delete(facade)?;
        return Ok(Begun::Stale);
    }
    // the AT_LLM tarpit counts a run where it is made, under the conversation's lock: never a message handed to the
    // active run, a step, a start that found the conversation taken, or a send again (J25); a refusal rolls this back
    count_run_hit(&agent.agent_id)?;
    let metadata = request
        .tail
        .as_ref()
        .filter(|tail| !tail.is_empty())
        .map(|tail| json!({"tail": tail}));
    step::append_message(
        facade,
        &conversation_id,
        Some(&run_id),
        "user",
        Some(&request.user_id),
        request.client_message_id.as_deref(),
        &step::message_parts(&request.text, &request.attachments),
        metadata.as_ref(),
        None,
    )?;
    if let (Some(index), RunState::Submitted) = (mentions, state) {
        crate::llm::skills::mention::append(
            facade,
            index,
            &conversation_id,
            &run_id,
            &request.user_id,
            request.surface,
            &request.text,
        )?;
    }
    Ok(Begun::Started(Started {
        run_id,
        conversation_id,
        state,
        queued: false,
        existing: false,
    }))
}

/// Take the conversation's one active run for `run_id`, with a conditional update: true when it was free.
///
/// # Errors
///
/// Returns an error when the row cannot be written.
pub fn take_active(facade: &EntityFacade, conversation_id: &str, run_id: &str) -> Result<bool, Error> {
    in_tx(facade, || take_active_in_tx(facade, conversation_id, run_id))
}

fn take_active_in_tx(facade: &EntityFacade, conversation_id: &str, run_id: &str) -> Result<bool, Error> {
    let updated = facade
        .find(CONVERSATION)?
        .eq("conversationId", Value::string(conversation_id))
        .condition(Condition::or(vec![
            Condition::is_null("activeRunId"),
            Condition::equals("activeRunId", Value::string(run_id)),
        ]))
        .for_update(true)
        .disable_authz()
        .update_all([("activeRunId".to_string(), Value::string(run_id))])?;
    if updated > 0 {
        return Ok(true);
    }
    // the row may already name this run, which update_all leaves unchanged
    Ok(facade
        .find(CONVERSATION)?
        .eq("conversationId", Value::string(conversation_id))
        .disable_authz()
        .one()?
        .and_then(|row| text_of(&row, "activeRunId"))
        .as_deref()
        == Some(run_id))
}

/// Clear the conversation's active run when it is `run_id`.
///
/// # Errors
///
/// Returns an error when the row cannot be written.
pub fn clear_active(facade: &EntityFacade, conversation_id: &str, run_id: &str) -> Result<(), Error> {
    in_tx(facade, || {
        facade
            .find(CONVERSATION)?
            .eq("conversationId", Value::string(conversation_id))
            .eq("activeRunId", Value::string(run_id))
            .for_update(true)
            .disable_authz()
            .update_all([("activeRunId".to_string(), Value::Null)])?;
        Ok(())
    })
}

/// Store input for a run, in the transaction in place: its kind (`AgikMessage`, `AgikAnswer`, `AgikDecision`,
/// `AgikRemote`), text and data, and the `<context>` tail of a message.
pub(crate) fn add_input(
    facade: &EntityFacade,
    run_id: &str,
    user_id: &str,
    kind_enum_id: &str,
    text: &str,
    data: Option<&Json>,
    tail: Option<&str>,
) -> Result<String, Error> {
    add_input_with(facade, run_id, user_id, kind_enum_id, text, data, tail, None)
}

/// [`add_input`] of a message with the client's own id of it, which the folded message keeps.
#[allow(clippy::too_many_arguments)]
pub(crate) fn add_input_with(
    facade: &EntityFacade,
    run_id: &str,
    user_id: &str,
    kind_enum_id: &str,
    text: &str,
    data: Option<&Json>,
    tail: Option<&str>,
    client_message_id: Option<&str>,
) -> Result<String, Error> {
    let mut row = facade.make_value(INPUT)?;
    row.set_sequenced_id_primary(facade)?;
    row.set("runId", Value::string(run_id));
    row.set("inputKindEnumId", Value::string(kind_enum_id));
    row.set("userId", Value::string(user_id));
    row.set("messageText", Value::string(text));
    let mut data = data.cloned();
    if let Some(tail) = tail.filter(|tail| !tail.is_empty()) {
        let mut object = match data.take() {
            Some(Json::Object(map)) => map,
            Some(other) => serde_json::Map::from_iter([("value".to_string(), other)]),
            None => serde_json::Map::new(),
        };
        object.insert("tail".to_string(), json!(tail));
        data = Some(Json::Object(object));
    }
    if let Some(data) = data {
        row.set("dataJson", Value::string(data.to_string()));
    }
    if let Some(client_message_id) = client_message_id {
        row.set("clientMessageId", Value::string(client_message_id));
    }
    row.set("receivedDate", stamp(now_millis()));
    row.create(facade)?;
    Ok(text_of(&row, "inputId").unwrap_or_default())
}

/// What a message the user sent before with `client_message_id` became: the run it started or went to, so the same
/// send again changes nothing. `None` when there is none (or no id).
///
/// # Errors
///
/// Returns an error when the rows cannot be read.
pub fn existing_message(
    facade: &EntityFacade,
    user_id: &str,
    client_message_id: Option<&str>,
) -> Result<Option<Started>, Error> {
    let Some(client_message_id) = client_message_id.filter(|id| !id.is_empty()) else {
        return Ok(None);
    };
    let message = facade
        .find(MESSAGE)?
        .eq("userId", Value::string(user_id))
        .eq("clientMessageId", Value::string(client_message_id))
        .disable_authz()
        .one()?;
    let (run_id, queued) = match message.and_then(|row| text_of(&row, "runId")) {
        Some(run_id) => {
            // a message folded into a run that was active when it came was queued
            let folded = facade
                .find(INPUT)?
                .eq("userId", Value::string(user_id))
                .eq("clientMessageId", Value::string(client_message_id))
                .disable_authz()
                .count()?
                > 0;
            (run_id, folded)
        }
        None => match facade
            .find(INPUT)?
            .eq("userId", Value::string(user_id))
            .eq("clientMessageId", Value::string(client_message_id))
            .disable_authz()
            .one()?
            .and_then(|row| text_of(&row, "runId"))
        {
            Some(run_id) => (run_id, true),
            None => return Ok(None),
        },
    };
    Ok(load_run(facade, &run_id)?.map(|run| Started {
        run_id: run.run_id,
        conversation_id: run.conversation_id,
        state: run.state,
        queued,
        existing: true,
    }))
}

/// Count a new run of `agent_id` against the user's AT_LLM tarpit (`agent:<agentId>`): one hit per run, never per
/// step or per message. A user over the limit is refused with the seconds to wait.
///
/// # Errors
///
/// Returns the tarpit's refusal ([`crate::context::ErrorKind::Tarpit`], with `retry_after`).
pub fn count_run_hit(agent_id: &str) -> Result<(), Error> {
    let name = format!("agent:{agent_id}");
    ExecutionContext::with_current(|ec| -> Result<(), Error> {
        ec.push_checked("AT_LLM", &name, "view", false, true, None)?;
        let _ = ec.pop();
        Ok(())
    })
    .unwrap_or(Ok(()))
}

/// Input for a run from its user: a message (folded in as steering while the run works), an answer to its question,
/// a decision, or a remote agent's reply. A paused run resumes with it. `kind` is the `AgentInputKind` `enumCode`.
///
/// # Errors
///
/// Returns an error when there is no such run of the user, it has ended, or the rows cannot be written.
pub fn submit_input(
    factory: &Arc<ExecutionContextFactory>,
    run_id: &str,
    user_id: &str,
    kind: &str,
    text: &str,
    data: Option<&Json>,
) -> Result<RunState, Error> {
    let facade = facade_of(factory)?;
    let kind_enum_id = match kind {
        "message" => "AgikMessage",
        "answer" => "AgikAnswer",
        // an approval is decided only through decide#AgentApproval, by its user in an interactive session
        "decision" => {
            return Err(Error::new(
                "an approval is decided with decide#AgentApproval, not as run input",
            ))
        }
        "remote" => "AgikRemote",
        other => return Err(Error::new(format!("{other} is not a kind of run input"))),
    };
    quietly(|| {
        let run = load_run(&facade, run_id)?
            .filter(|run| run.user_id == user_id)
            .ok_or_else(|| Error::new(format!("there is no agent run {run_id}")))?;
        if run.state.is_terminal() {
            return Err(Error::new(format!("agent run {run_id} has ended")));
        }
        // a form's answer must fit the form before the run takes it
        if kind == "answer" && run.pause_kind == Some(PauseKind::Question) {
            if let Some(question_id) = &run.pause_ref {
                crate::llm::tools::ask::check_answer(&facade, question_id, data).map_err(Error::new)?;
            }
        }
        in_tx(&facade, || {
            add_input(&facade, run_id, user_id, kind_enum_id, text, data, None)
        })?;
        // a run waiting for an approval or a hand-off goes on only with its decision or the user's save; a message
        // meanwhile waits for the step after it
        let decided_elsewhere = matches!(run.pause_kind, Some(PauseKind::Approval | PauseKind::Handoff));
        if run.state.is_paused() && !decided_elsewhere {
            resume(factory, &facade, run_id)?;
        }
        Ok(load_run(&facade, run_id)?.map_or(run.state, |run| run.state))
    })
}

/// A paused run goes on with the input the runtime gave it (an approval's decision, a hand-off's outcome); one that is
/// not paused is left as it is.
///
/// # Errors
///
/// Returns an error when the run cannot be resumed (another node holds it, the rows cannot be written).
pub(crate) fn resume_paused(
    factory: &Arc<ExecutionContextFactory>,
    facade: &EntityFacade,
    run_id: &str,
) -> Result<(), Error> {
    resume(factory, facade, run_id)
}

/// A paused run goes on: a budget pause is decided here (continue raises the caps it reached; anything else ends the
/// run), any other pause moves the run to Working and queues a step, which resumes the paused tool call with the
/// input.
fn resume(factory: &Arc<ExecutionContextFactory>, facade: &EntityFacade, run_id: &str) -> Result<(), Error> {
    let Some(run) = load_run(facade, run_id)? else {
        return Ok(());
    };
    if !run.state.is_paused() {
        return Ok(());
    }
    // an MCP call's run has no model loop: it goes on with the client's retry of the call (MRTR), which reads the
    // decision (crate::llm::mcp)
    if run.surface == Surface::Mcp {
        return Ok(());
    }
    let engine = engine(factory);
    let mut sink = EventSink::new(factory, facade, run_id, &run.user_id);
    if run.pause_kind == Some(PauseKind::Budget) {
        let inputs = step::open_inputs(facade, run_id)?;
        let go_on = inputs.iter().any(|input| input.kind == "message" || affirmative(input));
        if let Some(question_id) = &run.pause_ref {
            step::answer_question(facade, question_id, &json!({"continue": go_on}))?;
            sink.emit(
                "question.answered",
                json!({"questionId": question_id, "continue": go_on}),
            );
        }
        // an answer is used up here; a message stays for the next step to fold in
        for input in inputs.iter().filter(|input| input.kind != "message") {
            step::consume_input(facade, &input.input_id)?;
        }
        if !go_on {
            return transition(
                facade,
                &mut sink,
                run_id,
                RunState::Canceled,
                &Change::reason("the user stopped the run at its budget").unleased(),
            );
        }
        let agent = AgentDef::load(facade, &run.agent_id)?.unwrap_or_default();
        let base = RunLimits::of(
            &agent,
            &crate::llm::facade(factory).conf().limits,
            &RunLimits::default(),
        );
        in_tx(facade, || raise_caps(facade, &run, &base))?;
    }
    if !lease::claim(facade, run_id, engine.node(), lease::lease_millis(factory))? {
        return Err(Error::new(format!("agent run {run_id} is held by another node")));
    }
    transition(facade, &mut sink, run_id, RunState::Working, &Change::default())?;
    submit_when_committed(factory, facade, run_id)
}

fn affirmative(input: &step::OpenInput) -> bool {
    if let Some(data) = &input.data {
        if let Some(flag) = data.get("continue").and_then(Json::as_bool) {
            return flag;
        }
        if let Some(choice) = data.get("choice").and_then(Json::as_str) {
            return choice.eq_ignore_ascii_case("continue");
        }
    }
    matches!(
        input.text.trim().to_ascii_lowercase().as_str(),
        "continue" | "yes" | "y" | "ok" | "go on"
    )
}

/// Raise each cap the run has reached by its base amount.
fn raise_caps(facade: &EntityFacade, run: &RunRow, base: &RunLimits) -> Result<(), Error> {
    let Some(mut row) = facade
        .find(RUN)?
        .eq("runId", Value::string(&run.run_id))
        .for_update(true)
        .disable_authz()
        .one()?
    else {
        return Ok(());
    };
    let caps = &run.limits;
    if let (Some(max), Some(add)) = (caps.max_steps, base.max_steps) {
        if run.step_count >= max {
            row.set("maxSteps", Value::I64(i64::from(max + add)));
        }
    }
    if let Some(max) = caps.max_tool_calls {
        if run.tool_call_count >= max {
            row.set(
                "maxToolCalls",
                Value::I64(i64::from(max + base.max_tool_calls.unwrap_or(max))),
            );
        }
    }
    if let Some(max) = caps.max_tokens {
        if run.input_tokens + run.output_tokens >= max {
            let add = base.max_tokens.unwrap_or(max);
            row.set("maxTokens", Value::I64(i64::try_from(max + add).unwrap_or(i64::MAX)));
        }
    }
    if let Some(max) = caps.max_cost {
        if run.cost >= max {
            row.set("maxCost", Value::Decimal(max + base.max_cost.unwrap_or(max)));
        }
    }
    if let (Some(max), Some(add)) = (caps.max_run_seconds, base.max_run_seconds) {
        if u64::try_from(run.active_seconds).unwrap_or(0) >= max {
            row.set(
                "maxRunSeconds",
                Value::I64(i64::try_from(max + add).unwrap_or(i64::MAX)),
            );
        }
    }
    row.update(facade)?;
    Ok(())
}

/// Ask a run to stop (cancel, or the drawer's Stop, which keeps the partial answer: the text streamed so far is
/// stored either way). A run that holds no worker (submitted with none, or paused) is Canceled now; a run this node
/// works hears it at once through its token, one another node works at that node's next renewal. A service call
/// already running finishes; nothing after it starts. `user_id` is the owner, checked when given.
///
/// # Errors
///
/// Returns an error when there is no such run (of the user), or the rows cannot be written.
pub fn cancel_run(
    factory: &Arc<ExecutionContextFactory>,
    run_id: &str,
    user_id: Option<&str>,
) -> Result<RunState, Error> {
    let facade = facade_of(factory)?;
    let engine = engine(factory);
    quietly(|| {
        let run = load_run(&facade, run_id)?
            .filter(|run| user_id.is_none_or(|user_id| run.user_id == user_id))
            .ok_or_else(|| Error::new(format!("there is no agent run {run_id}")))?;
        if run.state.is_terminal() {
            return Ok(run.state);
        }
        in_tx(&facade, || {
            facade
                .find(RUN)?
                .eq("runId", Value::string(run_id))
                .for_update(true)
                .disable_authz()
                .update_all([("cancelRequested".to_string(), Value::string("Y"))])
        })?;
        let local = engine.local_if(run_id);
        if let Some(local) = &local {
            local.cancel.cancel();
        }
        let idle = run.state.is_paused()
            || (run.state == RunState::Submitted
                && local.as_ref().is_none_or(|local| !local.busy.load(Ordering::SeqCst))
                && run.lease_owner.as_deref().is_none_or(|owner| owner == engine.node()));
        if idle {
            let mut sink = EventSink::new(factory, &facade, run_id, &run.user_id);
            transition(
                &facade,
                &mut sink,
                run_id,
                RunState::Canceled,
                &Change::reason("canceled").unleased(),
            )?;
            engine.forget(run_id);
        }
        Ok(load_run(&facade, run_id)?.map_or(run.state, |run| run.state))
    })
}

/// Wait up to `timeout` for the run to end or pause, with no step of it running here. Out: its state then.
///
/// # Errors
///
/// Returns an error when there is no such run or it cannot be read.
pub fn wait_for(factory: &Arc<ExecutionContextFactory>, run_id: &str, timeout: Duration) -> Result<RunState, Error> {
    let facade = facade_of(factory)?;
    let engine = engine(factory);
    let deadline = Instant::now() + timeout;
    loop {
        let run = quietly(|| load_run(&facade, run_id))?
            .ok_or_else(|| Error::new(format!("there is no agent run {run_id}")))?;
        let settled = (run.state.is_terminal() || run.state.is_paused())
            && engine
                .local_if(run_id)
                .is_none_or(|local| !local.busy.load(Ordering::SeqCst));
        if settled || Instant::now() >= deadline {
            return Ok(run.state);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// How a write needs the run's lease. Every write of the engine reads the run row for update in its transaction and
/// checks its `leaseOwner` first, so a node that lost the lease to another one writes nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Hold {
    /// This node holds it: a step's writes.
    #[default]
    Lease,
    /// This node holds it or no one does: a run with no worker (paused, ended, not started), and the writes after a
    /// transition gave the lease up in the same transaction.
    LeaseOrNone,
}

/// How the error of a write refused for a lost lease starts.
pub const LEASE_LOST: &str = "lease lost:";

/// Whether `error` is a write refused because another node holds the run's lease.
pub fn is_lease_lost(error: &Error) -> bool {
    error.message().contains(LEASE_LOST)
}

/// The run's row, read for update in the transaction in place, when `node` may write it as `hold` says.
///
/// # Errors
///
/// Returns an error when there is no such run, or another node holds its lease ([`LEASE_LOST`]).
pub(crate) fn held(facade: &EntityFacade, run_id: &str, node: &str, hold: Hold) -> Result<EntityValue, Error> {
    let row = facade
        .find(RUN)?
        .eq("runId", Value::string(run_id))
        .for_update(true)
        .disable_authz()
        .one()?
        .ok_or_else(|| Error::new(format!("there is no agent run {run_id}")))?;
    let owner = text_of(&row, "leaseOwner");
    match (owner.as_deref(), hold) {
        (Some(owner), _) if owner == node => Ok(row),
        (None, Hold::LeaseOrNone) => Ok(row),
        (owner, _) => Err(Error::new(format!(
            "{LEASE_LOST} agent run {run_id} is held by {}, not by {node}",
            owner.unwrap_or("no one")
        ))),
    }
}

/// What a transition also records.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Change {
    pub reason: Option<String>,
    pub pause: Option<Pause>,
    pub error: Option<(String, String)>,
    pub result: Option<Json>,
    pub hold: Hold,
    /// Leave the run as it is when a message for it is waiting (a run about to complete goes on to fold it in),
    /// checked under the conversation's lock, which a message queued to the run takes first.
    pub unless_input: bool,
}

impl Change {
    pub(crate) fn reason(reason: &str) -> Self {
        Self {
            reason: Some(reason.to_string()),
            ..Self::default()
        }
    }

    pub(crate) fn failed(code: &str, text: &str) -> Self {
        Self {
            reason: Some(text.to_string()),
            error: Some((code.to_string(), text.to_string())),
            ..Self::default()
        }
    }

    /// For a run that holds no lease, or this node's.
    pub(crate) fn unleased(mut self) -> Self {
        self.hold = Hold::LeaseOrNone;
        self
    }
}

/// Move a run to `to`, as the state table allows (a move to the state it is in does nothing), with its events and
/// signal, in one transaction (or the one in place). Leaving Working for a pause or an end gives up the lease; an end
/// clears the conversation's active run and closes the tool calls it left waiting.
///
/// # Errors
///
/// Returns an error when the table does not allow the move or the rows cannot be written.
pub(crate) fn transition(
    facade: &EntityFacade,
    sink: &mut EventSink,
    run_id: &str,
    to: RunState,
    change: &Change,
) -> Result<(), Error> {
    in_tx(facade, || {
        let mut row = held(facade, run_id, sink.node(), change.hold)?;
        let run = RunRow::of(&row);
        if run.state == to {
            return Ok(());
        }
        run.state.move_to(to)?;
        if change.unless_input {
            // a message queued meanwhile holds the conversation until its input is stored: wait for it, then look
            facade
                .find(CONVERSATION)?
                .eq("conversationId", Value::string(&run.conversation_id))
                .for_update(true)
                .disable_authz()
                .one()?;
            if step::open_inputs(facade, run_id)?.iter().any(|input| input.kind == "message") {
                return Ok(());
            }
        }
        let now = now_millis();
        row.set("statusId", Value::string(to.status_id()));
        row.set(
            "statusReason",
            change.reason.as_ref().map_or(Value::Null, Value::string),
        );
        match &change.pause {
            Some(pause) => {
                row.set("pauseKindEnumId", Value::string(pause.kind.enum_id()));
                row.set("pauseRef", Value::string(&pause.reference));
                row.set("pauseExpireDate", pause.expires.map_or(Value::Null, stamp));
            }
            None => {
                row.set("pauseKindEnumId", Value::Null);
                row.set("pauseRef", Value::Null);
                row.set("pauseExpireDate", Value::Null);
            }
        }
        if to == RunState::Working && run.state == RunState::Submitted {
            row.set("startedDate", stamp(now));
        }
        if let Some((code, text)) = &change.error {
            row.set("errorCode", Value::string(code));
            row.set("errorText", Value::string(text));
        }
        if let Some(result) = &change.result {
            row.set("resultJson", Value::string(result.to_string()));
        }
        if to.is_terminal() || to.is_paused() {
            row.set("leaseOwner", Value::Null);
            row.set("leaseExpireDate", Value::Null);
        }
        if to.is_terminal() {
            row.set("endedDate", stamp(now));
        }
        row.update(facade)?;
        if to.is_terminal() {
            clear_active(facade, &run.conversation_id, run_id)?;
            step::close_open_calls(facade, run_id, sink)?;
            // what it left waiting can no longer be decided or saved for it
            crate::llm::approval::expire_for_run(facade, sink, run_id)?;
        }
        let mut payload = json!({"runId": run_id, "conversationId": run.conversation_id, "state": to.wire(),
            "statusId": to.status_id(), "reason": change.reason});
        if let Some(pause) = &change.pause {
            payload["pauseKind"] = json!(pause.kind.enum_id());
            payload["pauseRef"] = json!(pause.reference);
        }
        if let Some((code, _)) = &change.error {
            payload["errorCode"] = json!(code);
        }
        sink.emit("run.state", payload.clone());
        if to.is_terminal() {
            sink.emit(
                "run.done",
                json!({"runId": run_id, "state": to.wire(), "result": change.result, "errorCode": change.error.as_ref().map(|(code, _)| code)}),
            );
        }
        if to != RunState::Working || run.state == RunState::Submitted {
            sink.signal(payload);
        }
        sink.flush()
    })
}
