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

//! `artifact` (plans/design-dev-aide.md 13.2, READ): `find`, `describe`, `refs` and `at` over the artifact index. An
//! answer gives each artifact's id, kind, name, location and line, its component and whether that component is the
//! site's or upstream (D17), so the model knows where an extension goes.
//!
//! With `project`, the same answers are about another Moqui project on the server's machine (a Java Moqui checkout,
//! an MR Moqui clone or runtime, [`moqui_devkit::project`]): read into memory on first use and kept fresh with its
//! files, no restart. Its artifacts have no side (it is not this Workspace's), and the answer names the project.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moqui_devkit::index::{self, ArtifactRow, At, Direction, RefEntry};
use moqui_devkit::project::cache::{self as projects, ProjectIndex};
use moqui_llm::run::context::TurnContext;
use moqui_llm::tools::format::ToolError;
use moqui_llm::tools::schema::{object, Property};
use moqui_llm::tools::{caps, AgentTool, Classification, ToolOutcome, ToolSpec};
use serde_json::{json, Value as Json};

use super::{arg, context, list_arg};
use crate::roots;
use crate::tier::Tier;
use crate::workspace::Context;

pub struct ArtifactTool(ToolSpec);

/// The `project` argument's description, the same in each tool that takes it.
pub(crate) const PROJECT_ARG: &str =
    "Another Moqui project on the server's machine, by its absolute directory: a Java \
     Moqui checkout (framework/src/main/java or groovy beside runtime/), an MR Moqui clone or an MR Moqui runtime. \
     Left out: this server's own runtime";

impl Default for ArtifactTool {
    fn default() -> Self {
        let mut spec = ToolSpec::new(
            "artifact",
            "The Moqui artifact index: find artifacts by name (find), describe one with its file, line and extensions \
             (describe: an entity's fields, a service's parameters, a screen's transitions and forms), list what uses \
             it or what it uses (refs), or name what is at a line of a file (at). Ids look like \
             entity:mantle.order.OrderHeader, service:a.B.verb#Noun, screen:component://..., form:<screen>#Name. \
             With project, about another Moqui project's artifacts.",
            object(&[
                Property::required("action", "string", "find, describe, refs or at"),
                Property::optional(
                    "text",
                    "string",
                    "find: a name, camel case or segments (OrdHdr, place#Order)",
                ),
                Property::optional("id", "string", "describe, refs: an artifact id, or a name"),
                Property::optional("direction", "string", "refs: in (what uses it, the default) or out"),
                Property::optional(
                    "kinds",
                    "string",
                    "find, refs: kinds, comma separated (entity, service, screen, form, field ...)",
                ),
                Property::optional(
                    "file",
                    "string",
                    "at: component://<component>/<path>; refs with no id: a file, for every artifact it defines \
                     (with project, an absolute path in it works too)",
                ),
                Property::optional("line", "integer", "at: the line"),
                Property::optional("word", "string", "at: the name on the line"),
                Property::optional("project", "string", PROJECT_ARG),
            ]),
            Classification::Read,
        )
        .needs(&[caps::DEV]);
        spec.mcp = true;
        Self(spec)
    }
}

/// An artifact for the model: its location in place of the file's path, and which side of the line it is on.
pub(crate) fn shown(context: &Context, artifact: &ArtifactRow) -> Json {
    let site = roots::locate(&context.roots, Path::new(&artifact.path))
        .is_some_and(|(root, relative)| root.is_site_path(&relative));
    let location = super::location_of(context, Path::new(&artifact.path)).unwrap_or_else(|| artifact.path.clone());
    let mut out = json!({
        "id": artifact.id, "kind": artifact.kind, "name": artifact.name, "component": artifact.component,
        "file": location, "line": artifact.line, "side": if site { "site" } else { "upstream" },
    });
    if !artifact.summary.is_empty() {
        out["summary"] = json!(artifact.summary);
    }
    out
}

impl AgentTool for ArtifactTool {
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

/// The project a call names (`project`), or none for the server's own runtime. With `model`, a project whose entity or
/// service files changed is booted again first (for the checks and a service's parameters).
pub(crate) fn project_of(context: &Context, args: &Json, model: bool) -> Result<Option<Arc<ProjectIndex>>, ToolError> {
    let Some(named) = arg(args, "project") else {
        return Ok(None);
    };
    projects::requested(named, &context.factory, model).map_err(|err| ToolError::new("project", err.to_string()))
}

/// `{root, layout}` of a project, as the answers about it name it.
pub(crate) fn project_json(project: &ProjectIndex) -> Json {
    json!({"root": project.project.root.display().to_string(), "layout": project.project.layout.as_str()})
}

/// Where the answers come from: the server's own index (its database rows), or another project's in memory.
enum Source<'a> {
    Own(&'a Context),
    Project(Arc<ProjectIndex>),
}

fn failed(err: moqui_lib::error::Error) -> ToolError {
    ToolError::new("index", err.to_string())
}

impl Source<'_> {
    fn find(&self, text: &str, kinds: &[String], limit: usize) -> Result<Vec<ArtifactRow>, ToolError> {
        match self {
            Self::Own(context) => index::find(&context.factory, text, kinds, &[], limit).map_err(failed),
            Self::Project(project) => Ok(project.find(text, kinds, &[], limit)),
        }
    }

