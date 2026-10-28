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

//! `org.moqui.devaide.ScreenServices`: the screen builder's server side (plans/design-dev-aide.md 11). The screen tree
//! as the screen facade mounts it, one level at a time; the screen view of a file or an app page (the base screen, where
//! it renders, the site's `screen-extend` of it for an upstream screen, D17); the canvas buffers, the unsaved texts of a
//! Design frame, which the screen overlay's provider answers for `buffer:<id>` tokens (F14) so the frame renders the
//! buffer and not the file; the stamps of a stamped render resolved to their files (F10); and a new subscreen from an
//! archetype of the style guide's gallery.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use moqui_core::context::{Error, ExecutionContext, ExecutionContextFactory};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_core::screen::overlay::Overlay;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use crate::roots::{self, Root};
use crate::site;
use crate::tier::Tier;
use crate::workspace::{json_value, text, Context};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(get_screen_children), get_screen_children)?;
    registry.service_function(moqui_core::service_location!(get_screen_view), get_screen_view)?;
    registry.service_function(moqui_core::service_location!(put_canvas_buffer), put_canvas_buffer)?;
    registry.service_function(moqui_core::service_location!(resolve_stamps), resolve_stamps)?;
    registry.service_function(moqui_core::service_location!(create_subscreen), create_subscreen)
}

/// The root the tree starts at: the apps under the shell.
const APPS: &str = "component://webroot/screen/webroot/apps.xml";
/// The shell's path of `APPS`.
const APPS_PATH: &str = "/qapps";

/// `component://<component>/<path>` of a file of a root.
fn location_of(root: &Root, relative: &str) -> Option<String> {
    let component = root.component_of(relative)?;
    let rest = if component.path.is_empty() {
        relative.to_string()
    } else {
        relative.strip_prefix(&format!("{}/", component.path))?.to_string()
    };
    Some(format!("component://{}/{rest}", component.name))
}

/// The file of a `component://` location, or a path, as the Workspace opens it.
fn file_of(context: &Context, location: &str) -> Option<(String, String, bool)> {
    let path = match location.strip_prefix("component://") {
        Some(rest) => {
            let (name, rest) = rest.split_once('/')?;
            context
                .factory
                .components()
                .iter()
                .find(|component| component.name == name)
                .map(|component| component.location.join(rest))?
        }
        None => Path::new(location).to_path_buf(),
    };
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    let (root, relative) = roots::locate(&context.roots, &path)?;
    let site = root.is_site_path(&relative);
    Some((root.id.clone(), relative, site))
}

/// `get#ScreenChildren`: the subscreens of a screen (the apps under the shell when none is named), each with its URL
/// path under `path`, its file, and how it is mounted.
fn get_screen_children(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let location = Some(text(parameters, "location"))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| APPS.into());
    let path = Some(text(parameters, "path"))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| APPS_PATH.into());
    let children: Vec<Json> = moqui_core::screen::info::screen_children(&context.factory, &location)?
        .into_iter()
        .map(|row| {
            let mut out = moqui_lib::json::to_json(&Value::Map(row));
            let name = out["name"].as_str().unwrap_or_default().to_string();
            out["path"] = json!(format!("{path}/{name}"));
            if let Some((root, relative, site)) = out["location"]
                .as_str()
                .and_then(|location| file_of(&context, location))
            {
                out["rootId"] = json!(root);
                out["relativePath"] = json!(relative);
                out["site"] = json!(site);
            }
            out
        })
        .collect();
    let mut out = Parameters::new();
    out.insert("children".into(), json_value(&children));
    Ok(out)
}

