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

//! The tool registry (plans/LLM-A2A-Assist.md 4.3, 4.4 and 4.6).
//!
//! A Rust tool is an [`AgentTool`] registered with [`submit_tool!`](crate::submit_tool) through
//! `inventory::collect!(ToolRegistration)`, as native components are, so an add-on crate can add tools. Its
//! [`ToolSpec`] gives the name, the description and input schema the model reads, the [`Classification`] (READ, WRITE,
//! DESTRUCTIVE, EXTERNAL) and the surface capabilities it needs ([`caps`]).
//!
//! What a run is offered ([`offered`]) is decided deterministically, sorted by name, so the frozen prefix of a
//! conversation stays the same: the tools whose capabilities the surface has ([`ClientCaps`]), in the prompt variant,
//! that the agent's [`ToolPolicy`] (its `AgentToolPolicy` rows: a classification ceiling, an approval mode, per-run
//! limits) does not leave out, and whose artifact the agent's role group permits ([`crate::user::group_permits`]).
//!
//! Service tools are the services an `AgentToolExposure` row opts in, and they are never in the visible list: with
//! the add-on tools an agent does not pin (a [`ToolSpec::lazy`] tool no policy rule names exactly) they form the lazy
//! catalog ([`catalog`]) the model reaches through `find_tools`, `describe_tools` and `run_tool` ([`lazy`]), so the
//! tool list of a conversation stays the same however the catalog changes. A direct call of a catalog name is
//! answered with guidance to call `run_tool` ([`lazy_names`], [`suggest::unknown_tool`]).

pub mod apps;
pub mod ask;
pub mod catalog;
pub mod data;
pub mod dates;
pub mod delegate;
pub mod format;
pub mod lazy;
pub mod remember;
pub mod mcp_client;
pub mod repair;
pub mod schema;
pub mod search;
pub mod skill;
pub mod spill;
pub mod suggest;
pub mod screen;
pub mod ui;

use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};

use serde_json::Value as Json;

use super::conf::PromptVariant;
use super::prompt::Surface;
use super::provider;
use super::run::context::TurnContext;
use crate::entity::{EntityFacade, EntityValue};

pub use inventory::submit;

/// What a tool does, from least to most: a READ runs under the read fence; the others are writes (EXTERNAL reaches
/// another system) and run one at a time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Classification {
    #[default]
    Read,
    Write,
    Destructive,
    External,
}

impl Classification {
    pub const ALL: [Classification; 4] = [
        Classification::Read,
        Classification::Write,
        Classification::Destructive,
        Classification::External,
    ];

    /// The `AgentClassification` enumeration's `enumCode`.
    pub fn as_str(self) -> &'static str {
        match self {
            Classification::Read => "read",
            Classification::Write => "write",
            Classification::Destructive => "destructive",
            Classification::External => "external",
        }
    }

    pub fn enum_id(self) -> &'static str {
        match self {
            Classification::Read => "AgclRead",
            Classification::Write => "AgclWrite",
            Classification::Destructive => "AgclDestructive",
            Classification::External => "AgclExternal",
        }
    }

    /// From an `enumCode` or an `enumId`.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        Self::ALL
            .into_iter()
            .find(|class| class.as_str().eq_ignore_ascii_case(text) || class.enum_id() == text)
    }

    pub fn is_read(self) -> bool {
        self == Classification::Read
    }
}

/// Surface capabilities (plan 4.3): what the client of a conversation can do, which decides the tools offered.
pub mod caps {
    /// The run has a screen session: the host installed `ScreenSessions` ([`crate::llm::screen`]).
    pub const SCREEN: &str = "screen";
    /// The user's page follows the agent's screen session.
    pub const SCREEN_FOLLOW: &str = "screen.follow";
    pub const UI_OPENUI: &str = "ui.write:openui";
    pub const UI_VUE_SFC: &str = "ui.write:vue-sfc";
    /// Approval cards in the conversation.
    pub const APPROVAL_INLINE: &str = "approval.inline";
    /// Approvals by a link to the user's own session.
    pub const APPROVAL_LINK: &str = "approval.link";
    /// Writes run by policy, with no one to ask (a job).
    pub const APPROVAL_POLICY: &str = "approval.policy";
    pub const QUESTION_INLINE: &str = "question.inline";
    /// A question as a data part the client answers (A2A input-required).
    pub const QUESTION_DATA: &str = "question.data";
}

