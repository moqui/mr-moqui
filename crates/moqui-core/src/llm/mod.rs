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

//! The agent runtime and the model providers (plans/LLM-A2A-Assist.md), behind the `llm` feature. One runtime serves
//! every surface: the Assist drawer, chat agents, triggered and scheduled agents, `ec.llm` and the AI tasks, A2A, MCP
//! and the OpenAI-compatible gateway.
//!
//! [`LlmFacade`] is a factory extension ([`facade`]): it holds the typed `llm-facade` conf ([`conf`]) and resolves
//! features, tiers and models against the conf merged with the database rows ([`registry`]). [`provider`] makes the
//! calls; [`usage`] logs them and counts them against the quotas; [`run::state`] is the run state table.
//! [`LlmFacade::complete`] is `ec.llm.complete` for app code, with structured output. [`run`] is the run engine (the
//! agent pool, the step jobs, the leases and the recovery sweep, the events), [`tools`] the tool registry, [`approval`]
//! the approvals of writes, [`assist`] the drawer's JSON calls, [`prompt`] the context builder; [`start`] starts the
//! engine's background work on a booted server.

pub mod a2a;
pub mod ai_task;
pub mod approval;
pub mod assist;
pub mod chat;
pub mod conf;
pub mod embed;
pub mod facts;
pub mod gateway;
pub mod json_mode;
pub mod mcp;
pub mod memory;
pub mod prompt;
pub mod provider;
pub mod registry;
pub mod remote;
pub mod run;
pub mod screen;
pub(crate) mod services;
pub mod skills;
pub mod tokens;
pub mod tools;
pub mod trigger;
pub mod ui;
pub mod usage;
pub mod window;

#[cfg(test)]
mod learn_tests;
#[cfg(test)]
mod members_tests;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};
use rust_decimal::Decimal;

use crate::context::ExecutionContextFactory;
use crate::entity::{EntityFacade, EntityValue};

use conf::{LlmConf, ReasoningLevel, Tier};
use provider::{CallOptions, Finish, Message, ModelRequest, Recorder, ResponseSchema};
use registry::{Registry, Target, Verdict, Want};
use usage::{CallRecord, CallSource, Usage};

/// The factory's LLM state: the conf, read once when the facade is first used.
pub struct LlmFacade {
    conf: LlmConf,
}

impl LlmFacade {
    /// A facade over `conf`; its problems are logged, one error each.
    pub fn new(conf: LlmConf) -> Self {
        for problem in &conf.problems {
            tracing::error!("{problem}");
        }
        Self { conf }
    }

    pub fn conf(&self) -> &LlmConf {
        &self.conf
    }

    /// False when the conf turns the facade off (`llm-facade enabled="false"`): no call is made.
    pub fn enabled(&self) -> bool {
        self.conf.enabled
    }

    /// The conf merged with the `LlmEndpoint`, `LlmModel` and `LlmFeatureBinding` rows. A row that does not read is
    /// logged and left out; when the rows cannot be read at all, the conf alone is used and that is logged.
    pub fn registry(&self, facade: &EntityFacade) -> Registry {
        match Registry::load(&self.conf, facade) {
            Ok(registry) => {
                for problem in &registry.problems {
                    tracing::error!("{problem}");
                }
                registry
            }
            Err(error) => {
                tracing::error!("the LLM model rows could not be read, so only the llm-facade conf applies: {error}");
                Registry::from_conf(&self.conf)
            }
        }
    }

    /// Where a call for `want` goes.
    ///
    /// # Errors
    ///
    /// Returns an error when the facade is off or nothing asked for can take a call.
    pub fn resolve(&self, facade: &EntityFacade, want: &Want<'_>) -> Result<Target, Error> {
        if !self.conf.enabled {
            return Err(Error::new("the llm-facade is disabled"));
        }
        self.registry(facade).resolve(want)
    }

    /// The model bound to `feature`, else its fallback.
    ///
    /// # Errors
    ///
    /// As [`Self::resolve`].
    pub fn resolve_feature(&self, facade: &EntityFacade, feature: &str) -> Result<Target, Error> {
        self.resolve(
            facade,
            &Want {
                feature: Some(feature),
                ..Want::default()
            },
        )
    }

    /// Log a call made to `target` (its prices give the cost), with the conf's content-log settings. Out: the `callId`.
    ///
    /// # Errors
    ///
    /// Returns an error when the row cannot be written.
    pub fn record_call(&self, facade: &EntityFacade, target: &Target, call: &CallRecord) -> Result<String, Error> {
        usage::record_call(facade, &self.conf.content_log, &target.model.prices, call)
    }

