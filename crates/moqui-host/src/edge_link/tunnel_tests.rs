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

use tokio::io::AsyncReadExt as _;

use super::*;

#[tokio::test]
async fn a_passthrough_tunnel_starts_with_the_clients_address_then_its_hello() {
    let (client_side, mut client) = tokio::io::duplex(4096);
    let (tunnel_side, mut local) = tokio::io::duplex(4096);
    let (_stop, shutdown) = tokio::sync::watch::channel(false);
    let browser: SocketAddr = "203.0.113.9:51000".parse().unwrap();
    let edge: SocketAddr = "192.0.2.1:443".parse().unwrap();
    let relayed = tokio::spawn(relay(
        shutdown,
        client_side,
        tunnel_side,
        b"\x16\x03\x01hello",
        browser,
        edge,
    ));
    // what the local node reads: the PROXY header naming the browser, then the ClientHello's bytes
    let header = crate::proxy_header::read(&mut local).await.unwrap();
    assert_eq!(header, Some(browser));
    let mut hello = [0_u8; 8];
    local.read_exact(&mut hello).await.unwrap();
    assert_eq!(&hello, b"\x16\x03\x01hello");
    // then both ways
    client.write_all(b"more").await.unwrap();
    let mut more = [0_u8; 4];
    local.read_exact(&mut more).await.unwrap();
    assert_eq!(&more, b"more");
    local.write_all(b"back").await.unwrap();
    let mut back = [0_u8; 4];
    client.read_exact(&mut back).await.unwrap();
    assert_eq!(&back, b"back");
    drop((client, local));
    relayed.await.unwrap();
}

/// E12: an all-in-one node's XMPP tunnel port may not take its own XMPP server's port.
#[test]
fn a_tunnel_port_takes_the_same_listener_only_on_the_same_port_and_an_overlapping_host() {
    let any: SocketAddr = "0.0.0.0:5222".parse().unwrap();
    let one: SocketAddr = "192.0.2.5:5222".parse().unwrap();
    assert!(same_listener(any, 5222, ""));
    assert!(same_listener(any, 5222, "192.0.2.7"));
    assert!(same_listener(one, 5222, "0.0.0.0"));
    assert!(same_listener(one, 5222, "192.0.2.5"));
    assert!(
        same_listener(one, 5222, "xmpp.example.com"),
        "a name is not resolved here"
    );
    assert!(!same_listener(one, 5222, "192.0.2.7"));
    assert!(!same_listener(any, 5223, ""));
    assert!(!same_listener("0.0.0.0:0".parse().unwrap(), 0, ""));
}

/// E16: one pair of XMPP ports for every pairing's domain.
#[test]
fn an_xmpp_connection_goes_to_the_pairing_whose_domain_it_names() {
    let pairings = vec![
        ("local-1".to_string(), "chat.one.example".to_string()),
        ("local-2".to_string(), "chat.two.example".to_string()),
        ("local-3".to_string(), String::new()),
    ];
    let active = |nodes: &[&str]| nodes.iter().map(|node| (*node).to_string()).collect::<Vec<_>>();
    let both = active(&["local-1", "local-2"]);
    let to = |node: &str| XmppRoute::Pairing(node.to_string());
    assert_eq!(
        xmpp_route(Some("chat.one.example"), None, &pairings, &both),
        to("local-1")
    );
    assert_eq!(
        xmpp_route(Some("chat.two.example"), None, &pairings, &both),
        to("local-2")
    );
    assert!(matches!(
        xmpp_route(Some("chat.three.example"), None, &pairings, &both),
        XmppRoute::Refused(_)
    ));
    assert!(
        matches!(xmpp_route(None, None, &pairings, &both), XmppRoute::Refused(_)),
        "no domain, two paired"
    );
    // one pairing: it, whatever the client names, as before a node named its domain; whether it or another is linked
    // never decides it (a pairing away is refused when its tunnel opens)
    assert_eq!(xmpp_route(None, None, &pairings, &active(&["local-3"])), to("local-3"));
    assert_eq!(
        xmpp_route(Some("x.example"), None, &pairings, &active(&["local-3"])),
        to("local-3")
    );
    // an all-in-one node's own domain, its subdomains and a stream naming none are its own
    let own = Some("chat.own.example");
    assert_eq!(
        xmpp_route(Some("chat.own.example"), own, &pairings, &both),
        XmppRoute::Own
    );
    assert_eq!(
        xmpp_route(Some("rooms.chat.own.example"), own, &pairings, &both),
        XmppRoute::Own
    );
    assert_eq!(xmpp_route(None, own, &pairings, &both), XmppRoute::Own);
    assert_eq!(
        xmpp_route(Some("chat.two.example"), own, &pairings, &both),
        to("local-2")
    );
    // a domain two pairings name is refused
    let doubled = vec![
        ("local-1".to_string(), "chat.one.example".to_string()),
        ("local-2".to_string(), "chat.one.example".to_string()),
    ];
    assert!(matches!(
        xmpp_route(Some("chat.one.example"), None, &doubled, &both),
        XmppRoute::Refused(why) if why.contains("local-1, local-2")
    ));
}