/// The capabilities in effect for a run: the surface's, lowered by what the client says it can do. A client may lower
/// them but never raise them; internal callers (a test, a surface adapter) may add their own.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClientCaps {
    caps: BTreeSet<String>,
}

impl ClientCaps {
    /// The capabilities of a surface, as plan 4.3 lists them.
    pub fn for_surface(surface: Surface) -> Self {
        let list: &[&str] = match surface {
            Surface::Drawer => &[
                caps::SCREEN_FOLLOW,
                caps::UI_OPENUI,
                caps::UI_VUE_SFC,
                caps::APPROVAL_INLINE,
                caps::QUESTION_INLINE,
            ],
            Surface::Chat => &[caps::QUESTION_INLINE, caps::APPROVAL_LINK],
            Surface::A2a => &[caps::QUESTION_DATA, caps::APPROVAL_LINK],
            Surface::Mcp => &[caps::APPROVAL_LINK],
            Surface::Job => &[caps::APPROVAL_POLICY],
            // the terminal's Assist pane (plan 4.12) follows on its own screen and asks inline as the drawer does
            Surface::Tui => &[caps::SCREEN_FOLLOW, caps::APPROVAL_INLINE, caps::QUESTION_INLINE],
        };
        Self::of(list.iter().copied())
    }

    pub fn of<'a>(caps: impl IntoIterator<Item = &'a str>) -> Self {
        Self {
            caps: caps
                .into_iter()
                .map(str::trim)
                .filter(|cap| !cap.is_empty())
                .map(str::to_string)
                .collect(),
        }
    }

    /// Only the capabilities `client` also names.
    pub fn lowered<'a>(&self, client: impl IntoIterator<Item = &'a str>) -> Self {
        let client: BTreeSet<&str> = client.into_iter().map(str::trim).collect();
        Self {
            caps: self
                .caps
                .iter()
                .filter(|cap| client.contains(cap.as_str()))
                .cloned()
                .collect(),
        }
    }

    pub fn with<'a>(mut self, extra: impl IntoIterator<Item = &'a str>) -> Self {
        self.caps.extend(
            extra
                .into_iter()
                .map(str::trim)
                .filter(|cap| !cap.is_empty())
                .map(str::to_string),
        );
        self
    }

    pub fn has(&self, cap: &str) -> bool {
        self.caps.contains(cap)
    }

    /// Without the canvas dialects (`ui.write:<name>`) `allowed` refuses.
    pub fn dialects_only(mut self, allowed: impl Fn(&str) -> bool) -> Self {
        self.caps.retain(|cap| cap.strip_prefix("ui.write:").is_none_or(&allowed));
        self
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.caps.iter().map(String::as_str)
    }

    /// A comma list, as `AgentRun.capsText` keeps it.
    pub fn to_text(&self) -> String {
        self.caps.iter().cloned().collect::<Vec<_>>().join(",")
    }

    pub fn from_text(text: &str) -> Self {
        Self::of(text.split(','))
    }

    /// Someone can answer a question or an approval: the run pauses for them instead of failing.
    pub fn interactive(&self) -> bool {
        self.has(caps::QUESTION_INLINE) || self.has(caps::QUESTION_DATA) || self.has(caps::APPROVAL_INLINE)
    }
}

/// An artifact a tool acts on, which the agent's role group must permit (`group_permits`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolArtifact {
    /// `AT_SERVICE`, `AT_ENTITY`, `AT_XML_SCREEN` ...
    pub artifact_type: String,
    pub name: String,
    /// view, create, update, delete, all.
    pub action: String,
}

