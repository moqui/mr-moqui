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

//! The `llm-facade` conf element (plans/LLM-A2A-Assist.md section 6), read from the merged conf. A bad entry (an endpoint
//! with an unknown protocol, a model with no endpoint, a reasoning patch that is not a JSON object) is left out and named
//! in [`LlmConf::problems`]; the rest still loads.
//!
//! ```xml
//! <llm-facade enabled="true">
//!     <endpoint id="local" protocol="chat-completions" url="http://127.0.0.1:1234" local="true"/>
//!     <model id="local-qwen" endpoint="local" name="qwen3.8-27b" tier="balanced" context-window="32768">
//!         <reasoning level="low">{"reasoning_effort":"low"}</reasoning></model>
//!     <bind feature="assist" model="local-qwen" fallback="hosted" reasoning="low"/>
//!     <limits user-daily-tokens="2000000" user-daily-cost="20"/>
//!     <content-log enabled="false" redact="true" retention-days="14"/>
//! </llm-facade>
//! ```

use std::collections::BTreeMap;
use std::str::FromStr;

use moqui_lib::error::Error;
use moqui_lib::node::MNode;
use rust_decimal::Decimal;

/// The wire protocol of an endpoint; [`super::provider::protocol`] gives its implementation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ProtocolId {
    ChatCompletions,
    Responses,
    AnthropicMessages,
    /// The scripted fake of the tests.
    Fake,
}

impl ProtocolId {
    pub const ALL: [ProtocolId; 4] = [
        ProtocolId::ChatCompletions,
        ProtocolId::Responses,
        ProtocolId::AnthropicMessages,
        ProtocolId::Fake,
    ];

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|protocol| protocol.as_str() == text)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ProtocolId::ChatCompletions => "chat-completions",
            ProtocolId::Responses => "responses",
            ProtocolId::AnthropicMessages => "anthropic-messages",
            ProtocolId::Fake => "fake",
        }
    }

    /// The request path when the endpoint gives none.
    pub fn default_path(self) -> &'static str {
        match self {
            ProtocolId::ChatCompletions => "/v1/chat/completions",
            ProtocolId::Responses => "/v1/responses",
            ProtocolId::AnthropicMessages => "/v1/messages",
            ProtocolId::Fake => "",
        }
    }

    /// The auth header and the prefix of the key in it when the endpoint gives none.
    pub fn default_auth(self) -> (&'static str, &'static str) {
        match self {
            ProtocolId::AnthropicMessages => ("x-api-key", ""),
            _ => ("Authorization", "Bearer"),
        }
    }
}

/// A model's tier, which an agent may ask for in place of a feature.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tier {
    Fast,
    #[default]
    Balanced,
    Thorough,
}

impl Tier {
    pub const ALL: [Tier; 3] = [Tier::Fast, Tier::Balanced, Tier::Thorough];

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tier| tier.as_str() == text)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Fast => "fast",
            Tier::Balanced => "balanced",
            Tier::Thorough => "thorough",
        }
    }
}

/// How hard a model thinks; each level is a JSON patch of the model's (`reasoning` elements, `LlmModel.reasoningJson`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ReasoningLevel {
    None,
    #[default]
    Low,
    Medium,
    High,
    Max,
}

impl ReasoningLevel {
    pub const ALL: [ReasoningLevel; 5] = [
        ReasoningLevel::None,
        ReasoningLevel::Low,
        ReasoningLevel::Medium,
        ReasoningLevel::High,
        ReasoningLevel::Max,
    ];

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|level| level.as_str() == text)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ReasoningLevel::None => "none",
            ReasoningLevel::Low => "low",
            ReasoningLevel::Medium => "medium",
            ReasoningLevel::High => "high",
            ReasoningLevel::Max => "max",
        }
    }
}

/// How tools reach the model: the protocol's own tool calls, or signatures in the prompt and one JSON object back (plan 7.9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolMode {
    #[default]
    Native,
    Json,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PromptVariant {
    #[default]
    Standard,
    Small,
}

/// Which earlier reasoning goes back to the model: none, the last turn's, or all of it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReasoningReplay {
    None,
    #[default]
    Last,
    All,
}

/// A secret that does not show in `Debug` output.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(pub String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_empty() { "\"\"" } else { "\"***\"" })
    }
}

