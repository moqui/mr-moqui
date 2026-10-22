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

//! `tools/list` and `tools/call` of the MCP server (plans/LLM-A2A-Assist.md 4.16, 4.6, 4.8).
//!
//! The tools are the registry's, as the MCP agent is offered them on the `mcp` surface (whose one capability is
//! `approval.link`, plus `screen` when the host has screen sessions): the read tools `screen`, `search`, `data` and
//! `skill`, then the services an exposure opts in with `exposeMcp` that the agent's policy and role group and the
//! caller's own permissions let run, each by its tool name made safe for MCP (`[A-Za-z0-9_.-]`, at most 128), with
//! the exposure's title and description (an admin renames them for local terms) and `lockVersion`; past
//! `max-direct-tools` services, `find_tools`, `describe_tools` and `run_tool` stand for them. Sorted by name.
//!
//! A call of a read tool runs with no run, under the read fence, as the caller. A call that may write (a write
//! service, `run_tool`, `screen`, whose press of a saving button is held) is a run of the `mcp` surface: a conversation
//! (for `screen` the one its `session` argument names, so a screen session goes on across calls; it answers the
//! session with each result), one tool call, and the run's lease while it works, so its approvals, events and card are
//! those of any run. The run ends with the call and stores the result; a write pauses it AuthRequired for its owner
//! and answers `input_required` ([`super::state`]). The engine never steps such a run: a decision is read by the
//! client's retry, which runs the approved call here, once.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_lib::object::Value;
use serde_json::{json, Map, Value as Json};

use super::server::{McpError, Meta};
use super::{code, state, Caller};
use crate::context::{AgentOrigin, ErrorKind, ExecutionContext, ExecutionContextFactory};
use crate::entity::EntityFacade;
use crate::llm::approval::{self, ApprovalStatus, ApprovalView, Resumed};
use crate::llm::conf::PromptVariant;
use crate::llm::prompt::Surface;
use crate::llm::provider::CancelToken;
use crate::llm::run::context::{Approvals, RunRef, ScreenHandle, Spill, ToolCallRef, UserRef};
use crate::llm::run::step::{self, Part};
use crate::llm::run::{self, in_tx, quietly, AgentDef, Budget, Change, EventSink, RunState, TurnContext};
use crate::llm::tools::catalog::{Catalog, Entry};
use crate::llm::tools::{self, caps, AgentTool, Classification, ClientCaps, ToolInput, ToolOutcome, ToolPolicy};
use crate::llm::{flag_of, text_of};

/// The registry's tools the server lists by their own names.
const READ_TOOLS: [&str; 4] = ["data", "screen", "search", "skill"];
/// The lazy trio, listed instead of the services past `max-direct-tools`.
const LAZY_TOOLS: [&str; 3] = ["describe_tools", "find_tools", "run_tool"];
const RUN_TOOL: &str = "run_tool";
const SCREEN: &str = "screen";
/// The argument of `screen` that names its session.
const SESSION: &str = "session";
/// The key of the approval's input request.
const APPROVAL_INPUT: &str = "approval";

/// What a call of the server works with: the caller, the MCP agent and its policy, the surface's capabilities.
pub struct Ctx<'a> {
    pub factory: &'a Arc<ExecutionContextFactory>,
    pub facade: EntityFacade,
    pub caller: &'a Caller,
    pub user: UserRef,
    pub agent: AgentDef,
    pub policy: ToolPolicy,
    pub caps: ClientCaps,
}

impl<'a> Ctx<'a> {
    /// The context of `caller`'s request.
    ///
    /// # Errors
    ///
    /// An internal error when the MCP agent is missing, disabled, not exposed over MCP or does not serve `mcp`.
    pub fn new(factory: &'a Arc<ExecutionContextFactory>, caller: &'a Caller) -> Result<Self, McpError> {
        let facade = factory
            .entity()
            .ok_or_else(|| McpError::internal("the entity facade is not available"))?;
        let agent_id = crate::llm::facade(factory).conf().mcp.agent.clone();
        let agent = quietly(|| AgentDef::load(&facade, &agent_id))
            .map_err(|error| {
                tracing::error!("the MCP agent {agent_id} could not be read: {error}");
                McpError::internal("the MCP agent could not be read")
            })?
            .filter(|agent| {
                !agent.disabled
                    && agent.serves(Surface::Mcp)
                    && agent.row.as_ref().and_then(|row| flag_of(row, "exposeMcp")) == Some(true)
            })
            .ok_or_else(|| {
                tracing::error!(
                    "the MCP agent {agent_id} is missing, disabled, not exposed over MCP (exposeMcp) or does not \
                     serve mcp"
                );
                McpError::internal(format!("the MCP agent {agent_id} is not set up on this server"))
            })?;
        let policy = quietly(|| ToolPolicy::load(&facade, &agent.agent_id)).unwrap_or_else(|error| {
            tracing::error!(
                "the tool policy of the MCP agent {} could not be read: {error}",
                agent.agent_id
            );
            ToolPolicy::default()
        });
        let mut caps = ClientCaps::for_surface(Surface::Mcp);
        if crate::llm::screen::installed() {
            caps = caps.with([caps::SCREEN]);
        }
        Ok(Self {
            factory,
            facade,
            caller,
            user: UserRef {
                user_id: caller.user_id.clone(),
                username: caller.username.clone(),
            },
            agent,
            policy,
            caps,
        })
    }

    fn result_cap(&self) -> usize {
        usize::try_from(crate::llm::facade(self.factory).conf().mcp.max_result_bytes).unwrap_or(usize::MAX)
    }
}

/// What a listed tool runs.
#[derive(Clone)]
pub enum Runs {
    /// A tool of the registry, by its own name.
    Tool(Arc<dyn AgentTool>),
    /// An exposed service, through `run_tool`.
    Service(Box<Entry>),
}

/// One tool of `tools/list`.
#[derive(Clone)]
pub struct Listed {
    /// The MCP name.
    pub name: String,
    pub title: String,
    pub description: String,
    pub schema: Json,
    pub classification: Classification,
    pub runs: Runs,
    pub lock_version: Option<i64>,
}

impl Listed {
    /// The registry tool the call goes to.
    fn tool(&self) -> Option<Arc<dyn AgentTool>> {
        match &self.runs {
            Runs::Tool(tool) => Some(Arc::clone(tool)),
            Runs::Service(_) => tools::registered()
                .iter()
                .find(|tool| tool.spec().name == RUN_TOOL)
                .cloned(),
        }
    }

