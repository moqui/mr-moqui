//! Which `Origin` a request may come from: CORS (`MoquiServlet.handleCors`), the sockets and event streams
//! (`WebUtilities.webSocketOriginAllowed`) and the MCP transport.
//!
//! Java compares host names only, so a page on another port or scheme of the same host (another server on the same
//! machine, or the plain-http site of an https one) passed as the server's own. Here the request's own origin is the
//! whole tuple of RFC 6454: scheme, host and port. An `allow-origins` entry with a scheme is matched as a tuple too; a
//! bare domain keeps Java's rule, its host and the hosts under it on any scheme and port.

use crate::context::ExecutionContextFactory;

/// A web origin (RFC 6454): the scheme (`http` or `https`), the host in lower case (an IPv6 address in brackets) and
/// the port, defaulted for the scheme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteOrigin {
    pub scheme: String,
    pub host: String,
    pub port: u16,
}

impl SiteOrigin {
    /// An `Origin` value or a URL: `http` or `https` only, userinfo, path and query left out. None for anything else,
    /// `null` (an opaque origin) included.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().to_ascii_lowercase();
        let (scheme, rest) = text.split_once("://")?;
        let https = match scheme {
            "https" => true,
            "http" => false,
            _ => return None,
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        Self::of_authority(https, authority, None)
    }

    /// The scheme and a `Host`-style authority (`host`, `host:port`, `[v6]:port`, userinfo dropped); a `port` given
    /// replaces the authority's. None for an empty or malformed host or port.
    pub fn of_authority(https: bool, authority: &str, port: Option<&str>) -> Option<Self> {
        let authority = authority.trim().to_ascii_lowercase();
        let authority = authority.rsplit_once('@').map_or(authority.as_str(), |(_, host)| host);
        let (host, own_port) = if let Some(bracketed) = authority.strip_prefix('[') {
            let (address, after) = bracketed.split_once(']')?;
            let port = match after {
                "" => None,
                after => Some(after.strip_prefix(':')?),
            };
            (format!("[{address}]"), port)
        } else {
            match authority.split_once(':') {
                Some((host, port)) => (host.to_string(), Some(port)),
                None => (authority.to_string(), None),
            }
        };
        if host.is_empty()
            || host == "[]"
            || host
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '/' | '\\' | '@' | '?' | '#'))
        {
            return None;
        }
        let default = if https { 443 } else { 80 };
        let port = match port.or(own_port).map(str::trim) {
            None | Some("") => default,
            Some(port) => port.parse::<u16>().ok()?,
        };
        Some(Self {
            scheme: if https { "https" } else { "http" }.to_string(),
            host,
            port,
        })
    }
}

