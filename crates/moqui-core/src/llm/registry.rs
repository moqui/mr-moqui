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

//! The model registry (plan 4.2): the endpoints, models and feature bindings of the `llm-facade` conf merged with the
//! `LlmEndpoint`, `LlmModel` and `LlmFeatureBinding` rows, and the resolution of a feature, a tier or a model id to a
//! [`Target`] (an endpoint and a model) a provider call goes to.
//!
//! A row overrides the conf entry with the same id field by field: each field the row sets replaces the conf's, and an
//! empty one keeps it, so a row may only add prices to a conf model. A row with no conf entry is an entry of its own. A
//! row that does not read (a bad protocol, a reasoning patch that is not a JSON object) is left out or keeps the conf's
//! value, and is named in [`Registry::problems`]; the others still apply.
//!
//! The capability verdicts of `probe#Model` are `LlmModelCapability` rows: [`capabilities`], [`store_verdict`].

use std::collections::BTreeMap;

use moqui_lib::error::Error;
use moqui_lib::object::Value;

use super::conf::{
    json_object, parse_screen_size, BindConf, EndpointConf, LlmConf, ModelConf, PromptVariant, ProtocolId,
    ReasoningLevel, ReasoningReplay, Secret, Tier, ToolMode,
};
use super::{decimal_of, flag_of, int_of, text_of};
use crate::entity::{EntityFacade, EntityValue};

const LLM_ENDPOINT: &str = "moqui.llm.LlmEndpoint";
const LLM_MODEL: &str = "moqui.llm.LlmModel";
const LLM_FEATURE_BINDING: &str = "moqui.llm.LlmFeatureBinding";
const LLM_MODEL_CAPABILITY: &str = "moqui.llm.LlmModelCapability";

/// Where an entry of the registry came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Conf,
    Database,
    /// A conf entry a row overrides.
    Both,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Conf => "conf",
            Source::Database => "database",
            Source::Both => "conf and database",
        }
    }

    fn with_row(self) -> Self {
        match self {
            Source::Conf | Source::Both => Source::Both,
            Source::Database => Source::Database,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Endpoint {
    pub conf: EndpointConf,
    pub source: Source,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub conf: ModelConf,
    pub source: Source,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    pub conf: BindConf,
    pub source: Source,
}

/// Where a provider call goes: the endpoint, the model on it and the reasoning level, and for a feature, the feature and
/// the model it fell back from.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub endpoint: EndpointConf,
    pub model: ModelConf,
    pub reasoning: ReasoningLevel,
    pub feature: Option<String>,
    /// The bound model, when it could not be used and this is its fallback.
    pub fell_back_from: Option<String>,
}

/// What a caller asks the registry for: a model id first, then a feature's binding, then a tier.
#[derive(Clone, Debug, Default)]
pub struct Want<'a> {
    pub model: Option<&'a str>,
    pub feature: Option<&'a str>,
    pub tier: Option<Tier>,
    /// Replaces the binding's level.
    pub reasoning: Option<ReasoningLevel>,
    /// Only endpoints marked local (an agent that handles sensitive data).
    pub local_only: bool,
}

/// The endpoints, models and bindings in effect.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    pub endpoints: BTreeMap<String, Endpoint>,
    pub models: BTreeMap<String, Model>,
    pub bindings: BTreeMap<String, Binding>,
    /// The rows that did not read, each named with why.
    pub problems: Vec<String>,
}

impl Registry {
    /// The conf entries alone.
    pub fn from_conf(conf: &LlmConf) -> Self {
        let mut registry = Self::default();
        for endpoint in &conf.endpoints {
            registry.endpoints.insert(
                endpoint.id.clone(),
                Endpoint {
                    conf: endpoint.clone(),
                    source: Source::Conf,
                },
            );
        }
        for model in &conf.models {
            registry.models.insert(
                model.id.clone(),
                Model {
                    conf: model.clone(),
                    source: Source::Conf,
                },
            );
        }
        for bind in &conf.binds {
            registry.bindings.insert(
                bind.feature.clone(),
                Binding {
                    conf: bind.clone(),
                    source: Source::Conf,
                },
            );
        }
        registry
    }

