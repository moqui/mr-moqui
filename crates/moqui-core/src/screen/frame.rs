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

//! The frame of a parent level (docs/ui/Qvt-Client.md, "A level is its own screen"; docs/security/Attack-Surface.md,
//! "Parent frames").
//!
//! The `/qapps` shell loads each level of the page's path as its own `.qvt` request, and the terminal client each level
//! between the app root and the leaf as its own `.tui` request; the server checks each as the target of its own path. A
//! render of the whole path enters the parents without their own check, so a user whose grants open a screen below a
//! parent but not the parent itself (a non-inheritable grant on an area, with none on the app root) can open the page and
//! still be refused the parent's level. The client names the page it loads the level for in [`PAGE_PATH_HEADER`]; when
//! the user may not open the level itself but may open that page, and the level is a parent on the page's path, the
//! level answers as a frame: the placeholders of the levels below it and nothing else. No action of the level runs and
//! no widget of its own is written. Its tabs list the subscreens the user may open, as the menu data lists them: in qvt
//! the shell fills them from its menu data, in tui the frame carries them. The level asked for alone, or for a page the
//! user may not open, is refused as before.

use std::sync::Arc;

use moqui_lib::node::MNode;
use moqui_lib::object::{to_plain_string, Value};

use super::cache::ScreenFacade;
use super::definition::{Authentication, ScreenDef};
use super::render::{Job, RenderState};
use super::url::ScreenUrlInfo;
use crate::context::ExecutionContextFactory;
use crate::web;

/// The request header in which a client names the page a parent level is loaded for: the page's path under the same
/// root (`/apps/People/Time/LeaveRequests`), percent-encoded, with no query.
pub const PAGE_PATH_HEADER: &str = "X-Moqui-Page-Path";

/// Answer the qvt or tui level `info` as the frame of the page the request names, when the rule holds. Returns whether
/// it answered; otherwise the request goes on, and is refused or rendered as before (a page path that does not resolve
/// leaves it as it was).
pub(super) fn serve_frame(
    factory: &Arc<ExecutionContextFactory>,
    job: &Job<'_>,
    info: &Arc<ScreenUrlInfo>,
    mode: &str,
) -> bool {
    let tui = mode == "tui";
    if !(tui || mode == "qvt") || !job.method.eq_ignore_ascii_case("get") {
        return false;
    }
    let Some(page_path) = job
        .request
        .header(PAGE_PATH_HEADER)
        .map(str::trim)
        .filter(|path| !path.is_empty())
    else {
        return false;
    };
    // A screen on the level's path that grants anonymous access may let the level's own render through where the check
    // below does not (the render applies the grant, the check does not), so such a level renders or is refused as before.
    if info.target_transition_name.is_some()
        || info.file_resource.is_some()
        || info.path_defs.iter().any(|def| {
            matches!(
                def.authentication,
                Authentication::AnonymousView | Authentication::AnonymousAll
            )
        })
        || super::urls::path_permitted(Arc::clone(info))
    {
        return false;
    }
    let Some(page) = page_below(factory, job.root, info, page_path) else {
        return false;
    };
    if !super::urls::path_permitted(Arc::clone(&page))
        || super::render::path_items_available(&page, page.path_defs.len()).is_err()
        || crate::user::device::check_route(&page).is_err()
    {
        return false;
    }
    let (content_type, body) = if tui {
        ("application/json", tui_frame(factory, info, &page).to_string())
    } else {
        ("text/html;charset=UTF-8", frame_text(&info.target_screen))
    };
    let mut headers = vec![
        ("Content-Type".to_string(), content_type.to_string()),
        ("Cache-Control".to_string(), "private, no-store".to_string()),
    ];
    if !tui {
        headers.push(("X-Moqui-Qvt".to_string(), "merged-fragment".to_string()));
    }
    web::send_bytes_with_headers(200, headers, body.into_bytes());
    true
}

/// The page `path` names, when it is a screen strictly below the level `info` on the same screens: the level's names
/// start the page's, and each screen of the level is the page's at the same depth. A path that does not resolve, a
/// sparse (`//`) path, a transition and a file are none.
fn page_below(
    factory: &ExecutionContextFactory,
    root: &str,
    info: &ScreenUrlInfo,
    path: &str,
) -> Option<Arc<ScreenUrlInfo>> {
    let path = web::percent_decode(path);
    if !path.starts_with('/') || path.starts_with("//") || path.contains(['?', '#']) {
        return None;
    }
    let page = ScreenUrlInfo::parse(factory, root, &path, 0).ok()?;
    let level = info.path_defs.len();
    let below = page.target_transition_name.is_none()
        && page.file_resource.is_none()
        && page.path_defs.len() > level
        && page.full_path.len() > info.full_path.len()
        && page.full_path[..info.full_path.len()] == info.full_path[..]
        && page.path_defs[..level]
            .iter()
            .zip(&info.path_defs)
            .all(|(on_page, asked)| on_page.location == asked.location);
    below.then_some(page)
}

/// A placeholder of a level's widgets that a frame keeps.
#[derive(Debug, PartialEq, Eq)]
enum Placeholder {
    /// A `subscreens-menu` of tabs.
    Tabs(String),
    /// A `subscreens-panel` of tabs: the tabs and the active level under them.
    Panel(String),
    /// A `subscreens-active`, or a panel whose steps are the screen's own (wizard, stack) or that is a popup menu.
    Active(String),
}

