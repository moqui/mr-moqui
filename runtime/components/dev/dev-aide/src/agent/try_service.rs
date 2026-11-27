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

//! `try_service` (plans/archive/design-dev-aide.md 9.4 and 13.2): run a service in the sim overlay (the default) as the user,
//! and answer its out-parameters, messages and errors and the rows it would write, which a spec's `checkData` can be
//! written from; nothing is kept. A real run is a write the user approves, as any service call of an agent. In the sim
//! it may run as a proposal defines it (F14 for services, 13.3's "a Try on proposed text"): a change set of the
//! user's (`changeSet`, its approval id) or files as `propose_edit` takes them (`files`), their service and script
//! files standing at their locations for the Try's own calls only (`crate::try_overlay`).

use std::collections::BTreeMap;

use moqui_core::context::ExecutionContext;
use moqui_lib::object::Value;
use moqui_llm::approval::{self, ApprovalRequest, PreviewMode, Requested, Resumed, TargetKind};
use moqui_llm::run::context::TurnContext;
use moqui_llm::tools::format::ToolError;
use moqui_llm::tools::schema::{object, Property};
use moqui_llm::tools::{caps, AgentTool, Classification, ToolInput, ToolOutcome, ToolSpec};
use serde::Deserialize;
use serde_json::{json, Value as Json};

use super::{arg, session_context};
use crate::tier::Tier;

pub struct TryServiceTool(ToolSpec);

impl Default for TryServiceTool {
    fn default() -> Self {
        let mut spec = ToolSpec::new(
            "try_service",
            "Run a Moqui service in the sim (a copy of the database that is thrown away), as the user: its \
             out-parameters, messages and errors, and the rows it would create, update or delete. real=true runs it \
             for real after the user approves. changeSet or files run it as proposed text defines it, in the sim.",
            object(&[
                Property::required(
                    "service",
                    "string",
                    "The service name (a.B.verb#Noun) or an entity-auto name",
                ),
                Property::optional("parameters", "object", "Its in-parameters"),
                Property::optional(
                    "real",
                    "boolean",
                    "Run it for real (the user approves); the sim when left out",
                ),
                Property::optional(
                    "changeSet",
                    "string",
                    "The approval id of a change set you proposed: run it as the proposal defines it (sim only)",
                ),
                Property::optional(
                    "files",
                    "array",
                    "Proposed files as propose_edit takes them ([{file, ops?, edits?, text?}]): run the service as they \
                     define it, without proposing them (sim only)",
                ),
            ]),
            Classification::Read,
        )
        .needs(&[caps::DEV]);
        spec.may_pause = true;
        spec.lazy = true;
        Self(spec)
    }
}

impl AgentTool for TryServiceTool {
    fn spec(&self) -> &ToolSpec {
        &self.0
    }

    fn call(&self, ctx: &mut TurnContext<'_>, args: Json) -> ToolOutcome {
        match run(ctx, &args) {
            Ok(outcome) => outcome,
            Err(error) => error.outcome(),
        }
    }

    fn resume(&self, ctx: &mut TurnContext<'_>, _args: Json, _input: &ToolInput) -> ToolOutcome {
        let view = match approval::for_call(ctx) {
            Ok(Some(view)) => view,
            Ok(None) => return ToolOutcome::Failed("the run's approval is not there".into()),
            Err(error) => return ToolOutcome::Failed(error.to_string()),
        };
        match approval::resumed(&view) {
            Ok(Resumed::Approved) => real(ctx, &view.approval_id, &view.target_name, &view.args),
            Ok(Resumed::Done(text) | Resumed::HandedOff(text)) => ToolOutcome::text(text),
            Ok(Resumed::Waiting(pause)) => ToolOutcome::Pause(pause),
            Err(error) => ToolOutcome::Failed(error.to_string()),
        }
    }
}

