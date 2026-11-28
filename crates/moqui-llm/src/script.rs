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

//! `ec.llm`, for app code (plans/LLM-A2A-Assist.md 4.14), installed through moqui_core::llm_hooks.
//!
//! ```text
//! ec.llm.complete({prompt, system, messages, model, feature, tier, schema, schemaName, reasoning, maxOutput,
//!         timeoutSeconds, allowInTransaction})
//!     -> {text, json, modelId, finish, inputTokens, cachedInputTokens, outputTokens, cost, callIds}
//! ec.llm.stream(options, onText)    options as complete; onText(textSoFar) runs each time more text comes
//!     -> as complete; onText returning false stops the call: then the text so far, finish "canceled" and no json
//! ec.llm.embed(texts, {model, feature, allowInTransaction}) -> {vectors, dimension, modelId, modelName}
//! ec.llm.isEnabled() -> boolean
//! ec.llm.summarize(text, {maxWords, allowInTransaction}) -> string
//! ec.llm.classify(text, labels, {allowInTransaction}) -> label
//! ec.llm.extract(text, fields, {allowInTransaction}) -> {name: value}    fields: {name: description}
//! ec.llm.startRun(agentId, text) -> runId                  a background run as the current user (job surface; AgentAdmin)
//! ```
//!
//! `summarize`, `classify` and `extract` are the typed AI tasks of `org.moqui.agent.AiTaskServices`
//! ([`crate::ai_task`]), on their features' models.
//!
//! A model call is refused while a transaction is open (plans/archive/High-Level-Review.md HL-041, [`crate::provider::call`]):
//! call these from code with none open, or pass `allowInTransaction: true` in the options of `complete`, `summarize`,
//! `classify`, `extract` or `embed`, which suspends the transaction for the call and cuts the call to the time it has left.
//!
//! `messages` are earlier turns, each `{role: "user" | "assistant", text}`; `schema` is a JSON schema (a map or its
//! text), and with it `json` is the reply parsed and checked. The model is `model`, else the binding of `feature`, else
//! a model of `tier`. A call that ends without an answer throws.

use moqui_core::host_method;
use moqui_core::script::host::{with_ec, Call, HostClass, HostError, Path, Ret};
use moqui_lib::object::Value;

use crate::conf::{ReasoningLevel, Tier};
use crate::provider::Message;
use crate::CompleteRequest;

/// The paths this crate adds to the host table (`moqui_core::llm_hooks`).
pub(crate) static PATHS: &[Path] = &[Path::object("ec.llm", &LLM)];

static LLM: HostClass = HostClass {
    name: "host:Llm",
    methods: &[
        host_method!("ec.llm", "complete", [Value], None, complete),
        host_method!("ec.llm", "stream", [Value, Callback], None, leaf, stream),
        host_method!("ec.llm", "embed", [Value, OptValue], None, embed),
        host_method!("ec.llm", "isEnabled", [], is_enabled),
        host_method!("ec.llm", "summarize", [Text, OptValue], None, summarize),
        host_method!("ec.llm", "classify", [Text, Value, OptValue], None, classify),
        host_method!("ec.llm", "extract", [Text, Value, OptValue], None, extract),
        host_method!("ec.llm", "startRun", [Text, Text], None, start_run),
    ],
    getters: &[],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

fn factory() -> Result<std::sync::Arc<moqui_core::context::ExecutionContextFactory>, HostError> {
    with_ec(|ec| ec.factory().clone())
}

/// The entity facade of the entered context; the error the facades throw when there is none.
fn entity() -> Result<moqui_core::entity::EntityFacade, HostError> {
    with_ec(|ec| ec.factory().entity())?.ok_or_else(|| HostError::message("no entity facade is installed"))
}

/// `ec.llm.embed(texts, {feature|model, allowInTransaction})`. One vector per text, from the model's `/v1/embeddings`
/// ([`crate::embed`]); `texts` may be one string. A text that is blank, or not a string or a number, is refused,
/// naming its index, so the vectors stay one per text in the caller's order. An open transaction is refused unless
/// `allowInTransaction` is set, and then it is suspended for the call, which is cut to the time it has left
/// ([`crate::provider::outside_transaction`]). An empty list returns no vectors and does not call the model.
fn embed(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let items = match call.value_or_null(0) {
        Value::List(items) => items,
        text @ Value::String(_) => vec![text],
        _ => return Err(HostError::message("ec.llm.embed takes a list of texts")),
    };
    let mut texts = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let text = match item {
            Value::String(_) | Value::I32(_) | Value::I64(_) | Value::F64(_) | Value::Decimal(_) => {
                moqui_lib::object::to_plain_string(item)
            }
            _ => {
                return Err(HostError::message(format!(
                    "ec.llm.embed text {index} is not a string or a number"
                )))
            }
        };
        if text.trim().is_empty() {
            return Err(HostError::message(format!(
                "ec.llm.embed text {index} is blank; every text gets a vector in its place, so pass none blank"
            )));
        }
        texts.push(text);
    }
    let options = call.value(1);
    let (model, feature, allow) = match options {
        Some(Value::Map(map)) => (
            text_option(map, "model"),
            text_option(map, "feature"),
            allows_transaction(options),
        ),
        _ => (None, None, false),
    };
    if model.is_none() && feature.is_none() {
        return Err(HostError::message("ec.llm.embed needs a feature or a model"));
    }
    let (factory, facade) = (factory()?, entity()?);
    let llm = crate::facade(&factory);
    let target = llm
        .resolve(
            &facade,
            &crate::registry::Want {
                model: model.as_deref(),
                feature: feature.as_deref(),
                ..crate::registry::Want::default()
            },
        )
        .map_err(HostError::Lib)?;
    if let Some(refusal) = crate::provider::transaction_refusal(&target.model.id, allow) {
        return Err(HostError::message(refusal));
    }
    let vectors = crate::provider::outside_transaction(Some(&facade), &target.model.id, |ends| {
        llm.embed(&facade, &target, &texts, ends)
    })
    .and_then(|embedded| embedded)
    .map_err(HostError::Lib)?;
    let dimension = vectors.first().map_or(0, Vec::len);
    let mut rows = Vec::with_capacity(vectors.len());
    for vector in vectors {
        rows.push(Value::List(
            vector.into_iter().map(|value| Value::F64(f64::from(value))).collect(),
        ));
    }
    let mut out = indexmap::IndexMap::default();
    out.insert("vectors".to_string(), Value::List(rows));
    out.insert(
        "dimension".to_string(),
        Value::I64(i64::try_from(dimension).unwrap_or(0)),
    );
    out.insert("modelId".to_string(), Value::String(target.model.id.clone()));
    out.insert("modelName".to_string(), Value::String(target.model.name.clone()));
    Ok(Ret::Value(Value::Map(out)))
}

