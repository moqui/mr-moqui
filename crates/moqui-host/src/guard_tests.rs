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

use std::net::IpAddr;
use std::sync::Arc;

use axum::http::HeaderMap;
use moqui_core::web::address::{parse_cidrs, ClientAddressing};
use moqui_lib::node::MNode;

use super::{decode_path, Event, Glob, Guard, GuardConf};

fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

pub(crate) fn guard(xml: &str, trusted: &str) -> Arc<Guard> {
    let node = MNode::parse_text("web-protection", xml).unwrap();
    let addressing = Arc::new(ClientAddressing::new("X-Forwarded-For", parse_cidrs(trusted, "test")));
    Arc::new(Guard::new(GuardConf::from_node(Some(&node), addressing)))
}

fn status(guard: &Guard, client: &str, method: &str, path: &str) -> u16 {
    let client = ip(client);
    match guard.check(client, client, "", method, path, &HeaderMap::new()) {
        Ok(()) => 200,
        Err(refused) => refused.response.status().as_u16(),
    }
}

#[test]
fn globs_match_url_patterns() {
    let glob = |pattern: &str| Glob::new(pattern).unwrap();
    assert!(glob("/.git/**").matches("/.git/config"));
    assert!(glob("/.git/**").matches("/.git"));
    assert!(!glob("/.git/**").matches("/x/.git/config"));
    assert!(glob("**/*.php").matches("/wp/xmlrpc.php"));
    assert!(glob("**/*.php").matches("/index.PHP"));
    assert!(!glob("**/*.php").matches("/apps/php"));
    assert!(glob("**/*.asp*").matches("/a/b/default.aspx"));
    assert!(glob("/Login/**").matches("/login/login"));
    assert!(glob("/rest/login").matches("/rest/login"));
    assert!(!glob("/rest/login").matches("/rest/login/x"));
    assert_eq!(decode_path("/%2e%67it/config"), "/.git/config");
    assert_eq!(decode_path("/a+b%zz"), "/a+b%zz");
}

