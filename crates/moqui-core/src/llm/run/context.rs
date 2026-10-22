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

//! What a tool is handed ([`TurnContext`], plan 4.4): the run, the user, the agent, the surface and its capabilities,
//! the budget, the cancel token and the event sink, the screen session, the approvals and the spill. Nothing hides in
//! thread-locals but the loop-guard origin on the execution context ([`crate::context::AgentOrigin`]).

use std::marker::PhantomData;

use moqui_lib::error::Error;
use moqui_lib::object::Value;
use rust_decimal::Decimal;

use super::events::EventSink;
use crate::entity::{EntityFacade, EntityValue};
use crate::llm::conf::{LimitsConf, ReasoningLevel, Tier};
use crate::llm::prompt::Surface;
use crate::llm::provider::CancelToken;
use crate::llm::tools::{ApprovalMode, Classification, ClientCaps, ToolRule};
use crate::llm::{decimal_of, flag_of, int_of, text_of};

/// The run a tool call belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunRef {
    pub run_id: String,
    pub conversation_id: String,
    pub step_seq: i64,
}

/// The user a run works for, as whom its tools run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserRef {
    pub user_id: String,
    pub username: String,
}

/// The surfaces of an agent whose `surfaces` is empty: the interactive ones a person sits at.
pub const DEFAULT_SURFACES: [Surface; 2] = [Surface::Drawer, Surface::Tui];

/// An agent (`AgentDefinition`), as a run reads it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentDef {
    pub agent_id: String,
    pub name: String,
    pub role_group_id: Option<String>,
    pub run_as_user_id: Option<String>,
    pub feature_id: Option<String>,
    pub tier: Option<Tier>,
    pub reasoning: Option<ReasoningLevel>,
    /// Empty is [`DEFAULT_SURFACES`].
    pub surfaces: Vec<Surface>,
    pub start_path: Option<String>,
    pub max_steps: Option<u32>,
    pub max_tool_calls: Option<u32>,
    pub max_tokens: Option<u64>,
    pub max_cost: Option<Decimal>,
    pub max_run_seconds: Option<u64>,
    pub local_only: bool,
    pub disabled: bool,
    /// The canvas dialects it may write (`canvasDialects`); empty is OpenUI Lang only.
    pub canvas_dialects: Vec<String>,
    /// The row, for what a later phase reads of it (instructions, requireSkill).
    pub row: Option<EntityValue>,
}

impl AgentDef {
    /// The agent `agent_id`, or `None` when there is none.
    ///
    /// # Errors
    ///
    /// Returns an error when the row cannot be read.
    pub fn load(facade: &EntityFacade, agent_id: &str) -> Result<Option<Self>, Error> {
        let row = facade
            .find("moqui.agent.AgentDefinition")?
            .eq("agentId", Value::string(agent_id))
            .use_cache(true)
            .disable_authz()
            .one()?;
        Ok(row.map(|row| Self::of(&row)))
    }

    pub fn of(row: &EntityValue) -> Self {
        let positive = |field: &str| int_of(row, field).filter(|n| *n > 0);
        Self {
            agent_id: text_of(row, "agentId").unwrap_or_default(),
            name: text_of(row, "agentName").unwrap_or_default(),
            role_group_id: text_of(row, "roleGroupId"),
            run_as_user_id: text_of(row, "runAsUserId"),
            feature_id: text_of(row, "featureId"),
            tier: text_of(row, "tier").and_then(|tier| Tier::parse(&tier)),
            reasoning: text_of(row, "reasoningLevel").and_then(|level| ReasoningLevel::parse(&level)),
            surfaces: text_of(row, "surfaces")
                .map(|list| list.split(',').filter_map(Surface::parse).collect())
                .unwrap_or_default(),
            start_path: text_of(row, "startPath"),
            max_steps: positive("maxSteps").and_then(|n| u32::try_from(n).ok()),
            max_tool_calls: positive("maxToolCalls").and_then(|n| u32::try_from(n).ok()),
            max_tokens: positive("maxTokens").and_then(|n| u64::try_from(n).ok()),
            max_cost: decimal_of(row, "maxCost").filter(|cost| *cost > Decimal::ZERO),
            max_run_seconds: positive("maxRunSeconds").and_then(|n| u64::try_from(n).ok()),
            local_only: flag_of(row, "localOnly") == Some(true),
            disabled: flag_of(row, "disabled") == Some(true),
            canvas_dialects: text_of(row, "canvasDialects")
                .map(|list| list.split(',').map(str::trim).filter(|name| !name.is_empty()).map(str::to_string).collect())
                .unwrap_or_default(),
            row: Some(row.clone()),
        }
    }