/// The placeholders of `def`'s widgets in order, up to the first that holds the active level: a `subscreens-menu` is the
/// tabs (none for a popup), a `subscreens-active` the active level, and a `subscreens-panel` the active level under the
/// tabs (none for a popup; a wizard or stack, whose steps are the screen's own, is the active level alone). A screen
/// with none, or whose placeholder is in an included section, is the active level. Only the first active level counts,
/// as only one renders (a placeholder in a section's `fail-widgets` beside one in its `widgets` is not a second level).
fn placeholders(def: &ScreenDef) -> Vec<Placeholder> {
    let mut out = Vec::new();
    let found = def
        .root
        .widgets
        .as_ref()
        .and_then(|widgets| widgets.node().ok())
        .is_some_and(|widgets| collect_placeholders(&widgets, &mut out));
    if !found {
        out.push(Placeholder::Active("subscreens".to_string()));
    }
    out
}

/// Add the placeholders under `node`; true once the active level is found, after which nothing more is added.
fn collect_placeholders(node: &MNode, out: &mut Vec<Placeholder>) -> bool {
    let mut found = false;
    node.for_each_child(|child| {
        if found {
            return;
        }
        let kind = child.attribute("type").unwrap_or_default();
        let tabs = kind != "popup" && kind != "wizard" && kind != "stack";
        let id = || {
            child
                .attribute("id")
                .filter(|id| !id.is_empty())
                .unwrap_or_else(|| "subscreens".to_string())
        };
        match &*child.name_rc() {
            "subscreens-menu" => {
                if tabs {
                    out.push(Placeholder::Tabs(id()));
                }
            }
            "subscreens-active" => {
                out.push(Placeholder::Active(id()));
                found = true;
            }
            "subscreens-panel" => {
                out.push(if tabs {
                    Placeholder::Panel(id())
                } else {
                    Placeholder::Active(id())
                });
                found = true;
            }
            _ => found = collect_placeholders(child, out),
        }
    });
    found
}

/// The qvt frame of `def`: its placeholders as the qvt macros write them, with nothing else of the screen.
pub(super) fn frame_text(def: &ScreenDef) -> String {
    let mut out = String::new();
    for placeholder in placeholders(def) {
        match placeholder {
            Placeholder::Tabs(_) => out.push_str("<m-subscreens-tabs></m-subscreens-tabs>"),
            Placeholder::Panel(_) => {
                out.push_str("<m-subscreens-tabs></m-subscreens-tabs><m-subscreens-active></m-subscreens-active>");
            }
            Placeholder::Active(_) => out.push_str("<m-subscreens-active></m-subscreens-active>"),
        }
    }
    format!("<div class=\"m-frame\">{out}</div>")
}

/// The tui frame of the level `info` for `page`: the envelope of the level with its placeholders as the tui macros write
/// them, a tab bar (`subscreens`) holding the subscreens the user may open with the one on the page's path active, and
/// the active level. The tabs are the menu data's entries of the level for the page ([`super::menu::subscreen_entry`],
/// each authorized below the level), less those out of the menu, as the tui macros' `sri.getMenuItems` leaves them out.
fn tui_frame(
    factory: &Arc<ExecutionContextFactory>,
    info: &Arc<ScreenUrlInfo>,
    page: &ScreenUrlInfo,
) -> serde_json::Value {
    let facade = ScreenFacade::of(factory);
    let state = RenderState::new(
        factory,
        &facade,
        Arc::clone(info),
        "tui".to_string(),
        "application/json".to_string(),
        "UTF-8".to_string(),
    );
    state.screen_path_index.set(info.path_defs.len().saturating_sub(1));
    let mut nodes = Vec::new();
    for placeholder in placeholders(&info.target_screen) {
        let node = match placeholder {
            Placeholder::Tabs(id) | Placeholder::Panel(id) => serde_json::json!({
                "t": "subscreens",
                "id": super::sri::unique_id(&state, &id),
                "kind": "tab",
                "items": tab_items(&state, info, page),
                "c": [],
            }),
            Placeholder::Active(id) => serde_json::json!({
                "t": "active",
                "id": super::sri::unique_id(&state, &id),
                "c": [],
            }),
        };
        nodes.push(node);
    }
    super::render::json_envelope(
        info,
        serde_json::Value::Array(nodes),
        super::render::envelope_parameters(),
        "",
        super::render::empty_messages(),
    )
}

/// The tabs of the level `info` for `page`, as the tui macros write a tab bar's items.
fn tab_items(state: &RenderState, info: &ScreenUrlInfo, page: &ScreenUrlInfo) -> Vec<serde_json::Value> {
    let level = &info.target_screen;
    let names = &info.full_path;
    let next = page.full_path.get(names.len()).cloned().unwrap_or_default();
    let parent_path: String = names
        .iter()
        .map(|name| format!("/{}", moqui_lib::string::url_encode_if_needed(name)))
        .collect();
    let strip = super::sri::strip_controls;
    let text = |entry: &moqui_lib::object::ValueMap, name: &str| {
        strip(&entry.get(name).map(to_plain_string).unwrap_or_default())
    };
    let flag = |entry: &moqui_lib::object::ValueMap, name: &str| matches!(entry.get(name), Some(Value::Bool(true)));
    let mut items = Vec::new();
    for item in level.subscreens.sorted() {
        if !item.is_valid_in_current_context() || !super::menu::included(&level.location, item) {
            continue;
        }
        match super::menu::subscreen_entry(state, &info.path_defs, names, &parent_path, item, &next, true) {
            Ok(Some(Value::Map(entry))) => items.push(serde_json::json!({
                "title": text(&entry, "title"),
                "url": text(&entry, "pathWithParams"),
                "active": flag(&entry, "active"),
                "disabled": flag(&entry, "disableLink"),
            })),
            Ok(_) => {}
            Err(err) => super::menu::leave_out(&level.location, item, &err),
        }
    }
    items
}
