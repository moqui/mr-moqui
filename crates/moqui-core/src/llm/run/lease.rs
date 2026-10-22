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

//! Leases and recovery (plan 4.4). The node working a run holds its lease: `AgentRun.leaseOwner` (this node's id) and
//! `leaseExpireDate` (the conf's `lease-seconds`, 60), taken with a conditional update as `ServiceJobRunLock` is, so
//! two nodes never both take one. The keeper thread renews the leases of every local run every `heartbeat-seconds`
//! (15) and reads their `cancelRequested` back, so a cancel issued on another node reaches the local token; a lease
//! another node took cancels the local token too.
//!
//! The recovery sweep (on the scheduler's tick, and once at startup) takes over a Submitted or Working run whose lease
//! expired or that has none: at most [`MAX_ATTEMPTS`](super::MAX_ATTEMPTS) attempts in all, then it fails. The run
//! resumes from its last stored step; a tool call left running becomes `UnknownOutcome` ([`super::step`]).

use std::sync::Arc;
use std::time::Duration;

use moqui_db::sql::Condition;
use moqui_lib::error::Error;
use moqui_lib::object::Value;

use super::{engine, facade_of, now_millis, quietly, stamp, Change, EventSink, RunRow, RunState, RUN};
use crate::context::ExecutionContextFactory;
use crate::entity::EntityFacade;
use crate::transaction::TransactionAttribute;

/// The lease of a run, from the conf's `lease-seconds` (60; at least 5).
pub(crate) fn lease_millis(factory: &ExecutionContextFactory) -> i64 {
    i64::from(crate::llm::facade(factory).conf().limits.lease_seconds.max(5)) * 1000
}

/// Take or renew the run's lease for `owner` for `lease` milliseconds: when it has none, it is `owner`'s, or it
/// expired. True when `owner` holds it now. A conditional update in a transaction of its own.
///
/// # Errors
///
/// Returns an error when the row cannot be written.
pub fn claim(facade: &EntityFacade, run_id: &str, owner: &str, lease: i64) -> Result<bool, Error> {
    let now = now_millis();
    facade
        .transactions_for(RUN)?
        .run(TransactionAttribute::ForceNew, None, || {
            let updated = facade
                .find(RUN)?
                .eq("runId", Value::string(run_id))
                .condition(Condition::or(vec![
                    Condition::is_null("leaseOwner"),
                    Condition::equals("leaseOwner", Value::string(owner)),
                    Condition::less("leaseExpireDate", stamp(now)),
                ]))
                .for_update(true)
                .disable_authz()
                .update_all([
                    ("leaseOwner".to_string(), Value::string(owner)),
                    ("leaseExpireDate".to_string(), stamp(now + lease)),
                ])?;
            if updated > 0 {
                return Ok(true);
            }
            // a renewal within the same millisecond changes nothing
            Ok(facade
                .find(RUN)?
                .eq("runId", Value::string(run_id))
                .disable_authz()
                .one()?
                .and_then(|row| crate::llm::text_of(&row, "leaseOwner"))
                .as_deref()
                == Some(owner))
        })
}

/// Give up `owner`'s lease of the run: it lapses now, so the next sweep takes it.
///
/// # Errors
///
/// Returns an error when the row cannot be written.
pub fn release(facade: &EntityFacade, run_id: &str, owner: &str) -> Result<(), Error> {
    facade
        .transactions_for(RUN)?
        .run(TransactionAttribute::ForceNew, None, || {
            facade
                .find(RUN)?
                .eq("runId", Value::string(run_id))
                .eq("leaseOwner", Value::string(owner))
                .for_update(true)
                .disable_authz()
                .update_all([
                    ("leaseOwner".to_string(), Value::Null),
                    // lapsed now, so the next sweep takes it
                    ("leaseExpireDate".to_string(), stamp(now_millis() - 1)),
                ])
                .map(|_| ())
        })
}

/// What one renewal found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Renewal {
    pub renewed: usize,
    /// Runs whose `cancelRequested` is set: their tokens were canceled.
    pub canceled: Vec<String>,
    /// Runs another node holds now: marked lost (their steps stop without writing) and their tokens canceled.
    pub lost: Vec<String>,
}

