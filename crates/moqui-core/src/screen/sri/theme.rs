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

//! The theme of the render: its id, resource values and icon classes.
//!
//! A theme may name a parent (`ScreenTheme.parentScreenThemeId`, docs/ui/Qvt-Client.md, Themes). A resource type the
//! theme has any row of is the theme's own, so its rows replace the parent's (one empty row hides a parent's value); a
//! type it has no row of comes from the parent, and so on up the chain ([`theme_chain`]). Its icons are its own
//! patterns, then the parent's.

use std::collections::HashSet;
use std::sync::Mutex;

use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;

use crate::context::{Error, ExecutionContext};
use crate::screen::render::RenderState;
use crate::user;

/// `sri.getCurrentThemeId`: the user's theme for the screen theme type, else one of the user's groups', else the type's
/// `enumCode`, else a theme of the type with `DEFAULT` in its id. Found once for the render.
pub fn current_theme_id(state: &RenderState) -> String {
    if let Some(found) = state.theme_id.borrow().as_ref() {
        return found.clone();
    }
    let found = find_theme_id(state);
    *state.theme_id.borrow_mut() = Some(found.clone());
    found
}

fn find_theme_id(state: &RenderState) -> String {
    let mut theme_type = String::new();
    for def in state.url_info.render_defs() {
        if !def.screen_theme_type.is_empty() {
            theme_type.clone_from(&def.screen_theme_type);
        }
    }
    if theme_type.is_empty() {
        theme_type = "STT_INTERNAL".to_string();
    }
    let groups: Vec<String> = user::current_user_groups()
        .map(|groups| groups.iter().cloned().collect())
        .unwrap_or_default();
    theme_for(user::current_user_id().ok().flatten().as_deref(), groups, &theme_type).unwrap_or_default()
}

/// The screen theme type of the `/qapps` shell (`qapps.xml`), and the theme it falls back to.
pub const QUASAR_THEME_TYPE: &str = "STT_INTERNAL_QUASAR";
pub const DEFAULT_QUASAR_THEME: &str = "DEFAULT_QUASAR";

/// The theme of `user_id` for `theme_type` outside a render, found as a render finds the current user's
/// ([`find_theme_id`]), with the user's own groups; `fallback` when none is found. For the shell's theme values read
/// by a service, such as the counts service of `ShellServices` (BOSS seeds them on `DEFAULT_QUASAR`).
pub fn user_theme_id(user_id: Option<&str>, theme_type: &str, fallback: &str) -> String {
    theme_for(user_id, groups_of(user_id), theme_type).unwrap_or_else(|| fallback.to_string())
}

/// The theme `user_id` gets for `theme_type` without a `UserScreenTheme` of their own: one of their groups', else the
/// type's `enumCode`, else a `DEFAULT` theme of the type; `fallback` when none is found. A user who chooses this theme
/// needs no row, so a later change of the group's theme still reaches them.
pub(crate) fn inherited_theme_id(user_id: Option<&str>, theme_type: &str, fallback: &str) -> String {
    theme_for(None, groups_of(user_id), theme_type).unwrap_or_else(|| fallback.to_string())
}

fn groups_of(user_id: Option<&str>) -> Vec<String> {
    crate::user::user_group_ids(user_id)
        .map(|groups| groups.into_iter().collect())
        .unwrap_or_else(|_| vec!["ALL_USERS".to_string()])
}

/// The user's theme for the type, else one of the groups', else the type's `enumCode`, else a theme of the type with
/// `DEFAULT` in its id.
fn theme_for(user_id: Option<&str>, mut groups: Vec<String>, theme_type: &str) -> Option<String> {
    let facade = ExecutionContext::with_current(|ec| ec.factory().entity()).flatten()?;
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let text_of = |row: Option<crate::entity::EntityValue>, field: &str| {
        row.and_then(|row| row.get(field).map(to_plain_string))
            .filter(|text| !text.is_empty())
    };
    let mut theme = None;
    if let Some(user_id) = user_id.filter(|id| !id.is_empty()) {
        theme = text_of(
            facade.find("moqui.security.UserScreenTheme").ok().and_then(|find| {
                find.eq("userId", Value::string(user_id))
                    .eq("screenThemeTypeEnumId", Value::string(theme_type))
                    .one()
                    .ok()
                    .flatten()
            }),
            "screenThemeId",
        );
    }
    if theme.is_none() {
        // Reverse id order, so ALL_USERS comes after the named groups at the same sequence.
        groups.sort_unstable_by(|left, right| right.cmp(left));
        let rows = facade
            .find("moqui.security.UserGroupScreenTheme")
            .ok()
            .and_then(|find| {
                find.condition(moqui_db::sql::Condition::is_in(
                    "userGroupId",
                    groups.into_iter().map(Value::String).collect(),
                ))
                .eq("screenThemeTypeEnumId", Value::string(theme_type))
                .order_by("sequenceNum", false)
                .order_by("userGroupId", true)
                .list()
                .ok()
            });
        theme = text_of(rows.and_then(|rows| rows.values().first().cloned()), "screenThemeId");
    }
    if theme.is_none() {
        theme = text_of(
            facade
                .find("moqui.basic.Enumeration")
                .ok()
                .and_then(|find| find.eq("enumId", Value::string(theme_type)).one().ok().flatten()),
            "enumCode",
        );
    }
    if theme.is_none() {
        let rows = facade.find("moqui.screen.ScreenTheme").ok().and_then(|find| {
            find.eq("screenThemeTypeEnumId", Value::string(theme_type))
                .condition(moqui_db::sql::Condition::like(
                    "screenThemeId",
                    Value::string("%DEFAULT%"),
                ))
                .list()
                .ok()
        });
        theme = text_of(rows.and_then(|rows| rows.values().first().cloned()), "screenThemeId");
    }
    theme
}

