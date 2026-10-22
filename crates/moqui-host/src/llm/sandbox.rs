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

//! `GET /assist/sandbox`: the frame document of a Vue SFC canvas (plans/LLM-A2A-Assist.md 4.10; docs/Assist.md "Vue SFC
//! canvases"). The drawer loads it in `<iframe sandbox="allow-scripts">`, so it has an opaque origin, and it is sent with
//! a policy of its own (`moqui_core::llm::ui::vue_sfc::frame_document`), not the screens' (which forbid framing): the
//! `sandbox` directive, no network, form, frame or worker, scripts only Vue, Quasar and the frame runtime. The document
//! holds no data and needs no session; the policy names this site's origin, written out from the request (its `Host`,
//! and HTTPS on a TLS connection or from a trusted proxy), because `'self'` does not match an opaque origin.
//! `?v=` busts the caches of the scripts it loads; `?x=1` (the user's choice, `ASSIST_CANVAS_EXTERNAL`) lets images
//! come from other sites.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Query, Request, State};
use axum::response::Response;
use moqui_core::llm::ui::vue_sfc;

use crate::http::{forwarded_https, plain, HttpState, TlsConnection};

/// The origin of the page that frames the document: the scheme and the `Host` header, or `None` for a host that is
/// not a plain name and port.
fn origin_of(host: &str, https: bool) -> Option<String> {
    let origin = format!("{}://{}", if https { "https" } else { "http" }, host.trim());
    vue_sfc::plain_origin(&origin).then_some(origin)
}

/// The response for `host`, `https` and the query: the document, its policy and the frame headers.
fn response_for(host: &str, https: bool, query: &HashMap<String, String>) -> Response {
    let Some(origin) = origin_of(host, https) else {
        return plain(400, "the Host of the request is not a plain host name");
    };
    let version = query.get("v").map(String::as_str).unwrap_or_default();
    let external = query.get("x").is_some_and(|value| value == "1");
    let document = vue_sfc::frame_document(&origin, version, external);
    let mut builder = Response::builder()
        .status(200)
        .header("content-type", "text/html; charset=utf-8")
        .header("content-security-policy", &document.csp);
    for (name, value) in vue_sfc::FRAME_HEADERS {
        builder = builder.header(*name, *value);
    }
    builder
        .body(axum::body::Body::from(document.html))
        .unwrap_or_else(|_| plain(500, "the frame document header was not valid"))
}

/// The public host the browser used, in the order `web::refused_origin` resolves a request's own host so a proxy that
/// rewrites `Host` does not break the frame: `X-Forwarded-Host` (its first value) when the peer is a believed proxy,
/// else the request's `Host`, else the webapp conf's `https-host`/`http-host` for the scheme. Empty when none is set.
fn frame_host(forwarded: Option<&str>, host: Option<&str>, believed: bool, configured: (&str, &str), https: bool) -> String {
    let clean = |value: &str| value.split(',').next().unwrap_or("").trim().to_string();
    if believed {
        if let Some(value) = forwarded.map(clean).filter(|value| !value.is_empty()) {
            return value;
        }
    }
    if let Some(value) = host.map(clean).filter(|value| !value.is_empty()) {
        return value;
    }
    let (http_host, https_host) = configured;
    let primary = if https { https_host } else { http_host };
    let fallback = if https { http_host } else { https_host };
    if !primary.trim().is_empty() {
        primary.trim().to_string()
    } else {
        fallback.trim().to_string()
    }
}

