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

//! The usage of a run an external harness works (`externalHarnessId`): the harness reports its provider turns, each
//! becomes an `AgentRunStep` priced at the model's prices in the registry, the run's totals are the sum of its steps,
//! and a run past its `maxTokens` or `maxCost` gets `cancelRequested`, which the harness reads to stop its thread.
//!
//! A turn is keyed by its `stepSeq`: the harness reports the same turn again as it likes (each poll, say), and it
//! replaces the step it wrote before, so the totals never count a turn twice.

use moqui_lib::error::Error;
use moqui_lib::object::Value;
use rust_decimal::Decimal;

use super::context::Exhausted;
use super::{facade_of, in_tx, stamp, RUN};
use crate::registry::Registry;
use crate::usage::{self, Usage};
use crate::{decimal_of, int_of, text_of};
use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::{EntityFacade, EntityValue};

const STEP: &str = "moqui.agent.AgentRunStep";

/// One provider turn as the harness reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExternalStep {
    /// The turn's place in the run, from 1; the same turn always has the same one.
    pub seq: i64,
    /// A model id of the registry, or the provider's name of the model (a model's `name`).
    pub model: Option<String>,
    pub usage: Usage,
    pub finish_reason: Option<String>,
    pub started: Option<i64>,
    pub ended: Option<i64>,
}

/// The run's totals after the steps were written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Recorded {
    pub step_count: i64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub cost: Decimal,
    /// The cap the run passed, if it did.
    pub exhausted: Option<Exhausted>,
    pub cancel_requested: bool,
    /// The models of the steps that have no price in the registry: their steps count no cost.
    pub unpriced: Vec<String>,
}

/// The registry's model `name` names: its id, else the first whose provider name it is.
fn model_of<'a>(registry: &'a Registry, name: &str) -> Option<&'a crate::registry::Model> {
    registry
        .models
        .get(name)
        .or_else(|| registry.models.values().find(|model| model.conf.name == name))
}

/// Write `steps` for the external run `run_id`, roll its totals up and request its cancel when it passed a cap.
///
/// # Errors
///
/// Returns an error when there is no such run, it is not an external harness's, or the rows cannot be written.
pub fn record_usage(
    factory: &ExecutionContextFactory,
    run_id: &str,
    steps: &[ExternalStep],
) -> Result<Recorded, Error> {
    let facade = facade_of(factory)?;
    let registry = crate::facade(factory).registry(&facade);
    in_tx(&facade, || record_in_tx(&facade, &registry, run_id, steps))
}