    /// Whether it serves `surface`: one its `surfaces` names, and with none named only the interactive defaults, the
    /// drawer and the terminal; a chat room, A2A, MCP or a job must be named.
    pub fn serves(&self, surface: Surface) -> bool {
        if self.surfaces.is_empty() {
            return DEFAULT_SURFACES.contains(&surface);
        }
        self.surfaces.contains(&surface)
    }

    /// Whether it may write canvases of `dialect`: those its `canvasDialects` name, and with none named only OpenUI
    /// Lang. Vue SFC is off unless an agent names it (plan 4.10: generated code in a frame that may still navigate).
    pub fn allows_dialect(&self, dialect: &str) -> bool {
        if self.canvas_dialects.is_empty() {
            return dialect == "openui";
        }
        self.canvas_dialects.iter().any(|name| name == dialect)
    }

    /// The role groups that bound its tools; none is no bound.
    pub fn role_groups(&self) -> Vec<String> {
        self.role_group_id.iter().cloned().collect()
    }
}

/// The caps a run starts with: the agent's, else the conf's; a caller may lower them, never raise them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunLimits {
    pub max_steps: Option<u32>,
    pub max_tool_calls: Option<u32>,
    pub max_tokens: Option<u64>,
    pub max_cost: Option<Decimal>,
    pub max_run_seconds: Option<u64>,
}

impl RunLimits {
    /// The agent's caps, else the conf's (steps, run seconds), each lowered to `asked` when that is lower.
    pub fn of(agent: &AgentDef, limits: &LimitsConf, asked: &RunLimits) -> Self {
        fn lower<T: PartialOrd + Copy>(base: Option<T>, asked: Option<T>) -> Option<T> {
            match (base, asked) {
                (Some(base), Some(asked)) if asked < base => Some(asked),
                (None, Some(asked)) => Some(asked),
                (base, _) => base,
            }
        }
        Self {
            max_steps: lower(agent.max_steps.or(Some(limits.max_steps)), asked.max_steps),
            max_tool_calls: lower(agent.max_tool_calls, asked.max_tool_calls),
            max_tokens: lower(agent.max_tokens, asked.max_tokens),
            max_cost: lower(agent.max_cost, asked.max_cost),
            max_run_seconds: lower(
                agent.max_run_seconds.or(Some(limits.max_run_seconds)),
                asked.max_run_seconds,
            ),
        }
    }
}

/// What a run may still spend, and what it has spent: steps, tool calls, tokens, cost and wall time without pauses.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Budget {
    pub caps: RunLimits,
    pub steps: u32,
    pub tool_calls: u32,
    pub tokens: u64,
    pub cost: Decimal,
    /// Milliseconds spent working, pauses left out.
    pub active_ms: i64,
    /// Tokens and cost a tool spent on its own (a delegate's model calls), which the run adds to its totals.
    pub tool_tokens: u64,
    pub tool_cost: Decimal,
}

/// The cap a run reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exhausted {
    Steps,
    ToolCalls,
    Tokens,
    Cost,
    Time,
    /// The user's daily quota.
    Quota,
}

impl Exhausted {
    pub fn as_str(self) -> &'static str {
        match self {
            Exhausted::Steps => "steps",
            Exhausted::ToolCalls => "tool calls",
            Exhausted::Tokens => "tokens",
            Exhausted::Cost => "cost",
            Exhausted::Time => "time",
            Exhausted::Quota => "daily quota",
        }
    }
}

impl Budget {
    /// The first cap the run has reached, checked before a step.
    pub fn exhausted(&self) -> Option<Exhausted> {
        let caps = &self.caps;
        if caps.max_steps.is_some_and(|max| self.steps >= max) {
            return Some(Exhausted::Steps);
        }
        if caps.max_tool_calls.is_some_and(|max| self.tool_calls >= max) {
            return Some(Exhausted::ToolCalls);
        }
        if caps.max_tokens.is_some_and(|max| self.tokens >= max) {
            return Some(Exhausted::Tokens);
        }
        if caps.max_cost.is_some_and(|max| self.cost >= max) {
            return Some(Exhausted::Cost);
        }
        if caps
            .max_run_seconds
            .is_some_and(|max| self.active_ms >= i64::try_from(max.saturating_mul(1000)).unwrap_or(i64::MAX))
        {
            return Some(Exhausted::Time);
        }
        None
    }

    /// Whether one more tool call fits the run's cap.
    pub fn has_tool_call(&self) -> bool {
        self.caps.max_tool_calls.is_none_or(|max| self.tool_calls < max)
    }
}

