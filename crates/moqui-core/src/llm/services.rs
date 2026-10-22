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

//! The bodies of the `type="rust"` services of `org.moqui.agent.AgentAdminServices`: the effective configuration, the
//! admin's capability verdicts, the endpoint test and the capability probe, the usage rollup, the content cleanup, the
//! quota check, the dashboard totals, the prompt preview and the tool exposures' store with its `lockVersion` check; and
//! of `org.moqui.agent.AgentServices`, the internal entry to the run engine: start, cancel and input for a run, its
//! stored events, and the approvals (decide, preview, read, and leaving a hand-off).

use std::collections::BTreeMap;

use indexmap::IndexMap;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use moqui_db::sql::Condition;

use super::conf::{mask_secret, ReasoningLevel};
use super::provider::{self, CallOptions, Finish, Message, ModelRequest, Recorder, ToolSpec};
use super::registry::{self, Verdict, VerdictSource};
use super::run::RunState;
use super::usage;
use crate::context::ExecutionContext;
use crate::entity::EntityFacade;
use crate::service::rust_services::{rust_service, RustServiceFn};

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, get_effective_config)?;
    rust_service!(map, test_endpoint)?;
    rust_service!(map, probe_model)?;
    rust_service!(map, set_model_capability)?;
    rust_service!(map, rollup_usage)?;
    rust_service!(map, clean_call_content)?;
    rust_service!(map, check_quota)?;
    rust_service!(map, get_dashboard_totals)?;
    rust_service!(map, get_call_content)?;
    rust_service!(map, get_prompt_preview)?;
    rust_service!(map, start_agent_run)?;
    rust_service!(map, cancel_agent_run)?;
    rust_service!(map, submit_agent_input)?;
    rust_service!(map, get_agent_run_events)?;
    rust_service!(map, decide_agent_approval)?;
    rust_service!(map, preview_agent_approval)?;
    rust_service!(map, get_agent_approval)?;
    rust_service!(map, abandon_agent_handoff)?;
    rust_service!(map, store_tool_exposure)?;
    super::chat::register(map)?;
    super::trigger::register(map)?;
    super::gateway::register(map)?;
    super::ai_task::register(map)?;
    super::remote::register(map)?;
    super::skills::services::register(map)
}

type Parameters = BTreeMap<String, Value>;
type Out = Result<BTreeMap<String, Value>, LibError>;

