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

//! Anonymous token routes: `/<name>/<token>` paths that render a named screen in the public shell, for a visitor with
//! no session user, after a service checks the token. No Java counterpart. Each route is a `token-route` of the
//! webroot `webapp` in the conf:
//!
//! ```xml
//! <token-route name="s" screen-location="component://ironstone-boss/screen/PublicShare.xml"
//!         token-service="org.moqui.impl.FileServices.check#PublicShareToken" title="Shared files"/>
//! ```
//!
//! - `/<name>/<token>`: the public shell page (the webroot screen `PublicShell`, `webroot/PublicShell.xml`): Vue,
//!   Quasar, the theme's style sheets and the `m-*` components, no header, drawers or login; `WebrootPublic.qvt.js`
//!   mounts the named screen.
//! - `/<name>/<token>.qvt` (or `.qvue`, `.qjs`): the named screen itself in that render mode, which the shell loads.
//! - `/<name>/<token>/<path>`: a transition, subscreen or file below the named screen.
//! - A `POST` to `/<name>/<token>` itself, when the route has a `post-transition`: that transition of the named screen,
//!   as a `POST` to `/<name>/<token>/<transition>` would run it (an RFC 8058 one-click unsubscribe posts to the
//!   `List-Unsubscribe` URL, which is the page's). Without it a `POST` there is the shell page, as a `GET` is.
//!
//! The token service runs first, before anything else of the request: it gets `token` and `routeName` and answers
//! `valid` and `routeContext` (a map). A token it does not accept gets a short "link expired or not found" page (404),
//! and nothing of the screen runs. Otherwise the screen's context has `token` and `tokenRoute`
//! (`{name, token, basePath, title, context}`, `context` being the service's `routeContext`); a request parameter of
//! either name is replaced. The screen still checks the token in each service it calls (the public-share services do).

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_lib::node::MNode;
use moqui_lib::object::Value;

use crate::context::{Error, ExecutionContext, ExecutionContextFactory};
use crate::web::{WebRequest, WebResponse};

/// The path of the public shell page under the webroot screen.
pub const PUBLIC_SHELL_PATH: &str = "/PublicShell";

/// One `token-route` of the conf.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenRoute {
    pub name: String,
    pub screen_location: String,
    pub token_service: String,
    pub title: Option<String>,
    /// The transition of the named screen a `POST` to `/<name>/<token>` runs, after the token check.
    pub post_transition: Option<String>,
}

/// What a path under a route asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouteTarget {
    /// The public shell page.
    Shell,
    /// The named screen in a render mode (`qvt`, `qvue`, `qjs`).
    Screen(String),
    /// A path below the named screen, with its leading `/`.
    Below(String),
}

/// A request path under a token route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteMatch {
    pub route: TokenRoute,
    pub token: String,
    pub target: RouteTarget,
}

struct Routes(Vec<TokenRoute>);

/// The `token-route` elements of the webroot webapp, read from the conf once per factory. A route without a name,
/// screen or service is logged and left out; a name that is a webroot screen path (`apps`, `qapps`, `rest`, ...) is
/// left out too, so a route cannot take over a framework path.
fn routes(factory: &ExecutionContextFactory) -> Arc<Routes> {
    factory.extension(|factory| {
        let mut found = Vec::new();
        let Ok(conf) = MNode::parse_text("moqui-conf", factory.conf_text()) else {
            return Routes(found);
        };
        let Some(list) = conf.first_named("webapp-list") else {
            return Routes(found);
        };
        for webapp in list.children_named("webapp") {
            if webapp.attribute("name").as_deref() != Some("webroot") {
                continue;
            }
            for node in webapp.children_named("token-route") {
                let text = |name: &str| node.attribute(name).map(|text| text.trim().to_string()).filter(|text| !text.is_empty());
                let (Some(name), Some(screen_location), Some(token_service)) =
                    (text("name"), text("screen-location"), text("token-service"))
                else {
                    tracing::error!(attributes = ?node.attributes(), "token-route needs name, screen-location and token-service");
                    continue;
                };
                let reserved = [
                    "apps", "qapps", "rest", "rpc", "status", "error", "health", "metrics", "fop", "js", "css", "libs",
                    "images", "Login", "sso", "notws",
                ];
                if reserved.contains(&name.as_str()) || name.contains('/') {
                    tracing::error!(route = %name, "token-route name is a framework path; left out");
                    continue;
                }
                let post_transition = text("post-transition");
                if post_transition
                    .as_deref()
                    .is_some_and(|transition| transition.contains('/') || transition.contains('.'))
                {
                    tracing::error!(route = %name, "token-route post-transition is a transition name, not a path; left out");
                    continue;
                }
                found.retain(|route: &TokenRoute| route.name != name);
                found.push(TokenRoute {
                    name,
                    screen_location,
                    token_service,
                    title: text("title"),
                    post_transition,
                });
            }
        }
        Routes(found)
    })
}