    /// The conf entries with the rows over them, read through the entity cache.
    ///
    /// # Errors
    ///
    /// Returns an error when one of the three entities cannot be read at all.
    pub fn load(conf: &LlmConf, facade: &EntityFacade) -> Result<Self, Error> {
        let mut registry = Self::from_conf(conf);
        let rows = |entity: &str| -> Result<Vec<EntityValue>, Error> {
            Ok(facade
                .find(entity)?
                .use_cache(true)
                .disable_authz()
                .list()?
                .values()
                .to_vec())
        };
        registry.apply_endpoint_rows(&rows(LLM_ENDPOINT)?);
        registry.apply_model_rows(&rows(LLM_MODEL)?);
        registry.apply_binding_rows(&rows(LLM_FEATURE_BINDING)?);
        Ok(registry)
    }

    pub fn apply_endpoint_rows(&mut self, rows: &[EntityValue]) {
        for row in rows {
            let Some(id) = text_of(row, "endpointId") else {
                continue;
            };
            let protocol = match text_of(row, "protocol") {
                None => None,
                Some(text) => match ProtocolId::parse(&text) {
                    Some(protocol) => Some(protocol),
                    None => {
                        self.problems.push(format!(
                            "LlmEndpoint {id} protocol {text} is not chat-completions, responses, anthropic-messages or fake; the row is left out"
                        ));
                        continue;
                    }
                },
            };
            let (mut endpoint, source) = match self.endpoints.remove(&id) {
                Some(known) => (known.conf, known.source.with_row()),
                None => (
                    EndpointConf::new(&id, protocol.unwrap_or(ProtocolId::ChatCompletions), ""),
                    Source::Database,
                ),
            };
            if let Some(protocol) = protocol {
                endpoint.protocol = protocol;
            }
            let row_url = text_of(row, "baseUrl");
            let row_key = text_of(row, "apiKey");
            // the conf's key goes only to the conf's host: a row that moves the endpoint elsewhere brings its own
            if let Some(url) = &row_url {
                let keyed = endpoint.api_key.is_some() || endpoint.api_key_env.is_some();
                if keyed && row_key.is_none() && origin(url) != origin(&endpoint.url) {
                    endpoint.api_key = None;
                    endpoint.api_key_env = None;
                    self.problems.push(format!(
                        "LlmEndpoint {id} moves the endpoint from {} to {}, so the conf's key is not sent; set a key on the row",
                        origin(&endpoint.url),
                        origin(url)
                    ));
                }
            }
            set_text(&mut endpoint.url, row_url);
            set_some(&mut endpoint.path, text_of(row, "path"));
            if let Some(key) = row_key {
                endpoint.api_key = Some(Secret(key));
                // the row's key wins over a conf property
                endpoint.api_key_env = None;
            }
            set_some(&mut endpoint.auth_header, text_of(row, "authHeader"));
            set_some(&mut endpoint.auth_scheme, text_of(row, "authScheme"));
            if let Some(text) = text_of(row, "headersJson") {
                match json_object(&text) {
                    Ok(serde_json::Value::Object(map)) => {
                        for (name, value) in map {
                            let value = value.as_str().map_or_else(|| value.to_string(), str::to_string);
                            endpoint.headers.retain(|(known, _)| !known.eq_ignore_ascii_case(&name));
                            endpoint.headers.push((name, value));
                        }
                    }
                    Ok(_) => {}
                    Err(problem) => self.problems.push(format!("LlmEndpoint {id} headersJson: {problem}; the conf headers are used")),
                }
            }
            if let Some(seconds) = int_of(row, "connectTimeout").and_then(|n| u64::try_from(n).ok()) {
                endpoint.connect_timeout = seconds;
            }
            if let Some(seconds) = int_of(row, "idleTimeout").and_then(|n| u64::try_from(n).ok()) {
                endpoint.idle_timeout = seconds;
            }
            set_some(&mut endpoint.max_concurrent, positive(row, "maxConcurrent"));
            set_flag(&mut endpoint.allow_private_network, flag_of(row, "allowPrivateNetwork"));
            set_flag(&mut endpoint.local, flag_of(row, "isLocal"));
            set_flag(&mut endpoint.disabled, flag_of(row, "disabled"));
            self.endpoints.insert(id, Endpoint { conf: endpoint, source });
        }
    }