    fn named(&self, id: &str) -> Result<ArtifactRow, ToolError> {
        let found = match self {
            Self::Own(context) => index::named(&context.factory, id).map_err(failed)?,
            Self::Project(project) => project.named(id),
        };
        found.ok_or_else(|| {
            let near: Vec<String> = self
                .find(id, &[], 5)
                .unwrap_or_default()
                .into_iter()
                .map(|artifact| artifact.id)
                .collect();
            ToolError::new("not_found", format!("No artifact is named {id}.")).did_you_mean(near)
        })
    }

    fn refs(&self, id: &str, direction: Direction, kinds: &[String]) -> Result<Vec<RefEntry>, ToolError> {
        match self {
            Self::Own(context) => index::refs(&context.factory, id, direction, kinds).map_err(failed),
            Self::Project(project) => Ok(project.refs(id, direction, kinds)),
        }
    }

    fn children(&self, id: &str) -> Result<Vec<ArtifactRow>, ToolError> {
        match self {
            Self::Own(context) => index::children(&context.factory, id).map_err(failed),
            Self::Project(project) => Ok(project.children(id)),
        }
    }

    fn at(&self, path: &Path, line: u32, word: &str) -> Result<Option<At>, ToolError> {
        let path = path.display().to_string();
        match self {
            Self::Own(context) => index::at(&context.factory, &path, line, word).map_err(failed),
            Self::Project(project) => Ok(project.at(&path, line, word)),
        }
    }

    fn in_file(&self, path: &Path) -> Result<Vec<ArtifactRow>, ToolError> {
        match self {
            Self::Own(context) => index::in_file(&context.factory, &path.display().to_string()).map_err(failed),
            Self::Project(project) => Ok(project.in_file(path)),
        }
    }

    /// The file a call names (`file`): a Workspace file, or one in a component of the project.
    fn file(&self, named: &str) -> Result<PathBuf, ToolError> {
        match self {
            Self::Own(context) => super::file(context, named).map(|file| file.path),
            Self::Project(project) => project_file(project, named),
        }
    }

    fn location(&self, path: &str) -> String {
        let found = match self {
            Self::Own(context) => super::location_of(context, Path::new(path)),
            Self::Project(project) => project.location_of(Path::new(path)),
        };
        found.unwrap_or_else(|| path.to_string())
    }

    fn shown(&self, artifact: &ArtifactRow) -> Json {
        match self {
            Self::Own(context) => shown(context, artifact),
            Self::Project(_) => {
                let mut out = json!({
                    "id": artifact.id, "kind": artifact.kind, "name": artifact.name, "component": artifact.component,
                    "file": self.location(&artifact.path), "line": artifact.line,
                });
                if !artifact.summary.is_empty() {
                    out["summary"] = json!(artifact.summary);
                }
                out
            }
        }
    }

    /// A service's in and out parameters, from the service definitions of the server or of the project.
    fn parameters(&self, name: &str) -> Option<Json> {
        match self {
            Self::Own(_) => moqui_core::service::define::find(name).map(|def| projects::parameters_of(&def)),
            Self::Project(project) => project.service_parameters(name),
        }
    }
}

/// The file `named` (`component://<component>/<path>` or an absolute path) in a component of `project`.
pub(crate) fn project_file(project: &ProjectIndex, named: &str) -> Result<PathBuf, ToolError> {
    project.file(named).ok_or_else(|| {
        ToolError::new(
            "no_file",
            format!(
                "{named} is in no component of {}: name it as component://<component>/<path> or an absolute path in \
                 one.",
                project.project.root.display()
            ),
        )
    })
}

fn run(args: &Json) -> Result<Json, ToolError> {
    let context = context(Tier::View)?;
    let action = arg(args, "action").unwrap_or("find");
    let project = project_of(&context, args, action == "describe")?;
    let named_project = project.as_deref().map(project_json);
    let source = match project {
        Some(project) => Source::Project(project),
        None => Source::Own(&context),
    };
    let mut answer = answer(&source, action, args)?;
    if let (Some(named), Some(out)) = (named_project, answer.as_object_mut()) {
        out.insert("project".to_string(), named);
    }
    Ok(answer)
}

