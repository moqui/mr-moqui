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

use std::collections::BTreeMap;

use moqui_core::web::RequestVia;

use super::{is_loopback_host, Facts, Place};

fn facts(peer: &str, via: RequestVia, https: bool, headers: &[(&str, &str)]) -> Facts {
    let headers: BTreeMap<String, String> = headers
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    Facts::of(peer.parse().ok(), via, https, "", String::new(), |name| {
        headers.get(name).map(String::as_str)
    })
}

#[test]
fn a_loopback_page_of_the_servers_own_loopback_origin_is_local() {
    let local = facts(
        "127.0.0.1",
        RequestVia::Direct,
        false,
        &[("host", "localhost:8080"), ("origin", "http://localhost:8080")],
    );
    assert_eq!(local.place(), Place::Local);
    let six = facts(
        "::1",
        RequestVia::Direct,
        false,
        &[("host", "[::1]:8080"), ("origin", "http://[::1]:8080")],
    );
    assert_eq!(six.place(), Place::Local);
    let dev = facts(
        "127.0.0.1",
        RequestVia::Direct,
        false,
        &[("host", "app.localhost:8080"), ("origin", "http://app.localhost:8080")],
    );
    assert_eq!(dev.place(), Place::Local);
}

#[test]
fn a_proxy_a_tunnel_another_origin_or_the_edge_is_remote() {
    let remote = Place::Remote {
        secure: false,
        via_edge: false,
    };
    // a tunnel tool or a proxy on this machine: loopback, with another origin or a forwarding header
    let tunnel = facts(
        "127.0.0.1",
        RequestVia::Direct,
        false,
        &[("host", "dev.example.com"), ("origin", "http://dev.example.com")],
    );
    assert_eq!(tunnel.place(), remote);
    let proxied = facts(
        "127.0.0.1",
        RequestVia::Direct,
        false,
        &[
            ("host", "localhost:8080"),
            ("origin", "http://localhost:8080"),
            ("x-forwarded-for", "203.0.113.5"),
        ],
    );
    assert_eq!(proxied.place(), remote);
    // another port of this machine is another origin
    let other_port = facts(
        "127.0.0.1",
        RequestVia::Direct,
        false,
        &[("host", "localhost:8080"), ("origin", "http://localhost:3000")],
    );
    assert!(!other_port.own_origin());
    assert_eq!(other_port.place(), remote);
    // no Origin at all
    assert_eq!(
        facts("127.0.0.1", RequestVia::Direct, false, &[("host", "localhost:8080")]).place(),
        remote
    );
    // a LAN peer over HTTPS
    let lan = facts(
        "192.168.1.20",
        RequestVia::Direct,
        true,
        &[("host", "dev.lan"), ("origin", "https://dev.lan")],
    );
    assert_eq!(
        lan.place(),
        Place::Remote {
            secure: true,
            via_edge: false
        }
    );
    assert!(lan.own_origin());
    let edge = facts(
        "198.51.100.7",
        RequestVia::Edge,
        true,
        &[("host", "dev.example.com"), ("origin", "https://dev.example.com")],
    );
    assert_eq!(
        edge.place(),
        Place::Remote {
            secure: true,
            via_edge: true
        }
    );
}

#[test]
fn the_node_in_process_is_local() {
    let node = facts("127.0.0.1", RequestVia::InProcess, false, &[("host", "moqui.node")]);
    assert_eq!(node.place(), Place::Local);
}

#[test]
fn loopback_names() {
    for host in [
        "localhost",
        "LOCALHOST",
        "x.localhost",
        "127.0.0.1",
        "127.1.2.3",
        "[::1]",
        "::1",
    ] {
        assert!(is_loopback_host(host), "{host}");
    }
    for host in ["localhost.example.com", "10.0.0.1", "example.com", "0.0.0.0"] {
        assert!(!is_loopback_host(host), "{host}");
    }
}

use super::{check, offered, Refusal, Switches};
use crate::term::session::Kind;

const ON: Switches = Switches {
    command: true,
    host: true,
    remote: false,
    reauth_millis: 3_600_000,
};

fn local() -> Facts {
    facts(
        "127.0.0.1",
        RequestVia::Direct,
        false,
        &[("host", "localhost:8080"), ("origin", "http://localhost:8080")],
    )
}

#[test]
fn the_command_line_needs_its_switch_and_its_permission() {
    let all = |_: &str| true;
    let none = |_: &str| false;
    let shell_only = |permission: &str| permission == "SSH_SHELL";
    let recent = |_: i64| true;
    assert_eq!(
        check(ON, Kind::Apps, &local(), &none, &recent),
        Ok(()),
        "the apps need nothing"
    );
    assert_eq!(check(ON, Kind::Command, &local(), &all, &recent), Ok(()));
    assert_eq!(
        check(ON, Kind::Command, &local(), &none, &recent),
        Err(Refusal::NotAvailable)
    );
    let off = Switches { command: false, ..ON };
    assert_eq!(
        check(off, Kind::Command, &local(), &all, &recent),
        Err(Refusal::NotAvailable),
        "not a dev instance"
    );
    assert_eq!(
        check(ON, Kind::Host, &local(), &shell_only, &recent),
        Err(Refusal::NotAvailable),
        "SSH_SHELL alone"
    );
    assert_eq!(offered(ON, &local(), &shell_only), vec![Kind::Apps, Kind::Command]);
    assert_eq!(
        offered(off, &local(), &all),
        if cfg!(all(feature = "devtools", unix)) {
            vec![Kind::Apps, Kind::Host]
        } else {
            vec![Kind::Apps]
        }
    );
}

