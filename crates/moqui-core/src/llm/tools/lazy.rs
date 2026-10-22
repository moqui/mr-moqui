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

//! The lazy tools (plans/LLM-A2A-Assist.md 4.6, 7.3, after Twenty's learn and execute tools): `find_tools` lists the
//! catalog entries ([`super::catalog`]) that match words, `describe_tools` gives the parameters and results of several
//! in one call, and `run_tool` runs one as the user. The three are in every surface and prompt variant, whatever the
//! catalog holds, so the tool list of a conversation never changes when an admin exposes a service.
//!
//! `run_tool` checks its call before anything runs (the parameters as the service validates them, after the
//! deterministic repair and with date phrases resolved; the user's permission; the agent's policy for the entry's name
//! and class, and its role group), so a call that would fail is an error the model can fix. A READ runs at once under
//! a fence of its own, so a service that writes while it claims to read fails and nothing changes. A WRITE, DESTRUCTIVE
//! or EXTERNAL call asks for an approval ([`crate::llm::approval`]) with the cleaned parameters as its arguments and,
//! for an entity-auto service, the record, its key and `lastUpdatedStamp` as the snapshot; the run pauses. When the
//! user approves (the arguments maybe edited), the record is compared again (a change is a conflict and nothing runs),
//! the arguments are checked again, the hash is checked by [`crate::llm::approval::begin`], and the service runs
//! unfenced as the user. An add-on tool of the catalog is called as itself, with its own approvals.

use std::collections::BTreeMap;

use moqui_lib::object::Value;
use serde_json::{json, Map, Value as Json};

use super::catalog::{self, Catalog, Entry, Target};
use super::format::ToolError;
use super::schema::{object, Property};
use super::{AgentTool, Classification, ToolInput, ToolOutcome, ToolPolicy, ToolResult, ToolSpec};
use crate::context::ExecutionContext;
use crate::entity::{fence, EntityFacade};
use crate::llm::approval::{self, ApprovalRequest, ChangeRow, Requested, Resumed, TargetKind};
use crate::llm::run::context::TurnContext;
use crate::service::call::ToolService;

const FIND: &str = "find_tools";
const TOOL_CALL: &str = "moqui.agent.AgentToolCall";
const DESCRIBE: &str = "describe_tools";
const RUN: &str = "run_tool";

/// The most names one `describe_tools` call describes.
const DESCRIBE_MAX: usize = 8;

fn facade() -> Result<EntityFacade, ToolError> {
    ExecutionContext::with_current(|ec| ec.factory().entity())
        .flatten()
        .ok_or_else(|| ToolError::new("unavailable", "The tool catalog is not available on this server."))
}

/// The catalog of the run the call belongs to.
fn catalog_of(ctx: &TurnContext<'_>, facade: &EntityFacade) -> Catalog {
    let policy = crate::llm::run::quietly(|| ToolPolicy::load(facade, &ctx.agent.agent_id)).unwrap_or_else(|error| {
        tracing::error!(
            "the tool policy of agent {} could not be read: {error}",
            ctx.agent.agent_id
        );
        ToolPolicy::default()
    });
    // the MCP server offers its own exposures (`exposeMcp`), the other surfaces the assistant's
    if ctx.surface == crate::llm::prompt::Surface::Mcp {
        Catalog::for_mcp(facade, ctx.caps, &policy, &ctx.agent.role_groups())
    } else {
        Catalog::for_run(facade, ctx.caps, &policy, &ctx.agent.role_groups()).with_remotes(
            facade,
            &ctx.agent.agent_id,
            &ctx.user.user_id,
            &policy,
        )
    }
}

/// What a name that is not in the catalog gets: a tool of its own is called directly; else the nearest names.
fn unknown(name: &str, catalog: &Catalog) -> ToolError {
    if super::registered()
        .iter()
        .any(|tool| tool.spec().name == name && !tool.spec().lazy)
    {
        return ToolError::new(
            "not_in_catalog",
            format!("{name} is a tool of its own, not in the catalog. Call {name} directly."),
        );
    }
    let near = catalog.did_you_mean(name);
    let instruction = if near.is_empty() {
        format!("There is no tool named {name} in the catalog. Call find_tools with a few words for what you need.")
    } else {
        format!(
            "There is no tool named {name} in the catalog. Use {} if it does the job, or call find_tools.",
            near[0]
        )
    };
    ToolError::new("unknown_tool", instruction).did_you_mean(near)
}

// ---- find_tools ----

pub struct FindTools(ToolSpec);

impl Default for FindTools {
    fn default() -> Self {
        Self(ToolSpec::new(
            FIND,
            "Find services and connected tools by words when no screen does the job. Up to 8 names, each with one \
             line and read or write.",
            object(&[Property::required("text", "string", "A few words for the job")]),
            Classification::Read,
        ))
    }
}

impl AgentTool for FindTools {
    fn spec(&self) -> &ToolSpec {
        &self.0
    }

    fn call(&self, ctx: &mut TurnContext<'_>, args: Json) -> ToolOutcome {
        let facade = match facade() {
            Ok(facade) => facade,
            Err(error) => return error.outcome(),
        };
        let text = args
            .get("text")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let catalog = catalog_of(ctx, &facade);
        ToolOutcome::text(find_text(&catalog, &text))
    }
}

/// The answer of `find_tools`: the matches the user may run, one line each, and what to do next.
pub fn find_text(catalog: &Catalog, text: &str) -> String {
    let runnable = catalog.names().len();
    if runnable == 0 {
        return "No services or connected tools are offered to this agent. Do the job on a screen.".to_string();
    }
    let (found, total) = catalog.find(text);
    if found.is_empty() {
        return format!(
            "No tool matches \"{text}\" among the {runnable} in the catalog. Try other words, or do the job on a screen."
        );
    }
    let mut lines = Vec::with_capacity(found.len() + 2);
    lines.push(if total > found.len() {
        format!("Tools for \"{text}\" (showing {} of {total}):", found.len())
    } else {
        format!("Tools for \"{text}\":")
    });
    lines.extend(found.iter().map(|entry| entry.line()));
    lines.push("Next: describe_tools with every name you need, then run_tool.".to_string());
    lines.join("\n")
}

