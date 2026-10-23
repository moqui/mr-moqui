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

//! URL instances for `sri.buildUrl` and `sri.makeUrlByType`: a screen path or a transition resolved
//! against the screen being rendered, with its parameters.
//!
//! Not ported: `ScreenPathAlias` and `content` URLs.

use std::sync::Arc;

use indexmap::IndexMap;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use super::definition::{ResponseKind, ScreenDef, TransitionDef};
use super::render::parameter_value;
use super::sri::{self, active_screen_path, NodeLit};
use super::url::{encode_path, resolve_screen_path, ScreenUrlInfo};
use super::RenderState;
use crate::context::{Error, ErrorKind};
use crate::{script, web};

/// A URL for a link, with the parameters added to it.
#[derive(Clone, Debug, Default)]
pub struct UrlInstance {
    /// Path names from the root, with the default subscreens a plain path leads to.
    pub names: Vec<String>,
    /// Path names as given, without the defaults.
    pub minimal_names: Vec<String>,
    pub base: String,
    /// A url-type plain URL, used as is.
    pub plain: Option<String>,
    pub parameters: IndexMap<String, String>,
    /// False for a transition that is not read-only.
    pub read_only: bool,
    pub exists: bool,
    pub is_transition: bool,
    /// The HTTP method of the transition, lower case, empty for a screen.
    pub transition_method: String,
    /// The resolved path, shared with the URL cache: its screens and the transition it ends on, for `getDisableLink`.
    pub info: Option<Arc<ScreenUrlInfo>>,
}

impl UrlInstance {
    /// The screens of the resolved path, empty when the path did not resolve.
    pub fn path_defs(&self) -> &[Arc<ScreenDef>] {
        self.info.as_ref().map_or(&[], |info| info.path_defs.as_slice())
    }

    /// The transition the resolved path ends on.
    pub fn transition(&self) -> Option<&TransitionDef> {
        self.info.as_ref()?.transition(None)
    }

    /// The path with no host.
    pub fn path(&self) -> String {
        match &self.plain {
            Some(plain) => plain.clone(),
            None => encode_path(&self.names),
        }
    }

    pub fn url(&self) -> String {
        match &self.plain {
            Some(plain) if plain.contains("://") || plain.starts_with("//") => plain.clone(),
            Some(plain) => format!("{}{plain}", self.base.trim_end_matches('/')),
            None => format!("{}{}", self.base.trim_end_matches('/'), encode_path(&self.names)),
        }
    }

    pub fn minimal_path_url(&self) -> String {
        match &self.plain {
            Some(_) => self.url(),
            None => format!(
                "{}{}",
                self.base.trim_end_matches('/'),
                encode_path(&self.minimal_names)
            ),
        }
    }

    pub fn minimal_path_url_with_params(&self) -> String {
        join_query(self.minimal_path_url(), &self.parameter_string())
    }

    pub fn parameter_string(&self) -> String {
        let mut out = String::new();
        for (key, value) in &self.parameters {
            if value.is_empty() || key == "moquiSessionToken" {
                continue;
            }
            if !out.is_empty() {
                out.push('&');
            }
            out.push_str(&moqui_lib::string::url_encode_if_needed(key));
            out.push('=');
            out.push_str(&moqui_lib::string::url_encode_if_needed(value));
        }
        out
    }

    pub fn path_with_params(&self) -> String {
        join_query(self.path(), &self.parameter_string())
    }

    pub fn url_with_params(&self, extension: &str) -> String {
        let mut url = self.url();
        if !extension.is_empty() {
            url.push('.');
            url.push_str(extension);
        }
        join_query(url, &self.parameter_string())
    }

    /// `getDisableLink`: the path is not a screen or transition, the transition's condition does not hold, or the user may
    /// not use it (Java's `UrlInstance.getDisableLink`).
    ///
    /// # Errors
    ///
    /// Returns an error when the transition's condition fails to run.
    pub fn disable_link(&self) -> Result<bool, Error> {
        if !self.exists {
            return Ok(true);
        }
        if let Some(condition) = self.transition().and_then(|transition| transition.condition.as_ref()) {
            if !super::render::check_condition(condition)? {
                return Ok(true);
            }
        }
        Ok(!self.permitted())
    }

