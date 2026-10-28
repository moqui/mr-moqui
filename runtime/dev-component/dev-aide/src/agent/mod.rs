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

//! The dev tools of the agents (plans/design-dev-aide.md 13.2): `artifact`, `source` and `check` read the index, the
//! files and the checks; `propose_edit` proposes a change set the user accepts; `try_service` runs a service in sim;
//! `vcs` reads and writes git. Each needs the `dev.aide` capability, which a run has only on a dev instance with the
//! dev-aide loaded for a user with one of its permissions, and checks the instance, the component and the user's tier
//! again when called. The read tools are listed over MCP too (13.6).

pub mod artifact;
pub mod check;
pub mod propose;
pub mod source;
pub mod try_service;
pub mod vcs;

use std::path::{Path, PathBuf};

use moqui_core::context::ExecutionContext;
use moqui_llm::tools::format::ToolError;
use moqui_llm::tools::ToolOutcome;
use serde_json::Value as Json;

use crate::roots::{self, Root};
use crate::tier::Tier;
use crate::workspace::Context;

/// The dev-aide's context for a tool call that needs `least`, or the error the model reads.
pub(crate) fn context(least: Tier) -> Result<Context, ToolError> {
    ExecutionContext::with_current(|ec| Context::of(ec, least))
        .ok_or_else(|| ToolError::new("no_context", "The dev tools run inside a request of the server."))?
        .map_err(|err| {
            ToolError::new("not_allowed", err.to_string())
                .hint("The dev tools need a dev instance, the dev-aide component and a Dev AIDE permission.")
        })
}

/// A file a tool names: by a component location (`component://my-site/entity/SiteEntities.xml`), an absolute path,
/// or a root's id and a path in it (`my-site:entity/SiteEntities.xml`).
pub(crate) struct NamedFile<'a> {
    pub root: &'a Root,
    pub relative: String,
    pub path: PathBuf,
    /// `component://...` when the file is in a loaded component, else the path.
    pub shown: String,
}

pub(crate) fn file<'a>(context: &'a Context, named: &str) -> Result<NamedFile<'a>, ToolError> {
    let named = named.trim();
    if named.is_empty() {
        return Err(ToolError::new(
            "no_path",
            "Name the file: component://<component>/<path>, or root:path.",
        ));
    }
    let (root, relative) = if let Some(location) = named.strip_prefix("component://") {
        let (component, rest) = location.split_once('/').unwrap_or((location, ""));
        let dir = context
            .factory
            .components()
            .iter()
            .find(|info| info.name == component)
            .map(|info| info.location.clone())
            .ok_or_else(|| ToolError::new("no_component", format!("No component {component} is loaded.")))?;
        let full = std::fs::canonicalize(&dir).unwrap_or(dir).join(rest);
        roots::locate(&context.roots, &full)
            .ok_or_else(|| ToolError::new("no_root", format!("{named} is in no root of the Workspace.")))?
    } else if Path::new(named).is_absolute() {
        roots::locate(&context.roots, Path::new(named))
            .ok_or_else(|| ToolError::new("no_root", format!("{named} is in no root of the Workspace.")))?
    } else if let Some((id, rest)) = named.split_once(':') {
        let root = context
            .root(id)
            .map_err(|err| ToolError::new("no_root", err.to_string()))?;
        (root, rest.trim_start_matches('/').to_string())
    } else {
        return Err(ToolError::new(
            "bad_path",
            format!("{named}: name the file as component://<component>/<path>, root:path, or an absolute path."),
        ));
    };
    let resolved = context
        .resolve(root, &relative)
        .map_err(|err| ToolError::new("bad_path", err.to_string()))?;
    let shown = location_of(context, &resolved.path).unwrap_or_else(|| resolved.path.display().to_string());
    Ok(NamedFile {
        root,
        relative: resolved.relative,
        path: resolved.path,
        shown,
    })
}

/// `component://<component>/<path>` of a file in a loaded component.
pub(crate) fn location_of(context: &Context, path: &Path) -> Option<String> {
    context.factory.components().iter().find_map(|component| {
        let dir = std::fs::canonicalize(&component.location).unwrap_or_else(|_| component.location.clone());
        path.strip_prefix(&dir).ok().map(|relative| {
            format!(
                "component://{}/{}",
                component.name,
                relative.to_string_lossy().replace('\\', "/")
            )
        })
    })
}

/// A tool's answer as compact JSON for the model.
pub(crate) fn json(value: &Json) -> ToolOutcome {
    ToolOutcome::text(serde_json::to_string(value).unwrap_or_default())
}

pub(crate) fn arg<'a>(args: &'a Json, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Json::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

/// A list argument given as a list or a comma separated text.
pub(crate) fn list_arg(args: &Json, key: &str) -> Vec<String> {
    match args.get(key) {
        Some(Json::Array(items)) => items.iter().filter_map(Json::as_str).map(str::to_string).collect(),
        Some(Json::String(text)) => text
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}
