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

//! `sri.getMenuData`: the user-specific menu for a screen path as the `qvt` client reads it from the
//! `menuData` transition of the root screen. One entry for each screen on the path above the target with
//! the subscreens the user may view, and a last entry for the target.
//!
//! The last entry has the target's screen documents and the user's saved finds of its form-lists that say
//! `saved-finds="true"`, as Java's.

use moqui_lib::object::Value;

use super::definition::{ScreenDef, SubscreensItem};
use super::render::{Frames, RenderState};
use super::sri::screen_permitted;
use super::url::ScreenUrlInfo;
use super::urls::{self, UrlInstance};
use crate::context::{Error, ErrorKind, ExecutionContext};
use crate::{l10n, script, user, web};

type Item = moqui_lib::object::ValueMap;

fn text(value: &str) -> Value {
    Value::string(value)
}

/// Whether the user may view `target` below `parents`, the screens from the root down to its parent, decided on a fresh
/// stack as when the target renders below them ([`Frames::check`]): a grant on a parent reaches the screens below it,
/// while one on the screen that renders the check (a menu, the app list) does not.
pub(super) fn permitted_under(
    parents: &[std::sync::Arc<super::definition::ScreenDef>],
    target: &super::definition::ScreenDef,
) -> bool {
    let mut frames = Frames::check();
    for parent in parents {
        if frames.push_screen(parent, false).is_err() {
            return false;
        }
    }
    screen_permitted(target)
}

/// The image of a menu entry. A `url-screen` image is a path from the root screen.
fn menu_image(state: &RenderState, image: Option<&(String, String)>) -> (Value, Value) {
    let Some((path, kind)) = image.filter(|(path, _)| !path.is_empty()) else {
        return (Value::Null, Value::Null);
    };
    let path = if kind.is_empty() || kind == "url-screen" {
        urls::build_url(state, path).map_or_else(|_| path.clone(), |url| url.url())
    } else {
        path.clone()
    };
    (text(&path), text(kind))
}

fn path_with_params(url: &UrlInstance) -> String {
    let query = url.parameter_string();
    if query.is_empty() {
        url.path()
    } else {
        format!("{}?{query}", url.path())
    }
}

/// Leave a subscreen out of a menu when its entry cannot be made: its screen does not load or parse, or an expression of
/// its item fails. The error is logged with the screen's location and its siblings still show (AGENTS.md failure
/// tolerance). A security error leaves it out quietly, as a screen the user may not view is.
pub(crate) fn leave_out(parent: &str, item: &SubscreensItem, err: &Error) {
    if !super::sri::is_security(err) {
        tracing::error!(
            "Leaving subscreen {} [{}] of screen [{parent}] out of the menu: {err}",
            item.name,
            item.location
        );
    }
}

/// Whether `item`, a subscreen of the screen at `parent`, shows in a menu ([`SubscreensItem::in_menu`]): a
/// `menu-include` expression that fails leaves it out and is logged, as [`leave_out`] does. The user group is checked
/// apart ([`SubscreensItem::is_valid_in_current_context`]), since an item on the request's path shows either way.
pub(crate) fn included(parent: &str, item: &SubscreensItem) -> bool {
    item.in_menu().unwrap_or_else(|err| {
        leave_out(parent, item, &err);
        false
    })
}

/// Whether `target`'s own `<condition>` decides whether it is offered: a screen with a condition and no `fail-widgets`.
/// With `fail-widgets` the condition only chooses what the screen shows (Insights' record screens), so it is always
/// offered.
pub(crate) fn has_offer_condition(target: &ScreenDef) -> bool {
    target.root.condition.is_some() && target.root.fail_widgets.is_none()
}

/// Whether `target`, the screen of `item` below the screen at `parent`, is offered in the current context: its own
/// `<condition>` holds, or [`has_offer_condition`] is false. Evaluate it only where the context holds what the parents
/// set (their always-actions at least). A condition that fails is logged and leaves the screen offered, so one bad
/// expression does not hide a tab.
pub(crate) fn offered(parent: &str, item: &SubscreensItem, target: &ScreenDef) -> bool {
    let Some(condition) = target.root.condition.as_ref().filter(|_| has_offer_condition(target)) else {
        return true;
    };
    super::render::check_condition(condition).unwrap_or_else(|err| {
        tracing::error!(
            "The condition of subscreen {} [{}] of screen [{parent}] failed, so it stays in the menu: {err}",
            item.name,
            item.location
        );
        true
    })
}