    /// `isPermitted`: each screen of the path with its parents on the artifact stack, and the transition with its action.
    fn permitted(&self) -> bool {
        let path_defs = self.path_defs();
        let Some(last) = path_defs.len().checked_sub(1) else {
            return true;
        };
        let transition = self.transition();
        let mut frames = super::render::Frames::default();
        for (index, def) in path_defs.iter().enumerate() {
            if index == last && transition.is_none() {
                return sri::screen_permitted(def);
            }
            if frames.push_screen(def, false).is_err() {
                return false;
            }
        }
        let Some(transition) = transition else {
            return true;
        };
        let target = &path_defs[last];
        let required = target.authentication == super::definition::Authentication::Required;
        if required && crate::user::current_username().ok().flatten().is_none() {
            return false;
        }
        frames
            .push(
                "AT_XML_SCREEN_TRANS",
                &format!("{}/{}", target.location, transition.name),
                &super::render::transition_authz_action(transition),
                required,
            )
            .is_ok()
    }

    pub fn add_parameter(&mut self, name: &str, value: &Value) {
        if name.is_empty() || value.is_null() {
            return;
        }
        self.parameters.insert(name.to_string(), to_plain_string(value));
    }

    pub fn remove_parameter(&mut self, name: &str) {
        self.parameters.shift_remove(name);
    }

    /// The parameters without the ones that belong to one request.
    pub fn pass_through_parameters(&self) -> IndexMap<String, String> {
        let mut map = self.parameters.clone();
        for name in [
            "moquiFormName",
            "moquiSessionToken",
            "lastStandalone",
            "formListFindId",
            "moquiRequestStartTime",
            "webrootTT",
        ] {
            map.shift_remove(name);
        }
        map
    }
}

fn join_query(mut base: String, query: &str) -> String {
    if !query.is_empty() {
        base.push('?');
        base.push_str(query);
    }
    base
}

/// `sri.buildUrl(subscreenPath)`: a path from the active screen.
///
/// # Errors
///
/// Returns an error when an expression in the path does not expand.
pub fn build_url(state: &RenderState, subscreen_path: &str) -> Result<UrlInstance, Error> {
    let expanded = if subscreen_path.contains("${") {
        script::expand_localized(subscreen_path)?
    } else {
        subscreen_path.to_string()
    };
    let names = resolve_screen_path(&active_screen_path(state), &expanded);
    resolve_names(state, names)
}

/// A URL for path names from the root.
///
/// # Errors
///
/// Returns an error when the names cannot be resolved for a reason other than a missing screen.
pub fn build_url_names(state: &RenderState, names: Vec<String>) -> Result<UrlInstance, Error> {
    resolve_names(state, names)
}

/// `sri.buildUrlFromTarget`: a path from the target screen of the request.
///
/// # Errors
///
/// Same as [`build_url`].
pub fn build_url_from_target(state: &RenderState, subscreen_path: &str) -> Result<UrlInstance, Error> {
    let expanded = script::expand_localized(subscreen_path)?;
    let names = resolve_screen_path(&state.url_info.pre_transition_path, &expanded);
    resolve_names(state, names)
}

/// `sri.getScreenUrlInstance`/`getCurrentScreenUrl`: the URL of the request being rendered.
///
/// # Errors
///
/// Returns an error when the request's path cannot be resolved again.
pub fn current_url(state: &RenderState) -> Result<UrlInstance, Error> {
    resolve_names_at(state, state.url_info.full_path.clone(), state.url_info.last_standalone)
}

/// A built URL resolves with `lastStandalone` 0, as Java's `sri.buildUrl`: the request's value changes which screens
/// render, and so the `menu_image` the walk picks, but not the URL.
fn resolve_names(state: &RenderState, names: Vec<String>) -> Result<UrlInstance, Error> {
    resolve_names_at(state, names, 0)
}