fn is_enabled(_: &mut Call<'_>) -> Result<Ret, HostError> {
    let factory = factory()?;
    Ok(crate::facade(&factory).enabled().into())
}

fn summarize(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let text = call.text(0).unwrap_or_default().to_string();
    let max_words = match call.value(1) {
        Some(Value::Map(map)) => text_option(map, "maxWords").and_then(|text| text.parse().ok()),
        _ => None,
    };
    let allow = allows_transaction(call.value(1));
    let (factory, facade) = (factory()?, entity()?);
    let summary = crate::ai_task::summarize(&factory, &facade, &text, max_words, allow).map_err(HostError::Lib)?;
    Ok(summary.into())
}

fn classify(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let text = call.text(0).unwrap_or_default().to_string();
    let labels: Vec<String> = match call.value_or_null(1) {
        Value::List(items) => items.iter().map(moqui_lib::object::to_plain_string).collect(),
        _ => return Err(HostError::message("ec.llm.classify takes a list of labels")),
    };
    let allow = allows_transaction(call.value(2));
    let (factory, facade) = (factory()?, entity()?);
    let label = crate::ai_task::classify(&factory, &facade, &text, &labels, allow).map_err(HostError::Lib)?;
    Ok(label.into())
}

fn extract(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let text = call.text(0).unwrap_or_default().to_string();
    let fields = moqui_lib::json::to_json(&call.value_or_null(1));
    let allow = allows_transaction(call.value(2));
    let (factory, facade) = (factory()?, entity()?);
    let values = crate::ai_task::extract(&factory, &facade, &text, &fields, allow).map_err(HostError::Lib)?;
    Ok(Ret::Value(moqui_lib::json::from_json(&values)))
}

fn start_run(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let (agent_id, text) = (call.text(0).unwrap_or_default(), call.text(1).unwrap_or_default());
    let run_id = crate::ai_task::start_run(&factory()?, agent_id, text).map_err(HostError::Lib)?;
    Ok(run_id.into())
}

/// `allowInTransaction` of an options map: true or `Y` lets the model call be made with a transaction open (HL-041).
fn allows_transaction(options: Option<&Value>) -> bool {
    match options {
        Some(Value::Map(map)) => matches!(text_option(map, "allowInTransaction").as_deref(), Some("true" | "Y")),
        _ => false,
    }
}

