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

//! The model a run of the harness talks to. A live profile is a model of the registry (the `llm-facade` conf merged
//! with the `LlmModel` rows) whose endpoint must answer (a TCP connect to its host and port), or the run is skipped
//! ([`live`]); the agent's feature is bound to it with an `LlmFeatureBinding` row. A replay binds the feature to a model
//! of the scripted `fake` protocol on an endpoint of its own, with the recorded model's settings (prompt variant,
//! result cap, list limit, screen size), so the tools answer as they did ([`Rows::replay_model`]). `--seed` goes to a
//! Chat Completions model as `seed`, through its row's `apiConfigJson` beside the conf's own patch. Every row written is
//! put back as it was when the run ends ([`Rows::restore`]).

use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use moqui_lib::error::Error;
use moqui_lib::object::Value;
use serde_json::{json, Map, Value as Json};

use crate::context::ExecutionContextFactory;
use crate::entity::{EntityFacade, EntityValue};
use crate::llm::conf::{EndpointConf, ModelConf, PromptVariant, ProtocolId, ReasoningLevel, ReasoningReplay, ToolMode};
use crate::llm::run::{in_tx, quietly};

/// The endpoint and model a replay's fake answers on.
pub const REPLAY_MODEL: &str = "agent-eval-replay";

const ENDPOINT: &str = "moqui.llm.LlmEndpoint";
const MODEL: &str = "moqui.llm.LlmModel";
const BINDING: &str = "moqui.llm.LlmFeatureBinding";

/// The `LlmModel` fields a recording's model settings may set on the replay model.
const SETTINGS: [&str; 14] = [
    "tier",
    "contextWindow",
    "maxOutput",
    "toolMode",
    "parallelToolCalls",
    "promptVariant",
    "resultCapChars",
    "screenSize",
    "listLimit",
    "catalogSize",
    "maxToolCalls",
    "reasoningReplay",
    "intentPrefilter",
    "compactHighPercent",
];

/// How long the endpoint's host has to take a connection.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// A live profile's model.
#[derive(Clone, Debug)]
pub struct Target {
    pub model_id: String,
    /// The model's settings as `LlmModel` fields, for the transcript's header and a later replay.
    pub settings: Json,
    /// The conf's `api-config` patch, when the protocol takes a seed (Chat Completions).
    pub seed_base: Option<Json>,
}

/// What [`live`] found.
pub enum Live {
    Ready(Target),
    /// The endpoint does not answer; the reason.
    Skip(String),
}

/// The registry's model `profile`, after checking its endpoint answers.
///
/// # Errors
///
/// Returns an error when the facade is off or there is no such model.
pub fn live(factory: &ExecutionContextFactory, profile: &str) -> Result<Live, Error> {
    let facade = facade(factory)?;
    let llm = crate::llm::facade(factory);
    if !llm.enabled() {
        return Err(Error::new("the llm-facade is disabled, so there is no model to evaluate"));
    }
    let registry = quietly(|| llm.registry(&facade));
    let Some(model) = registry.models.get(profile) else {
        let known: Vec<&str> = registry.models.keys().map(String::as_str).collect();
        return Err(Error::new(format!(
            "there is no model {profile} in the llm-facade conf or the LlmModel rows (models: {}); the commented \
             local-qwen example in MoquiDefaultConf.xml is one to copy",
            known.join(", ")
        )));
    };
    let model = model.conf.clone();
    let endpoint = registry
        .endpoints
        .get(&model.endpoint)
        .map(|endpoint| endpoint.conf.clone())
        .ok_or_else(|| Error::new(format!("model {profile} names endpoint {}, which is not there", model.endpoint)))?;
    if let Err(reason) = endpoint_answers(&endpoint) {
        return Ok(Live::Skip(reason));
    }
    Ok(Live::Ready(Target {
        model_id: model.id.clone(),
        settings: settings(&model),
        seed_base: (endpoint.protocol == ProtocolId::ChatCompletions)
            .then(|| model.api_config.clone().unwrap_or_else(|| json!({}))),
    }))
}

/// The rows the harness wrote, each kept as it was first, to put back.
#[derive(Default)]
pub struct Rows {
    saved: Vec<(&'static str, &'static str, String, Option<EntityValue>)>,
}