fn answer(source: &Source<'_>, action: &str, args: &Json) -> Result<Json, ToolError> {
    match action {
        "find" => {
            let text = arg(args, "text").ok_or_else(|| ToolError::new("no_text", "find needs text: a name."))?;
            let found = source.find(text, &list_arg(args, "kinds"), 20)?;
            Ok(json!({"artifacts": found.iter().map(|artifact| source.shown(artifact)).collect::<Vec<_>>()}))
        }
        "describe" => describe(source, arg(args, "id").or_else(|| arg(args, "text")).unwrap_or("")),
        "refs" => refs(source, args),
        "at" => {
            let file = source.file(arg(args, "file").unwrap_or(""))?;
            let line = args.get("line").and_then(Json::as_u64).unwrap_or(0);
            let found = source.at(&file, u32::try_from(line).unwrap_or(0), arg(args, "word").unwrap_or(""))?;
            Ok(match found {
                Some((reference, artifact)) => json!({
                    "reference": reference.map(|entry| json!({"how": entry.ref_type, "names": entry.to_name, "resolved": entry.resolved})),
                    "artifact": artifact.map(|artifact| source.shown(&artifact)),
                }),
                None => json!({"artifact": null}),
            })
        }
        other => Err(ToolError::new(
            "bad_action",
            format!("{other}: the action is find, describe, refs or at."),
        )),
    }
}

fn describe(source: &Source<'_>, id: &str) -> Result<Json, ToolError> {
    let artifact = source.named(id)?;
    let users = source.refs(&artifact.id, Direction::In, &[])?;
    let uses = source.refs(&artifact.id, Direction::Out, &[])?;
    let extensions: Vec<Json> = users
        .iter()
        .filter(|entry| entry.ref_type == "extend" || entry.ref_type == "anchor")
        .filter_map(|entry| entry.other.as_ref().map(|other| source.shown(other)))
        .collect();
    // an entity's fields with their types (and which component adds each), a screen's transitions and forms
    let parts: Vec<Json> = source
        .children(&artifact.id)?
        .iter()
        .filter(|child| child.kind != "anchor")
        .map(|child| {
            let mut part = json!({"kind": child.kind, "name": child.short_name, "line": child.line});
            if !child.summary.is_empty() {
                part["summary"] = json!(child.summary);
            }
            if child.component != artifact.component {
                part["component"] = json!(child.component);
            }
            part
        })
        .collect();
    let mut out = source.shown(&artifact);
    out["usedBy"] = json!(users.len());
    out["uses"] = json!(uses.len());
    out["extensions"] = json!(extensions);
    if !parts.is_empty() {
        out["parts"] = json!(parts);
    }
    // a service's in and out parameters, as its definition has them (with what implements and extends add)
    if artifact.kind == "service" {
        if let Some(parameters) = source.parameters(&artifact.name) {
            out["parameters"] = parameters;
        }
    }
    Ok(out)
}

/// `refs` of an artifact (`id`), or of every artifact a file defines (`file` with no `id`).
fn refs(source: &Source<'_>, args: &Json) -> Result<Json, ToolError> {
    let direction = Direction::parse(arg(args, "direction").unwrap_or("in"));
    let kinds = list_arg(args, "kinds");
    let (named, targets) = match (arg(args, "id").or_else(|| arg(args, "text")), arg(args, "file")) {
        (None, Some(file)) => {
            let path = source.file(file)?;
            let shown = source.location(&path.display().to_string());
            let defined: Vec<String> = source.in_file(&path)?.into_iter().map(|artifact| artifact.id).collect();
            if defined.is_empty() {
                return Err(ToolError::new(
                    "not_found",
                    format!("{shown} defines no artifact in the index."),
                ));
            }
            (shown, defined)
        }
        (id, _) => {
            let artifact = source.named(id.unwrap_or(""))?;
            (artifact.id.clone(), vec![artifact.id])
        }
    };
    let mut found = Vec::new();
    for target in &targets {
        found.extend(source.refs(target, direction, &kinds)?);
    }
    let rows: Vec<Json> = found
        .iter()
        .take(200)
        .map(|entry| {
            let mut row = json!({"how": entry.ref_type, "file": source.location(&entry.path), "line": entry.line});
            match &entry.other {
                Some(other) => row["artifact"] = source.shown(other),
                None => {
                    row["name"] = json!(if direction == Direction::In {
                        entry.from.clone()
                    } else {
                        entry.to_name.clone()
                    });
                }
            }
            if entry.calls > 0 {
                row["calls"] = json!(entry.calls);
            }
            if !entry.resolved {
                row["unresolved"] = json!(true);
            }
            row
        })
        .collect();
    let mut out = json!({"artifact": named, "direction": if direction == Direction::In { "in" } else { "out" },
        "count": found.len(), "refs": rows});
    if targets.first() != Some(&named) {
        out["artifacts"] = json!(targets);
    }
    Ok(out)
}

moqui_llm::submit_tool!(ArtifactTool);
