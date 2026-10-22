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

//! `write_ui` (plans/LLM-A2A-Assist.md 7.3, 4.10, D5, D6): show a generated view in the drawer's canvas. It is listed
//! from the start with a small schema and answers `skill_required` until the dialect's skill (`openui-lang`, or
//! `vue-sfc` for an agent that writes Vue SFC canvases) is loaded in the conversation, so the 27.9k-character guide is
//! read only when a canvas is wanted; a kind the agent does not write is refused first. A program (or a `patch` of
//! the current canvas's statements, merged on the server) is parsed and checked before anything is shown
//! ([`crate::llm::ui::Dialect::prepare`]); every problem names its skill section and quotes the rule, and the model
//! fixes it in the next step. A program that checks becomes the conversation's canvas, a new version
//! ([`crate::llm::ui::Canvas`]), stored once: the history keeps what the model sent, never the merged program again.
//! The clients get `canvas.open` (the canvas whole) and `artifact.update`.
//!
//! `run` names a Mutation of the current canvas the agent wants run without the user's click: that is a write the user
//! has not clicked, so it is an approval card (the run pauses); a click on the canvas's own Button runs it with no card
//! (`canvasAction`, [`crate::llm::ui::write`]).

use std::sync::Arc;

use serde_json::{json, Value as Json};

use super::format::ToolError;
use super::schema::{object, Property};
use super::{caps, AgentTool, Classification, ToolInput, ToolOutcome, ToolSpec};
use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::fence;
use crate::llm::approval::{self, ApprovalRequest, PreviewMode, Requested, Resumed};
use crate::llm::run::context::TurnContext;
use crate::llm::run::{self, Hold};
use crate::llm::ui::{self, openui, write::HeldMutation, Canvas, WriteInput};

pub struct WriteUi(ToolSpec);

impl Default for WriteUi {
    fn default() -> Self {
        let mut spec = ToolSpec::new(
            "write_ui",
            "Show a generated view in the drawer: a chart, a dashboard, a batch confirm, several lists. Load skill openui-lang first; it has the language.",
            object(&[
                Property::optional("kind", "string", "openui"),
                Property::optional("title", "string", "Title"),
                Property::optional("program", "string", "Whole program"),
                Property::optional("patch", "string", "Changed statements only"),
                Property::optional("run", "string", "Mutation to run; asks approval"),
            ]),
            Classification::Read,
        )
        .needs_any(&[caps::UI_OPENUI, caps::UI_VUE_SFC]);
        spec.may_pause = true;
        Self(spec)
    }
}

/// The error of a call before its skill is loaded (plan 7.3).
pub fn skill_required(skill: &str) -> String {
    json!({"error": "skill_required", "skill": skill,
        "instruction": format!("write_ui needs the {skill} skill. Call skill with name \"{skill}\", then call write_ui again.")})
    .to_string()
}

/// The server's checks of a program's paths: a Query's and a Link's must be screens of the apps.
struct ScreenPaths<'a>(&'a ExecutionContextFactory);

impl openui::Paths for ScreenPaths<'_> {
    fn screen(&self, path: &str) -> Option<Result<(), String>> {
        let path = super::apps::app_path(path);
        Some(crate::llm::screen::check_screen_path(self.0, &path))
    }
}

fn factory() -> Option<Arc<ExecutionContextFactory>> {
    ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
}

impl AgentTool for WriteUi {
    fn spec(&self) -> &ToolSpec {
        &self.0
    }