/// The URL path under the shell where the screen at `location` renders: a walk of the tree from the apps, at most a
/// few hundred screens deep enough for an app's pages.
fn path_of(factory: &Arc<ExecutionContextFactory>, location: &str) -> Option<String> {
    let mut queue = std::collections::VecDeque::from([(APPS.to_string(), APPS_PATH.to_string(), 0usize)]);
    let mut seen = 0usize;
    while let Some((at, path, depth)) = queue.pop_front() {
        seen += 1;
        if seen > 600 || depth > 6 {
            break;
        }
        for row in moqui_core::screen::info::screen_children(factory, &at).unwrap_or_default() {
            let child = moqui_lib::object::to_plain_string(row.get("location").unwrap_or(&Value::Null));
            let name = moqui_lib::object::to_plain_string(row.get("name").unwrap_or(&Value::Null));
            let child_path = format!("{path}/{name}");
            if child == location {
                return Some(child_path);
            }
            if row.get("error").is_none() {
                queue.push_back((child, child_path, depth + 1));
            }
        }
    }
    None
}

/// `get#ScreenView`: a screen as the builder opens it, from its file (`rootId` and `path`, a screen or a
/// `screen-extend`) or an app page (`page`, its path and query): the base screen's location and file, where it renders
/// and with what parameters, whether it is upstream, and where its edits go (the base file in place, or the site's
/// `screen-extend` of it).
fn get_screen_view(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let page = text(parameters, "page");
    let (mut url_path, query) = match page.split_once('?') {
        Some((path, query)) => (path.to_string(), query.to_string()),
        None => (page.clone(), String::new()),
    };
    let location = if !url_path.is_empty() {
        // the shell's pages (/qapps/..., /apps/...) are the apps' screens; anything else is under the webroot
        let under_apps = ["/qapps/", "/apps/"]
            .iter()
            .find_map(|prefix| url_path.strip_prefix(prefix));
        let parsed = match under_apps {
            Some(rest) => moqui_core::screen::ScreenUrlInfo::parse(&context.factory, APPS, &format!("/{rest}"), 0)?,
            None => moqui_core::screen::ScreenUrlInfo::parse(
                &context.factory,
                moqui_core::screen::ROOT_SCREEN,
                &url_path,
                0,
            )?,
        };
        if under_apps.is_some() && url_path.starts_with("/apps/") {
            url_path = url_path.replacen("/apps/", "/qapps/", 1);
        }
        parsed.target_screen.location.clone()
    } else {
        let root = context.root(&text(parameters, "rootId"))?;
        let relative = text(parameters, "path");
        let location =
            location_of(root, &relative).ok_or_else(|| LibError::new(format!("{relative} is in no component")))?;
        // a screen-extend file opens on the screen it extends
        match location.split_once("/screen-extend/") {
            Some((_, rest)) => format!("component://{rest}"),
            None => location,
        }
    };
    if url_path.is_empty() {
        url_path = path_of(&context.factory, &location).unwrap_or_default();
    }
    let base = file_of(&context, &location);
    let base_site = base.as_ref().is_some_and(|(_, _, site)| *site);
    let in_place = base_site || context.upstream_edits();
    let after_scheme = location.strip_prefix("component://").unwrap_or(&location).to_string();
    let extension = (!in_place)
        .then(|| site::site_path(&context, &format!("screen-extend/{after_scheme}")))
        .flatten();
    let definition = moqui_core::screen::ScreenFacade::of(&context.factory).screen_def(&context.factory, &location)?;
    let parameters_list: Vec<Json> = definition
        .parameters
        .iter()
        .map(|parameter| json!({ "name": parameter.name, "required": parameter.required }))
        .collect();
    let view = json!({
        "location": location,
        "base": base.as_ref().map(|(root, path, site)| json!({ "rootId": root, "path": path, "site": site })),
        "urlPath": url_path,
        "query": query,
        "parameters": parameters_list,
        "upstream": !base_site,
        "inPlace": in_place,
        "extension": extension.map(|file| file.json()),
        "site": context.settings.default_site(),
        "renderModes": definition.render_modes,
        "tier": context.tier.name(),
    });
    let mut out = Parameters::new();
    out.insert("view".into(), json_value(&view));
    Ok(out)
}

/// The canvas buffers: by user and buffer id, the texts at their locations, for two hours since their last put.
struct Buffer {
    at: Instant,
    texts: Vec<(String, String)>,
}