/// The configured token routes.
pub fn token_routes(factory: &ExecutionContextFactory) -> Vec<TokenRoute> {
    routes(factory).0.clone()
}

/// The route a request path is under, its token and what it asks for; `None` for any other path. A token is 8 to 128
/// characters of letters, digits, `-` and `_`.
pub fn match_path(routes: &[TokenRoute], path: &str) -> Option<RouteMatch> {
    let rest = path.strip_prefix('/')?;
    let (name, rest) = rest.split_once('/')?;
    let route = routes.iter().find(|route| route.name == name)?;
    let (segment, below) = match rest.split_once('/') {
        Some((segment, below)) => (segment, Some(below)),
        None => (rest, None),
    };
    let (token, mode) = match segment.rsplit_once('.') {
        Some((token, mode)) if below.is_none() && matches!(mode, "qvt" | "qvue" | "qjs") => (token, Some(mode)),
        _ => (segment, None),
    };
    let valid = (8..=128).contains(&token.len())
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    if !valid {
        return None;
    }
    let target = match (mode, below) {
        (Some(mode), _) => RouteTarget::Screen(mode.to_string()),
        (None, Some(below)) if !below.is_empty() => RouteTarget::Below(format!("/{below}")),
        (None, _) => RouteTarget::Shell,
    };
    Some(RouteMatch {
        route: route.clone(),
        token: token.to_string(),
        target,
    })
}

/// The route of a request path in this factory's conf.
pub(super) fn find(factory: &ExecutionContextFactory, path: &str) -> Option<RouteMatch> {
    let routes = routes(factory);
    if routes.0.is_empty() {
        return None;
    }
    match_path(&routes.0, path)
}

/// Run the route's token service: the `routeContext` it answers for a token it accepts, else `None`. Errors the check
/// leaves are taken back out of the message facade; a failing service is logged and refuses the token.
fn check_token(found: &RouteMatch) -> Option<Value> {
    let mark = ExecutionContext::with_current(|ec| ec.message().error_mark());
    let parameters = BTreeMap::from([
        ("token".to_string(), Value::string(&found.token)),
        ("routeName".to_string(), Value::string(&found.route.name)),
    ]);
    let outcome = crate::service::sync(&found.route.token_service, &parameters);
    if let Some(mark) = mark {
        ExecutionContext::with_current(|ec| ec.message().take_errors_since(mark));
    }
    match outcome {
        Ok(out) if matches!(out.get("valid"), Some(Value::Bool(true))) => Some(
            out.get("routeContext")
                .cloned()
                .unwrap_or(Value::Map(indexmap::IndexMap::new())),
        ),
        Ok(_) => None,
        Err(err) => {
            tracing::error!(route = %found.route.name, service = %found.route.token_service, "token check failed: {err}");
            None
        }
    }
}

/// The short page for a token the route does not accept: 404, html for a browser and JSON for a script.
fn expired(request: &WebRequest) -> WebResponse {
    let html = request
        .header("accept")
        .is_some_and(|accept| accept.contains("text/html"));
    let message = "This link has expired or was not found.";
    if !html {
        return crate::web::json_error(404, message);
    }
    let body = format!(
        "<!DOCTYPE html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>Link not available</title><style>body{{font-family:system-ui,sans-serif;margin:0;display:flex;min-height:100vh;\
         align-items:center;justify-content:center;background:#f5f5f5;color:#222}}main{{max-width:28rem;padding:2rem;text-align:center}}\
         h1{{font-size:1.4rem;font-weight:500}}@media (prefers-color-scheme: dark){{body{{background:#1d1d1d;color:#eee}}}}</style></head>\
         <body><main><h1>Link not available</h1><p>{message}</p><p>Ask the person who shared it for a new link.</p></main></body></html>\n"
    );
    let mut response = WebResponse::new(404, body.into_bytes());
    response
        .headers
        .push(("Content-Type".to_string(), "text/html;charset=utf-8".to_string()));
    response
        .headers
        .push(("Cache-Control".to_string(), "no-store".to_string()));
    response
}