    pub fn apply_model_rows(&mut self, rows: &[EntityValue]) {
        for row in rows {
            let Some(id) = text_of(row, "modelId") else {
                continue;
            };
            let (mut model, source) = match self.models.remove(&id) {
                Some(known) => (known.conf, known.source.with_row()),
                None => match text_of(row, "endpointId") {
                    Some(endpoint) => (ModelConf::new(&id, endpoint, ""), Source::Database),
                    None => {
                        self.problems.push(format!("LlmModel {id} has no endpointId and no conf model; the row is left out"));
                        continue;
                    }
                },
            };
            set_text(&mut model.endpoint, text_of(row, "endpointId"));
            set_text(&mut model.name, text_of(row, "providerModel"));
            if let Some(text) = text_of(row, "tier") {
                match Tier::parse(&text) {
                    Some(tier) => model.tier = tier,
                    None => self.problems.push(format!("LlmModel {id} tier {text} is not fast, balanced or thorough; it is left out")),
                }
            }
            set_some(&mut model.context_window, positive(row, "contextWindow"));
            set_some(&mut model.max_output, positive(row, "maxOutput"));
            set_some(&mut model.prices.input, decimal_of(row, "priceInput"));
            set_some(&mut model.prices.cached_input, decimal_of(row, "priceCachedInput"));
            set_some(&mut model.prices.cache_write, decimal_of(row, "priceCacheWrite"));
            set_some(&mut model.prices.output, decimal_of(row, "priceOutput"));
            set_text(&mut model.prices.currency, text_of(row, "currencyUomId"));
            if let Some(text) = text_of(row, "reasoningJson") {
                match json_object(&text) {
                    Ok(serde_json::Value::Object(levels)) => {
                        for (level_text, patch) in levels {
                            match (ReasoningLevel::parse(&level_text), patch) {
                                (Some(level), patch @ serde_json::Value::Object(_)) => {
                                    model.reasoning.insert(level, patch);
                                }
                                _ => self.problems.push(format!(
                                    "LlmModel {id} reasoningJson {level_text} is not a level with a JSON object; it is left out"
                                )),
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(problem) => self.problems.push(format!("LlmModel {id} reasoningJson: {problem}; it is left out")),
                }
            }
            if let Some(text) = text_of(row, "apiConfigJson") {
                match json_object(&text) {
                    Ok(patch) => model.api_config = Some(patch),
                    Err(problem) => self.problems.push(format!("LlmModel {id} apiConfigJson: {problem}; it is left out")),
                }
            }
            let behavior = &mut model.behavior;
            match text_of(row, "toolMode").as_deref() {
                None => {}
                Some("native") => behavior.tool_mode = ToolMode::Native,
                Some("json") => behavior.tool_mode = ToolMode::Json,
                Some(other) => self.problems.push(format!("LlmModel {id} toolMode {other} is not native or json")),
            }
            set_some(&mut behavior.parallel_tool_calls, flag_of(row, "parallelToolCalls"));
            match text_of(row, "promptVariant").as_deref() {
                None => {}
                Some("standard") => behavior.prompt_variant = PromptVariant::Standard,
                Some("small") => behavior.prompt_variant = PromptVariant::Small,
                Some(other) => self.problems.push(format!("LlmModel {id} promptVariant {other} is not standard or small")),
            }
            set_some(&mut behavior.result_cap_chars, positive(row, "resultCapChars"));
            set_some(&mut behavior.screen_size, text_of(row, "screenSize").and_then(|text| parse_screen_size(&text)));
            set_some(&mut behavior.list_limit, positive(row, "listLimit"));
            set_some(&mut behavior.catalog_size, positive(row, "catalogSize"));
            match text_of(row, "reasoningReplay").as_deref() {
                None => {}
                Some("none") => behavior.reasoning_replay = ReasoningReplay::None,
                Some("last") => behavior.reasoning_replay = ReasoningReplay::Last,
                Some("all") => behavior.reasoning_replay = ReasoningReplay::All,
                Some(other) => self.problems.push(format!("LlmModel {id} reasoningReplay {other} is not none, last or all")),
            }
            set_some(
                &mut behavior.temperature,
                decimal_of(row, "temperature").and_then(|value| value.to_string().parse().ok()),
            );
            set_some(&mut behavior.max_tool_calls, positive(row, "maxToolCalls"));
            set_some(&mut behavior.compact_high_percent, positive(row, "compactHighPercent"));
            set_some(&mut behavior.compact_low_percent, positive(row, "compactLowPercent"));
            set_some(&mut behavior.max_concurrency, positive(row, "maxConcurrency"));
            match text_of(row, "intentPrefilter").as_deref() {
                None => {}
                Some("none" | "off") => behavior.intent_hint = false,
                Some("hint") => behavior.intent_hint = true,
                Some(other) => self.problems.push(format!("LlmModel {id} intentPrefilter {other} is not none or hint")),
            }
            if let Some(on) = flag_of(row, "sideCalls") {
                behavior.no_side_calls = !on;
            }
            set_some(
                &mut model.chars_per_token,
                decimal_of(row, "charsPerToken").and_then(|value| value.to_string().parse().ok()),
            );
            set_flag(&mut model.disabled, flag_of(row, "disabled"));
            self.models.insert(id, Model { conf: model, source });
        }
    }

    pub fn apply_binding_rows(&mut self, rows: &[EntityValue]) {
        for row in rows {
            let Some(feature) = text_of(row, "featureId") else {
                continue;
            };
            let (mut bind, source) = match self.bindings.remove(&feature) {
                Some(known) => (known.conf, known.source.with_row()),
                None => match text_of(row, "modelId") {
                    Some(model) => (
                        BindConf {
                            feature: feature.clone(),
                            model,
                            fallback: None,
                            reasoning: ReasoningLevel::Low,
                        },
                        Source::Database,
                    ),
                    None => {
                        self.problems.push(format!("LlmFeatureBinding {feature} has no modelId and no conf bind; the row is left out"));
                        continue;
                    }
                },
            };
            set_text(&mut bind.model, text_of(row, "modelId"));
            set_some(&mut bind.fallback, text_of(row, "fallbackModelId"));
            if let Some(text) = text_of(row, "reasoningLevel") {
                match ReasoningLevel::parse(&text) {
                    Some(level) => bind.reasoning = level,
                    None => self.problems.push(format!(
                        "LlmFeatureBinding {feature} reasoningLevel {text} is not none, low, medium, high or max; it is left out"
                    )),
                }
            }
            self.bindings.insert(feature, Binding { conf: bind, source });
        }
    }

    /// The model and its endpoint, or why they cannot take a call: an unknown or disabled model, a model with no
    /// provider name, an unknown or disabled endpoint, an endpoint with no URL, or one that is not local when
    /// `local_only`.
    /// The first usable model of exactly `tier` (by id), for a client that chose a tier: none when no model has it.
    pub fn tier_model(&self, tier: Tier, local_only: bool) -> Option<&str> {
        self.models
            .values()
            .find(|model| model.conf.tier == tier && self.usable(&model.conf.id, local_only).is_ok())
            .map(|model| model.conf.id.as_str())
    }

    /// The tiers some usable model has, in order: what a client may choose from.
    pub fn usable_tiers(&self, local_only: bool) -> Vec<Tier> {
        Tier::ALL
            .into_iter()
            .filter(|tier| self.tier_model(*tier, local_only).is_some())
            .collect()
    }

    pub fn usable(&self, model_id: &str, local_only: bool) -> Result<(&Model, &Endpoint), String> {
        let model = self
            .models
            .get(model_id)
            .ok_or_else(|| format!("model {model_id} is not configured"))?;
        if model.conf.disabled {
            return Err(format!("model {model_id} is disabled"));
        }
        if model.conf.name.is_empty() {
            return Err(format!("model {model_id} has no provider model name"));
        }
        let endpoint = self.endpoints.get(&model.conf.endpoint).ok_or_else(|| {
            format!("model {model_id} names endpoint {}, which is not configured", model.conf.endpoint)
        })?;
        if endpoint.conf.disabled {
            return Err(format!("endpoint {} of model {model_id} is disabled", endpoint.conf.id));
        }
        if endpoint.conf.url.is_empty() && endpoint.conf.protocol != ProtocolId::Fake {
            return Err(format!("endpoint {} of model {model_id} has no url", endpoint.conf.id));
        }
        if local_only && !endpoint.conf.local {
            return Err(format!("endpoint {} of model {model_id} is not local", endpoint.conf.id));
        }
        Ok((model, endpoint))
    }

    fn target(&self, model_id: &str, reasoning: ReasoningLevel, local_only: bool) -> Result<Target, String> {
        let (model, endpoint) = self.usable(model_id, local_only)?;
        Ok(Target {
            endpoint: endpoint.conf.clone(),
            model: model.conf.clone(),
            reasoning,
            feature: None,
            fell_back_from: None,
        })
    }

    /// A model by id.
    ///
    /// # Errors
    ///
    /// Returns an error saying why the model cannot take a call.
    pub fn resolve_model(&self, model_id: &str, reasoning: ReasoningLevel) -> Result<Target, Error> {
        self.target(model_id, reasoning, false).map_err(Error::new)
    }

    /// The model bound to `feature`, else its fallback.
    ///
    /// # Errors
    ///
    /// Returns an error when the feature has no binding, or when neither model can take a call (naming both reasons).
    pub fn resolve_feature(&self, feature: &str) -> Result<Target, Error> {
        self.resolve(&Want {
            feature: Some(feature),
            ..Want::default()
        })
    }

    /// The first usable model of `tier` by id; with none, a balanced one, then any.
    ///
    /// # Errors
    ///
    /// Returns an error when no model is usable.
    pub fn resolve_tier(&self, tier: Tier) -> Result<Target, Error> {
        self.resolve(&Want {
            tier: Some(tier),
            ..Want::default()
        })
    }

    /// What `want` asks for: its model, else its feature's binding (with the fallback), else its tier.
    ///
    /// # Errors
    ///
    /// Returns an error saying why nothing that was asked for can take a call.
    pub fn resolve(&self, want: &Want<'_>) -> Result<Target, Error> {
        if let Some(model_id) = want.model {
            return self
                .target(model_id, want.reasoning.unwrap_or_default(), want.local_only)
                .map_err(Error::new);
        }
        if let Some(feature) = want.feature {
            let Some(binding) = self.bindings.get(feature) else {
                return match want.tier {
                    Some(_) => self.resolve(&Want { feature: None, ..want.clone() }),
                    None => Err(Error::new(format!("no model is bound to feature {feature}"))),
                };
            };
            let reasoning = want.reasoning.unwrap_or(binding.conf.reasoning);
            let first = match self.target(&binding.conf.model, reasoning, want.local_only) {
                Ok(mut target) => {
                    target.feature = Some(feature.to_string());
                    return Ok(target);
                }
                Err(reason) => reason,
            };
            if let Some(fallback) = &binding.conf.fallback {
                return match self.target(fallback, reasoning, want.local_only) {
                    Ok(mut target) => {
                        target.feature = Some(feature.to_string());
                        target.fell_back_from = Some(binding.conf.model.clone());
                        Ok(target)
                    }
                    Err(second) => Err(Error::new(format!(
                        "feature {feature} has no usable model: {first}; fallback {second}"
                    ))),
                };
            }
            return Err(Error::new(format!("feature {feature} has no usable model: {first}")));
        }
        let tier = want.tier.unwrap_or_default();
        let reasoning = want.reasoning.unwrap_or_default();
        let usable = |wanted: Option<Tier>| {
            self.models.values().find(|model| {
                wanted.is_none_or(|tier| model.conf.tier == tier) && self.usable(&model.conf.id, want.local_only).is_ok()
            })
        };
        let model = usable(Some(tier))
            .or_else(|| usable(Some(Tier::Balanced)))
            .or_else(|| usable(None))
            .ok_or_else(|| Error::new(format!("no usable model for tier {}", tier.as_str())))?;
        self.target(&model.conf.id, reasoning, want.local_only).map_err(Error::new)
    }
}

/// `scheme://host:port` of a URL, lower case, with no user, path or default port: where a key goes.
pub fn origin(url: &str) -> String {
    let url = url.trim();
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let scheme = scheme.to_ascii_lowercase();
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host).to_ascii_lowercase();
    let host = match (scheme.as_str(), host.as_str()) {
        ("https", host) => host.strip_suffix(":443").unwrap_or(host).to_string(),
        ("http", host) => host.strip_suffix(":80").unwrap_or(host).to_string(),
        _ => host,
    };
    format!("{scheme}://{host}")
}

fn set_text(slot: &mut String, value: Option<String>) {
    if let Some(value) = value {
        *slot = value;
    }
}

fn set_some<T>(slot: &mut Option<T>, value: Option<T>) {
    if value.is_some() {
        *slot = value;
    }
}

fn set_flag(slot: &mut bool, value: Option<bool>) {
    if let Some(value) = value {
        *slot = value;
    }
}

fn positive(row: &EntityValue, field: &str) -> Option<u32> {
    int_of(row, field).and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0)
}

/// The capabilities `probe#Model` checks.
pub const CAPABILITIES: [&str; 6] = [
    "tools",
    "parallel-tools",
    "json-schema",
    "streaming",
    "reasoning-replay",
    "prompt-caching",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Yes,
    No,
    Partial,
}

impl Verdict {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "yes" => Some(Verdict::Yes),
            "no" => Some(Verdict::No),
            "partial" => Some(Verdict::Partial),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Yes => "yes",
            Verdict::No => "no",
            Verdict::Partial => "partial",
        }
    }
}

