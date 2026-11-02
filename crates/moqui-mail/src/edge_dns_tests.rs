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

use std::collections::HashMap;

use super::*;

/// Fixed answers; a name not here has no records, and `broken` fails its lookup.
#[derive(Default)]
struct Fixed {
    mx: HashMap<String, Vec<(u16, String)>>,
    txt: HashMap<String, Vec<String>>,
    ips: HashMap<String, Vec<IpAddr>>,
    srv: HashMap<String, Vec<super::super::dns::Srv>>,
    broken: Vec<String>,
}

impl Lookup for Fixed {
    fn mx(&self, domain: &str) -> Result<Vec<(u16, String)>, String> {
        if self.broken.iter().any(|name| name == domain) {
            return Err(format!("the MX records of {domain}: timed out"));
        }
        Ok(self.mx.get(domain).cloned().unwrap_or_default())
    }
    fn txt(&self, name: &str) -> Result<Vec<String>, String> {
        Ok(self.txt.get(name).cloned().unwrap_or_default())
    }
    fn ips(&self, host: &str) -> Result<Vec<IpAddr>, String> {
        Ok(self.ips.get(host).cloned().unwrap_or_default())
    }
    fn srv(&self, name: &str) -> Result<Vec<super::super::dns::Srv>, String> {
        if self.broken.iter().any(|broken| broken == name) {
            return Err(format!("the SRV records of {name}: timed out"));
        }
        Ok(self.srv.get(name).cloned().unwrap_or_default())
    }
}

fn srv(priority: u16, port: u16, target: &str) -> super::super::dns::Srv {
    super::super::dns::Srv {
        priority,
        weight: 5,
        port,
        target: target.to_string(),
    }
}

fn expected() -> Expected {
    Expected {
        mx_host: "edge.example.net".to_string(),
        addresses: vec!["203.0.113.5".parse().unwrap()],
        dkim_selector: "moqui".to_string(),
        dkim_record: "v=DKIM1; k=rsa; p=ABC def".to_string(),
        mta: "moqui".to_string(),
        xmpp: XmppExpected::default(),
        turn_hosts: Vec::new(),
        media_ip: None,
        sip: SipExpected::default(),
    }
}

#[test]
fn a_turn_host_name_points_at_the_edge_node_or_its_media_address() {
    let turn = |hosts: &[&str]| Expected {
        turn_hosts: hosts.iter().map(|host| (*host).to_string()).collect(),
        media_ip: Some("198.51.100.20".parse().unwrap()),
        ..expected()
    };
    let mut fixed = Fixed::default();
    fixed
        .ips
        .insert("turn.example.net".into(), vec!["198.51.100.20".parse().unwrap()]);
    fixed
        .ips
        .insert("relay.example.net".into(), vec!["203.0.113.5".parse().unwrap()]);
    fixed
        .ips
        .insert("stale.example.net".into(), vec!["192.0.2.9".parse().unwrap()]);
    let rows = check(
        &[],
        &["erp.example.com".to_string()],
        &turn(&[
            "Turn.Example.net.",
            "relay.example.net",
            "stale.example.net",
            "203.0.113.5",
            "erp.example.com",
            "turn.example.net",
        ]),
        &fixed,
    );
    assert!(
        ok_of(&rows, "A/AAAA", "turn.example.net"),
        "the media address: {rows:?}"
    );
    assert!(ok_of(&rows, "A/AAAA", "relay.example.net"), "an edge address: {rows:?}");
    assert!(!ok_of(&rows, "A/AAAA", "stale.example.net"), "{rows:?}");
    let turn_rows: Vec<&DnsRow> = rows
        .iter()
        .filter(|row| row.note.starts_with("TURN for calls"))
        .collect();
    assert_eq!(
        turn_rows.len(),
        3,
        "one row a name; an address and a web host name get none of their own: {rows:?}"
    );
    assert!(!rows.iter().any(|row| row.name == "203.0.113.5"), "{rows:?}");
    assert_eq!(
        rows.iter().filter(|row| row.name == "erp.example.com").count(),
        1,
        "{rows:?}"
    );
}

fn ok_of(rows: &[DnsRow], kind: &str, name: &str) -> bool {
    rows.iter()
        .find(|row| row.kind == kind && row.name == name)
        .unwrap_or_else(|| panic!("{kind} {name}: {rows:?}"))
        .ok
}

