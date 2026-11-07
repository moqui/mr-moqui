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
//! ec.llm.isEnabled() -> boolean
//! ec.llm.summarize(text, {maxWords}) -> string
//! ec.llm.classify(text, labels) -> label
//! ec.llm.extract(text, fields) -> {name: value}          fields: {name: description}
//! ec.llm.startRun(agentId, text) -> runId                  a background run as the current user (job surface; AgentAdmin)
//! ```
//!
//! `summarize`, `classify` and `extract` are the typed AI tasks of `org.moqui.agent.AiTaskServices`
//! ([`crate::ai_task`]), on their features' models.
//!
//! A model call is refused while a transaction is open (plans/High-Level-Review.md HL-041, [`crate::provider::call`]):
//! call these from code with none open, or pass `allowInTransaction: true` to `complete`, which suspends the
//! transaction for the call and cuts the call to the time it has left.
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
        host_method!("ec.llm", "isEnabled", [], is_enabled),
        host_method!("ec.llm", "summarize", [Text, OptValue], None, summarize),
        host_method!("ec.llm", "classify", [Text, Value], None, classify),
        host_method!("ec.llm", "extract", [Text, Value], None, extract),
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
    let (factory, facade) = (factory()?, entity()?);
    let summary = crate::ai_task::summarize(&factory, &facade, &text, max_words).map_err(HostError::Lib)?;
    Ok(summary.into())
}

fn classify(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let text = call.text(0).unwrap_or_default().to_string();
    let labels: Vec<String> = match call.value_or_null(1) {
        Value::List(items) => items.iter().map(moqui_lib::object::to_plain_string).collect(),
        _ => return Err(HostError::message("ec.llm.classify takes a list of labels")),
    };
    let (factory, facade) = (factory()?, entity()?);
    let label = crate::ai_task::classify(&factory, &facade, &text, &labels).map_err(HostError::Lib)?;
    Ok(label.into())
}

fn extract(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let text = call.text(0).unwrap_or_default().to_string();
    let fields = moqui_lib::json::to_json(&call.value_or_null(1));
    let (factory, facade) = (factory()?, entity()?);
    let values = crate::ai_task::extract(&factory, &facade, &text, &fields).map_err(HostError::Lib)?;
    Ok(Ret::Value(moqui_lib::json::from_json(&values)))
}

fn start_run(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let (agent_id, text) = (call.text(0).unwrap_or_default(), call.text(1).unwrap_or_default());
    let run_id = crate::ai_task::start_run(&factory()?, agent_id, text).map_err(HostError::Lib)?;
    Ok(run_id.into())
}

fn text_option(map: &indexmap::IndexMap<String, moqui_lib::object::Value>, key: &str) -> Option<String> {
    map.get(key)
        .filter(|value| !value.is_null())
        .map(moqui_lib::object::to_plain_string)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn complete(call: &mut Call<'_>) -> Result<Ret, HostError> {
    {
        let Value::Map(map) = call.value_or_null(0) else {
            return Err(HostError::message("ec.llm.complete takes an options map"));
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
        let (factory, facade) = (factory()?, entity()?);
        let completion = crate::facade(&factory)
            .complete(&facade, &ask)
            .map_err(HostError::Lib)?;
        let mut out = indexmap::IndexMap::new();
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
        Ok(Ret::Value(Value::Map(out)))
    }
}

#[cfg(test)]
mod tests {
    use moqui_core::context::ExecutionContextFactory;
    use moqui_core::script::{clear_context_stack, eval_condition, run_script};

    const CONF: &str = r#"<moqui-conf><llm-facade>
        <endpoint id="script-fake" protocol="fake"/>
        <model id="script-model" endpoint="script-fake" name="fake-1"/>
        <bind feature="classify" model="script-model" reasoning="none"/>
        <bind feature="summarize" model="script-model" reasoning="none"/>
        <bind feature="extract" model="script-model" reasoning="none"/>
    </llm-facade></moqui-conf>"#;

    #[test]
    fn ec_llm_complete_answers_with_the_checked_json_of_a_schema() {
        clear_context_stack();
        let context = ExecutionContextFactory::test_context_with_conf(CONF);
        context.factory().set_entity(moqui_core::testing::open());
        let _entered = context.enter();
        crate::provider::fake::clear("script-fake");
        let holds = |expression: &str| assert!(eval_condition(expression).unwrap(), "{expression}");
        holds("ec.llm.isEnabled() === true");
        holds(
            r#"(function () {
                const answer = ec.llm.complete({feature: "classify", prompt: "Invoice #12",
                    schema: {type: "object", required: ["label"], properties: {label: {enum: ["invoice", "other"]}}}});
                return answer.json.label === "invoice" && answer.modelId === "script-model" && answer.finish === "stop"
                    && answer.callIds.length === 1 && answer.inputTokens > 0;
            })()"#,
        );
        holds(r#"ec.llm.complete({model: "script-model", prompt: "Say hi"}).text === "Fake reply to: Say hi""#);
        // the typed AI tasks: their schemas give the fake's answers
        holds(r#"ec.llm.classify("Invoice #12", ["invoice", "other"]) === "invoice""#);
        holds(r#"typeof ec.llm.summarize("A long text.", {maxWords: 20}) === "string""#);
        holds(
            r#"(function () { const values = ec.llm.extract("Ship to Acme", {customer: "The customer"}); return "customer" in values; })()"#,
        );
        for (script, expected) in [
            (
                r#"ec.llm.complete({model: "script-model"})"#,
                "needs a prompt or messages",
            ),
            (r#"ec.llm.complete({tier: "huge", prompt: "x"})"#, "tier huge"),
            (
                r#"ec.llm.complete({model: "nope", prompt: "x"})"#,
                "model nope is not configured",
            ),
        ] {
            let error = run_script(script).unwrap_err().to_string();
            assert!(error.contains(expected), "{script}: {error}");
        }
    }
}
