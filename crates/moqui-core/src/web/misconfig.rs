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

//! Checks from Java's `SecurityMisconfigTests` that read the shipped conf or call a helper that exists.
//!
//! No function here for `hostAllowedByConf` (beyond the mail client's `allowed-hosts` list),
//! `isSafeSinglePathSegment`, or `simplifyRequestParameters`. Those helpers are not added.

use moqui_lib::node::MNode;

fn default_conf() -> MNode {
    let path = crate::testing::framework_dir().join("conf/MoquiDefaultConf.xml");
    MNode::parse_file(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

fn webroot(conf: &MNode) -> MNode {
    conf.first_named("webapp-list")
        .expect("webapp-list")
        .first_named_attrs("webapp", &[("name", "webroot")])
        .expect("webroot")
}

/// A webroot attribute as shipped. `${name}` is that file's `default-property`, not the process environment.
fn shipped_attr(conf: &MNode, name: &str) -> String {
    let raw = webroot(conf)
        .attribute(name)
        .unwrap_or_else(|| panic!("webroot has no {name}"));
    let Some(property) = raw.strip_prefix("${").and_then(|body| body.strip_suffix('}')) else {
        return raw;
    };
    conf.first_named_attrs("default-property", &[("name", property.trim())])
        .unwrap_or_else(|| panic!("no default-property {property} for webroot {name}"))
        .attribute("value")
        .unwrap_or_default()
}

#[test]
fn default_conf_handle_cors_is_true_and_allow_origins_is_empty() {
    let conf = default_conf();
    assert_eq!(shipped_attr(&conf, "handle-cors"), "true");
    assert_eq!(shipped_attr(&conf, "allow-origins"), "");
}

#[test]
fn default_conf_tarpit_is_on_for_screens_transitions_and_services_and_off_for_entities() {
    let conf = default_conf();
    let facade = conf
        .first_named("artifact-execution-facade")
        .expect("artifact-execution-facade");
    let tarpit = |kind: &str| {
        facade
            .first_named_attrs("artifact-execution", &[("type", kind)])
            .unwrap_or_else(|| panic!("no artifact-execution {kind}"))
            .attribute("tarpit-enabled")
            .unwrap_or_else(|| panic!("{kind} has no tarpit-enabled"))
    };
    assert_eq!(tarpit("AT_XML_SCREEN"), "true");
    assert_eq!(tarpit("AT_XML_SCREEN_TRANS"), "true");
    assert_eq!(tarpit("AT_SERVICE"), "true");
    assert_eq!(tarpit("AT_ENTITY"), "false");
}

#[test]
fn default_conf_screen_secure_sets_hsts() {
    let conf = default_conf();
    let value = webroot(&conf)
        .first_named_attrs(
            "response-header",
            &[("type", "screen-secure"), ("name", "Strict-Transport-Security")],
        )
        .expect("screen-secure Strict-Transport-Security")
        .attribute("value")
        .expect("hsts value");
    assert!(value.contains("max-age="), "{value}");
}

/// The mail client reads `email-client` `allowed-hosts` (`client_setting("allowed-hosts")`).
#[test]
fn default_conf_email_client_allowed_hosts_is_empty() {
    let conf = default_conf();
    let hosts = conf
        .first_named("email-client")
        .expect("email-client")
        .attribute("allowed-hosts");
    assert_eq!(hosts.as_deref(), Some(""));
}

#[test]
fn default_conf_upload_executable_allow_is_false() {
    let conf = default_conf();
    assert_eq!(shipped_attr(&conf, "upload-executable-allow"), "false");
}

/// `moqui.security.hmac.replay` is `type="distributed"`. The live cache stays local unless a distributed
/// cache is configured (`docs/security/Proof-Tests.md`).
#[test]
fn default_conf_declares_the_hmac_replay_cache_distributed() {
    let conf = default_conf();
    let cache = conf
        .first_named("cache-list")
        .expect("cache-list")
        .first_named_attrs("cache", &[("name", "moqui.security.hmac.replay")])
        .expect("moqui.security.hmac.replay");
    assert_eq!(cache.attribute("type").as_deref(), Some("distributed"));
}

/// The demo `ArtifactTarpit` row. The file is read; nothing is requested over HTTP.
#[test]
fn demo_data_has_the_all_screens_tarpit_row() {
    let path = crate::testing::framework_dir()
        .parent()
        .expect("base-component")
        .join("webroot/data/WebrootSecurityDemoData.xml");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    assert!(text.contains("artifactGroupId=\"ALL_SCREENS\""), "{}", path.display());
    assert!(text.contains("maxHitsCount=\"120\""), "{}", path.display());
    assert!(text.contains("userGroupId=\"ALL_USERS\""), "{}", path.display());
}

#[test]
fn is_same_origin_redirect_matches_the_groovy_rows() {
    let rows = [
        ("/apps", true),
        ("https://localhost/apps", true),
        ("https://evil.example/phish", false),
        ("//evil.example", false),
        ("javascript:alert(1)", false),
        ("data:text/html,hi", false),
        ("vbscript:msg", false),
        ("/\\evil.example", false),
        ("https://localhost@evil.example/", false),
        ("", false),
        ("/apps\r\nLocation: https://evil.example", false),
        ("/apps\0evil", false),
    ];
    for (url, allowed) in rows {
        assert_eq!(super::is_same_origin_redirect(url, Some("localhost")), allowed, "{url}");
    }
}

#[test]
fn an_absolute_redirect_is_refused_when_the_configured_host_is_empty() {
    assert!(super::is_same_origin_redirect("/apps", Some("")));
    assert!(super::is_same_origin_redirect("/apps", None));
    assert!(!super::is_same_origin_redirect("https://localhost/apps", Some("")));
    assert!(!super::is_same_origin_redirect("https://localhost/apps", None));
    assert!(!super::is_same_origin_redirect("https://evil.example/phish", Some("")));
}

/// `X-Forwarded-For` is not the client when `client-ip-header` is empty. The peer address is.
#[test]
fn client_ip_ignores_x_forwarded_for_when_the_header_is_empty() {
    let _lock = crate::user::session::cookie_property_lock();
    assert!(
        super::client_addressing().header().is_empty(),
        "webapp_client_ip_header is set; this proof needs the shipped empty header"
    );
    let mut request = super::WebRequest::new("GET", "/status");
    request.peer_ip = "203.0.113.9".to_string();
    request.set_header("x-forwarded-for", "127.0.0.1");
    request.set_header("x-real-ip", "198.51.100.7");
    assert_eq!(super::client_ip(&request), "203.0.113.9");
}

/// `parse_hop` in `web/address.rs` canonicalizes a hop: both Java loopback spellings are one address.
#[test]
fn loopback_forms_canonicalize_to_one_address() {
    let compressed = super::address::parse_hop("::1");
    let full = super::address::parse_hop("0:0:0:0:0:0:0:1");
    assert_eq!(compressed, full);
    assert_eq!(compressed.expect("loopback").to_string(), "::1");
}

/// Empty `Origin` is allowed. The same host counts only with the same scheme and port.
/// Java's `webSocketOriginAllowed` matched the host only; this does not assert that (`docs/security/Proof-Tests.md`).
#[test]
fn an_empty_origin_is_allowed_and_the_same_site_matches_scheme_host_and_port() {
    let _lock = crate::user::session::cookie_property_lock();
    let _allow = hold_property("webapp_allow_origins", "not-this-test.example");
    let own = [super::origin::SiteOrigin::parse("http://localhost:8080").expect("origin")];
    let allowed = |origin: Option<&str>| super::origin::origin_allowed_by(None, origin, &own);
    assert!(allowed(None));
    assert!(allowed(Some("")));
    assert!(allowed(Some("http://localhost:8080")));
    assert!(!allowed(Some("http://localhost:9090")));
    assert!(!allowed(Some("https://localhost:8080")));
    assert!(!allowed(Some("https://evil.example")));
}

/// Password keys are dropped without regard to case. The value is edited in place.
#[test]
fn strip_credential_parameters_drops_password_keys_without_regard_to_case() {
    let mut value = serde_json::json!({
        "username": "sec.none.only",
        "password": "SecNone1!!",
        "newPassword": "x",
        "NEWPASSWORDVERIFY": "x",
        "authPassword": "y",
        "api_key": "k",
        "locale": "en"
    });
    super::strip_credential_parameters(&mut value);
    let map = value.as_object().expect("object");
    assert_eq!(
        map.get("username").and_then(serde_json::Value::as_str),
        Some("sec.none.only")
    );
    assert_eq!(map.get("locale").and_then(serde_json::Value::as_str), Some("en"));
    for name in [
        "password",
        "newPassword",
        "NEWPASSWORDVERIFY",
        "authPassword",
        "api_key",
    ] {
        assert!(map.get(name).is_none(), "{name}");
    }
}

/// Clears `name` when dropped, including when an assertion fails.
struct ClearProperty(&'static str);

impl Drop for ClearProperty {
    fn drop(&mut self) {
        moqui_lib::env::clear_property(self.0);
    }
}

fn hold_property(name: &'static str, value: &str) -> ClearProperty {
    moqui_lib::env::set_property(name, value);
    ClearProperty(name)
}