    fn call(&self, ctx: &mut TurnContext<'_>, args: Json) -> ToolOutcome {
        let text = |key: &str| args.get(key).and_then(Json::as_str).map(str::to_string).filter(|text| !text.trim().is_empty());
        let kind = text("kind").unwrap_or_else(|| "openui".to_string());
        let Some(dialect) = ui::dialect(&kind) else {
            return ToolError::new("unknown_kind", format!("There is no canvas kind {kind}. Use kind openui."))
                .did_you_mean(ui::dialects().iter().filter(|dialect| dialect.available()).map(|dialect| dialect.name().to_string()).collect())
                .outcome();
        };
        // a kind this server, this drawer or this agent does not offer is answered before its skill is asked for
        if !dialect.available() || !ctx.caps.has(&format!("ui.write:{}", dialect.name())) {
            let instead = if dialect.name() != "openui" && ctx.caps.has(caps::UI_OPENUI) {
                "Write the view with kind openui (skill openui-lang)."
            } else {
                "Answer in text."
            };
            return ToolError::new(
                "dialect_unavailable",
                format!("This agent does not show {kind} canvases here, so nothing was shown. {instead}"),
            )
            .outcome();
        }
        if crate::llm::skills::loaded(ctx, dialect.skill()).is_none() {
            return ToolOutcome::Failed(skill_required(dialect.skill()));
        }
        let Some(factory) = factory() else {
            return ToolOutcome::Failed("write_ui runs only in a run's context".to_string());
        };
        let Some(run) = ctx.run.cloned() else {
            return ToolError::new("no_conversation", "A canvas belongs to a conversation, and this call has none; answer in text.").outcome();
        };
        let Some(facade) = factory.entity() else {
            return ToolOutcome::Failed("the canvas cannot be stored here".to_string());
        };
        let current = match run::quietly(|| Canvas::of_conversation(&facade, &run.conversation_id)) {
            Ok(current) => current,
            Err(error) => {
                tracing::error!("the canvas of conversation {} could not be read: {error}", run.conversation_id);
                return ToolOutcome::Failed("the canvas could not be read now".to_string());
            }
        };
        if let Some(statement) = text("run") {
            return propose_run(ctx, &factory, current.as_ref(), &statement);
        }
        let program = text("program");
        let patch = text("patch");
        if program.is_none() && patch.is_none() {
            return ToolError::new(
                "no_program",
                "Call write_ui with program (the whole program) or patch (only the changed statements of the canvas).",
            )
            .outcome();
        }
        let previous = current.as_ref().filter(|canvas| canvas.dialect == dialect.name());
        if patch.is_some() && program.is_none() && previous.is_none() {
            return ToolError::new(
                "no_canvas",
                "There is no canvas to patch yet, so nothing was shown. Send the whole program with program.",
            )
            .outcome();
        }
        let title = text("title")
            .or_else(|| previous.map(|canvas| canvas.title.clone()))
            .unwrap_or_else(|| "Canvas".to_string());
        let input = WriteInput {
            title: title.clone(),
            program: program.clone(),
            patch: if program.is_some() { None } else { patch.clone() },
        };
        let prepared = match dialect.prepare(&factory, previous.map(|canvas| canvas.program.as_str()), &input, &ScreenPaths(&factory)) {
            Ok(prepared) => prepared,
            Err(refused) => return ToolOutcome::Failed(refused.error),
        };
        // the canvas is the run's own record, written under its lease; the read fence lifts for it as for an approval
        let node = ctx.events.node().to_string();
        let stored = {
            let _lifted = fence::lift();
            run::quietly(|| {
                run::in_tx(&facade, || {
                    run::held(&facade, &run.run_id, &node, Hold::Lease)?;
                    Canvas::write_version(&facade, current.as_ref(), &run.conversation_id, &run.run_id, dialect.name(), &title, &prepared)
                })
            })
        };
        let canvas = match stored {
            Ok(canvas) => canvas,
            Err(error) if error.to_string().contains(ui::CONFLICT) => {
                return ToolError::new(
                    "canvas_changed",
                    "The canvas changed while this was written, so nothing was shown. Send it again.",
                )
                .outcome();
            }
            Err(error) => {
                tracing::error!("the canvas of run {} could not be stored: {error}", run.run_id);
                return ToolOutcome::Failed("the canvas could not be stored, so nothing was shown".to_string());
            }
        };
        ctx.events.emit("canvas.open", canvas.to_json());
        ctx.events.emit(
            "artifact.update",
            json!({"artifactId": canvas.artifact_id, "kind": "canvas", "name": canvas.title, "version": canvas.version,
                "dialect": canvas.dialect}),
        );
        ToolOutcome::text(shown_text(&canvas, &prepared, patch.is_some() && program.is_none()))
    }