/// Why `service` is not tried in the sim, if it is not: a service of the Dev AIDE's own that is not read-only, or any
/// of the Terminal's. Those write files, run git, call a group server or start programs, none of which goes into the
/// sim's overlay; the fence refuses them as well (`workspace::unfenced`), and this says so before anything runs.
#[must_use]
pub fn sim_refusal(service: &str) -> Option<String> {
    let info = moqui_core::service::info::service_detail(service);
    let text = |key: &str| {
        info.as_ref()
            .and_then(|info| info.get(key))
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default()
    };
    let read_only = matches!(
        info.as_ref().and_then(|info| info.get("readOnly")),
        Some(Value::Bool(true))
    );
    // the Terminal's services start programs on the host, which nothing undoes: none runs from a sim, read-only or not
    if service.starts_with("org.moqui.terminal.") || text("location").starts_with("moqui_host::term::") {
        return Some(format!(
            "{service} is a Terminal service that starts programs on the host: the sim cannot hold that"
        ));
    }
    // the framework's development tools that write the host with the file system or restart it (save#ConfFile,
    // remove#Component, apply#XmlOps, restart#Server), which no fence holds: only their reads run in a sim (SEC-DEV-81)
    let verb = service
        .rsplit('.')
        .next()
        .and_then(|name| name.split('#').next())
        .unwrap_or_default();
    let reads = matches!(verb, "get" | "find" | "check" | "preview" | "scan");
    if text("location").starts_with("moqui_devkit::") && !read_only && !reads {
        return Some(format!(
            "{service} is a development tool that writes the server's files or restarts it: the sim cannot hold that"
        ));
    }
    // one not found is judged by its name: never let through unread
    let own = service.starts_with("org.moqui.devaide.") || text("location").starts_with("dev_aide::");
    (own && !read_only).then(|| {
        format!(
            "{service} is a Dev AIDE service that writes files, runs git or calls a server: the sim cannot hold that"
        )
    })
}

fn parameters_of(args: &Json) -> BTreeMap<String, Value> {
    match moqui_lib::json::from_json(args.get("parameters").unwrap_or(&Json::Null)) {
        Value::Map(map) => map
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect(),
        _ => BTreeMap::new(),
    }
}

/// The messages and errors of the entered context, cleared.
fn messages() -> (Vec<String>, Vec<String>) {
    ExecutionContext::with_current(|ec| {
        let message = ec.message();
        (message.messages().to_vec(), message.errors().to_vec())
    })
    .unwrap_or_default()
}

/// The overlay of the proposed text a Try names: a change set of the user's (`changeSet`) and files as `propose_edit`
/// takes them (`files`).
fn proposed(
    context: &crate::workspace::Context,
    args: &Json,
) -> Result<Option<std::sync::Arc<moqui_core::service::overlay::ServiceOverlay>>, ToolError> {
    let mut files = Vec::new();
    if let Some(id) = arg(args, "changeSet").filter(|id| !id.trim().is_empty()) {
        files.extend(
            crate::try_overlay::change_set_files(context, id.trim())
                .map_err(|err| ToolError::new("no_change_set", err.to_string()))?,
        );
    }
    if let Some(given) = args.get("files").filter(|files| !files.is_null()) {
        let set = crate::agent::propose::ChangeSet::deserialize(&json!({ "files": given })).map_err(|err| {
            ToolError::new("bad_arguments", format!("The files do not read: {err}"))
                .hint("files is a list of {file, ops | edits | text}, as propose_edit takes it.")
        })?;
        files.extend(
            crate::try_overlay::proposed_files(context, &set)
                .map_err(|err| ToolError::new("does_not_apply", err.to_string()))?,
        );
    }
    crate::try_overlay::overlay_of(&files).map_err(|err| ToolError::new("does_not_parse", err.to_string()))
}