/// What the model reads about a tool and what the runtime needs to know of it.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub title: String,
    pub description: String,
    /// A JSON schema object; `null` is an object with no properties.
    pub parameters: Json,
    pub classification: Classification,
    /// Every one of these must be among the run's capabilities.
    pub needs: Vec<String>,
    /// One of these must be among them, when there are any (`ask`: an inline question or a question as data).
    pub needs_any: Vec<String>,
    /// The prompt variants that offer it; empty is all.
    pub variants: Vec<PromptVariant>,
    pub artifact: Option<ToolArtifact>,
    /// It may pause the run (`ask`, an approval): it runs on its own, never beside other reads.
    pub may_pause: bool,
    /// An add-on tool of the lazy catalog: offered as a tool of its own only to an agent that pins it (an
    /// `AgentToolPolicy` row whose pattern is its exact name), else reached through `run_tool`.
    pub lazy: bool,
}

impl ToolSpec {
    pub fn new(name: &str, description: &str, parameters: Json, classification: Classification) -> Self {
        Self {
            name: name.to_string(),
            title: String::new(),
            description: description.to_string(),
            parameters,
            classification,
            needs: Vec::new(),
            needs_any: Vec::new(),
            variants: Vec::new(),
            artifact: None,
            may_pause: false,
            lazy: false,
        }
    }

    pub fn needs(mut self, caps: &[&str]) -> Self {
        self.needs = caps.iter().map(|cap| (*cap).to_string()).collect();
        self
    }

    pub fn needs_any(mut self, caps: &[&str]) -> Self {
        self.needs_any = caps.iter().map(|cap| (*cap).to_string()).collect();
        self
    }

    /// What is sent to the model.
    pub fn to_provider(&self) -> provider::ToolSpec {
        provider::ToolSpec::new(self.name.clone(), self.description.clone(), self.parameters.clone())
    }

    pub fn fits(&self, caps: &ClientCaps, variant: PromptVariant) -> bool {
        self.needs.iter().all(|need| caps.has(need))
            && (self.needs_any.is_empty() || self.needs_any.iter().any(|need| caps.has(need)))
            && (self.variants.is_empty() || self.variants.contains(&variant))
    }
}

/// What a tool call came to.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolOutcome {
    Done(ToolResult),
    /// The run waits: for an approval, an answer, a hand-off, a remote task or an authentication.
    Pause(Pause),
    /// An error the model reads.
    Failed(String),
}

impl ToolOutcome {
    pub fn text(text: impl Into<String>) -> Self {
        ToolOutcome::Done(ToolResult {
            text: text.into(),
            data: None,
        })
    }
}

/// A tool's result: the text the model reads, and data a client may show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolResult {
    pub text: String,
    pub data: Option<Json>,
}

/// What a run pauses for (the `AgentPauseKind` enumeration).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseKind {
    Approval,
    Question,
    Handoff,
    RemoteTask,
    Auth,
    Budget,
}

impl PauseKind {
    pub const ALL: [PauseKind; 6] = [
        PauseKind::Approval,
        PauseKind::Question,
        PauseKind::Handoff,
        PauseKind::RemoteTask,
        PauseKind::Auth,
        PauseKind::Budget,
    ];

    pub fn enum_id(self) -> &'static str {
        match self {
            PauseKind::Approval => "AgpsApproval",
            PauseKind::Question => "AgpsQuestion",
            PauseKind::Handoff => "AgpsHandoff",
            PauseKind::RemoteTask => "AgpsRemoteTask",
            PauseKind::Auth => "AgpsAuth",
            PauseKind::Budget => "AgpsBudget",
        }
    }

    pub fn from_enum_id(enum_id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.enum_id() == enum_id)
    }

    /// An authentication need is AuthRequired; the rest are InputRequired.
    pub fn state(self) -> super::run::RunState {
        match self {
            PauseKind::Auth => super::run::RunState::AuthRequired,
            _ => super::run::RunState::InputRequired,
        }
    }
}

