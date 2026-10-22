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
//! A transition with no condition, actions, or conditional response and a screen-path default response is an alias
//! for that response URL (`UrlInstance.expandTransitionAliasUrl`). `ScreenPathAlias` and `//` paths are resolved in
//! [`super::url::prepare_screen_path`]. `content` URLs are not supported, as in Java.

use std::sync::Arc;

use indexmap::IndexMap;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use super::definition::{ParameterDef, ResponseKind, ScreenDef, TransitionDef};
use super::render::parameter_value;
use super::sri::{self, active_screen_path, NodeLit};
use super::url::{encode_path, parse_last_standalone, resolve_screen_path, ScreenUrlInfo};
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

    /// The path with no host. A screen that does not exist is `#`, as Java's `getUrlWithBase`.
    /// A plain URL has no screen path, so this is empty, as Java's `getPath`.
    pub fn path(&self) -> String {
        if self.missing() {
            return "#".to_string();
        }
        match &self.plain {
            Some(_) => String::new(),
            None => encode_path(&self.names),
        }
    }

    /// The URL. A plain value is returned as written, including `mailto:` and a path that starts
    /// with `/`. The base is not prefixed. A screen path is the base plus the encoded names.
    pub fn url(&self) -> String {
        if self.missing() {
            return "#".to_string();
        }
        match &self.plain {
            Some(plain) => plain.clone(),
            None => format!("{}{}", self.base.trim_end_matches('/'), encode_path(&self.names)),
        }
    }

    /// The path of the screens only, without a transition or extra path names, as Java's `getScreenOnlyPath`.
    pub fn screen_only_path(&self) -> String {
        if self.missing() {
            return "#".to_string();
        }
        match (&self.plain, &self.info) {
            (Some(plain), _) => plain.clone(),
            (None, Some(info)) => encode_path(&info.pre_transition_path),
            (None, None) => encode_path(&self.names),
        }
    }

    pub fn minimal_path_url(&self) -> String {
        if self.missing() {
            return "#".to_string();
        }
        match &self.plain {
            Some(_) => self.url(),
            None => format!(
                "{}{}",
                self.base.trim_end_matches('/'),
                encode_path(&self.minimal_names)
            ),
        }
    }

    /// False when this is a plain URL, the screen is missing, or the target transition's default response is
    /// `url-type="plain"`, `type="none"`, or declares a parameter named `renderMode`. Java's `UrlInstance.isScreenUrl`.
    pub fn is_screen_url(&self) -> bool {
        if self.plain.is_some() || !self.exists {
            return false;
        }
        !self
            .transition()
            .is_some_and(|transition| response_leaves_screen(&transition.default_response))
    }

    fn missing(&self) -> bool {
        self.plain.is_none() && !self.exists
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

    /// `isPermitted`: each screen of the path with its parents on the artifact stack, the single service of a transition
    /// that is not read-only, and the transition with its action. The frames are a check on a fresh stack, not artifact
    /// hits ([`super::render::Frames::check`]).
    fn permitted(&self) -> bool {
        let path_defs = self.path_defs();
        let Some(last) = path_defs.len().checked_sub(1) else {
            return true;
        };
        let transition = self.transition();
        let mut frames = super::render::Frames::check();
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
        if let Some(service) = transition.single_service.as_deref().filter(|name| !name.is_empty()) {
            if !transition.read_only && !service_permitted(target, service) {
                return false;
            }
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

/// Whether the current user may use the screen or transition `info` resolves to, as its link would be enabled
/// ([`UrlInstance::disable_link`] without the condition): the screens of its path with their parents on the artifact
/// stack, the single service of a transition that is not read-only, and the transition with its action.
pub(crate) fn path_permitted(info: Arc<ScreenUrlInfo>) -> bool {
    UrlInstance {
        exists: true,
        info: Some(info),
        ..UrlInstance::default()
    }
    .permitted()
}

/// Whether the user may call a transition's single service, as `ScreenUrlInfo.isPermitted` checks it: with the service's
/// authorization action, inheriting from the last screen, and with no allow record needed when that screen or the
/// service is anonymous for the action. The frame is popped before the transition's own is pushed, as Java does not
/// stack the transition.
fn service_permitted(target: &ScreenDef, service: &str) -> bool {
    use super::definition::Authentication;
    use crate::service::define::Authenticate;
    let action = crate::service::call::service_authz_action(service);
    let authenticate = crate::service::call::service_authenticate(service);
    let allowed = match action.as_str() {
        "view" => {
            target.authentication == Authentication::AnonymousView
                || matches!(
                    authenticate,
                    Some(Authenticate::AnonymousView | Authenticate::AnonymousAll)
                )
        }
        "all" | "create" | "update" | "delete" => {
            target.authentication == Authentication::AnonymousAll || authenticate == Some(Authenticate::AnonymousAll)
        }
        _ => false,
    };
    let mut frames = super::render::Frames::untracked();
    frames.push("AT_SERVICE", service, &action, !allowed).is_ok()
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
    build_url_inline(state, subscreen_path).map(|(url, _)| url)
}

/// [`build_url`] and the inline `?` and `~` parameters of the path, which a transition alias carries to its target.
fn build_url_inline(state: &RenderState, subscreen_path: &str) -> Result<(UrlInstance, Vec<(String, String)>), Error> {
    let expanded = if subscreen_path.contains("${") {
        script::expand_localized(subscreen_path)?
    } else {
        subscreen_path.to_string()
    };
    if is_external(&expanded) {
        return Ok((plain_url(expanded), Vec::new()));
    }
    let prepared = match resolve_path(state, &active_screen_path(state), &expanded) {
        Ok(prepared) => prepared,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok((missing_url(), Vec::new())),
        Err(err) => return Err(err),
    };
    let mut url = resolve_names_at(state, prepared.names, state.url_info.last_standalone, prepared.cache)?;
    // Inline `?` and `~` parameters override screen parameters. Link parameters are added after this.
    let mut inline = Vec::new();
    for (name, value) in prepared.inline {
        if !value.is_empty() {
            url.parameters.insert(name.clone(), value.clone());
            inline.push((name, value));
        }
    }
    Ok((url, inline))
}

/// A URL for path names from the root.
///
/// # Errors
///
/// Returns an error when the names cannot be resolved for a reason other than a missing screen.
pub fn build_url_names(state: &RenderState, names: Vec<String>) -> Result<UrlInstance, Error> {
    resolve_names_at(state, names, state.url_info.last_standalone, true)
}

/// `sri.buildUrlFromTarget`: a path from the target screen of the request.
///
/// # Errors
///
/// Same as [`build_url`].
pub fn build_url_from_target(state: &RenderState, subscreen_path: &str) -> Result<UrlInstance, Error> {
    let expanded = script::expand_localized(subscreen_path)?;
    if is_external(&expanded) {
        return Ok(plain_url(expanded));
    }
    let prepared = match resolve_path(state, &state.url_info.pre_transition_path, &expanded) {
        Ok(prepared) => prepared,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(missing_url()),
        Err(err) => return Err(err),
    };
    let mut url = resolve_names_at(state, prepared.names, state.url_info.last_standalone, prepared.cache)?;
    for (name, value) in prepared.inline {
        if !value.is_empty() {
            url.parameters.insert(name, value);
        }
    }
    Ok(url)
}

/// `sri.getScreenUrlInstance`/`getCurrentScreenUrl`: the URL of the request being rendered.
///
/// # Errors
///
/// Returns an error when the request's path cannot be resolved again.
pub fn current_url(state: &RenderState) -> Result<UrlInstance, Error> {
    resolve_names_at(
        state,
        state.url_info.full_path.clone(),
        state.url_info.last_standalone,
        true,
    )
}

/// A URL for path names from the root, resolved with `last_standalone`. That only changes which screens render, so
/// the URL is the same for any value, and the menu reads its `menu_image` from the shared walk.
pub(super) fn resolve_names_at(
    state: &RenderState,
    names: Vec<String>,
    last_standalone: i32,
    cache: bool,
) -> Result<UrlInstance, Error> {
    let root = state
        .url_info
        .path_defs
        .first()
        .map(|def| def.location.as_str())
        .ok_or_else(|| Error::new("no root screen"))?;
    let resolved = ScreenUrlInfo::resolve_cached(&state.factory, root, &names, last_standalone, cache);
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

/// A link target for a server-static template: where it goes and what it is. The template evaluates the parameters, so they
/// are the definitions, not values.
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
    /// Where an alias transition sends a navigation. Empty when the target is not an alias, so a form still posts `path`.
    pub screen_path: String,
    /// Java's `isScreenUrl` for this target. False for a `type="none"` or plain-response transition, and for a missing
    /// screen, so a static anchor is `<a>` and the client does not `setUrl` it.
    pub screen_url: bool,
    /// The parameters the target gives a link to `path`, as the server render reads them for a [`UrlInstance`]: each
    /// screen's on the path, the transition's, and the in-parameters of its one service. A parameter with no `from` or
    /// `value` is read by its name.
    pub parameters: Vec<ParameterDef>,
    /// The parameters of a link to `screen_path`: the screens' the alias names, then its response's own.
    pub screen_parameters: Vec<ParameterDef>,
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
            // The target is not known yet. The template keeps an m-link, as it did before isScreenUrl.
            screen_url: true,
            ..StaticUrl::default()
        });
    }
    if is_external(text) {
        return Ok(StaticUrl {
            path: text.to_string(),
            exists: true,
            read_only: true,
            ..StaticUrl::default()
        });
    }
    let active = active_screen_path(state);
    let prepared = match resolve_path(state, &active, text) {
        Ok(prepared) => prepared,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Ok(StaticUrl {
                path: "#".to_string(),
                read_only: true,
                ..StaticUrl::default()
            })
        }
        Err(err) => return Err(err),
    };
    let names = prepared.names;
    let root = state
        .url_info
        .path_defs
        .first()
        .map(|def| def.location.clone())
        .ok_or_else(|| Error::new("no root screen"))?;
    match ScreenUrlInfo::resolve_cached(
        &state.factory,
        &root,
        &names,
        state.url_info.last_standalone,
        prepared.cache,
    ) {
        Ok(info) => {
            let alias = alias_url(state, &info)?;
            let (screen_path, screen_url) = match &alias {
                Some(alias) => (alias.path(), alias.is_screen_url()),
                None => (String::new(), info_is_screen_url(&info)),
            };
            let transition = info.transition(None);
            let parameters = link_parameters(info.parameter_items.values(), transition, &[]);
            let screen_parameters = match alias.as_ref().and_then(|alias| alias.info.as_deref()) {
                Some(target) => {
                    let response = transition.map(|item| item.default_response.parameters.as_slice());
                    link_parameters(target.parameter_items.values(), None, response.unwrap_or_default())
                }
                None => Vec::new(),
            };
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
                screen_path,
                screen_url,
                parameters,
                screen_parameters,
            })
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {
            let def = sri::active_def(state);
            let local = names
                .strip_prefix(active.as_slice())
                .and_then(|rest| (rest.len() == 1).then(|| rest[0].clone()))
                .and_then(|name| def.transition(&name, None).cloned());
            let screen_url = local
                .as_ref()
                .is_some_and(|item| !response_leaves_screen(&item.default_response));
            let parameters = local
                .as_ref()
                .map(|item| link_parameters(def.parameters.iter(), Some(item), &[]))
                .unwrap_or_default();
            Ok(StaticUrl {
                path: encode_path(&names),
                exists: local.is_some(),
                is_transition: local.is_some(),
                is_screen: false,
                read_only: local.as_ref().is_none_or(|item| item.read_only),
                method: local.map(|item| item.method.to_ascii_lowercase()).unwrap_or_default(),
                screen_path: String::new(),
                screen_url,
                parameters,
                screen_parameters: Vec::new(),
            })
        }
        Err(err) => Err(err),
    }
}

