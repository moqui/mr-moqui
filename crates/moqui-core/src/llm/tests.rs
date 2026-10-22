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

//! The `llm-facade` conf, the registry over the database rows, usage and cost, quotas and the run state table against
//! its seed rows, on the in-memory database.

use std::collections::BTreeSet;
use std::str::FromStr;

use moqui_lib::node::MNode;
use moqui_lib::object::{to_plain_string, Value};
use rust_decimal::Decimal;

use super::conf::{LlmConf, Prices, ProtocolId, ReasoningLevel, Tier, ToolMode};
use super::registry::{self, Registry, Source, Verdict, VerdictSource, Want};
use super::run::state::{RunState, TRANSITIONS};
use super::usage::{self, CallRecord, CallSource, Usage};
use crate::entity::EntityFacade;
use crate::testing;

fn dec(text: &str) -> Decimal {
    Decimal::from_str(text).unwrap()
}

const CONF: &str = r#"<moqui-conf><llm-facade>
    <endpoint id="local" protocol="chat-completions" url="http://127.0.0.1:1234/" local="true" max-concurrent="1"/>
    <endpoint id="hosted" protocol="anthropic-messages" url="https://api.anthropic.com" api-key="sk-ant-0123456789abcdef">
        <header name="anthropic-version" value="2023-06-01"/></endpoint>
    <endpoint id="bad" protocol="smoke-signals" url="http://x"/>
    <model id="local-qwen" endpoint="local" name="qwen3.8-27b" tier="balanced" context-window="32768" max-output="6144"
            tool-mode="json" screen-size="100x30" max-tool-calls="12">
        <api-config>{"chat_template_kwargs":{"preserve_thinking":true}}</api-config>
        <reasoning level="low">{"reasoning_effort":"low"}</reasoning>
        <reasoning level="medium">not json</reasoning>
    </model>
    <model id="hosted-sonnet" endpoint="hosted" name="claude-sonnet-5" tier="thorough" price-input="3" price-cached-input="0.3"
            price-cache-write="3.75" price-output="15"/>
    <model id="unnamed" endpoint="local" tier="fast"/>
    <model id="orphan" endpoint="nowhere" name="x"/>
    <bind feature="assist" model="local-qwen" fallback="hosted-sonnet" reasoning="medium"/>
    <bind feature="title" model="unnamed" reasoning="none"/>
    <limits user-daily-tokens="1000" user-daily-cost="0.5"/>
    <content-log enabled="true" redact-keys="ssn"/>
</llm-facade></moqui-conf>"#;

#[test]
fn the_conf_reads_its_entries_and_names_the_bad_ones() {
    let conf = LlmConf::from_conf_text(CONF).unwrap();
    assert!(conf.enabled);
    assert_eq!(
        conf.endpoints.iter().map(|endpoint| endpoint.id.as_str()).collect::<Vec<_>>(),
        ["local", "hosted"]
    );
    let local = conf.endpoint("local").unwrap();
    assert_eq!(local.request_url(), "http://127.0.0.1:1234/v1/chat/completions");
    assert!(local.local && local.max_concurrent == Some(1) && local.auth().is_none());
    let hosted = conf.endpoint("hosted").unwrap();
    assert_eq!(hosted.request_url(), "https://api.anthropic.com/v1/messages");
    assert_eq!(
        hosted.auth(),
        Some(("x-api-key".to_string(), "sk-ant-0123456789abcdef".to_string()))
    );
    assert_eq!(hosted.headers, [("anthropic-version".to_string(), "2023-06-01".to_string())]);
    // the key does not show in debug output
    assert!(!format!("{hosted:?}").contains("0123456789"));

    let qwen = conf.model("local-qwen").unwrap();
    assert_eq!(qwen.context_window, Some(32768));
    assert_eq!(qwen.behavior.tool_mode, ToolMode::Json);
    assert_eq!(qwen.behavior.screen_size, Some((100, 30)));
    assert_eq!(qwen.behavior.max_tool_calls, Some(12));
    assert_eq!(qwen.reasoning.keys().copied().collect::<Vec<_>>(), [ReasoningLevel::Low]);
    assert_eq!(qwen.api_config.as_ref().unwrap()["chat_template_kwargs"]["preserve_thinking"], true);
    let sonnet = conf.model("hosted-sonnet").unwrap();
    assert_eq!(sonnet.tier, Tier::Thorough);
    assert_eq!(sonnet.prices.cached_input, Some(dec("0.3")));
    assert_eq!(sonnet.prices.currency, "USD");

    assert_eq!(conf.bind("assist").unwrap().reasoning, ReasoningLevel::Medium);
    assert_eq!(conf.limits.user_daily_tokens, 1000);
    assert_eq!(conf.limits.user_daily_cost, Some(dec("0.5")));
    assert_eq!(conf.limits.max_steps, 40);
    assert!(conf.content_log.enabled && conf.content_log.redact);
    assert_eq!(conf.content_log.redact_keys, ["ssn"]);
    assert!(!conf.a2a.enabled && !conf.mcp.server_enabled && !conf.gateway.enabled);

    let problems = conf.problems.join("\n");
    assert!(problems.contains("endpoint bad protocol smoke-signals"), "{problems}");
    assert!(problems.contains("model local-qwen reasoning medium"), "{problems}");
    assert!(problems.contains("model orphan names endpoint nowhere"), "{problems}");
    assert_eq!(conf.problems.len(), 3, "{problems}");
}

