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

//! URL objects for `sri.buildUrl` and `sri.makeUrlByType`: a [`URL`] object holds a [`UrlInstance`], with the methods
//! the macros call on a Java `UrlInstance`. The `sri` functions that make one are hidden functions of the `sri` shim
//! (`sri.rs`), which puts them on the global `sri`.

use std::sync::{Arc, Mutex};

use moqui_lib::object::{HostObject, Value};

use super::{lock_state, method, Call, HostClass, HostError, Method, Ret};
use crate::screen::urls::{self, UrlInstance};
use crate::screen::{with_current_render, RenderState};

/// The `sri` functions that make a URL, which the `sri` shim (`sri.rs`) puts on `sri`.
pub(super) static SRI_URLS: &[Method] = &[
    method!("sri", "buildUrl", [Text], Keep, build_url),
    method!("sri", "buildUrlFromTarget", [Text], Keep, build_url_from_target),
    method!("sri", "getScreenUrlInstance", [], Keep, current_url),
    method!("sri", "getCurrentScreenUrl", [], Keep, current_url),
    method!(
        "sri",
        "makeUrlByType",
        [Text, Text, OptNode, OptText],
        Keep,
        make_url_by_type
    ),
    method!("sri", "isAnchorLink", [OptNode, Host], is_anchor_link),
    method!("sri", "activeInCurrentMenu", [], Keep, active_in_current_menu),
];