/// A pause a tool asks for: its kind, what it waits on (an approval or question id), when it expires, and a stored
/// event for the clients (`approval.requested`, `question.asked`).
#[derive(Clone, Debug, PartialEq)]
pub struct Pause {
    pub kind: PauseKind,
    pub reference: String,
    /// Epoch milliseconds.
    pub expires: Option<i64>,
    pub event: Option<(String, Json)>,
}

/// Input that arrived for a paused tool call: an answer, a decision or a remote agent's reply.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolInput {
    /// The `AgentInputKind` `enumCode`: message, answer, decision, remote.
    pub kind: String,
    pub text: String,
    pub data: Option<Json>,
}

/// A tool. Its call runs on the run's step thread (or, for independent reads, a thread of its own) in an execution
/// context logged in as the user, a READ under the read fence.
pub trait AgentTool: Send + Sync {
    fn spec(&self) -> &ToolSpec;

    fn call(&self, ctx: &mut TurnContext<'_>, args: Json) -> ToolOutcome;

    /// The call paused and `input` came for it: what the model reads now. By default the input's text.
    fn resume(&self, _ctx: &mut TurnContext<'_>, _args: Json, input: &ToolInput) -> ToolOutcome {
        ToolOutcome::text(if input.text.is_empty() {
            input.data.as_ref().map(Json::to_string).unwrap_or_default()
        } else {
            input.text.clone()
        })
    }
}

/// A Rust tool, submitted with [`submit_tool!`](crate::submit_tool).
pub struct ToolRegistration {
    pub make: fn() -> Arc<dyn AgentTool>,
}

inventory::collect!(ToolRegistration);

/// The `make` of a tool that is `Default`.
pub fn make_default<T: AgentTool + Default + 'static>() -> Arc<dyn AgentTool> {
    Arc::new(T::default())
}

/// Register a Rust tool type (`AgentTool + Default`) from any linked crate:
///
/// ```ignore
/// moqui_core::submit_tool!(MyTool);
/// ```
#[macro_export]
macro_rules! submit_tool {
    ($tool:ty) => {
        $crate::llm::tools::submit! {
            $crate::llm::tools::ToolRegistration { make: $crate::llm::tools::make_default::<$tool> }
        }
    };
}

/// Every registered Rust tool, sorted by name; a second tool of a name is logged and left out.
pub fn registered() -> &'static [Arc<dyn AgentTool>] {
    static TOOLS: OnceLock<Vec<Arc<dyn AgentTool>>> = OnceLock::new();
    TOOLS.get_or_init(|| {
        let mut tools: Vec<Arc<dyn AgentTool>> = inventory::iter::<ToolRegistration>
            .into_iter()
            .map(|registration| (registration.make)())
            .collect();
        tools.sort_by(|left, right| left.spec().name.cmp(&right.spec().name));
        let mut kept: Vec<Arc<dyn AgentTool>> = Vec::with_capacity(tools.len());
        for tool in tools {
            if kept.last().is_some_and(|last| last.spec().name == tool.spec().name) {
                tracing::error!(
                    "a second agent tool named {} is registered, so it is left out",
                    tool.spec().name
                );
                continue;
            }
            kept.push(tool);
        }
        kept
    })
}

/// The names of the lazy catalog (plan 4.6: exposed services by tool and service name, MCP client tools, A2A delegates
/// and add-on tools no agent pins), which a model may call by mistake as tools of their own; [`suggest::unknown_tool`]
/// answers such a call with guidance to `run_tool`. Exposures that cannot be read are logged and left out.
pub fn lazy_names(facade: &EntityFacade) -> Vec<String> {
    catalog::all_names(facade)
}

/// How a tool's calls are approved (the `AgentApprovalMode` enumeration).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ApprovalMode {
    /// The user decides each write.
    #[default]
    Ask,
    /// Writes run without asking: background agents only.
    Policy,
    /// The tool is not offered.
    Never,
}

