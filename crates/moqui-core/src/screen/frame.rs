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
//! The `/qapps` shell loads each level of the page's path as its own `.qvt` request, and the server checks each as the
//! target of its own path. A render of the whole path enters the parents without their own check, so a user whose
//! grants open a screen below a parent but not the parent itself (a non-inheritable grant on an area, with none on the
//! app root) can open the page and still be refused the parent's level. The shell names the page it loads the level for
//! in [`PAGE_PATH_HEADER`]; when the user may not open the level itself but may open that page, and the level is a
//! parent on the page's path, the level answers as a frame: the placeholders of the levels below it and nothing else.
//! No action of the level runs and no widget of its own is written; its menu is the menu data's, which lists only the
//! screens the user may open. The level asked for alone, or for a page the user may not open, is refused as before.

use std::sync::Arc;

use moqui_lib::node::MNode;

use super::definition::ScreenDef;
use super::render::Job;
use super::url::ScreenUrlInfo;
use crate::context::ExecutionContextFactory;
use crate::web;

/// The request header in which the shell names the page a parent level is loaded for: the page's path under the same
/// root (`/apps/People/Time/LeaveRequests`), percent-encoded, with no query.
pub const PAGE_PATH_HEADER: &str = "X-Moqui-Page-Path";

/// Answer the qvt level `info` as the frame of the page the request names, when the rule holds. Returns whether it
/// answered; otherwise the request goes on, and is refused or rendered as before (a page path that does not resolve
/// leaves it as it was).
pub(super) fn serve_frame(
    factory: &ExecutionContextFactory,
    job: &Job<'_>,
    info: &Arc<ScreenUrlInfo>,
    mode: &str,
) -> bool {
    if mode != "qvt" || !job.method.eq_ignore_ascii_case("get") {
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
    if info.target_transition_name.is_some()
        || info.file_resource.is_some()
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
    web::send_bytes_with_headers(
        200,
        vec![
            ("Content-Type".to_string(), "text/html;charset=UTF-8".to_string()),
            ("Cache-Control".to_string(), "private, no-store".to_string()),
            ("X-Moqui-Qvt".to_string(), "merged-fragment".to_string()),
        ],
        frame_text(&info.target_screen).into_bytes(),
    );
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
        && page.full_path.len() >= info.full_path.len()
        && page.full_path[..info.full_path.len()] == info.full_path[..]
        && page.path_defs[..level]
            .iter()
            .zip(&info.path_defs)
            .all(|(on_page, asked)| on_page.location == asked.location);
    below.then_some(page)
}

/// The placeholders of `def`'s widgets in order, as the qvt macros write them, with nothing else of the screen: a
/// `subscreens-menu` is the tabs (none for a popup), a `subscreens-active` the active level, and a `subscreens-panel`
/// the active level under the tabs (none for a popup; a wizard or stack, whose steps are the screen's own, is the
/// active level alone). A screen with none, or whose placeholder is in an included section, is the active level.
pub(super) fn frame_text(def: &ScreenDef) -> String {
    let mut out = String::new();
    if let Some(widgets) = def.root.widgets.as_ref().and_then(|widgets| widgets.node().ok()) {
        write_placeholders(&widgets, &mut out);
    }
    if !out.contains("<m-subscreens-active>") {
        out.push_str("<m-subscreens-active></m-subscreens-active>");
    }
    format!("<div class=\"m-frame\">{out}</div>")
}

fn write_placeholders(node: &MNode, out: &mut String) {
    node.for_each_child(|child| {
        let kind = child.attribute("type").unwrap_or_default();
        let tabs = kind != "popup" && kind != "wizard" && kind != "stack";
        match &*child.name_rc() {
            "subscreens-menu" if tabs => out.push_str("<m-subscreens-tabs></m-subscreens-tabs>"),
            "subscreens-active" => out.push_str("<m-subscreens-active></m-subscreens-active>"),
            "subscreens-panel" => {
                if tabs {
                    out.push_str("<m-subscreens-tabs></m-subscreens-tabs>");
                }
                out.push_str("<m-subscreens-active></m-subscreens-active>");
            }
            _ => write_placeholders(child, out),
        }
    });
}