/// The screen session of a run's conversation ([`crate::llm::screen`]): the `screen` tool opens it through the
/// installed `ScreenSessions` on its first call of a step (or finds the one this node holds) and keeps its id here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScreenHandle {
    pub conversation_id: String,
    /// Empty until the session is opened or found.
    pub session_id: String,
    /// The columns and rows a new session draws, the model's `screen-size`; `None` is the default 100x30.
    pub size: Option<(u16, u16)>,
}

impl ScreenHandle {
    pub fn new(conversation_id: &str) -> Self {
        Self {
            conversation_id: conversation_id.to_string(),
            session_id: String::new(),
            size: None,
        }
    }

    #[must_use]
    pub fn sized(mut self, size: Option<(u16, u16)>) -> Self {
        self.size = size;
        self
    }
}

/// The tool call being made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallRef {
    pub tool_call_id: String,
    pub provider_call_id: String,
    pub name: String,
    pub classification: Classification,
}

/// How the writes of this call are approved ([`crate::llm::approval`]): the approval mode of its policy rule, whether
/// someone can be asked, what the policy says of a write of this tool (a READ tool such as `screen` writes only through
/// an approval, judged as a WRITE), and how long an approval waits.
pub struct Approvals<'a> {
    pub mode: ApprovalMode,
    /// Someone can be asked: the surface has an inline or linked approval.
    pub can_ask: bool,
    /// The policy's rule for a WRITE of this tool: an approval is asked only when it is offered.
    pub write_rule: ToolRule,
    /// Minutes an approval waits for its decision (the conf's `approval-ttl-minutes`).
    pub ttl_minutes: u32,
    _run: PhantomData<&'a ()>,
}

impl Approvals<'_> {
    pub fn new(mode: ApprovalMode, can_ask: bool) -> Self {
        Self {
            mode,
            can_ask,
            write_rule: ToolRule {
                mode,
                ..ToolRule::default()
            },
            ttl_minutes: 30,
            _run: PhantomData,
        }
    }

    #[must_use]
    pub fn with_write_rule(mut self, rule: ToolRule) -> Self {
        self.write_rule = rule;
        self
    }

    #[must_use]
    pub fn with_ttl_minutes(mut self, minutes: u32) -> Self {
        self.ttl_minutes = minutes.max(1);
        self
    }
}

/// How large a result may be, and where one over the model's cap goes ([`crate::llm::tools::spill`]): the run stores it
/// at `dbresource://agent/<runId>/<toolCallId>` after the tool returns and the model gets a preview.
pub struct Spill<'a> {
    pub result_cap: usize,
    /// The rows a list read answers when the call does not say (the model's `list-limit`); 0 is the tool's default.
    pub list_limit: usize,
    pub run_id: Option<&'a str>,
    pub tool_call_id: &'a str,
}

impl Spill<'_> {
    /// `text` within the result cap, cut with a note: what a result comes to when it cannot be stored.
    pub fn fit(&self, text: &str) -> String {
        let count = text.chars().count();
        if count <= self.result_cap || self.result_cap == 0 {
            return text.to_string();
        }
        let kept: String = text.chars().take(self.result_cap).collect();
        format!("{kept}\n…(cut at {} of {count} characters)", self.result_cap)
    }

    /// `text` within the result cap: as it is when it fits, else a preview in its place and the artifact that holds
    /// it whole, which the run writes with the result under its lease ([`crate::llm::tools::spill::Artifact`]). A run
    /// with no id (a direct MCP call) cuts the text instead.
    pub fn finish(&self, text: &str) -> (String, Option<crate::llm::tools::spill::Artifact>) {
        if self.result_cap == 0 || text.chars().count() <= self.result_cap {
            return (text.to_string(), None);
        }
        let Some(run_id) = self.run_id else {
            return (self.fit(text), None);
        };
        let artifact = crate::llm::tools::spill::Artifact::new(run_id, self.tool_call_id, text);
        let preview = crate::llm::tools::spill::preview(
            text,
            self.result_cap,
            &crate::llm::tools::spill::artifact_id(self.tool_call_id),
        );
        (preview, Some(artifact))
    }
}

/// Everything a tool call is given.
pub struct TurnContext<'a> {
    /// `None` for a direct MCP `tools/call`.
    pub run: Option<&'a RunRef>,
    pub user: &'a UserRef,
    pub agent: &'a AgentDef,
    pub surface: Surface,
    pub caps: &'a ClientCaps,
    pub budget: &'a mut Budget,
    pub cancel: &'a CancelToken,
    pub events: &'a mut EventSink,
    pub screen: Option<&'a mut ScreenHandle>,
    pub approvals: Approvals<'a>,
    pub spill: Spill<'a>,
    pub call: &'a ToolCallRef,
}