/// The origin a request was made to, as the browser saw it: https over TLS or from a believed proxy's
/// `X-Forwarded-Proto` (`https`, which the host works out); the host and port of a believed proxy's `X-Forwarded-Host`
/// (its first value), else of `Host`; the port of a believed proxy's `X-Forwarded-Port` (its first value) when it
/// sends one. `believed` is [`ClientAddressing::believes`](super::address::ClientAddressing::believes) of the peer.
pub fn request_site(
    https: bool,
    believed: bool,
    host: Option<&str>,
    forwarded_host: Option<&str>,
    forwarded_port: Option<&str>,
) -> Option<SiteOrigin> {
    let first = |value: Option<&str>| {
        value
            .and_then(|value| value.split(',').next())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let (forwarded_host, forwarded_port) = if believed {
        (first(forwarded_host), first(forwarded_port))
    } else {
        (None, None)
    };
    let authority = forwarded_host.or_else(|| first(host))?;
    SiteOrigin::of_authority(https, &authority, forwarded_port.as_deref())
}

/// The conf webroot's own URLs (`webapp_root_url_for`, http and https) as origins.
pub fn configured_origins(factory: &ExecutionContextFactory) -> Vec<SiteOrigin> {
    let mut origins = Vec::new();
    for secure in [false, true] {
        if let Some(origin) = SiteOrigin::parse(&factory.webapp_root_url_for("webroot", Some(secure))) {
            if !origins.contains(&origin) {
                origins.push(origin);
            }
        }
    }
    origins
}

/// The origins a request counts as its own: its [`request_site`], then the [`configured_origins`].
pub fn own_origins(factory: &ExecutionContextFactory, site: Option<SiteOrigin>) -> Vec<SiteOrigin> {
    let mut origins: Vec<SiteOrigin> = site.into_iter().collect();
    for origin in configured_origins(factory) {
        if !origins.contains(&origin) {
            origins.push(origin);
        }
    }
    origins
}

/// The own origins of a request to the screens: [`own_origins`] with the request's https flag, believed by the
/// peer's address as the factory's `ClientAddressing` says.
pub(crate) fn request_own_origins(factory: &ExecutionContextFactory, request: &super::WebRequest) -> Vec<SiteOrigin> {
    let believed = request.peer_ip.trim().parse::<std::net::IpAddr>().is_ok_and(|peer| {
        factory
            .extension(super::address::ClientAddressing::of_factory)
            .believes(peer)
    });
    let site = request_site(
        request.https,
        believed,
        request.header("host"),
        request.header("x-forwarded-host"),
        request.header("x-forwarded-port"),
    );
    own_origins(factory, site)
}

/// Whether a request from `origin` may go on, as Java's `WebUtilities.webSocketOriginAllowed` but with the whole
/// origin compared: no Origin (not a browser); an `allow-origins` of `*`; one of `own` (scheme, host and port); an
/// `allow-origins` entry with a scheme, matched the same way (`https://app.example:8443`, its port defaulted); or a
/// bare `allow-origins` domain (`moqui.org`), which allows that host and any host under it on a dot boundary
/// (`www.moqui.org`, not `foo-moqui.org`) on any scheme and port, as Java did (a bare `moqui.org:8443` on that port
/// only; an IP address names itself, never hosts under it). Case-insensitive. An Origin that is not an http or https
/// origin, `null` included, is refused unless `*`. With no factory, the current context's conf.
pub fn origin_allowed_by(factory: Option<&ExecutionContextFactory>, origin: Option<&str>, own: &[SiteOrigin]) -> bool {
    let Some(origin) = origin.map(str::trim).filter(|text| !text.is_empty()) else {
        return true;
    };
    let allow = allowed_entries(factory);
    allow.iter().any(|entry| entry == "*") || origin_named(&allow, origin, own)
}

/// [`origin_allowed_by`] without `*`: whether a request from `origin` may carry the caller's credentials, the CORS
/// answer's `Access-Control-Allow-Credentials` or a socket that the browser opens with the session cookie. `*` lets
/// any origin read what needs no credentials, never a signed-in user's answers (SEC-WEB-10).
pub fn origin_credentialed_by(
    factory: Option<&ExecutionContextFactory>,
    origin: Option<&str>,
    own: &[SiteOrigin],
) -> bool {
    let Some(origin) = origin.map(str::trim).filter(|text| !text.is_empty()) else {
        return true;
    };
    origin_named(&allowed_entries(factory), origin, own)
}

fn allowed_entries(factory: Option<&ExecutionContextFactory>) -> Vec<String> {
    super::allow_origins(factory)
        .iter()
        .map(|text| text.to_ascii_lowercase())
        .collect()
}

/// Whether `origin` is one of `own` or named by an `allow-origins` entry other than `*`.
fn origin_named(allow: &[String], origin: &str, own: &[SiteOrigin]) -> bool {
    let Some(origin) = SiteOrigin::parse(origin) else {
        return false;
    };
    if own.contains(&origin) {
        return true;
    }
    allow.iter().any(|entry| {
        if entry.contains("://") {
            return SiteOrigin::parse(entry).is_some_and(|allowed| allowed == origin);
        }
        // a bare domain may name a port, which then must match; the scheme does not matter
        let Some(bare) = SiteOrigin::of_authority(origin.scheme == "https", entry.trim_matches('.'), None) else {
            return false;
        };
        let domain = bare.host.as_str();
        let port_named = entry.rsplit_once(':').is_some_and(|(_, port)| !port.contains(']'));
        let address = domain.parse::<std::net::IpAddr>().is_ok();
        (!port_named || bare.port == origin.port)
            && (origin.host == domain
                || (!address && domain.contains('.') && origin.host.ends_with(&format!(".{domain}"))))
    })
}

#[cfg(test)]
mod tests {
    use super::{origin_allowed_by, origin_credentialed_by, request_site, SiteOrigin};

    fn site(text: &str) -> SiteOrigin {
        SiteOrigin::parse(text).unwrap()
    }

    #[test]
    fn an_origin_parses_to_scheme_host_and_port() {
        assert_eq!(
            site("HTTPS://User:pw@Shop.Example"),
            SiteOrigin {
                scheme: "https".to_string(),
                host: "shop.example".to_string(),
                port: 443
            }
        );
        assert_eq!(site("http://shop.example").port, 80);
        assert_eq!(site("http://shop.example:80/path?q"), site("http://shop.example"));
        assert_eq!(site("https://[::1]:8443").host, "[::1]");
        assert_eq!(site("https://[::1]:8443").port, 8443);
        for bad in [
            "null",
            "",
            "ftp://shop.example",
            "shop.example",
            "http://",
            "http://a:99999",
            "http://[::1",
        ] {
            assert_eq!(SiteOrigin::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_request_site_follows_a_believed_proxy_only() {
        let own = request_site(
            false,
            false,
            Some("erp.example:8080"),
            Some("shop.example"),
            Some("8443"),
        );
        assert_eq!(own, Some(site("http://erp.example:8080")), "not believed");
        let proxied = request_site(true, true, Some("10.0.0.5:8080"), Some("shop.example, inner"), None);
        assert_eq!(proxied, Some(site("https://shop.example")));
        let ported = request_site(true, true, Some("10.0.0.5:8080"), Some("shop.example"), Some("8443"));
        assert_eq!(ported, Some(site("https://shop.example:8443")));
        let host_ported = request_site(false, true, Some("shop.example"), None, Some("8000"));
        assert_eq!(host_ported, Some(site("http://shop.example:8000")));
        assert_eq!(request_site(false, false, None, None, None), None);
    }

    /// Stricter than Java, which compares the host only: the request's own origin is its scheme, host and port.
    #[test]
    fn an_origin_is_allowed_by_its_scheme_host_and_port() {
        let _lock = crate::user::session::cookie_property_lock();
        moqui_lib::env::set_property(
            "webapp_allow_origins",
            "moqui.org, https://app.example:8443, http://plain.example",
        );
        let own = [site("http://site.test:8080")];
        let allowed = |origin: Option<&str>| origin_allowed_by(None, origin, &own);
        // no Origin (not a browser), the request's own origin
        assert!(allowed(None));
        assert!(allowed(Some("HTTP://Site.Test:8080")));
        // the same host on another port or scheme is another site
        assert!(!allowed(Some("http://site.test")));
        assert!(!allowed(Some("http://site.test:9090")));
        assert!(!allowed(Some("https://site.test:8080")));
        // the default port is the same as none
        assert!(origin_allowed_by(
            None,
            Some("https://site.test:443"),
            &[site("https://site.test")]
        ));
        assert!(origin_allowed_by(
            None,
            Some("http://site.test"),
            &[site("http://site.test:80")]
        ));
        // an entry with a scheme is the whole origin, its port defaulted
        assert!(allowed(Some("https://app.example:8443")));
        assert!(!allowed(Some("https://app.example")));
        assert!(!allowed(Some("http://app.example:8443")));
        assert!(allowed(Some("http://plain.example:80")));
        assert!(!allowed(Some("https://plain.example")));
        // a bare domain is its host and the hosts under it on a dot boundary, any scheme and port, as Java
        assert!(allowed(Some("https://www.MOQUI.org")));
        assert!(allowed(Some("http://moqui.org:8080")));
        assert!(!allowed(Some("https://foo-moqui.org")));
        assert!(!allowed(Some("https://evil.test")));
        moqui_lib::env::set_property("webapp_allow_origins", "ported.example:8443, [::2]");
        assert!(allowed(Some("http://www.ported.example:8443")));
        assert!(!allowed(Some("https://ported.example")));
        assert!(allowed(Some("http://[::2]:3000")));
        // an opaque or non-web origin
        assert!(!allowed(Some("null")));
        assert!(!allowed(Some("file://site.test")));
        moqui_lib::env::set_property("webapp_allow_origins", "*");
        assert!(allowed(Some("null")));
        moqui_lib::env::clear_property("webapp_allow_origins");
    }

    /// SEC-WEB-10: `*` allows any origin but never with credentials; the dev conf names only this machine, on any port
    /// and either scheme, and an IP address entry names no host under it.
    #[test]
    fn a_wildcard_origin_carries_no_credentials_and_the_dev_conf_allows_only_this_machine() {
        let _lock = crate::user::session::cookie_property_lock();
        let own = [site("http://site.test:8080")];
        moqui_lib::env::set_property("webapp_allow_origins", "*");
        assert!(origin_allowed_by(None, Some("https://evil.test"), &own));
        assert!(!origin_credentialed_by(None, Some("https://evil.test"), &own));
        assert!(
            origin_credentialed_by(None, Some("http://site.test:8080"), &own),
            "its own origin"
        );
        assert!(origin_credentialed_by(None, None, &own), "no Origin: not a browser");

        let conf = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../runtime/conf/MoquiDevConf.xml"
        ))
        .expect("the dev conf");
        let value = conf
            .split("name=\"webapp_allow_origins\" value=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .expect("the dev conf names webapp_allow_origins");
        assert!(!value.split(',').any(|entry| entry.trim() == "*"), "{value}");
        moqui_lib::env::set_property("webapp_allow_origins", value);
        let credentialed = |origin: &str| origin_credentialed_by(None, Some(origin), &own);
        assert!(credentialed("http://localhost:5173"));
        assert!(credentialed("https://localhost:8443"));
        assert!(credentialed("http://127.0.0.1:3000"));
        assert!(!credentialed("https://evil.test"));
        assert!(!credentialed("http://www.localhost:5173"));
        assert!(!credentialed("http://evil.127.0.0.1:3000"));
        assert!(!origin_allowed_by(None, Some("https://evil.test"), &own));
        moqui_lib::env::clear_property("webapp_allow_origins");
    }
}