/// Who gave a verdict. An admin's is never replaced by a probe ("a 200 alone proves nothing").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerdictSource {
    Probe,
    Registry,
    Admin,
}

impl VerdictSource {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "probe" => Some(VerdictSource::Probe),
            "registry" => Some(VerdictSource::Registry),
            "admin" => Some(VerdictSource::Admin),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            VerdictSource::Probe => "probe",
            VerdictSource::Registry => "registry",
            VerdictSource::Admin => "admin",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CapabilityVerdict {
    pub capability: String,
    pub verdict: Verdict,
    pub source: VerdictSource,
    /// Milliseconds since the epoch.
    pub probe_date: Option<i64>,
    pub evidence: Option<String>,
}

/// The stored verdicts of a model, by capability.
///
/// # Errors
///
/// Returns an error when the rows cannot be read.
pub fn capabilities(facade: &EntityFacade, model_id: &str) -> Result<BTreeMap<String, CapabilityVerdict>, Error> {
    let mut out = BTreeMap::new();
    for row in facade
        .find(LLM_MODEL_CAPABILITY)?
        .eq("modelId", Value::string(model_id))
        .use_cache(true)
        .disable_authz()
        .list()?
        .iter()
    {
        let (Some(capability), Some(verdict)) = (
            text_of(row, "capability"),
            text_of(row, "verdict").as_deref().and_then(Verdict::parse),
        ) else {
            continue;
        };
        out.insert(
            capability.clone(),
            CapabilityVerdict {
                capability,
                verdict,
                source: text_of(row, "source")
                    .as_deref()
                    .and_then(VerdictSource::parse)
                    .unwrap_or(VerdictSource::Registry),
                probe_date: row.get("probeDate").and_then(super::millis_of),
                evidence: text_of(row, "evidence"),
            },
        );
    }
    Ok(out)
}

/// Store a verdict. A probe's or the registry's does not replace an admin's; then nothing is written and the result is
/// false.
///
/// # Errors
///
/// Returns an error when the row cannot be read or written.
pub fn store_verdict(
    facade: &EntityFacade,
    model_id: &str,
    capability: &str,
    verdict: Verdict,
    source: VerdictSource,
    evidence: Option<&str>,
) -> Result<bool, Error> {
    let existing = facade
        .find(LLM_MODEL_CAPABILITY)?
        .eq("modelId", Value::string(model_id))
        .eq("capability", Value::string(capability))
        .use_cache(false)
        .disable_authz()
        .one()?;
    if source != VerdictSource::Admin
        && existing
            .as_ref()
            .and_then(|row| text_of(row, "source"))
            .is_some_and(|known| known == VerdictSource::Admin.as_str())
    {
        return Ok(false);
    }
    let mut row = match existing {
        Some(row) => row,
        None => {
            let mut row = facade.make_value(LLM_MODEL_CAPABILITY)?;
            row.set("modelId", Value::string(model_id));
            row.set("capability", Value::string(capability));
            row
        }
    };
    row.set("verdict", Value::string(verdict.as_str()));
    row.set("source", Value::string(source.as_str()));
    row.set("probeDate", crate::user::now_value());
    row.set(
        "evidence",
        evidence.map_or(Value::Null, Value::string),
    );
    row.create_or_update(facade)?;
    Ok(true)
}