/// The most themes a chain holds, the theme itself included; a longer one is cut there.
const MAX_THEME_CHAIN: usize = 8;

/// The theme chains cut for a cycle or for their length, each logged once while the server runs.
#[derive(Default)]
struct CutChains(Mutex<HashSet<String>>);

fn first_cut(theme_id: &str) -> bool {
    ExecutionContext::with_current(|ec| {
        let cut = ec.factory().extension(|_| CutChains::default());
        let first = lock(&cut.0).insert(theme_id.to_string());
        first
    })
    .unwrap_or(true)
}

/// `theme_id` and its ancestors by `ScreenTheme.parentScreenThemeId`, the theme first. A parent that is already in the
/// chain (a cycle), or one past [`MAX_THEME_CHAIN`] themes, ends it there, logged once per theme; a parent that cannot
/// be read ends it too. Just `theme_id` without an entity facade.
pub fn theme_chain(theme_id: &str) -> Vec<String> {
    let mut chain = vec![theme_id.to_string()];
    let Some(facade) = ExecutionContext::with_current(|ec| ec.factory().entity()).flatten() else {
        return chain;
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    while let Some(current) = chain.last() {
        let parent = match facade
            .find("moqui.screen.ScreenTheme")
            .and_then(|find| find.eq("screenThemeId", Value::string(current)).use_cache(true).one())
        {
            Ok(row) => row
                .and_then(|row| row.get("parentScreenThemeId").map(to_plain_string))
                .filter(|parent| !parent.is_empty()),
            Err(err) => {
                tracing::error!("Could not read the parent of theme {current}: {err}");
                None
            }
        };
        let Some(parent) = parent else { break };
        if chain.contains(&parent) || chain.len() >= MAX_THEME_CHAIN {
            if first_cut(theme_id) {
                tracing::error!(
                    "The parents of theme {theme_id} ({} then {parent}) are a cycle or longer than {MAX_THEME_CHAIN}; \
                     the chain ends at {current}",
                    chain.join(" > ")
                );
            }
            break;
        }
        chain.push(parent);
    }
    chain
}

/// The `ScreenThemeResource` values of `theme_id` for `resource_type`, in sequence, read outside a render: the theme's
/// own when it has any row of the type, else those of the nearest ancestor that has ([`theme_chain`]). An empty value
/// is left out, so one empty row hides the parent's values. Empty without an entity facade.
///
/// # Errors
///
/// Returns an error when the entity cannot be read.
pub fn theme_resource_values(theme_id: &str, resource_type: &str) -> Result<Vec<String>, Error> {
    let Some(facade) = ExecutionContext::with_current(|ec| ec.factory().entity()).flatten() else {
        return Ok(Vec::new());
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    for theme in theme_chain(theme_id) {
        let list = facade
            .find("moqui.screen.ScreenThemeResource")?
            .eq("screenThemeId", Value::string(&theme))
            .eq("resourceTypeEnumId", Value::string(resource_type))
            .order_by("sequenceNum", false)
            .list()?;
        if list.is_empty() {
            continue;
        }
        return Ok(list
            .iter()
            .filter_map(|row| row.get("resourceValue").map(to_plain_string))
            .filter(|text| !text.is_empty())
            .collect());
    }
    Ok(Vec::new())
}

/// `sri.getThemeValues(type, themeId)`: the `ScreenThemeResource` values in sequence, kept for the render for the current theme.
///
/// # Errors
///
/// Returns an error when the entity cannot be read.
pub fn get_theme_values(state: &RenderState, resource_type: &str, theme_id: &str) -> Result<Vec<String>, Error> {
    let current = theme_id.is_empty() || theme_id == "null";
    if current {
        if let Some(found) = state.theme_values.borrow().get(resource_type) {
            return Ok(found.clone());
        }
    }
    let theme = if current {
        current_theme_id(state)
    } else {
        theme_id.to_string()
    };
    let values = theme_resource_values(&theme, resource_type)?;
    if current {
        state
            .theme_values
            .borrow_mut()
            .insert(resource_type.to_string(), values.clone());
    }
    Ok(values)
}

/// `sri.getThemeIconClass(text)`: the icon class of the first `ScreenThemeIcon` pattern that matches, the theme's own
/// patterns before its parent's, kept by text for the theme.
///
/// # Errors
///
/// Returns an error when the entity cannot be read.
pub fn get_theme_icon_class(state: &RenderState, text: &str) -> Result<Option<String>, Error> {
    let theme = current_theme_id(state);
    if let Some(found) = state.facade.theme_icon(&theme, text) {
        return Ok(found);
    }
    let Some(facade) = ExecutionContext::with_current(|ec| ec.factory().entity()).flatten() else {
        return Ok(None);
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut icon = None;
    'chain: for owner in theme_chain(&theme) {
        let list = facade
            .find("moqui.screen.ScreenThemeIcon")?
            .eq("screenThemeId", Value::string(&owner))
            .list()?;
        for row in list.iter() {
            let pattern = row.get("textPattern").map(to_plain_string).unwrap_or_default();
            if moqui_lib::object::regex_full_match(text, &pattern).unwrap_or(false) {
                icon = row.get("iconClass").map(to_plain_string);
                break 'chain;
            }
        }
    }
    state.facade.put_theme_icon(&theme, text, icon.clone());
    Ok(icon)
}