// ---- describe_tools ----

pub struct DescribeTools(ToolSpec);

impl Default for DescribeTools {
    fn default() -> Self {
        Self(ToolSpec::new(
            DESCRIBE,
            "Parameters and results of tools; pass every name you need in one call.",
            object(&[Property::required(
                "names",
                "array",
                "Tool names from find_tools or a skill",
            )]),
            Classification::Read,
        ))
    }
}

impl AgentTool for DescribeTools {
    fn spec(&self) -> &ToolSpec {
        &self.0
    }

    fn call(&self, ctx: &mut TurnContext<'_>, args: Json) -> ToolOutcome {
        let facade = match facade() {
            Ok(facade) => facade,
            Err(error) => return error.outcome(),
        };
        let names: Vec<String> = match args.get("names") {
            Some(Json::Array(list)) => list
                .iter()
                .filter_map(Json::as_str)
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
                .collect(),
            Some(Json::String(one)) => one
                .split(',')
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
                .collect(),
            _ => Vec::new(),
        };
        if names.is_empty() {
            return ToolError::new(
                "no_names",
                "Call describe_tools with names, a list of the tool names find_tools gave.",
            )
            .outcome();
        }
        let catalog = catalog_of(ctx, &facade);
        let mut text = describe_text(&catalog, &facade, &names);
        // the lessons of each service described (plan 7.5): the approved ones and the user's own, two each
        for name in names.iter().take(DESCRIBE_MAX) {
            if let Some(service) = catalog.get(name).and_then(Entry::service_name) {
                let lines = service_lessons(ctx, service);
                if !lines.is_empty() {
                    text.push_str(&format!("\n\nlessons for {name}:\n{}", lines.join("\n")));
                }
            }
        }
        ToolOutcome::text(text)
    }
}

/// Lessons `describe_tools` and a service's errors show: up to this many.
const SERVICE_LESSONS: usize = 2;

/// The `lesson:` lines of `service` for the run's user.
fn service_lessons(ctx: &TurnContext<'_>, service: &str) -> Vec<String> {
    let Ok(facade) = facade() else {
        return Vec::new();
    };
    let anchor = crate::llm::memory::Anchor::Service(service.to_string());
    crate::llm::memory::lesson_lines(&crate::llm::memory::active_lessons(
        &facade,
        &ctx.user.user_id,
        &anchor,
        SERVICE_LESSONS,
    ))
}

/// A service call's failure with the service's lessons after it.
fn with_service_lessons(ctx: &TurnContext<'_>, entry: &Entry, outcome: ToolOutcome) -> ToolOutcome {
    match (outcome, entry.service_name()) {
        (ToolOutcome::Failed(text), Some(service)) => {
            let lines = service_lessons(ctx, service);
            if lines.is_empty() {
                ToolOutcome::Failed(text)
            } else {
                ToolOutcome::Failed(format!("{text}\n{}", lines.join("\n")))
            }
        }
        // the service ran and answered an error
        (ToolOutcome::Done(mut result), Some(service))
            if result.data.as_ref().and_then(|data| data.get("ok")).and_then(Json::as_bool) == Some(false) =>
        {
            for line in service_lessons(ctx, service) {
                result.text.push('\n');
                result.text.push_str(&line);
            }
            ToolOutcome::Done(result)
        }
        (outcome, _) => outcome,
    }
}