impl Rows {
    /// Bind `feature` to `model_id` at `reasoning`.
    ///
    /// # Errors
    ///
    /// Returns an error when the row cannot be read or written.
    pub fn bind(
        &mut self,
        factory: &ExecutionContextFactory,
        feature: &str,
        model_id: &str,
        reasoning: &str,
    ) -> Result<(), Error> {
        let facade = facade(factory)?;
        quietly(|| {
            self.save(&facade, BINDING, "featureId", feature)?;
            put(
                &facade,
                BINDING,
                &[("featureId", feature), ("modelId", model_id), ("reasoningLevel", reasoning)],
                false,
            )
        })
    }

    /// The replay model on the fake endpoint, with `recorded`'s settings (a transcript header's `model`); the fields
    /// it does not name are the defaults.
    ///
    /// # Errors
    ///
    /// Returns an error when a row cannot be written.
    pub fn replay_model(&mut self, factory: &ExecutionContextFactory, recorded: &Json) -> Result<(), Error> {
        let facade = facade(factory)?;
        // a recording holds only the steps' replies, so the replay model makes no side call (the intent hint, the
        // compaction summary, the schema repair), which would take the next recorded reply: the deterministic summary
        let mut fields: Vec<(String, String)> = vec![
            ("modelId".to_string(), REPLAY_MODEL.to_string()),
            ("endpointId".to_string(), REPLAY_MODEL.to_string()),
            ("providerModel".to_string(), "replay".to_string()),
            ("sideCalls".to_string(), "N".to_string()),
        ];
        for field in SETTINGS {
            if let Some(value) = recorded.get(field).filter(|value| !value.is_null()) {
                let text = value.as_str().map_or_else(|| value.to_string(), str::to_string);
                fields.push((field.to_string(), text));
            }
        }
        let fields: Vec<(&str, &str)> = fields.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
        quietly(|| {
            self.save(&facade, ENDPOINT, "endpointId", REPLAY_MODEL)?;
            self.save(&facade, MODEL, "modelId", REPLAY_MODEL)?;
            put(
                &facade,
                ENDPOINT,
                &[
                    ("endpointId", REPLAY_MODEL),
                    ("protocol", "fake"),
                    ("description", "Agent evaluation replay"),
                    ("isLocal", "Y"),
                ],
                false,
            )?;
            put(&facade, MODEL, &fields, true)
        })
    }

    /// Send `seed` to the model: its row's `apiConfigJson` is `base` (the conf's patch) with `seed` set.
    ///
    /// # Errors
    ///
    /// Returns an error when the row cannot be written.
    pub fn seed(&mut self, factory: &ExecutionContextFactory, model_id: &str, base: &Json, seed: u64) -> Result<(), Error> {
        let facade = facade(factory)?;
        let mut patch = base.as_object().cloned().unwrap_or_else(Map::new);
        patch.insert("seed".to_string(), json!(seed));
        let text = Json::Object(patch).to_string();
        quietly(|| {
            self.save(&facade, MODEL, "modelId", model_id)?;
            put(&facade, MODEL, &[("modelId", model_id), ("apiConfigJson", &text)], false)
        })
    }

    /// Put every row back as it was: a row that was there gets its fields again, one that was not is removed. A row
    /// that cannot be put back is logged and the others go on.
    pub fn restore(&mut self, factory: &ExecutionContextFactory) {
        let Ok(facade) = facade(factory) else {
            return;
        };
        for (entity, key, id, before) in self.saved.drain(..).rev() {
            let done = quietly(|| {
                in_tx(&facade, || {
                    if let Some(now) = facade.find(entity)?.eq(key, Value::string(&id)).disable_authz().one()? {
                        now.delete(&facade)?;
                    }
                    if let Some(row) = &before {
                        row.create(&facade)?;
                    }
                    Ok(())
                })
            });
            if let Err(error) = done {
                tracing::error!("the {entity} row {id} the agent evaluation changed was not put back: {error}");
            }
        }
    }

    /// Keep the row of `entity` with the key `id` as it is now, the first time only.
    fn save(&mut self, facade: &EntityFacade, entity: &'static str, key: &'static str, id: &str) -> Result<(), Error> {
        if self.saved.iter().any(|(saved, _, saved_id, _)| *saved == entity && saved_id == id) {
            return Ok(());
        }
        let row = facade.find(entity)?.eq(key, Value::string(id)).disable_authz().one()?;
        self.saved.push((entity, key, id.to_string(), row));
        Ok(())
    }
}