#[test]
fn records_that_name_the_edge_node_pass_and_others_say_why_not() {
    let mut fixed = Fixed::default();
    fixed
        .mx
        .insert("example.com".into(), vec![(10, "Edge.Example.net.".into())]);
    fixed
        .txt
        .insert("example.com".into(), vec!["v=spf1 mx -all".into(), "other".into()]);
    fixed.txt.insert(
        "moqui._domainkey.example.com".into(),
        vec!["v=DKIM1; k=rsa; p=ABCdef".into()],
    );
    fixed
        .txt
        .insert("_dmarc.example.com".into(), vec!["v=DMARC1; p=reject".into()]);
    fixed
        .mx
        .insert("other.org".into(), vec![(5, "mx.provider.test".into())]);
    fixed
        .txt
        .insert("other.org".into(), vec!["v=spf1 include:provider.test -all".into()]);
    fixed
        .txt
        .insert("moqui._domainkey.other.org".into(), vec!["v=DKIM1; p=XYZ".into()]);
    fixed
        .ips
        .insert("erp.example.com".into(), vec!["203.0.113.5".parse().unwrap()]);
    fixed
        .ips
        .insert("old.example.com".into(), vec!["198.51.100.1".parse().unwrap()]);
    fixed.broken.push("broken.example".into());
    let rows = check(
        &["example.com".into(), "other.org".into(), "broken.example".into()],
        &[
            "erp.example.com".into(),
            "old.example.com".into(),
            "none.example.com".into(),
        ],
        &expected(),
        &fixed,
    );
    assert!(ok_of(&rows, "MX", "example.com"));
    assert!(ok_of(&rows, "SPF", "example.com"));
    assert!(
        ok_of(&rows, "DKIM", "moqui._domainkey.example.com"),
        "the key matches with its white space dropped"
    );
    assert!(ok_of(&rows, "DMARC", "_dmarc.example.com"));
    assert!(!ok_of(&rows, "MX", "other.org"));
    assert!(!ok_of(&rows, "SPF", "other.org"));
    assert!(!ok_of(&rows, "DKIM", "moqui._domainkey.other.org"));
    assert!(!ok_of(&rows, "DMARC", "_dmarc.other.org"));
    assert!(ok_of(&rows, "A/AAAA", "erp.example.com"));
    assert!(!ok_of(&rows, "A/AAAA", "old.example.com"));
    assert!(!ok_of(&rows, "A/AAAA", "none.example.com"));
    let broken = rows
        .iter()
        .find(|row| row.kind == "MX" && row.name == "broken.example")
        .unwrap();
    assert!(
        !broken.ok && broken.note.contains("timed out"),
        "a failed lookup is a row that says so: {broken:?}"
    );
    assert!(
        !rows.iter().any(|row| row.name.starts_with("_sip")),
        "no SIP records without a SIP domain, and none per mail domain: {rows:?}"
    );
    assert!(
        !rows.iter().any(|row| row.name.starts_with("_xmpp")),
        "no XMPP records without an XMPP domain: {rows:?}"
    );
}