    /// One completion for app code (`ec.llm.complete`): the model asked for (a model id, else a feature's binding, else
    /// a tier), the system text, the prompt or messages, and with a JSON schema a structured reply, parsed and checked.
    /// A model whose `json-schema` verdict is no gets the schema in its instructions instead. A structured reply that
    /// does not fit is asked again once with what was wrong. The call counts against the user's `api` quota and ends by
    /// its `timeout` (default [`COMPLETE_TIMEOUT`]). A transaction in place is suspended while the model answers, and
    /// each `LlmCall` is written in a transaction of its own, so a caller's rollback keeps the usage.
    ///
    /// # Errors
    ///
    /// Returns an error when the facade is off, the current user is external (`EXTERNAL_USER`), no model can take the
    /// call, the quota is used up, or the call ends without an answer (an error, a refusal, a filter, an overflow, an
    /// empty reply, a reply that does not fit the schema).
    pub fn complete(&self, facade: &EntityFacade, ask: &CompleteRequest) -> Result<Completion, Error> {
        let user_id = crate::user::current_user_id().ok().flatten();
        // an external user (EXTERNAL_USER: a client contact) gets nothing from a model, by any app's call either
        if user_id.as_deref().is_some_and(crate::user::is_external_user) {
            return Err(Error::new(
                "the LLM call is refused: the models are not available to this user",
            ));
        }
        let target = self.resolve(
            facade,
            &Want {
                model: ask.model.as_deref(),
                feature: ask.feature.as_deref(),
                tier: ask.tier,
                reasoning: ask.reasoning,
                local_only: false,
            },
        )?;
        if let Some(user_id) = &user_id {
            let status = usage::check_quota(facade, &self.conf.limits, user_id, "api", jiff::Timestamp::now().as_millisecond())?;
            if let Some(reason) = status.exceeded {
                return Err(Error::new(format!("the LLM call is refused: {reason}")));
            }
        }
        let mut request = ModelRequest {
            system: ask.system.iter().filter(|text| !text.is_empty()).cloned().collect(),
            history: ask.messages.clone(),
            reasoning: ask.reasoning,
            max_output: ask.max_output,
            ..ModelRequest::default()
        };
        if let Some(prompt) = ask.prompt.as_ref().filter(|prompt| !prompt.is_empty()) {
            request.history.push(Message::user(prompt.clone()));
        }
        if request.history.is_empty() {
            return Err(Error::new("ec.llm.complete needs a prompt or messages"));
        }
        if let Some(schema) = &ask.schema {
            let native = registry::capabilities(facade, &target.model.id)
                .ok()
                .and_then(|verdicts| verdicts.get("json-schema").map(|verdict| verdict.verdict))
                != Some(Verdict::No);
            if native {
                request.response_schema = Some(ResponseSchema {
                    name: ask.schema_name.clone().unwrap_or_else(|| "answer".to_string()),
                    schema: schema.clone(),
                    strict: false,
                });
            } else {
                request.system.push(format!(
                    "Answer with only one JSON value that follows this JSON schema, with no other text:\n{schema}"
                ));
            }
        }
        let mut options = CallOptions::new(ask.source);
        options.deadline = Some(std::time::Instant::now() + ask.timeout.unwrap_or(COMPLETE_TIMEOUT));
        options.user_id.clone_from(&user_id);
        options.nudge = Some(provider::EMPTY_NUDGE.to_string());
        options.recorder = Some(Recorder { llm: self, facade });
        let mut reply = provider::call(&target, &request, &mut options);
        let mut usage = reply.usage;
        let mut call_ids = reply.call_ids.clone();
        let mut json = None;
        if let Some(schema) = &ask.schema {
            let checked = |text: &str| {
                provider::reply_json(text).and_then(|value| provider::check_schema(&value, schema).map(|()| value))
            };
            match (&reply.finish, checked(&reply.text)) {
                (Finish::Stop, Ok(value)) => json = Some(value),
                (Finish::Stop, Err(problem)) => {
                    request.history.push(reply.assistant_message());
                    request.history.push(Message::user(format!(
                        "That reply does not follow the schema: {problem}. Answer again with only the JSON."
                    )));
                    reply = provider::call(&target, &request, &mut options);
                    usage.add(&reply.usage);
                    call_ids.extend(reply.call_ids.iter().cloned());
                    if reply.finish == Finish::Stop {
                        json = Some(checked(&reply.text).map_err(|problem| {
                            Error::new(format!("the reply of model {} does not follow the schema: {problem}", target.model.id))
                        })?);
                    }
                }
                _ => {}
            }
        }
        match &reply.finish {
            Finish::Stop | Finish::ToolCalls => {}
            Finish::Length if ask.schema.is_none() => {}
            other => {
                let why = match other.error() {
                    Some(error) => error.to_string(),
                    None => other.as_str().replace('_', " "),
                };
                return Err(Error::new(format!("the call to model {} ended without an answer: {why}", target.model.id)));
            }
        }
        Ok(Completion {
            text: reply.text,
            json,
            model_id: target.model.id.clone(),
            finish: reply.finish.as_str().to_string(),
            cost: usage::cost(&target.model.prices, &usage),
            usage,
            call_ids,
        })
    }
}