    /// Whether a call may write, so it needs a run that can wait for an approval.
    fn may_write(&self) -> bool {
        match &self.runs {
            Runs::Tool(tool) => tool.spec().may_pause || !tool.spec().classification.is_read(),
            Runs::Service(entry) => !entry.classification.is_read(),
        }
    }

    /// The arguments of the registry tool: a service's go to `run_tool` by its catalog name, and `screen`'s session is
    /// the server's, not the tool's.
    fn tool_args(&self, arguments: &Json) -> Json {
        match &self.runs {
            Runs::Service(entry) => json!({"name": entry.name, "parameters": arguments}),
            Runs::Tool(tool) if tool.spec().name == SCREEN => {
                let mut args = arguments.clone();
                if let Some(map) = args.as_object_mut() {
                    map.remove(SESSION);
                }
                args
            }
            Runs::Tool(_) => arguments.clone(),
        }
    }

    pub fn to_json(&self) -> Json {
        let read = self.classification.is_read() && !self.may_write();
        let mut annotations =
            json!({"readOnlyHint": read, "openWorldHint": self.classification == Classification::External});
        if !read {
            annotations["destructiveHint"] = json!(self.classification == Classification::Destructive);
            annotations["idempotentHint"] = json!(false);
        }
        let mut meta = json!({"org.moqui/classification": self.classification.as_str()});
        if let Some(version) = self.lock_version {
            meta["org.moqui/lockVersion"] = json!(version);
        }
        let mut tool = Map::new();
        tool.insert("name".to_string(), json!(self.name));
        if !self.title.is_empty() {
            tool.insert("title".to_string(), json!(self.title));
        }
        tool.insert("description".to_string(), json!(self.description));
        tool.insert("inputSchema".to_string(), self.schema.clone());
        tool.insert("annotations".to_string(), annotations);
        tool.insert("_meta".to_string(), meta);
        Json::Object(tool)
    }
}

/// `name` as an MCP tool name: the characters outside `[A-Za-z0-9_.-]` as `_`, at most 128.
pub fn mcp_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .take(128)
        .collect()
}

/// An input schema as MCP takes it: an object schema; none is one that takes no properties.
fn input_schema(schema: &Json) -> Json {
    match schema {
        Json::Object(map) => {
            let mut map = map.clone();
            map.entry("type".to_string()).or_insert_with(|| json!("object"));
            Json::Object(map)
        }
        _ => json!({"type": "object", "additionalProperties": false}),
    }
}

/// `screen`'s schema with its `session`.
fn screen_schema(schema: &Json) -> Json {
    let mut schema = input_schema(schema);
    if let Some(properties) = schema.get_mut("properties").and_then(Json::as_object_mut) {
        properties.insert(
            SESSION.to_string(),
            json!({"type": "string", "description":
                "The session a result named, to go on from that screen; leave it out to start a new one"}),
        );
    }
    schema
}

/// The tools of the caller, sorted by name: what [`list`] answers and [`call`] runs.
pub fn listed(ctx: &Ctx<'_>) -> Vec<Listed> {
    let offered = tools::offered(
        &ctx.caps,
        PromptVariant::Standard,
        &ctx.policy,
        &ctx.agent.role_groups(),
        &[],
    );
    let registry_tool = |name: &str| {
        offered
            .iter()
            .find(|offered| offered.tool.spec().name == name)
            .map(|offered| Arc::clone(&offered.tool))
    };
    let of_tool = |tool: Arc<dyn AgentTool>| {
        let spec = tool.spec().clone();
        Listed {
            name: spec.name.clone(),
            title: spec.title.clone(),
            schema: if spec.name == SCREEN {
                screen_schema(&spec.parameters)
            } else {
                input_schema(&spec.parameters)
            },
            description: spec.description.clone(),
            classification: spec.classification,
            runs: Runs::Tool(tool),
            lock_version: None,
        }
    };
    let mut out: Vec<Listed> = READ_TOOLS
        .iter()
        .filter_map(|name| registry_tool(name))
        .map(of_tool)
        .collect();
    let catalog = Catalog::for_mcp(&ctx.facade, &ctx.caps, &ctx.policy, &ctx.agent.role_groups());
    let services: Vec<&Entry> = catalog
        .entries
        .iter()
        .filter(|entry| entry.allowed && entry.exposure().is_some() && entry.user_may_run())
        .collect();
    let max = usize::try_from(crate::llm::facade(ctx.factory).conf().mcp.max_direct_tools).unwrap_or(usize::MAX);
    if services.len() > max {
        out.extend(LAZY_TOOLS.iter().filter_map(|name| registry_tool(name)).map(of_tool));
    } else {
        for entry in services {
            let name = mcp_name(&entry.name);
            if out.iter().any(|listed| listed.name == name) {
                tracing::error!(
                    "the exposed tool {} is left out of the MCP tools: its MCP name {name} is taken",
                    entry.name
                );
                continue;
            }
            let schema = match quietly(|| entry.schema(&ctx.facade)) {
                Ok(schema) => schema,
                Err(error) => {
                    tracing::error!(
                        "the exposed tool {} is left out of the MCP tools: its schema: {error}",
                        entry.name
                    );
                    continue;
                }
            };
            let mut description = if entry.description.is_empty() {
                entry.title.clone()
            } else {
                entry.description.clone()
            };
            if description.is_empty() {
                description = format!("Runs {} as the user.", entry.service_name().unwrap_or(&entry.name));
            }
            if !entry.classification.is_read() {
                description.push_str(" It waits for the user's approval in Moqui before it changes anything.");
            }
            out.push(Listed {
                name,
                title: entry.title.clone(),
                description,
                schema: input_schema(&schema),
                classification: entry.classification,
                lock_version: entry.exposure().map(|exposure| exposure.lock_version),
                runs: Runs::Service(Box::new(entry.clone())),
            });
        }
    }
    out.sort_by(|left, right| left.name.cmp(&right.name));
    out
}