    fn resume(&self, ctx: &mut TurnContext<'_>, _args: Json, _input: &ToolInput) -> ToolOutcome {
        let view = match approval::for_call(ctx) {
            Ok(Some(view)) => view,
            Ok(None) => return ToolOutcome::Failed("this call has no approval to go on with".to_string()),
            Err(error) => return ToolOutcome::Failed(format!("the approval could not be read: {error}")),
        };
        match approval::resumed(&view) {
            Ok(Resumed::Approved) => {}
            Ok(Resumed::Waiting(pause)) => return ToolOutcome::Pause(pause),
            Ok(Resumed::HandedOff(text) | Resumed::Done(text)) => return ToolOutcome::text(text),
            Err(error) => return ToolOutcome::Failed(format!("the approval could not be read: {error}")),
        }
        let Some(held) = HeldMutation::of_args(&view.args) else {
            return ToolOutcome::Failed("the approved write could not be read".to_string());
        };
        let Some(factory) = factory() else {
            return ToolOutcome::Failed("write_ui runs only in a run's context".to_string());
        };
        // what was true when the card was asked is checked again: the policy, the role group, the user's permission
        if !ctx.approvals.write_rule.offered {
            return ToolOutcome::Failed("this agent may no longer make this write, so nothing was run".to_string());
        }
        if let Err(why) = ui::write::check_write(&factory, &ctx.agent.role_groups(), &held) {
            return ToolOutcome::Failed(format!("{why}, so nothing was run"));
        }
        let grant = match approval::begin(ctx, &view.approval_id, &view.args) {
            Ok(grant) => grant,
            Err(text) => return ToolOutcome::Failed(text),
        };
        let run_id = ctx.run.map(|run| run.run_id.clone());
        let executed = ui::write::execute(&factory, &ctx.agent.agent_id, run_id.as_deref(), &grant.approval_id, &held);
        approval::finish(
            ctx,
            &grant.approval_id,
            executed.ok,
            &json!({"ok": executed.ok, "error": executed.error, "messages": executed.data["messageInfos"]}),
        );
        ToolOutcome::text(ui::write::outcome_text(&executed, &held))
    }
}

/// What the model reads of a canvas shown: its version and parts, never the program again.
fn shown_text(canvas: &Canvas, prepared: &ui::Prepared, patched: bool) -> String {
    let names: Vec<&str> = prepared.statements.iter().map(String::as_str).take(20).collect();
    let mut text = format!(
        "canvas v{} {}: \"{}\" ({} statements: {}{})",
        canvas.version,
        if patched { "patched and shown" } else { "shown" },
        canvas.title,
        prepared.statements.len(),
        names.join(", "),
        if prepared.statements.len() > names.len() { ", …" } else { "" }
    );
    if !prepared.mutations.is_empty() && canvas.dialect == "vue-sfc" {
        text.push_str(&format!(
            "\nActions {} run when the user submits them and confirms; to run one without that, call write_ui with run (it waits for approval).",
            prepared.mutations.join(", ")
        ));
    } else if !prepared.mutations.is_empty() {
        text.push_str(&format!(
            "\nMutations {} run when the user clicks their Button; to run one without a click, call write_ui with run (it waits for approval).",
            prepared.mutations.join(", ")
        ));
    }
    text.push_str(if canvas.dialect == "vue-sfc" {
        "\nThe user sees it now. Say in one line what it shows; to change it, send a patch with only the changed blocks."
    } else {
        "\nThe user sees it now. Say in one line what it shows; to change it, send a patch with only the changed statements."
    });
    text
}

/// `run`: the agent proposes a Mutation of the canvas without the user's click, which is an approval card.
fn propose_run(ctx: &mut TurnContext<'_>, factory: &Arc<ExecutionContextFactory>, canvas: Option<&Canvas>, statement: &str) -> ToolOutcome {
    let Some(canvas) = canvas else {
        return ToolError::new("no_canvas", "There is no canvas, so there is no Mutation to run. Show one with program first.").outcome();
    };
    let run_id = ctx.run.map(|run| run.run_id.clone());
    let held = match ui::write::hold(factory, ctx.agent, run_id.as_deref(), canvas, statement, None) {
        Ok(held) => held,
        Err(why) => {
            let rule = if canvas.dialect == "vue-sfc" { ui::vue_sfc::ACTIONS } else { openui::MUTATION_SHAPE };
            return ToolError::new("mutation", format!("{why}, so nothing was run."))
                .skill(format!("{}#{}", rule.skill, rule.section))
                .rule(rule.text)
                .outcome();
        }
    };
    let request = ApprovalRequest {
        target_kind: held.target_kind(),
        target_name: held.target_name(),
        classification: Classification::Write,
        args: held.to_args(),
        summary: held.summary(&canvas.title),
        changes: held.changes(),
        snapshot: json!({}),
        record_refs: Json::Null,
        preview: PreviewMode::None,
        handoff: None,
    };
    match approval::request(ctx, &request) {
        Requested::Pause(pause) => ToolOutcome::Pause(pause),
        Requested::Refused(text) => ToolOutcome::Failed(text),
        Requested::Approved(grant) => {
            let executed = ui::write::execute(factory, &ctx.agent.agent_id, run_id.as_deref(), &grant.approval_id, &held);
            approval::finish(ctx, &grant.approval_id, executed.ok, &json!({"ok": executed.ok, "error": executed.error}));
            ToolOutcome::text(ui::write::outcome_text(&executed, &held))
        }
    }
}

crate::submit_tool!(WriteUi);