/// The menu entry of `item`, a subscreen of the screen at `parent_names` and `parent_path`, or None when the user may not
/// view it or a required parameter is missing. `parents` are the screens from the root down to its parent, for the
/// authorization. `next_item` is the subscreen on the path, and `in_menu` whether the item shows in the menu
/// ([`included`]) and not only for being on the path. The shell data (`shell.rs`) makes its entries here too.
pub(super) fn subscreen_entry(
    state: &RenderState,
    parents: &[std::sync::Arc<super::definition::ScreenDef>],
    parent_names: &[String],
    parent_path: &str,
    item: &SubscreensItem,
    next_item: &str,
    in_menu: bool,
) -> Result<Option<Value>, Error> {
    let screen_path = format!("{parent_path}/{}", super::url::encode_path_name(&item.name));
    let mut names = parent_names.to_vec();
    names.push(item.name.clone());
    let mut url = urls::resolve_names_at(state, names, 0, true)?;
    // The page and sort of the list being shown are not a sibling's (a link or the current URL keeps them).
    if item.name != next_item {
        url.drop_list_state();
    }
    let Some(sui) = url.info.as_ref() else {
        return Ok(None);
    };
    let target = state.facade.screen_def(&state.factory, &item.location)?;
    if !permitted_under(parents, &target) {
        return Ok(None);
    }
    let missing = sui
        .parameter_items
        .values()
        .any(|parameter| parameter.required && url.parameters.get(&parameter.name).is_none_or(String::is_empty));
    if missing {
        return Ok(None);
    }
    let (image, image_type) = menu_image(state, sui.menu_image.as_ref());
    let disabled = match &item.disable_when {
        Some(expression) => script::eval_condition(expression)?,
        None => false,
    };
    let mut entry = Item::default();
    entry.insert("name".into(), text(&item.name));
    entry.insert(
        "title".into(),
        text(&script::expand(&l10n::localize(&item.menu_title)?)?),
    );
    entry.insert("path".into(), text(&screen_path));
    entry.insert("pathWithParams".into(), text(&path_with_params(&url)));
    entry.insert("image".into(), image);
    entry.insert("imageType".into(), image_type);
    // the caption its run of menu items is drawn under (menu-group), when it has one
    if !item.menu_group.is_empty() {
        entry.insert("group".into(), text(&l10n::localize(&item.menu_group)?));
    }
    if in_menu {
        entry.insert("menuInclude".into(), Value::Bool(true));
    }
    if item.name == next_item {
        entry.insert("active".into(), Value::Bool(true));
    }
    if disabled {
        entry.insert("disableLink".into(), Value::Bool(true));
    }
    Ok(Some(Value::Map(entry)))
}