/// A URL a link is made from.
pub(super) static URL: HostClass = HostClass {
    name: "host:UrlInstance",
    methods: &[
        method!("UrlInstance", "getPath", [], get_path),
        method!("UrlInstance", "getUrl", [], get_url),
        method!("UrlInstance", "getPathWithParams", [], get_path_with_params),
        method!("UrlInstance", "getUrlWithParams", [OptText], get_url_with_params),
        method!("UrlInstance", "getScreenOnlyPath", [], get_screen_only_path),
        method!("UrlInstance", "getScreenPathUrl", [], get_screen_path_url),
        method!("UrlInstance", "getMinimalPathUrl", [], get_minimal_path_url),
        method!("UrlInstance", "isReadOnly", [], read_only),
        method!("UrlInstance", "isScreenUrl", [], is_screen_url),
        method!("UrlInstance", "isTransition", [], is_transition),
        method!("UrlInstance", "getTransitionMethod", [], get_transition_method),
        method!("UrlInstance", "getDisableLink", [], get_disable_link),
        method!("UrlInstance", "cloneUrlInstance", [], clone_url_instance),
        method!("UrlInstance", "addParameter", [Value, Value], add_parameter),
        method!("UrlInstance", "addParameters", [Value], add_parameters),
        method!("UrlInstance", "removeParameter", [Text], remove_parameter),
        method!("UrlInstance", "getParameterMap", [], get_parameter_map),
        method!(
            "UrlInstance",
            "getPassThroughParameterMap",
            [],
            get_pass_through_parameter_map
        ),
    ],
    getters: &[
        method!("UrlInstance", "path", [], get_path),
        method!("UrlInstance", "url", [], get_url),
        method!("UrlInstance", "pathWithParams", [], get_path_with_params),
        method!("UrlInstance", "readOnly", [], read_only),
    ],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

type State = Mutex<UrlInstance>;

fn wrap(url: UrlInstance) -> Ret {
    let state: State = Mutex::new(url);
    Ret::Object(HostObject::new(state), &URL)
}

/// `body` on the running screen render (the method's flush ran first).
fn with_render<T>(body: impl FnOnce(&RenderState) -> Result<T, crate::context::Error>) -> Result<T, HostError> {
    match with_current_render(body) {
        Some(Ok(value)) => Ok(value),
        Some(Err(err)) => Err(HostError::Context(err)),
        None => Err(HostError::message("no screen render is running")),
    }
}

fn build_url(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let path = call.text_or_empty(0);
    with_render(|state| urls::build_url(state, &path)).map(wrap)
}

fn build_url_from_target(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let path = call.text_or_empty(0);
    with_render(|state| urls::build_url_from_target(state, &path)).map(wrap)
}

fn current_url(_: &mut Call<'_>) -> Result<Ret, HostError> {
    with_render(urls::current_url).map(wrap)
}

fn make_url_by_type(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let url = call.text_or_empty(0);
    let url_type = call.text_or_empty(1);
    let parent = call.node(2).cloned();
    // Java expands only when this argument is the string "true". An omitted argument does not.
    // The screen macros pass "true" themselves when the expand-transition-url attribute is omitted.
    let expand = call.text(3) == Some("true");
    with_render(|state| urls::make_url_by_type(state, &url, &url_type, parent.as_deref(), expand)).map(wrap)
}

fn is_anchor_link(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let instance = call
        .host::<State>(1)
        .ok_or_else(|| HostError::Type("isAnchorLink needs a URL".to_string()))?;
    let instance = lock_state(&instance, "UrlInstance")?.clone();
    let link = match call.node(0) {
        Some(link) => Arc::clone(link),
        None => Arc::new(crate::screen::sri::NodeLit::default()),
    };
    Ok(urls::is_anchor_link(&link, &instance).into())
}

fn active_in_current_menu(_: &mut Call<'_>) -> Result<Ret, HostError> {
    with_render(|state| Ok(urls::active_in_current_menu(state))).map(Ret::from)
}

/// `read` of this URL.
fn read<T>(call: &Call<'_>, read: impl FnOnce(&UrlInstance) -> T) -> Result<T, HostError> {
    let state = call.this::<State>()?;
    let url = lock_state(&state, "UrlInstance")?;
    Ok(read(&url))
}

fn get_path(call: &mut Call<'_>) -> Result<Ret, HostError> {
    read(call, UrlInstance::path).map(Ret::from)
}

fn get_url(call: &mut Call<'_>) -> Result<Ret, HostError> {
    read(call, UrlInstance::url).map(Ret::from)
}

fn get_path_with_params(call: &mut Call<'_>) -> Result<Ret, HostError> {
    read(call, UrlInstance::path_with_params).map(Ret::from)
}

fn get_url_with_params(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let extension = call.text(0).unwrap_or("").to_string();
    read(call, |url| url.url_with_params(&extension)).map(Ret::from)
}

/// Java's `getScreenOnlyPath`: the screens of the path, without a transition or extra path names.
fn get_screen_only_path(call: &mut Call<'_>) -> Result<Ret, HostError> {
    read(call, UrlInstance::screen_only_path).map(Ret::from)
}

/// Java's `getScreenPathUrl`: [`get_screen_only_path`] after the base URL.
fn get_screen_path_url(call: &mut Call<'_>) -> Result<Ret, HostError> {
    read(call, |url| {
        let path = url.screen_only_path();
        if url.plain.is_some() || path == "#" {
            path
        } else {
            format!("{}{path}", url.base.trim_end_matches('/'))
        }
    })
    .map(Ret::from)
}

fn get_minimal_path_url(call: &mut Call<'_>) -> Result<Ret, HostError> {
    read(call, UrlInstance::minimal_path_url).map(Ret::from)
}

fn read_only(call: &mut Call<'_>) -> Result<Ret, HostError> {
    read(call, |url| url.read_only).map(Ret::from)
}

/// Java's `UrlInstance.isScreenUrl`: false for a plain URL, a missing screen, or a transition whose default response is
/// plain, `type="none"`, or declares `renderMode`.
fn is_screen_url(call: &mut Call<'_>) -> Result<Ret, HostError> {
    read(call, UrlInstance::is_screen_url).map(Ret::from)
}

fn is_transition(call: &mut Call<'_>) -> Result<Ret, HostError> {
    read(call, |url| url.is_transition).map(Ret::from)
}

fn get_transition_method(call: &mut Call<'_>) -> Result<Ret, HostError> {
    read(call, |url| url.transition_method.clone()).map(Ret::from)
}

fn get_disable_link(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let (exists, disabled, path) = read(call, |url| (url.exists, url.disable_link(), url.path()))?;
    let disabled = disabled.map_err(|err| HostError::message(err.message()))?;
    if !exists {
        // A static template does not write a disabled link, so a report records that the merged page did.
        crate::screen::statics::record_note("disable-link", "link", &format!("{path} is not a screen or transition"));
    }
    Ok(disabled.into())
}

fn clone_url_instance(call: &mut Call<'_>) -> Result<Ret, HostError> {
    read(call, UrlInstance::clone).map(wrap)
}

/// Change this URL; returns it, so calls chain as they do on a Java `UrlInstance`.
fn changed(call: &Call<'_>, change: impl FnOnce(&mut UrlInstance)) -> Result<Ret, HostError> {
    let state = call.this::<State>()?;
    change(&mut *lock_state(&state, "UrlInstance")?);
    Ok(Ret::This)
}

fn add_parameter(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = moqui_lib::object::to_plain_string(&call.value_or_null(0));
    let value = call.value_or_null(1);
    changed(call, |url| url.add_parameter(&name, &value))
}

fn add_parameters(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let entries = match call.value_or_null(0) {
        Value::Map(entries) => entries,
        _ => indexmap::IndexMap::new(),
    };
    changed(call, |url| {
        for (key, value) in entries {
            if key != "context" {
                url.add_parameter(&key, &value);
            }
        }
    })
}

fn remove_parameter(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.text_or_empty(0);
    changed(call, |url| url.remove_parameter(&name))
}

fn strings(map: &indexmap::IndexMap<String, String>) -> Ret {
    let map = map
        .iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect();
    Ret::Value(Value::Map(map))
}

fn get_parameter_map(call: &mut Call<'_>) -> Result<Ret, HostError> {
    read(call, |url| strings(&url.parameters))
}

fn get_pass_through_parameter_map(call: &mut Call<'_>) -> Result<Ret, HostError> {
    read(call, |url| strings(&url.pass_through_parameters()))
}