fn run(ctx: &mut TurnContext<'_>, args: &Json) -> Result<ToolOutcome, ToolError> {
    let service = arg(args, "service")
        .ok_or_else(|| ToolError::new("no_service", "Name the service to run."))?
        .to_string();
    // a session with worktrees reads its proposal's files there (21.8)
    let context = session_context(ctx, Tier::View)?;
    let asks_proposal = arg(args, "changeSet").is_some_and(|id| !id.trim().is_empty())
        || args.get("files").is_some_and(|files| !files.is_null());
    // proposed text is code run in the server: the extend tier, as a snippet (SEC-DEV-02)
    if asks_proposal && context.tier == Tier::View {
        return Err(ToolError::new(
            "not_allowed",
            "Trying proposed text needs the extend tier of Dev AIDE.",
        ));
    }
    let overlay = proposed(&context, args)?;
    let defined_here = overlay.as_ref().is_some_and(|overlay| overlay.defines(&service));
    if !defined_here && !moqui_core::service::is_service_defined(&service, Some(&context.facade)) {
        return Err(ToolError::new("not_found", format!("No service is named {service}."))
            .hint("Find it with the artifact tool: action find, kinds service."));
    }
    let parameters = parameters_of(args);
    let real_run = args.get("real").and_then(Json::as_bool).unwrap_or(false);
    if real_run && overlay.is_some() {
        return Err(ToolError::new(
            "not_in_sim",
            "Proposed text runs in the sim only; a real run runs the files as they are.",
        )
        .hint("Leave real out to try the proposal, or leave changeSet and files out to run the saved files."));
    }
    if !real_run {
        // judged as the proposal defines it, so a proposed definition cannot carry a refused one past the check
        if let Some(why) = crate::try_overlay::over(overlay.clone(), || sim_refusal(&service)) {
            return Err(ToolError::new("not_in_sim", why).hint("Run it with real=true, which the user approves."));
        }
    }
    if real_run {
        if context.tier == Tier::View {
            return Err(ToolError::new(
                "not_allowed",
                "Running a service for real needs the extend tier of Dev AIDE.",
            ));
        }
        let request = ApprovalRequest {
            target_kind: TargetKind::Service,
            target_name: service.clone(),
            classification: Classification::Write,
            args: args.get("parameters").cloned().unwrap_or_else(|| json!({})),
            summary: format!("Run {service} for real"),
            preview: PreviewMode::OnDemand,
            ..ApprovalRequest::default()
        };
        return Ok(match approval::request(ctx, &request) {
            Requested::Pause(pause) => ToolOutcome::Pause(pause),
            Requested::Approved(grant) => real(ctx, &grant.approval_id, &service, &request.args),
            Requested::Refused(why) => ToolOutcome::Failed(why),
        });
    }
    if !moqui_llm::approval::preview::available(&context.facade) {
        return Err(ToolError::new(
            "no_sim",
            "This server has no sim (it needs embedded Postgres); run it with real=true, which the user approves.",
        ));
    }
    let started = std::time::Instant::now();
    let label = format!("Try in sim of {service}");
    let shown = overlay.as_deref().map(crate::try_overlay::overlay_json);
    let ran = moqui_llm::approval::preview::run_in_sim(&context.factory, &ctx.user.username, &label, || {
        let out = crate::try_overlay::over(overlay, || moqui_core::service::sync(&service, &parameters));
        let (messages, errors) = messages();
        // a call that failed answers its own error, not that the sim was rolled back
        match out {
            Err(error) => Err(moqui_lib::error::Error::new(
                format!("{error} {}", errors.join("; ")).trim().to_string(),
            )),
            Ok(_) if !errors.is_empty() => Err(moqui_lib::error::Error::new(errors.join("; "))),
            out => Ok((out, messages, errors)),
        }
    });
    let millis = started.elapsed().as_millis();
    let mut answer = match ran {
        Ok(run) => {
            let (out, messages, errors) = run.value;
            let out = match out {
                Ok(out) => json!(out
                    .iter()
                    .map(|(key, value)| (key.clone(), moqui_lib::json::to_json(value)))
                    .collect::<serde_json::Map<String, Json>>()),
                Err(error) => json!({"error": error.to_string()}),
            };
            json!({"service": service, "sim": true, "out": out, "messages": messages, "errors": errors,
                "rows": run.rows, "moreRows": run.more, "millis": millis})
        }
        Err(error) => json!({"service": service, "sim": true, "failed": error.to_string(), "millis": millis}),
    };
    if let Some(shown) = shown {
        answer["overlay"] = shown;
    }
    Ok(super::json(&answer))
}

/// The approved real run.
fn real(ctx: &mut TurnContext<'_>, approval_id: &str, service: &str, parameters: &Json) -> ToolOutcome {
    // asked again at the approved run, as propose_edit's accept asks: a user who lost the extend tier, or an instance no
    // longer dev, since the approval was asked gets no real run (SEC-DEV-85)
    if let Err(error) = super::context(crate::tier::Tier::Extend) {
        return error.outcome();
    }
    if let Err(why) = approval::begin(ctx, approval_id, parameters) {
        return ToolOutcome::text(why);
    }
    let _lifted = moqui_core::entity::fence::lift();
    let parameters = parameters_of(&json!({"parameters": parameters}));
    let out = moqui_core::service::sync(service, &parameters);
    let (messages, errors) = messages();
    let ok = out.is_ok() && errors.is_empty();
    let result = match &out {
        Ok(out) => json!({"out": out.iter().map(|(key, value)| (key.clone(), moqui_lib::json::to_json(value)))
            .collect::<serde_json::Map<String, Json>>(), "messages": messages, "errors": errors}),
        Err(error) => json!({"error": error.to_string(), "errors": errors}),
    };
    approval::finish(ctx, approval_id, ok, &result);
    super::json(&json!({"service": service, "sim": false, "result": result}))
}

moqui_llm::submit_tool!(TryServiceTool);