impl ApprovalMode {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "ask" | "AgamAsk" => Some(ApprovalMode::Ask),
            "policy" | "AgamPolicy" => Some(ApprovalMode::Policy),
            "never" | "AgamNever" => Some(ApprovalMode::Never),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalMode::Ask => "ask",
            ApprovalMode::Policy => "policy",
            ApprovalMode::Never => "never",
        }
    }
}

/// One `AgentToolPolicy` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyRule {
    pub sequence: i64,
    pub pattern: String,
    /// The highest classification it may run; `None` is any.
    pub ceiling: Option<Classification>,
    pub mode: ApprovalMode,
    pub allow_destructive: bool,
    pub calls_per_run: Option<u32>,
}

/// What policy says of one tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolRule {
    pub offered: bool,
    pub mode: ApprovalMode,
    pub calls_per_run: Option<u32>,
    /// The rule allows DESTRUCTIVE calls.
    pub allow_destructive: bool,
    /// The `sequenceNum` of the rule that decided; `None` when no rule matched.
    pub sequence: Option<i64>,
}

impl Default for ToolRule {
    fn default() -> Self {
        Self {
            offered: true,
            mode: ApprovalMode::Ask,
            calls_per_run: None,
            allow_destructive: false,
            sequence: None,
        }
    }
}

/// An agent's tool policy: its rules in sequence order, the first whose pattern matches a tool deciding. A tool no
/// rule matches is offered, its writes asked for; a DESTRUCTIVE tool needs a rule that allows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolPolicy {
    pub rules: Vec<PolicyRule>,
}

impl ToolPolicy {
    /// The agent's rows. A row that does not read is logged and left out.
    ///
    /// # Errors
    ///
    /// Returns an error when the rows cannot be read.
    pub fn load(facade: &EntityFacade, agent_id: &str) -> Result<Self, moqui_lib::error::Error> {
        let rows = facade
            .find("moqui.agent.AgentToolPolicy")?
            .eq("agentId", moqui_lib::object::Value::string(agent_id))
            .order_by("sequenceNum", false)
            .use_cache(true)
            .disable_authz()
            .list()?;
        Ok(Self {
            rules: rows
                .values()
                .iter()
                .filter_map(|row| policy_rule(agent_id, row))
                .collect(),
        })
    }

    /// Whether a rule names `name` exactly (no `*`) and offers it: the agent pins a lazy add-on tool so, which makes it
    /// a tool of its own instead of a catalog entry.
    pub fn pins(&self, name: &str) -> bool {
        self.rules
            .iter()
            .any(|rule| rule.pattern.trim() == name && rule.mode != ApprovalMode::Never)
    }

    pub fn rule_for(&self, spec: &ToolSpec) -> ToolRule {
        self.rule_as(&spec.name, spec.classification)
    }

    /// What policy says of a call of the tool `name` that is `classification`: a READ tool that writes only through
    /// an approval (`screen`'s held saves) is judged as a WRITE for it, so an agent capped at READ gets no approval.
    pub fn rule_as(&self, name: &str, classification: Classification) -> ToolRule {
        let Some(rule) = self
            .rules
            .iter()
            .find(|rule| schema::name_matches(&rule.pattern, name))
        else {
            return ToolRule {
                offered: classification != Classification::Destructive,
                ..ToolRule::default()
            };
        };
        let under_ceiling = rule.ceiling.is_none_or(|ceiling| classification <= ceiling);
        let destructive_ok = classification != Classification::Destructive || rule.allow_destructive;
        ToolRule {
            offered: rule.mode != ApprovalMode::Never && under_ceiling && destructive_ok,
            mode: rule.mode,
            calls_per_run: rule.calls_per_run,
            allow_destructive: rule.allow_destructive,
            sequence: Some(rule.sequence),
        }
    }
}

