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
}

fn expected() -> Expected {
    Expected {
        mx_host: "edge.example.net".to_string(),
        addresses: vec!["203.0.113.5".parse().unwrap()],
        dkim_selector: "moqui".to_string(),
        dkim_record: "v=DKIM1; k=rsa; p=ABC def".to_string(),
        mta: "moqui".to_string(),
    }
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
        rows.iter().any(|row| row.later && row.name == "_sips._tcp.example.com"),
        "the SIP and XMPP records are listed for later"
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