/// `…wxyz` (the last 4 characters) for a key of 12 or more, `***` for a shorter one, empty for none: what the admin
/// screens show.
pub fn mask_secret(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    match chars.len() {
        0 => String::new(),
        1..=11 => "***".to_string(),
        len => format!("…{}", chars[len - 4..].iter().collect::<String>()),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointConf {
    pub id: String,
    pub protocol: ProtocolId,
    pub url: String,
    /// Empty is [`ProtocolId::default_path`].
    pub path: Option<String>,
    pub api_key: Option<Secret>,
    /// A property or environment variable holding the key, read when `api_key` is empty. From the conf only: a
    /// database row cannot name one, so an admin screen cannot point a key it does not own at a host it chose.
    pub api_key_env: Option<String>,
    pub auth_header: Option<String>,
    pub auth_scheme: Option<String>,
    pub headers: Vec<(String, String)>,
    /// Seconds.
    pub connect_timeout: u64,
    /// Seconds without a byte of the response before a call fails as retryable.
    pub idle_timeout: u64,
    pub max_concurrent: Option<u32>,
    pub allow_private_network: bool,
    pub local: bool,
    pub disabled: bool,
}

impl EndpointConf {
    pub fn new(id: impl Into<String>, protocol: ProtocolId, url: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            protocol,
            url: url.into(),
            path: None,
            api_key: None,
            api_key_env: None,
            auth_header: None,
            auth_scheme: None,
            headers: Vec::new(),
            connect_timeout: 10,
            idle_timeout: 120,
            max_concurrent: None,
            allow_private_network: false,
            local: false,
            disabled: false,
        }
    }

    /// The URL of a request: the base URL and the path (the protocol's own when none is given), with one `/` between.
    pub fn request_url(&self) -> String {
        let path = self.path.as_deref().unwrap_or(self.protocol.default_path());
        format!("{}/{}", self.url.trim_end_matches('/'), path.trim_start_matches('/'))
            .trim_end_matches('/')
            .to_string()
    }

    /// The key: `api-key`, else the property or environment variable `api-key-env` names. None sends no auth header.
    pub fn resolved_key(&self) -> Option<String> {
        self.api_key
            .as_ref()
            .map(|key| key.0.clone())
            .filter(|key| !key.is_empty())
            .or_else(|| {
                self.api_key_env
                    .as_deref()
                    .map(moqui_lib::env::get_prop_or_env)
                    .filter(|key| !key.is_empty())
            })
    }

    /// The auth header and its value, when there is a key.
    pub fn auth(&self) -> Option<(String, String)> {
        let key = self.resolved_key()?;
        let (header, scheme) = self.protocol.default_auth();
        let header = self.auth_header.clone().unwrap_or_else(|| header.to_string());
        let scheme = self.auth_scheme.clone().unwrap_or_else(|| scheme.to_string());
        let value = if scheme.is_empty() || scheme == "none" {
            key
        } else {
            format!("{scheme} {key}")
        };
        Some((header, value))
    }
}

/// Prices per million tokens. Cached input and cache writes cost the input price when theirs are not given.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Prices {
    pub input: Option<Decimal>,
    pub cached_input: Option<Decimal>,
    pub cache_write: Option<Decimal>,
    pub output: Option<Decimal>,
    /// A `Uom` id, as USD.
    pub currency: String,
}

impl Prices {
    /// No price at all: the model's calls cost nothing that is known (a local model).
    pub fn is_unknown(&self) -> bool {
        self.input.is_none() && self.output.is_none() && self.cached_input.is_none() && self.cache_write.is_none()
    }
}

/// The per-model settings for small models (plan 7.9); `None` is the framework's default.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelBehavior {
    pub tool_mode: ToolMode,
    pub parallel_tool_calls: Option<bool>,
    pub prompt_variant: PromptVariant,
    pub result_cap_chars: Option<u32>,
    /// Columns and rows of a headless screen session.
    pub screen_size: Option<(u16, u16)>,
    pub list_limit: Option<u32>,
    pub catalog_size: Option<u32>,
    pub reasoning_replay: ReasoningReplay,
    pub temperature: Option<f64>,
    pub max_tool_calls: Option<u32>,
    pub compact_high_percent: Option<u32>,
    pub compact_low_percent: Option<u32>,
    pub max_concurrency: Option<u32>,
    /// `intent-prefilter="hint"`: a tiny pre-call names the likely intent in the tail (`likely: data`).
    pub intent_hint: bool,
    /// `side-calls="false"`: no call besides a step's own (no intent hint, the deterministic compaction summary, no
    /// schema repair call). The agent evaluation's replay model has it, since a recording holds only the steps' replies.
    pub no_side_calls: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelConf {
    pub id: String,
    pub endpoint: String,
    /// The provider's name of the model; empty is not configured.
    pub name: String,
    pub tier: Tier,
    pub context_window: Option<u32>,
    pub max_output: Option<u32>,
    pub prices: Prices,
    /// The JSON object patched onto a request body at each level.
    pub reasoning: BTreeMap<ReasoningLevel, serde_json::Value>,
    /// The JSON object patched onto every request body, before the reasoning patch.
    pub api_config: Option<serde_json::Value>,
    pub behavior: ModelBehavior,
    pub chars_per_token: Option<f64>,
    pub disabled: bool,
}

impl ModelConf {
    pub fn new(id: impl Into<String>, endpoint: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            endpoint: endpoint.into(),
            name: name.into(),
            tier: Tier::Balanced,
            context_window: None,
            max_output: None,
            prices: Prices {
                currency: "USD".to_string(),
                ..Prices::default()
            },
            reasoning: BTreeMap::new(),
            api_config: None,
            behavior: ModelBehavior::default(),
            chars_per_token: None,
            disabled: false,
        }
    }
}

/// A feature's model, the model to fall back to, and the reasoning level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindConf {
    pub feature: String,
    pub model: String,
    pub fallback: Option<String>,
    pub reasoning: ReasoningLevel,
}