fn buffers() -> &'static Mutex<HashMap<(String, String), Buffer>> {
    static BUFFERS: OnceLock<Mutex<HashMap<(String, String), Buffer>>> = OnceLock::new();
    BUFFERS.get_or_init(|| Mutex::new(HashMap::new()))
}

const BUFFER_LIFE: Duration = Duration::from_secs(2 * 60 * 60);
const MAX_BUFFERS: usize = 200;

/// `put#CanvasBuffer`: the unsaved texts a Design frame renders (`files`, a list of `{rootId, path, text}`, the
/// user's own files to write), kept under `bufferId` for the frame's `buffer:<id>` token.
fn put_canvas_buffer(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let user = moqui_core::user::current_user_id()?.unwrap_or_default();
    let id = text(parameters, "bufferId");
    if id.is_empty() || id.len() > 64 || !id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-') {
        return Err(LibError::new("a buffer id is letters, digits and -"));
    }
    let files: Vec<Json> = match parameters.get("files") {
        Some(Value::String(raw)) => serde_json::from_str(raw).unwrap_or_default(),
        Some(value) => serde_json::from_value(moqui_lib::json::to_json(value)).unwrap_or_default(),
        None => Vec::new(),
    };
    let mut texts = Vec::new();
    for file in files.iter().take(20) {
        let root = context.root(file["rootId"].as_str().unwrap_or_default())?;
        let relative = file["path"].as_str().unwrap_or_default();
        let resolved = context.resolve(root, relative)?;
        let body = file["text"].as_str().unwrap_or_default();
        if body.len() as u64 > context.settings.max_edit_bytes {
            return Err(LibError::new(format!("{relative} is over the edit limit")));
        }
        let location = location_of(root, &resolved.relative).unwrap_or_else(|| resolved.path.display().to_string());
        texts.push((location, body.to_string()));
    }
    let mut kept = buffers().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    kept.retain(|_, buffer| buffer.at.elapsed() < BUFFER_LIFE);
    if kept.len() >= MAX_BUFFERS {
        if let Some(oldest) = kept
            .iter()
            .min_by_key(|(_, buffer)| buffer.at)
            .map(|(key, _)| key.clone())
        {
            kept.remove(&oldest);
        }
    }
    let count = texts.len();
    kept.insert(
        (user, id),
        Buffer {
            at: Instant::now(),
            texts,
        },
    );
    let mut out = Parameters::new();
    out.insert("files".into(), Value::I64(i64::try_from(count).unwrap_or(0)));
    Ok(out)
}

/// The overlay of a `buffer:<id>` token: the current user's canvas buffer of that id (an empty one renders the files).
pub(crate) fn buffer_overlay(factory: &Arc<ExecutionContextFactory>, id: &str) -> Option<Overlay> {
    let user = moqui_core::user::current_user_id().ok().flatten()?;
    moqui_core::devtools::dev_aide_user(factory, &user).then_some(())?;
    let kept = buffers().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut overlay = Overlay::new();
    if let Some(buffer) = kept.get(&(user, id.to_string())) {
        for (location, text) in &buffer.texts {
            overlay.insert_location(factory, location, text);
        }
    }
    Some(overlay)
}