#[test]
fn off_without_trusted_proxies_unless_enabled() {
    assert!(!guard(r#"<web-protection/>"#, "").conf().enabled);
    assert!(guard(r#"<web-protection/>"#, "10.0.0.0/8").conf().enabled);
    assert!(guard(r#"<web-protection enabled="true"/>"#, "").conf().enabled);
    assert!(
        !guard(r#"<web-protection enabled="false"/>"#, "10.0.0.0/8")
            .conf()
            .enabled
    );
    let off = guard(
        r#"<web-protection><rate-limit name="all" requests="1" burst="1"/></web-protection>"#,
        "",
    );
    for _ in 0..5 {
        assert_eq!(status(&off, "203.0.113.5", "GET", "/"), 200);
    }
}

#[test]
fn a_rate_limit_answers_429_past_its_burst_and_only_where_it_applies() {
    let guard = guard(
        r#"<web-protection enabled="true">
                <rate-limit name="all" requests="1" seconds="60" burst="3"/>
                <rate-limit name="login" path="/Login/**" methods="POST" requests="1" seconds="60" burst="1"/>
            </web-protection>"#,
        "",
    );
    assert_eq!(status(&guard, "203.0.113.5", "POST", "/Login/login"), 200);
    // the login limit refuses the second POST, and its refusal takes no token from the other limit
    assert_eq!(status(&guard, "203.0.113.5", "POST", "/Login/login"), 429);
    assert_eq!(status(&guard, "203.0.113.5", "GET", "/Login/login"), 200);
    assert_eq!(status(&guard, "203.0.113.5", "GET", "/apps"), 200);
    assert_eq!(status(&guard, "203.0.113.5", "GET", "/apps"), 429);
    // another client has its own tokens
    assert_eq!(status(&guard, "203.0.113.6", "GET", "/apps"), 200);
    let client = ip("203.0.113.5");
    let refused = guard
        .check(client, client, "", "GET", "/", &HeaderMap::new())
        .err()
        .unwrap();
    let retry: u64 = refused.response.headers()["retry-after"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=60).contains(&retry), "{retry}");
}

/// An `anonymous="true"` limit counts only the requests with no `Authorization`, `api_key` or `login_key` header: a
/// clone of a public repository, not a signed-in client's (docs/interfaces/Git-Hosting.md, Access).
#[test]
fn an_anonymous_rate_limit_counts_only_requests_without_credentials() {
    let guard = guard(
        r#"<web-protection enabled="true">
                <rate-limit name="git-anonymous" path="/git/**,**/info/refs" anonymous="true" requests="1"
                        seconds="60" burst="2"/>
            </web-protection>"#,
        "",
    );
    let client = ip("203.0.113.5");
    let with = |name: &'static str, value: &'static str| {
        let mut headers = HeaderMap::new();
        headers.insert(name, value.parse().unwrap());
        guard
            .check(client, client, "", "GET", "/git/code.git/info/refs", &headers)
            .map_or_else(|refused| refused.response.status().as_u16(), |()| 200)
    };
    assert_eq!(status(&guard, "203.0.113.5", "GET", "/git/code.git/info/refs"), 200);
    assert_eq!(status(&guard, "203.0.113.5", "GET", "/moqui/mr-moqui/info/refs"), 200);
    assert_eq!(
        status(&guard, "203.0.113.5", "POST", "/git/code.git/git-upload-pack"),
        429
    );
    // the same client with credentials is not counted, whatever kind they are
    for _ in 0..3 {
        assert_eq!(with("authorization", "Basic am9objpzZWNyZXQ="), 200);
    }
    assert_eq!(with("api_key", "k"), 200);
    assert_eq!(with("login_key", "k"), 200);
    // and its requests took no token: it is still refused without them, and other paths are not limited
    assert_eq!(status(&guard, "203.0.113.5", "GET", "/git/code.git/info/refs"), 429);
    assert_eq!(status(&guard, "203.0.113.5", "GET", "/apps"), 200);
}

/// A path rule is asked again for the path a request is served at when that is not the path it came with (git at a
/// page's URL, served under `/git/`): an operator's deny of `/git/**` stops it too.
#[test]
fn a_path_rule_denies_the_path_a_request_is_served_at() {
    let guard = guard(
        r#"<web-protection enabled="true">
                <path-rule name="no-git" action="deny" path="/git/**"/>
                <client-rule name="office" action="exempt" cidr="198.51.100.0/24"/>
            </web-protection>"#,
        "",
    );
    let served = |client: &str, path: &str| {
        guard
            .check_served_path(ip(client), "", path)
            .map_or_else(|refused| refused.response.status().as_u16(), |()| 200)
    };
    // the page's URL passes the rules by its own path
    assert_eq!(status(&guard, "203.0.113.5", "GET", "/moqui/mr-moqui/info/refs"), 200);
    assert_eq!(served("203.0.113.5", "/git/moqui/mr-moqui/info/refs"), 404);
    assert_eq!(served("203.0.113.5", "/%67it/moqui/mr-moqui/info/refs"), 404);
    assert_eq!(served("203.0.113.5", "/moqui/mr-moqui"), 200);
    assert_eq!(served("198.51.100.7", "/git/moqui/mr-moqui/info/refs"), 200);
    assert_eq!(guard.counts.denied.load(std::sync::atomic::Ordering::Relaxed), 2);
}

#[test]
fn rules_deny_by_client_method_path_and_agent_and_exempt_skips_them() {
    let guard = guard(
        r#"<web-protection enabled="true">
                <requests methods="GET,POST"/>
                <client-rule action="exempt" cidr="127.0.0.0/8"/>
                <client-rule action="deny" cidr="198.51.100.0/24, nope"/>
                <path-rule action="deny" path="/.env,/.git/**,**/*.php"/>
                <user-agent-rule action="deny" contains="sqlmap"/>
            </web-protection>"#,
        "",
    );
    assert_eq!(status(&guard, "198.51.100.9", "GET", "/"), 403);
    assert_eq!(status(&guard, "203.0.113.5", "DELETE", "/"), 405);
    assert_eq!(status(&guard, "203.0.113.5", "GET", "/%2Eenv"), 404);
    assert_eq!(status(&guard, "203.0.113.5", "GET", "/x/index.php"), 404);
    assert_eq!(status(&guard, "127.0.0.1", "GET", "/x/index.php"), 200);
    assert_eq!(status(&guard, "203.0.113.5", "GET", "/apps"), 200);
    let mut headers = HeaderMap::new();
    headers.insert("user-agent", "sqlmap/1.7".parse().unwrap());
    let client = ip("203.0.113.5");
    let refused = guard.check(client, client, "", "GET", "/apps", &headers).err().unwrap();
    assert_eq!(refused.response.status(), 403);
    assert_eq!(refused.response.headers()["connection"], "close");
}

#[test]
fn events_past_a_threshold_ban_the_client_and_its_connections() {
    let guard = guard(
        r#"<web-protection enabled="true">
                <ban name="auth-failure" events="auth-failure" threshold="3" seconds="600" duration="900"/>
            </web-protection>"#,
        "",
    );
    let client = ip("203.0.113.5");
    guard.record(client, Event::AuthFailure);
    guard.record(client, Event::AuthFailure);
    assert_eq!(status(&guard, "203.0.113.5", "GET", "/"), 200);
    assert!(guard.admit(client).is_some());
    guard.record(client, Event::AuthFailure);
    assert_eq!(status(&guard, "203.0.113.5", "GET", "/"), 403);
    assert!(guard.admit(client).is_none());
    // a rate-limited event does not count toward this ban
    assert_eq!(status(&guard, "203.0.113.6", "GET", "/"), 200);
    assert!(
        guard.metrics().contains("edge_banned_clients 1\n"),
        "{}",
        guard.metrics()
    );
}

#[test]
fn connections_per_client_are_capped_and_given_back() {
    let guard = guard(
        r#"<web-protection enabled="true"><connections max-per-client="2"/></web-protection>"#,
        "10.0.0.0/8",
    );
    let client = ip("203.0.113.5");
    let first = guard.admit(client).unwrap();
    let _second = guard.admit(client).unwrap();
    assert!(guard.admit(client).is_none());
    drop(first);
    assert!(guard.admit(client).is_some());
    // a trusted proxy carries many clients and is not capped
    let proxy = ip("10.0.0.2");
    let held: Vec<_> = (0..5).map(|_| guard.admit(proxy).unwrap()).collect();
    assert_eq!(held.len(), 5);
}

#[test]
fn ipv6_clients_share_their_prefix() {
    let guard = guard(
        r#"<web-protection enabled="true"><rate-limit name="all" requests="1" seconds="60" burst="1"/></web-protection>"#,
        "",
    );
    assert_eq!(status(&guard, "2001:db8:1:2::10", "GET", "/"), 200);
    assert_eq!(status(&guard, "2001:db8:1:2::99", "GET", "/"), 429);
    assert_eq!(status(&guard, "2001:db8:1:3::10", "GET", "/"), 200);
}

#[test]
fn a_full_table_serves_new_clients_untracked() {
    let guard = guard(
        r#"<web-protection enabled="true" max-tracked-clients="1">
                <rate-limit name="all" requests="1" seconds="60" burst="1"/>
            </web-protection>"#,
        "",
    );
    assert_eq!(status(&guard, "203.0.113.5", "GET", "/"), 200);
    for _ in 0..3 {
        assert_eq!(status(&guard, "203.0.113.6", "GET", "/"), 200);
    }
    assert!(guard.metrics().contains("edge_untracked 3\n"), "{}", guard.metrics());
}

#[test]
fn the_default_conf_is_off_until_there_are_trusted_proxies() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/components/base/framework/conf/MoquiDefaultConf.xml"
    ))
    .unwrap();
    let node = super::protection_node(&text);
    assert!(node.is_some(), "MoquiDefaultConf.xml has a web-protection");
    let off = GuardConf::from_node(node.as_ref(), Arc::new(ClientAddressing::default()));
    assert!(!off.enabled);
    let behind = Arc::new(ClientAddressing::new("X-Real-IP", parse_cidrs("10.0.0.0/8", "test")));
    let on = GuardConf::from_node(node.as_ref(), behind);
    assert!(on.enabled);
    let names: Vec<&str> = on.rate_limits.iter().map(|limit| limit.name.as_str()).collect();
    assert_eq!(names, ["all", "login", "git-anonymous"]);
    // git with no credentials, under /git/ and at a page's URL; a page is not counted
    let git = &on.rate_limits[2];
    assert!(git.anonymous);
    for path in [
        "/git/moqui/mr-moqui.git/info/refs",
        "/moqui/mr-moqui/info/refs",
        "/moqui/mr-moqui.git/git-upload-pack",
        "/moqui/mr-moqui.git/info/lfs/objects/batch",
    ] {
        assert!(git.applies("", "POST", path, false), "{path}");
        assert!(!git.applies("", "POST", path, true), "{path}");
    }
    assert!(!git.applies("", "GET", "/moqui/mr-moqui/-/issues", false));
    assert_eq!(on.bans.len(), 2);
    let exempt = |address: &str| {
        on.client_rules
            .iter()
            .any(|rule| !rule.deny && rule.cidrs.iter().any(|range| range.contains(ip(address))))
    };
    assert!(exempt("127.0.0.1"));
    assert!(exempt("::1"));
    let denied = |path: &str| {
        on.path_rules
            .iter()
            .any(|rule| rule.globs.iter().any(|glob| glob.matches(path)))
    };
    assert!(denied("/.env"));
    assert!(!denied("/apps/system"));
    assert_eq!(on.methods.len(), 7);
    assert_eq!(on.max_per_client, 64);
    assert_eq!(on.queue_timeout, Some(std::time::Duration::from_secs(10)));
    assert_eq!(on.retry_after, 5);
    let limits = crate::http::ListenLimits::from_node(node.as_ref());
    assert_eq!(limits.max_uri, 8192);
    assert_eq!(limits.max_header_bytes, 65536);
    assert_eq!(limits.max_headers, 100);
}

#[test]
fn bad_entries_are_left_out_and_the_rest_apply() {
    let guard = guard(
        r#"<web-protection enabled="true">
                <rate-limit name="bad" requests="many"/>
                <rate-limit name="off" requests="1" burst="1" enabled="false"/>
                <rate-limit name="good" requests="1" seconds="60" burst="1"/>
                <ban name="bad" events="nonsense" threshold="1" seconds="1" duration="1"/>
            </web-protection>"#,
        "",
    );
    assert_eq!(guard.conf().rate_limits.len(), 1);
    assert!(guard.conf().bans.is_empty());
    assert_eq!(status(&guard, "203.0.113.5", "GET", "/"), 200);
    assert_eq!(status(&guard, "203.0.113.5", "GET", "/"), 429);
}

/// A conf reload gives the guard new rules in place (plans/archive/Component-Deploy.md): a client seen before counts against
/// the new rate limit, whose window starts again, with no index past the old rule count.
#[test]
fn replaced_rules_apply_to_clients_already_seen() {
    let guard = guard(
        r#"<web-protection enabled="true"><rate-limit name="all" requests="1" seconds="60" burst="1"/></web-protection>"#,
        "",
    );
    assert_eq!(status(&guard, "203.0.113.9", "GET", "/"), 200);
    assert_eq!(status(&guard, "203.0.113.9", "GET", "/"), 429);
    let node = MNode::parse_text(
        "web-protection",
        r#"<web-protection enabled="true">
                <rate-limit name="all" requests="100" seconds="60" burst="100"/>
                <rate-limit name="form" path="/r/*/submit" methods="POST" requests="1" seconds="60" burst="1"/>
            </web-protection>"#,
    )
    .unwrap();
    let addressing = Arc::new(ClientAddressing::new("X-Forwarded-For", parse_cidrs("", "test")));
    guard.replace_conf(GuardConf::from_node(Some(&node), addressing));
    assert_eq!(guard.conf().rate_limits.len(), 2);
    assert_eq!(status(&guard, "203.0.113.9", "GET", "/"), 200);
    assert_eq!(status(&guard, "203.0.113.9", "POST", "/r/a/submit"), 200);
    assert_eq!(status(&guard, "203.0.113.9", "POST", "/r/a/submit"), 429);
}

fn status_for(guard: &Guard, host: &str, client: &str, path: &str) -> u16 {
    let client = ip(client);
    match guard.check(client, client, host, "GET", path, &HeaderMap::new()) {
        Ok(()) => 200,
        Err(refused) => refused.response.status().as_u16(),
    }
}

/// A rule with `host` applies to that host's requests only, on top of the rules without one, so one site is stricter
/// than another (plans/Edge-Node2.md §4.3); each rule counts what it refused.
#[test]
fn a_rule_for_a_host_makes_that_site_stricter() {
    let guard = guard(
        r#"<web-protection enabled="true">
                <rate-limit name="all" requests="1" seconds="60" burst="5"/>
                <rate-limit name="shop" host="shop.example.com" requests="1" seconds="60" burst="1"/>
                <path-rule name="shop-admin" host="*.example.com" action="deny" path="/admin/**"/>
                <client-rule name="shop-office" host="shop.example.com" action="deny" cidr="198.51.100.0/24"/>
                <user-agent-rule host="forum.example.com" action="deny" contains="crawler"/>
            </web-protection>"#,
        "",
    );
    // the shop's limit refuses its second request; another host has only the general limit
    assert_eq!(status_for(&guard, "shop.example.com", "203.0.113.5", "/"), 200);
    assert_eq!(status_for(&guard, "shop.example.com", "203.0.113.5", "/"), 429);
    assert_eq!(status_for(&guard, "forum.example.com", "203.0.113.6", "/"), 200);
    assert_eq!(status_for(&guard, "forum.example.com", "203.0.113.6", "/"), 200);
    // a wildcard host: any name under it, not the parent itself nor a name that only ends the same
    assert_eq!(status_for(&guard, "forum.example.com", "203.0.113.7", "/admin/x"), 404);
    assert_eq!(status_for(&guard, "example.com", "203.0.113.7", "/admin/x"), 200);
    assert_eq!(status_for(&guard, "badexample.com", "203.0.113.8", "/admin/x"), 200);
    // a client rule for one host refuses there only, and not the connection, which names no host yet
    assert_eq!(status_for(&guard, "shop.example.com", "198.51.100.4", "/"), 403);
    assert_eq!(status_for(&guard, "forum.example.com", "198.51.100.4", "/"), 200);
    assert!(guard.admit(ip("198.51.100.4")).is_some());
    let mut headers = HeaderMap::new();
    headers.insert("user-agent", "SomeCrawler/1".parse().unwrap());
    let client = ip("203.0.113.20");
    assert!(guard
        .check(client, client, "forum.example.com", "GET", "/", &headers)
        .is_err());
    assert!(guard
        .check(client, client, "shop.example.com", "GET", "/", &headers)
        .is_ok());
    // what each refused, in the report
    let report = guard.report();
    let count = |name: &str| {
        report["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|rule| rule["name"] == name)
            .unwrap_or_else(|| panic!("{name}: {report}"))["count"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(count("shop"), 1);
    assert_eq!(count("all"), 0);
    assert_eq!(count("path-rule shop-admin"), 1);
    assert_eq!(count("client-rule shop-office"), 1);
    assert_eq!(count("user-agent-rule #1"), 1);
    let shop = report["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|rule| rule["name"] == "shop")
        .unwrap()
        .clone();
    assert_eq!(shop["hosts"], "shop.example.com");
}

#[test]
fn the_request_host_is_read_without_its_port() {
    assert_eq!(super::request_host("Shop.Example.com:8443"), "shop.example.com");
    assert_eq!(super::request_host("shop.example.com."), "shop.example.com");
    assert_eq!(super::request_host("[2001:db8::1]:443"), "[2001:db8::1]");
    assert_eq!(super::request_host(""), "");
}

/// A ban started here is told ([`super::Guard::set_notifier`]); one told by another node is applied, never to a
/// trusted proxy or an exempt client; one a paired node told holds for its hosts only; a lift ends them.
#[test]
fn shared_bans_are_told_applied_scoped_and_lifted() {
    let guard = guard(
        r#"<web-protection enabled="true">
                <ban name="denied" events="denied" threshold="1" seconds="60" duration="600"/>
                <path-rule action="deny" path="/.env"/>
                <client-rule action="exempt" cidr="192.0.2.0/24"/>
            </web-protection>"#,
        "10.0.0.0/8",
    );
    let told = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let telling = std::sync::Arc::clone(&told);
    guard.set_notifier(std::sync::Arc::new(move |notice: &super::BanNotice| {
        telling.lock().unwrap().push(notice.clone());
    }));
    assert_eq!(status(&guard, "203.0.113.5", "GET", "/.env"), 404);
    assert_eq!(status(&guard, "203.0.113.5", "GET", "/"), 403);
    {
        let told = told.lock().unwrap();
        assert_eq!(told.len(), 1);
        assert_eq!(told[0].address, ip("203.0.113.5"));
        assert_eq!(told[0].seconds, 600);
        assert!(!told[0].lift);
    }
    // another member's ban: applied here, and not told on
    assert!(guard.apply_shared(ip("203.0.113.6"), 60, "member m-2: auth", None));
    assert_eq!(status(&guard, "203.0.113.6", "GET", "/"), 403);
    assert!(guard.admit(ip("203.0.113.6")).is_none());
    assert_eq!(told.lock().unwrap().len(), 1);
    // never a trusted proxy or an exempt client, and not for no time
    assert!(!guard.apply_shared(ip("10.1.2.3"), 60, "member m-2: auth", None));
    assert!(!guard.apply_shared(ip("192.0.2.9"), 60, "member m-2: auth", None));
    assert!(!guard.apply_shared(ip("203.0.113.7"), 0, "member m-2: auth", None));
    // a paired node's ban holds for its own hosts only
    assert!(guard.apply_shared(ip("203.0.113.8"), 60, "node local-1: auth", Some("local-1")));
    assert!(guard.banned_for("local-1", ip("203.0.113.8")));
    assert!(!guard.banned_for("local-2", ip("203.0.113.8")));
    assert_eq!(status(&guard, "203.0.113.8", "GET", "/"), 200);
    let report = guard.report();
    let bans = report["bans"].as_array().unwrap();
    assert_eq!(bans.len(), 3, "{report}");
    assert!(bans
        .iter()
        .any(|ban| ban["address"] == "203.0.113.8" && ban["node"] == "local-1"));
    assert!(bans
        .iter()
        .any(|ban| ban["address"] == "203.0.113.6" && ban["source"] == "member m-2: auth"));
    // the node's lift ends only its own; an operator's lift ends every ban of the address and is told
    assert!(!guard.lift(ip("203.0.113.6"), Some("local-1"), false));
    assert!(guard.lift(ip("203.0.113.8"), Some("local-1"), false));
    assert!(!guard.banned_for("local-1", ip("203.0.113.8")));
    assert!(guard.lift(ip("203.0.113.5"), None, true));
    assert_eq!(status(&guard, "203.0.113.5", "GET", "/"), 200);
    let told = told.lock().unwrap();
    assert_eq!(told.len(), 2);
    assert!(told[1].lift);
}
