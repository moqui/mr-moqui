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

//! The menu items of a screen, and whether the user may view a screen.

use std::sync::Arc;

use super::active_def;
use crate::context::{Error, ExecutionContext};
use crate::screen::definition::ScreenDef;
use crate::screen::render::RenderState;
use crate::screen::url::ScreenUrlInfo;
use crate::{script, user};

/// One item of a screen's menu: a subscreen with the URL of its minimal path.
#[derive(Clone, Debug, Default)]
pub struct MenuEntry {
    pub name: String,
    /// The subscreen takes parameters, so a list of applications leaves it out.
    pub has_parameters: bool,
    pub title: String,
    pub url: String,
    pub disabled: bool,
    pub in_path: bool,
    pub image: String,
    pub image_type: String,
}

/// `getMenuSubscreensItems` of the active screen with what the macros ask of each item's URL: the
/// subscreens that appear in the menu, in menu order, that the user may view.
///
/// # Errors
///
/// Returns an error when a path cannot be resolved or a `disable-when` expression fails.
pub fn menu_items(state: &RenderState, screen_path: Option<&str>) -> Result<Vec<MenuEntry>, Error> {
    let root = state
        .url_info
        .path_defs
        .first()
        .map(|root| root.location.clone())
        .ok_or_else(|| Error::new("no root screen"))?;
    // The items of another screen: the path is resolved from the root, and its items are paths below it.
    let (def, prefix) = match screen_path {
        Some(path) => {
            let names = crate::screen::url::resolve_screen_path(&[], path);
            let info = ScreenUrlInfo::resolve(&state.factory, &root, &names, 0)?;
            // The screen the path names, not the default subscreen the walk goes on to.
            let named = info.path_defs.get(names.len()).unwrap_or(&info.target_screen);
            (Arc::clone(named), Some(names))
        }
        None => (active_def(state), None),
    };
    let mut out = Vec::new();
    for item in def
        .subscreens
        .sorted()
        .into_iter()
        .filter(|item| item.menu_include && item.is_valid_in_current_context())
    {
        let url = match &prefix {
            Some(names) => {
                let mut full = names.clone();
                full.push(item.name.clone());
                crate::screen::urls::build_url_names(state, full)?
            }
            None => crate::screen::urls::build_url(state, &item.name)?,
        };
        if !url.exists {
            continue;
        }
        let Ok(target) = state.facade.screen_def(&state.factory, &item.location) else {
            continue;
        };
        if !screen_permitted(&target) {
            continue;
        }
        let disabled = match &item.disable_when {
            Some(expression) => script::eval_condition(expression)?,
            None => false,
        };
        // The image of the walk the URL was built from: resolving the full path again would count its default
        // subscreens as explicit, so a default's image would replace the item's own, unlike Java's `sui.menuImage`.
        let image = url
            .info
            .as_ref()
            .and_then(|info| info.menu_image.clone())
            .unwrap_or_default();
        out.push(MenuEntry {
            name: item.name.clone(),
            has_parameters: !target.parameters.is_empty(),
            title: item.menu_title.clone(),
            url: url.minimal_path_url_with_params(),
            disabled,
            in_path: state.url_info.full_path.starts_with(&url.minimal_names),
            image: image.0,
            image_type: image.1,
        });
    }
    Ok(out)
}

/// Whether the user may view a screen: a screen that requires authentication needs a user and
/// an authorization record. Without an execution context nothing is permitted.
pub(in crate::screen) fn screen_permitted(def: &ScreenDef) -> bool {
    let required = def.authentication == crate::screen::definition::Authentication::Required;
    if required && user::current_username().ok().flatten().is_none() {
        return false;
    }
    ExecutionContext::with_current(|ec| {
        match ec.push_checked("AT_XML_SCREEN", &def.location, "view", required, false, None) {
            Ok(()) => {
                // A check, not an access: Java's isPermitted, which is not counted as a hit.
                ec.untrack_hit();
                let _ = ec.pop();
                true
            }
            Err(_) => false,
        }
    })
    .unwrap_or(false)
}