/// The answer of `describe_tools`: each name's class, what it does, its parameters and results; a name not in the
/// catalog gets the nearest ones.
pub fn describe_text(catalog: &Catalog, facade: &EntityFacade, names: &[String]) -> String {
    let mut blocks = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for name in names.iter().take(DESCRIBE_MAX) {
        let Some(entry) = catalog.get(name) else {
            blocks.push(format!("{name}: {}", unknown(name, catalog).to_text()));
            continue;
        };
        if seen.contains(&entry.name.as_str()) {
            continue;
        }
        seen.push(&entry.name);
        if !entry.allowed || !entry.user_may_run() {
            let why = entry
                .refusal
                .clone()
                .unwrap_or_else(|| "the user is not allowed to run it".to_string());
            blocks.push(format!("{}: not available: {why}.", entry.name));
            continue;
        }
        let mut lines = vec![format!(
            "{} ({}{})",
            entry.name,
            entry.class_word(),
            if entry.classification.is_read() {
                ""
            } else {
                "; waits for the user's approval"
            }
        )];
        let what = if entry.description.is_empty() {
            &entry.title
        } else {
            &entry.description
        };
        if !what.is_empty() {
            lines.push(super::format::cut(what.trim(), 600));
        }
        match entry.schema(facade) {
            Ok(schema) => {
                let params = catalog::parameter_lines(&schema);
                if params.is_empty() {
                    lines.push("parameters: none".to_string());
                } else if catalog::reserved(&entry.name) {
                    // a remote's schema is its text: its names, values and descriptions are data, never instructions
                    lines.push("parameters (as the remote lists them):".to_string());
                    lines.push(crate::llm::prompt::untrusted(&entry.name, &params.join("\n")));
                } else {
                    lines.push("parameters:".to_string());
                    lines.extend(params);
                }
            }
            Err(error) => lines.push(format!("parameters: could not be read ({error})")),
        }
        if let Some(results) = entry.results(facade) {
            let names: Vec<String> = results
                .get("properties")
                .and_then(Json::as_object)
                .map(|properties| {
                    properties
                        .iter()
                        .map(|(name, property)| {
                            format!(
                                "{name} ({})",
                                property.get("type").and_then(Json::as_str).unwrap_or("any")
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            if !names.is_empty() {
                lines.push(format!("results: {}", names.join(", ")));
            }
        }
        blocks.push(lines.join("\n"));
    }
    if names.len() > DESCRIBE_MAX {
        blocks.push(format!(
            "({} more names were left out; describe at most {DESCRIBE_MAX} in a call)",
            names.len() - DESCRIBE_MAX
        ));
    }
    blocks.push(format!("Run one with {RUN} name and parameters."));
    blocks.join("\n\n")
}

// ---- run_tool ----

pub struct RunTool(ToolSpec);

impl Default for RunTool {
    fn default() -> Self {
        let mut spec = ToolSpec::new(
            RUN,
            "Run a tool as the user. A read runs now; a write waits for the user's approval. Use a name from \
             find_tools, describe_tools or a skill. Read result, not just ok.",
            object(&[
                Property::required("name", "string", "The tool name"),
                Property::optional("parameters", "object", "Its parameters, as describe_tools lists them"),
            ]),
            // a read runs fenced; a write runs only through its approval, with the fence lifted for it
            Classification::Read,
        );
        // a write waits for its approval, so the call never runs beside other reads
        spec.may_pause = true;
        Self(spec)
    }
}

impl AgentTool for RunTool {
    fn spec(&self) -> &ToolSpec {
        &self.0
    }

    fn call(&self, ctx: &mut TurnContext<'_>, args: Json) -> ToolOutcome {
        let (entry, parameters) = match prepare(ctx, &args) {
            Ok(prepared) => prepared,
            Err(error) => return error.outcome(),
        };
        match &entry.target {
            Target::AddOn(tool) => {
                if let Err(error) = claim(ctx, &entry) {
                    return error.outcome();
                }
                let rule = rule_for(ctx, &entry);
                ctx.approvals.write_rule = rule;
                ctx.approvals.mode = rule.mode;
                run_add_on(tool.as_ref(), &entry, ctx, parameters, None)
            }
            Target::Service(exposure) => {
                let exposure = exposure.clone();
                let outcome = run_service(ctx, &entry, &exposure, &parameters);
                with_service_lessons(ctx, &entry, outcome)
            }
        }
    }

    fn resume(&self, ctx: &mut TurnContext<'_>, args: Json, input: &ToolInput) -> ToolOutcome {
        let facade = match facade() {
            Ok(facade) => facade,
            Err(error) => return error.outcome(),
        };
        let name = args.get("name").and_then(Json::as_str).unwrap_or_default().to_string();
        let catalog = catalog_of(ctx, &facade);
        if let Some(Entry {
            target: Target::AddOn(tool),
            ..
        }) = catalog.get(&name)
        {
            let entry = catalog.get(&name).cloned().expect("found above");
            let rule = rule_for(ctx, &entry);
            ctx.approvals.write_rule = rule;
            ctx.approvals.mode = rule.mode;
            let parameters = args.get("parameters").cloned().unwrap_or_else(|| json!({}));
            return run_add_on(tool.as_ref(), &entry, ctx, parameters, Some(input));
        }
        let view = match approval::for_call(ctx) {
            Ok(Some(view)) => view,
            Ok(None) => return ToolOutcome::Failed("this call has no approval to go on with".to_string()),
            Err(error) => return ToolOutcome::Failed(format!("the approval could not be read: {error}")),
        };
        match approval::resumed(&view) {
            Ok(Resumed::Approved) => {
                let entry = catalog
                    .get(&name)
                    .filter(|entry| entry.service_name() == Some(view.target_name.as_str()))
                    .or_else(|| catalog.get(&view.target_name))
                    .cloned();
                let Some(entry) = entry else {
                    approval::conflict(ctx, &view.approval_id, &json!({"exposure": "removed"}));
                    return ToolOutcome::text(format!(
                        "The user approved {}, but it is no longer in the tool catalog, so nothing ran.",
                        view.target_name
                    ));
                };
                let rule = rule_for(ctx, &entry);
                ctx.approvals.write_rule = rule;
                ctx.approvals.mode = rule.mode;
                let outcome = execute(ctx, &facade, &entry, &view);
                with_service_lessons(ctx, &entry, outcome)
            }
            Ok(Resumed::Waiting(pause)) => ToolOutcome::Pause(pause),
            Ok(Resumed::Done(text) | Resumed::HandedOff(text)) => ToolOutcome::text(text),
            Err(error) => ToolOutcome::Failed(format!("the approval could not be read: {error}")),
        }
    }
}

/// The agent's policy rule for a call of the catalog entry in its class.
fn rule_for(ctx: &TurnContext<'_>, entry: &Entry) -> super::ToolRule {
    let policy = facade()
        .ok()
        .and_then(|facade| crate::llm::run::quietly(|| ToolPolicy::load(&facade, &ctx.agent.agent_id)).ok())
        .unwrap_or_default();
    policy.rule_as(&entry.name, entry.classification)
}

/// Count this call against the entry's calls-per-run limit (the policy's `callsPerRun` for the entry's name; `run_tool`
/// itself counts against the run's tool calls), and note the entry on the call's row so later calls count it. A call of
/// no run is not counted.
fn claim(ctx: &TurnContext<'_>, entry: &Entry) -> Result<(), ToolError> {
    let Some(run) = ctx.run else {
        return Ok(());
    };
    let facade = facade()?;
    let limit = rule_for(ctx, entry).calls_per_run;
    let node = ctx.events.node().to_string();
    let tool_call_id = ctx.call.tool_call_id.clone();
    // run bookkeeping the runtime writes under the run's lease, so a read tool's fence lifts for it
    let _lifted = fence::lift();
    let claimed = crate::llm::run::quietly(|| {
        crate::llm::run::in_tx(&facade, || {
            crate::llm::run::held(&facade, &run.run_id, &node, crate::llm::run::Hold::Lease)?;
            if let Some(limit) = limit {
                let before = facade
                    .find(TOOL_CALL)?
                    .eq("runId", Value::string(&run.run_id))
                    .eq("catalogToolName", Value::string(&entry.name))
                    .disable_authz()
                    .list()?
                    .values()
                    .iter()
                    .filter(|row| crate::llm::text_of(row, "toolCallId").as_deref() != Some(tool_call_id.as_str()))
                    .count();
                if before >= usize::try_from(limit).unwrap_or(usize::MAX) {
                    return Ok(false);
                }
            }
            facade
                .find(TOOL_CALL)?
                .eq("toolCallId", Value::string(&tool_call_id))
                .disable_authz()
                .update_all([("catalogToolName".to_string(), Value::string(&entry.name))])?;
            Ok(true)
        })
    });
    match claimed {
        Ok(true) => Ok(()),
        Ok(false) => Err(ToolError::new(
            "limit",
            format!(
                "{} may be run {} times in a run, and it was. Do the rest another way or tell the user.",
                entry.name,
                limit.unwrap_or_default()
            ),
        )),
        Err(error) => {
            tracing::error!("the call of {} could not be counted: {error}", entry.name);
            Err(ToolError::new(
                "unavailable",
                format!("{} could not be run now, so nothing ran.", entry.name),
            ))
        }
    }
}

/// The entry `args` name and its parameters as an object, or the error the model reads: no name, not in the catalog,
/// not one this agent may run.
fn prepare(ctx: &TurnContext<'_>, args: &Json) -> Result<(Entry, Json), ToolError> {
    let facade = facade()?;
    let name = args
        .get("name")
        .and_then(Json::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| ToolError::new("no_name", "Call run_tool with name, a tool name from find_tools."))?;
    let parameters = match args.get("parameters") {
        None | Some(Json::Null) => json!({}),
        Some(Json::Object(map)) => Json::Object(map.clone()),
        Some(Json::String(text)) => match serde_json::from_str::<Json>(text) {
            Ok(Json::Object(map)) => Json::Object(map),
            _ => {
                return Err(ToolError::new(
                    "arguments",
                    "parameters must be a JSON object of parameter names and values.",
                ))
            }
        },
        Some(_) => {
            return Err(ToolError::new(
                "arguments",
                "parameters must be a JSON object of parameter names and values.",
            ))
        }
    };
    let catalog = catalog_of(ctx, &facade);
    let Some(entry) = catalog.get(name) else {
        return Err(unknown(name, &catalog));
    };
    if !entry.allowed {
        let why = entry.refusal.clone().unwrap_or_default();
        return Err(ToolError::new(
            "not_allowed",
            format!(
                "{} cannot be run here: {why}. Do the job another way or tell the user.",
                entry.name
            ),
        ));
    }
    Ok((entry.clone(), parameters))
}

/// Call an add-on tool of the catalog as itself: a READ under a fence of its own, a write with the fence lifted (it asks
/// for its own approvals).
fn run_add_on(
    tool: &dyn AgentTool,
    entry: &Entry,
    ctx: &mut TurnContext<'_>,
    parameters: Json,
    input: Option<&ToolInput>,
) -> ToolOutcome {
    let repaired = super::repair::repair(&parameters, &tool.spec().parameters);
    if let Err(problem) = super::schema::check_arguments(&repaired.args, &tool.spec().parameters) {
        return ToolError::new(
            "arguments",
            format!(
                "The parameters of {} do not fit: {problem}. describe_tools shows them.",
                entry.name
            ),
        )
        .ignored(repaired.ignored)
        .outcome();
    }
    let call = |ctx: &mut TurnContext<'_>| match input {
        Some(input) => tool.resume(ctx, repaired.args.clone(), input),
        None => tool.call(ctx, repaired.args.clone()),
    };
    if entry.classification.is_read() {
        let guard = fence::enter(format!("tool {}", entry.name));
        let outcome = call(ctx);
        if let Some(refused) = fenced_refusal(&entry.name, &guard) {
            return refused;
        }
        outcome
    } else {
        let _lifted = fence::lift();
        call(ctx)
    }
}

/// The failure of a READ that tried to write under its fence, if it did.
fn fenced_refusal(name: &str, guard: &fence::FenceGuard) -> Option<ToolOutcome> {
    let violations = guard.violations();
    if violations.is_empty() {
        return None;
    }
    let tried: Vec<String> = violations.iter().map(fence::Violation::describe).collect();
    tracing::error!("catalog tool {name} is a read but tried {}", tried.join(", "));
    Some(
        ToolError::new(
            "read_tool_wrote",
            format!(
                "{name} is offered as a read, but it tried {}, which a read may not do; nothing was changed. Tell the \
                 user; an admin must classify it as a write.",
                tried.join(", ")
            ),
        )
        .outcome(),
    )
}

/// A service call checked before it runs: its parameters cleaned, and what was repaired, ignored and resolved.
struct Checked {
    service: ToolService,
    cleaned: BTreeMap<String, Value>,
    /// The record an entity-auto update or delete names, as it is now.
    record: Option<catalog::EntityRecord>,
    ignored: Vec<String>,
    resolved: BTreeMap<String, String>,
}

/// The precheck of a service call: the parameters repaired against the tool's schema, date phrases resolved in the
/// user's time zone, then validated and converted as the service does; the user's permission.
fn check_service(
    ctx: &TurnContext<'_>,
    entry: &Entry,
    exposure: &catalog::Exposure,
    parameters: &Json,
) -> Result<Checked, ToolError> {
    let service = ToolService::of(&exposure.service_name).map_err(|error| {
        ToolError::new(
            "unavailable",
            format!("{} cannot run now: {error}. Tell the user.", entry.name),
        )
    })?;
    let facade = facade()?;
    let schema = entry.schema(&facade).unwrap_or(Json::Null);
    let repaired = super::repair::repair(parameters, &schema);
    let mut args = repaired.args;
    let resolved = resolve_dates(ctx, &mut args, &schema);
    super::schema::check_arguments(&args, &schema).map_err(|problem| {
        ToolError::new(
            "arguments",
            format!(
                "The parameters of {} do not fit: {problem}. Fix them and call run_tool again; describe_tools shows them.",
                entry.name
            ),
        )
        .ignored(repaired.ignored.clone())
    })?;
    let input: BTreeMap<String, Value> = match moqui_lib::json::from_json(&args) {
        Value::Map(map) => map.into_iter().collect(),
        _ => BTreeMap::new(),
    };
    let cleaned = service.clean(&input).map_err(|error| {
        ToolError::new(
            "arguments",
            format!(
                "The parameters of {} are not valid: {}. Fix them and call run_tool again; describe_tools shows them.",
                entry.name,
                super::format::cut(error.message(), 600)
            ),
        )
        .ignored(repaired.ignored.clone())
    })?;
    // an entity-auto write runs on the one record its whole key names, which an update or a delete must find
    let record = entity_target(&facade, &service, &entry.name, &cleaned)?;
    {
        if let Err(error) = catalog::user_permitted(&service) {
            return Err(ToolError::new(
                "not_allowed",
                format!(
                    "The user is not allowed to run {}: {}. That is the answer about permission; do not try another way.",
                    entry.name,
                    super::format::cut(error.message(), 300)
                ),
            ));
        }
    }
    Ok(Checked {
        service,
        cleaned,
        record,
        ignored: repaired.ignored,
        resolved,
    })
}

/// The record an entity-auto write names: its key whole, with no wildcard or pattern in it (a delete with `*` is a
/// delete of every match, which a card for one record must never stand for), and for an update or a delete the record
/// there now. `None` for a service that is not entity-auto, and for a create or store of a record not there yet.
fn entity_target(
    facade: &EntityFacade,
    service: &ToolService,
    name: &str,
    cleaned: &BTreeMap<String, Value>,
) -> Result<Option<catalog::EntityRecord>, ToolError> {
    let Some(entity) = service.entity_noun() else {
        return Ok(None);
    };
    let pk = crate::llm::run::quietly(|| catalog::entity_pk(facade, &entity))
        .map_err(|error| ToolError::new("unavailable", format!("{name} cannot run now: {error}. Tell the user.")))?;
    let one_record = matches!(service.verb(), "update" | "delete");
    let mut missing = Vec::new();
    for field in &pk {
        match cleaned.get(field).filter(|value| !moqui_lib::object::is_empty(value)) {
            None if one_record => missing.push(field.as_str()),
            None => {}
            Some(value) => {
                let text = moqui_lib::object::to_plain_string(value);
                if text.contains('*') || text.contains('%') {
                    return Err(ToolError::new(
                        "arguments",
                        format!(
                            "{name} works on one record: {field} must be its id, not a pattern ({text}). Find the \
                             records and run it for each, one at a time."
                        ),
                    ));
                }
            }
        }
    }
    if !missing.is_empty() {
        return Err(ToolError::new(
            "arguments",
            format!(
                "{name} needs the key of the record: {}. Find the record first (search, or a list on a screen), then \
                 call run_tool again with it.",
                missing.join(", ")
            ),
        ));
    }
    let record = crate::llm::run::quietly(|| catalog::entity_record(facade, &entity, cleaned)).map_err(|error| {
        ToolError::new(
            "unavailable",
            format!("the record of {name} could not be read: {error}"),
        )
    })?;
    if record.is_none() && one_record {
        let key: Vec<String> = pk
            .iter()
            .filter_map(|field| {
                cleaned
                    .get(field)
                    .map(|value| format!("{field} {}", moqui_lib::object::to_plain_string(value)))
            })
            .collect();
        return Err(ToolError::new(
            "not_found",
            format!(
                "There is no record with {} for {name}, so nothing was asked. Find the record first.",
                key.join(", ")
            ),
        ));
    }
    Ok(record)
}

/// Date phrases (`today`, `start of last week`) in a date parameter (by its format or its name), resolved in the user's
/// time zone; what was used, by name.
fn resolve_dates(ctx: &TurnContext<'_>, args: &mut Json, schema: &Json) -> BTreeMap<String, String> {
    let mut resolved = BTreeMap::new();
    let Some(map) = args.as_object_mut() else {
        return resolved;
    };
    let properties = schema.get("properties").and_then(Json::as_object);
    let mut now = None;
    for (name, value) in map.iter_mut() {
        let Some(text) = value.as_str() else {
            continue;
        };
        let format = properties
            .and_then(|properties| properties.get(name))
            .and_then(|property| property.get("format"))
            .and_then(Json::as_str)
            .unwrap_or_default();
        if !(format.starts_with("date") || super::dates::is_date_name(name)) {
            continue;
        }
        let now = now.get_or_insert_with(|| super::data::user_now(ctx));
        let end = super::dates::is_end_name(name);
        if let Some(date) = super::dates::resolve(text, now, jiff::civil::Weekday::Monday, end) {
            let date = if format == "date" {
                date.split(' ').next().unwrap_or(&date).to_string()
            } else {
                date
            };
            resolved.insert(name.clone(), date.clone());
            *value = Json::String(date);
        }
    }
    resolved
}

/// The cleaned parameters as the approval's arguments: JSON, nulls left out.
fn args_json(cleaned: &BTreeMap<String, Value>) -> Json {
    Json::Object(
        cleaned
            .iter()
            .filter(|(_, value)| !value.is_null())
            .map(|(name, value)| (name.clone(), moqui_lib::json::to_json(value)))
            .collect(),
    )
}

/// Check and run a service tool: a READ at once and fenced, a write through its approval.
fn run_service(
    ctx: &mut TurnContext<'_>,
    entry: &Entry,
    exposure: &catalog::Exposure,
    parameters: &Json,
) -> ToolOutcome {
    let checked = match check_service(ctx, entry, exposure, parameters) {
        Ok(checked) => checked,
        Err(error) => return error.outcome(),
    };
    if let Err(error) = claim(ctx, entry) {
        return error.outcome();
    }
    if entry.classification.is_read() {
        let guard = fence::enter(format!("tool {}", entry.name));
        let ran = call_service(&checked.service, &checked.cleaned);
        if let Some(refused) = fenced_refusal(&entry.name, &guard) {
            return refused;
        }
        drop(guard);
        return answer(entry, ran, &checked, None);
    }
    let rule = rule_for(ctx, entry);
    ctx.approvals.write_rule = rule;
    ctx.approvals.mode = rule.mode;
    let facade = match facade() {
        Ok(facade) => facade,
        Err(error) => return error.outcome(),
    };
    let request = approval_request(&facade, entry, exposure, &checked);
    match approval::request(ctx, &request) {
        Requested::Pause(pause) => ToolOutcome::Pause(pause),
        Requested::Refused(why) => ToolError::new("not_allowed", why).outcome(),
        Requested::Approved(grant) => {
            // a background agent's policy approved it: it runs now
            match crate::llm::run::quietly(|| approval::load(&facade, &grant.approval_id))
                .ok()
                .flatten()
            {
                Some(view) => execute(ctx, &facade, entry, &view),
                None => ToolOutcome::Failed("the approval could not be read, so nothing ran".to_string()),
            }
        }
    }
}

/// The approval a service write asks for: the cleaned parameters as its arguments; for an entity-auto service the
/// record's fields current against proposed, and the record with its key and `lastUpdatedStamp` as the snapshot the
/// conflict check compares.
fn approval_request(
    facade: &EntityFacade,
    entry: &Entry,
    exposure: &catalog::Exposure,
    checked: &Checked,
) -> ApprovalRequest {
    let args = args_json(&checked.cleaned);
    let record = checked.record.clone().or_else(|| {
        checked.service.entity_noun().and_then(|entity| {
            crate::llm::run::quietly(|| catalog::entity_record(facade, &entity, &checked.cleaned)).unwrap_or_else(
                |error| {
                    tracing::warn!(
                        "the record of {} could not be read for its approval: {error}",
                        entry.name
                    );
                    None
                },
            )
        })
    });
    let verb = checked.service.verb().to_string();
    let changes: Vec<ChangeRow> = match &record {
        Some(record) if verb == "delete" => record
            .fields
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(name, _)| *name != "lastUpdatedStamp")
            .map(|(name, value)| ChangeRow {
                field: name.clone(),
                label: name.clone(),
                current: plain(value),
                proposed: String::new(),
            })
            .collect(),
        Some(record) => args
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(name, _)| record.key.get(name.as_str()).is_none() && !record.hidden.contains(name))
            .filter_map(|(name, value)| {
                let current = record.fields.get(name).map(plain).unwrap_or_default();
                let proposed = plain(value);
                (current != proposed).then(|| ChangeRow {
                    field: name.clone(),
                    label: name.clone(),
                    current,
                    proposed,
                })
            })
            .collect(),
        None => args
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(name, _)| !super::data::secret_name(name))
            .map(|(name, value)| ChangeRow {
                field: name.clone(),
                label: name.clone(),
                current: String::new(),
                proposed: plain(value),
            })
            .collect(),
    };
    let key_text = record
        .as_ref()
        .and_then(|record| record.key.as_object())
        .map(|key| key.values().map(plain).collect::<Vec<_>>().join(", "))
        .filter(|text| !text.is_empty());
    let label = exposure.title.clone().unwrap_or_else(|| entry.name.clone());
    let summary = match key_text {
        Some(key) => format!("{label}: {verb} {key}"),
        None => label,
    };
    let mut snapshot = record
        .as_ref()
        .map(catalog::EntityRecord::snapshot)
        .unwrap_or_else(|| json!({}));
    snapshot["lockVersion"] = json!(exposure.lock_version);
    snapshot["tool"] = json!(entry.name);
    ApprovalRequest {
        target_kind: TargetKind::Service,
        target_name: checked.service.name().to_string(),
        classification: if entry.classification.is_read() {
            Classification::Write
        } else {
            entry.classification
        },
        args,
        summary,
        changes,
        record_refs: record
            .as_ref()
            .map(|record| json!({"entity": record.entity, "pk": record.key}))
            .unwrap_or(Json::Null),
        snapshot,
        preview: exposure.preview,
        handoff: None,
    }
}

fn plain(value: &Json) -> String {
    match value {
        Json::Null => String::new(),
        Json::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Run an approved service write: the agent and the user still allowed to run it (the policy, the role group and the
/// user's permission may have changed while the approval waited), the exposure and the record compared with the snapshot
/// (a change of either is a conflict and nothing runs), the hash checked as the approval begins, the arguments checked
/// again exactly as they were hashed (an edit replaced them: they must already be what the checks make of them, and name
/// the same record), then the service unfenced as the user, and its result recorded on the approval.
fn execute(
    ctx: &mut TurnContext<'_>,
    facade: &EntityFacade,
    entry: &Entry,
    view: &approval::ApprovalView,
) -> ToolOutcome {
    let Some(exposure) = entry.exposure().cloned() else {
        return ToolOutcome::Failed("this approval is not of a service".to_string());
    };
    let snapshot = view.snapshot.get("record").cloned().unwrap_or_else(|| json!({}));
    // the exposure was edited after the approval was asked: what the user approved may not be what it is now
    if snapshot
        .get("lockVersion")
        .and_then(Json::as_i64)
        .is_some_and(|asked| asked != exposure.lock_version)
    {
        approval::conflict(ctx, &view.approval_id, &json!({"exposure": "changed"}));
        return ToolOutcome::text(format!(
            "The tool {} was changed by an admin after the user approved it, so nothing ran. Describe it again and \
             propose the call anew if it is still wanted.",
            entry.name
        ));
    }
    // the policy or the role group changed while the approval waited, or the user lost the permission
    let refusal = if entry.allowed {
        ToolService::of(&exposure.service_name)
            .ok()
            .and_then(|service| catalog::user_permitted(&service).err())
            .map(|error| {
                format!(
                    "the user may no longer run it ({})",
                    super::format::cut(error.message(), 300)
                )
            })
    } else {
        Some(
            entry
                .refusal
                .clone()
                .unwrap_or_else(|| "this agent may no longer run it".to_string()),
        )
    };
    if let Some(why) = refusal {
        return refuse_approved(ctx, view, &entry.name, &why);
    }
    let edited = view.snapshot.get("edited").and_then(Json::as_bool).unwrap_or(false);
    if let (Some(entity), Some(asked_stamp)) = (
        snapshot.get("entity").and_then(Json::as_str),
        snapshot.get("lastUpdatedStamp").and_then(Json::as_str),
    ) {
        let key: BTreeMap<String, Value> = snapshot
            .get("pk")
            .and_then(Json::as_object)
            .map(|key| {
                key.iter()
                    .map(|(name, value)| (name.clone(), moqui_lib::json::from_json(value)))
                    .collect()
            })
            .unwrap_or_default();
        let now = crate::llm::run::quietly(|| catalog::entity_record(facade, entity, &key))
            .ok()
            .flatten();
        let changed = match &now {
            Some(record) => record.stamp.as_deref() != Some(asked_stamp),
            None => true,
        };
        if changed {
            let conflicts =
                json!({"record": now.as_ref().map(|record| record.fields.clone()), "was": snapshot.get("record")});
            approval::conflict(ctx, &view.approval_id, &conflicts);
            let values = now.as_ref().map_or_else(
                || "the record is gone".to_string(),
                |record| super::format::cut(&record.fields.to_string(), 1200),
            );
            return ToolOutcome::text(format!(
                "The record changed after {} was proposed, so nothing ran. It is now: {values}. Propose the write anew \
                 if it is still wanted.",
                entry.name
            ));
        }
    }
    let grant = match approval::begin(ctx, &view.approval_id, &view.args) {
        Ok(grant) => grant,
        Err(why) => return ToolOutcome::text(why),
    };
    let service = match ToolService::of(&exposure.service_name) {
        Ok(service) => service,
        Err(error) => {
            approval::finish(ctx, &grant.approval_id, false, &json!({"error": error.to_string()}));
            return ToolOutcome::text(format!(
                "The user approved {}, but it cannot run now ({error}), so nothing ran.",
                entry.name
            ));
        }
    };
    // the approved arguments run exactly as hashed: they must fit the tool's schema as they are, and checking them as
    // the service does must change nothing (the proposed ones were checked before they were hashed; an edit must be too)
    let schema = entry.schema(facade).unwrap_or(Json::Null);
    let invalid = |ctx: &mut TurnContext<'_>, problem: String| {
        approval::finish(ctx, &grant.approval_id, false, &json!({"error": problem}));
        ToolError::new(
            "arguments",
            format!(
                "The approved parameters of {} are not valid: {}, so nothing ran. Propose the call again with valid \
                 parameters.",
                entry.name,
                super::format::cut(&problem, 600)
            ),
        )
        .outcome()
    };
    if let Err(problem) = super::schema::check_arguments(&view.args, &schema) {
        return invalid(ctx, problem);
    }
    let input: BTreeMap<String, Value> = match moqui_lib::json::from_json(&view.args) {
        Value::Map(map) => map.into_iter().collect(),
        _ => BTreeMap::new(),
    };
    let cleaned = match service.clean(&input) {
        Ok(cleaned) => cleaned,
        Err(error) => return invalid(ctx, error.message().to_string()),
    };
    if moqui_lib::json::canonical(&args_json(&cleaned)) != moqui_lib::json::canonical(&view.args) {
        return invalid(
            ctx,
            "checking them changes them, so they are not exactly what was approved".to_string(),
        );
    }
    // an edit names the same one record: its key whole, no pattern, the record there, and not another record
    match entity_target(facade, &service, &entry.name, &cleaned) {
        Err(error) => {
            approval::finish(ctx, &grant.approval_id, false, &json!({"error": error.instruction}));
            return error.outcome();
        }
        Ok(target) => {
            let asked = snapshot.get("pk").filter(|key| key.is_object());
            let now = target.as_ref().map(|record| &record.key);
            if asked.is_some() && asked != now {
                return invalid(
                    ctx,
                    "an edit may not change which record it writes; propose the write for the other record anew"
                        .to_string(),
                );
            }
        }
    }
    let ran = {
        let _lifted = fence::lift();
        call_service(&service, &cleaned)
    };
    let result = match &ran {
        Ok(result) => json!({"ok": true, "result": strip_nulls(&result_json(result))}),
        Err(error) => json!({"ok": false, "error": error}),
    };
    approval::finish(ctx, &grant.approval_id, ran.is_ok(), &result);
    let checked = Checked {
        service,
        cleaned,
        record: None,
        ignored: Vec::new(),
        resolved: BTreeMap::new(),
    };
    let mut line = if ran.is_ok() {
        format!("The user approved {}, and it ran.", entry.name)
    } else {
        format!("The user approved {}, but it failed; nothing was saved.", entry.name)
    };
    if edited {
        let proposed = view.snapshot.get("proposedArgs").cloned().unwrap_or(Json::Null);
        let changed: Vec<String> = view
            .args
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(name, value)| proposed.get(name.as_str()) != Some(*value))
            .map(|(name, value)| {
                format!(
                    "{name} {} (you proposed {})",
                    plain(value),
                    proposed
                        .get(name.as_str())
                        .map(plain)
                        .unwrap_or_else(|| "nothing".to_string())
                )
            })
            .collect();
        line.push_str(&format!(" The user edited the parameters: {}.", changed.join(", ")));
    }
    answer(entry, ran, &checked, Some((line, grant.approval_id)))
}

/// An approved call that may not run now: the approval fails with why (through Executing, as a call that began), and
/// nothing runs.
fn refuse_approved(ctx: &mut TurnContext<'_>, view: &approval::ApprovalView, name: &str, why: &str) -> ToolOutcome {
    if let Ok(grant) = approval::begin(ctx, &view.approval_id, &view.args) {
        approval::finish(ctx, &grant.approval_id, false, &json!({"error": why}));
    }
    ToolError::new(
        "not_allowed",
        format!("The user approved {name}, but {why}, so nothing ran. Tell the user."),
    )
    .outcome()
}

/// Run the service now, as the user, with what it reports as errors an error.
fn call_service(service: &ToolService, cleaned: &BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>, String> {
    let mark = ExecutionContext::with_current(|ec| ec.message().error_mark());
    let ran =
        crate::service::call::sync_checked(service.name(), cleaned, &crate::service::call::CallOptions::default());
    // the errors are the result's; the run's later calls do not see them
    if let Some(mark) = mark {
        ExecutionContext::with_current(|ec| ec.message().take_errors_since(mark));
    }
    ran.map_err(|error| error.message().to_string())
}

fn result_json(result: &BTreeMap<String, Value>) -> Json {
    Json::Object(
        result
            .iter()
            .map(|(name, value)| (name.clone(), plain_json(value, 0)))
            .collect(),
    )
}

/// A result value as JSON, the entity values and lists a service hands back as script objects read as maps and lists.
fn plain_json(value: &Value, depth: usize) -> Json {
    if depth > 16 {
        return Json::Null;
    }
    match value {
        Value::Map(map) => Json::Object(
            map.iter()
                .map(|(name, item)| (name.clone(), plain_json(item, depth + 1)))
                .collect(),
        ),
        Value::List(items) => Json::Array(items.iter().map(|item| plain_json(item, depth + 1)).collect()),
        Value::Object(_) => {
            if let Some(map) = crate::script::value_map(value) {
                Json::Object(
                    map.iter()
                        .map(|(name, item)| (name.clone(), plain_json(item, depth + 1)))
                        .collect(),
                )
            } else if let Some(items) = crate::script::value_items(value) {
                Json::Array(items.iter().map(|item| plain_json(item, depth + 1)).collect())
            } else {
                Json::Null
            }
        }
        other => moqui_lib::json::to_json(other),
    }
}

/// A JSON value with its nulls (and the maps and lists they empty) left out.
fn strip_nulls(value: &Json) -> Json {
    match value {
        Json::Object(map) => Json::Object(
            map.iter()
                .filter(|(_, value)| !value.is_null())
                .map(|(name, value)| (name.clone(), strip_nulls(value)))
                .collect::<Map<String, Json>>(),
        ),
        Json::Array(items) => Json::Array(items.iter().filter(|item| !item.is_null()).map(strip_nulls).collect()),
        other => other.clone(),
    }
}

/// What the model reads of a call that ran: `{ok, tool, result}` or `{ok: false, error}`, with what was ignored and the
/// dates resolved; after an approval its line comes first.
fn answer(
    entry: &Entry,
    ran: Result<BTreeMap<String, Value>, String>,
    checked: &Checked,
    approved: Option<(String, String)>,
) -> ToolOutcome {
    let mut body = Map::new();
    body.insert("ok".to_string(), json!(ran.is_ok()));
    body.insert("tool".to_string(), json!(entry.name));
    match &ran {
        Ok(result) => {
            body.insert("result".to_string(), strip_nulls(&result_json(result)));
        }
        Err(error) => {
            body.insert("error".to_string(), json!(super::format::cut(error, 1200)));
        }
    }
    if !checked.ignored.is_empty() {
        body.insert(
            "ignored".to_string(),
            json!(checked
                .ignored
                .iter()
                .map(|name| format!("{name} (not a parameter of {})", entry.name))
                .collect::<Vec<_>>()),
        );
    }
    if !checked.resolved.is_empty() {
        body.insert("resolved".to_string(), json!(checked.resolved));
    }
    let mut data = json!({"ok": ran.is_ok(), "tool": entry.name, "service": checked.service.name()});
    let text = match approved {
        Some((line, approval_id)) => {
            body.insert(
                "approval".to_string(),
                json!(if ran.is_ok() { "executed" } else { "failed" }),
            );
            data["approvalId"] = json!(approval_id);
            format!("{line}\n{}", Json::Object(body))
        }
        None => Json::Object(body).to_string(),
    };
    if ran.is_err() && !text.starts_with("The user") {
        return ToolOutcome::Failed(text);
    }
    ToolOutcome::Done(ToolResult { text, data: Some(data) })
}

crate::submit_tool!(FindTools);
crate::submit_tool!(DescribeTools);
crate::submit_tool!(RunTool);