fn text_option(map: &moqui_lib::object::ValueMap, key: &str) -> Option<String> {
    map.get(key)
        .filter(|value| !value.is_null())
        .map(moqui_lib::object::to_plain_string)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn complete(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let ask = complete_request(call, "complete")?;
    let (factory, facade) = (factory()?, entity()?);
    let completion = crate::facade(&factory)
        .complete(&facade, &ask)
        .map_err(HostError::Lib)?;
    Ok(Ret::Value(completion_value(completion)))
}

/// `ec.llm.stream(options, onText)`: [`complete`] with `onText` called with the text so far as it comes. A leaf, so it
/// can call back: the checks before the call (the model, the quota, which may run nested scripts) run with the engine
/// lent, then the call runs and calls back on this thread. A callback that throws stops the call, and the error is
/// thrown.
fn stream(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let ask = complete_request(call, "stream")?;
    let (factory, facade) = (factory()?, entity()?);
    let llm = crate::facade(&factory);
    let mut prepared = None;
    call.ops
        .lend(&mut || prepared = Some(llm.prepare_complete(&facade, &ask)))?;
    let prepared = prepared
        .ok_or_else(|| HostError::message("ec.llm.stream did not prepare the call"))?
        .map_err(HostError::Lib)?;
    let mut failed = None;
    let ops = &mut *call.ops;
    let mut on_text = |text: &str| match ops.call_back(0, vec![Value::String(text.to_string())]) {
        Ok(Value::Bool(false)) => false,
        Ok(_) => true,
        Err(error) => {
            failed = Some(error);
            false
        }
    };
    let completion = llm.run_complete(&facade, &ask, prepared, Some(&mut on_text));
    if let Some(error) = failed {
        return Err(error);
    }
    Ok(Ret::Value(completion_value(completion.map_err(HostError::Lib)?)))
}

/// The [`crate::CompleteRequest`] of `ec.llm.complete`'s (or `stream`'s) options map.
fn complete_request(call: &Call<'_>, name: &str) -> Result<CompleteRequest, HostError> {
    {
        let Value::Map(map) = call.value_or_null(0) else {
            return Err(HostError::message(format!("ec.llm.{name} takes an options map")));
        };
        let mut ask = CompleteRequest {
            model: text_option(&map, "model"),
            feature: text_option(&map, "feature"),
            system: text_option(&map, "system"),
            prompt: text_option(&map, "prompt"),
            schema_name: text_option(&map, "schemaName"),
            ..CompleteRequest::default()
        };
        if let Some(tier) = text_option(&map, "tier") {
            ask.tier = Some(
                Tier::parse(&tier)
                    .ok_or_else(|| HostError::message(format!("tier {tier} is not fast, balanced or thorough")))?,
            );
        }
        if let Some(level) = text_option(&map, "reasoning") {
            ask.reasoning = Some(ReasoningLevel::parse(&level).ok_or_else(|| {
                HostError::message(format!("reasoning {level} is not none, low, medium, high or max"))
            })?);
        }
        ask.max_output = text_option(&map, "maxOutput").and_then(|text| text.parse().ok());
        ask.allow_in_transaction = matches!(text_option(&map, "allowInTransaction").as_deref(), Some("true" | "Y"));
        ask.timeout = text_option(&map, "timeoutSeconds")
            .and_then(|text| text.parse::<u64>().ok())
            .map(std::time::Duration::from_secs);
        ask.schema = match map.get("schema") {
            None | Some(moqui_lib::object::Value::Null) => None,
            Some(moqui_lib::object::Value::String(text)) => Some(
                serde_json::from_str(text)
                    .map_err(|error| HostError::message(format!("the schema is not JSON: {error}")))?,
            ),
            Some(value) => Some(moqui_lib::json::to_json(value)),
        };
        if let Some(moqui_lib::object::Value::List(messages)) = map.get("messages") {
            for message in messages {
                let moqui_lib::object::Value::Map(message) = message else {
                    continue;
                };
                let text = text_option(message, "text")
                    .or_else(|| text_option(message, "content"))
                    .unwrap_or_default();
                ask.messages.push(match text_option(message, "role").as_deref() {
                    Some("assistant") => Message::assistant(text),
                    _ => Message::user(text),
                });
            }
        }
        Ok(ask)
    }
}

/// A completion as the script sees it.
fn completion_value(completion: crate::Completion) -> Value {
    {
        let mut out = indexmap::IndexMap::default();
        let count = |value: u64| moqui_lib::object::Value::I64(i64::try_from(value).unwrap_or(i64::MAX));
        out.insert("text".to_string(), moqui_lib::object::Value::String(completion.text));
        out.insert(
            "json".to_string(),
            completion
                .json
                .as_ref()
                .map_or(moqui_lib::object::Value::Null, moqui_lib::json::from_json),
        );
        out.insert(
            "modelId".to_string(),
            moqui_lib::object::Value::String(completion.model_id),
        );
        out.insert(
            "finish".to_string(),
            moqui_lib::object::Value::String(completion.finish),
        );
        out.insert("inputTokens".to_string(), count(completion.usage.input));
        out.insert("cachedInputTokens".to_string(), count(completion.usage.cached_input));
        out.insert("outputTokens".to_string(), count(completion.usage.output));
        out.insert(
            "cost".to_string(),
            completion
                .cost
                .map_or(moqui_lib::object::Value::Null, moqui_lib::object::Value::Decimal),
        );
        out.insert(
            "callIds".to_string(),
            moqui_lib::object::Value::List(
                completion
                    .call_ids
                    .into_iter()
                    .map(moqui_lib::object::Value::String)
                    .collect(),
            ),
        );
        Value::Map(out)
    }
}

#[cfg(test)]
#[path = "script_tests.rs"]
mod tests;