/// Serve a request under a token route: the token check first, then the shell page, the named screen in a render mode,
/// or a path below it, with `token` and `tokenRoute` in the parameters.
#[allow(clippy::too_many_arguments)]
pub(super) fn dispatch(
    factory: &Arc<ExecutionContextFactory>,
    request: &WebRequest,
    webroot: &str,
    parameters: &BTreeMap<String, Value>,
    json_list: Option<&[Value]>,
    login_error: Option<&str>,
    flags: (bool, bool),
    found: RouteMatch,
) -> Result<WebResponse, Error> {
    let Some(context) = check_token(&found) else {
        return Ok(expired(request));
    };
    let base_path = format!("/{}/{}", found.route.name, found.token);
    let mut route = indexmap::IndexMap::new();
    route.insert("name".to_string(), Value::string(&found.route.name));
    route.insert("token".to_string(), Value::string(&found.token));
    route.insert("basePath".to_string(), Value::string(&base_path));
    route.insert(
        "title".to_string(),
        found.route.title.as_ref().map_or(Value::Null, Value::string),
    );
    route.insert("context".to_string(), context);
    let mut parameters = parameters.clone();
    parameters.insert("token".to_string(), Value::string(&found.token));
    // a POST to the page's URL itself (an RFC 8058 one-click) runs the route's post-transition
    let target = match (&found.target, &found.route.post_transition) {
        (RouteTarget::Shell, Some(transition)) if request.method.eq_ignore_ascii_case("post") => {
            RouteTarget::Below(format!("/{transition}"))
        }
        (target, _) => target.clone(),
    };
    let (root, path) = match &target {
        RouteTarget::Shell => {
            // the named screen's render modes, so the shell asks for its .qvue or .qjs when it has one
            let modes = super::ScreenFacade::of(factory)
                .screen_def(factory, &found.route.screen_location)
                .map(|def| def.render_modes.clone())
                .unwrap_or_else(|err| {
                    tracing::error!(screen = %found.route.screen_location, "token-route screen does not load: {err}");
                    Vec::new()
                });
            route.insert(
                "renderModes".to_string(),
                Value::List(modes.into_iter().map(Value::String).collect()),
            );
            parameters.remove("renderMode");
            (webroot.to_string(), PUBLIC_SHELL_PATH.to_string())
        }
        RouteTarget::Screen(mode) => {
            parameters.insert("renderMode".to_string(), Value::string(mode));
            (found.route.screen_location.clone(), "/".to_string())
        }
        RouteTarget::Below(path) => (found.route.screen_location.clone(), path.clone()),
    };
    parameters.insert("tokenRoute".to_string(), Value::Map(route));
    let mut inner = request.clone();
    inner.path = path;
    super::dispatch_request(factory, &inner, &root, &parameters, json_list, login_error, flags, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(name: &str) -> TokenRoute {
        TokenRoute {
            name: name.to_string(),
            screen_location: format!("component://test/{name}.xml"),
            token_service: "test.check#Token".to_string(),
            title: None,
            post_transition: None,
        }
    }

    #[test]
    fn a_path_under_a_route_is_the_shell_the_screen_in_a_mode_or_a_path_below() {
        let routes = [route("s"), route("u")];
        let at = |path: &str| match_path(&routes, path).map(|found| (found.route.name, found.token, found.target));
        assert_eq!(
            at("/s/abcDEF12_-"),
            Some(("s".into(), "abcDEF12_-".into(), RouteTarget::Shell))
        );
        assert_eq!(
            at("/s/abcDEF12/"),
            Some(("s".into(), "abcDEF12".into(), RouteTarget::Shell))
        );
        assert_eq!(
            at("/s/abcDEF12.qvue"),
            Some(("s".into(), "abcDEF12".into(), RouteTarget::Screen("qvue".into())))
        );
        assert_eq!(
            at("/u/abcDEF12/list/rows"),
            Some(("u".into(), "abcDEF12".into(), RouteTarget::Below("/list/rows".into())))
        );
        assert_eq!(at("/s/short"), None, "a token is at least 8 characters");
        assert_eq!(at("/s/bad*token!"), None);
        assert_eq!(at("/s/abcDEF12.html"), None, "only the qvt render modes");
        assert_eq!(at("/call/abcDEF12"), None, "not a configured route");
        assert_eq!(at("/s"), None);
        assert_eq!(at("/qapps/s/abcDEF12"), None);
    }
}
