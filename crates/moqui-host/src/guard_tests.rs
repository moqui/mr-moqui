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
    match guard.check(client, client, method, path, &HeaderMap::new()) {
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
        .check(client, client, "GET", "/", &HeaderMap::new())
        .err()
        .unwrap();
    let retry: u64 = refused.response.headers()["retry-after"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=60).contains(&retry), "{retry}");
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
    let refused = guard.check(client, client, "GET", "/apps", &headers).err().unwrap();
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
        "/../../runtime/base-component/framework/conf/MoquiDefaultConf.xml"
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
    assert_eq!(names, ["all", "login"]);
    assert_eq!(on.bans.len(), 2);
    assert!(on.exempt.iter().any(|range| range.contains(ip("127.0.0.1"))));
    assert!(on.exempt.iter().any(|range| range.contains(ip("::1"))));
    assert!(on.deny_paths.iter().any(|glob| glob.matches("/.env")));
    assert!(!on.deny_paths.iter().any(|glob| glob.matches("/apps/system")));
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