/// The features the framework binds; a component may bind its own.
/// `transcribe` (dictation in the drawer) and `text-action` (the inline text actions) are bound only by a site that wants
/// them: a text action falls back to the fast tier, dictation to the browser's speech API when the admin allows it.
pub const FEATURES: [&str; 8] = ["assist", "title", "summarize", "classify", "ai-field", "embed", "transcribe", "text-action"];

#[derive(Clone, Debug, PartialEq)]
pub struct LimitsConf {
    pub max_concurrent_runs: u32,
    pub run_queue: u32,
    pub max_steps: u32,
    pub max_run_seconds: u64,
    /// Tokens per user per day when no `LlmQuota` row applies; 0 is no limit.
    pub user_daily_tokens: u64,
    /// Cost per user per day when no `LlmQuota` row applies; `None` is no limit.
    pub user_daily_cost: Option<Decimal>,
    pub approval_ttl_minutes: u32,
    pub lease_seconds: u32,
    pub heartbeat_seconds: u32,
    pub event_text_flush_ms: u32,
}

impl Default for LimitsConf {
    fn default() -> Self {
        Self {
            max_concurrent_runs: 8,
            run_queue: 64,
            max_steps: 40,
            max_run_seconds: 900,
            user_daily_tokens: 2_000_000,
            user_daily_cost: Some(Decimal::from(20)),
            approval_ttl_minutes: 30,
            lease_seconds: 60,
            heartbeat_seconds: 15,
            event_text_flush_ms: 300,
        }
    }
}

/// Whether the content of calls is kept in `LlmCall`, redacted, and for how long.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentLogConf {
    pub enabled: bool,
    pub redact: bool,
    /// More JSON keys whose values are redacted, beyond the secret-looking ones.
    pub redact_keys: Vec<String>,
    pub retention_days: u32,
}

impl Default for ContentLogConf {
    fn default() -> Self {
        Self {
            enabled: false,
            redact: true,
            redact_keys: Vec::new(),
            retention_days: 14,
        }
    }
}

/// The drawer's composer extras and memory (`assist`, plans/LLM-A2A-Assist.md 4.11, 7.6): the files a message may
/// carry, the audio dictation sends, the browser's speech API (off: it sends the audio to the browser's vendor), the
/// longest text of an inline text action, and whether a user who never chose lets the agent make memories.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssistConf {
    pub max_attachment_bytes: u64,
    pub max_attachments: u32,
    /// Media types a file may have: `type/subtype`, or `type/*`.
    pub attachment_types: Vec<String>,
    pub max_audio_bytes: u64,
    pub browser_dictation: bool,
    pub max_text_action_chars: u32,
    /// The `ASSIST_MEMORY` preference of a user who has not set it.
    pub memory_default: bool,
}

impl Default for AssistConf {
    fn default() -> Self {
        Self {
            max_attachment_bytes: 10 * 1024 * 1024,
            max_attachments: 5,
            attachment_types: [
                "text/*",
                "application/json",
                "application/xml",
                "application/pdf",
                "image/png",
                "image/jpeg",
                "image/gif",
                "image/webp",
            ]
            .iter()
            .map(|kind| (*kind).to_string())
            .collect(),
            max_audio_bytes: 10 * 1024 * 1024,
            browser_dictation: false,
            max_text_action_chars: 8000,
            memory_default: true,
        }
    }
}

