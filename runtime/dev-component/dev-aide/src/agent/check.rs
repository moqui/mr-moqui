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

//! `check` (plans/design-dev-aide.md 13.2, READ): the checks of a file, saved or as proposed text (the schema, the
//! references, the expressions; D17's upstream edit), each problem naming its rule and the skill section that has it;
//! with no file, the problems of the last reloads. The words are the editor's Problems panel's, so a person and a model
//! fix them the same way.

use moqui_core::devtools::{check, recent_reports, Problem, Severity};
use moqui_llm::run::context::TurnContext;
use moqui_llm::tools::format::ToolError;
use moqui_llm::tools::schema::{object, Property};
use moqui_llm::tools::{caps, AgentTool, Classification, ToolOutcome, ToolSpec};
use serde_json::{json, Value as Json};

use super::{arg, context, file};
use crate::tier::Tier;
use crate::workspace::Context;

/// The rule of an edit to an upstream component while the lock is on, and its skill section.
pub(crate) const UPSTREAM_RULE: &str = "upstream.edit";
pub(crate) const UPSTREAM_SKILL: &str = "dev-aide/skill/site-customizing.md#Extend, do not fork";

pub struct CheckTool(ToolSpec);

impl Default for CheckTool {
    fn default() -> Self {
        let mut spec = ToolSpec::new(
            "check",
            "Check a Moqui file, saved or as proposed text: schema, references, expressions, and whether an edit there is \
             an upstream edit that belongs in the site component. Each problem names its rule and the skill section with \
             it. With no file: the problems of the last reloads.",
            object(&[
                Property::optional("file", "string", "component://<component>/<path>, or root:path"),
                Property::optional("text", "string", "The proposed text of the file; the saved text when left out"),
            ]),
            Classification::Read,
        )
        .needs(&[caps::DEV]);
        spec.mcp = true;
        Self(spec)
    }
}

impl AgentTool for CheckTool {
    fn spec(&self) -> &ToolSpec {
        &self.0
    }

    fn call(&self, _ctx: &mut TurnContext<'_>, args: Json) -> ToolOutcome {
        match run(&args) {
            Ok(answer) => super::json(&answer),
            Err(error) => error.outcome(),
        }
    }
}

fn run(args: &Json) -> Result<Json, ToolError> {
    let context = context(Tier::View)?;
    let Some(named) = arg(args, "file") else {
        let reports = recent_reports(&context.factory);
        let problems: Vec<&Problem> = reports.iter().take(20).flat_map(|report| report.problems()).collect();
        return Ok(json!({"lastReloads": reports.len().min(20), "problems": problems}));
    };
    let named = file(&context, named)?;
    let proposed = args.get("text").and_then(Json::as_str);
    let text = match proposed {
        Some(text) => text.to_string(),
        None => std::fs::read_to_string(&named.path)
            .map_err(|err| ToolError::new("not_read", format!("{}: {err}", named.shown)))?,
    };
    let mut problems = problems_of(&context, &named.path, &text);
    if proposed.is_some() {
        problems.extend(upstream_problem(&context, named.root, &named.relative, &named.path));
    }
    let errors = problems
        .iter()
        .filter(|problem| problem.severity == Severity::Error)
        .count();
    Ok(json!({"file": named.shown, "errors": errors, "problems": problems}))
}

/// The checks of `text` as the file at `path`: an XML file's schema, references and expressions.
pub(crate) fn problems_of(context: &Context, path: &std::path::Path, text: &str) -> Vec<Problem> {
    if path.extension().is_some_and(|ext| ext == "xml") {
        check::check_text(&context.factory, path, text)
    } else {
        Vec::new()
    }
}

/// D17's rule: a change to a file of an upstream component while the lock is on, with where the extension goes.
pub(crate) fn upstream_problem(
    context: &Context,
    root: &crate::roots::Root,
    relative: &str,
    path: &std::path::Path,
) -> Option<Problem> {
    if root.is_site_path(relative) || context.upstream_edits() {
        return None;
    }
    let component = root
        .component_of(relative)
        .map_or_else(|| root.label.clone(), |component| component.name.clone());
    let site = context
        .settings
        .default_site()
        .map_or_else(|| "a site component".to_string(), str::to_string);
    let extension = match relative.split('/').find(|part| !part.is_empty() && *part != component) {
        Some("entity") => "an extend-entity in the site component's entity file",
        Some("screen") => "a screen-extend file in the site component's screen-extend directory",
        Some("service") => "a new service, or a SECA rule, in the site component",
        Some("data") => "a seed data file in the site component",
        _ => "an extension in the site component",
    };
    Some(Problem {
        path: path.display().to_string(),
        line: 0,
        column: 0,
        severity: Severity::Error,
        rule: UPSTREAM_RULE.to_string(),
        message: format!(
            "{component} is upstream: extend it in {site} with {extension}. An edit to an upstream component is a fork \
             that every upgrade fights."
        ),
        skill: Some(UPSTREAM_SKILL.to_string()),
    })
}
moqui_llm::submit_tool!(CheckTool);

#[cfg(test)]
mod tests {
    use super::*;

    /// The upstream rule's sentence is in its skill section word for word (plans/design-dev-aide.md 13.1).
    #[test]
    fn the_upstream_rule_is_in_its_skill_section() {
        let skill = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/skill/site-customizing.md")).unwrap();
        let (_, section) = UPSTREAM_SKILL.split_once('#').unwrap();
        let start = skill.find(&format!("\n## {section}\n")).expect("the section");
        let body = &skill[start + 1..];
        let body = &body[..body[3..].find("\n## ").map_or(body.len(), |end| end + 3)];
        assert!(body.contains("An edit to an upstream component is a fork that every upgrade fights."));
    }
}