/// Write the row of `entity` with these fields (the first is its key): over the row there, or anew when `fresh`.
fn put(facade: &EntityFacade, entity: &str, fields: &[(&str, &str)], fresh: bool) -> Result<(), Error> {
    in_tx(facade, || {
        let (key, id) = fields[0];
        let found = facade.find(entity)?.eq(key, Value::string(id)).disable_authz().one()?;
        let mut row = match found {
            Some(row) if fresh => {
                row.delete(facade)?;
                facade.make_value(entity)?
            }
            Some(row) => row,
            None => facade.make_value(entity)?,
        };
        for &(field, value) in fields {
            row.set(field, Value::string(value));
        }
        row.create_or_update(facade)
    })
}

fn facade(factory: &ExecutionContextFactory) -> Result<EntityFacade, Error> {
    factory
        .entity()
        .ok_or_else(|| Error::new("the entity facade is not installed"))
}

/// A model's settings as `LlmModel` fields, for a transcript's header.
pub fn settings(model: &ModelConf) -> Json {
    let behavior = &model.behavior;
    json!({
        "modelId": model.id,
        "endpointId": model.endpoint,
        "providerModel": model.name,
        "tier": model.tier.as_str(),
        "contextWindow": model.context_window,
        "maxOutput": model.max_output,
        "toolMode": match behavior.tool_mode { ToolMode::Native => "native", ToolMode::Json => "json" },
        "parallelToolCalls": behavior.parallel_tool_calls.map(|on| if on { "Y" } else { "N" }),
        "promptVariant": match behavior.prompt_variant { PromptVariant::Standard => "standard", PromptVariant::Small => "small" },
        "resultCapChars": behavior.result_cap_chars,
        "screenSize": behavior.screen_size.map(|(columns, rows)| format!("{columns}x{rows}")),
        "listLimit": behavior.list_limit,
        "catalogSize": behavior.catalog_size,
        "maxToolCalls": behavior.max_tool_calls,
        "reasoningReplay": match behavior.reasoning_replay {
            ReasoningReplay::None => "none",
            ReasoningReplay::Last => "last",
            ReasoningReplay::All => "all",
        },
        "intentPrefilter": if behavior.intent_hint { "hint" } else { "none" },
        "compactHighPercent": behavior.compact_high_percent,
    })
}

/// The reasoning level of the feature's binding, else low.
pub fn feature_reasoning(factory: &ExecutionContextFactory, feature: &str) -> ReasoningLevel {
    let Ok(facade) = facade(factory) else {
        return ReasoningLevel::Low;
    };
    let registry = quietly(|| crate::llm::facade(factory).registry(&facade));
    registry
        .bindings
        .get(feature)
        .map_or(ReasoningLevel::Low, |binding| binding.conf.reasoning)
}

/// Whether the endpoint takes a connection: the `fake` protocol always does; another must accept a TCP connection on
/// its URL's host and port within a few seconds. `Err` says why not, for the skip line.
pub fn endpoint_answers(endpoint: &EndpointConf) -> Result<(), String> {
    if endpoint.protocol == ProtocolId::Fake {
        return Ok(());
    }
    let url = endpoint.url.trim();
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| format!("endpoint {} has no URL with a scheme ({url:?})", endpoint.id))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let default_port = if scheme.eq_ignore_ascii_case("https") { 443 } else { 80 };
    let has_port = if authority.starts_with('[') {
        authority.contains("]:")
    } else {
        authority.contains(':')
    };
    let host_port = if has_port {
        authority.to_string()
    } else {
        format!("{authority}:{default_port}")
    };
    let addresses: Vec<_> = host_port
        .to_socket_addrs()
        .map_err(|err| format!("endpoint {} at {url} does not resolve: {err}", endpoint.id))?
        .collect();
    let mut last = format!("endpoint {} at {url} has no address", endpoint.id);
    for address in addresses {
        match TcpStream::connect_timeout(&address, PROBE_TIMEOUT) {
            Ok(_) => return Ok(()),
            Err(err) => last = format!("endpoint {} at {url} does not answer: {err}", endpoint.id),
        }
    }
    Err(last)
}