/// A URL for path names from the root, resolved with `last_standalone`. That only changes which screens render, so
/// the URL is the same for any value, and the menu reads its `menu_image` from the shared walk.
pub(super) fn resolve_names_at(
    state: &RenderState,
    names: Vec<String>,
    last_standalone: i32,
) -> Result<UrlInstance, Error> {
    let root = state
        .url_info
        .path_defs
        .first()
        .map(|def| def.location.as_str())
        .ok_or_else(|| Error::new("no root screen"))?;
    let resolved = ScreenUrlInfo::resolve(&state.factory, root, &names, last_standalone);
    let mut url = UrlInstance {
        minimal_names: names.clone(),
        names,
        base: web::base_url(),
        exists: true,
        read_only: true,
        ..UrlInstance::default()
    };
    match resolved {
        Ok(info) => {
            url.names = info.full_path.clone();
            url.minimal_names = if info.always_use_full_path {
                info.full_path.clone()
            } else {
                info.minimal_path.clone()
            };
            for item in info.parameter_items.values() {
                if let Some(value) = parameter_value(item)? {
                    let text = to_plain_string(&value);
                    if !text.is_empty() {
                        url.parameters.insert(item.name.clone(), text);
                    }
                }
            }
            if let Some(transition) = info.transition(None) {
                url.is_transition = true;
                url.transition_method = transition.method.to_ascii_lowercase();
                url.read_only = transition.read_only;
                for item in &transition.parameters {
                    if let Some(value) = parameter_value(item)? {
                        let text = to_plain_string(&value);
                        if !text.is_empty() {
                            url.parameters.insert(item.name.clone(), text);
                        }
                    }
                }
                // The in-parameters of the transition's one service from the context, so a link to a transition such
                // as `deleteItem` in a form-list row carries that row's keys (Java's `UrlInstance.getParameterMap`).
                if let Some(service) = &transition.single_service {
                    for name in service_parameter_names(service) {
                        let value = script::context_value(&name)
                            .filter(|value| !is_empty(value))
                            .or_else(|| web::parameter(&name));
                        if let Some(value) = value {
                            let text = to_plain_string(&value);
                            if !text.is_empty() {
                                url.parameters.insert(name, text);
                            }
                        }
                    }
                }
            } else if info.target_transition_name.is_some() {
                url.is_transition = true;
                url.read_only = false;
            }
            url.info = Some(info);
        }
        Err(err) if err.kind() == ErrorKind::NotFound => url.exists = false,
        Err(err) => return Err(err),
    }
    Ok(url)
}

/// The in-parameter names of a service, or the primary key fields of the entity of an implicit entity-auto service.
fn service_parameter_names(service: &str) -> Vec<String> {
    if let Some(names) = crate::service::call::service_in_parameter_names(service) {
        return names;
    }
    crate::service::call::entity_auto_noun(service)
        .and_then(|entity| super::form::entity_shape(&entity).ok())
        .map(|shape| shape.pk_fields.clone())
        .unwrap_or_default()
}

/// A link target for a server-static template: where it goes and what it is, with no parameters, since the template evaluates those.
#[derive(Clone, Debug, Default)]
pub struct StaticUrl {
    pub path: String,
    /// The path leads to a screen or transition of this server.
    pub exists: bool,
    pub is_transition: bool,
    /// A screen, not a transition (and not a URL of another kind).
    pub is_screen: bool,
    pub read_only: bool,
    /// The HTTP method of the transition, lower case.
    pub method: String,
}