#[test]
fn the_xmpp_domains_client_srv_records_name_the_edge_node_and_its_tunnel_ports() {
    let xmpp = |domain: &str, port: u16, tls_port: u16| Expected {
        xmpp: XmppExpected {
            domain: domain.to_string(),
            hosts: vec!["edge.example.net".to_string(), "Edge2.example.net.".to_string()],
            port,
            tls_port,
        },
        ..expected()
    };
    let mut fixed = Fixed::default();
    fixed.srv.insert(
        "_xmpp-client._tcp.chat.example.com".into(),
        vec![srv(0, 5222, "Edge.Example.net.")],
    );
    fixed.srv.insert(
        "_xmpps-client._tcp.chat.example.com".into(),
        vec![srv(0, 5223, "edge2.example.net"), srv(10, 5223, "office.example.com")],
    );
    fixed.srv.insert(
        "_xmpp-client._tcp.other.example.com".into(),
        vec![srv(0, 5333, "edge.example.net")],
    );
    fixed.broken.push("_xmpps-client._tcp.other.example.com".into());
    let rows = check(&[], &[], &xmpp("Chat.example.com", 5222, 5223), &fixed);
    let starttls = rows
        .iter()
        .find(|row| row.name == "_xmpp-client._tcp.chat.example.com")
        .unwrap();
    assert!(
        starttls.ok && starttls.expected == "0 5 5222 edge.example.net",
        "{starttls:?}"
    );
    let direct = rows
        .iter()
        .find(|row| row.name == "_xmpps-client._tcp.chat.example.com")
        .unwrap();
    assert!(
        !direct.ok && direct.note.contains("may go past it"),
        "a record past the edge node fails: {direct:?}"
    );
    let server = rows
        .iter()
        .find(|row| row.name == "_xmpp-server._tcp.chat.example.com")
        .unwrap();
    assert!(
        server.later && server.note.contains("federation"),
        "the server record is C3b's, for later: {server:?}"
    );
    let rows = check(&[], &[], &xmpp("other.example.com", 5222, 5223), &fixed);
    let wrong_port = rows
        .iter()
        .find(|row| row.name == "_xmpp-client._tcp.other.example.com")
        .unwrap();
    assert!(
        !wrong_port.ok && wrong_port.note.contains("port 5222"),
        "{wrong_port:?}"
    );
    let broken = rows
        .iter()
        .find(|row| row.name == "_xmpps-client._tcp.other.example.com")
        .unwrap();
    assert!(!broken.ok && broken.note.contains("timed out"), "{broken:?}");
    // a tunnel port that is off has no record to set; a missing record says what it is for
    let rows = check(&[], &[], &xmpp("none.example.com", 5222, 0), &fixed);
    assert!(
        !rows.iter().any(|row| row.name.starts_with("_xmpps-client")),
        "{rows:?}"
    );
    let missing = rows
        .iter()
        .find(|row| row.name == "_xmpp-client._tcp.none.example.com")
        .unwrap();
    assert!(!missing.ok && missing.found.is_empty(), "{missing:?}");
}

#[test]
fn the_sip_domains_srv_records_name_the_edge_node_and_its_sip_ports() {
    let sip = |domain: &str, port: u16, tls_port: u16| Expected {
        sip: SipExpected {
            domain: domain.to_string(),
            hosts: vec!["edge.example.net".to_string()],
            port,
            tls_port,
        },
        ..expected()
    };
    let mut fixed = Fixed::default();
    fixed.srv.insert(
        "_sips._tcp.sip.example.com".into(),
        vec![srv(0, 5061, "Edge.example.net.")],
    );
    fixed.srv.insert(
        "_sip._udp.sip.example.com".into(),
        vec![srv(0, 5070, "edge.example.net")],
    );
    fixed.broken.push("_sip._tcp.sip.example.com".into());
    let rows = check(&[], &[], &sip("SIP.example.com", 5060, 5061), &fixed);
    let tls = rows
        .iter()
        .find(|row| row.name == "_sips._tcp.sip.example.com")
        .unwrap();
    assert!(tls.ok && tls.expected == "0 5 5061 edge.example.net", "{tls:?}");
    let udp = rows.iter().find(|row| row.name == "_sip._udp.sip.example.com").unwrap();
    assert!(!udp.ok && udp.note.contains("port 5060 (UDP)"), "another port: {udp:?}");
    let tcp = rows.iter().find(|row| row.name == "_sip._tcp.sip.example.com").unwrap();
    assert!(!tcp.ok && tcp.note.contains("timed out"), "{tcp:?}");
    // a listener that is off has no records; a missing record says what it is for
    let rows = check(&[], &[], &sip("sip.example.org", 0, 5061), &fixed);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(
        rows[0].name == "_sips._tcp.sip.example.org" && !rows[0].ok && rows[0].note.contains("Phones over TLS"),
        "{rows:?}"
    );
    // a realm that is no DNS name (an address, or moqui) has no SRV records to check
    for realm in ["203.0.113.5", "moqui"] {
        let rows = check(&[], &[], &sip(realm, 5060, 5061), &fixed);
        assert!(
            rows.len() == 1 && rows[0].later && rows[0].note.contains("sip_domain"),
            "{realm}: {rows:?}"
        );
    }
}