/// `resolve#Stamps`: the stamps of a stamped render (`file:line:column`, the file a `component://` location or a
/// path) as the Workspace opens them: each with its root, path, line and column, and whether it is the site's.
fn resolve_stamps(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let stamps: Vec<String> = match parameters.get("stamps") {
        Some(Value::List(items)) => items.iter().map(moqui_lib::object::to_plain_string).collect(),
        Some(Value::String(raw)) if raw.starts_with('[') => serde_json::from_str(raw).unwrap_or_default(),
        Some(value) => moqui_lib::object::to_plain_string(value)
            .split(',')
            .map(str::to_string)
            .collect(),
        None => Vec::new(),
    };
    let mut resolved = serde_json::Map::new();
    for stamp in stamps.iter().take(500) {
        let mut parts = stamp.rsplitn(3, ':');
        let (Some(column), Some(line), Some(file)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let entry = match file_of(&context, file) {
            Some((root, path, site)) => json!({ "rootId": root, "path": path, "site": site,
                "line": line.parse::<u32>().unwrap_or(0), "column": column.parse::<u32>().unwrap_or(0) }),
            None => json!({ "file": file, "line": line.parse::<u32>().unwrap_or(0) }),
        };
        resolved.insert(stamp.clone(), entry);
    }
    let mut out = Parameters::new();
    out.insert("stamps".into(), json_value(&Json::Object(resolved)));
    Ok(out)
}

/// The archetypes a new screen starts from (the style guide's §3, as the example component's gallery shows them).
const ARCHETYPES: &[&str] = &[
    "Landing",
    "List",
    "Detail",
    "Record360",
    "Task",
    "Board",
    "Calendar",
    "Split",
    "Settings",
    "Wizard",
    "Dialog",
    "Report",
    "Editor",
    "Panel",
];

/// `create#Subscreen`: a new screen file in the directory of a parent screen (`rootId` and `parentPath`, the parent's
/// file; the new one at `<parent dir>/<Parent>/<name>.xml`), from an archetype of the gallery (its XML, with its menu
/// title) or empty. Answers its path and whether the parent is upstream, when its directory is not the site's: the
/// item then goes in the site's `screen-extend` of the parent, which the client writes.
fn create_subscreen(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let root = context.root(&text(parameters, "rootId"))?;
    let parent = text(parameters, "parentPath");
    let name = text(parameters, "name");
    if name.is_empty() || !name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
        return Err(LibError::new("a screen name is letters, digits and _"));
    }
    let archetype = text(parameters, "archetype");
    let title = Some(text(parameters, "menuTitle"))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| name.clone());
    let directory = parent
        .strip_suffix(".xml")
        .ok_or_else(|| LibError::new("the parent is a screen file"))?;
    let relative = format!("{directory}/{name}.xml");
    let resolved = context.resolve(root, &relative)?;
    if resolved.path.exists() {
        return Err(LibError::new(format!("{relative} is there already")));
    }
    let body = if ARCHETYPES.contains(&archetype.as_str()) {
        let gallery = context
            .factory
            .components()
            .iter()
            .find(|component| component.name == "example")
            .map(|component| component.location.join(format!("screen/Gallery/{archetype}.xml")));
        gallery
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|text| retitle(&text, &title))
            .ok_or_else(|| LibError::new(format!("the {archetype} archetype is not in the example component")))?
    } else {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<screen xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:noNamespaceSchemaLocation=\"http://moqui.org/xsd/xml-screen-3.xsd\"\n        default-menu-title=\"{}\">\n    <widgets>\n    </widgets>\n</screen>\n",
            title.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;")
        )
    };
    let saved = crate::workspace::save_text(&context, root, &resolved, &body, "", None)?;
    let mut out = saved;
    out.insert("path".into(), Value::string(relative));
    out.insert("upstreamParent".into(), Value::Bool(!root.is_site_path(&parent)));
    Ok(out)
}

/// An archetype's XML with `default-menu-title` set to `title`.
fn retitle(text: &str, title: &str) -> String {
    let escaped = title.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;");
    match moqui_lib::xml_edit::outline(text) {
        Ok(_) => moqui_lib::xml_edit::apply(
            text,
            &[moqui_lib::xml_edit::Op::SetAttribute {
                at: moqui_lib::xml_edit::Locator::Path(Vec::new()),
                name: "default-menu-title".into(),
                value: title.to_string(),
            }],
        )
        .map_or_else(|_| text.to_string(), |applied| applied.text),
        Err(_) => text.replacen("<screen ", &format!("<screen default-menu-title=\"{escaped}\" "), 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_archetype_takes_its_new_title() {
        let text = "<screen default-menu-title=\"List\">\n    <widgets/>\n</screen>\n";
        assert_eq!(
            retitle(text, "Widgets"),
            "<screen default-menu-title=\"Widgets\">\n    <widgets/>\n</screen>\n"
        );
    }
}