/// The parameter definitions of a link target, in the order the server render adds them (a later one with a name replaces
/// the earlier): the screens', the transition's, the in-parameters of the transition's one service, then `extra`.
fn link_parameters<'a>(
    screens: impl Iterator<Item = &'a ParameterDef>,
    transition: Option<&TransitionDef>,
    extra: &[ParameterDef],
) -> Vec<ParameterDef> {
    let mut out: IndexMap<String, ParameterDef> = IndexMap::new();
    let add = |out: &mut IndexMap<String, ParameterDef>, parameter: &ParameterDef| {
        out.insert(parameter.name.clone(), parameter.clone());
    };
    screens.for_each(|parameter| add(&mut out, parameter));
    if let Some(transition) = transition {
        transition
            .parameters
            .iter()
            .for_each(|parameter| add(&mut out, parameter));
        if let Some(service) = &transition.single_service {
            for name in service_parameter_names(service) {
                out.entry(name.clone()).or_insert(ParameterDef {
                    name,
                    required: false,
                    from: None,
                    value: None,
                });
            }
        }
    }
    extra.iter().for_each(|parameter| add(&mut out, parameter));
    out.into_values().collect()
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
    let mut inline = Vec::new();
    let mut url = match kind.as_str() {
        "transition" | "screen" => {
            let (url, parameters) = build_url_inline(state, orig_url)?;
            inline = parameters;
            url
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
    // Before the link's own parameters, so those still override the alias response.
    if expand_transition_url {
        if let Some(info) = url.info.clone() {
            if let Some(mut alias) = alias_url(state, &info)? {
                // The url's own query (`list?enumTypeId=X`) goes to the screen the alias names, as it went to the
                // transition; it was lost with the transition's url.
                for (name, value) in inline {
                    alias.parameters.insert(name, value);
                }
                url = alias;
            }
        }
    }
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

/// A transition that only names another screen. Java's `expandTransitionAliasUrl` rewrites a link to that screen
/// instead of leaving it on the transition, which a client router would try to open as a subscreen.
fn is_transition_alias(transition: &TransitionDef) -> bool {
    let response = &transition.default_response;
    transition.condition.is_none()
        && transition.service_actions.is_none()
        && transition.actions.is_none()
        && transition.actions_error.is_none()
        && transition.single_service.is_none()
        && transition.conditional_responses.is_empty()
        && response.kind == ResponseKind::Url
        && response.url_type == "screen-path"
}

/// The screen URL an alias transition stands for, or nothing when `info` does not end on one.
fn alias_url(state: &RenderState, info: &ScreenUrlInfo) -> Result<Option<UrlInstance>, Error> {
    let Some(transition) = info.transition(None).cloned() else {
        return Ok(None);
    };
    if !is_transition_alias(&transition) {
        return Ok(None);
    }
    // The extra path names become the transition's path-parameter values before the response parameters are read,
    // and they stay in the context, as Java's `setAllParameters`.
    for (name, value) in transition.path_parameters.iter().zip(info.extra_path.iter()) {
        script::set_context_name(name, Value::string(value))?;
    }
    let response = &transition.default_response;
    let target = script::expand(&response.url)?;
    let mut parameters: IndexMap<String, String> = IndexMap::new();
    for parameter in &response.parameters {
        if let Some(value) = parameter_value(parameter)? {
            let text = to_plain_string(&value);
            if !text.is_empty() {
                parameters.insert(parameter.name.clone(), text);
            }
        }
    }
    if let Some(expression) = &response.parameter_map {
        for (name, value) in script::eval_map(expression)? {
            let text = to_plain_string(&value);
            if !text.is_empty() {
                parameters.insert(name, text);
            }
        }
    }
    if is_external(&target) {
        let mut url = plain_url(target);
        for (name, value) in parameters {
            url.parameters.insert(name, value);
        }
        return Ok(Some(url));
    }
    let standalone = parameters
        .get("lastStandalone")
        .map(|text| parse_last_standalone(text, info.last_standalone))
        .unwrap_or(info.last_standalone);
    let prepared = match resolve_path(state, &info.pre_transition_path, &target) {
        Ok(prepared) => prepared,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Some(missing_url())),
        Err(err) => return Err(err),
    };
    let mut url = resolve_names_at(state, prepared.names, standalone, prepared.cache)?;
    // The destination's own `?` and `~` parameters override its screen parameters. The response parameters win.
    for (name, value) in prepared.inline {
        if !value.is_empty() {
            url.parameters.entry(name).or_insert(value);
        }
    }
    for (name, value) in parameters {
        url.parameters.insert(name, value);
    }
    Ok(Some(url))
}

fn response_leaves_screen(response: &super::definition::ResponseDef) -> bool {
    response.url_type == "plain"
        || response.kind == ResponseKind::None
        || response
            .parameters
            .iter()
            .any(|parameter| parameter.name == "renderMode")
}

fn info_is_screen_url(info: &ScreenUrlInfo) -> bool {
    !info
        .transition(None)
        .is_some_and(|transition| response_leaves_screen(&transition.default_response))
}

fn is_external(url: &str) -> bool {
    url.starts_with("http:") || url.starts_with("https:")
}

fn plain_url(text: String) -> UrlInstance {
    UrlInstance {
        plain: Some(text),
        base: web::base_url(),
        exists: true,
        read_only: true,
        ..UrlInstance::default()
    }
}

fn missing_url() -> UrlInstance {
    UrlInstance {
        base: web::base_url(),
        exists: false,
        read_only: true,
        ..UrlInstance::default()
    }
}

fn resolve_path(state: &RenderState, from: &[String], url: &str) -> Result<super::url::ScreenPath, Error> {
    let root = state
        .url_info
        .path_defs
        .first()
        .map(|def| def.location.as_str())
        .ok_or_else(|| Error::new("no root screen"))?;
    resolve_screen_path(&state.factory, root, from, url)
}

/// `sri.isAnchorLink(linkNode, urlInstance)`.
///
/// `read-only="true"` overrides `hasActions()`: a transition that has actions and says it is
/// read-only is a GET anchor. Java's html macros (`DefaultScreenMacros.html.ftl` `linkFormLink`
/// and `linkFormForm`) follow `hasActions` and POST that transition. Parity-Diffs P3-03.
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
