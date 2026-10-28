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

//! `try_service` (plans/design-dev-aide.md 9.4 and 13.2): run a service in the sim overlay (the default) as the user,
//! and answer its out-parameters, messages and errors and the rows it would write, which a spec's `checkData` can be
//! written from; nothing is kept. A real run is a write the user approves, as any service call of an agent.

use std::collections::BTreeMap;

use moqui_core::context::ExecutionContext;
use moqui_core::llm::approval::{self, ApprovalRequest, PreviewMode, Requested, Resumed, TargetKind};
use moqui_core::llm::run::context::TurnContext;
use moqui_core::llm::tools::format::ToolError;
use moqui_core::llm::tools::schema::{object, Property};
use moqui_core::llm::tools::{caps, AgentTool, Classification, ToolInput, ToolOutcome, ToolSpec};
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use super::{arg, context};
use crate::tier::Tier;

pub struct TryServiceTool(ToolSpec);

impl Default for TryServiceTool {
    fn default() -> Self {
        let mut spec = ToolSpec::new(
            "try_service",
            "Run a Moqui service in the sim (a copy of the database that is thrown away), as the user: its \
             out-parameters, messages and errors, and the rows it would create, update or delete. real=true runs it \
             for real after the user approves.",
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

fn run(ctx: &mut TurnContext<'_>, args: &Json) -> Result<ToolOutcome, ToolError> {
    let service = arg(args, "service")
        .ok_or_else(|| ToolError::new("no_service", "Name the service to run."))?
        .to_string();
    let context = context(Tier::View)?;
    if !moqui_core::service::is_service_defined(&service, Some(&context.facade)) {
        return Err(ToolError::new("not_found", format!("No service is named {service}."))
            .hint("Find it with the artifact tool: action find, kinds service."));
    }
    let parameters = parameters_of(args);
    if args.get("real").and_then(Json::as_bool).unwrap_or(false) {
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
    if !moqui_core::llm::approval::preview::available(&context.facade) {
        return Err(ToolError::new(
            "no_sim",
            "This server has no sim (it needs embedded Postgres); run it with real=true, which the user approves.",
        ));
    }
    let started = std::time::Instant::now();
    let label = format!("Try in sim of {service}");
    let ran = moqui_core::llm::approval::preview::run_in_sim(&context.factory, &ctx.user.username, &label, || {
        let out = moqui_core::service::sync(&service, &parameters);
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
    Ok(super::json(&match ran {
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
    }))
}

/// The approved real run.
fn real(ctx: &mut TurnContext<'_>, approval_id: &str, service: &str, parameters: &Json) -> ToolOutcome {
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

moqui_core::submit_tool!(TryServiceTool);
