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
//! <token-route name="s" screen-location="component://mr-boss/screen/PublicShare.xml"
//!         token-service="org.moqui.resource.FileServices.check#PublicShareToken" title="Shared files"/>
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
//!
//! The named screen is the root of its render, and its links, form actions and redirects to a screen start with
//! `/<name>/<token>` (`web::set_link_prefix`), so a form posts to `/<name>/<token>/<transition>`; a server-static
//! template, cached for every visitor, is made without it. The token is the route's credential: what the server records
//! of a path or URL ([`redact_url`], [`redact_json`]) has `/<name>/:token/...` instead.

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

/// The `token-route` elements of the webroot webapp, read from the conf once per conf generation. A route without a
/// name, screen or service is logged and left out; a name that is a webroot screen path (`apps`, `qapps`, `rest`, ...)
/// is left out too, so a route cannot take over a framework path.
fn routes(factory: &ExecutionContextFactory) -> Arc<Routes> {
    factory.conf_derived(|factory| {
        let mut found = Vec::new();
        let Ok(conf) = MNode::parse_shared("moqui-conf", &factory.conf_text()) else {
            return Routes(found);
        };
        let Some(list) = conf.first_named("webapp-list") else {
            return Routes(found);
        };
        for webapp in list.named_iter("webapp") {
            if !webapp.attribute_is("name", "webroot") {
                continue;
            }
            for node in webapp.named_iter("token-route") {
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

/// What a token route's token reads as where a path is recorded: `/i/{token}/accept` is `/i/:token/accept`.
pub const REDACTED_TOKEN: &str = ":token";

/// `path` with the token of a token route replaced by [`REDACTED_TOKEN`], for telemetry and logs (a route's token is
/// its credential, and every trace, log line and request record would otherwise hold it); `None` when the path is under
/// no route, or the segment after the route's name is no token (8 to 128 of the token characters, ending the segment
/// or followed by a render-mode suffix). The rest of the path is kept: `/u/{token}/oneClick` names the transition.
pub fn redact_path(routes: &[TokenRoute], path: &str) -> Option<String> {
    let rest = path.strip_prefix('/')?;
    let (name, after) = rest.split_once('/')?;
    if !routes.iter().any(|route| route.name == name) {
        return None;
    }
    let token_len = after
        .bytes()
        .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'-' || *byte == b'_')
        .count();
    let ends = after[token_len..]
        .chars()
        .next()
        .is_none_or(|next| matches!(next, '/' | '.' | '?' | '#' | ';'));
    if !(8..=128).contains(&token_len) || !ends {
        return None;
    }
    Some(format!("/{name}/{REDACTED_TOKEN}{}", &after[token_len..]))
}

/// [`redact_path`] for a path (`/i/...`) or an absolute URL (`https://host/i/...`) in recorded text, such as a
/// `Referer` or a `returnTo` parameter; any other text, and a path under no route, as it is.
pub fn redact_url<'a>(routes: &[TokenRoute], text: &'a str) -> std::borrow::Cow<'a, str> {
    use std::borrow::Cow;
    if routes.is_empty() {
        return Cow::Borrowed(text);
    }
    if text.starts_with('/') && !text.starts_with("//") {
        return redact_path(routes, text).map_or(Cow::Borrowed(text), Cow::Owned);
    }
    // an absolute URL, or a scheme-relative one (`//host/...`)
    let host_start = match text.strip_prefix("//") {
        Some(_) => 2,
        None => match text.find("://") {
            Some(scheme_end) => scheme_end + 3,
            None => return Cow::Borrowed(text),
        },
    };
    let Some(path_start) = text[host_start..].find('/').map(|at| host_start + at) else {
        return Cow::Borrowed(text);
    };
    match redact_path(routes, &text[path_start..]) {
        Some(path) => Cow::Owned(format!("{}{path}", &text[..path_start])),
        None => Cow::Borrowed(text),
    }
}

/// [`redact_url`] with this factory's token routes: what the request log and a log line record of a path or URL.
pub fn redact<'a>(factory: &ExecutionContextFactory, text: &'a str) -> std::borrow::Cow<'a, str> {
    redact_url(&routes(factory).0, text)
}

/// [`redact_url`] for each path or URL within free text (a span name `GET /i/{token}`, a log body, a message), the words
/// being split at white space, quotes, brackets and parentheses; the separators are kept.
pub fn redact_text<'a>(routes: &[TokenRoute], text: &'a str) -> std::borrow::Cow<'a, str> {
    use std::borrow::Cow;
    if routes.is_empty() || !text.contains('/') {
        return Cow::Borrowed(text);
    }
    let separator = |ch: char| ch.is_whitespace() || matches!(ch, '"' | '\'' | '`' | '<' | '>' | '(' | ')' | '[' | ']');
    let mut out: Option<String> = None;
    let mut start = 0;
    for (at, ch) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        if !separator(ch) {
            continue;
        }
        let word = &text[start..at];
        if let Cow::Owned(redacted) = redact_url(routes, word) {
            let out = out.get_or_insert_with(|| text[..start].to_string());
            out.push_str(&redacted);
        } else if let Some(out) = out.as_mut() {
            out.push_str(word);
        }
        if let Some(out) = out.as_mut() {
            if at < text.len() {
                out.push(ch);
            }
        }
        start = at + ch.len_utf8();
    }
    out.map_or(Cow::Borrowed(text), Cow::Owned)
}

