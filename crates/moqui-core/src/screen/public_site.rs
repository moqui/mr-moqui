//! Operator-bound public site routes. Site IDs and screen roots never come from request parameters.
use crate::context::{Error, ErrorKind, ExecutionContextFactory};
use crate::web::WebRequest;
use moqui_lib::node::MNode;
use moqui_lib::object::Value;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PublicSiteRoute {
    pub root: String,
    pub path: String,
    pub site_id: String,
    pub topic_id: Option<String>,
    pub topic_slug: Option<String>,
    pub post_id: Option<String>,
    /// The resolver service's `screenParameters` map: named screen parameters for this path (a shop's product or
    /// category id). They take the place of request parameters of the same name; the site ids and the Forum's three
    /// ids are bound after them.
    pub screen_parameters: std::collections::BTreeMap<String, Value>,
}

/// The parameter that names the site a request was routed to, bound by the conf ([`parameters`]); a request that
/// was not routed has none, whatever it sends.
pub(super) const BOUND_SITE_PARAMETER: &str = "resolvedSiteId";

/// First path segments that always belong to the framework, never to a public site.
const FRAMEWORK_ROUTES: [&str; 29] = [
    "apps",
    "qapps",
    "rest",
    "rpc",
    "status",
    "error",
    "health",
    "metrics",
    "fop",
    "js",
    "css",
    "libs",
    "images",
    "static",
    "assets",
    "Login",
    "SecondFactor",
    "PasswordReset",
    "sso",
    "notws",
    "termws",
    "PublicShell",
    "s",
    "u",
    "cal",
    "r",
    // the PWA: its service worker, manifest and the push transitions
    "sw.js",
    "manifest.webmanifest",
    "WebPush",
];

/// The framework routes a restricted host still serves (`host-check-service`, Commerce F87): the static files a page
/// there loads (the preview frame's scripts, styles and libraries).
const STATIC_ROUTES: [&str; 6] = ["js", "css", "libs", "images", "static", "assets"];