/// The menu data for `path_names`, or None when a JSON error was sent instead: 401 without a user, 404
/// for a path that names no screen, 403 for a screen the user may not view.
///
/// A subscreen beside the path whose entry fails is logged and left out ([`leave_out`]).
///
/// # Errors
///
/// Returns an error when a screen on the path does not load.
pub fn menu_data(state: &RenderState, path_names: &[String]) -> Result<Option<Value>, Error> {
    if user::current_username()?.is_none() {
        web::send_json_error(401, "Authentication required");
        return Ok(None);
    }
    let root = state
        .url_info
        .path_defs
        .first()
        .map(|def| def.location.clone())
        .ok_or_else(|| Error::new("no root screen"))?;
    let info = match ScreenUrlInfo::resolve(&state.factory, &root, path_names, 0) {
        Ok(info) => info,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            web::send_json_error(404, &format!("Screen not found for path {path_names:?}"));
            return Ok(None);
        }
        Err(err) => return Err(err),
    };
    if !permitted_under(
        &info.path_defs[..info.path_defs.len().saturating_sub(1)],
        &info.target_screen,
    ) {
        web::send_json_error(403, &format!("View not permitted for path {path_names:?}"));
        return Ok(None);
    }

    let mut full_path = info.full_path.clone();
    let extra = info.extra_path.clone();
    if !extra.is_empty() {
        full_path.truncate(full_path.len().saturating_sub(extra.len()));
    }
    let Some(last_item) = full_path.last().cloned() else {
        web::send_json_error(404, &format!("Screen not found for path {path_names:?}"));
        return Ok(None);
    };

    // A subscreen's own condition reads what the screens above it set in their always-actions (the app root's appRoot,
    // a 360's tab list), as when the page renders; so when a listed subscreen has one, the always-actions of the path
    // below the root (whose own ran for this transition) run first, as a render runs them.
    let conditions = needs_conditions(state, &info, full_path.len());
    if conditions {
        let mut frames = Frames::untracked();
        for (index, def) in info.path_defs.iter().enumerate() {
            frames.enter_screen(def, false)?;
            if index >= 1 {
                if let Some(actions) = &def.always_actions {
                    // an error they add is logged, not answered: it would make the menu data a 500
                    let mark = ExecutionContext::with_current(|ec| ec.message().error_mark());
                    // a proposed screen's actions run with authorization required (SEC-AI-102)
                    let _required = super::render::require_if_proposed(def);
                    let result = actions.run();
                    let added = mark
                        .and_then(|mark| ExecutionContext::with_current(|ec| ec.message().take_errors_since(mark)))
                        .unwrap_or_default();
                    if let Err(err) = result {
                        tracing::warn!(
                            "Error running always-actions in {} while getting menu data: {err}",
                            def.location
                        );
                    } else if !added.is_empty() {
                        tracing::warn!(
                            "Errors from always-actions in {} while getting menu data: {}",
                            def.location,
                            added.join("; ")
                        );
                    }
                }
            }
        }
    }
    // Menu titles may come from values set in pre-actions: they run for every screen except the first two
    // (webroot and the app root), and an error in one is logged and does not stop the menu.
    {
        let mut frames = Frames::untracked();
        for (index, def) in info.path_defs.iter().enumerate() {
            frames.enter_screen(def, false)?;
            if index >= 2 {
                if let Some(actions) = &def.pre_actions {
                    let _required = super::render::require_if_proposed(def);
                    if let Err(err) = actions.run() {
                        tracing::warn!(
                            "Error running pre-actions in {} while getting menu data: {err}",
                            def.location
                        );
                    }
                }
            }
        }
    }

    let mut current_path = String::new();
    let mut out: Vec<Value> = Vec::new();
    let mut cur_screen = std::sync::Arc::clone(info.path_defs.first().ok_or_else(|| Error::new("no root screen"))?);
    for index in 0..full_path.len() - 1 {
        let path_item = &full_path[index];
        let next_item = &full_path[index + 1];
        current_path.push('/');
        current_path.push_str(&super::url::encode_path_name(path_item));
        let Some(ssi) = cur_screen.subscreens.get(path_item).cloned() else {
            break;
        };
        cur_screen = state.facade.screen_def(&state.factory, &ssi.location)?;

        let mut subscreens: Vec<Value> = Vec::new();
        for item in cur_screen.subscreens.sorted() {
            if !item.is_valid_in_current_context() {
                continue;
            }
            // the item on the path shows whatever its menu-include says; its menuInclude flag still says it
            let on_path = &item.name == next_item;
            let in_menu = included(&cur_screen.location, item);
            if !in_menu && !on_path {
                continue;
            }
            let parents = &info.path_defs[..(index + 2).min(info.path_defs.len())];
            match subscreen_entry(
                state,
                parents,
                &full_path[..=index],
                &current_path,
                item,
                next_item,
                in_menu,
            ) {
                Ok(Some(entry)) => {
                    // the entry loaded the item's screen, so this finds it cached
                    let shown = !conditions
                        || state
                            .facade
                            .screen_def(&state.factory, &item.location)
                            .is_ok_and(|target| offered(&cur_screen.location, item, &target));
                    if shown {
                        subscreens.push(entry);
                    }
                }
                Ok(None) => {}
                Err(err) => leave_out(&cur_screen.location, item, &err),
            }
        }

        let cur_url = urls::resolve_names_at(state, full_path[..=index].to_vec(), 0, true)?;
        let sui = match &cur_url.info {
            Some(sui) => std::sync::Arc::clone(sui),
            None => ScreenUrlInfo::resolve(&state.factory, &root, &full_path[..=index], 0)?,
        };
        let (image, image_type) = menu_image(state, sui.menu_image.as_ref());
        let localized = l10n::localize(&ssi.menu_title)?;
        let title = if localized.is_empty() {
            l10n::localize(&cur_screen.default_menu_title)?
        } else {
            localized
        };
        let mut entry = Item::default();
        entry.insert("name".into(), text(path_item));
        entry.insert("title".into(), text(&title));
        entry.insert("subscreens".into(), Value::List(subscreens));
        entry.insert("path".into(), text(&current_path));
        let query = cur_url.parameter_string();
        entry.insert(
            "pathWithParams".into(),
            text(&if query.is_empty() {
                current_path.clone()
            } else {
                format!("{current_path}?{query}")
            }),
        );
        entry.insert("hasTabMenu".into(), Value::Bool(cur_screen.has_tab_menu));
        entry.insert("renderModes".into(), render_modes(&cur_screen.render_modes));
        entry.insert("image".into(), image);
        entry.insert("imageType".into(), image_type);
        insert_density(&mut entry, &cur_screen);
        out.push(Value::Map(entry));
    }

    let mut last_url = urls::build_url_names(state, full_path.clone())?;
    for (name, value) in web::parameters() {
        last_url.add_parameter(&name, &value);
    }
    current_path.push('/');
    current_path.push_str(&super::url::encode_path_name(&last_item));
    let last_path = current_path.clone();
    let query = last_url.parameter_string();
    if !query.is_empty() {
        current_path.push('?');
        current_path.push_str(&query);
    }
    // The image of the full path, every name explicit as for the entries above: the request's own walk counts a default
    // subscreen as not explicit, so at an app's root (/apps/Hub reaching Home) its image would be the app's, not Home's.
    let last_image = last_url
        .info
        .as_ref()
        .map_or(info.menu_image.as_ref(), |sui| sui.menu_image.as_ref());
    let (image, image_type) = menu_image(state, last_image);
    let last_ssi = cur_screen.subscreens.get(&last_item);
    let localized = match last_ssi {
        Some(item) => l10n::localize(&item.menu_title)?,
        None => String::new(),
    };
    let mut last_title = if localized.is_empty() {
        l10n::localize(&info.target_screen.default_menu_title)?
    } else {
        localized
    };
    if last_title.contains("${") {
        last_title = script::expand(&last_title)?;
    }
    let extra_encoded: Vec<Value> = extra
        .iter()
        .map(|name| text(&super::url::encode_path_name(name)))
        .collect();
    let mut entry = Item::default();
    entry.insert("name".into(), text(&last_item));
    entry.insert("title".into(), text(&last_title));
    entry.insert("path".into(), text(&last_path));
    entry.insert("pathWithParams".into(), text(&current_path));
    entry.insert("image".into(), image);
    entry.insert("imageType".into(), image_type);
    entry.insert("extraPathList".into(), Value::List(extra_encoded));
    entry.insert(
        "screenDocList".into(),
        Value::List(super::saved_finds::screen_document_list(&info.target_screen.location)?),
    );
    entry.insert("renderModes".into(), render_modes(&info.target_screen.render_modes));
    insert_density(&mut entry, &info.target_screen);
    // the declared actions and panels of the screen (ext.rs): the terminal client lists them, a .qvue renders them
    let (ext_actions, ext_panels) = super::ext::items_json(&state.factory, &root, &full_path, &info.target_screen)?;
    entry.insert("extActions".into(), ext_actions);
    entry.insert("extPanels".into(), ext_panels);
    let mut saved_finds = Vec::new();
    for (name, form) in &info.target_screen.forms {
        if !form.list || !form.saved_finds {
            continue;
        }
        let location = super::saved_finds::form_location(&info.target_screen.location, true, name);
        let saved = super::saved_finds::saved_find_location(&location, form.dynamic, &form.find_key);
        let active = script::context_value("formListFindId")
            .map(|value| moqui_lib::object::to_plain_string(&value))
            .filter(|id| !id.is_empty())
            .or(super::saved_finds::user_default_find_id(&info.target_screen.location)?);
        for find in super::saved_finds::user_form_list_finds(&saved)? {
            let Value::Map(find) = find else { continue };
            let Some(Value::Map(row)) = find.get("formListFind") else {
                continue;
            };
            let id = row
                .get("formListFindId")
                .map(moqui_lib::object::to_plain_string)
                .unwrap_or_default();
            let mut item = Item::default();
            item.insert("name".into(), text(&id));
            item.insert("title".into(), find.get("description").cloned().unwrap_or(Value::Null));
            item.insert("image".into(), entry.get("image").cloned().unwrap_or(Value::Null));
            item.insert(
                "imageType".into(),
                entry.get("imageType").cloned().unwrap_or(Value::Null),
            );
            item.insert("path".into(), text(&last_path));
            item.insert(
                "pathWithParams".into(),
                text(&format!("{last_path}?formListFindId={id}")),
            );
            if active.as_deref() == Some(id.as_str()) {
                item.insert("active".into(), Value::Bool(true));
            }
            saved_finds.push(Value::Map(item));
        }
    }
    entry.insert("savedFinds".into(), Value::List(saved_finds));
    out.push(Value::Map(entry));
    Ok(Some(Value::List(out)))
}

/// Whether a subscreen listed in the menu data of `info` (the subscreens of each of its first `levels` screens below the
/// root) has its own condition ([`has_offer_condition`]), so the menu needs the context a render would have. A screen
/// that does not load is left for the entry to report.
fn needs_conditions(state: &RenderState, info: &ScreenUrlInfo, levels: usize) -> bool {
    info.path_defs
        .iter()
        .skip(1)
        .take(levels.saturating_sub(1))
        .any(|level| {
            level.subscreens.sorted().into_iter().any(|item| {
                state
                    .facade
                    .screen_def(&state.factory, &item.location)
                    .is_ok_and(|target| has_offer_condition(&target))
            })
        })
}

/// The screen's `density` (handheld), which the shell applies to the screen and the screens below it; none when unset.
fn insert_density(entry: &mut Item, screen: &super::definition::ScreenDef) {
    if let Some(density) = &screen.density {
        entry.insert("density".into(), text(density));
    }
}

fn render_modes(modes: &[String]) -> Value {
    Value::List(modes.iter().map(|mode| text(mode)).collect())
}