/// Renew the leases of every run this node works, in one update, and read back their `cancelRequested` and owners.
///
/// # Errors
///
/// Returns an error when the rows cannot be read or written.
pub fn renew(factory: &Arc<ExecutionContextFactory>) -> Result<Renewal, Error> {
    let engine = engine(factory);
    let ids = engine.local_ids();
    let mut renewal = Renewal::default();
    if ids.is_empty() {
        return Ok(renewal);
    }
    let facade = facade_of(factory)?;
    let lease = lease_millis(factory);
    let node = engine.node().to_string();
    let in_ids = || Condition::is_in("runId", ids.iter().map(Value::string).collect());
    let rows = quietly(|| -> Result<Vec<RunRow>, Error> {
        renewal.renewed = usize::try_from(facade.transactions_for(RUN)?.run(
            TransactionAttribute::ForceNew,
            None,
            || {
                facade
                    .find(RUN)?
                    .condition(in_ids())
                    .eq("leaseOwner", Value::string(&node))
                    .for_update(true)
                    .disable_authz()
                    .update_all([("leaseExpireDate".to_string(), stamp(now_millis() + lease))])
            },
        )?)
        .unwrap_or(0);
        Ok(facade
            .find(RUN)?
            .condition(in_ids())
            .disable_authz()
            .list()?
            .values()
            .iter()
            .map(RunRow::of)
            .collect())
    })?;
    for run in rows {
        let Some(local) = engine.local_if(&run.run_id) else {
            continue;
        };
        if run.cancel_requested && !local.cancel.is_canceled() {
            local.cancel.cancel();
            renewal.canceled.push(run.run_id.clone());
        }
        let held_elsewhere = run.lease_owner.as_deref().is_some_and(|owner| owner != node);
        if held_elsewhere {
            tracing::warn!(
                "agent run {} is held by {} now, so this node stops working it",
                run.run_id,
                run.lease_owner.as_deref().unwrap_or("")
            );
            // the step working it stops without writing and leaves the run's state to the node that has it
            local.lost.store(true, std::sync::atomic::Ordering::SeqCst);
            local.cancel.cancel();
            if !local.busy.load(std::sync::atomic::Ordering::SeqCst) {
                engine.forget_if(&run.run_id, &local);
            }
            renewal.lost.push(run.run_id.clone());
        }
    }
    Ok(renewal)
}

/// The keeper thread: [`renew`] every `heartbeat-seconds`, until the factory is dropped. A failure is logged.
pub(crate) fn start_keeper(factory: &Arc<ExecutionContextFactory>) {
    let every = Duration::from_secs(u64::from(
        crate::llm::facade(factory).conf().limits.heartbeat_seconds.max(1),
    ));
    let weak = Arc::downgrade(factory);
    let spawned = std::thread::Builder::new()
        .name("moqui-agent-lease".to_string())
        .spawn(move || loop {
            std::thread::sleep(every);
            let Some(factory) = weak.upgrade() else {
                return;
            };
            let context = factory.new_context();
            let _entered = context.enter();
            if let Err(error) = renew(&factory) {
                tracing::error!("the agent run leases could not be renewed: {error}");
            }
        });
    if let Err(error) = spawned {
        tracing::error!("the agent lease keeper did not start: {error}");
    }
}

/// [`sweep`] with its failure logged.
pub(crate) fn sweep_logged(factory: &Arc<ExecutionContextFactory>) {
    let context = factory.new_context();
    let _entered = context.enter();
    match sweep(factory) {
        Ok(0) => {}
        Ok(taken) => tracing::info!("the agent recovery sweep took over {taken} runs"),
        Err(error) => tracing::error!("the agent recovery sweep failed: {error}"),
    }
}

/// Take over every Submitted or Working run whose lease lapsed (or that was never leased, a lease's time after it was
/// submitted) and that this node does not work:
/// claim it, count the attempt, and queue its next step; a run past its attempts fails, one asked to cancel is
/// Canceled. Each run is handled on its own: one that fails is logged and the sweep goes on. Out: the runs taken over.
///
/// # Errors
///
/// Returns an error when the runs cannot be read.
pub fn sweep(factory: &Arc<ExecutionContextFactory>) -> Result<usize, Error> {
    let facade = facade_of(factory)?;
    let engine = engine(factory);
    let now = now_millis();
    let candidates: Vec<RunRow> = quietly(|| -> Result<Vec<RunRow>, Error> {
        Ok(facade
            .find(RUN)?
            .condition(Condition::is_in(
                "statusId",
                vec![
                    Value::string(RunState::Submitted.status_id()),
                    Value::string(RunState::Working.status_id()),
                ],
            ))
            .condition(Condition::or(vec![
                Condition::less("leaseExpireDate", stamp(now)),
                // a run never leased is left a lease's time to start
                Condition::and(vec![
                    Condition::is_null("leaseExpireDate"),
                    Condition::less("submittedDate", stamp(now - lease_millis(factory))),
                ]),
            ]))
            .disable_authz()
            .list()?
            .values()
            .iter()
            .map(RunRow::of)
            .collect())
    })?;
    match expire_pauses(factory) {
        Ok(0) => {}
        Ok(expired) => tracing::info!("the agent recovery sweep ended {expired} runs whose pause expired"),
        Err(error) => tracing::error!("the expired pauses of agent runs could not be read: {error}"),
    }
    let mut taken = 0;
    for run in candidates {
        if engine.is_local(&run.run_id) {
            continue;
        }
        match quietly(|| take_over(factory, &facade, &run)) {
            Ok(true) => taken += 1,
            Ok(false) => {}
            Err(error) => tracing::error!("agent run {} could not be taken over: {error}", run.run_id),
        }
    }
    Ok(taken)
}