fn normalize_host(host: &str) -> String {
    host.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// The hosts of the conf's `public-site` routes (mr-pass's sites), normalized: on an edge node, names no local node's
/// setup may claim (plans/archive/Edge-Node.md §5.7).
pub fn hosts(factory: &ExecutionContextFactory) -> Vec<String> {
    let Ok(conf) = MNode::parse_shared("moqui-conf", &factory.conf_text()) else {
        return Vec::new();
    };
    let Some(list) = conf.first_named("webapp-list") else {
        return Vec::new();
    };
    let mut hosts = Vec::new();
    for webapp in list.named_iter("webapp") {
        for route in webapp.named_iter("public-site") {
            if let Some(host) = route.attribute("host") {
                hosts.push(normalize_host(&host));
            }
        }
    }
    hosts
}

/// Whether `host` (a `Host` header, with or without a port) is the host of a `public-site` route.
pub fn is_site_host(factory: &ExecutionContextFactory, host: &str) -> bool {
    let host = normalize_host(host.rsplit_once(':').map_or(host, |(name, port)| {
        // an IPv6 literal's last colon is not a port's
        if port.bytes().all(|byte| byte.is_ascii_digit()) {
            name
        } else {
            host
        }
    }));
    !host.is_empty() && hosts(factory).contains(&host)
}

/// Whether `first`, the first segment of a request path, always belongs to the framework ([`FRAMEWORK_ROUTES`]) and
/// never to a public site's pages.
pub fn is_framework_route(first: &str) -> bool {
    FRAMEWORK_ROUTES.contains(&first)
}

/// Whether a request for a framework route (a [`FRAMEWORK_ROUTES`] path that is not one of the [`STATIC_ROUTES`], or a
/// path a servlet other than the screens' takes, `servlet_route`) is refused on its host: a `<public-site>` of the host
/// names a `host-check-service`, which is called with the site, host and path and answers `restricted` true (a site's
/// theme preview host, Commerce F87). Such a host serves its site and static files only, so nobody signs in to the
/// framework or reaches its apps, REST, token routes or servlets on it. A static route counts only when the screen path
/// stays under it: `/libs/../apps` is the apps. A service that fails refuses the route (the error is logged): the check
/// guards a security boundary.
pub(super) fn refuses_framework_route(
    factory: &ExecutionContextFactory,
    request: &WebRequest,
    servlet_route: bool,
) -> bool {
    // a host whose root-screen says framework-routes="refuse" serves its root's screens and the static files only: no
    // token route or servlet (its root has none of the framework's screens, so those are Not Found already)
    if (servlet_route || super::token_route::find(factory, &request.path).is_some())
        && super::info::host_refuses_framework_routes(factory, request.header("host").unwrap_or(""))
    {
        return true;
    }
    let first = request.path.trim_start_matches('/').split('/').next().unwrap_or("");
    if !servlet_route && !FRAMEWORK_ROUTES.contains(&first) {
        return false;
    }
    if !servlet_route && STATIC_ROUTES.contains(&first) && screen_path_first(&request.path) == first {
        return false;
    }
    match MNode::parse_shared("moqui-conf", &factory.conf_text()) {
        Ok(conf) => host_restricted(factory, &conf, request),
        Err(err) => {
            tracing::error!(
                "Public site host check of {}: the conf did not parse: {err}",
                super::token_route::redact(factory, &request.path)
            );
            true
        }
    }
}

/// The first name of the screen path a request path resolves to ([`screen_path_names`]).
fn screen_path_first(path: &str) -> String {
    screen_path_names(path).into_iter().next().unwrap_or_default()
}

/// The names of the screen path a request path resolves to, as the screen URL reads it (`url::cleanup_path_names`,
/// Java's `cleanupPathNameList`): empty names, `.` and `~name=value` drop, `..` pops.
pub(super) fn screen_path_names(path: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for name in path.split('/') {
        match name {
            "" | "." => {}
            ".." => {
                names.pop();
            }
            _ if name.starts_with('~') => {}
            _ => names.push(name.to_string()),
        }
    }
    names
}

/// Whether `first`, the first name of a screen path, is one of the shared static file routes ([`STATIC_ROUTES`]).
pub(super) fn is_static_route(first: &str) -> bool {
    STATIC_ROUTES.contains(&first)
}

/// [`refuses_framework_route`] for a framework route on the request's host, over `conf`.
fn host_restricted(factory: &ExecutionContextFactory, conf: &MNode, request: &WebRequest) -> bool {
    let Some(list) = conf.first_named("webapp-list") else {
        return false;
    };
    let host = request_host(request);
    for webapp in list
        .named_iter("webapp")
        .filter(|webapp| webapp.attribute_is("name", "webroot"))
    {
        for route in webapp.named_iter("public-site") {
            if route.attribute("host").map(|h| normalize_host(&h)).as_deref() != Some(host.as_str()) {
                continue;
            }
            let Some(service) = route.attribute("host-check-service").filter(|s| !s.is_empty()) else {
                continue;
            };
            let input = std::collections::BTreeMap::from([
                (
                    "siteId".to_string(),
                    Value::string(route.attribute("site-id").unwrap_or_default()),
                ),
                ("hostname".to_string(), Value::string(&host)),
                ("path".to_string(), Value::string(&request.path)),
            ]);
            let options = crate::service::call::CallOptions {
                throw_on_error: true,
                ..Default::default()
            };
            match crate::service::call::sync_checked(&service, &input, &options) {
                Ok(output) => {
                    let restricted = output.get("restricted").is_some_and(|value| match value {
                        Value::Bool(flag) => *flag,
                        other => matches!(moqui_lib::object::to_plain_string(other).as_str(), "true" | "Y"),
                    });
                    if restricted {
                        return true;
                    }
                }
                Err(err) => {
                    tracing::error!(
                        "Public site host check {service} for {host}{}: {err}",
                        super::token_route::redact(factory, &request.path)
                    );
                    return true;
                }
            }
        }
    }
    false
}

/// The request's host, normalized, without its port.
fn request_host(request: &WebRequest) -> String {
    // "Forum.Test.:8080" and "forum.test" name the same host.
    request
        .headers
        .get("host")
        .map(|h| normalize_host(h.split(':').next().unwrap_or("")))
        .unwrap_or_default()
}

/// The public site route of a request: the site's root, the screen path under it and the site the conf binds, for a
/// path under a mount of a `public-site` of the request's host. None for any other request.
///
/// # Errors
///
/// A path under a named mount on a host no site names, or one the site's resolver does not find, is Not Found; an
/// incomplete `public-site` is Unavailable.
pub(super) fn resolve(
    factory: &ExecutionContextFactory,
    request: &WebRequest,
) -> Result<Option<PublicSiteRoute>, Error> {
    let conf = MNode::parse_shared("moqui-conf", &factory.conf_text())?;
    resolve_conf(&conf, request)
}

fn resolve_conf(conf: &MNode, request: &WebRequest) -> Result<Option<PublicSiteRoute>, Error> {
    let Some(list) = conf.first_named("webapp-list") else {
        return Ok(None);
    };
    let host = request_host(request);
    // Authentication, API, libraries and token routes keep their framework handlers on every host but one whose
    // `host-check-service` restricts it (`refuses_framework_route`, checked before any servlet or token route runs).
    let first = request.path.trim_start_matches('/').split('/').next().unwrap_or("");
    if FRAMEWORK_ROUTES.contains(&first) {
        return Ok(None);
    }
    // A path under a site's named mount (`/forum`) on a host no site names is concealed. A mount at the root claims
    // only its own host: every other host's paths stay with the webapp's screens (a shop at the root of its host
    // beside the BOSS app on another).
    let mut public_path = false;
    for webapp in list.named_iter("webapp") {
        if !webapp.attribute_is("name", "webroot") {
            continue;
        }
        for route in webapp.named_iter("public-site") {
            let own_host = route.attribute("host").map(|h| normalize_host(&h)).as_deref() == Some(host.as_str());
            let mut matches_site_path = false;
            for mount in route.named_iter("mount") {
                let prefix = mount.attribute("path").unwrap_or_default();
                if request.path == prefix
                    || request
                        .path
                        .starts_with(&(prefix.trim_end_matches('/').to_string() + "/"))
                {
                    matches_site_path |= own_host;
                    public_path |= !prefix.trim_matches('/').is_empty();
                }
            }
            if !matches_site_path {
                continue;
            }
            let site_id = route.attribute("site-id").unwrap_or_default().to_string();
            let root = route.attribute("screen-location").unwrap_or_default().to_string();
            if site_id.is_empty() || root.is_empty() {
                return Err(Error::of_kind(
                    ErrorKind::Unavailable,
                    "Public site configuration is incomplete",
                ));
            }
            if let Some(service) = route.attribute("resolver-service").filter(|s| !s.is_empty()) {
                let input = std::collections::BTreeMap::from([
                    ("siteId".to_string(), Value::string(&site_id)),
                    ("hostname".to_string(), Value::string(&host)),
                    ("path".to_string(), Value::string(&request.path)),
                ]);
                let output = crate::service::call::sync_checked(
                    &service,
                    &input,
                    &crate::service::call::CallOptions {
                        throw_on_error: true,
                        ..Default::default()
                    },
                )?;
                let path = output
                    .get("screenPath")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if !path.starts_with('/') || path.contains("..") || path.starts_with("//") {
                    return Err(Error::of_kind(ErrorKind::NotFound, "Page not found"));
                }
                let text = |name: &str| output.get(name).and_then(Value::as_str).map(str::to_string);
                let screen_parameters = match output.get("screenParameters") {
                    Some(Value::Map(entries)) => entries.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                    _ => std::collections::BTreeMap::new(),
                };
                return Ok(Some(PublicSiteRoute {
                    root,
                    path,
                    site_id,
                    topic_id: text("topicId"),
                    topic_slug: text("topicSlug"),
                    post_id: text("postId"),
                    screen_parameters,
                }));
            }
            let mut selected = None;
            for mount in route.named_iter("mount") {
                let prefix = mount.attribute("path").unwrap_or_default().to_string();
                let screen = mount.attribute("screen").unwrap_or_default().to_string();
                if !prefix.starts_with('/') || prefix.contains("..") || screen.contains("..") {
                    return Err(Error::of_kind(ErrorKind::Unavailable, "Invalid public site mount"));
                }
                if request.path == prefix
                    || request
                        .path
                        .starts_with(&(prefix.trim_end_matches('/').to_string() + "/"))
                {
                    let suffix = request.path.strip_prefix(prefix.trim_end_matches('/')).unwrap_or("");
                    if suffix.split('/').any(|segment| segment == "..") {
                        return Err(Error::of_kind(ErrorKind::NotFound, "Page not found"));
                    }
                    let path = format!("/{}{}", screen.trim_matches('/'), suffix);
                    if selected.as_ref().is_none_or(|(length, _)| prefix.len() > *length) {
                        selected = Some((prefix.len(), path));
                    }
                }
            }
            return selected
                .map(|(_, path)| {
                    Some(PublicSiteRoute {
                        root,
                        path,
                        site_id,
                        topic_id: None,
                        topic_slug: None,
                        post_id: None,
                        screen_parameters: std::collections::BTreeMap::new(),
                    })
                })
                .ok_or_else(|| Error::of_kind(ErrorKind::NotFound, "Page not found"));
        }
    }
    if public_path {
        return Err(Error::of_kind(ErrorKind::NotFound, "Public hostname not found"));
    }
    Ok(None)
}

pub(super) fn parameters(
    route: &PublicSiteRoute,
    input: &std::collections::BTreeMap<String, Value>,
) -> std::collections::BTreeMap<String, Value> {
    let mut out = input.clone();
    out.extend(route.screen_parameters.clone());
    for (key, value) in [
        ("rootCommEventId", &route.topic_id),
        ("requestedTopicSlug", &route.topic_slug),
        ("postId", &route.post_id),
    ] {
        if let Some(text) = value {
            out.insert(key.to_string(), Value::string(text));
        }
    }
    out.insert("siteId".to_string(), Value::string(&route.site_id));
    out.insert(BOUND_SITE_PARAMETER.to_string(), Value::string(&route.site_id));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn conf() -> MNode {
        MNode::parse_shared("test", r#"<moqui-conf><webapp-list><webapp name="webroot"><public-site host="forum.test" site-id="SITE_A" screen-location="component://mr-pass/screen/Pass.xml"><mount path="/forum" screen="Forum"/><mount path="/account" screen="Account"/></public-site></webapp></webapp-list></moqui-conf>"#).expect("conf")
    }
    #[test]
    fn public_route_binds_host_site_and_path() {
        let mut request = WebRequest::new("GET", "/forum/Topics");
        request.headers.insert("host".into(), "forum.test:8080".into());
        let route = resolve_conf(&conf(), &request).expect("route").expect("matched");
        assert_eq!(route.path, "/Forum/Topics");
        let injected = std::collections::BTreeMap::from([("siteId".into(), Value::string("OTHER"))]);
        assert_eq!(
            parameters(&route, &injected).get("siteId"),
            Some(&Value::string("SITE_A"))
        );
    }
    #[test]
    fn host_case_trailing_dot_and_parent_segments() {
        let mut request = WebRequest::new("GET", "/forum/../account/Settings");
        request.headers.insert("host".into(), "Forum.Test.".into());
        assert_eq!(
            resolve_conf(&conf(), &request).expect_err("parent").kind(),
            ErrorKind::NotFound
        );
        let mut request = WebRequest::new("GET", "/forum/Topics");
        request.headers.insert("host".into(), "Forum.Test.".into());
        assert!(resolve_conf(&conf(), &request).expect("route").is_some());
    }
    #[test]
    fn unknown_public_host_is_concealed_and_framework_paths_remain_available() {
        let request = WebRequest::new("GET", "/forum");
        assert_eq!(
            resolve_conf(&conf(), &request).expect_err("host").kind(),
            ErrorKind::NotFound
        );
        let request = WebRequest::new("GET", "/rest/s1/pass");
        assert!(resolve_conf(&conf(), &request).expect("framework").is_none());
    }
    #[test]
    fn a_site_mounted_at_the_root_never_takes_the_status_probes() {
        let conf = MNode::parse_shared("test", r#"<moqui-conf><webapp-list><webapp name="webroot"><public-site host="forum.test" site-id="SITE_A" screen-location="component://mr-pass/screen/Pass.xml"><mount path="/" screen="Forum"/></public-site></webapp></webapp-list></moqui-conf>"#).expect("conf");
        for path in ["/metrics", "/health"] {
            for query in [None, Some("format=prometheus")] {
                let mut request = WebRequest::new("GET", path);
                request.headers.insert("host".into(), "forum.test".into());
                request.raw_query = query.map(str::to_string);
                assert!(
                    resolve_conf(&conf, &request).expect(path).is_none(),
                    "{path} stays the status handler's"
                );
            }
        }
        let mut request = WebRequest::new("GET", "/Topics");
        request.headers.insert("host".into(), "forum.test".into());
        assert!(resolve_conf(&conf, &request).expect("site").is_some());
    }
    fn root_and_forum() -> MNode {
        MNode::parse_shared("test", r#"<moqui-conf><webapp-list><webapp name="webroot"><public-site host="shop.test" site-id="SHOP" screen-location="component://mr-shop/screen/Shop.xml"><mount path="/" screen="Store"/></public-site><public-site host="forum.test" site-id="SITE_A" screen-location="component://mr-pass/screen/Pass.xml"><mount path="/forum" screen="Forum"/></public-site></webapp></webapp-list></moqui-conf>"#).expect("conf")
    }
    fn on(host: &str, path: &str) -> WebRequest {
        let mut request = WebRequest::new("GET", path);
        request.headers.insert("host".into(), host.into());
        request
    }
    #[test]
    fn a_root_mount_claims_only_its_own_host() {
        let conf = root_and_forum();
        // another host's paths, and a request with no host, stay with the webapp
        for request in [
            on("boss.test", "/Topics"),
            on("boss.test", "/"),
            WebRequest::new("GET", "/Login2"),
        ] {
            assert!(
                resolve_conf(&conf, &request).expect("webapp").is_none(),
                "{} stays the webapp's",
                request.path
            );
        }
        // the root mount's own host still routes, its paths and its root
        let route = resolve_conf(&conf, &on("Shop.Test:8443", "/p/Widget"))
            .expect("route")
            .expect("shop");
        assert_eq!(
            (route.site_id.as_str(), route.path.as_str()),
            ("SHOP", "/Store/p/Widget")
        );
        assert!(resolve_conf(&conf, &on("shop.test", "/")).expect("route").is_some());
        // on the shop's host a named mount of another site's is the shop's path
        let route = resolve_conf(&conf, &on("shop.test", "/forum"))
            .expect("route")
            .expect("shop");
        assert_eq!(route.site_id, "SHOP");
        // framework paths stay reserved on the shop's host and elsewhere
        for host in ["shop.test", "boss.test"] {
            for path in ["/apps/Boss", "/rest/s1/pass", "/Login", "/status"] {
                assert!(resolve_conf(&conf, &on(host, path)).expect(path).is_none());
            }
        }
        // a named mount on a host no site names is still concealed
        assert_eq!(
            resolve_conf(&conf, &on("boss.test", "/forum/Topics"))
                .expect_err("concealed")
                .kind(),
            ErrorKind::NotFound
        );
    }
    #[test]
    fn resolver_screen_parameters_bind_before_the_site_and_forum_ids() {
        let route = PublicSiteRoute {
            root: "component://mr-shop/screen/Shop.xml".into(),
            path: "/Store/Product".into(),
            site_id: "SHOP".into(),
            topic_id: Some("T1".into()),
            topic_slug: None,
            post_id: None,
            screen_parameters: std::collections::BTreeMap::from([
                ("productId".into(), Value::string("WIDGET")),
                ("siteId".into(), Value::string("OTHER")),
                ("rootCommEventId".into(), Value::string("T2")),
            ]),
        };
        let input = std::collections::BTreeMap::from([("productId".into(), Value::string("FROM_REQUEST"))]);
        let out = parameters(&route, &input);
        assert_eq!(out.get("productId"), Some(&Value::string("WIDGET")));
        assert_eq!(out.get("siteId"), Some(&Value::string("SHOP")));
        assert_eq!(out.get("rootCommEventId"), Some(&Value::string("T1")));
    }
}
