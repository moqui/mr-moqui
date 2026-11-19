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
use crate::web::WebRequest;

fn request(peer: &str, via: RequestVia, headers: &[(&str, &str)]) -> WebRequest {
    let mut request = WebRequest::new("POST", "/Login/createInitialAdminAccount");
    request.peer_ip = peer.to_string();
    request.via = via;
    for (name, value) in headers {
        request.set_header(*name, *value);
    }
    request
}

/// SEC-WEB-08: only a loopback peer that reached the listener itself, through no proxy, is this machine.
#[test]
fn only_a_loopback_peer_with_no_proxy_header_is_local() {
    assert!(request_is_local(&request("127.0.0.1", RequestVia::Direct, &[]), "", false));
    assert!(request_is_local(&request("::1", RequestVia::Direct, &[]), "", false));
    assert!(request_is_local(&request("127.0.0.1", RequestVia::InProcess, &[]), "", false));
    assert!(!request_is_local(&request("10.0.0.5", RequestVia::Direct, &[]), "", false));
    assert!(!request_is_local(&request("", RequestVia::Direct, &[]), "", false));
    // the edge node's link and a proxy on this host carry other clients
    assert!(!request_is_local(&request("127.0.0.1", RequestVia::Edge, &[]), "", false));
    assert!(!request_is_local(
        &request("127.0.0.1", RequestVia::Direct, &[("X-Forwarded-For", "203.0.113.9")]),
        "",
        false
    ));
    assert!(!request_is_local(
        &request("127.0.0.1", RequestVia::Direct, &[("Forwarded", "for=203.0.113.9")]),
        "",
        false
    ));
    assert!(!request_is_local(
        &request("127.0.0.1", RequestVia::Direct, &[("x-client-ip", "203.0.113.9")]),
        "x-client-ip",
        false
    ));
}

/// SEC-CLI-101: with `initial-admin-local` `in-process` (the Desktop app's node), a loopback peer of the listener is
/// any account of the computer, so only a request the server's process made itself (the app's window) is this
/// machine, and with no request only work with no address is (not a loopback SSH peer).
#[test]
fn in_process_only_a_request_of_the_servers_own_process_is_local() {
    assert!(request_is_local(&request("127.0.0.1", RequestVia::InProcess, &[]), "", true));
    assert!(!request_is_local(&request("127.0.0.1", RequestVia::Direct, &[]), "", true));
    assert!(!request_is_local(&request("::1", RequestVia::Direct, &[]), "", true));
    assert!(!request_is_local(
        &request("127.0.0.1", RequestVia::InProcess, &[("X-Forwarded-For", "203.0.113.9")]),
        "",
        true
    ));
    assert!(no_request_is_local("", true));
    assert!(!no_request_is_local("127.0.0.1", true));
    assert!(!no_request_is_local("::1", true));
    assert!(no_request_is_local("127.0.0.1", false));
    assert!(!no_request_is_local("203.0.113.5", false));
}

/// From another address the secret given is compared with this server's, its dashes, spaces and case aside. With no
/// request the login address decides (the terminal's dialog over SSH): none, or loopback, is this machine.
#[test]
fn the_setup_secret_is_this_servers_and_no_other() {
    let ctx = crate::context::ExecutionContextFactory::test_context();
    let _entered = ctx.enter();
    let expected = secret(ctx.factory()).0.clone();
    assert_eq!(expected.len(), 24);
    assert!(initial_admin_local());
    assert!(super::super::with_client_ip("127.0.0.1", initial_admin_local));
    super::super::with_client_ip("203.0.113.5", || {
        assert!(!initial_admin_local());
        assert!(initial_admin_allowed(&expected));
        assert!(initial_admin_allowed(&expected.to_uppercase().replace('-', " ")));
        assert!(!initial_admin_allowed(""));
        assert!(!initial_admin_allowed("abcd-efgh-jkmn-pqrs-tuvw"));
    });
}