fn take_over(factory: &Arc<ExecutionContextFactory>, facade: &EntityFacade, run: &RunRow) -> Result<bool, Error> {
    let engine = engine(factory);
    if !claim(facade, &run.run_id, engine.node(), lease_millis(factory))? {
        return Ok(false);
    }
    let mut sink = EventSink::new(factory, facade, &run.run_id, &run.user_id);
    if run.cancel_requested {
        super::transition(
            facade,
            &mut sink,
            &run.run_id,
            RunState::Canceled,
            &Change::reason("canceled"),
        )?;
        return Ok(true);
    }
    // a lease that lapsed counts as an attempt only once the run had started
    let attempt = if run.lease_owner.is_some() {
        run.attempt + 1
    } else {
        run.attempt.max(1)
    };
    if attempt > super::MAX_ATTEMPTS {
        super::transition(
            facade,
            &mut sink,
            &run.run_id,
            RunState::Failed,
            &Change::failed(
                "recovery_exhausted",
                &format!(
                    "the run was taken over {} times and stopped each time",
                    super::MAX_ATTEMPTS - 1
                ),
            ),
        )?;
        return Ok(true);
    }
    super::in_tx(facade, || {
        facade
            .find(RUN)?
            .eq("runId", Value::string(&run.run_id))
            .for_update(true)
            .disable_authz()
            .update_all([("attempt".to_string(), Value::I64(attempt))])
    })?;
    tracing::info!(
        "agent run {} is taken over by {} (attempt {attempt}; its lease of {} lapsed)",
        run.run_id,
        engine.node(),
        run.lease_owner.as_deref().unwrap_or("no one")
    );
    engine.local(&run.run_id);
    if let Err(error) = super::pool::submit(factory, &run.run_id) {
        tracing::error!(
            "agent run {} could not be queued after its takeover: {error}",
            run.run_id
        );
        release(facade, &run.run_id, engine.node())?;
        engine.forget(&run.run_id);
        return Ok(false);
    }
    Ok(true)
}

/// End every paused run whose `pauseExpireDate` has passed: Failed `pause_expired`, as the state table has it, the
/// question it waited on expired, and its approval expired (in the transition, [`crate::llm::approval::expire_for_run`]);
/// a hand-off that expired is abandoned instead and the run resumes with that. Each run is handled on its own. Out:
/// the runs handled.
///
/// # Errors
///
/// Returns an error when the runs cannot be read.
pub fn expire_pauses(factory: &Arc<ExecutionContextFactory>) -> Result<usize, Error> {
    let facade = facade_of(factory)?;
    let paused: Vec<RunRow> = quietly(|| -> Result<Vec<RunRow>, Error> {
        Ok(facade
            .find(RUN)?
            .condition(Condition::is_in(
                "statusId",
                vec![
                    Value::string(RunState::InputRequired.status_id()),
                    Value::string(RunState::AuthRequired.status_id()),
                ],
            ))
            .condition(Condition::less("pauseExpireDate", stamp(now_millis())))
            .disable_authz()
            .list()?
            .values()
            .iter()
            .map(RunRow::of)
            .collect())
    })?;
    let mut ended = 0;
    for run in paused {
        // a hand-off that expired is the user leaving it: the run goes on with that, it does not fail
        if run.pause_kind == Some(crate::llm::tools::PauseKind::Handoff) {
            match quietly(|| crate::llm::approval::handoff::abandon_paused(factory, &facade, &run, "expired")) {
                Ok(true) => {
                    ended += 1;
                    continue;
                }
                // no open watch: the run ends as for any expired pause
                Ok(false) => {}
                Err(error) => {
                    tracing::error!(
                        "the hand-off of agent run {} could not be ended at its expiry: {error}",
                        run.run_id
                    );
                    continue;
                }
            }
        }
        let expired = quietly(|| -> Result<(), Error> {
            let mut sink = EventSink::new(factory, &facade, &run.run_id, &run.user_id);
            super::in_tx(&facade, || {
                if let Some(question_id) = &run.pause_ref {
                    facade
                        .find(super::QUESTION)?
                        .eq("questionId", Value::string(question_id))
                        .eq("runId", Value::string(&run.run_id))
                        .eq("statusId", Value::string("AgqOpen"))
                        .for_update(true)
                        .disable_authz()
                        .update_all([("statusId".to_string(), Value::string("AgqExpired"))])?;
                }
                super::transition(
                    &facade,
                    &mut sink,
                    &run.run_id,
                    RunState::Failed,
                    &Change::failed("pause_expired", "the run waited past its pause's expiry").unleased(),
                )
            })
        });
        match expired {
            Ok(()) => ended += 1,
            Err(error) => tracing::error!(
                "agent run {} could not be ended at its pause's expiry: {error}",
                run.run_id
            ),
        }
    }
    Ok(ended)
}