/// `tools/list`: every tool in one page, private to the caller, fresh for the conf's `tools-ttl-ms`.
///
/// # Errors
///
/// A cursor this server did not make, or no MCP agent.
pub fn list(factory: &Arc<ExecutionContextFactory>, caller: &Caller, params: &Json) -> Result<Json, McpError> {
    if params
        .get("cursor")
        .is_some_and(|cursor| !cursor.is_null() && cursor.as_str() != Some(""))
    {
        return Err(McpError::invalid_params(
            "this server answers tools/list in one page, so the cursor is not one it made",
        ));
    }
    count_hit("tools/list")?;
    let ctx = Ctx::new(factory, caller)?;
    let tools: Vec<Json> = cached(&ctx).iter().map(Listed::to_json).collect();
    Ok(json!({"resultType": "complete", "tools": tools,
        "ttlMs": crate::llm::facade(factory).conf().mcp.tools_ttl_ms, "cacheScope": "private"}))
}

/// The tool lists built for the factory, by caller, each kept for the conf's `tools-ttl-ms` (what the caller may cache
/// too): a call and a list do not build it again. What a call runs is checked again as it runs. They are a factory
/// extension, never a process static keyed by the factory's address: a factory made after another was dropped may
/// have its address, and would get its tools.
#[derive(Default)]
struct Lists(std::sync::Mutex<std::collections::HashMap<String, Built>>);

/// A list and when it was built.
type Built = (Instant, Arc<Vec<Listed>>);

/// The most lists kept; past it the lapsed ones are dropped, then all.
const LISTS_MAX: usize = 1024;

/// [`listed`] for the caller, from the cache while it is fresh.
fn cached(ctx: &Ctx<'_>) -> Arc<Vec<Listed>> {
    let ttl = Duration::from_millis(crate::llm::facade(ctx.factory).conf().mcp.tools_ttl_ms);
    let cache = ctx.factory.extension(|_| Lists::default());
    let key = &ctx.user.user_id;
    let now = Instant::now();
    if let Some((built, list)) = moqui_lib::sync::lock(&cache.0).get(key) {
        if now.duration_since(*built) < ttl {
            return Arc::clone(list);
        }
    }
    let list = Arc::new(listed(ctx));
    if !ttl.is_zero() {
        let mut lists = moqui_lib::sync::lock(&cache.0);
        if lists.len() >= LISTS_MAX {
            lists.retain(|_, (built, _)| now.duration_since(*built) < ttl);
            if lists.len() >= LISTS_MAX {
                lists.clear();
            }
        }
        lists.insert(key.clone(), (now, Arc::clone(&list)));
    }
    list
}

/// What a call answers.
pub enum Called {
    Now(Json),
    /// The answer comes after a wait, on a thread of the host's own.
    Wait(Box<dyn FnOnce() -> Json + Send>),
}

/// A `CallToolResult` of text.
pub fn tool_result(text: &str, is_error: bool) -> Json {
    json!({"resultType": "complete", "content": [{"type": "text", "text": text}], "isError": is_error})
}

/// Count a call against the caller's AT_LLM tarpit (`mcp:<tool>`).
fn count_hit(name: &str) -> Result<(), McpError> {
    let artifact = format!("mcp:{name}");
    let counted = ExecutionContext::with_current(|ec| {
        let pushed = ec.push_checked("AT_LLM", &artifact, "view", false, true, None);
        if pushed.is_ok() {
            let _ = ec.pop();
        }
        pushed
    })
    .unwrap_or(Ok(()));
    match counted {
        Ok(()) => Ok(()),
        Err(error) if ErrorKind::of(&error) == ErrorKind::Tarpit => {
            let seconds = error.retry_after().unwrap_or(60);
            Err(
                McpError::new(429, code::TOO_MANY, "too many tool calls; try again later")
                    .data(json!({"retryAfterSeconds": seconds}))
                    .header("retry-after", seconds.to_string()),
            )
        }
        Err(error) => {
            tracing::error!("the MCP call of {name} could not be counted: {error}");
            Err(McpError::internal("the call could not be counted"))
        }
    }
}

/// `tools/call`: a read now; a write as a run that waits for its owner (MRTR); a retry with its `requestState`.
///
/// # Errors
///
/// A JSON-RPC error: no such tool, bad parameters or state, a client that cannot open the approval link, the tarpit.
pub fn call(
    factory: &Arc<ExecutionContextFactory>,
    caller: &Caller,
    params: &Json,
    meta: &Meta,
) -> Result<Called, McpError> {
    let name = params
        .get("name")
        .and_then(Json::as_str)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| McpError::invalid_params("tools/call needs the name of a tool"))?;
    let arguments = match params.get("arguments") {
        None | Some(Json::Null) => json!({}),
        Some(arguments @ Json::Object(_)) => arguments.clone(),
        Some(_) => return Err(McpError::invalid_params("arguments must be an object")),
    };
    // counted before anything is built, an unknown name as any other
    count_hit(&name.chars().take(128).collect::<String>())?;
    let ctx = Ctx::new(factory, caller)?;
    let Some(listed) = cached(&ctx).iter().find(|listed| listed.name == name).cloned() else {
        return Err(McpError::invalid_params(format!("Unknown tool: {name}")));
    };
    let digest = state::digest(name, &arguments);
    let origin = ExecutionContext::with_current(|ec| {
        let before = ec.agent_origin();
        ec.set_agent_origin(Some(AgentOrigin {
            agent_id: ctx.agent.agent_id.clone(),
            run_id: None,
        }));
        before
    })
    .flatten();
    let called = match params.get("requestState") {
        Some(Json::String(text)) => retry(
            &ctx,
            &listed,
            &arguments,
            text,
            params.get("inputResponses"),
            &digest,
            meta,
        ),
        Some(Json::Null) | None => fresh(&ctx, &listed, &arguments, &digest, meta),
        Some(_) => Err(McpError::invalid_params(
            "requestState must be the string the server gave",
        )),
    };
    ExecutionContext::with_current(|ec| ec.set_agent_origin(origin));
    called
}