fn policy_rule(agent_id: &str, row: &EntityValue) -> Option<PolicyRule> {
    let sequence = super::int_of(row, "sequenceNum").unwrap_or_default();
    let Some(pattern) = super::text_of(row, "toolPattern") else {
        tracing::error!("tool policy {sequence} of agent {agent_id} has no toolPattern, so it is left out");
        return None;
    };
    let ceiling = match super::text_of(row, "classificationEnumId") {
        None => None,
        Some(text) => match Classification::parse(&text) {
            Some(class) => Some(class),
            None => {
                tracing::error!("tool policy {sequence} of agent {agent_id} has an unknown classification {text}, so it is left out");
                return None;
            }
        },
    };
    let mode = match super::text_of(row, "approvalModeEnumId") {
        None => ApprovalMode::Ask,
        Some(text) => {
            match ApprovalMode::parse(&text) {
                Some(mode) => mode,
                None => {
                    tracing::error!("tool policy {sequence} of agent {agent_id} has an unknown approval mode {text}, so it is left out");
                    return None;
                }
            }
        }
    };
    Some(PolicyRule {
        sequence,
        pattern,
        ceiling,
        mode,
        allow_destructive: super::flag_of(row, "allowDestructive") == Some(true),
        calls_per_run: super::int_of(row, "callsPerRun").and_then(|n| u32::try_from(n).ok()),
    })
}

/// A tool offered to a run, with what policy says of it.
#[derive(Clone)]
pub struct Offered {
    pub tool: Arc<dyn AgentTool>,
    pub rule: ToolRule,
}

impl std::fmt::Debug for Offered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Offered")
            .field("name", &self.tool.spec().name)
            .field("rule", &self.rule)
            .finish()
    }
}

/// The tools offered to a run, sorted by name: those registered whose capabilities `caps` has, in `variant`, that
/// `policy` offers, and whose artifact `role_groups` permit (none is no bound); a lazy add-on tool only when `policy`
/// pins it (it is a catalog entry otherwise). A permission check that cannot be read leaves the tool out, logged.
pub fn offered(
    caps: &ClientCaps,
    variant: PromptVariant,
    policy: &ToolPolicy,
    role_groups: &[String],
    extra: &[Arc<dyn AgentTool>],
) -> Vec<Offered> {
    let mut out: Vec<Offered> = Vec::new();
    for tool in registered().iter().chain(extra) {
        let spec = tool.spec();
        if !spec.fits(caps, variant) || (spec.lazy && !policy.pins(&spec.name)) {
            continue;
        }
        let rule = policy.rule_for(spec);
        if !rule.offered {
            continue;
        }
        if let (Some(artifact), false) = (&spec.artifact, role_groups.is_empty()) {
            match crate::user::group_permits(role_groups, &artifact.artifact_type, &artifact.name, &artifact.action) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(error) => {
                    tracing::error!(
                        "whether the role group may use tool {} could not be read, so it is not offered: {error}",
                        spec.name
                    );
                    continue;
                }
            }
        }
        out.push(Offered {
            tool: Arc::clone(tool),
            rule,
        });
    }
    out.sort_by(|left, right| left.tool.spec().name.cmp(&right.tool.spec().name));
    out.dedup_by(|later, earlier| later.tool.spec().name == earlier.tool.spec().name);
    out
}

/// The provider specs of the tools a surface and variant offer with no agent policy: what the prompt budget tests and
/// the prompt preview measure.
pub fn surface_specs(surface: Surface, variant: PromptVariant) -> Vec<provider::ToolSpec> {
    offered(
        // as a server with screen sessions offers them
        &ClientCaps::for_surface(surface).with([caps::SCREEN]),
        variant,
        &ToolPolicy::default(),
        &[],
        &[],
    )
    .iter()
    .map(|offered| offered.tool.spec().to_provider())
    .collect()
}

#[cfg(test)]
#[path = "tests.rs"]
pub(crate) mod tests;

#[cfg(test)]
#[path = "core_tests.rs"]
mod core_tests;

#[cfg(test)]
#[path = "lazy_tests.rs"]
mod lazy_tests;

#[cfg(test)]
#[path = "remote_tests.rs"]
mod remote_tests;