/// Every string in `json` with [`redact_text`] applied: what a browser sends (`/rum/*` OTLP records, a replay's rrweb
/// events, whose snapshot holds the page's links) before it is stored. Object keys are left as they are.
pub fn redact_json(routes: &[TokenRoute], json: &mut serde_json::Value) {
    use serde_json::Value as Json;
    if routes.is_empty() {
        return;
    }
    match json {
        Json::String(text) => {
            if let std::borrow::Cow::Owned(redacted) = redact_text(routes, text) {
                *text = redacted;
            }
        }
        Json::Array(items) => items.iter_mut().for_each(|item| redact_json(routes, item)),
        Json::Object(map) => map.values_mut().for_each(|item| redact_json(routes, item)),
        _ => {}
    }
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
                .unwrap_or(Value::Map(indexmap::IndexMap::default())),
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
    let mut route = indexmap::IndexMap::default();
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
    // the named screen is the render's root, so its links, form actions and redirects start with the route's path: a
    // transition is /<name>/<token>/<transition>, not /<transition> outside the route (the shell's root is the webroot)
    let _prefix = (!matches!(target, RouteTarget::Shell)).then(|| crate::web::set_link_prefix(&base_path));
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

    #[test]
    fn a_routes_token_is_redacted_from_a_recorded_path_or_url() {
        let routes = [route("i"), route("u"), route("call")];
        let path = |text: &str| redact_path(&routes, text);
        assert_eq!(path("/i/abcDEF12_-xyz").as_deref(), Some("/i/:token"));
        assert_eq!(path("/i/abcDEF12_-xyz/").as_deref(), Some("/i/:token/"));
        assert_eq!(path("/u/abcDEF12345/oneClick").as_deref(), Some("/u/:token/oneClick"));
        assert_eq!(path("/call/abcDEF12345.qvt").as_deref(), Some("/call/:token.qvt"));
        assert_eq!(path("/i/short"), None, "no token under 8 characters");
        assert_eq!(path("/i/abcDEF12*x"), None);
        assert_eq!(path("/s/abcDEF12345"), None, "not a configured route");
        assert_eq!(path("/apps/i/abcDEF12345"), None, "a route is at the root only");
        assert_eq!(path("/i"), None);
        let url = |text: &str| redact_url(&routes, text).into_owned();
        assert_eq!(url("/i/abcDEF12345/accept?x=1"), "/i/:token/accept?x=1");
        assert_eq!(
            url("https://staff.example.com/i/abcDEF12345/accept"),
            "https://staff.example.com/i/:token/accept"
        );
        assert_eq!(
            url("http://localhost:8080/call/abcDEF12345"),
            "http://localhost:8080/call/:token"
        );
        assert_eq!(
            url("https://example.com/apps/System"),
            "https://example.com/apps/System"
        );
        assert_eq!(url("https://example.com"), "https://example.com");
        assert_eq!(url("//staff.example.com/i/abcDEF12345"), "//staff.example.com/i/:token");
        assert_eq!(url("plain text"), "plain text");
        assert_eq!(redact_url(&[], "/i/abcDEF12345"), "/i/abcDEF12345");
    }

    #[test]
    fn a_routes_token_is_redacted_within_text_and_json() {
        let routes = [route("i"), route("tk")];
        let text = |text: &str| redact_text(&routes, text).into_owned();
        assert_eq!(text("GET /i/abcDEF12345/accept"), "GET /i/:token/accept");
        assert_eq!(
            text("from (https://h.example.com/tk/abcDEF12345?x=1) and [/i/abcDEF12345]"),
            "from (https://h.example.com/tk/:token?x=1) and [/i/:token]"
        );
        assert_eq!(
            text("GET /apps/i/abcDEF12345"),
            "GET /apps/i/abcDEF12345",
            "not under a route"
        );
        assert_eq!(text("caf\u{e9} /i/abcDEF12345\n"), "caf\u{e9} /i/:token\n");
        assert!(matches!(
            redact_text(&routes, "no path here"),
            std::borrow::Cow::Borrowed(_)
        ));
        let mut json = serde_json::json!({
            "name": "GET /tk/abcDEF12345.qvt",
            "attributes": [{"key": "url.full", "value": {"stringValue": "https://h/i/abcDEF12345/factor"}}],
            "events": [{"type": 2, "data": {"attributes": {"href": "/tk/abcDEF12345/files"}}}, 4, null]
        });
        redact_json(&routes, &mut json);
        let shown = json.to_string();
        assert!(!shown.contains("abcDEF12345"), "{shown}");
        assert_eq!(
            json["attributes"][0]["value"]["stringValue"],
            "https://h/i/:token/factor"
        );
        assert_eq!(json["events"][0]["data"]["attributes"]["href"], "/tk/:token/files");
    }
}