#[test]
fn another_origin_is_refused_whatever_allow_origins_says() {
    let all = |_: &str| true;
    let recent = |_: i64| true;
    let other = facts(
        "127.0.0.1",
        RequestVia::Direct,
        false,
        &[("host", "localhost:8080"), ("origin", "http://localhost:3000")],
    );
    assert_eq!(
        check(ON, Kind::Command, &other, &all, &recent),
        Err(Refusal::NotAvailable)
    );
    let none = facts("127.0.0.1", RequestVia::Direct, false, &[("host", "localhost:8080")]);
    assert_eq!(
        check(ON, Kind::Command, &none, &all, &recent),
        Err(Refusal::NotAvailable),
        "no Origin"
    );
    let node = facts("127.0.0.1", RequestVia::InProcess, false, &[("host", "moqui.node")]);
    assert_eq!(
        check(ON, Kind::Command, &node, &all, &recent),
        Ok(()),
        "the node in process"
    );
}

#[test]
fn from_elsewhere_it_takes_remote_https_and_a_recent_sign_in() {
    let all = |_: &str| true;
    let recent = |_: i64| true;
    let stale = |_: i64| false;
    let lan_http = facts(
        "192.168.1.20",
        RequestVia::Direct,
        false,
        &[("host", "dev.lan:8080"), ("origin", "http://dev.lan:8080")],
    );
    let lan_https = facts(
        "192.168.1.20",
        RequestVia::Direct,
        true,
        &[("host", "dev.lan"), ("origin", "https://dev.lan")],
    );
    assert_eq!(
        check(ON, Kind::Command, &lan_https, &all, &recent),
        Err(Refusal::NotAvailable),
        "remote is off"
    );
    let remote = Switches { remote: true, ..ON };
    assert_eq!(
        check(remote, Kind::Command, &lan_http, &all, &recent),
        Err(Refusal::NotAvailable),
        "plain http"
    );
    assert_eq!(
        check(remote, Kind::Command, &lan_https, &all, &stale),
        Err(Refusal::ReauthRequired)
    );
    assert_eq!(check(remote, Kind::Command, &lan_https, &all, &recent), Ok(()));
    // a stale sign-in does not hide the kind; opening it asks
    assert!(offered(remote, &lan_https, &all).contains(&Kind::Command));
}

#[test]
fn a_listing_with_no_origin_is_taken_as_its_own() {
    let get = facts("127.0.0.1", RequestVia::Direct, false, &[("host", "localhost:8080")]);
    assert_eq!(get.clone().listing().place(), Place::Local);
    let six = facts("::1", RequestVia::Direct, false, &[("host", "[::1]:8080")]);
    assert_eq!(six.listing().place(), Place::Local);
    assert_ne!(get.place(), Place::Local, "opening carries its origin");
}

/// The Desktop app as a client of a server on its own machine: its requests come from loopback with a device key and no
/// origin, so they are not local, and the listing offers what opening allows (it offered the command line, then
/// refused it).
#[test]
fn a_device_keys_listing_offers_what_it_may_open() {
    let all = |_: &str| true;
    let app = facts(
        "127.0.0.1",
        RequestVia::Direct,
        false,
        &[("host", "localhost:8091"), ("login_key", "k")],
    );
    let listed = app.clone().listing();
    assert_ne!(listed.place(), Place::Local);
    assert_eq!(offered(ON, &listed, &all), vec![Kind::Apps]);
    assert_eq!(
        check(ON, Kind::Command, &app, &all, &|_| true),
        Err(Refusal::NotAvailable)
    );
}

#[test]
fn a_device_key_with_no_origin_is_taken_and_its_place_is_its_peers() {
    let all = |_: &str| true;
    let recent = |_: i64| true;
    let app = facts(
        "198.51.100.4",
        RequestVia::Direct,
        true,
        &[("host", "dev.example.com"), ("login_key", "k")],
    );
    assert_eq!(
        app.place(),
        Place::Remote {
            secure: true,
            via_edge: false
        }
    );
    let remote = Switches { remote: true, ..ON };
    assert_eq!(check(remote, Kind::Command, &app, &all, &recent), Ok(()));
    assert_eq!(
        check(remote, Kind::Command, &app, &all, &|_| false),
        Err(Refusal::ReauthRequired)
    );
    // a key with another site's Origin is still refused
    let other = facts(
        "198.51.100.4",
        RequestVia::Direct,
        true,
        &[
            ("host", "dev.example.com"),
            ("login_key", "k"),
            ("origin", "https://evil.example"),
        ],
    );
    assert_eq!(
        check(remote, Kind::Command, &other, &all, &recent),
        Err(Refusal::NotAvailable)
    );
}