impl AssistConf {
    /// Whether a file of `media_type` may be attached.
    pub fn allows_type(&self, media_type: &str) -> bool {
        let media_type = media_type.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
        self.attachment_types.iter().any(|allowed| {
            let allowed = allowed.trim().to_ascii_lowercase();
            match allowed.strip_suffix("/*") {
                Some(kind) => media_type.split('/').next() == Some(kind),
                None => allowed == media_type,
            }
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenSessionsConf {
    pub max: u32,
    pub max_per_user: u32,
    pub idle_minutes: u32,
}

impl Default for ScreenSessionsConf {
    fn default() -> Self {
        Self {
            max: 64,
            max_per_user: 4,
            idle_minutes: 30,
        }
    }
}

/// A `data-entity`: entities the `data` tool reads directly, by a regular expression on the package and one on the
/// entity name (a pattern left out matches any; both left out match none), as Java's `allowed-basic-entity`. With no
/// `data-entity` at all the `moqui.basic` package is readable; `readable="false"` turns a pattern off. The entities are
/// read with authorization off: encrypted fields are never read, and fields named like secrets (password, token,
/// secret, credential, a key) only when the pattern says `secret-fields="true"`; any other plaintext secret of a listed
/// entity is readable, so list only entities fit for every user of the agent.
/// A typed AI task a component adds (`ai-task`, [`crate::llm::ai_task`]): its name, the service that prepares its call
/// (an implementation of `org.moqui.agent.AiTaskServices.prepare#AiTask`), the feature whose model runs it, and what it
/// does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiTaskConf {
    pub name: String,
    pub service: String,
    pub feature: Option<String>,
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataEntityConf {
    pub package: Option<String>,
    pub entity: Option<String>,
    pub readable: bool,
    /// Fields named like secrets are read too.
    pub secret_fields: bool,
}

impl DataEntityConf {
    /// The default: the `moqui.basic` package (Enumeration, StatusItem, Uom, Geo and the rest).
    pub fn defaults() -> Vec<Self> {
        vec![Self {
            package: Some("moqui.basic".to_string()),
            entity: None,
            readable: true,
            secret_fields: false,
        }]
    }

    /// Whether this pattern names `package` and `entity`. A pattern that is not a regular expression matches none.
    pub fn matches(&self, package: &str, entity: &str) -> bool {
        if self.package.is_none() && self.entity.is_none() {
            return false;
        }
        let fits = |pattern: &Option<String>, text: &str| {
            pattern
                .as_deref()
                .is_none_or(|pattern| moqui_lib::object::regex_full_match(text, pattern).unwrap_or(false))
        };
        fits(&self.package, package) && fits(&self.entity, entity)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct A2aPushConf {
    pub allow_http: bool,
    pub allow_hosts: Vec<String>,
    pub timeout: u64,
    pub max_attempts: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct A2aSecurityConf {
    pub api_key: bool,
    pub bearer: bool,
    pub oidc_issuer: Option<String>,
    pub oidc_audience: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct A2aConf {
    pub enabled: bool,
    pub public_url: Option<String>,
    pub default_agent: String,
    pub blocking_wait: u64,
    pub max_part_bytes: u64,
    pub max_parts: u32,
    pub trust_forwarded_headers: bool,
    pub push: A2aPushConf,
    pub security: A2aSecurityConf,
    /// The A2A client (13.12): delegate tools to the remote agents of `AgentRemoteA2a` rows.
    pub client_enabled: bool,
    /// Seconds a delegation waits for the remote task to end or pause before it is canceled.
    pub client_timeout: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpConf {
    pub server_enabled: bool,
    pub protocol_version: String,
    pub tools_ttl_ms: u64,
    /// Seconds a call of a remote MCP server's tool may take: the MCP client's (13.12); the server's own calls run as
    /// the services they call allow.
    pub call_timeout: u64,
    /// The MCP client (13.12) is allowed.
    pub client_enabled: bool,
    /// The largest tool result in characters: the server cuts its results there, the client (13.12) refuses larger.
    pub max_result_bytes: u64,
    /// The agent whose policy and role group bound the server's tools (`agent`, `mcp` by default).
    pub agent: String,
    /// The base URL the server advertises (its card, the approval links); else from the request.
    pub public_url: Option<String>,
    pub trust_forwarded_headers: bool,
    /// The key of the `requestState` MACs, standard base64; else one derived from `session_cookie_key`.
    pub request_state_key: Option<String>,
    /// Seconds a retry of a call whose approval is still pending waits for the decision before it answers again.
    pub approval_wait: u64,
    /// Past this many exposed services, `tools/list` offers the lazy trio instead of each service.
    pub max_direct_tools: u32,
}

/// A model name the gateway answers to (`gateway/alias`): the model it calls, or the feature whose binding it follows.
/// An `LlmModelAlias` row of the same name replaces it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayAliasConf {
    pub name: String,
    pub model: Option<String>,
    pub feature: Option<String>,
    pub description: Option<String>,
}

/// The OpenAI-compatible gateway at `/llm/v1` ([`crate::llm::gateway`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayConf {
    pub enabled: bool,
    /// Only a key or app password an `LlmApiClient` with the `gateway` scope names may call.
    pub require_client: bool,
    /// Seconds a call may take in all.
    pub timeout: u64,
    pub aliases: Vec<GatewayAliasConf>,
}

/// The typed `llm-facade`.
#[derive(Clone, Debug, PartialEq)]
pub struct LlmConf {
    pub enabled: bool,
    pub endpoints: Vec<EndpointConf>,
    pub models: Vec<ModelConf>,
    pub binds: Vec<BindConf>,
    pub limits: LimitsConf,
    pub content_log: ContentLogConf,
    pub screen_sessions: ScreenSessionsConf,
    /// The entities `data` reads directly.
    pub data_entities: Vec<DataEntityConf>,
    /// The services that add facts about a user to a conversation's prefix (`user-facts`, in order), each an
    /// implementation of `org.moqui.agent.AgentServices.get#UserFacts` ([`crate::llm::facts`]).
    pub user_fact_services: Vec<String>,
    /// The AI tasks components add (`ai-task`, by name).
    pub ai_tasks: Vec<AiTaskConf>,
    pub a2a: A2aConf,
    pub mcp: McpConf,
    pub gateway: GatewayConf,
    pub assist: AssistConf,
    /// The entries left out, each named with why.
    pub problems: Vec<String>,
}

impl Default for LlmConf {
    fn default() -> Self {
        Self::from_node(None)
    }
}

impl LlmConf {
    /// Read the merged conf (a `moqui-conf` root or the `llm-facade` element). No element is the defaults, with no
    /// endpoints or models.
    pub fn from_conf(conf: &MNode) -> Self {
        let node = if conf.name() == "llm-facade" {
            Some(conf.clone())
        } else {
            conf.first_named("llm-facade")
        };
        Self::from_node(node.as_ref())
    }

    /// Read from the merged conf text a factory keeps.
    ///
    /// # Errors
    ///
    /// Returns an error when the text does not parse.
    pub fn from_conf_text(text: &str) -> Result<Self, Error> {
        Ok(Self::from_conf(&MNode::parse_text("conf", text)?))
    }

    fn from_node(node: Option<&MNode>) -> Self {
        let mut problems = Vec::new();
        let child = |name: &str| node.and_then(|node| node.first_named(name));
        let mut endpoints: Vec<EndpointConf> = Vec::new();
        let mut models: Vec<ModelConf> = Vec::new();
        let mut binds: Vec<BindConf> = Vec::new();
        if let Some(node) = node {
            for element in node.children_named("endpoint") {
                match endpoint_from(&element) {
                    Ok(endpoint) if endpoints.iter().any(|known| known.id == endpoint.id) => {
                        problems.push(format!("llm-facade endpoint {} is listed twice; the first is used", endpoint.id))
                    }
                    Ok(endpoint) => endpoints.push(endpoint),
                    Err(problem) => problems.push(problem),
                }
            }
            for element in node.children_named("model") {
                match model_from(&element, &mut problems) {
                    Ok(model) if models.iter().any(|known| known.id == model.id) => {
                        problems.push(format!("llm-facade model {} is listed twice; the first is used", model.id))
                    }
                    Ok(model) => models.push(model),
                    Err(problem) => problems.push(problem),
                }
            }
            for element in node.children_named("bind") {
                let (Some(feature), Some(model)) = (attr(&element, "feature"), attr(&element, "model")) else {
                    problems.push("llm-facade bind needs a feature and a model".to_string());
                    continue;
                };
                let reasoning = match attr(&element, "reasoning") {
                    None => ReasoningLevel::Low,
                    Some(text) => ReasoningLevel::parse(&text).unwrap_or_else(|| {
                        problems.push(format!("llm-facade bind {feature} reasoning {text} is not none, low, medium, high or max; low is used"));
                        ReasoningLevel::Low
                    }),
                };
                if binds.iter().any(|known| known.feature == feature) {
                    problems.push(format!("llm-facade bind {feature} is listed twice; the first is used"));
                    continue;
                }
                binds.push(BindConf {
                    feature,
                    model,
                    fallback: attr(&element, "fallback"),
                    reasoning,
                });
            }
            for model in &models {
                if !endpoints.iter().any(|endpoint| endpoint.id == model.endpoint) {
                    problems.push(format!(
                        "llm-facade model {} names endpoint {}, which the conf does not have (a LlmEndpoint row may)",
                        model.id, model.endpoint
                    ));
                }
            }
        }
        let limits = child("limits").map_or_else(LimitsConf::default, |limits| {
            let base = LimitsConf::default();
            LimitsConf {
                max_concurrent_runs: number(&limits, "max-concurrent-runs").unwrap_or(base.max_concurrent_runs),
                run_queue: number(&limits, "run-queue").unwrap_or(base.run_queue),
                max_steps: number(&limits, "max-steps").unwrap_or(base.max_steps),
                max_run_seconds: number(&limits, "max-run-seconds").unwrap_or(base.max_run_seconds),
                user_daily_tokens: number(&limits, "user-daily-tokens").unwrap_or(base.user_daily_tokens),
                user_daily_cost: match attr(&limits, "user-daily-cost") {
                    None => base.user_daily_cost,
                    Some(text) => Decimal::from_str(&text).ok().filter(|cost| !cost.is_zero()),
                },
                approval_ttl_minutes: number(&limits, "approval-ttl-minutes").unwrap_or(base.approval_ttl_minutes),
                lease_seconds: number(&limits, "lease-seconds").unwrap_or(base.lease_seconds),
                heartbeat_seconds: number(&limits, "heartbeat-seconds").unwrap_or(base.heartbeat_seconds),
                event_text_flush_ms: number(&limits, "event-text-flush-ms").unwrap_or(base.event_text_flush_ms),
            }
        });
        let content_log = child("content-log").map_or_else(ContentLogConf::default, |log| ContentLogConf {
            enabled: flag(&log, "enabled", false),
            redact: flag(&log, "redact", true),
            redact_keys: list(&log, "redact-keys"),
            retention_days: number(&log, "retention-days").unwrap_or(14),
        });
        let screen_sessions = child("screen-sessions").map_or_else(ScreenSessionsConf::default, |sessions| {
            ScreenSessionsConf {
                max: number(&sessions, "max").unwrap_or(64),
                max_per_user: number(&sessions, "max-per-user").unwrap_or(4),
                idle_minutes: number(&sessions, "idle-minutes").unwrap_or(30),
            }
        });
        let data_entities = match node.map(|node| node.children_named("data-entity")) {
            Some(elements) if !elements.is_empty() => elements
                .iter()
                .map(|element| DataEntityConf {
                    package: attr(element, "package"),
                    entity: attr(element, "entity"),
                    readable: flag(element, "readable", true),
                    secret_fields: flag(element, "secret-fields", false),
                })
                .collect(),
            _ => DataEntityConf::defaults(),
        };
        let mut user_fact_services: Vec<String> = Vec::new();
        for element in node.map(|node| node.children_named("user-facts")).unwrap_or_default() {
            match attr(&element, "service") {
                Some(service) if !user_fact_services.contains(&service) => user_fact_services.push(service),
                Some(_) => {}
                None => problems.push("llm-facade user-facts needs a service".to_string()),
            }
        }
        let mut ai_tasks: Vec<AiTaskConf> = Vec::new();
        for element in node.map(|node| node.children_named("ai-task")).unwrap_or_default() {
            match (attr(&element, "name"), attr(&element, "service")) {
                (Some(name), Some(service)) => {
                    ai_tasks.retain(|task| task.name != name);
                    ai_tasks.push(AiTaskConf {
                        name,
                        service,
                        feature: attr(&element, "feature"),
                        description: attr(&element, "description"),
                    });
                }
                _ => problems.push("llm-facade ai-task needs a name and a service".to_string()),
            }
        }
        let a2a = child("a2a");
        let push = a2a.as_ref().and_then(|a2a| a2a.first_named("push"));
        let security = a2a.as_ref().and_then(|a2a| a2a.first_named("security"));
        let a2a = A2aConf {
            enabled: a2a.as_ref().is_some_and(|a2a| flag(a2a, "enabled", false)),
            public_url: a2a.as_ref().and_then(|a2a| attr(a2a, "public-url")),
            default_agent: a2a
                .as_ref()
                .and_then(|a2a| attr(a2a, "default-agent"))
                .unwrap_or_else(|| "assist".to_string()),
            blocking_wait: a2a.as_ref().and_then(|a2a| number(a2a, "blocking-wait")).unwrap_or(120),
            max_part_bytes: a2a.as_ref().and_then(|a2a| number(a2a, "max-part-bytes")).unwrap_or(65536),
            max_parts: a2a.as_ref().and_then(|a2a| number(a2a, "max-parts")).unwrap_or(32),
            trust_forwarded_headers: a2a.as_ref().is_some_and(|a2a| flag(a2a, "trust-forwarded-headers", false)),
            push: A2aPushConf {
                allow_http: push.as_ref().is_some_and(|push| flag(push, "allow-http", false)),
                allow_hosts: push.as_ref().map(|push| list(push, "allow-hosts")).unwrap_or_default(),
                timeout: push.as_ref().and_then(|push| number(push, "timeout")).unwrap_or(15),
                max_attempts: push.as_ref().and_then(|push| number(push, "max-attempts")).unwrap_or(6),
            },
            security: A2aSecurityConf {
                api_key: security.as_ref().is_none_or(|security| flag(security, "api-key", true)),
                bearer: security.as_ref().is_none_or(|security| flag(security, "bearer", true)),
                oidc_issuer: security.as_ref().and_then(|security| attr(security, "oidc-issuer")),
                oidc_audience: security.as_ref().and_then(|security| attr(security, "oidc-audience")),
            },
            client_enabled: a2a.as_ref().is_none_or(|a2a| flag(a2a, "client-enabled", true)),
            client_timeout: a2a.as_ref().and_then(|a2a| number(a2a, "client-timeout")).unwrap_or(300),
        };
        let mcp = child("mcp");
        let mcp = McpConf {
            server_enabled: mcp.as_ref().is_some_and(|mcp| flag(mcp, "server-enabled", false)),
            protocol_version: mcp
                .as_ref()
                .and_then(|mcp| attr(mcp, "protocol-version"))
                .unwrap_or_else(|| "2026-07-28".to_string()),
            tools_ttl_ms: mcp.as_ref().and_then(|mcp| number(mcp, "tools-ttl-ms")).unwrap_or(300_000),
            call_timeout: mcp.as_ref().and_then(|mcp| number(mcp, "call-timeout")).unwrap_or(60),
            client_enabled: mcp.as_ref().is_none_or(|mcp| flag(mcp, "client-enabled", true)),
            max_result_bytes: mcp.as_ref().and_then(|mcp| number(mcp, "max-result-bytes")).unwrap_or(1_000_000),
            agent: mcp
                .as_ref()
                .and_then(|mcp| attr(mcp, "agent"))
                .unwrap_or_else(|| "mcp".to_string()),
            public_url: mcp.as_ref().and_then(|mcp| attr(mcp, "public-url")),
            trust_forwarded_headers: mcp.as_ref().is_some_and(|mcp| flag(mcp, "trust-forwarded-headers", false)),
            request_state_key: mcp.as_ref().and_then(|mcp| attr(mcp, "request-state-key")),
            approval_wait: mcp.as_ref().and_then(|mcp| number(mcp, "approval-wait")).unwrap_or(20),
            max_direct_tools: mcp.as_ref().and_then(|mcp| number(mcp, "max-direct-tools")).unwrap_or(40),
        };
        let gateway = child("gateway");
        let mut aliases: Vec<GatewayAliasConf> = Vec::new();
        for element in gateway.as_ref().map(|gateway| gateway.children_named("alias")).unwrap_or_default() {
            let (name, model, feature) = (attr(&element, "name"), attr(&element, "model"), attr(&element, "feature"));
            match name {
                Some(name) if model.is_some() || feature.is_some() => {
                    aliases.retain(|alias| alias.name != name);
                    aliases.push(GatewayAliasConf {
                        name,
                        model,
                        feature,
                        description: attr(&element, "description"),
                    });
                }
                _ => problems.push("llm-facade gateway alias needs a name and a model or a feature".to_string()),
            }
        }
        let gateway = GatewayConf {
            enabled: gateway.as_ref().is_some_and(|gateway| flag(gateway, "enabled", false)),
            require_client: gateway.as_ref().is_none_or(|gateway| flag(gateway, "require-client", true)),
            timeout: gateway.as_ref().and_then(|gateway| number(gateway, "timeout")).unwrap_or(300),
            aliases,
        };
        let assist = child("assist").map_or_else(AssistConf::default, |element| {
            let base = AssistConf::default();
            let types = list(&element, "attachment-types");
            AssistConf {
                max_attachment_bytes: number(&element, "max-attachment-bytes").unwrap_or(base.max_attachment_bytes),
                max_attachments: number(&element, "max-attachments").unwrap_or(base.max_attachments),
                attachment_types: if types.is_empty() { base.attachment_types } else { types },
                max_audio_bytes: number(&element, "max-audio-bytes").unwrap_or(base.max_audio_bytes),
                browser_dictation: flag(&element, "browser-dictation", false),
                max_text_action_chars: number(&element, "max-text-action-chars").unwrap_or(base.max_text_action_chars),
                memory_default: flag(&element, "memory-default", true),
            }
        });
        Self {
            enabled: node.is_none_or(|node| flag(node, "enabled", true)),
            endpoints,
            models,
            binds,
            limits,
            content_log,
            screen_sessions,
            data_entities,
            user_fact_services,
            ai_tasks,
            a2a,
            mcp,
            gateway,
            assist,
            problems,
        }
    }

    pub fn endpoint(&self, id: &str) -> Option<&EndpointConf> {
        self.endpoints.iter().find(|endpoint| endpoint.id == id)
    }

    pub fn model(&self, id: &str) -> Option<&ModelConf> {
        self.models.iter().find(|model| model.id == id)
    }

    pub fn bind(&self, feature: &str) -> Option<&BindConf> {
        self.binds.iter().find(|bind| bind.feature == feature)
    }

    /// Whether `data` may read the entity of `package` and `entity` directly: a readable pattern names it and no pattern
    /// that is off does.
    pub fn data_entity_readable(&self, package: &str, entity: &str) -> bool {
        let named = |readable: bool| {
            self.data_entities
                .iter()
                .any(|pattern| pattern.readable == readable && pattern.matches(package, entity))
        };
        named(true) && !named(false)
    }

    /// Whether `data` reads the fields named like secrets of a readable entity: a readable pattern that names it says
    /// `secret-fields="true"`.
    pub fn data_entity_secrets(&self, package: &str, entity: &str) -> bool {
        self.data_entities
            .iter()
            .any(|pattern| pattern.readable && pattern.secret_fields && pattern.matches(package, entity))
    }
}

fn endpoint_from(element: &MNode) -> Result<EndpointConf, String> {
    let id = attr(element, "id").ok_or("llm-facade endpoint needs an id")?;
    let protocol = match attr(element, "protocol") {
        None => ProtocolId::ChatCompletions,
        Some(text) => ProtocolId::parse(&text).ok_or_else(|| {
            format!("llm-facade endpoint {id} protocol {text} is not chat-completions, responses, anthropic-messages or fake; the endpoint is left out")
        })?,
    };
    let mut endpoint = EndpointConf::new(&id, protocol, attr(element, "url").unwrap_or_default());
    endpoint.path = attr(element, "path");
    endpoint.api_key = attr(element, "api-key").map(Secret);
    endpoint.api_key_env = attr(element, "api-key-env");
    endpoint.auth_header = attr(element, "auth-header");
    endpoint.auth_scheme = attr(element, "auth-scheme");
    endpoint.headers = element
        .children_named("header")
        .iter()
        .filter_map(|header| Some((attr(header, "name")?, attr(header, "value").unwrap_or_default())))
        .collect();
    endpoint.connect_timeout = number(element, "connect-timeout").unwrap_or(10);
    endpoint.idle_timeout = number(element, "idle-timeout").unwrap_or(120);
    endpoint.max_concurrent = number(element, "max-concurrent").filter(|max| *max > 0);
    endpoint.allow_private_network = flag(element, "allow-private-network", false);
    endpoint.local = flag(element, "local", false);
    endpoint.disabled = flag(element, "disabled", false);
    Ok(endpoint)
}

fn model_from(element: &MNode, problems: &mut Vec<String>) -> Result<ModelConf, String> {
    let id = attr(element, "id").ok_or("llm-facade model needs an id")?;
    let endpoint = attr(element, "endpoint").ok_or_else(|| format!("llm-facade model {id} needs an endpoint"))?;
    let mut model = ModelConf::new(&id, endpoint, attr(element, "name").unwrap_or_default());
    let mut parsed = |name: &str, value: Option<String>, known: &str| {
        if let Some(text) = &value {
            problems.push(format!("llm-facade model {id} {name} {text} is not {known}; the default is used"));
        }
    };
    if let Some(text) = attr(element, "tier") {
        match Tier::parse(&text) {
            Some(tier) => model.tier = tier,
            None => parsed("tier", Some(text), "fast, balanced or thorough"),
        }
    }
    model.context_window = number(element, "context-window");
    model.max_output = number(element, "max-output");
    let mut price = |name: &str| match attr(element, name) {
        None => None,
        Some(text) => match Decimal::from_str(&text) {
            Ok(price) => Some(price),
            Err(_) => {
                problems.push(format!("llm-facade model {id} {name} {text} is not a number; no price is used"));
                None
            }
        },
    };
    model.prices = Prices {
        input: price("price-input"),
        cached_input: price("price-cached-input"),
        cache_write: price("price-cache-write"),
        output: price("price-output"),
        currency: attr(element, "currency").unwrap_or_else(|| "USD".to_string()),
    };
    for reasoning in element.children_named("reasoning") {
        let level_text = attr(&reasoning, "level").unwrap_or_default();
        let Some(level) = ReasoningLevel::parse(&level_text) else {
            problems.push(format!("llm-facade model {id} reasoning level {level_text} is not none, low, medium, high or max; it is left out"));
            continue;
        };
        match json_object(reasoning.text().as_deref().unwrap_or("")) {
            Ok(patch) => {
                model.reasoning.insert(level, patch);
            }
            Err(problem) => problems.push(format!("llm-facade model {id} reasoning {level_text}: {problem}; it is left out")),
        }
    }
    if let Some(api) = element.first_named("api-config") {
        match json_object(api.text().as_deref().unwrap_or("")) {
            Ok(patch) => model.api_config = Some(patch),
            Err(problem) => problems.push(format!("llm-facade model {id} api-config: {problem}; it is left out")),
        }
    }
    let behavior = &mut model.behavior;
    match attr(element, "tool-mode").as_deref() {
        None | Some("native") => {}
        Some("json") => behavior.tool_mode = ToolMode::Json,
        Some(other) => problems.push(format!("llm-facade model {id} tool-mode {other} is not native or json; native is used")),
    }
    behavior.parallel_tool_calls = attr(element, "parallel-tool-calls").map(|text| text == "true");
    match attr(element, "prompt-variant").as_deref() {
        None | Some("standard") => {}
        Some("small") => behavior.prompt_variant = PromptVariant::Small,
        Some(other) => problems.push(format!("llm-facade model {id} prompt-variant {other} is not standard or small; standard is used")),
    }
    behavior.result_cap_chars = number(element, "result-cap-chars");
    behavior.screen_size = attr(element, "screen-size").and_then(|text| parse_screen_size(&text));
    behavior.list_limit = number(element, "list-limit");
    behavior.catalog_size = number(element, "catalog-size");
    match attr(element, "reasoning-replay").as_deref() {
        None | Some("last") => {}
        Some("none") => behavior.reasoning_replay = ReasoningReplay::None,
        Some("all") => behavior.reasoning_replay = ReasoningReplay::All,
        Some(other) => problems.push(format!("llm-facade model {id} reasoning-replay {other} is not none, last or all; last is used")),
    }
    behavior.temperature = number(element, "temperature");
    behavior.max_tool_calls = number(element, "max-tool-calls");
    behavior.compact_high_percent = number(element, "compact-high-percent");
    behavior.compact_low_percent = number(element, "compact-low-percent");
    behavior.max_concurrency = number(element, "max-concurrency");
    match attr(element, "intent-prefilter").as_deref() {
        None | Some("none" | "off") => {}
        Some("hint") => behavior.intent_hint = true,
        Some(other) => problems.push(format!("llm-facade model {id} intent-prefilter {other} is not none or hint; none is used")),
    }
    behavior.no_side_calls = !flag(element, "side-calls", true);
    model.chars_per_token = number(element, "chars-per-token");
    model.disabled = flag(element, "disabled", false);
    Ok(model)
}

/// `100x30` as columns and rows.
pub fn parse_screen_size(text: &str) -> Option<(u16, u16)> {
    let (columns, rows) = text.trim().split_once(['x', 'X'])?;
    Some((columns.trim().parse().ok()?, rows.trim().parse().ok()?))
}

/// A JSON object from conf or row text; empty text is an empty object.
pub fn json_object(text: &str) -> Result<serde_json::Value, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(serde_json::Value::Object(serde_json::Map::new()));
    }
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(value @ serde_json::Value::Object(_)) => Ok(value),
        Ok(_) => Err("not a JSON object".to_string()),
        Err(error) => Err(format!("not JSON ({error})")),
    }
}

/// An attribute with `${name}` expanded and trimmed. Empty is `None`.
fn attr(node: &MNode, name: &str) -> Option<String> {
    let raw = node.attribute(name)?;
    let expanded = moqui_lib::env::expand(&raw).unwrap_or(raw);
    let trimmed = expanded.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn number<T: FromStr>(node: &MNode, name: &str) -> Option<T> {
    attr(node, name).and_then(|text| text.parse().ok())
}

fn flag(node: &MNode, name: &str, default: bool) -> bool {
    match attr(node, name).as_deref() {
        Some("true") => true,
        Some("false") => false,
        _ => default,
    }
}

fn list(node: &MNode, name: &str) -> Vec<String> {
    attr(node, name)
        .map(|text| {
            text.split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}
