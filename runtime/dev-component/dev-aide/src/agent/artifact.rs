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

use std::path::Path;

use moqui_core::devtools::index::{self, ArtifactRow, Direction};
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

impl Default for ArtifactTool {
    fn default() -> Self {
        let mut spec = ToolSpec::new(
            "artifact",
            "The Moqui artifact index: find artifacts by name (find), describe one with its file, line and extensions \
             (describe), list what uses it or what it uses (refs), or name what is at a line of a file (at). Ids look \
             like entity:mantle.order.OrderHeader, service:a.B.verb#Noun, screen:component://..., form:<screen>#Name.",
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
                Property::optional("file", "string", "at: component://<component>/<path>"),
                Property::optional("line", "integer", "at: the line"),
                Property::optional("word", "string", "at: the name on the line"),
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

fn run(args: &Json) -> Result<Json, ToolError> {
    let context = context(Tier::View)?;
    let factory = &context.factory;
    let failed = |err: moqui_lib::error::Error| ToolError::new("index", err.to_string());
    let named = |id: &str| -> Result<ArtifactRow, ToolError> {
        index::named(factory, id).map_err(failed)?.ok_or_else(|| {
            let near: Vec<String> = index::find(factory, id, &[], &[], 5)
                .unwrap_or_default()
                .into_iter()
                .map(|artifact| artifact.id)
                .collect();
            ToolError::new("not_found", format!("No artifact is named {id}.")).did_you_mean(near)
        })
    };
    match arg(args, "action").unwrap_or("find") {
        "find" => {
            let text = arg(args, "text").ok_or_else(|| ToolError::new("no_text", "find needs text: a name."))?;
            let found = index::find(factory, text, &list_arg(args, "kinds"), &[], 20).map_err(failed)?;
            Ok(json!({"artifacts": found.iter().map(|artifact| shown(&context, artifact)).collect::<Vec<_>>()}))
        }
        "describe" => {
            let artifact = named(arg(args, "id").or_else(|| arg(args, "text")).unwrap_or(""))?;
            let users = index::refs(factory, &artifact.id, Direction::In, &[]).map_err(failed)?;
            let uses = index::refs(factory, &artifact.id, Direction::Out, &[]).map_err(failed)?;
            let extensions: Vec<Json> = users
                .iter()
                .filter(|entry| entry.ref_type == "extend" || entry.ref_type == "anchor")
                .filter_map(|entry| entry.other.as_ref().map(|other| shown(&context, other)))
                .collect();
            // an entity's fields with their types (and which component adds each), a screen's transitions and forms
            let parts: Vec<Json> = index::children(factory, &artifact.id)
                .map_err(failed)?
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
            let mut out = shown(&context, &artifact);
            out["usedBy"] = json!(users.len());
            out["uses"] = json!(uses.len());
            out["extensions"] = json!(extensions);
            if !parts.is_empty() {
                out["parts"] = json!(parts);
            }
            Ok(out)
        }
        "refs" => {
            let artifact = named(arg(args, "id").or_else(|| arg(args, "text")).unwrap_or(""))?;
            let direction = Direction::parse(arg(args, "direction").unwrap_or("in"));
            let found = index::refs(factory, &artifact.id, direction, &list_arg(args, "kinds")).map_err(failed)?;
            let rows: Vec<Json> = found
                .iter()
                .take(200)
                .map(|entry| {
                    let mut row = json!({
                        "how": entry.ref_type,
                        "file": super::location_of(&context, Path::new(&entry.path)).unwrap_or_else(|| entry.path.clone()),
                        "line": entry.line,
                    });
                    match &entry.other {
                        Some(other) => row["artifact"] = shown(&context, other),
                        None => row["name"] = json!(if direction == Direction::In { entry.from.clone() } else { entry.to_name.clone() }),
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
            Ok(
                json!({"artifact": artifact.id, "direction": if direction == Direction::In { "in" } else { "out" },
                "count": found.len(), "refs": rows}),
            )
        }
        "at" => {
            let file = super::file(&context, arg(args, "file").unwrap_or(""))?;
            let line = args.get("line").and_then(Json::as_u64).unwrap_or(0);
            let found = index::at(
                factory,
                &file.path.display().to_string(),
                u32::try_from(line).unwrap_or(0),
                arg(args, "word").unwrap_or(""),
            )
            .map_err(failed)?;
            Ok(match found {
                Some((reference, artifact)) => json!({
                    "reference": reference.map(|entry| json!({"how": entry.ref_type, "names": entry.to_name, "resolved": entry.resolved})),
                    "artifact": artifact.map(|artifact| shown(&context, &artifact)),
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

moqui_llm::submit_tool!(ArtifactTool);
