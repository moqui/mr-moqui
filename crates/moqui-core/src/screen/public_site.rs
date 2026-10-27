//! Operator-bound public site routes. Site IDs and screen roots never come from request parameters.
use moqui_lib::node::MNode;
use moqui_lib::object::Value;
use crate::context::{Error, ErrorKind, ExecutionContextFactory};
use crate::web::WebRequest;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PublicSiteRoute {
    pub root: String,
    pub path: String,
    pub site_id: String,
    pub topic_id: Option<String>,
    pub topic_slug: Option<String>,
    pub post_id: Option<String>,
}

pub(super) fn resolve(
    factory: &ExecutionContextFactory,
    request: &WebRequest,
    current_root: &str,
) -> Result<Option<PublicSiteRoute>, Error> {
    let conf = MNode::parse_shared("moqui-conf", factory.conf_text())?;
    resolve_conf(&conf, request, current_root)
}

fn resolve_conf(conf: &MNode, request: &WebRequest, current_root: &str) -> Result<Option<PublicSiteRoute>, Error> {
    let Some(list) = conf.first_named("webapp-list") else {
        return Ok(None);
    };
    let host = request
        .headers
        .get("host")
        .map(|h| h.split(':').next().unwrap_or("").to_ascii_lowercase())
        .unwrap_or_default();
    let first = request.path.trim_start_matches('/').split('/').next().unwrap_or("");
    if [
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
    ]
    .contains(&first)
    {
        return Ok(None);
    }
    let mut public_path = false;
    for webapp in list.named_iter("webapp") {
        if !webapp.attribute_is("name", "webroot") {
            continue;
        }
        for route in webapp.named_iter("public-site") {
            let mut matches_site_path = false;
            for mount in route.named_iter("mount") {
                let prefix = mount.attribute("path").unwrap_or_default();
                if request.path == prefix
                    || request
                        .path
                        .starts_with(&(prefix.trim_end_matches('/').to_string() + "/"))
                {
                    public_path = true;
                    matches_site_path = true;
                }
            }
            if route.attribute("host").as_deref() != Some(host.as_str()) {
                continue;
            }
            if !matches_site_path {
                continue;
            }
            let site_id = route.attribute("site-id").unwrap_or_default().to_string();
            let root = route.attribute("screen-location").unwrap_or_default().to_string();
            if root == current_root {
                return Ok(None);
            }
            if site_id.is_empty() || root.is_empty() {
                return Err(Error::of_kind(
                    ErrorKind::Unavailable,
                    "Public site configuration is incomplete",
                ));
            }
            // Authentication, API, libraries and token routes retain their framework handlers.
            let first = request.path.trim_start_matches('/').split('/').next().unwrap_or("");
            if [
                "rest",
                "sso",
                "notws",
                "termws",
                "static",
                "js",
                "css",
                "assets",
                "libs",
                "PublicShell",
                "s",
                "u",
                "cal",
                "r",
            ]
            .contains(&first)
            {
                return Ok(None);
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
                return Ok(Some(PublicSiteRoute {
                    root,
                    path,
                    site_id,
                    topic_id: text("topicId"),
                    topic_slug: text("topicSlug"),
                    post_id: text("postId"),
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
    out.insert("resolvedSiteId".to_string(), Value::string(&route.site_id));
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
        let route = resolve_conf(&conf(), &request, "webroot")
            .expect("route")
            .expect("matched");
        assert_eq!(route.path, "/Forum/Topics");
        let injected = std::collections::BTreeMap::from([("siteId".into(), Value::string("OTHER"))]);
        assert_eq!(
            parameters(&route, &injected).get("siteId"),
            Some(&Value::string("SITE_A"))
        );
    }
    #[test]
    fn unknown_public_host_is_concealed_and_framework_paths_remain_available() {
        let request = WebRequest::new("GET", "/forum");
        assert_eq!(
            resolve_conf(&conf(), &request, "webroot").expect_err("host").kind(),
            ErrorKind::NotFound
        );
        let request = WebRequest::new("GET", "/rest/s1/pass");
        assert!(resolve_conf(&conf(), &request, "webroot").expect("framework").is_none());
    }
}