/// A first call.
fn fresh(ctx: &Ctx<'_>, listed: &Listed, arguments: &Json, digest: &str, meta: &Meta) -> Result<Called, McpError> {
    let tool = listed
        .tool()
        .ok_or_else(|| McpError::internal("the tool is not registered on this server"))?;
    if !listed.may_write() {
        return Ok(Called::Now(direct(ctx, tool.as_ref(), &listed.tool_args(arguments))));
    }
    // a write is approved by its owner through a link: a client that cannot show one cannot make it
    if !meta.url_elicitation() && writes(ctx, listed, arguments) {
        return Err(missing_url_elicitation());
    }
    let key = match state::key(ctx.factory) {
        Ok(key) => key,
        Err(error) => {
            tracing::error!("an MCP write was refused: {error}");
            return Ok(Called::Now(tool_result(
                "Writes are not available on this MCP server (it has no key for approvals), so nothing was changed.",
                true,
            )));
        }
    };
    let session = match arguments.get(SESSION) {
        Some(Json::String(id)) if listed.name == SCREEN && !id.trim().is_empty() => Some(id.trim().to_string()),
        _ => None,
    };
    let call_run = match CallRun::start(ctx, listed, tool.as_ref(), arguments, session.as_deref(), meta)? {
        Ok(call_run) => call_run,
        Err(busy) => return Ok(Called::Now(tool_result(&busy, true))),
    };
    let outcome = call_run.invoke(ctx, tool.as_ref(), &listed.tool_args(arguments), None);
    call_run.finish(ctx, listed, outcome, digest, &key, meta)
}

/// Whether the call is a write known before it runs: a write service, or `run_tool` naming one (a `screen` press is
/// known only once the screen holds it).
fn writes(ctx: &Ctx<'_>, listed: &Listed, arguments: &Json) -> bool {
    match &listed.runs {
        Runs::Service(entry) => !entry.classification.is_read(),
        Runs::Tool(tool) if tool.spec().name == RUN_TOOL => {
            let name = arguments.get("name").and_then(Json::as_str).unwrap_or_default();
            Catalog::for_mcp(&ctx.facade, &ctx.caps, &ctx.policy, &ctx.agent.role_groups())
                .get(name)
                .is_some_and(|entry| !entry.classification.is_read())
        }
        Runs::Tool(_) => false,
    }
}

/// The error of a write for a client that does not take URL elicitations.
fn missing_url_elicitation() -> McpError {
    McpError::new(
        400,
        code::MISSING_CLIENT_CAPABILITY,
        "a write waits for its user's approval in Moqui, through a link the client shows (a URL elicitation), and \
         this client does not take them",
    )
    .data(json!({"requiredCapabilities": {"elicitation": {"url": {}}}}))
}

/// A read with no run: fenced as a READ, as the caller.
fn direct(ctx: &Ctx<'_>, tool: &dyn AgentTool, args: &Json) -> Json {
    let tool_call_id = format!("mcp-{}", state::nonce().unwrap_or_default());
    let mut sink = EventSink::new(ctx.factory, &ctx.facade, "", &ctx.user.user_id).detached();
    let mut budget = Budget::default();
    let cancel = CancelToken::new();
    let reference = ToolCallRef {
        tool_call_id: tool_call_id.clone(),
        provider_call_id: tool_call_id.clone(),
        name: tool.spec().name.clone(),
        classification: tool.spec().classification,
    };
    let spill = Spill {
        result_cap: ctx.result_cap(),
        list_limit: 0,
        run_id: None,
        tool_call_id: &tool_call_id,
    };
    let turn = TurnContext {
        run: None,
        user: &ctx.user,
        agent: &ctx.agent,
        surface: Surface::Mcp,
        caps: &ctx.caps,
        budget: &mut budget,
        cancel: &cancel,
        events: &mut sink,
        screen: None,
        approvals: approvals(ctx, tool, false),
        spill,
        call: &reference,
    };
    let outcome = step::invoke(tool, turn, args.clone(), None);
    let fit = |text: &str| {
        Spill {
            result_cap: ctx.result_cap(),
            list_limit: 0,
            run_id: None,
            tool_call_id: &tool_call_id,
        }
        .fit(text)
    };
    match outcome {
        ToolOutcome::Done(result) => tool_result(&fit(&result.text), false),
        ToolOutcome::Failed(text) => tool_result(&fit(&text), true),
        ToolOutcome::Pause(_) => tool_result(
            "This call needs an answer the MCP server cannot ask for, so it did not run.",
            true,
        ),
    }
}

