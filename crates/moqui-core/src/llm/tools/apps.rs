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

//! The apps as the read tools see them: a render of the webroot as the run's user, fenced, with every write held
//! ([`render`]), and the screens of the suite shell's data, the menus to depth two and the palette actions of the apps
//! the user may open ([`shell_entries`]). `search` ranks these; `data` names the nearest of them for a path that is not
//! a screen. Paths are written as `/apps/...`, a `/qapps/...` link as its `/apps/` twin.

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_lib::object::Value;
use serde_json::Value as Json;

use crate::context::{Error, ExecutionContext, ExecutionContextFactory};
use crate::llm::run::context::TurnContext;
use crate::screen::{make_render, RenderOptions, RenderOrigin, RenderOutput, TransitionGate};

/// The factory of the execution context the tool runs in.
pub fn factory() -> Result<Arc<ExecutionContextFactory>, Error> {
    ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
        .ok_or_else(|| Error::new("no execution context is entered"))
}

/// A path as the model writes it: `/apps/...` for a `/qapps/...` one, a leading `/`.
pub fn app_path(path: &str) -> String {
    let path = path.trim();
    let path = if let Some(rest) = path.strip_prefix("/qapps/") {
        format!("/apps/{rest}")
    } else if path == "/qapps" {
        "/apps".to_string()
    } else {
        path.to_string()
    };
    if path.starts_with('/') {
        path
    } else {
        format!("/{path}")
    }
}

/// Render `path` of the webroot with `parameters` as the run's user: under the read fence, every write transition held,
/// from the agent's origin. `mode` is a render mode (`tui`) or none for the path's own.
///
/// # Errors
///
/// Returns the render's error (its kind says 401, 403 or 404).
pub fn render(
    ctx: &TurnContext<'_>,
    path: &str,
    parameters: BTreeMap<String, Value>,
    mode: Option<&str>,
) -> Result<RenderOutput, Error> {
    let factory = factory()?;
    let root = crate::screen::info::root_screen_for_host(&factory, "", "webroot")?;
    let mut render = make_render(&factory)
        .root_screen(root)
        .screen_path(path)
        .parameters(parameters)
        .options(RenderOptions {
            origin: RenderOrigin::Agent {
                agent_id: ctx.agent.agent_id.clone(),
                run_id: ctx.run.map(|run| run.run_id.clone()),
            },
            gate: TransitionGate::Hold,
            write_fence: true,
        });
    if let Some(mode) = mode {
        render = render.render_mode(mode);
    }
    render.render()
}

/// The JSON a render answered.
///
/// # Errors
///
/// Returns an error when the status is not 200 or the body is not JSON.
pub fn render_json(ctx: &TurnContext<'_>, path: &str, parameters: BTreeMap<String, Value>) -> Result<Json, Error> {
    let output = render(ctx, path, parameters, None)?;
    if output.status != 200 {
        return Err(Error::new(format!("{path} answered {}", output.status)));
    }
    serde_json::from_slice(&output.body).map_err(|error| Error::new(format!("{path} did not answer JSON: {error}")))
}

/// A screen or an action of the shell data.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellEntry {
    pub title: String,
    /// The app and the menu entries above it, `System › Security`.
    pub trail: String,
    pub path: String,
}

/// The screens (each app root, then its menu to depth two) and the palette actions of the apps the user may open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellEntries {
    pub screens: Vec<ShellEntry>,
    pub actions: Vec<ShellEntry>,
}

/// [`ShellEntries`] from the `shellData` transition of the webroot. A shell that does not answer is logged and has
/// none.
pub fn shell_entries(ctx: &TurnContext<'_>) -> ShellEntries {
    match render_json(ctx, "/shellData", BTreeMap::new()) {
        Ok(data) => entries_of(&data),
        Err(error) => {
            tracing::error!("the shell data for the agent tools could not be read: {error}");
            ShellEntries::default()
        }
    }
}

fn text(value: &Json, key: &str) -> String {
    value
        .get(key)
        .and_then(Json::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// [`ShellEntries`] of a `shellData` answer, as the palette's Screens and Actions groups make them.
pub fn entries_of(data: &Json) -> ShellEntries {
    let mut out = ShellEntries::default();
    for app in data.get("apps").and_then(Json::as_array).into_iter().flatten() {
        let app_title = text(app, "title");
        let app_path = text(app, "path");
        if !app_path.is_empty() {
            out.screens.push(ShellEntry {
                title: app_title.clone(),
                trail: String::new(),
                path: self::app_path(&app_path),
            });
        }
        let mut parents: Vec<String> = Vec::new();
        for entry in app.get("menu").and_then(Json::as_array).into_iter().flatten() {
            let depth = entry
                .get("depth")
                .and_then(Json::as_u64)
                .and_then(|depth| usize::try_from(depth).ok())
                .unwrap_or(1)
                .max(1);
            parents.truncate(depth - 1);
            let title = text(entry, "title");
            let path = text(entry, "path");
            if !path.is_empty() {
                let trail: Vec<String> = std::iter::once(app_title.clone())
                    .chain(parents.iter().filter(|title| !title.is_empty()).cloned())
                    .collect();
                out.screens.push(ShellEntry {
                    title: title.clone(),
                    trail: trail.join(" › "),
                    path: self::app_path(&path),
                });
            }
            while parents.len() < depth - 1 {
                parents.push(String::new());
            }
            parents.push(title);
        }
        let actions = app
            .get("providers")
            .and_then(|providers| providers.get("paletteActions"))
            .and_then(Json::as_array);
        for action in actions.into_iter().flatten() {
            let path = text(action, "url");
            if path.is_empty() || out.actions.iter().any(|known| known.path == self::app_path(&path)) {
                continue;
            }
            out.actions.push(ShellEntry {
                title: text(action, "label"),
                trail: app_title.clone(),
                path: self::app_path(&path),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_shell_data_gives_screens_with_their_trail_and_actions() {
        let data = json!({"apps": [{"title": "Open App", "path": "/qapps/open", "menu": [
            {"title": "List", "path": "/qapps/open/List", "depth": 1},
            {"title": "Sub", "path": "/qapps/open/Sub", "depth": 1},
            {"title": "Deep one", "path": "/qapps/open/Sub/Deep1", "depth": 2}],
            "providers": {"paletteActions": [{"label": "Open the list", "url": "/qapps/open/List"}]}}]});
        let entries = entries_of(&data);
        let screens: Vec<(&str, &str, &str)> = entries
            .screens
            .iter()
            .map(|entry| (entry.title.as_str(), entry.trail.as_str(), entry.path.as_str()))
            .collect();
        assert_eq!(
            screens,
            [
                ("Open App", "", "/apps/open"),
                ("List", "Open App", "/apps/open/List"),
                ("Sub", "Open App", "/apps/open/Sub"),
                ("Deep one", "Open App › Sub", "/apps/open/Sub/Deep1"),
            ]
        );
        assert_eq!(entries.actions[0].title, "Open the list");
        assert_eq!(entries.actions[0].trail, "Open App");
        assert_eq!(app_path("qapps"), "/qapps");
        assert_eq!(app_path("/qapps/x?a=1"), "/apps/x?a=1");
    }
}