fn text(parameters: &Parameters, key: &str) -> Option<String> {
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

fn out(entries: impl IntoIterator<Item = (&'static str, Value)>) -> BTreeMap<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

fn map(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Map(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect::<IndexMap<_, _>>(),
    )
}

fn opt<T: ToString>(value: Option<T>) -> Value {
    value.map_or(Value::Null, |value| Value::string(value.to_string()))
}

fn number(value: Option<u32>) -> Value {
    value.map_or(Value::Null, |value| Value::I64(i64::from(value)))
}

fn count(value: u64) -> Value {
    Value::I64(i64::try_from(value).unwrap_or(i64::MAX))
}

/// The AgentAdmin permission, else an error message and false.
fn admin() -> bool {
    match crate::user::has_permission("AgentAdmin") {
        Ok(true) => true,
        _ => {
            add_error("this needs the AgentAdmin permission");
            false
        }
    }
}

fn llm() -> Option<std::sync::Arc<super::LlmFacade>> {
    ExecutionContext::with_current(|ec| super::facade(ec.factory()))
}

fn now_millis() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

/// `get#EffectiveConfig`: the endpoints, models and bindings in effect (the conf with the rows over it), keys masked,
/// with where each came from, whether it can take a call, and the problems of the conf and the rows.
fn get_effective_config(_parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !admin() {
        return Ok(BTreeMap::new());
    }
    let Some(llm) = llm() else {
        return Ok(BTreeMap::new());
    };
    let registry = llm.registry(facade);
    let endpoint_list = registry
        .endpoints
        .values()
        .map(|endpoint| {
            let conf = &endpoint.conf;
            map([
                ("endpointId", Value::string(&conf.id)),
                ("protocol", Value::string(conf.protocol.as_str())),
                ("baseUrl", Value::string(&conf.url)),
                ("requestUrl", Value::string(conf.request_url())),
                ("apiKeyMasked", Value::string(mask_secret(&conf.resolved_key().unwrap_or_default()))),
                ("apiKeyEnv", opt(conf.api_key_env.as_ref())),
                ("maxConcurrent", number(conf.max_concurrent)),
                ("isLocal", Value::string(if conf.local { "Y" } else { "N" })),
                ("disabled", Value::string(if conf.disabled { "Y" } else { "N" })),
                ("source", Value::string(endpoint.source.as_str())),
            ])
        })
        .collect();
    let model_list = registry
        .models
        .values()
        .map(|model| {
            let conf = &model.conf;
            let price = |price: Option<rust_decimal::Decimal>| price.map_or(Value::Null, Value::Decimal);
            map([
                ("modelId", Value::string(&conf.id)),
                ("endpointId", Value::string(&conf.endpoint)),
                ("providerModel", Value::string(&conf.name)),
                ("tier", Value::string(conf.tier.as_str())),
                ("contextWindow", number(conf.context_window)),
                ("maxOutput", number(conf.max_output)),
                ("priceInput", price(conf.prices.input)),
                ("priceCachedInput", price(conf.prices.cached_input)),
                ("priceCacheWrite", price(conf.prices.cache_write)),
                ("priceOutput", price(conf.prices.output)),
                ("currencyUomId", Value::string(&conf.prices.currency)),
                (
                    "reasoningLevels",
                    Value::string(
                        conf.reasoning
                            .keys()
                            .map(|level| level.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                    ),
                ),
                ("disabled", Value::string(if conf.disabled { "Y" } else { "N" })),
                ("source", Value::string(model.source.as_str())),
                (
                    "usable",
                    Value::string(match registry.usable(&conf.id, false) {
                        Ok(_) => "yes".to_string(),
                        Err(reason) => reason,
                    }),
                ),
            ])
        })
        .collect();
    let binding_list = registry
        .bindings
        .values()
        .map(|binding| {
            let resolved = registry.resolve_feature(&binding.conf.feature);
            map([
                ("featureId", Value::string(&binding.conf.feature)),
                ("modelId", Value::string(&binding.conf.model)),
                ("fallbackModelId", opt(binding.conf.fallback.as_ref())),
                ("reasoningLevel", Value::string(binding.conf.reasoning.as_str())),
                ("source", Value::string(binding.source.as_str())),
                (
                    "resolvedModelId",
                    resolved.as_ref().map_or(Value::Null, |target| Value::string(&target.model.id)),
                ),
                (
                    "resolveError",
                    resolved.err().map_or(Value::Null, |error| Value::string(error.to_string())),
                ),
            ])
        })
        .collect();
    let conf = llm.conf();
    let problems = conf
        .problems
        .iter()
        .chain(registry.problems.iter())
        .map(Value::string)
        .collect();
    let limits = &conf.limits;
    Ok(out([
        ("enabled", Value::Bool(conf.enabled)),
        ("endpointList", Value::List(endpoint_list)),
        ("modelList", Value::List(model_list)),
        ("bindingList", Value::List(binding_list)),
        ("problemList", Value::List(problems)),
        (
            "limits",
            map([
                ("maxConcurrentRuns", number(Some(limits.max_concurrent_runs))),
                ("runQueue", number(Some(limits.run_queue))),
                ("maxSteps", number(Some(limits.max_steps))),
                ("maxRunSeconds", count(limits.max_run_seconds)),
                ("userDailyTokens", count(limits.user_daily_tokens)),
                ("userDailyCost", limits.user_daily_cost.map_or(Value::Null, Value::Decimal)),
            ]),
        ),
        (
            "contentLog",
            map([
                ("enabled", Value::Bool(conf.content_log.enabled)),
                ("redact", Value::Bool(conf.content_log.redact)),
                ("retentionDays", number(Some(conf.content_log.retention_days))),
            ]),
        ),
    ]))
}

/// The target of `modelId` (or of `endpointId`'s first model), or an error message and `None`.
fn target_of(parameters: &Parameters, facade: &EntityFacade) -> Option<registry::Target> {
    let llm = llm()?;
    let registry = llm.registry(facade);
    let model_id = match (text(parameters, "modelId"), text(parameters, "endpointId")) {
        (Some(model_id), _) => model_id,
        (None, Some(endpoint_id)) => match registry.models.values().find(|model| model.conf.endpoint == endpoint_id) {
            Some(model) => model.conf.id.clone(),
            None => {
                add_error(format!("endpoint {endpoint_id} has no model to test with"));
                return None;
            }
        },
        (None, None) => {
            add_error("modelId or endpointId is required");
            return None;
        }
    };
    match registry.resolve_model(&model_id, ReasoningLevel::None) {
        Ok(target) => Some(target),
        Err(error) => {
            add_error(error.to_string());
            None
        }
    }
}

/// The limit of one admin test or probe call.
const ADMIN_CALL_SECONDS: u64 = 90;

/// The options of an admin call: the current user, a deadline, no re-ask at the output limit, and the call log.
fn admin_options<'a>(llm: &'a super::LlmFacade, facade: &'a EntityFacade, source: usage::CallSource) -> CallOptions<'a> {
    let mut options = CallOptions::new(source);
    options.user_id = crate::user::current_user_id().ok().flatten();
    options.deadline = Some(std::time::Instant::now() + std::time::Duration::from_secs(ADMIN_CALL_SECONDS));
    options.length_retry = false;
    options.recorder = Some(Recorder { llm, facade });
    options
}

fn message(text: impl Into<String>, kind: &str) {
    let text = text.into();
    ExecutionContext::with_current(|ctx| ctx.message().add_message_typed(text, kind));
}

/// Whether a reply is an answer of some kind: not an error, a cancel, an overflow, a filter or a refusal.
fn answered(finish: &Finish) -> bool {
    matches!(finish, Finish::Stop | Finish::ToolCalls | Finish::Length)
}

/// `test#Endpoint`: one tiny call to a model, recorded with source `test`. A failure is the answer (`ok` false and
/// the message), not a service error, so its `LlmCall` row stays.
fn test_endpoint(parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !admin() {
        return Ok(BTreeMap::new());
    }
    let (Some(target), Some(llm)) = (target_of(parameters, facade), llm()) else {
        return Ok(BTreeMap::new());
    };
    let request = ModelRequest {
        history: vec![Message::user("Reply with the single word: ok")],
        reasoning: Some(ReasoningLevel::None),
        max_output: Some(64),
        ..ModelRequest::default()
    };
    let mut options = admin_options(&llm, facade, usage::CallSource::Test);
    let started = std::time::Instant::now();
    let reply = provider::call(&target, &request, &mut options);
    let latency = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
    let ok = answered(&reply.finish);
    let error_text = reply.finish.error().map(ToString::to_string);
    if ok {
        message(format!("Model {} answered in {latency} ms: {}", target.model.id, reply.text.trim()), "success");
    } else {
        message(
            format!(
                "Model {} did not answer ({}): {}",
                target.model.id,
                reply.finish.as_str(),
                error_text.clone().unwrap_or_default()
            ),
            "danger",
        );
    }
    Ok(out([
        ("ok", Value::Bool(ok)),
        ("httpStatus", reply.http_status.map_or(Value::Null, |status| Value::I64(i64::from(status)))),
        ("latencyMs", Value::I64(latency)),
        ("replyText", Value::string(reply.text.trim())),
        ("finish", Value::string(reply.finish.as_str())),
        ("errorText", opt(error_text)),
        ("modelId", Value::string(&target.model.id)),
        ("callId", opt(reply.call_ids.last())),
    ]))
}

/// The prefix of the prompt-caching probe: about 1,500 tokens, over the smallest prefix the hosted caches keep.
fn caching_prefix() -> String {
    let mut text = String::from("You answer questions about this reference list. ");
    for index in 0..300 {
        text.push_str(&format!("Item {index} is the number {} written out. ", index * 7));
    }
    text
}

/// One probe: its verdict and evidence, from the calls it made.
struct Probe {
    capability: &'static str,
    /// `None` when a call failed for a reason that says nothing of the capability (a timeout, a network error, a
    /// rate limit, a cancel): nothing is stored, so an earlier verdict stands.
    verdict: Option<Verdict>,
    evidence: String,
}

/// Whether a reply failed for a reason that says nothing of what the model can do. A bad request does say: the server
/// refused the feature.
fn transient(finish: &Finish) -> bool {
    match finish {
        Finish::Canceled => true,
        Finish::Error(error) => error.class != provider::ErrorClass::BadRequest,
        _ => false,
    }
}

fn probe_one(capability: &'static str, target: &registry::Target, send: &mut dyn FnMut(&ModelRequest) -> provider::Reply) -> Probe {
    let mut failure: Option<String> = None;
    let mut call = |request: &ModelRequest| {
        let reply = send(request);
        if failure.is_none() && transient(&reply.finish) {
            failure = Some(match reply.finish.error() {
                Some(error) => error.to_string(),
                None => reply.finish.as_str().to_string(),
            });
        }
        reply
    };
    let get_time = ToolSpec::new("get_time", "The current time.", serde_json::json!({"type": "object", "properties": {}}));
    let get_date = ToolSpec::new("get_date", "The current date.", serde_json::json!({"type": "object", "properties": {}}));
    let failed = |reply: &provider::Reply| match reply.finish.error() {
        Some(error) => format!("{} ({error})", reply.finish.as_str()),
        None => reply.finish.as_str().to_string(),
    };
    let (verdict, evidence) = match capability {
        "tools" => {
            let reply = call(&ModelRequest {
                tools: vec![get_time],
                history: vec![Message::user("Call the get_time tool now.")],
                reasoning: Some(ReasoningLevel::None),
                max_output: Some(256),
                ..ModelRequest::default()
            });
            let names: Vec<&str> = reply.tool_calls.iter().map(|call| call.name.as_str()).collect();
            if names.contains(&"get_time") {
                (Verdict::Yes, format!("called {}", names.join(", ")))
            } else if !reply.bad_tool_calls.is_empty() {
                (Verdict::Partial, format!("a tool call with bad arguments: {}", reply.bad_tool_calls[0].error))
            } else {
                (Verdict::No, format!("no tool call; finish {}", failed(&reply)))
            }
        }
        "parallel-tools" => {
            let reply = call(&ModelRequest {
                tools: vec![get_time, get_date],
                history: vec![Message::user("Call get_time and get_date at the same time, in one reply.")],
                reasoning: Some(ReasoningLevel::None),
                max_output: Some(256),
                ..ModelRequest::default()
            });
            match reply.tool_calls.len() {
                0 => (Verdict::No, format!("no tool call; finish {}", failed(&reply))),
                1 => (Verdict::Partial, format!("one call in the reply ({})", reply.tool_calls[0].name)),
                count => (Verdict::Yes, format!("{count} calls in one reply")),
            }
        }
        "json-schema" => {
            let schema = serde_json::json!({"type": "object", "properties": {"answer": {"type": "string"}},
                "required": ["answer"], "additionalProperties": false});
            let reply = call(&ModelRequest {
                history: vec![Message::user("What is the capital of France? Answer in the JSON asked for.")],
                response_schema: Some(provider::ResponseSchema {
                    name: "answer".to_string(),
                    schema: schema.clone(),
                    strict: true,
                }),
                reasoning: Some(ReasoningLevel::None),
                max_output: Some(256),
                ..ModelRequest::default()
            });
            if !answered(&reply.finish) {
                (Verdict::No, format!("the schema request failed: {}", failed(&reply)))
            } else {
                match provider::reply_json(&reply.text) {
                    Ok(value) => match provider::check_schema(&value, &schema) {
                        Ok(()) => (Verdict::Yes, format!("the reply follows the schema: {value}")),
                        Err(problem) => (Verdict::Partial, format!("JSON that does not follow the schema: {problem}")),
                    },
                    Err(_) => (Verdict::No, format!("no JSON in the reply: {}", reply.text.chars().take(120).collect::<String>())),
                }
            }
        }
        "streaming" => {
            let reply = call(&ModelRequest {
                history: vec![Message::user("Count from 1 to 20, separated by spaces.")],
                reasoning: Some(ReasoningLevel::None),
                max_output: Some(256),
                ..ModelRequest::default()
            });
            match (answered(&reply.finish), reply.text_deltas) {
                (false, _) => (Verdict::No, format!("the call failed: {}", failed(&reply))),
                (true, 0 | 1) => (Verdict::Partial, format!("the reply came in {} piece", reply.text_deltas)),
                (true, pieces) => (Verdict::Yes, format!("the reply came in {pieces} pieces")),
            }
        }
        "reasoning-replay" => {
            let level = target
                .model
                .reasoning
                .keys()
                .copied()
                .find(|level| *level != ReasoningLevel::None)
                .unwrap_or(ReasoningLevel::Medium);
            let mut request = ModelRequest {
                tools: vec![get_time],
                history: vec![Message::user("Think briefly, then call the get_time tool.")],
                reasoning: Some(level),
                max_output: Some(2048),
                ..ModelRequest::default()
            };
            let first = call(&request);
            match (&first.replay, first.tool_calls.first()) {
                (None, _) => (Verdict::No, format!("nothing to replay at reasoning {}; finish {}", level.as_str(), failed(&first))),
                (Some(_), None) => (Verdict::Partial, "a replay but no tool call to continue from".to_string()),
                (Some(bag), Some(tool_call)) => {
                    let kinds: Vec<String> = bag
                        .items
                        .iter()
                        .map(|item| serde_json::to_value(item).ok().and_then(|value| value.get("kind").and_then(|kind| kind.as_str().map(str::to_string))).unwrap_or_default())
                        .collect();
                    request.history.push(first.assistant_message());
                    request.history.push(Message::tool_result(&tool_call.id, &tool_call.name, "12:00"));
                    let second = call(&request);
                    if answered(&second.finish) {
                        (Verdict::Yes, format!("{} sent back and the next call answered", kinds.join(", ")))
                    } else {
                        (Verdict::No, format!("the call with {} sent back failed: {}", kinds.join(", "), failed(&second)))
                    }
                }
            }
        }
        "prompt-caching" => {
            let request = ModelRequest {
                system: vec![caching_prefix()],
                history: vec![Message::user("Which number is item 3? Answer with the number only.")],
                reasoning: Some(ReasoningLevel::None),
                max_output: Some(32),
                cache_key: Some(format!("probe-{}", target.model.id)),
                ..ModelRequest::default()
            };
            let first = call(&request);
            let second = call(&request);
            let cached = second.usage.cached_input.max(first.usage.cached_input);
            if !answered(&second.finish) {
                (Verdict::No, format!("the second call failed: {}", failed(&second)))
            } else if cached > 0 {
                (
                    Verdict::Yes,
                    format!("{cached} of {} input tokens cached on the second call", second.usage.input),
                )
            } else if first.usage.cache_write > 0 {
                (Verdict::Partial, format!("{} tokens written to the cache, none read", first.usage.cache_write))
            } else {
                (Verdict::No, format!("no cached tokens reported ({} input)", second.usage.input))
            }
        }
        other => (Verdict::No, format!("no probe for {other}")),
    };
    match failure {
        Some(failure) => Probe {
            capability,
            verdict: None,
            evidence: format!("no verdict: a call failed ({failure})"),
        },
        None => Probe {
            capability,
            verdict: Some(verdict),
            evidence,
        },
    }
}

/// `probe#Model`: tiny live requests for each capability, each verdict stored with its evidence unless an admin's is
/// there; a probe that fails is a verdict of no, and the others go on. Out: `capabilityList`, each with `stored`.
fn probe_model(parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !admin() {
        return Ok(BTreeMap::new());
    }
    let (Some(target), Some(llm)) = (target_of(parameters, facade), llm()) else {
        return Ok(BTreeMap::new());
    };
    let mut options = admin_options(&llm, facade, usage::CallSource::Probe);
    let mut list = Vec::new();
    let mut yes = 0;
    for capability in registry::CAPABILITIES {
        // each call gets its own deadline, so one slow call does not use up the next one's time
        let probe = probe_one(capability, &target, &mut |request| {
            options.deadline = Some(std::time::Instant::now() + std::time::Duration::from_secs(ADMIN_CALL_SECONDS));
            provider::call(&target, request, &mut options)
        });
        let stored = match probe.verdict {
            None => false,
            Some(verdict) => {
                let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
                match registry::store_verdict(
                    facade,
                    &target.model.id,
                    probe.capability,
                    verdict,
                    VerdictSource::Probe,
                    Some(&probe.evidence),
                ) {
                    Ok(stored) => stored,
                    Err(error) => {
                        tracing::error!("the {} verdict of model {} was not stored: {error}", probe.capability, target.model.id);
                        false
                    }
                }
            }
        };
        if probe.verdict == Some(Verdict::Yes) {
            yes += 1;
        }
        list.push(map([
            ("capability", Value::string(probe.capability)),
            ("verdict", Value::string(probe.verdict.map_or("unknown", Verdict::as_str))),
            ("evidence", Value::string(&probe.evidence)),
            ("stored", Value::Bool(stored)),
        ]));
    }
    message(
        format!("Model {}: {yes} of {} capabilities probed yes", target.model.id, registry::CAPABILITIES.len()),
        "info",
    );
    Ok(out([("capabilityList", Value::List(list))]))
}

/// `set#ModelCapability`: an admin's verdict on a capability, which a probe never replaces; an empty verdict removes
/// the admin's so the next probe decides.
fn set_model_capability(parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !admin() {
        return Ok(BTreeMap::new());
    }
    let (Some(model_id), Some(capability)) = (text(parameters, "modelId"), text(parameters, "capability")) else {
        add_error("modelId and capability are required");
        return Ok(BTreeMap::new());
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    match text(parameters, "verdict") {
        None => {
            facade
                .find("moqui.llm.LlmModelCapability")?
                .eq("modelId", Value::string(&model_id))
                .eq("capability", Value::string(&capability))
                .delete_all()?;
        }
        Some(verdict_text) => {
            let Some(verdict) = Verdict::parse(&verdict_text) else {
                add_error(format!("verdict {verdict_text} is not yes, no or partial"));
                return Ok(BTreeMap::new());
            };
            let evidence = text(parameters, "evidence")
                .unwrap_or_else(|| format!("set by {}", crate::user::current_user_id().ok().flatten().unwrap_or_default()));
            registry::store_verdict(facade, &model_id, &capability, verdict, VerdictSource::Admin, Some(&evidence))?;
        }
    }
    Ok(BTreeMap::new())
}

/// `rollup#LlmUsage`: recompute `LlmUsageDaily` for `usageDate`, or for today and the `days` before it (default 1),
/// each day in its own transaction. A day that fails is logged and rolled back, and the others go on. Out: `rowCount`.
fn rollup_usage(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let dates: Vec<jiff::civil::Date> = match text(parameters, "usageDate") {
        Some(date_text) => match date_text.get(..10).unwrap_or(&date_text).parse::<jiff::civil::Date>() {
            Ok(date) => vec![date],
            Err(error) => {
                add_error(format!("usageDate {date_text} is not a date: {error}"));
                return Ok(BTreeMap::new());
            }
        },
        None => {
            let days = text(parameters, "days").and_then(|days| days.parse::<i64>().ok()).unwrap_or(1).clamp(0, 366);
            let today = usage::day_of(now_millis())?;
            (0..=days)
                .filter_map(|back| today.checked_sub(jiff::Span::new().days(back)).ok())
                .collect()
        }
    };
    let mut rows = 0;
    for date in dates {
        // each day in a transaction of its own: one that fails rolls back alone
        let rolled = facade.transactions_for("moqui.llm.LlmUsageDaily").and_then(|transactions| {
            transactions.run(crate::transaction::TransactionAttribute::ForceNew, None, || usage::rollup_day(facade, date))
        });
        match rolled {
            Ok(written) => rows += written,
            Err(error) => tracing::error!("the LLM usage of {date} was not rolled up: {error}"),
        }
    }
    Ok(out([("rowCount", Value::I64(i64::try_from(rows).unwrap_or(i64::MAX)))]))
}

/// `clean#LlmCallContent`: clear the content of calls older than `retentionDays` (default the conf's). Out:
/// `clearedCount`.
fn clean_call_content(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let days = match text(parameters, "retentionDays").and_then(|days| days.parse::<u32>().ok()) {
        Some(days) => days,
        None => llm().map_or(14, |llm| llm.conf().content_log.retention_days),
    };
    let cleared = usage::clean_call_content(facade, days, now_millis())?;
    Ok(out([("clearedCount", Value::I64(cleared))]))
}

/// `check#LlmQuota`: whether a user (default the current one) may make more calls in a scope (default all). Another
/// user's quota needs AgentAdmin.
fn check_quota(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let current = crate::user::current_user_id().ok().flatten().unwrap_or_default();
    let user_id = text(parameters, "userId").unwrap_or_else(|| current.clone());
    if user_id.is_empty() {
        add_error("no user to check the quota of");
        return Ok(BTreeMap::new());
    }
    if user_id != current && !admin() {
        return Ok(BTreeMap::new());
    }
    let scope = text(parameters, "scope").unwrap_or_else(|| "all".to_string());
    let Some(llm) = llm() else {
        return Ok(BTreeMap::new());
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let status = usage::check_quota(facade, &llm.conf().limits, &user_id, &scope, now_millis())?;
    Ok(out([
        ("withinQuota", Value::Bool(status.is_within())),
        ("reason", opt(status.exceeded.as_ref())),
        ("period", Value::string(&status.limit.period)),
        ("usedTokens", count(status.used.usage.total())),
        ("usedCost", Value::Decimal(status.used.cost)),
        ("maxTokens", status.limit.max_tokens.map_or(Value::Null, count)),
        ("maxCost", status.limit.max_cost.map_or(Value::Null, Value::Decimal)),
        ("limitFrom", Value::string(&status.limit.from)),
    ]))
}

/// `get#DashboardTotals`: the calls of today (from `LlmCall`) and of this month (the rollup of the earlier days plus
/// today), for all users, and the runs submitted today, waiting for input and failed today.
fn get_dashboard_totals(_parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !admin() {
        return Ok(BTreeMap::new());
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let today = usage::day_of(now_millis())?;
    let (start, end) = usage::day_bounds(today)?;
    let today_totals = usage::totals_between(facade, start, end)?;
    let mut month = usage::daily_totals_between(facade, today.first_of_month(), today)?;
    month.calls += today_totals.calls;
    month.errors += today_totals.errors;
    month.usage.add(&today_totals.usage);
    month.cost += today_totals.cost;
    let totals = |totals: &usage::Totals| {
        map([
            ("callCount", count(totals.calls)),
            ("errorCount", count(totals.errors)),
            ("inputTokens", count(totals.usage.input)),
            ("cachedInputTokens", count(totals.usage.cached_input)),
            ("outputTokens", count(totals.usage.output)),
            ("totalTokens", count(totals.usage.total())),
            ("cost", Value::Decimal(totals.cost.normalize())),
            ("currencyUomId", opt(totals.currency.as_ref())),
        ])
    };
    let stamp = crate::user::stamp_value;
    let runs = || -> Result<crate::entity::EntityFind, LibError> { facade.find("moqui.agent.AgentRun") };
    let since_today = || Condition::greater_equal("submittedDate", stamp(start));
    let runs_today = runs()?.condition(since_today()).count()?;
    let runs_failed = runs()?
        .condition(since_today())
        .eq("statusId", Value::string(RunState::Failed.status_id()))
        .count()?;
    let runs_waiting = runs()?
        .condition(Condition::is_in(
            "statusId",
            vec![
                Value::string(RunState::InputRequired.status_id()),
                Value::string(RunState::AuthRequired.status_id()),
            ],
        ))
        .count()?;
    Ok(out([
        ("today", totals(&today_totals)),
        ("month", totals(&month)),
        ("runsToday", Value::I64(runs_today)),
        ("runsWaiting", Value::I64(runs_waiting)),
        ("runsFailedToday", Value::I64(runs_failed)),
    ]))
}

/// `get#LlmCallContent`: the request and response of a call for its own user, or for a user with AgentAudit, whose
/// read is logged in `AgentAuditLog` (D12): AgentAdmin alone does not show another user's content.
fn get_call_content(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(call_id) = text(parameters, "callId") else {
        add_error("callId is required");
        return Ok(BTreeMap::new());
    };
    let viewer = crate::user::current_user_id().ok().flatten().filter(|id| !id.is_empty());
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let Some(row) = facade.find("moqui.llm.LlmCall")?.eq("callId", Value::string(&call_id)).one()? else {
        return Ok(out([
            ("contentShown", Value::Bool(false)),
            ("reason", Value::string(format!("there is no call {call_id}"))),
        ]));
    };
    let owner = super::text_of(&row, "userId");
    let own = viewer.is_some() && owner == viewer;
    let audit = !own && viewer.is_some() && crate::user::has_permission("AgentAudit").unwrap_or(false);
    if !own && !audit {
        return Ok(out([
            ("contentShown", Value::Bool(false)),
            ("audited", Value::Bool(false)),
            ("reason", Value::string("The content of another user's call needs the AgentAudit permission.")),
        ]));
    }
    if audit {
        let mut log = facade.make_value("moqui.agent.AgentAuditLog")?;
        log.set_sequenced_id_primary(facade)?;
        log.set("auditorUserId", opt(viewer.as_ref()));
        log.set("targetUserId", opt(owner.as_ref()));
        log.set("callId", Value::string(&call_id));
        log.set("runId", opt(super::text_of(&row, "runId")));
        log.set(
            "reason",
            Value::string(text(parameters, "reason").unwrap_or_else(|| "the content of an LLM call".to_string())),
        );
        log.set("auditDate", crate::user::now_value());
        log.create(facade)?;
    }
    Ok(out([
        ("contentShown", Value::Bool(true)),
        ("audited", Value::Bool(audit)),
        ("requestJson", opt(super::text_of(&row, "requestJson"))),
        ("responseJson", opt(super::text_of(&row, "responseJson"))),
    ]))
}

/// `get#PromptPreview`: the prefix of a new conversation of an agent on a surface and variant for a user, with a sample
/// tail, measured section by section, and the request as the model's protocol would send it. Nothing is stored.
fn get_prompt_preview(parameters: &Parameters, facade: &EntityFacade) -> Out {
    use super::prompt::{self, Catalog, PrefixSpec, PromptTexts, PromptVars, Surface};
    use super::tokens::{Estimator, Window};
    if !admin() {
        return Ok(BTreeMap::new());
    }
    let (Some(llm), Some(factory)) = (llm(), ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory())))
    else {
        return Ok(BTreeMap::new());
    };
    let agent_id = text(parameters, "agentId").unwrap_or_else(|| "assist".to_string());
    let viewer = crate::user::current_user_id().ok().flatten().filter(|id| !id.is_empty());
    let user_id = text(parameters, "userId").or_else(|| viewer.clone()).unwrap_or_default();
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let Some(agent) = facade
        .find("moqui.agent.AgentDefinition")?
        .eq("agentId", Value::string(&agent_id))
        .one()?
    else {
        add_error(format!("there is no agent {agent_id}"));
        return Ok(BTreeMap::new());
    };
    let surface = text(parameters, "surface")
        .or_else(|| {
            super::text_of(&agent, "surfaces").and_then(|list| list.split(',').next().map(|first| first.trim().to_string()))
        })
        .and_then(|name| Surface::parse(&name))
        .unwrap_or_default();
    let registry = llm.registry(facade);
    let model_id = text(parameters, "modelId");
    let feature = super::text_of(&agent, "featureId").unwrap_or_else(|| "assist".to_string());
    let tier = super::text_of(&agent, "tier").and_then(|tier| super::conf::Tier::parse(&tier));
    let resolved = match &model_id {
        Some(model_id) => registry.resolve_model(model_id, ReasoningLevel::None),
        None => registry.resolve(&registry::Want {
            feature: Some(&feature),
            tier,
            ..registry::Want::default()
        }),
    };
    let target = match resolved {
        Ok(target) => Some(target),
        Err(error) => {
            message(format!("No model resolves, so the defaults are used: {error}"), "warning");
            None
        }
    };
    let variant = text(parameters, "variant")
        .and_then(|variant| prompt::parse_variant(&variant))
        .or_else(|| target.as_ref().map(|target| target.model.behavior.prompt_variant))
        .unwrap_or_default();
    let user_facts = if user_id.is_empty() {
        super::facts::UserFacts::default()
    } else {
        super::facts::collect(&factory, facade, &user_id, &agent_id)
    };
    let facts = user_facts.facts.clone();
    let fact = |label: &str| facts.iter().find(|(name, _)| name == label).map(|(_, value)| value.clone());
    let vars = PromptVars {
        user_name: fact("name").or_else(|| fact("username")).unwrap_or_else(|| "the user".to_string()),
        language_name: prompt::language_name(fact("locale").as_deref()),
        ..PromptVars::default()
    };
    // another user's memory is their content: it is shown only for the viewer's own user
    let memory_shown = !user_id.is_empty() && viewer.as_deref() == Some(user_id.as_str());
    let memory = if memory_shown {
        prompt::user_memory(facade, &user_id, &agent_id)?
    } else {
        Vec::new()
    };
    let tools = prompt::offered_tools(surface, variant);
    // the catalog a conversation started on the sample page would list
    let index = super::skills::SkillIndex::for_audience(
        &factory,
        facade,
        &super::skills::Audience {
            user_id: Some(&user_id).filter(|id| !id.is_empty()).map(String::as_str),
            agent_id: Some(&agent_id),
            surface: Some(surface),
        },
    );
    let pins = super::skills::pins(facade, &agent_id);
    let tool_names: Vec<String> = tools.iter().map(|tool| tool.name.clone()).collect();
    let sample_page = text(parameters, "samplePath");
    let catalog: Catalog = super::skills::catalog(
        &index,
        &super::skills::CatalogInput {
            pins: &pins,
            first_text: "",
            page: sample_page.as_deref(),
            tools: &tool_names,
        },
    );
    let instructions = prompt::agent_instructions(&factory, &agent);
    let prefix = prompt::build_prefix(
        &PromptTexts::of(&factory),
        &PrefixSpec {
            agent_id: &agent_id,
            surface,
            variant,
            vars: &vars,
            agent_instructions: &instructions,
            org_instructions: &user_facts.org_instructions,
            catalog: &catalog,
            user_facts: &facts,
            memory: &memory,
            tools: &tools,
            tool_mode: target.as_ref().map(|target| target.model.behavior.tool_mode).unwrap_or_default(),
        },
    );
    let tail = prompt::sample_tail(surface, text(parameters, "samplePath").as_deref(), fact("time zone").as_deref());
    let estimator = target.as_ref().map(|target| Estimator::for_model(&target.model)).unwrap_or_default();
    let window = match &target {
        Some(target) => Window::for_model(&target.model, None, target.reasoning != ReasoningLevel::None),
        None => Window::for_model(&super::conf::ModelConf::new("default", "", ""), None, false),
    };
    let preview = prompt::preview(&prefix, &tail, &estimator, &window);
    let size = |part: &prompt::PartSize| {
        map([
            ("name", Value::string(&part.name)),
            ("chars", count(part.chars as u64)),
            ("tokens", count(part.tokens)),
            ("cap", part.cap.map_or(Value::Null, |cap| count(cap as u64))),
            ("share", Value::F64(part.share)),
        ])
    };
    let section_list = Value::List(preview.sections.iter().chain([&preview.tail]).map(size).collect());
    let tool_list = Value::List(preview.tools.iter().map(size).collect());
    let tools_hash = prefix.tools_hash.clone();
    let system_text = prefix.system_text.clone();
    let frozen = prompt::FrozenPrefix::from(prefix);
    let request = frozen.request(
        vec![Message::user("(the user's message)")],
        Some(tail.clone()),
        Some("PREVIEW"),
    );
    let request_json = target.as_ref().map(|target| {
        match provider::protocol(target.endpoint.protocol).encode(&request, target) {
            Ok(call) => serde_json::to_string_pretty(&call.body).unwrap_or_default(),
            Err(error) => format!("the request could not be encoded: {error}"),
        }
    });
    let tools_chars: usize = preview.tools.iter().map(|part| part.chars).sum();
    let totals = map([
        ("totalChars", count(preview.total_chars as u64)),
        ("totalTokens", count(preview.total_tokens)),
        ("toolsChars", count(tools_chars as u64)),
        ("toolCount", count(preview.tools.len() as u64)),
        ("contextWindow", count(window.context)),
        ("budget", count(window.budget)),
        ("highWatermark", count(window.high)),
        ("charsPerToken", Value::F64((estimator.k * 100.0).round() / 100.0)),
    ]);
    Ok(out([
        ("agentId", Value::string(&agent_id)),
        ("surface", Value::string(surface.as_str())),
        ("variant", Value::string(prompt::variant_str(variant))),
        ("modelId", opt(target.as_ref().map(|target| target.model.id.clone()))),
        ("systemText", Value::string(system_text)),
        ("tailText", Value::string(tail)),
        ("toolsHash", Value::string(tools_hash)),
        ("sectionList", section_list),
        ("toolList", tool_list),
        (
            "catalogList",
            Value::List(
                catalog
                    .entries
                    .iter()
                    .map(|entry| {
                        map([
                            ("name", Value::string(&entry.name)),
                            ("description", Value::string(&entry.description)),
                            ("kind", Value::string(&entry.kind)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("catalogMore", count(catalog.more as u64)),
        ("totals", totals),
        ("requestJson", opt(request_json)),
        ("memoryShown", Value::Bool(memory_shown)),
    ]))
}

/// The current user, with `AgentUse`; else an error is added and `None`.
fn agent_user() -> Option<String> {
    let user_id = crate::user::current_user_id().ok().flatten().filter(|id| !id.is_empty());
    let Some(user_id) = user_id else {
        add_error("an agent run needs a logged-in user");
        return None;
    };
    match crate::user::has_permission("AgentUse") {
        Ok(true) => Some(user_id),
        _ => {
            add_error("this needs the AgentUse permission");
            None
        }
    }
}

fn factory() -> Option<std::sync::Arc<crate::context::ExecutionContextFactory>> {
    ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory()))
}

/// `start#AgentRun`: a run of an agent for the current user (or the message handed to the conversation's active run).
fn start_agent_run(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let (Some(user_id), Some(factory)) = (agent_user(), factory()) else {
        return Ok(BTreeMap::new());
    };
    let Some(message_text) = text(parameters, "messageText") else {
        add_error("a run needs a messageText");
        return Ok(BTreeMap::new());
    };
    let surface = match text(parameters, "surface") {
        Some(name) => match super::prompt::Surface::parse(&name) {
            Some(surface) => surface,
            None => {
                add_error(format!("{name} is not an agent surface"));
                return Ok(BTreeMap::new());
            }
        },
        None => super::prompt::Surface::Drawer,
    };
    // a job, A2A and MCP run through their own adapters; from this service only an admin may start one
    if !matches!(surface, super::prompt::Surface::Drawer | super::prompt::Surface::Chat | super::prompt::Surface::Tui)
        && !crate::user::has_permission("AgentAdmin").unwrap_or(false)
    {
        add_error(format!("a run on the {} surface needs the AgentAdmin permission", surface.as_str()));
        return Ok(BTreeMap::new());
    }
    let request = super::run::StartRequest {
        user_id,
        agent_id: text(parameters, "agentId").unwrap_or_default(),
        conversation_id: text(parameters, "conversationId"),
        surface,
        text: message_text,
        tail: text(parameters, "tail"),
        client_request_id: text(parameters, "clientRequestId"),
        client_message_id: text(parameters, "clientMessageId"),
        ..super::run::StartRequest::default()
    };
    match super::run::start_run(&factory, &request) {
        Ok(started) => Ok(out([
            ("runId", Value::string(&started.run_id)),
            ("conversationId", Value::string(&started.conversation_id)),
            ("statusId", Value::string(started.state.status_id())),
            ("queued", Value::Bool(started.queued)),
        ])),
        Err(error) => {
            add_error(error.to_string());
            Ok(BTreeMap::new())
        }
    }
}

/// `cancel#AgentRun`: ask one of the current user's runs to stop.
fn cancel_agent_run(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let (Some(user_id), Some(factory)) = (agent_user(), factory()) else {
        return Ok(BTreeMap::new());
    };
    let run_id = text(parameters, "runId").unwrap_or_default();
    match super::run::cancel_run(&factory, &run_id, Some(&user_id)) {
        Ok(state) => Ok(out([("statusId", Value::string(state.status_id()))])),
        Err(error) => {
            add_error(error.to_string());
            Ok(BTreeMap::new())
        }
    }
}

/// `submit#AgentInput`: input for one of the current user's runs.
fn submit_agent_input(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let (Some(user_id), Some(factory)) = (agent_user(), factory()) else {
        return Ok(BTreeMap::new());
    };
    let run_id = text(parameters, "runId").unwrap_or_default();
    let kind = text(parameters, "inputKind").unwrap_or_else(|| "message".to_string());
    let data = parameters
        .get("data")
        .filter(|value| !is_empty(value))
        .map(moqui_lib::json::to_json);
    let message = text(parameters, "messageText").unwrap_or_default();
    match super::run::submit_input(&factory, &run_id, &user_id, &kind, &message, data.as_ref()) {
        Ok(state) => Ok(out([("statusId", Value::string(state.status_id()))])),
        Err(error) => {
            add_error(error.to_string());
            Ok(BTreeMap::new())
        }
    }
}

/// `get#AgentRunEvents`: the stored events of one of the current user's runs after `afterSeq`; another user's run is
/// not found.
fn get_agent_run_events(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let user_id = crate::user::current_user_id().ok().flatten().unwrap_or_default();
    let run_id = text(parameters, "runId").unwrap_or_default();
    let after = text(parameters, "afterSeq").and_then(|seq| seq.parse::<i64>().ok()).unwrap_or(0);
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let Some(run) = super::run::load_run(facade, &run_id)?.filter(|run| !user_id.is_empty() && run.user_id == user_id) else {
        add_error(format!("there is no agent run {run_id}"));
        return Ok(BTreeMap::new());
    };
    let events = super::run::events::stored_after(facade, &run_id, after, 1000)?;
    let last = events.last().map_or(after, |event| event.seq);
    Ok(out([
        (
            "eventList",
            Value::List(events.iter().map(|event| moqui_lib::json::from_json(&event.to_json())).collect()),
        ),
        ("lastEventSeq", Value::I64(last)),
        ("statusId", Value::string(run.state.status_id())),
    ]))
}

fn approval_error(error: &super::approval::DecideError, approval_id: &str) {
    match error {
        // another user's approval is not named
        super::approval::DecideError::NotFound => add_error(format!("there is no approval {approval_id}")),
        other => add_error(other.to_string()),
    }
}

/// `decide#AgentApproval`: the current user's decision on one of their approvals, only from an interactive session (a
/// browser session with its token, or the terminal); a header-authenticated request, an agent's screen session,
/// `agent-apps` and internal code are refused, and another user's approval is not found.
fn decide_agent_approval(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    use super::approval::{Channel, Decider, Decision};
    let (Some(_), Some(factory)) = (agent_user(), factory()) else {
        return Ok(BTreeMap::new());
    };
    let approval_id = text(parameters, "approvalId").unwrap_or_default();
    let decider = match Decider::from_request(text(parameters, "channel").and_then(|name| Channel::parse(&name))) {
        Ok(decider) => decider,
        Err(why) => {
            add_error(why);
            return Ok(BTreeMap::new());
        }
    };
    let decision = match text(parameters, "decision").as_deref() {
        Some("approve") => Decision::Approve {
            arguments: parameters
                .get("arguments")
                .filter(|value| !is_empty(value))
                .map(moqui_lib::json::to_json),
        },
        Some("reject") => Decision::Reject {
            feedback: text(parameters, "feedback").unwrap_or_default(),
        },
        Some("edit") => Decision::Edit,
        other => {
            add_error(format!(
                "{} is not a decision; it is approve, reject or edit",
                other.unwrap_or("nothing")
            ));
            return Ok(BTreeMap::new());
        }
    };
    match super::approval::decide(&factory, &approval_id, &decider, &decision) {
        Ok(decided) => Ok(out([
            ("statusId", Value::string(decided.status.status_id())),
            ("handoffId", opt(decided.handoff_id)),
        ])),
        Err(error) => {
            approval_error(&error, &approval_id);
            Ok(BTreeMap::new())
        }
    }
}

/// `preview#AgentApproval`: the effects of one of the current user's pending approvals, run in the sim; another
/// user's is not found.
fn preview_agent_approval(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let (Some(user_id), Some(factory)) = (agent_user(), factory()) else {
        return Ok(BTreeMap::new());
    };
    let approval_id = text(parameters, "approvalId").unwrap_or_default();
    match super::approval::preview::preview_approval(&factory, &approval_id, &user_id) {
        Ok(preview) => Ok(out([("preview", moqui_lib::json::from_json(&preview))])),
        Err(error) => {
            approval_error(&error, &approval_id);
            Ok(BTreeMap::new())
        }
    }
}

/// `get#AgentApproval`: one of the current user's approvals as its card shows it; another user's is not found.
fn get_agent_approval(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(user_id) = agent_user() else {
        return Ok(BTreeMap::new());
    };
    let approval_id = text(parameters, "approvalId").unwrap_or_default();
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    match super::approval::load_for_owner(facade, &approval_id, &user_id)? {
        Some(view) => {
            let mut card = view.to_json();
            card["canPreview"] = serde_json::json!(super::approval::preview::available(facade));
            Ok(out([("approval", moqui_lib::json::from_json(&card))]))
        }
        None => {
            add_error(format!("there is no approval {approval_id}"));
            Ok(BTreeMap::new())
        }
    }
}

/// `abandon#AgentHandoff`: the current user left a hand-off without saving (the drawer navigated away); the run goes
/// on with that. Only from an interactive session, only the watch's user.
fn abandon_agent_handoff(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    use super::approval::{Channel, Decider};
    let (Some(_), Some(factory)) = (agent_user(), factory()) else {
        return Ok(BTreeMap::new());
    };
    let handoff_id = text(parameters, "handoffId").unwrap_or_default();
    let decider = match Decider::from_request(text(parameters, "channel").and_then(|name| Channel::parse(&name))) {
        Ok(decider) => decider,
        Err(why) => {
            add_error(why);
            return Ok(BTreeMap::new());
        }
    };
    let reason = text(parameters, "reason").unwrap_or_else(|| "left".to_string());
    match super::approval::handoff::abandon_handoff(&factory, &handoff_id, &decider, &reason) {
        Ok(()) => Ok(BTreeMap::new()),
        Err(super::approval::DecideError::NotFound) => {
            add_error(format!("there is no hand-off {handoff_id}"));
            Ok(BTreeMap::new())
        }
        Err(error) => {
            add_error(error.to_string());
            Ok(BTreeMap::new())
        }
    }
}

/// `store#AgentToolExposure`: create or change a tool exposure, refused when the row moved on since the editor read
/// its `lockVersion` (a conflict, the row unchanged) or when what it says does not read: a reserved or visible tool
/// name, a service that is not defined, an unknown classification or preview mode, a schema override that is not a
/// JSON object. A field left out keeps its value; an empty one clears it. Out: the new `lockVersion` and the class the
/// service has of its own.
fn store_tool_exposure(parameters: &Parameters, facade: &EntityFacade) -> Out {
    use super::tools::catalog::{self, EXPOSURE};
    use super::tools::Classification;
    if !admin() {
        return Ok(BTreeMap::new());
    }
    let Some(tool_name) = text(parameters, "toolName") else {
        add_error("toolName is required");
        return Ok(BTreeMap::new());
    };
    if tool_name.chars().count() > 100 || tool_name.chars().any(char::is_whitespace) {
        add_error(format!("the tool name {tool_name} has white space or is over 100 characters"));
        return Ok(BTreeMap::new());
    }
    if catalog::reserved(&tool_name) {
        add_error(format!("the tool name {tool_name} takes a prefix kept for MCP and A2A tools (mcp__, a2a__)"));
        return Ok(BTreeMap::new());
    }
    if super::tools::registered()
        .iter()
        .any(|tool| tool.spec().name == tool_name && !tool.spec().lazy)
    {
        add_error(format!("{tool_name} is the name of a tool of its own; choose another"));
        return Ok(BTreeMap::new());
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let existing = facade
        .find(EXPOSURE)?
        .eq("toolName", Value::string(&tool_name))
        .for_update(true)
        .one()?;
    let read_version = parameters.get("lockVersion").filter(|value| !is_empty(value)).map(|value| {
        to_plain_string(value).trim().parse::<i64>().unwrap_or(-1)
    });
    let stored_version = existing.as_ref().map(|row| super::int_of(row, "lockVersion").unwrap_or(0));
    match (stored_version, read_version) {
        (Some(stored), Some(read)) if stored != read => {
            add_error(format!(
                "the tool {tool_name} was changed by someone else since you opened it (it is version {stored} now, you read \
                 {read}); reload it and make your change again"
            ));
            return Ok(BTreeMap::new());
        }
        (Some(stored), None) => {
            add_error(format!(
                "the tool {tool_name} exists (version {stored}); open it and save with its lockVersion to change it"
            ));
            return Ok(BTreeMap::new());
        }
        (None, Some(read)) if read != 0 => {
            add_error(format!("the tool {tool_name} was removed since you opened it"));
            return Ok(BTreeMap::new());
        }
        _ => {}
    }
    let given = |field: &str| parameters.contains_key(field);
    let service_name = text(parameters, "serviceName")
        .or_else(|| existing.as_ref().and_then(|row| super::text_of(row, "serviceName")));
    let Some(service_name) = service_name else {
        add_error("serviceName is required");
        return Ok(BTreeMap::new());
    };
    let service = match crate::service::call::ToolService::of(&service_name) {
        Ok(service) => service,
        Err(error) => {
            add_error(format!("{service_name} is not a service that can be a tool: {error}"));
            return Ok(BTreeMap::new());
        }
    };
    if let Some(class) = text(parameters, "classificationEnumId") {
        if Classification::parse(&class).is_none() {
            add_error(format!("{class} is not a classification (AgclRead, AgclWrite, AgclDestructive, AgclExternal)"));
            return Ok(BTreeMap::new());
        }
    }
    if let Some(mode) = text(parameters, "previewMode") {
        if super::approval::PreviewMode::parse(&mode).is_none() {
            add_error(format!("{mode} is not a preview mode (none, on-demand, always)"));
            return Ok(BTreeMap::new());
        }
    }
    if let Some(schema) = text(parameters, "schemaOverrideJson") {
        match serde_json::from_str::<serde_json::Value>(&schema) {
            Ok(value) if value.is_object() => {}
            Ok(_) => {
                add_error("the schema override is not a JSON object");
                return Ok(BTreeMap::new());
            }
            Err(error) => {
                add_error(format!("the schema override does not read as JSON: {error}"));
                return Ok(BTreeMap::new());
            }
        }
    }
    let version = stored_version.map_or(1, |stored| stored + 1);
    let mut row = match existing {
        Some(row) => row,
        None => {
            let mut row = facade.make_value(EXPOSURE)?;
            row.set("toolName", Value::string(&tool_name));
            row
        }
    };
    row.set("serviceName", Value::string(service.name()));
    for field in [
        "title",
        "description",
        "schemaOverrideJson",
        "classificationEnumId",
        "exposeAssistant",
        "exposeMcp",
        "previewMode",
        "disabled",
    ] {
        if given(field) {
            row.set(field, text(parameters, field).map_or(Value::Null, Value::string));
        }
    }
    row.set("lockVersion", Value::I64(version));
    if stored_version.is_some() {
        row.update(facade)?;
    } else {
        row.create(facade)?;
    }
    Ok(out([
        ("lockVersion", Value::I64(version)),
        (
            "serviceClassification",
            Value::string(catalog::service_classification(&service).enum_id()),
        ),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::conf::{EndpointConf, ModelConf, ProtocolId};
    use crate::llm::provider::fake;

    fn target(id: &str) -> registry::Target {
        fake::clear(id);
        registry::Target {
            endpoint: EndpointConf::new(id, ProtocolId::Fake, ""),
            model: ModelConf::new(format!("{id}-model"), id, "fake-1"),
            reasoning: ReasoningLevel::None,
            feature: None,
            fell_back_from: None,
        }
    }

    fn probe(capability: &'static str, target: &registry::Target) -> Probe {
        let mut options = CallOptions::new(usage::CallSource::Probe);
        options.max_attempts = 1;
        probe_one(capability, target, &mut |request| provider::call(target, request, &mut options))
    }

    #[test]
    fn a_probe_whose_call_fails_gives_no_verdict_and_a_refused_feature_is_no() {
        let target = target("probe-fails");
        assert_eq!(probe("tools", &target).verdict, Some(Verdict::Yes));
        fake::script("probe-fails", vec![fake::Reply::status(503, "")]);
        let failed = probe("tools", &target);
        assert_eq!(failed.verdict, None);
        assert!(failed.evidence.starts_with("no verdict"), "{}", failed.evidence);
        // the second call of the cache probe is rate limited: still no verdict
        fake::script(
            "probe-fails",
            vec![fake::Reply::text("7"), fake::Reply::status(429, r#"{"error":{"message":"slow down"}}"#)],
        );
        assert_eq!(probe("prompt-caching", &target).verdict, None);
        // a server that refuses the schema says no
        fake::script(
            "probe-fails",
            vec![fake::Reply::status(400, r#"{"error":{"message":"response_format is not supported"}}"#)],
        );
        assert_eq!(probe("json-schema", &target).verdict, Some(Verdict::No));
    }
}