/// How the writes of a call of `tool` are approved: by link (`can_ask`), its policy rule for a write of it (a READ
/// tool's held writes are judged as a WRITE), the conf's time to live.
fn approvals(ctx: &Ctx<'_>, tool: &dyn AgentTool, can_ask: bool) -> Approvals<'static> {
    let spec = tool.spec();
    let rule = if spec.classification.is_read() {
        ctx.policy.rule_as(&spec.name, Classification::Write)
    } else {
        ctx.policy.rule_for(spec)
    };
    Approvals::new(rule.mode, can_ask)
        .with_write_rule(rule)
        .with_ttl_minutes(crate::llm::facade(ctx.factory).conf().limits.approval_ttl_minutes)
}

/// The run of a call that may write, held by this node while the call works.
struct CallRun {
    run: RunRef,
    tool_call_id: String,
    local: Arc<run::LocalRun>,
}

impl Drop for CallRun {
    fn drop(&mut self) {
        self.local.busy.store(false, Ordering::SeqCst);
    }
}

/// A screen session that is busy with another call.
type Busy = String;

impl CallRun {
    /// A run of the call: in the conversation `session` names (the caller's, on the `mcp` surface), else a new one;
    /// Working, with this node's lease, its one tool call running and the messages the owner reads in the drawer.
    ///
    /// # Errors
    ///
    /// A session that is not the caller's, or rows that cannot be written; `Ok(Err(why))` when the session is busy.
    fn start(
        ctx: &Ctx<'_>,
        listed: &Listed,
        tool: &dyn AgentTool,
        arguments: &Json,
        session: Option<&str>,
        meta: &Meta,
    ) -> Result<Result<Self, Busy>, McpError> {
        let facade = &ctx.facade;
        let engine = run::engine(ctx.factory);
        let node = engine.node().to_string();
        let lease = run::lease::lease_millis(ctx.factory);
        let now = run::now_millis();
        let spec = tool.spec();
        let started = quietly(|| {
            in_tx(
                facade,
                || -> Result<Result<(String, String, String), Busy>, moqui_lib::error::Error> {
                    let conversation_id = match session {
                        Some(id) => {
                            let row = facade
                                .find(run::CONVERSATION)?
                                .eq("conversationId", Value::string(id))
                                .for_update(true)
                                .disable_authz()
                                .one()?
                                .filter(|row| {
                                    text_of(row, "userId").as_deref() == Some(ctx.user.user_id.as_str())
                                        && text_of(row, "surfaceEnumId").as_deref() == Some(Surface::Mcp.enum_id())
                                });
                            let Some(row) = row else {
                                return Ok(Err(format!(
                                    "There is no screen session {id} of yours; leave session out to start a new one."
                                )));
                            };
                            if let Some(active) = text_of(&row, "activeRunId") {
                                match run::load_run(facade, &active)? {
                                    Some(other) if !other.state.is_terminal() => {
                                        return Ok(Err(format!(
                                            "Screen session {id} is busy: another call runs in it or waits for its \
                                         approval. Wait for it, or leave session out to start a new one."
                                        )))
                                    }
                                    _ => run::clear_active(facade, id, &active)?,
                                }
                            }
                            id.to_string()
                        }
                        None => {
                            let mut row = facade.make_value(run::CONVERSATION)?;
                            row.set_sequenced_id_primary(facade)?;
                            row.set("userId", Value::string(&ctx.user.user_id));
                            row.set("agentId", Value::string(&ctx.agent.agent_id));
                            row.set("surfaceEnumId", Value::string(Surface::Mcp.enum_id()));
                            row.set("statusId", Value::string("AgcActive"));
                            row.set("title", Value::string(format!("MCP: {}", listed.name)));
                            row.set("lastMessageSeq", Value::I64(0));
                            row.set("createdDate", run::stamp(now));
                            row.create(facade)?;
                            text_of(&row, "conversationId").unwrap_or_default()
                        }
                    };
                    let mut row = facade.make_value(run::RUN)?;
                    row.set_sequenced_id_primary(facade)?;
                    let run_id = text_of(&row, "runId").unwrap_or_default();
                    for (field, value) in [
                        ("conversationId", Value::string(&conversation_id)),
                        ("agentId", Value::string(&ctx.agent.agent_id)),
                        ("userId", Value::string(&ctx.user.user_id)),
                        ("surfaceEnumId", Value::string(Surface::Mcp.enum_id())),
                        ("capsText", Value::string(ctx.caps.to_text())),
                        ("statusId", Value::string(RunState::Submitted.status_id())),
                        ("attempt", Value::I64(1)),
                        ("cancelRequested", Value::string("N")),
                        ("lastEventSeq", Value::I64(0)),
                        ("stepCount", Value::I64(1)),
                        ("toolCallCount", Value::I64(1)),
                        ("submittedDate", run::stamp(now)),
                        ("leaseOwner", Value::string(&node)),
                        ("leaseExpireDate", run::stamp(now + lease)),
                    ] {
                        row.set(field, value);
                    }
                    row.create(facade)?;
                    if !run::take_active(facade, &conversation_id, &run_id)? {
                        return Err(moqui_lib::error::Error::new("the conversation was taken meanwhile"));
                    }
                    let mut call = facade.make_value(run::TOOL_CALL)?;
                    call.set_sequenced_id_primary(facade)?;
                    let tool_call_id = text_of(&call, "toolCallId").unwrap_or_default();
                    let args = listed.tool_args(arguments);
                    for (field, value) in [
                        ("providerCallId", Value::string(format!("mcp-{tool_call_id}"))),
                        ("runId", Value::string(&run_id)),
                        ("stepSeq", Value::I64(1)),
                        ("toolName", Value::string(&spec.name)),
                        ("classificationEnumId", Value::string(spec.classification.enum_id())),
                        ("argumentsJson", Value::string(args.to_string())),
                        ("argumentsHash", Value::string(moqui_lib::json::canonical_sha256(&args))),
                        ("statusId", Value::string("AgtcRunning")),
                        ("startedDate", run::stamp(now)),
                        ("surfaceEnumId", Value::string(Surface::Mcp.enum_id())),
                        ("userId", Value::string(&ctx.user.user_id)),
                    ] {
                        call.set(field, value);
                    }
                    call.create(facade)?;
                    step::append_message(
                        facade,
                        &conversation_id,
                        Some(&run_id),
                        "user",
                        Some(&ctx.user.user_id),
                        None,
                        &[Part::text(&format!(
                            "{} called {} over MCP.",
                            meta.client_name(),
                            listed.name
                        ))],
                        None,
                        None,
                    )?;
                    step::append_message(
                        facade,
                        &conversation_id,
                        Some(&run_id),
                        "assistant",
                        None,
                        None,
                        &[Part {
                            kind: "AgpkToolCall",
                            data: Some(
                                json!({"id": format!("mcp-{tool_call_id}"), "name": spec.name, "arguments": args}),
                            ),
                            tool_call_id: Some(tool_call_id.clone()),
                            ..Part::default()
                        }],
                        None,
                        None,
                    )?;
                    Ok(Ok((conversation_id, run_id, tool_call_id)))
                },
            )
        });
        let (conversation_id, run_id, tool_call_id) = match started {
            Ok(Ok(ids)) => ids,
            Ok(Err(busy)) => return Ok(Err(busy)),
            Err(error) => {
                tracing::error!("the run of an MCP call of {} could not be made: {error}", listed.name);
                return Err(McpError::internal("the call could not be started"));
            }
        };
        let local = engine.local(&run_id);
        local.busy.store(true, Ordering::SeqCst);
        let call_run = Self {
            run: RunRef {
                run_id: run_id.clone(),
                conversation_id,
                step_seq: 1,
            },
            tool_call_id,
            local,
        };
        let mut sink = EventSink::new(ctx.factory, facade, &run_id, &ctx.user.user_id);
        if let Err(error) =
            quietly(|| run::transition(facade, &mut sink, &run_id, RunState::Working, &Change::default()))
        {
            tracing::error!("the run {run_id} of an MCP call could not start: {error}");
            engine.forget(&run_id);
            return Err(McpError::internal("the call could not be started"));
        }
        Ok(Ok(call_run))
    }

    /// Call (or resume, with `input`) the tool in the run, as the caller.
    fn invoke(&self, ctx: &Ctx<'_>, tool: &dyn AgentTool, args: &Json, input: Option<&ToolInput>) -> ToolOutcome {
        let mut sink = EventSink::new(ctx.factory, &ctx.facade, &self.run.run_id, &ctx.user.user_id);
        let mut budget = Budget::default();
        let mut screen = (tool.spec().name == SCREEN).then(|| ScreenHandle::new(&self.run.conversation_id));
        let reference = ToolCallRef {
            tool_call_id: self.tool_call_id.clone(),
            provider_call_id: format!("mcp-{}", self.tool_call_id),
            name: tool.spec().name.clone(),
            classification: tool.spec().classification,
        };
        ExecutionContext::with_current(|ec| {
            ec.set_agent_origin(Some(AgentOrigin {
                agent_id: ctx.agent.agent_id.clone(),
                run_id: Some(self.run.run_id.clone()),
            }));
        });
        let turn = TurnContext {
            run: Some(&self.run),
            user: &ctx.user,
            agent: &ctx.agent,
            surface: Surface::Mcp,
            caps: &ctx.caps,
            budget: &mut budget,
            cancel: &self.local.cancel,
            events: &mut sink,
            screen: screen.as_mut(),
            approvals: approvals(ctx, tool, true),
            spill: Spill {
                result_cap: ctx.result_cap(),
                list_limit: 0,
                run_id: None,
                tool_call_id: &self.tool_call_id,
            },
            call: &reference,
        };
        let outcome = step::invoke(tool, turn, args.clone(), input);
        sink.flush_logged();
        outcome
    }

    /// What the call came to: a result ends the run with it stored; an approval pauses the run and answers
    /// `input_required`.
    fn finish(
        self,
        ctx: &Ctx<'_>,
        listed: &Listed,
        outcome: ToolOutcome,
        digest: &str,
        key: &[u8],
        meta: &Meta,
    ) -> Result<Called, McpError> {
        let fit = |text: &str| {
            Spill {
                result_cap: ctx.result_cap(),
                list_limit: 0,
                run_id: None,
                tool_call_id: &self.tool_call_id,
            }
            .fit(text)
        };
        let session_note = |text: String| {
            if listed.name == SCREEN {
                format!(
                    "{text}\nsession: {} (pass it as session to go on from this screen)",
                    self.run.conversation_id
                )
            } else {
                text
            }
        };
        match outcome {
            ToolOutcome::Done(result) => {
                let text = session_note(fit(&result.text));
                let mut answer = tool_result(&text, false);
                if listed.name == SCREEN {
                    answer["_meta"] = json!({"org.moqui/session": self.run.conversation_id});
                }
                self.end(ctx, &text, false, &answer)?;
                Ok(Called::Now(answer))
            }
            ToolOutcome::Failed(text) => {
                let text = session_note(fit(&text));
                let answer = tool_result(&text, true);
                self.end(ctx, &text, true, &answer)?;
                Ok(Called::Now(answer))
            }
            ToolOutcome::Pause(pause) if pause.kind == tools::PauseKind::Approval => {
                let view = quietly(|| approval::load(&ctx.facade, &pause.reference)).ok().flatten();
                let Some(view) = view else {
                    self.cancel(ctx, "the approval could not be read");
                    return Err(McpError::internal("the approval could not be read"));
                };
                if !meta.url_elicitation() {
                    self.cancel(ctx, "the MCP client takes no URL elicitations");
                    return Err(missing_url_elicitation());
                }
                self.pause(ctx, pause)?;
                Ok(Called::Now(input_required(ctx, &view, listed, digest, key, true)?))
            }
            ToolOutcome::Pause(_) => {
                self.cancel(ctx, "the tool asked for input the MCP server cannot ask for");
                Ok(Called::Now(tool_result(
                    "This call needs an answer the MCP server cannot ask for, so it did not run.",
                    true,
                )))
            }
        }
    }

    /// The call ended: its result on the tool call and in the conversation, and the run Completed with `answer`.
    fn end(&self, ctx: &Ctx<'_>, text: &str, is_error: bool, answer: &Json) -> Result<(), McpError> {
        let facade = &ctx.facade;
        let mut sink = EventSink::new(ctx.factory, facade, &self.run.run_id, &ctx.user.user_id);
        let node = sink.node().to_string();
        let status = if is_error { "AgtcFailed" } else { "AgtcDone" };
        let ended = quietly(|| {
            in_tx(facade, || {
                run::held(facade, &self.run.run_id, &node, run::Hold::Lease)?;
                facade
                    .find(run::TOOL_CALL)?
                    .eq("toolCallId", Value::string(&self.tool_call_id))
                    .for_update(true)
                    .disable_authz()
                    .update_all([
                        ("statusId".to_string(), Value::string(status)),
                        ("endedDate".to_string(), run::stamp(run::now_millis())),
                        (
                            "resultSummary".to_string(),
                            Value::string(text.chars().take(4000).collect::<String>()),
                        ),
                    ])?;
                step::append_message(
                    facade,
                    &self.run.conversation_id,
                    Some(&self.run.run_id),
                    "tool",
                    None,
                    None,
                    &[Part {
                        kind: "AgpkToolResult",
                        text: Some(text.to_string()),
                        tool_call_id: Some(self.tool_call_id.clone()),
                        metadata: Some(json!({"providerCallId": format!("mcp-{}", self.tool_call_id),
                            "isError": is_error})),
                        ..Part::default()
                    }],
                    None,
                    None,
                )?;
                sink.emit(
                    "tool.result",
                    json!({"toolCallId": self.tool_call_id, "status": if is_error { "failed" } else { "done" },
                        "isError": is_error, "text": text.chars().take(2000).collect::<String>()}),
                );
                let mut stored = answer.clone();
                if let Some(map) = stored.as_object_mut() {
                    map.remove("_meta");
                }
                run::transition(
                    facade,
                    &mut sink,
                    &self.run.run_id,
                    RunState::Completed,
                    &Change {
                        result: Some(json!({"mcp": stored, "meta": answer.get("_meta")})),
                        ..Change::default()
                    },
                )
            })
        });
        run::engine(ctx.factory).forget(&self.run.run_id);
        ended.map_err(|error| {
            tracing::error!("the MCP call run {} could not end: {error}", self.run.run_id);
            McpError::internal("the call's result could not be stored")
        })
    }

    /// The call waits for its approval: the run pauses AuthRequired for it.
    fn pause(&self, ctx: &Ctx<'_>, pause: tools::Pause) -> Result<(), McpError> {
        let facade = &ctx.facade;
        let mut sink = EventSink::new(ctx.factory, facade, &self.run.run_id, &ctx.user.user_id);
        let node = sink.node().to_string();
        if let Some((event, payload)) = &pause.event {
            sink.emit(event, payload.clone());
        }
        let to = approval::pause_state(pause.kind, Surface::Mcp);
        let paused = quietly(|| {
            in_tx(facade, || {
                run::held(facade, &self.run.run_id, &node, run::Hold::Lease)?;
                facade
                    .find(run::TOOL_CALL)?
                    .eq("toolCallId", Value::string(&self.tool_call_id))
                    .for_update(true)
                    .disable_authz()
                    .update_all([("statusId".to_string(), Value::string("AgtcAwaiting"))])?;
                run::transition(
                    facade,
                    &mut sink,
                    &self.run.run_id,
                    to,
                    &Change {
                        pause: Some(pause),
                        ..Change::default()
                    },
                )
            })
        });
        run::engine(ctx.factory).forget(&self.run.run_id);
        paused.map_err(|error| {
            tracing::error!("the MCP call run {} could not pause: {error}", self.run.run_id);
            McpError::internal("the approval could not be asked for")
        })
    }

    /// The call cannot go on here: the run is Canceled, which expires its approval.
    fn cancel(&self, ctx: &Ctx<'_>, why: &str) {
        let mut sink = EventSink::new(ctx.factory, &ctx.facade, &self.run.run_id, &ctx.user.user_id);
        let canceled = quietly(|| {
            run::transition(
                &ctx.facade,
                &mut sink,
                &self.run.run_id,
                RunState::Canceled,
                &Change::reason(why).unleased(),
            )
        });
        if let Err(error) = canceled {
            tracing::error!("the MCP call run {} could not be canceled: {error}", self.run.run_id);
        }
        run::engine(ctx.factory).forget(&self.run.run_id);
    }
}

/// The `input_required` answer of a call waiting for `view`: the link the owner decides at (with the elicitation,
/// when `ask`), and the state the retry echoes.
fn input_required(
    ctx: &Ctx<'_>,
    view: &ApprovalView,
    listed: &Listed,
    digest: &str,
    key: &[u8],
    ask: bool,
) -> Result<Json, McpError> {
    let run = quietly(|| run::load_run(&ctx.facade, &view.run_id))
        .ok()
        .flatten()
        .ok_or_else(|| McpError::internal("the call's run could not be read"))?;
    let url = approval::link(&ctx.caller.base_url, &run.conversation_id);
    let claims = state::Claims {
        v: state::VERSION,
        aid: view.approval_id.clone(),
        sub: ctx.user.user_id.clone(),
        cid: view.run_id.clone(),
        tool: listed.name.clone(),
        dig: digest.to_string(),
        exp: view.expires.unwrap_or_else(|| run::now_millis() + 30 * 60_000),
        n: state::nonce().map_err(|error| McpError::internal(error.to_string()))?,
    };
    let mut result = json!({"resultType": "input_required", "requestState": state::seal(key, &claims),
        "_meta": {"org.moqui/approval": approval::link_data(view, &url)}});
    if ask {
        let summary = view.summary();
        let what = if summary.is_empty() {
            listed.name.clone()
        } else {
            summary
        };
        result["inputRequests"] = json!({APPROVAL_INPUT: {"method": "elicitation/create", "params": {"mode": "url",
            "url": url, "message": format!(
                "{what}: this waits for your approval in Moqui. Open the link, signed in to Moqui, and approve or \
                 reject it there; the call goes on when you come back.")}}});
    }
    Ok(result)
}

/// The retry of a call with the `requestState` its `input_required` gave.
fn retry(
    ctx: &Ctx<'_>,
    listed: &Listed,
    arguments: &Json,
    text: &str,
    responses: Option<&Json>,
    digest: &str,
    meta: &Meta,
) -> Result<Called, McpError> {
    let key = state::key(ctx.factory).map_err(|error| {
        tracing::error!("an MCP retry was refused: {error}");
        McpError::invalid_params("the requestState cannot be checked on this server")
    })?;
    let expect = state::Expect {
        user_id: &ctx.user.user_id,
        tool: &listed.name,
        digest,
        now: run::now_millis(),
    };
    let claims = state::open(&key, text, &expect).map_err(|refused| {
        tracing::warn!("an MCP requestState of {} was refused: {refused:?}", ctx.user.user_id);
        McpError::invalid_params(refused.message())
    })?;
    let view = quietly(|| approval::load_for_owner(&ctx.facade, &claims.aid, &ctx.user.user_id))
        .ok()
        .flatten()
        .filter(|view| view.run_id == claims.cid)
        .ok_or_else(|| McpError::invalid_params("the approval of this requestState is not there"))?;
    let run = quietly(|| run::load_run(&ctx.facade, &claims.cid))
        .ok()
        .flatten()
        .ok_or_else(|| McpError::invalid_params("the call of this requestState is not there"))?;
    if run.state.is_terminal() {
        return Ok(Called::Now(replay(&ctx.facade, &claims.cid)));
    }
    if !run.state.is_paused() {
        // another retry runs it now: its answer, once it ends
        return Ok(Called::Wait(waiter(ctx, &claims.cid, &claims.aid, text, false)));
    }
    let waiting = matches!(approval::resumed(&view), Ok(Resumed::Waiting(_)));
    if waiting {
        let action = responses
            .and_then(|responses| responses.get(APPROVAL_INPUT))
            .and_then(|response| response.get("action"))
            .and_then(Json::as_str);
        if matches!(action, Some("decline" | "cancel")) {
            return Ok(Called::Now(tool_result(
                "The approval link was not opened, so nothing was changed. The approval waits in Moqui until it \
                 expires; call again with the same requestState after deciding it there.",
                true,
            )));
        }
        return Ok(Called::Wait(waiter(ctx, &claims.cid, &claims.aid, text, true)));
    }
    resume(ctx, listed, arguments, &claims, text, digest, &key, meta)
}

/// The decided call goes on: this node takes the run, and the tool reads the decision (an approved write runs once).
/// A run another retry works (here, or on a node that holds its lease) is waited for.
#[allow(clippy::too_many_arguments)]
fn resume(
    ctx: &Ctx<'_>,
    listed: &Listed,
    arguments: &Json,
    claims: &state::Claims,
    state_text: &str,
    digest: &str,
    key: &[u8],
    meta: &Meta,
) -> Result<Called, McpError> {
    let tool = listed
        .tool()
        .ok_or_else(|| McpError::internal("the tool is not registered on this server"))?;
    let engine = run::engine(ctx.factory);
    let wait = || Ok(Called::Wait(waiter(ctx, &claims.cid, &claims.aid, state_text, false)));
    let local = engine.local(&claims.cid);
    if local.busy.swap(true, Ordering::SeqCst) {
        return wait();
    }
    let facade = &ctx.facade;
    // from here the run is this request's on this node until the call run drops
    let mut call_run = CallRun {
        run: RunRef {
            run_id: claims.cid.clone(),
            conversation_id: String::new(),
            step_seq: 1,
        },
        tool_call_id: quietly(|| approval::load(facade, &claims.aid))
            .ok()
            .flatten()
            .map(|view| view.tool_call_id)
            .unwrap_or_default(),
        local,
    };
    let lease = run::lease::lease_millis(ctx.factory);
    if !matches!(
        quietly(|| run::lease::claim(facade, &claims.cid, engine.node(), lease)),
        Ok(true)
    ) {
        engine.forget(&claims.cid);
        return wait();
    }
    let row = quietly(|| run::load_run(facade, &claims.cid)).ok().flatten();
    let Some(row) = row.filter(|row| row.state.is_paused()) else {
        if let Err(error) = quietly(|| run::lease::release(facade, &claims.cid, engine.node())) {
            tracing::error!(
                "the lease of MCP call run {} could not be given up: {error}",
                claims.cid
            );
        }
        engine.forget(&claims.cid);
        drop(call_run);
        let ended = quietly(|| run::load_run(facade, &claims.cid))
            .ok()
            .flatten()
            .is_none_or(|row| row.state.is_terminal());
        return if ended {
            Ok(Called::Now(replay(facade, &claims.cid)))
        } else {
            wait()
        };
    };
    call_run.run.conversation_id.clone_from(&row.conversation_id);
    let mut sink = EventSink::new(ctx.factory, facade, &claims.cid, &ctx.user.user_id);
    let went_on = quietly(|| -> Result<(), moqui_lib::error::Error> {
        run::transition(facade, &mut sink, &claims.cid, RunState::Working, &Change::default())?;
        // the decision's input is read here, from the approval, not by a step
        for input in step::open_inputs(facade, &claims.cid)? {
            step::consume_input(facade, &input.input_id)?;
        }
        in_tx(facade, || {
            facade
                .find(run::TOOL_CALL)?
                .eq("toolCallId", Value::string(&call_run.tool_call_id))
                .for_update(true)
                .disable_authz()
                .update_all([("statusId".to_string(), Value::string("AgtcRunning"))])
                .map(|_| ())
        })
    });
    if let Err(error) = went_on {
        tracing::error!("the MCP call run {} could not go on: {error}", claims.cid);
        engine.forget(&claims.cid);
        return Err(McpError::internal("the call could not go on"));
    }
    let view = quietly(|| approval::load(facade, &claims.aid)).ok().flatten();
    let input = ToolInput {
        kind: "decision".to_string(),
        text: view
            .as_ref()
            .map(|view| view.status.as_str().to_string())
            .unwrap_or_default(),
        data: Some(json!({"approvalId": claims.aid})),
    };
    let outcome = call_run.invoke(ctx, tool.as_ref(), &listed.tool_args(arguments), Some(&input));
    // a write that will not run (rejected, expired, a conflict, refused) is an error result, its text the why
    let ran_not = view.as_ref().is_some_and(|view| {
        matches!(
            view.status,
            ApprovalStatus::Rejected | ApprovalStatus::Expired | ApprovalStatus::Conflict | ApprovalStatus::Failed
        )
    });
    let outcome = match outcome {
        ToolOutcome::Done(result) if ran_not => ToolOutcome::Failed(result.text),
        outcome => outcome,
    };
    call_run.finish(ctx, listed, outcome, digest, key, meta)
}

/// The stored answer of a call whose run ended: its result, else why it has none.
fn replay(facade: &EntityFacade, run_id: &str) -> Json {
    let row = quietly(|| {
        facade
            .find(run::RUN)?
            .eq("runId", Value::string(run_id))
            .disable_authz()
            .one()
    })
    .ok()
    .flatten();
    let stored = row
        .as_ref()
        .and_then(|row| text_of(row, "resultJson"))
        .and_then(|text| serde_json::from_str::<Json>(&text).ok());
    if let Some(stored) = stored {
        if let Some(mut answer) = stored.get("mcp").cloned().filter(Json::is_object) {
            if let Some(meta) = stored.get("meta").filter(|meta| !meta.is_null()) {
                answer["_meta"] = meta.clone();
            }
            return answer;
        }
    }
    let error = row.as_ref().and_then(|row| text_of(row, "errorCode"));
    let state = row
        .as_ref()
        .and_then(|row| text_of(row, "statusId"))
        .and_then(|status| RunState::from_status_id(&status));
    let text = match (state, error.as_deref()) {
        (_, Some("pause_expired")) => "The approval expired before the user decided, so nothing was changed.",
        (Some(RunState::Canceled), _) => "The call was canceled, so nothing was changed.",
        _ => "The call ended without a result, so nothing more was changed.",
    };
    tool_result(text, true)
}

/// The wait of a retry on a thread of its own: until the approval is decided (`for_decision`) or the run ends or
/// pauses again, at most `approval-wait`; then the stored answer of an ended run, else `input_required` with the same
/// state (no new elicitation), which the client retries at once.
fn waiter(
    ctx: &Ctx<'_>,
    run_id: &str,
    approval_id: &str,
    state_text: &str,
    for_decision: bool,
) -> Box<dyn FnOnce() -> Json + Send> {
    let factory = Arc::clone(ctx.factory);
    let run_id = run_id.to_string();
    let approval_id = approval_id.to_string();
    let state_text = state_text.to_string();
    let wait = Duration::from_secs(crate::llm::facade(ctx.factory).conf().mcp.approval_wait);
    Box::new(move || {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let Some(facade) = factory.entity() else {
            return tool_result("The server could not read the call, so try again.", true);
        };
        let deadline = Instant::now() + wait;
        loop {
            let run = run::load_run(&facade, &run_id).ok().flatten();
            match &run {
                Some(run) if run.state.is_terminal() => return replay(&facade, &run_id),
                None => return tool_result("The call is not there any more.", true),
                _ => {}
            }
            let paused = run.as_ref().is_some_and(|run| run.state.is_paused());
            // decided is what the call resumes with that is not a wait: a hand-off waits for the user's own save
            let decided = for_decision
                && approval::load(&facade, &approval_id)
                    .ok()
                    .flatten()
                    .is_none_or(|view| !matches!(approval::resumed(&view), Ok(Resumed::Waiting(_))));
            if decided || (!for_decision && paused) || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let mut answer = json!({"resultType": "input_required"});
        if !state_text.is_empty() {
            answer["requestState"] = json!(state_text);
        }
        answer
    })
}
