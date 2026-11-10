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

use super::*;

fn policy(own: &[&str], targets: &str) -> TargetPolicy {
    TargetPolicy {
        own: own.iter().filter_map(|url| SiteOrigin::parse(url)).collect(),
        prefixes: parse_prefixes(targets),
        any: false,
    }
}

#[test]
fn loopback_and_this_server_are_allowed() {
    let policy = policy(&["https://erp.example.com", "http://erp.example.com:8080"], "");
    for url in [
        "http://localhost:8080/apps",
        "http://127.0.0.1",
        "http://127.3.4.5:9000/x",
        "http://[::1]:8080/",
        "https://app.localhost/",
        "https://erp.example.com/vapps/marble",
        "https://ERP.example.com:443/",
        "http://erp.example.com:8080/apps",
    ] {
        assert_eq!(policy.allow(url), Ok(()), "{url}");
    }
    // the same host on another port or scheme is not this server
    assert!(policy.allow("https://erp.example.com:8443/").is_err());
    assert!(policy.allow("http://erp.example.com/").is_err());
}

#[test]
fn another_site_is_refused_unless_listed() {
    let open = policy(&[], "");
    let err = open.allow("https://example.org/").unwrap_err();
    assert!(err.contains("load_runner_targets"), "{err}");

    let listed = policy(&[], "https://staging.example.com, https://example.org/app/");
    assert_eq!(listed.allow("https://staging.example.com/anything?x=1"), Ok(()));
    assert_eq!(listed.allow("https://example.org/app"), Ok(()));
    assert_eq!(listed.allow("https://example.org/app/orders"), Ok(()));
    // a prefix is matched by origin and on a path segment, never as text
    assert!(listed.allow("https://example.org/application").is_err());
    assert!(listed.allow("https://example.org/").is_err());
    assert!(listed.allow("https://staging.example.com.evil.net/").is_err());
    assert!(listed.allow("https://staging.example.com:8443/").is_err());
}

#[test]
fn plain_http_to_another_machine_needs_its_own_entry() {
    let https_only = policy(&[], "https://staging.example.com");
    let err = https_only.allow("http://staging.example.com/").unwrap_err();
    assert!(err.contains("plain http"), "{err}");

    let named = policy(&[], "http://10.0.0.5:8080");
    assert_eq!(named.allow("http://10.0.0.5:8080/apps"), Ok(()));
    assert!(named.allow("http://10.0.0.6:8080/apps").is_err());
}

#[test]
fn a_dev_instance_reaches_any_url_but_not_another_scheme() {
    let dev = TargetPolicy {
        any: true,
        ..TargetPolicy::default()
    };
    assert_eq!(dev.allow("http://example.org/"), Ok(()));
    assert!(dev.allow("ftp://example.org/").is_err());
    assert!(dev.allow("example.org").is_err());
}

#[test]
fn userinfo_and_malformed_urls_are_refused() {
    let policy = policy(&[], "https://example.org");
    // reaches example.com, not loopback
    assert!(policy.allow("http://127.0.0.1@example.com/").is_err());
    assert!(policy.allow("https://user@example.org/").is_err());
    assert!(policy.allow("http://local\\host/").is_err());
    assert!(policy.allow("").is_err());
    assert!(policy.allow("http://:80/").is_err());
    // a bad entry is left out, the rest kept
    assert_eq!(parse_prefixes("nonsense, https://ok.example").len(), 1);
}

#[test]
fn limits_fall_back_to_their_defaults() {
    assert_eq!(limit_of("", DEFAULT_MAX_RUNS), 1);
    assert_eq!(limit_of(" 3 ", DEFAULT_MAX_RUNS), 3);
    assert_eq!(limit_of("0", DEFAULT_MAX_USERS), 1000);
    assert_eq!(limit_of("-2", DEFAULT_MAX_USERS), 1000);
    assert_eq!(limit_of("many", DEFAULT_MAX_USERS), 1000);
}