pub(crate) async fn frame(
    State(state): State<Arc<HttpState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Query(query): Query<HashMap<String, String>>,
    request: Request,
) -> Response {
    let over_tls = request.extensions().get::<TlsConnection>().is_some();
    let https = over_tls || forwarded_https(request.headers(), peer.ip(), state.guard.conf.addressing());
    let header = |name: &str| request.headers().get(name).and_then(|value| value.to_str().ok());
    let believed = state.guard.conf.addressing().believes(peer.ip());
    let host = frame_host(
        header("x-forwarded-host"),
        header("host"),
        believed,
        (&state.factory.webapp_setting("webroot", "http-host"), &state.factory.webapp_setting("webroot", "https-host")),
        https,
    );
    response_for(&host, https, &query)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(response: &Response, name: &str) -> String {
        response.headers().get(name).and_then(|value| value.to_str().ok()).unwrap_or_default().to_string()
    }

    #[test]
    fn the_frame_document_has_its_own_strict_policy() {
        let response = response_for("moqui.example.com:8443", true, &HashMap::from([("v".to_string(), "3.1".to_string())]));
        assert_eq!(response.status(), 200);
        assert_eq!(header(&response, "content-type"), "text/html; charset=utf-8");
        let csp = header(&response, "content-security-policy");
        let origin = "https://moqui.example.com:8443";
        for directive in [
            "sandbox allow-scripts;".to_string(),
            "default-src 'none'".to_string(),
            "connect-src 'none'".to_string(),
            "form-action 'none'".to_string(),
            "frame-src 'none'".to_string(),
            "worker-src 'none'".to_string(),
            "base-uri 'none'".to_string(),
            format!("frame-ancestors 'self' {origin}"),
            format!("script-src {origin}/libs/vue/ {origin}/libs/quasar/ {origin}/js/WebrootAssistSandbox.js 'unsafe-eval'"),
            format!("img-src data: {origin}/images/ {origin}/libs/;"),
        ] {
            assert!(csp.contains(&directive), "{directive} is not in {csp}");
        }
        // no allow-same-origin, and nothing from another site
        assert!(!csp.contains("allow-same-origin") && !csp.contains("https:;") && !csp.contains('*'), "{csp}");
        assert_eq!(header(&response, "referrer-policy"), "no-referrer");
        assert_eq!(header(&response, "x-content-type-options"), "nosniff");
        assert!(header(&response, "permissions-policy").contains("camera=()"));
        assert!(header(&response, "x-frame-options").is_empty(), "the site's frame-ancestors decides, not X-Frame-Options");
        // the user's choice of external content adds other sites' images, nothing else
        let external = response_for("localhost:8080", false, &HashMap::from([("x".to_string(), "1".to_string())]));
        let csp = header(&external, "content-security-policy");
        assert!(csp.contains("img-src data: http://localhost:8080/images/ http://localhost:8080/libs/ https:;"), "{csp}");
        assert!(csp.contains("connect-src 'none'"), "{csp}");
        // a Host that is not a plain host name is refused rather than written into the policy
        assert_eq!(response_for("evil.example.com; script-src *", false, &HashMap::new()).status(), 400);
        assert_eq!(response_for("", false, &HashMap::new()).status(), 400);
    }

    #[test]
    fn the_frame_host_follows_a_believed_proxy_then_host_then_the_conf() {
        let conf = ("app.internal:8080", "app.example.com");
        // a believed proxy's X-Forwarded-Host is the public host, over the rewritten Host
        assert_eq!(frame_host(Some("app.example.com"), Some("app.internal:8080"), true, conf, true), "app.example.com");
        // its first value only, trimmed
        assert_eq!(frame_host(Some("app.example.com, inner:9"), Some("x"), true, conf, true), "app.example.com");
        // an unbelieved peer's forwarded header is ignored; the Host is used
        assert_eq!(frame_host(Some("evil.example.com"), Some("app.internal:8080"), false, conf, true), "app.internal:8080");
        // no Host at all (a rewrite stripped it): the conf's host for the scheme, else the other
        assert_eq!(frame_host(None, None, true, conf, true), "app.example.com");
        assert_eq!(frame_host(None, None, true, conf, false), "app.internal:8080");
        assert_eq!(frame_host(None, None, true, ("", "app.example.com"), false), "app.example.com");
        assert_eq!(frame_host(None, None, false, ("", ""), true), "");
    }
}