#[test]
fn a_fixed_srv_record_is_what_the_resolver_answers() {
    let fixed = vec![srv(0, 5222, "edge.example.net")];
    super::super::dns::records().fix_srv("_xmpp-client._tcp.fixed-edge-dns.test", &fixed);
    assert_eq!(
        super::super::dns::srv("_XMPP-client._tcp.fixed-edge-dns.test.").unwrap(),
        fixed
    );
}

#[test]
fn spf_allows_the_edge_node_by_its_mx_host_or_address_and_mx_only_once_the_mx_is_it() {
    let expected = expected();
    assert_eq!(spf_allows("v=spf1 mx ~all", &expected, true), SpfMatch::Yes);
    assert_eq!(
        spf_allows("v=spf1 mx ~all", &expected, false),
        SpfMatch::No,
        "mx names the edge node only by the MX"
    );
    assert_eq!(
        spf_allows("v=spf1 a:edge.example.net -all", &expected, false),
        SpfMatch::Yes
    );
    assert_eq!(
        spf_allows("v=spf1 include:EDGE.example.net -all", &expected, false),
        SpfMatch::Include
    );
    assert_eq!(
        spf_allows("v=spf1 ip4:203.0.113.5/32 -all", &expected, false),
        SpfMatch::Yes
    );
    assert_eq!(
        spf_allows("v=spf1 ip4:203.0.113.0/24 -all", &expected, false),
        SpfMatch::Yes,
        "a network"
    );
    assert_eq!(
        spf_allows("v=spf1 ip4:203.0.112.0/24 -all", &expected, false),
        SpfMatch::No
    );
    assert_eq!(
        spf_allows("v=spf1 ip4:203.0.113.6 -all", &expected, false),
        SpfMatch::No
    );
    assert_eq!(spf_allows("v=spf1 -all", &expected, true), SpfMatch::No);
    assert!(in_network(&"2001:db8::5".parse().unwrap(), "2001:db8::/32"));
    assert!(!in_network(&"2001:db9::5".parse().unwrap(), "2001:db8::/32"));
    assert!(!in_network(&"203.0.113.5".parse().unwrap(), "2001:db8::/32"));
    assert!(in_network(&"203.0.113.5".parse().unwrap(), "0.0.0.0/0"));
}

#[test]
fn postfix_keys_are_checked_for_presence_and_aaaa_only_when_the_edge_node_lists_ipv6() {
    let mut fixed = Fixed::default();
    fixed
        .txt
        .insert("moqui._domainkey.example.com".into(), vec!["v=DKIM1; p=RSPAMD".into()]);
    fixed.ips.insert(
        "erp.example.com".into(),
        vec!["203.0.113.5".parse().unwrap(), "2001:db8::9".parse().unwrap()],
    );
    let postfix = Expected {
        mta: "postfix".to_string(),
        dkim_record: String::new(),
        ..expected()
    };
    let rows = check(&["example.com".into()], &["erp.example.com".into()], &postfix, &fixed);
    let dkim = rows.iter().find(|row| row.kind == "DKIM").unwrap();
    assert!(dkim.ok && dkim.note.contains("Postfix"), "{dkim:?}");
    let ips = rows.iter().find(|row| row.kind == "A/AAAA").unwrap();
    assert!(ips.ok && ips.note.contains("not checked"), "{ips:?}");
    let no_key = Expected {
        dkim_record: String::new(),
        ..expected()
    };
    let rows = check(&["example.com".into()], &[], &no_key, &fixed);
    let dkim = rows.iter().find(|row| row.kind == "DKIM").unwrap();
    assert!(!dkim.ok && dkim.note.contains("no DKIM key yet"), "{dkim:?}");
    let v6 = Expected {
        addresses: vec!["203.0.113.5".parse().unwrap(), "2001:db8::5".parse().unwrap()],
        ..expected()
    };
    let rows = check(&[], &["erp.example.com".into()], &v6, &fixed);
    assert!(
        !rows[0].ok,
        "an AAAA not the edge node's fails once it lists IPv6: {rows:?}"
    );
}

#[test]
fn a_fixed_txt_record_is_what_the_resolver_answers() {
    super::super::dns::records()
        .fix_txt("_dmarc.fixed-edge-dns.test", "v=DMARC1; p=none")
        .unwrap();
    assert_eq!(
        super::super::dns::txt("_dmarc.fixed-edge-dns.test").unwrap(),
        vec!["v=DMARC1; p=none".to_string()]
    );
}