/// What `ec.llm.complete` asks for; its source is `api` by default.
#[derive(Clone, Debug)]
pub struct CompleteRequest {
    pub model: Option<String>,
    pub feature: Option<String>,
    pub tier: Option<Tier>,
    pub system: Option<String>,
    pub prompt: Option<String>,
    /// Earlier messages, before the prompt.
    pub messages: Vec<Message>,
    /// A JSON schema the reply must follow.
    pub schema: Option<serde_json::Value>,
    pub schema_name: Option<String>,
    pub reasoning: Option<ReasoningLevel>,
    pub max_output: Option<u32>,
    pub source: CallSource,
    /// The whole completion's limit, retries and the schema repair included; `None` is [`COMPLETE_TIMEOUT`].
    pub timeout: Option<std::time::Duration>,
}

/// The limit of a completion that gives none.
pub const COMPLETE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

impl Default for CompleteRequest {
    fn default() -> Self {
        Self {
            model: None,
            feature: None,
            tier: None,
            system: None,
            prompt: None,
            messages: Vec::new(),
            schema: None,
            schema_name: None,
            reasoning: None,
            max_output: None,
            source: CallSource::Api,
            timeout: None,
        }
    }
}

/// The answer of [`LlmFacade::complete`].
#[derive(Clone, Debug, PartialEq)]
pub struct Completion {
    pub text: String,
    /// The structured reply, with a schema.
    pub json: Option<serde_json::Value>,
    pub model_id: String,
    pub finish: String,
    pub usage: Usage,
    pub cost: Option<Decimal>,
    pub call_ids: Vec<String>,
}

/// The factory's facade, made on first use from the merged conf.
pub fn facade(factory: &ExecutionContextFactory) -> Arc<LlmFacade> {
    factory.extension(|factory| {
        let conf = LlmConf::from_conf_text(factory.conf_text()).unwrap_or_else(|error| {
            tracing::error!("the llm-facade conf could not be read, so it is off: {error}");
            LlmConf {
                enabled: false,
                ..LlmConf::default()
            }
        });
        LlmFacade::new(conf)
    })
}

/// Start the run engine's background work on a booted server: the lease keeper, the recovery sweep on the scheduler's
/// tick and one sweep now (the host calls it after the cluster starts, so the lease owner is the cluster instance id),
/// the batch sweep of the agent triggers, the screen render's observer of the hand-off watches, and the sync of the skill
/// files into their rows.
pub fn start(factory: &Arc<ExecutionContextFactory>) {
    run::start(factory);
    trigger::start(factory);
    approval::handoff::install(factory);
    a2a::push::start(factory);
    if let Some(facade) = factory.entity() {
        skills::sync::ensure_synced(factory, &facade);
    }
}

/// A field as trimmed text; empty is `None`.
pub(crate) fn text_of(row: &EntityValue, field: &str) -> Option<String> {
    row.get(field)
        .filter(|value| !value.is_null())
        .map(to_plain_string)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

pub(crate) fn int_of(row: &EntityValue, field: &str) -> Option<i64> {
    match row.get(field)? {
        Value::I32(n) => Some(i64::from(*n)),
        Value::I64(n) => Some(*n),
        Value::Decimal(n) => i64::try_from(n.trunc()).ok(),
        Value::F64(n) if n.is_finite() => Some(n.trunc() as i64),
        Value::Null => None,
        other => to_plain_string(other).trim().parse().ok(),
    }
}

pub(crate) fn decimal_of(row: &EntityValue, field: &str) -> Option<Decimal> {
    match row.get(field)? {
        Value::Decimal(n) => Some(*n),
        Value::I32(n) => Some(Decimal::from(*n)),
        Value::I64(n) => Some(Decimal::from(*n)),
        Value::F64(n) => Decimal::try_from(*n).ok(),
        Value::Null => None,
        other => to_plain_string(other).trim().parse().ok(),
    }
}

/// `Y` or `N` as a flag; empty is `None`.
pub(crate) fn flag_of(row: &EntityValue, field: &str) -> Option<bool> {
    match text_of(row, field)?.as_str() {
        "Y" | "true" => Some(true),
        "N" | "false" => Some(false),
        _ => None,
    }
}

/// A date-time value as milliseconds since the epoch.
pub(crate) fn millis_of(value: &Value) -> Option<i64> {
    match value {
        Value::Timestamp(stamp) => Some(stamp.as_millisecond()),
        Value::I64(n) => Some(*n),
        _ => None,
    }
}