fn record_in_tx(
    facade: &EntityFacade,
    registry: &Registry,
    run_id: &str,
    steps: &[ExternalStep],
) -> Result<Recorded, Error> {
    let mut run = facade
        .find(RUN)?
        .eq("runId", Value::string(run_id))
        .for_update(true)
        .disable_authz()
        .one()?
        .ok_or_else(|| Error::new(format!("there is no agent run {run_id}")))?;
    if text_of(&run, "externalHarnessId").is_none() {
        return Err(Error::new(format!(
            "agent run {run_id} is not an external harness's: its own steps record its usage"
        )));
    }
    let mut unpriced = Vec::new();
    for step in steps {
        if step.seq < 1 {
            return Err(Error::new(format!("a step of run {run_id} has no stepSeq from 1")));
        }
        let model = step.model.as_deref().and_then(|name| model_of(registry, name));
        let cost = model.and_then(|model| usage::cost(&model.conf.prices, &step.usage));
        if cost.is_none() {
            let name = step.model.clone().unwrap_or_else(|| "unknown".to_string());
            if !unpriced.contains(&name) {
                unpriced.push(name);
            }
        }
        let existing = facade
            .find(STEP)?
            .eq("runId", Value::string(run_id))
            .eq("stepSeq", Value::I64(step.seq))
            .disable_authz()
            .one()?;
        let mut row = match &existing {
            Some(row) => row.clone(),
            None => {
                let mut row = facade.make_value(STEP)?;
                row.set("runId", Value::string(run_id));
                row.set("stepSeq", Value::I64(step.seq));
                row
            }
        };
        let model_id = model.map(|model| model.conf.id.clone()).or_else(|| step.model.clone());
        row.set("modelId", model_id.map_or(Value::Null, Value::string));
        row.set(
            "finishReason",
            step.finish_reason.as_ref().map_or(Value::Null, Value::string),
        );
        for (field, value) in [
            ("inputTokens", step.usage.input),
            ("cachedInputTokens", step.usage.cached_input),
            ("cacheWriteTokens", step.usage.cache_write),
            ("outputTokens", step.usage.output),
            ("reasoningTokens", step.usage.reasoning),
        ] {
            row.set(field, Value::I64(i64::try_from(value).unwrap_or(i64::MAX)));
        }
        row.set("cost", cost.map_or(Value::Null, Value::Decimal));
        row.set("startedDate", step.started.map_or(Value::Null, stamp));
        row.set("endedDate", step.ended.map_or(Value::Null, stamp));
        if existing.is_some() {
            row.update(facade)?;
        } else {
            row.create(facade)?;
        }
    }
    // the totals are the sum of every step, so a turn reported again is not counted twice
    let rows = facade
        .find(STEP)?
        .eq("runId", Value::string(run_id))
        .disable_authz()
        .list()?;
    let mut recorded = Recorded {
        unpriced,
        ..Recorded::default()
    };
    let tokens = |row: &EntityValue, field: &str| int_of(row, field).and_then(|n| u64::try_from(n).ok()).unwrap_or(0);
    for row in rows.values() {
        recorded.step_count = recorded.step_count.max(int_of(row, "stepSeq").unwrap_or(0));
        recorded.input_tokens = recorded.input_tokens.saturating_add(tokens(row, "inputTokens"));
        recorded.cached_input_tokens = recorded
            .cached_input_tokens
            .saturating_add(tokens(row, "cachedInputTokens"));
        recorded.output_tokens = recorded.output_tokens.saturating_add(tokens(row, "outputTokens"));
        recorded.cost += decimal_of(row, "cost").unwrap_or_default();
    }
    let count = |value: u64| Value::I64(i64::try_from(value).unwrap_or(i64::MAX));
    run.set("stepCount", Value::I64(recorded.step_count));
    run.set("inputTokens", count(recorded.input_tokens));
    run.set("cachedInputTokens", count(recorded.cached_input_tokens));
    run.set("outputTokens", count(recorded.output_tokens));
    run.set("cost", Value::Decimal(recorded.cost));
    // the caps as the native engine reads them: tokens are the input and output, and reaching a cap is passing it
    let used = recorded.input_tokens.saturating_add(recorded.output_tokens);
    let max_tokens = int_of(&run, "maxTokens").and_then(|n| u64::try_from(n).ok()).filter(|n| *n > 0);
    let max_cost = decimal_of(&run, "maxCost").filter(|cost| *cost > Decimal::ZERO);
    recorded.exhausted = if max_tokens.is_some_and(|max| used >= max) {
        Some(Exhausted::Tokens)
    } else if max_cost.is_some_and(|max| recorded.cost >= max) {
        Some(Exhausted::Cost)
    } else {
        None
    };
    let terminal = text_of(&run, "statusId")
        .and_then(|status| super::RunState::from_status_id(&status))
        .is_some_and(|state| state.is_terminal());
    if let Some(cap) = recorded.exhausted.filter(|_| !terminal) {
        if crate::flag_of(&run, "cancelRequested") != Some(true) {
            let reason = match cap {
                Exhausted::Cost => format!(
                    "the run passed its cost budget: {} of {}",
                    recorded.cost.round_dp(4).normalize(),
                    max_cost.unwrap_or_default().normalize()
                ),
                _ => format!(
                    "the run passed its token budget: {used} of {}",
                    max_tokens.unwrap_or_default()
                ),
            };
            run.set("cancelRequested", Value::string("Y"));
            run.set("statusReason", Value::string(reason));
        }
    }
    recorded.cancel_requested = crate::flag_of(&run, "cancelRequested") == Some(true);
    run.update(facade)?;
    Ok(recorded)
}