#[test]
fn the_shipped_conf_reads_without_problems() {
    let path = testing::framework_dir().join("conf/MoquiDefaultConf.xml");
    let conf = LlmConf::from_conf(&MNode::parse_file(path).unwrap());
    assert!(conf.problems.is_empty(), "{:?}", conf.problems);
    assert!(conf.enabled);
    let endpoint = conf.endpoint("default").unwrap();
    assert_eq!(endpoint.protocol, ProtocolId::ChatCompletions);
    assert_eq!(conf.model("default").unwrap().endpoint, "default");
    for feature in ["assist", "title", "summarize", "classify", "ai-field"] {
        assert_eq!(conf.bind(feature).unwrap().model, "default", "{feature}");
    }
    assert_eq!(conf.bind("title").unwrap().reasoning, ReasoningLevel::None);
    assert_eq!(conf.limits, super::conf::LimitsConf::default());
    assert_eq!(conf.content_log.retention_days, 14);
    assert_eq!(conf.a2a.default_agent, "assist");
    assert!(conf.a2a.security.api_key && !conf.a2a.push.allow_http);
    assert_eq!(conf.mcp.protocol_version, "2026-07-28");
    assert!(conf.gateway.require_client);
    assert!(conf.data_entity_readable("moqui.basic", "StatusItem"));
    assert!(!conf.data_entity_readable("moqui.basic.email", "EmailServer"));
    assert!(!conf.data_entity_readable("moqui.security", "UserAccount"));
}

#[test]
fn data_entities_merge_by_package_and_entity_and_one_may_be_turned_off() {
    let base = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><llm-facade><data-entity package="moqui.basic"/></llm-facade></moqui-conf>"#,
    )
    .unwrap();
    let over = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><llm-facade>
            <data-entity package="moqui.basic" entity="Geo.*" readable="false"/>
            <data-entity package="example" entity="Example(Item)?" secret-fields="true"/>
        </llm-facade></moqui-conf>"#,
    )
    .unwrap();
    crate::context::merge_conf(&base, &over);
    let conf = LlmConf::from_conf(&base);
    assert_eq!(conf.data_entities.len(), 3);
    assert!(conf.data_entity_readable("moqui.basic", "Enumeration"));
    assert!(!conf.data_entity_readable("moqui.basic", "GeoAssoc"));
    assert!(conf.data_entity_readable("example", "ExampleItem"));
    assert!(!conf.data_entity_readable("example", "ExampleFeature"));
    assert!(conf.data_entity_secrets("example", "ExampleItem") && !conf.data_entity_secrets("moqui.basic", "Enumeration"));
    // none listed is the moqui.basic package
    let plain = LlmConf::from_conf(&MNode::parse_text("moqui-conf", "<moqui-conf><llm-facade/></moqui-conf>").unwrap());
    assert!(plain.data_entity_readable("moqui.basic", "Uom") && !plain.data_entity_readable("other", "Uom"));
}

#[test]
fn a_runtime_conf_merges_into_the_llm_facade_by_id() {
    let base = MNode::parse_text("moqui-conf", CONF).unwrap();
    let over = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><llm-facade>
            <endpoint id="local" url="http://10.0.0.5:8080"><header name="X-Team" value="ops"/></endpoint>
            <endpoint id="lmstudio" url="http://127.0.0.1:1234" local="true"/>
            <model id="local-qwen" max-output="4096"><reasoning level="high">{"reasoning_effort":"high"}</reasoning>
                <api-config>{"top_k":20}</api-config></model>
            <bind feature="title" model="local-qwen"/>
            <limits user-daily-cost="5"/>
            <a2a enabled="true"><security bearer="false"/></a2a>
        </llm-facade></moqui-conf>"#,
    )
    .unwrap();
    crate::context::merge_conf(&base, &over);
    let conf = LlmConf::from_conf(&base);
    let local = conf.endpoint("local").unwrap();
    assert_eq!(local.url, "http://10.0.0.5:8080");
    // the base attributes stay
    assert!(local.local && local.max_concurrent == Some(1));
    assert_eq!(local.headers, [("X-Team".to_string(), "ops".to_string())]);
    assert!(conf.endpoint("lmstudio").is_some());
    let qwen = conf.model("local-qwen").unwrap();
    assert_eq!((qwen.max_output, qwen.context_window), (Some(4096), Some(32768)));
    assert_eq!(
        qwen.reasoning.keys().copied().collect::<Vec<_>>(),
        [ReasoningLevel::Low, ReasoningLevel::High]
    );
    // an api-config replaces the base one
    assert_eq!(qwen.api_config.as_ref().unwrap(), &serde_json::json!({"top_k": 20}));
    let title = conf.bind("title").unwrap();
    assert_eq!(title.model, "local-qwen");
    assert_eq!(title.reasoning, ReasoningLevel::None);
    assert_eq!(conf.limits.user_daily_cost, Some(dec("5")));
    assert_eq!(conf.limits.user_daily_tokens, 1000);
    assert!(conf.content_log.enabled);
    assert!(conf.a2a.enabled && !conf.a2a.security.bearer && conf.a2a.security.api_key);
}

