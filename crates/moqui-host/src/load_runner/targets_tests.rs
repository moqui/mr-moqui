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
        ssh: parse_ssh_targets(targets),
        own_ssh: None,
        any: false,
    }
}

#[test]
fn ssh_reaches_loopback_this_servers_listener_and_the_listed_hosts() {
    let mut policy = policy(
        &["https://erp.example.com"],
        "https://api.example.com/rest, ssh://Shell.Example.com:2222 ssh://bastion.example.com",
    );
    assert_eq!(policy.allow_ssh("127.0.0.1", 2222), Ok(()));
    assert_eq!(policy.allow_ssh("localhost", 22), Ok(()));
    assert_eq!(policy.allow_ssh("shell.example.com", 2222), Ok(()));
    assert_eq!(policy.allow_ssh("bastion.example.com", 22), Ok(()));
    // another port, another host, and an https prefix's host are not allowed
    assert!(policy.allow_ssh("shell.example.com", 22).is_err());
    let err = policy.allow_ssh("api.example.com", 22).unwrap_err();
    assert!(err.contains("load_runner_targets"), "{err}");
    // the ssh entries are not http prefixes
    assert!(policy.allow("https://shell.example.com:2222/").is_err());
    // this server's own listener, by its name or the address it binds
    assert!(policy.allow_ssh("erp.example.com", 2222).is_err());
    policy.own_ssh = Some("10.0.0.5:2222".parse().unwrap());
    assert_eq!(policy.allow_ssh("erp.example.com", 2222), Ok(()));
    assert_eq!(policy.allow_ssh("10.0.0.5", 2222), Ok(()));
    assert!(policy.allow_ssh("erp.example.com", 2223).is_err());
    // an entry that does not read is left out, the others kept
    assert_eq!(
        parse_ssh_targets("ssh://user@h.example.com:22 ssh://h2.example.com:2200/path ssh://ok.example.com:2201"),
        vec![("ok.example.com".to_string(), 2201)]
    );
}

#[test]
fn loopback_and_this_server_are_allowed() {
    let policy = policy(&["https://erp.example.com", "http://erp.example.com:8080"], "");
    for url in [
        "http://localhost:8080/apps",
        "http://127.0.0.1",
        "http://127.3.4.5:9000/x",
        "http://[::1]:8080/",
        "https://erp.example.com/vapps/marble",
        "https://ERP.example.com:443/",
    ] {
        assert_eq!(policy.allow(url), Ok(()), "{url}");
    }
    // the same host on another port or scheme is not this server
    assert!(policy.allow("https://erp.example.com:8443/").is_err());
    assert!(policy.allow("http://erp.example.com/").is_err());
    // this server's own http URL on a public name: behind a load balancer that is another machine, in the clear
    let err = policy.allow("http://erp.example.com:8080/apps").unwrap_err();
    assert!(err.contains("plain http"), "{err}");
    // a name under .localhost may resolve anywhere
    assert!(policy.allow("https://app.localhost/").is_err());
    assert!(policy.allow("http://localhost.example.com/").is_err());
}

#[test]
fn a_prefix_is_matched_on_the_path_a_request_is_sent_to() {
    let listed = policy(&[], "https://partner.example/loadtest/");
    assert_eq!(listed.allow("https://partner.example/loadtest/orders"), Ok(()));
    assert_eq!(listed.allow("https://partner.example/loadtest/a/../b"), Ok(()));
    for url in [
        "https://partner.example/loadtest/../admin",
        "https://partner.example/loadtest/%2e%2e/admin",
        "https://partner.example/loadtest/%2E%2E/admin",
        "https://partner.example/loadtest/./../admin",
        "https://partner.example/loadtest/.%2e/admin",
        "https://partner.example/loadtest\\..\\admin",
    ] {
        assert!(listed.allow(url).is_err(), "{url}");
    }
    // a prefix written with dot segments means where they lead
    let dotted = policy(&[], "https://partner.example/a/../loadtest/");
    assert_eq!(dotted.allow("https://partner.example/loadtest/x"), Ok(()));
    assert!(dotted.allow("https://partner.example/a/x").is_err());
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
fn target_kinds_are_service_this_and_url() {
    assert_eq!(TargetKind::parse(""), Ok(TargetKind::This));
    assert_eq!(TargetKind::parse("this"), Ok(TargetKind::This));
    assert_eq!(TargetKind::parse("service"), Ok(TargetKind::Service));
    assert_eq!(TargetKind::parse("url"), Ok(TargetKind::Url));
    assert!(TargetKind::parse("ftp").is_err());
}

#[test]
fn scenario_users_must_be_whole_and_positive() {
    let scenarios = |users: Value| {
        let mut browse = indexmap::IndexMap::default();
        browse.insert("users".to_string(), users);
        let mut map = indexmap::IndexMap::default();
        map.insert("browse".to_string(), Value::Map(browse));
        map.insert("order".to_string(), Value::Map(indexmap::IndexMap::default()));
        Value::Map(map)
    };
    let users = override_users(Some(&scenarios(Value::I64(3)))).unwrap();
    assert_eq!(users, BTreeMap::from([("browse".to_string(), 3)]));
    assert_eq!(
        override_users(Some(&scenarios(Value::String("4".into())))).unwrap()["browse"],
        4
    );
    assert_eq!(override_users(Some(&scenarios(Value::F64(5.0)))).unwrap()["browse"], 5);
    assert!(override_users(None).unwrap().is_empty());
    for bad in [
        Value::I64(0),
        Value::I64(-1),
        Value::F64(1.5),
        Value::String("many".into()),
    ] {
        let err = override_users(Some(&scenarios(bad.clone()))).unwrap_err();
        assert!(err.contains("browse"), "{bad:?}: {err}");
    }
    assert_eq!(check_users(1000), Ok(()));
    assert!(check_users(1001).is_err());
}

#[test]
fn limits_fall_back_to_their_defaults() {
    assert_eq!(limit_of("", DEFAULT_MAX_RUNS), 1);
    assert_eq!(limit_of(" 3 ", DEFAULT_MAX_RUNS), 3);
    assert_eq!(limit_of("0", DEFAULT_MAX_USERS), 1000);
    assert_eq!(limit_of("-2", DEFAULT_MAX_USERS), 1000);
    assert_eq!(limit_of("many", DEFAULT_MAX_USERS), 1000);
}