/// The target of a link written into a static template: the name resolved against the screen being rendered.
///
/// A screen that is not mounted in the tree has no path from the root. A name that is a transition of the active screen is still
/// found, so the compatibility report can check a screen file by itself.
///
/// # Errors
///
/// Returns an error when the names resolve for a reason other than a missing screen.
pub fn static_url(state: &RenderState, text: &str, url_type: &str) -> Result<StaticUrl, Error> {
    if text.contains("${") || !matches!(url_type, "transition" | "screen") {
        return Ok(StaticUrl {
            path: text.to_string(),
            read_only: true,
            ..StaticUrl::default()
        });
    }
    let active = active_screen_path(state);
    let names = resolve_screen_path(&active, text);
    let root = state
        .url_info
        .path_defs
        .first()
        .map(|def| def.location.clone())
        .ok_or_else(|| Error::new("no root screen"))?;
    match ScreenUrlInfo::resolve(&state.factory, &root, &names, state.url_info.last_standalone) {
        Ok(info) => {
            let transition = info.transition(None);
            let is_transition = transition.is_some() || info.target_transition_name.is_some();
            Ok(StaticUrl {
                path: encode_path(&info.full_path),
                exists: true,
                is_transition,
                is_screen: !is_transition,
                read_only: transition.map_or(info.target_transition_name.is_none(), |item| item.read_only),
                method: transition
                    .map(|item| item.method.to_ascii_lowercase())
                    .unwrap_or_default(),
            })
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {
            let def = sri::active_def(state);
            let local = names
                .strip_prefix(active.as_slice())
                .and_then(|rest| (rest.len() == 1).then(|| rest[0].clone()))
                .and_then(|name| def.transition(&name, None).cloned());
            Ok(StaticUrl {
                path: encode_path(&names),
                exists: local.is_some(),
                is_transition: local.is_some(),
                is_screen: false,
                read_only: local.as_ref().is_none_or(|item| item.read_only),
                method: local.map(|item| item.method.to_ascii_lowercase()).unwrap_or_default(),
            })
        }
        Err(err) => Err(err),
    }
}

/// `sri.makeUrlByType(url, urlType, parameterParent, expandTransitionUrl)`.
///
/// # Errors
///
/// Returns an error for a `content` URL, or when an expression fails.
pub fn make_url_by_type(
    state: &RenderState,
    orig_url: &str,
    url_type: &str,
    parameter_parent: Option<&NodeLit>,
    expand_transition_url: bool,
) -> Result<UrlInstance, Error> {
    let kind = script::expand_localized(url_type)?;
    let mut url = match kind.as_str() {
        "transition" | "screen" => {
            let url = build_url(state, orig_url)?;
            if expand_transition_url {
                expand_transition_alias(state, url)?
            } else {
                url
            }
        }
        "content" => return Err(Error::new("The url-type of content is not yet supported")),
        _ => {
            let plain = script::expand_localized(orig_url)?;
            UrlInstance {
                plain: Some(plain),
                base: web::base_url(),
                exists: true,
                read_only: true,
                ..UrlInstance::default()
            }
        }
    };
    if let Some(parent) = parameter_parent {
        let map_expr = parent.attr("parameter-map");
        if !map_expr.is_empty() {
            if let Some(map) = script::value_map(&script::eval_value(map_expr)?) {
                for (key, value) in map {
                    if key != "context" {
                        url.add_parameter(&key, &value);
                    }
                }
            }
        }
        for parameter in parent.children.iter().filter(|child| child.name == "parameter") {
            let name = parameter.attr("name");
            let from = if parameter.attr("from").is_empty() {
                name
            } else {
                parameter.attr("from")
            };
            let value = sri::context_value(from, parameter.attr("value"))?;
            if !is_empty(&value) || !value.is_null() {
                url.add_parameter(name, &value);
            }
        }
    }
    Ok(url)
}

/// Java's `UrlInstance.expandTransitionAliasUrl`: a transition with no condition, actions, service or conditional
/// response, whose default response is a screen-path URL, is an alias for that screen. The link goes to the screen
/// directly, with the parameters of the screens on its path from the context (OrderDetail's `shipmentDetail` carries
/// the `shipmentId` ShipmentDetail declares) and the response's own parameters.
fn expand_transition_alias(state: &RenderState, url: UrlInstance) -> Result<UrlInstance, Error> {
    let Some(info) = url.info.clone() else {
        return Ok(url);
    };
    let Some(transition) = info.transition(None) else {
        return Ok(url);
    };
    let response = &transition.default_response;
    if transition.condition.is_some()
        || transition.actions.is_some()
        || transition.single_service.is_some()
        || !transition.conditional_responses.is_empty()
        || response.kind != ResponseKind::Url
        || response.url_type != "screen-path"
    {
        return Ok(url);
    }
    let mut alias_parameters = IndexMap::new();
    for parameter in &response.parameters {
        if let Some(value) = parameter_value(parameter)? {
            alias_parameters.insert(parameter.name.clone(), to_plain_string(&value));
        }
    }
    if let Some(expression) = &response.parameter_map {
        for (name, value) in script::eval_map(expression)? {
            alias_parameters.insert(name, to_plain_string(&value));
        }
    }
    let target = if response.url.contains("${") {
        script::expand(&response.url)?
    } else {
        response.url.clone()
    };
    let mut alias = resolve_names(state, resolve_screen_path(&info.pre_transition_path, &target))?;
    alias.parameters.extend(alias_parameters);
    Ok(alias)
}

/// `sri.isAnchorLink(linkNode, urlInstance)`.
pub fn is_anchor_link(link: &NodeLit, url: &UrlInstance) -> bool {
    let link_type = link.attr("link-type");
    let url_type = link.attr("url-type");
    link_type == "anchor"
        || link_type == "anchor-button"
        || ((link_type.is_empty() || link_type == "auto")
            && ((!url_type.is_empty() && url_type != "transition") || url.read_only))
}

/// `sri.activeInCurrentMenu`: whether a subscreen of the active screen that appears in the menu is
/// on the path of the request.
pub fn active_in_current_menu(state: &RenderState) -> bool {
    let def = sri::active_def(state);
    let path = &state.url_info.full_path;
    def.subscreens
        .items
        .iter()
        .filter(|item| item.menu_include)
        .any(|item| {
            let mut names = active_screen_path(state);
            names.push(item.name.clone());
            path.len() >= names.len() && path[..names.len()] == names[..]
        })
}