fn facade() -> EntityFacade {
    testing::open()
}

fn row(facade: &EntityFacade, entity: &str, fields: &[(&str, &str)]) {
    let mut value = facade.make_value(entity).unwrap();
    for (name, text) in fields {
        value.set(*name, Value::string(*text));
    }
    value.create(facade).unwrap();
}

#[test]
fn a_row_overrides_the_conf_entry_with_its_id_field_by_field() {
    let facade = facade();
    let conf = LlmConf::from_conf_text(CONF).unwrap();
    row(
        &facade,
        "moqui.llm.LlmEndpoint",
        &[("endpointId", "local"), ("baseUrl", "http://gpu-box:1234"), ("headersJson", r#"{"X-Lab":"1"}"#)],
    );
    row(
        &facade,
        "moqui.llm.LlmEndpoint",
        &[("endpointId", "db-only"), ("protocol", "responses"), ("baseUrl", "https://api.example.com")],
    );
    row(&facade, "moqui.llm.LlmEndpoint", &[("endpointId", "broken"), ("protocol", "telex")]);
    row(
        &facade,
        "moqui.llm.LlmModel",
        &[("modelId", "local-qwen"), ("priceInput", "0.1"), ("reasoningJson", r#"{"high":{"reasoning_effort":"high"}}"#)],
    );
    row(
        &facade,
        "moqui.llm.LlmModel",
        &[("modelId", "db-model"), ("endpointId", "db-only"), ("providerModel", "gpt-x"), ("tier", "fast")],
    );
    row(&facade, "moqui.llm.LlmModel", &[("modelId", "hosted-sonnet"), ("disabled", "Y")]);
    row(&facade, "moqui.llm.LlmFeatureBinding", &[("featureId", "summarize"), ("modelId", "db-model")]);
    let registry = Registry::load(&conf, &facade).unwrap();

    let local = &registry.endpoints["local"];
    assert_eq!(local.source, Source::Both);
    assert_eq!(local.conf.url, "http://gpu-box:1234");
    // what the row leaves empty stays the conf's
    assert!(local.conf.local && local.conf.protocol == ProtocolId::ChatCompletions);
    assert_eq!(local.conf.headers, [("X-Lab".to_string(), "1".to_string())]);
    assert_eq!(registry.endpoints["db-only"].source, Source::Database);
    assert_eq!(registry.endpoints["db-only"].conf.protocol, ProtocolId::Responses);
    assert!(!registry.endpoints.contains_key("broken"));
    assert!(registry.problems.iter().any(|problem| problem.contains("LlmEndpoint broken")), "{:?}", registry.problems);

    let qwen = &registry.models["local-qwen"].conf;
    assert_eq!(qwen.prices.input, Some(dec("0.1")));
    assert_eq!(qwen.name, "qwen3.8-27b");
    assert_eq!(
        qwen.reasoning.keys().copied().collect::<Vec<_>>(),
        [ReasoningLevel::Low, ReasoningLevel::High]
    );

    // the assist feature: its model, at the bound level
    let assist = registry.resolve_feature("assist").unwrap();
    assert_eq!((assist.model.id.as_str(), assist.endpoint.id.as_str()), ("local-qwen", "local"));
    assert_eq!(assist.reasoning, ReasoningLevel::Medium);
    assert_eq!(assist.feature.as_deref(), Some("assist"));
    assert!(assist.fell_back_from.is_none());
    // a database-only binding to a database-only model
    let summarize = registry.resolve_feature("summarize").unwrap();
    assert_eq!(summarize.model.name, "gpt-x");
    assert_eq!(summarize.endpoint.request_url(), "https://api.example.com/v1/responses");
    // title's model has no name: no fallback, so an error that says why
    let error = registry.resolve_feature("title").unwrap_err().to_string();
    assert!(error.contains("model unnamed has no provider model name"), "{error}");
    assert!(registry.resolve_feature("classify").unwrap_err().to_string().contains("no model is bound"));
    // a tier: fast is db-model (unnamed is not usable), thorough has only the disabled sonnet, so a balanced one
    assert_eq!(registry.resolve_tier(Tier::Fast).unwrap().model.id, "db-model");
    assert_eq!(registry.resolve_tier(Tier::Thorough).unwrap().model.id, "local-qwen");
    // local only
    let want = Want {
        model: Some("db-model"),
        local_only: true,
        ..Want::default()
    };
    assert!(registry.resolve(&want).unwrap_err().to_string().contains("is not local"));
}

#[test]
fn a_row_that_moves_an_endpoint_to_another_host_does_not_get_the_conf_key() {
    let facade = facade();
    let conf = LlmConf::from_conf_text(CONF).unwrap();
    assert!(conf.endpoint("hosted").unwrap().auth().is_some());
    // the same host (a default port written out) keeps the key
    row(&facade, "moqui.llm.LlmEndpoint", &[("endpointId", "hosted"), ("baseUrl", "https://API.anthropic.com:443/")]);
    let registry = Registry::load(&conf, &facade).unwrap();
    assert!(registry.endpoints["hosted"].conf.auth().is_some());
    // another host drops it, and says so
    facade
        .find("moqui.llm.LlmEndpoint")
        .unwrap()
        .eq("endpointId", Value::string("hosted"))
        .update_all([("baseUrl".to_string(), Value::string("https://collector.example.net"))])
        .unwrap();
    let registry = Registry::load(&conf, &facade).unwrap();
    assert!(registry.endpoints["hosted"].conf.auth().is_none());
    assert!(
        registry.problems.iter().any(|problem| problem.contains("conf's key is not sent")),
        "{:?}",
        registry.problems
    );
    // unless the row brings its own
    facade
        .find("moqui.llm.LlmEndpoint")
        .unwrap()
        .eq("endpointId", Value::string("hosted"))
        .update_all([("apiKey".to_string(), Value::string("sk-row-key-0123456789"))])
        .unwrap();
    let registry = Registry::load(&conf, &facade).unwrap();
    assert_eq!(
        registry.endpoints["hosted"].conf.auth(),
        Some(("x-api-key".to_string(), "sk-row-key-0123456789".to_string()))
    );
    // a row cannot name a property for the key: the entity has no such field
    let entities = std::fs::read_to_string(testing::framework_dir().join("entity/LlmEntities.xml")).unwrap();
    assert!(!entities.contains("name=\"apiKeyEnv\""));
    assert_eq!(registry::origin("HTTP://user:pw@Box:80/v1"), "http://box");
}

#[test]
fn a_masked_key_shows_only_its_last_four_characters() {
    assert_eq!(crate::llm::conf::mask_secret("sk-ant-0123456789abcdef"), "…cdef");
    assert_eq!(crate::llm::conf::mask_secret("short-key"), "***");
    assert_eq!(crate::llm::conf::mask_secret(""), "");
}

#[test]
fn a_feature_falls_back_when_its_model_cannot_take_a_call() {
    let facade = facade();
    let conf = LlmConf::from_conf_text(CONF).unwrap();
    row(&facade, "moqui.llm.LlmModel", &[("modelId", "local-qwen"), ("disabled", "Y")]);
    let registry = Registry::load(&conf, &facade).unwrap();
    let assist = registry.resolve_feature("assist").unwrap();
    assert_eq!(assist.model.id, "hosted-sonnet");
    assert_eq!(assist.fell_back_from.as_deref(), Some("local-qwen"));
    assert_eq!(assist.reasoning, ReasoningLevel::Medium);
    row(&facade, "moqui.llm.LlmEndpoint", &[("endpointId", "hosted"), ("disabled", "Y")]);
    let registry = Registry::load(&conf, &facade).unwrap();
    let error = registry.resolve_feature("assist").unwrap_err().to_string();
    assert!(
        error.contains("model local-qwen is disabled") && error.contains("endpoint hosted of model hosted-sonnet is disabled"),
        "{error}"
    );
}

#[test]
fn an_admin_verdict_is_not_replaced_by_a_probe() {
    let facade = facade();
    assert!(registry::store_verdict(&facade, "m1", "tools", Verdict::Yes, VerdictSource::Probe, Some("200 with a tool call")).unwrap());
    assert!(registry::store_verdict(&facade, "m1", "streaming", Verdict::No, VerdictSource::Admin, Some("set by admin")).unwrap());
    assert!(!registry::store_verdict(&facade, "m1", "streaming", Verdict::Yes, VerdictSource::Probe, None).unwrap());
    assert!(registry::store_verdict(&facade, "m1", "tools", Verdict::Partial, VerdictSource::Probe, None).unwrap());
    let verdicts = registry::capabilities(&facade, "m1").unwrap();
    assert_eq!(verdicts["streaming"].verdict, Verdict::No);
    assert_eq!(verdicts["streaming"].source, VerdictSource::Admin);
    assert_eq!(verdicts["tools"].verdict, Verdict::Partial);
    assert!(verdicts["tools"].probe_date.is_some());
}

#[test]
fn cost_counts_cached_input_and_cache_writes_at_their_prices() {
    let prices = Prices {
        input: Some(dec("3")),
        cached_input: Some(dec("0.3")),
        cache_write: Some(dec("3.75")),
        output: Some(dec("15")),
        currency: "USD".to_string(),
    };
    let usage = Usage {
        input: 100_000,
        cached_input: 80_000,
        cache_write: 10_000,
        output: 2_000,
        reasoning: 500,
    };
    // 10k plain at 3, 80k cached at 0.3, 10k written at 3.75, 2k out at 15, per million
    assert_eq!(usage::cost(&prices, &usage), Some(dec("0.1215")));
    // no cache prices: the input price
    let plain = Prices {
        cached_input: None,
        cache_write: None,
        ..prices.clone()
    };
    assert_eq!(usage::cost(&plain, &usage), Some(dec("0.33")));
    assert_eq!(usage::cost(&Prices::default(), &usage), None);
    assert_eq!(usage.total(), 102_000);
}

fn call(user: &str, model: &str, source: CallSource, start: i64, usage: Usage) -> CallRecord {
    CallRecord {
        user_id: Some(user.to_string()),
        model_id: model.to_string(),
        endpoint_id: "hosted".to_string(),
        provider_model: "claude-sonnet-5".to_string(),
        protocol: "anthropic-messages".to_string(),
        source,
        start_millis: start,
        duration_ms: 900,
        attempt: 1,
        usage,
        request_json: Some(r#"{"model":"x","api_key":"abc","messages":[{"content":"my key is sk-proj-0123456789abcdefghij"}],"max_tokens":50}"#.to_string()),
        response_json: Some("Bearer abcdefghijklmnopqrstuvwxyz".to_string()),
        ..CallRecord::default()
    }
}

#[test]
fn calls_roll_up_by_day_user_model_and_source() {
    let facade = facade();
    let conf = LlmConf::from_conf_text(CONF).unwrap();
    let prices = conf.model("hosted-sonnet").unwrap().prices.clone();
    let today = usage::day_of(jiff::Timestamp::now().as_millisecond()).unwrap();
    let (start, _) = usage::day_bounds(today).unwrap();
    let yesterday = today.yesterday().unwrap();
    let (yesterday_start, _) = usage::day_bounds(yesterday).unwrap();
    let tokens = |input: u64, cached: u64, output: u64| Usage {
        input,
        cached_input: cached,
        output,
        ..Usage::default()
    };
    let first = usage::record_call(
        &facade,
        &conf.content_log,
        &prices,
        &call("U1", "hosted-sonnet", CallSource::Agent, start + 1000, tokens(1000, 0, 100)),
    )
    .unwrap();
    usage::record_call(&facade, &conf.content_log, &prices, &call("U1", "hosted-sonnet", CallSource::Agent, start + 2000, tokens(2000, 1000, 200))).unwrap();
    usage::record_call(&facade, &conf.content_log, &prices, &call("U1", "hosted-sonnet", CallSource::Gateway, start + 3000, tokens(10, 0, 1))).unwrap();
    let mut failed = call("U2", "hosted-sonnet", CallSource::Agent, start + 4000, Usage::default());
    failed.error_class = Some("rate_limited".to_string());
    failed.user_id = None;
    usage::record_call(&facade, &conf.content_log, &prices, &failed).unwrap();
    usage::record_call(&facade, &conf.content_log, &prices, &call("U1", "hosted-sonnet", CallSource::Agent, yesterday_start + 5, tokens(7, 0, 3))).unwrap();

    // the content is kept with content-log on, redacted
    let stored = facade
        .find("moqui.llm.LlmCall")
        .unwrap()
        .eq("callId", Value::string(&first))
        .one()
        .unwrap()
        .unwrap();
    let request = to_plain_string(stored.get("requestJson").unwrap());
    assert!(request.contains(r#""api_key":"[redacted]""#) && request.contains("my key is [redacted]"), "{request}");
    assert!(request.contains(r#""max_tokens":50"#), "{request}");
    assert_eq!(to_plain_string(stored.get("responseJson").unwrap()), "Bearer [redacted]");
    // 1000 in at 3 and 100 out at 15 per million
    assert_eq!(super::decimal_of(&stored, "cost"), Some(dec("0.0045")));

    assert_eq!(usage::rollup_day(&facade, today).unwrap(), 3);
    // again: the same rows
    assert_eq!(usage::rollup_day(&facade, today).unwrap(), 3);
    let daily = facade
        .find("moqui.llm.LlmUsageDaily")
        .unwrap()
        .eq("usageDate", Value::Date(today))
        .list()
        .unwrap();
    assert_eq!(daily.len(), 3);
    let of = |user: &str, source: &str| {
        daily
            .iter()
            .find(|row| super::text_of(row, "userId").as_deref() == Some(user) && super::text_of(row, "source").as_deref() == Some(source))
            .unwrap()
            .clone()
    };
    let agent = of("U1", "agent");
    assert_eq!(super::int_of(&agent, "callCount"), Some(2));
    assert_eq!(super::int_of(&agent, "inputTokens"), Some(3000));
    assert_eq!(super::int_of(&agent, "cachedInputTokens"), Some(1000));
    assert_eq!(super::int_of(&agent, "totalTokens"), Some(3300));
    // 0.0045 + (1000 at 3 + 1000 at 0.3 + 200 at 15) / 1M
    assert_eq!(super::decimal_of(&agent, "cost"), Some(dec("0.0108")));
    assert_eq!(super::text_of(&agent, "currencyUomId").as_deref(), Some("USD"));
    let unknown = of(usage::NO_USER, "agent");
    assert_eq!((super::int_of(&unknown, "callCount"), super::int_of(&unknown, "errorCount")), (Some(1), Some(1)));
    assert_eq!(super::int_of(&of("U1", "gateway"), "callCount"), Some(1));
    assert_eq!(usage::rollup_day(&facade, yesterday).unwrap(), 1);

    // the totals the dashboard shows
    let (_, end) = usage::day_bounds(today).unwrap();
    let totals = usage::totals_between(&facade, start, end).unwrap();
    assert_eq!((totals.calls, totals.errors, totals.usage.total()), (4, 1, 3311));
    let earlier = usage::daily_totals_between(&facade, yesterday, today).unwrap();
    assert_eq!((earlier.calls, earlier.usage.total()), (1, 10));

    // the content goes after the retention days; the counts stay
    let now = jiff::Timestamp::now().as_millisecond();
    assert_eq!(usage::clean_call_content(&facade, 0, now + 1).unwrap(), 5);
    let cleared = facade.find("moqui.llm.LlmCall").unwrap().eq("callId", Value::string(&first)).one().unwrap().unwrap();
    assert!(cleared.get("requestJson").is_none_or(Value::is_null));
    assert_eq!(super::int_of(&cleared, "inputTokens"), Some(1000));
}

#[test]
fn no_content_is_kept_with_the_content_log_off() {
    let facade = facade();
    let conf = LlmConf::default();
    let id = usage::record_call(
        &facade,
        &conf.content_log,
        &Prices::default(),
        &call("U1", "m", CallSource::Probe, jiff::Timestamp::now().as_millisecond(), Usage::default()),
    )
    .unwrap();
    let stored = facade.find("moqui.llm.LlmCall").unwrap().eq("callId", Value::string(&id)).one().unwrap().unwrap();
    assert!(stored.get("requestJson").is_none_or(Value::is_null));
    assert!(stored.get("cost").is_none_or(Value::is_null));
    assert_eq!(super::text_of(&stored, "source").as_deref(), Some("probe"));
}

#[test]
fn redaction_takes_secret_keys_and_token_shaped_words() {
    let extra = vec!["taxId".to_string()];
    let text = r#"{"headers":{"Authorization":"Bearer x","X-Api-Key":"k"},"tax_id":"123","usage":{"prompt_tokens":5},
        "note":"call ghp_abcdefghijklmnopqrstuvwxyz0123 or AKIAABCDEFGHIJKLMNOP","jwt":"eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.abcdefghij"}"#;
    let redacted: serde_json::Value = serde_json::from_str(&usage::redact(text, &extra)).unwrap();
    assert_eq!(redacted["headers"]["Authorization"], "[redacted]");
    assert_eq!(redacted["headers"]["X-Api-Key"], "[redacted]");
    assert_eq!(redacted["tax_id"], "[redacted]");
    assert_eq!(redacted["usage"]["prompt_tokens"], 5);
    assert_eq!(redacted["note"], "call [redacted] or [redacted]");
    assert_eq!(redacted["jwt"], "[redacted]");
    assert_eq!(usage::redact("plain sk-short text.", &[]), "plain sk-short text.");
    assert_eq!(
        usage::redact("key sk-0123456789abcdefghijkl.", &[]),
        "key [redacted]."
    );
}

#[test]
fn the_quota_is_the_users_row_else_the_largest_group_row_else_the_conf() {
    let facade = facade();
    let conf = LlmConf::from_conf_text(CONF).unwrap();
    let now = jiff::Timestamp::now().as_millisecond();
    let prices = conf.model("hosted-sonnet").unwrap().prices.clone();
    usage::record_call(&facade, &conf.content_log, &prices, &call("U1", "hosted-sonnet", CallSource::Agent, now - 10, Usage {
        input: 900,
        output: 200,
        ..Usage::default()
    }))
    .unwrap();
    // the conf's 1000 tokens a day: 1100 used
    let status = usage::check_quota(&facade, &conf.limits, "U1", "all", now).unwrap();
    assert!(!status.is_within());
    assert_eq!(status.limit.from, "conf");
    assert!(status.exceeded.unwrap().contains("token quota of 1000 today"));
    // the conf's limit is for all sources together: a gateway call counts the agent's calls too
    let status = usage::check_quota(&facade, &conf.limits, "U1", "gateway", now).unwrap();
    assert!(!status.is_within() && status.limit.all_sources);

    // a group the user is in, and ALL_USERS: the larger applies
    row(&facade, "moqui.security.UserGroupMember", &[("userGroupId", "G1"), ("userId", "U1"), ("fromDate", "2020-01-01 00:00:00")]);
    row(&facade, "moqui.llm.LlmQuota", &[("quotaId", "Q1"), ("userGroupId", "G1"), ("maxTokens", "5000")]);
    row(&facade, "moqui.llm.LlmQuota", &[("quotaId", "Q2"), ("userGroupId", "ALL_USERS"), ("maxTokens", "2000")]);
    let status = usage::check_quota(&facade, &conf.limits, "U1", "all", now).unwrap();
    assert!(status.is_within(), "{status:?}");
    assert_eq!(status.limit.max_tokens, Some(5000));
    assert_eq!(status.used.usage.total(), 1100);

    // the user's own row wins, here a cost limit: 900 at 3 and 200 at 15 per million is 0.0057
    row(&facade, "moqui.llm.LlmQuota", &[("quotaId", "Q3"), ("userId", "U1"), ("maxCost", "0.005")]);
    let status = usage::check_quota(&facade, &conf.limits, "U1", "all", now).unwrap();
    assert_eq!(status.limit.from, "Q3");
    assert_eq!(status.limit.max_tokens, None);
    assert!(status.exceeded.unwrap().contains("cost quota of 0.005"));
    // a row for the gateway counts gateway calls only; the all row still counts every source for other scopes
    row(&facade, "moqui.llm.LlmQuota", &[("quotaId", "Q4"), ("userId", "U1"), ("scope", "gateway"), ("maxTokens", "100")]);
    let status = usage::check_quota(&facade, &conf.limits, "U1", "gateway", now).unwrap();
    assert!(status.is_within() && status.limit.from == "Q4" && !status.limit.all_sources, "{status:?}");
    assert_eq!(status.used.usage.total(), 0);
    assert!(!usage::check_quota(&facade, &conf.limits, "U1", "agent", now).unwrap().is_within());
}

#[test]
fn a_day_limit_and_a_month_limit_are_checked_each_on_its_own() {
    let facade = facade();
    let conf = LlmConf::from_conf_text(CONF).unwrap();
    let now = jiff::Timestamp::now().as_millisecond();
    let prices = conf.model("hosted-sonnet").unwrap().prices.clone();
    usage::record_call(&facade, &conf.content_log, &prices, &call("U2", "hosted-sonnet", CallSource::Api, now - 10, Usage {
        input: 400,
        output: 200,
        ..Usage::default()
    }))
    .unwrap();
    row(&facade, "moqui.security.UserGroupMember", &[("userGroupId", "G2"), ("userId", "U2"), ("fromDate", "2020-01-01 00:00:00")]);
    // a generous month and a tight day: not one limit of 5000 for both
    row(&facade, "moqui.llm.LlmQuota", &[("quotaId", "QM"), ("userGroupId", "G2"), ("period", "month"), ("maxTokens", "5000")]);
    row(&facade, "moqui.llm.LlmQuota", &[("quotaId", "QD"), ("userGroupId", "ALL_USERS"), ("period", "day"), ("maxTokens", "500")]);
    let limits = usage::quota_limits(&facade, &conf.limits, "U2", "api").unwrap();
    let mut periods: Vec<(String, Option<u64>)> = limits.iter().map(|limit| (limit.period.clone(), limit.max_tokens)).collect();
    periods.sort();
    assert_eq!(periods, [("day".to_string(), Some(500)), ("month".to_string(), Some(5000))]);
    let status = usage::check_quota(&facade, &conf.limits, "U2", "api", now).unwrap();
    assert_eq!(status.limit.from, "QD");
    assert!(status.exceeded.unwrap().contains("token quota of 500 today"));
}

/// The run states and their transitions in `run/state.rs` are the seed rows of `data/AgentTypeData.xml`.
#[test]
fn the_run_state_table_is_the_seed_status_flow() {
    let data = MNode::parse_file(testing::framework_dir().join("data/AgentTypeData.xml")).unwrap();
    let items: Vec<(String, String)> = data
        .children_named("moqui.basic.StatusItem")
        .iter()
        .filter(|item| item.attribute("statusTypeId").as_deref() == Some("AgentRun"))
        .map(|item| (item.attribute("statusId").unwrap(), item.attribute("statusCode").unwrap()))
        .collect();
    let states: Vec<(String, String)> = RunState::ALL
        .iter()
        .map(|state| (state.status_id().to_string(), state.wire().to_string()))
        .collect();
    assert_eq!(items, states);
    let run_ids: BTreeSet<String> = items.into_iter().map(|(id, _)| id).collect();
    let seeded: BTreeSet<(String, String)> = data
        .children_named("moqui.basic.StatusFlowTransition")
        .iter()
        .filter(|row| row.attribute("statusId").is_some_and(|id| run_ids.contains(&id)))
        .map(|row| (row.attribute("statusId").unwrap(), row.attribute("toStatusId").unwrap()))
        .collect();
    let table: BTreeSet<(String, String)> = TRANSITIONS
        .iter()
        .map(|(from, to)| (from.status_id().to_string(), to.status_id().to_string()))
        .collect();
    assert_eq!(seeded, table);
    for state in RunState::ALL {
        assert_eq!(state.is_terminal(), !TRANSITIONS.iter().any(|(from, _)| *from == state), "{state:?}");
        assert_eq!(RunState::from_wire(state.wire()), Some(state));
    }
    assert!(RunState::Working.move_to(RunState::Submitted).is_err());
    assert_eq!(RunState::InputRequired.move_to(RunState::Working).unwrap(), RunState::Working);
}

/// The approval and hand-off status tables are the seed's `StatusFlowTransition` rows.
#[test]
fn the_approval_and_handoff_tables_match_the_seed() {
    use crate::llm::approval::handoff::{self, HandoffStatus};
    use crate::llm::approval::{self, ApprovalStatus};
    let data = MNode::parse_file(testing::framework_dir().join("data/AgentTypeData.xml")).unwrap();
    let flows = |type_id: &str| -> BTreeSet<(String, String)> {
        let ids: BTreeSet<String> = data
            .children_named("moqui.basic.StatusItem")
            .iter()
            .filter(|item| item.attribute("statusTypeId").as_deref() == Some(type_id))
            .filter_map(|item| item.attribute("statusId"))
            .collect();
        data.children_named("moqui.basic.StatusFlowTransition")
            .iter()
            .filter(|row| row.attribute("statusId").is_some_and(|id| ids.contains(&id)))
            .map(|row| (row.attribute("statusId").unwrap(), row.attribute("toStatusId").unwrap()))
            .collect()
    };
    let approvals: BTreeSet<(String, String)> = approval::TRANSITIONS
        .iter()
        .map(|(from, to)| (from.status_id().to_string(), to.status_id().to_string()))
        .collect();
    assert_eq!(flows("AgentApproval"), approvals);
    let handoffs: BTreeSet<(String, String)> = handoff::TRANSITIONS
        .iter()
        .map(|(from, to)| (from.status_id().to_string(), to.status_id().to_string()))
        .collect();
    assert_eq!(flows("AgentHandoff"), handoffs);
    for status in ApprovalStatus::ALL {
        assert_eq!(ApprovalStatus::from_status_id(status.status_id()), Some(status));
    }
    for status in HandoffStatus::ALL {
        assert_eq!(HandoffStatus::from_status_id(status.status_id()), Some(status));
    }
}

#[test]
fn every_status_flow_transition_of_the_agent_seed_names_known_statuses() {
    let data = MNode::parse_file(testing::framework_dir().join("data/AgentTypeData.xml")).unwrap();
    let statuses: BTreeSet<String> = data
        .children_named("moqui.basic.StatusItem")
        .iter()
        .filter_map(|item| item.attribute("statusId"))
        .collect();
    for transition in data.children_named("moqui.basic.StatusFlowTransition") {
        for field in ["statusId", "toStatusId"] {
            let id = transition.attribute(field).unwrap();
            assert!(statuses.contains(&id), "{id}");
        }
    }
}

#[test]
fn the_agent_seed_loads() {
    let facade = facade();
    let text = std::fs::read_to_string(testing::framework_dir().join("data/AgentTypeData.xml")).unwrap();
    let loaded = crate::entity::load_feed::load_entity_xml_text(
        &facade,
        "AgentTypeData.xml",
        &text,
        &crate::entity::load_feed::LoadOptions::default(),
    )
    .unwrap();
    assert!(loaded.records > 100, "{}", loaded.records);
    let surface = facade
        .find("moqui.basic.Enumeration")
        .unwrap()
        .eq("enumId", Value::string("AgsfDrawer"))
        .one()
        .unwrap();
    assert!(surface.is_some());
    let assist = facade.find("moqui.agent.AgentDefinition").unwrap().eq("agentId", Value::string("assist")).one().unwrap();
    assert!(assist.is_some_and(|agent| super::text_of(&agent, "isSystem").as_deref() == Some("Y")));
}
