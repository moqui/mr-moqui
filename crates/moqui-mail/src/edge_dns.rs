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

//! `org.moqui.impl.EdgeServices.check#EdgeDns` (plans/Edge-Node.md §5.7, the Apply step, E10): the DNS records a
//! local node behind an edge node needs, each looked up now with this crate's resolver ([`super::dns`]) and compared
//! with what it should be: MX, SPF, the DKIM key and DMARC for each mail domain, A and AAAA for each host name, and
//! the XMPP domain's client SRV records (`_xmpp-client._tcp`, `_xmpps-client._tcp`), which name the edge node and its
//! tunnel ports, A and AAAA for each name in the edge node's TURN URLs (E10 calls), and the SIP domain's SRV records
//! (`_sips._tcp`, `_sip._udp`, `_sip._tcp`), which name the edge node and its SIP ports (E10 phones).
//! The expected values come from the edge node's offer, which the caller passes, so the check itself needs no link.
//! One record that cannot be looked up is a row that says so; the others are still checked.

use std::collections::BTreeMap;
use std::net::IpAddr;

use moqui_core::entity::EntityFacade;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

/// One record to check.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DnsRow {
    /// MX, SPF, DKIM, DMARC, A/AAAA or SRV.
    pub kind: String,
    pub name: String,
    pub expected: String,
    pub found: String,
    pub ok: bool,
    pub note: String,
    /// A record listed and not checked: of an area not built yet, or one with nothing to check.
    pub later: bool,
}

/// What the records should name: the edge node's MX host, addresses and DKIM key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Expected {
    pub mx_host: String,
    pub addresses: Vec<IpAddr>,
    pub dkim_selector: String,
    pub dkim_record: String,
    /// The edge node's MTA: `moqui` (it signs with its own key, `dkim_record`) or `postfix` (Rspamd signs, with a key
    /// the edge node does not know).
    pub mta: String,
    pub xmpp: XmppExpected,
    /// The host names of the edge node's TURN URLs (`turn_public_host` there), each checked for A and AAAA as the
    /// host names are, against [`Expected::addresses`] and the address of its media node ([`Expected::media_ip`]).
    pub turn_hosts: Vec<String>,
    /// The address of the edge node's media node, which its TURN host defaults to.
    pub media_ip: Option<IpAddr>,
    pub sip: SipExpected,
}

/// The SIP records: the edge node's SIP domain (its `sip_domain`, the realm phones register to), whose SRV records
/// (RFC 3263) name the edge node and its SIP ports (plans/Edge-Node.md §5.4 "Voice (SIP)", E8). None when `domain`
/// is empty; a port of 0 is off, and its records are not listed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SipExpected {
    pub domain: String,
    /// The edge node's names an SRV target may be; the first is the one to set.
    pub hosts: Vec<String>,
    /// The port of its SIP address for UDP and TCP, for `_sip._udp` and `_sip._tcp`.
    pub port: u16,
    /// The port of its SIP address for TLS, for `_sips._tcp`.
    pub tls_port: u16,
}

/// The XMPP records: the local node's XMPP domain, whose client SRV records name the edge node and its tunnel ports
/// (plans/Edge-Node.md §5.4, E9). None when `domain` is empty; a port of 0 is off, and its record is not listed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct XmppExpected {
    pub domain: String,
    /// The edge node's names an SRV target may be; the first is the one to set.
    pub hosts: Vec<String>,
    /// `edge_xmpp_port` (STARTTLS), for `_xmpp-client._tcp`.
    pub port: u16,
    /// `edge_xmpp_tls_port` (direct TLS), for `_xmpps-client._tcp`.
    pub tls_port: u16,
}

fn bare(name: &str) -> String {
    name.trim().trim_end_matches('.').to_ascii_lowercase()
}

fn row(kind: &str, name: &str, expected: impl Into<String>) -> DnsRow {
    DnsRow {
        kind: kind.to_string(),
        name: name.to_string(),
        expected: expected.into(),
        ..DnsRow::default()
    }
}

/// The `p=` of a DKIM key record, with no white space.
fn dkim_key(record: &str) -> Option<String> {
    record.split(';').find_map(|tag| {
        let (name, value) = tag.split_once('=')?;
        (name.trim() == "p").then(|| value.chars().filter(|ch| !ch.is_whitespace()).collect())
    })
}

/// How an SPF record lets the edge node send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SpfMatch {
    /// It does not.
    No,
    /// By `mx`, `a:` its host, or a network with its address.
    Yes,
    /// By `include:` its host, which then needs an SPF record of its own.
    Include,
}

/// Whether `ip` is in the network `text` (an address, or an address and a prefix length).
fn in_network(ip: &IpAddr, text: &str) -> bool {
    let (address, prefix) = text
        .split_once('/')
        .map_or((text, None), |(address, prefix)| (address, Some(prefix)));
    let Ok(network) = address.parse::<IpAddr>() else {
        return false;
    };
    let bits = |ip: &IpAddr| -> (u128, u32) {
        match ip {
            IpAddr::V4(ip) => (u128::from(u32::from(*ip)), 32),
            IpAddr::V6(ip) => (u128::from(*ip), 128),
        }
    };
    let ((ours, width), (theirs, network_width)) = (bits(ip), bits(&network));
    if width != network_width {
        return false;
    }
    let Ok(prefix) = prefix.map_or(Ok(width), str::parse::<u32>) else {
        return false;
    };
    if prefix > width {
        return false;
    }
    let shift = width - prefix;
    shift >= 128 || (ours >> shift) == (theirs >> shift)
}

/// How an SPF record lets the edge node send: `mx` counts only when the domain's MX is the edge node (`mx_ok`).
fn spf_allows(record: &str, expected: &Expected, mx_ok: bool) -> SpfMatch {
    let mx_host = bare(&expected.mx_host);
    let mut found = SpfMatch::No;
    for term in record.split_whitespace().skip(1) {
        let term = term.trim_start_matches(['+', '?']).to_ascii_lowercase();
        if (term == "mx" || term.starts_with("mx/")) && mx_ok {
            return SpfMatch::Yes;
        }
        if let Some(host) = term.strip_prefix("a:") {
            if !mx_host.is_empty() && bare(host.split('/').next().unwrap_or_default()) == mx_host {
                return SpfMatch::Yes;
            }
        }
        if let Some(host) = term.strip_prefix("include:") {
            if !mx_host.is_empty() && bare(host) == mx_host {
                found = SpfMatch::Include;
            }
        }
        if let Some(network) = term.strip_prefix("ip4:").or_else(|| term.strip_prefix("ip6:")) {
            if expected.addresses.iter().any(|ip| in_network(ip, network)) {
                return SpfMatch::Yes;
            }
        }
    }
    found
}

/// The records of `domains` and `host_names`, each looked up by `lookup` ([`super::dns`] in the service).
pub fn check(domains: &[String], host_names: &[String], expected: &Expected, lookup: &dyn Lookup) -> Vec<DnsRow> {
    let mut rows = Vec::new();
    let mx_host = bare(&expected.mx_host);
    for domain in domains
        .iter()
        .map(|domain| bare(domain))
        .filter(|domain| !domain.is_empty())
    {
        let mut mx = row("MX", &domain, format!("10 {mx_host}"));
        match lookup.mx(&domain) {
            Ok(hosts) => {
                mx.found = hosts
                    .iter()
                    .map(|(preference, host)| format!("{preference} {}", bare(host)))
                    .collect::<Vec<_>>()
                    .join(", ");
                mx.ok = !mx_host.is_empty() && hosts.iter().any(|(_, host)| bare(host) == mx_host);
                if !mx.ok && !hosts.is_empty() {
                    mx.note = "Mail goes elsewhere: point the MX at the edge node".to_string();
                }
            }
            Err(err) => mx.note = err,
        }
        let mx_ok = mx.ok;
        rows.push(mx);
        let texts = lookup.txt(&domain);
        let mut spf = row("SPF", &domain, "v=spf1 mx ~all");
        match &texts {
            Ok(texts) => {
                let records: Vec<&String> = texts
                    .iter()
                    .filter(|text| text.trim_start().to_ascii_lowercase().starts_with("v=spf1"))
                    .collect();
                spf.found = records.iter().map(|text| text.as_str()).collect::<Vec<_>>().join(" | ");
                let allows = match records.as_slice() {
                    [record] => spf_allows(record, expected, mx_ok),
                    _ => SpfMatch::No,
                };
                spf.ok = allows != SpfMatch::No;
                if records.len() > 1 {
                    spf.note = "More than one SPF record: merge them into one".to_string();
                } else if allows == SpfMatch::Include {
                    spf.note = format!("include: works only when {mx_host} publishes an SPF record of its own");
                } else if records.len() == 1 && !spf.ok {
                    spf.note = if mx_ok {
                        "Add mx, or a: with the edge node's host, to the record".to_string()
                    } else {
                        "Add a: with the edge node's host, or its address, to the record (mx counts once the MX is \
                         the edge node)"
                            .to_string()
                    };
                }
            }
            Err(err) => spf.note = err.clone(),
        }
        rows.push(spf);
        let selector = match expected.dkim_selector.trim() {
            "" => "moqui".to_string(),
            selector => selector.to_ascii_lowercase(),
        };
        let dkim_name = format!("{selector}._domainkey.{domain}");
        let postfix = expected.mta.trim() == "postfix";
        let mut dkim = row(
            "DKIM",
            &dkim_name,
            if postfix {
                "The key Postfix and Rspamd sign with"
            } else {
                expected.dkim_record.trim()
            },
        );
        match lookup.txt(&dkim_name) {
            Ok(texts) => {
                dkim.found = texts.join(" | ");
                match dkim_key(&expected.dkim_record) {
                    _ if postfix => {
                        // Rspamd's key is not the edge node's to tell: only that a record is there is checked
                        dkim.ok = !texts.is_empty();
                        dkim.note = "The edge node signs with Postfix and Rspamd: publish their key; only that a \
                                     record is there is checked"
                            .to_string();
                    }
                    Some(key) => dkim.ok = texts.iter().any(|text| dkim_key(text).as_deref() == Some(key.as_str())),
                    None => {
                        dkim.ok = false;
                        dkim.note = "The edge node has no DKIM key yet: test again after it makes one, when it first \
                                     signs or is asked for its offer"
                            .to_string();
                    }
                }
                if !dkim.ok && !texts.is_empty() && dkim.note.is_empty() {
                    dkim.note = "Another key: publish the edge node's".to_string();
                }
            }
            Err(err) => dkim.note = err,
        }
        rows.push(dkim);
        let dmarc_name = format!("_dmarc.{domain}");
        let mut dmarc = row("DMARC", &dmarc_name, "v=DMARC1; p=quarantine");
        match lookup.txt(&dmarc_name) {
            Ok(texts) => {
                let records: Vec<&String> = texts
                    .iter()
                    .filter(|text| text.trim_start().to_ascii_lowercase().starts_with("v=dmarc1"))
                    .collect();
                dmarc.found = records.iter().map(|text| text.as_str()).collect::<Vec<_>>().join(" | ");
                dmarc.ok = records.len() == 1;
            }
            Err(err) => dmarc.note = err,
        }
        rows.push(dmarc);
    }
    let hosts: Vec<String> = host_names
        .iter()
        .map(|host| bare(host))
        .filter(|host| !host.is_empty())
        .collect();
    for host in &hosts {
        rows.push(address_row(host, &expected.addresses, lookup));
    }
    // the TURN host of the edge node's URLs, when it is a name: the edge node's addresses, or its media node's
    let mut turn_addresses = expected.addresses.clone();
    if let Some(ip) = expected.media_ip.filter(|ip| !turn_addresses.contains(ip)) {
        turn_addresses.push(ip);
    }
    let mut turn_hosts: Vec<String> = Vec::new();
    for host in expected.turn_hosts.iter().map(|host| bare(host)) {
        // an address needs no record, and a web host name of the same name has its row already
        if !host.is_empty() && host.parse::<IpAddr>().is_err() && !turn_hosts.contains(&host) && !hosts.contains(&host)
        {
            turn_hosts.push(host);
        }
    }
    for host in turn_hosts {
        let mut ips = address_row(&host, &turn_addresses, lookup);
        ips.note = if ips.note.is_empty() {
            "TURN for calls".to_string()
        } else {
            format!("TURN for calls; {}", ips.note)
        };
        rows.push(ips);
    }
    let later = |name: String, note: &str| DnsRow {
        kind: "SRV".to_string(),
        name,
        ok: true,
        note: note.to_string(),
        later: true,
        ..DnsRow::default()
    };
    let xmpp_domain = bare(&expected.xmpp.domain);
    if !xmpp_domain.is_empty() {
        rows.extend(xmpp_rows(&xmpp_domain, &expected.xmpp, lookup));
        // federation (C3b) will have the edge node's 5269 listener: listed for later
        rows.push(later(
            format!("_xmpp-server._tcp.{xmpp_domain}"),
            "For federation with other XMPP servers (C3b), not available yet",
        ));
    }
    let sip_domain = bare(&expected.sip.domain);
    if !sip_domain.is_empty() {
        rows.extend(sip_rows(&sip_domain, &expected.sip, lookup));
    }
    rows
}

/// The SRV records of the SIP domain: `_sips._tcp` with the TLS port, `_sip._udp` and `_sip._tcp` with the other, each
/// port that is on. A domain that is no DNS name (an address, or `moqui`, the realm when `sip_domain` is not set) has
/// none to check; a row says so.
fn sip_rows(domain: &str, sip: &SipExpected, lookup: &dyn Lookup) -> Vec<DnsRow> {
    if domain.parse::<IpAddr>().is_ok() || !domain.contains('.') {
        return vec![DnsRow {
            kind: "SRV".to_string(),
            name: domain.to_string(),
            ok: true,
            note: "The edge node's SIP domain is no DNS name, so phones and trunks reach it by its address: set \
                   sip_domain there for SRV records"
                .to_string(),
            later: true,
            ..DnsRow::default()
        }];
    }
    let hosts: Vec<String> = sip
        .hosts
        .iter()
        .map(|host| bare(host))
        .filter(|host| !host.is_empty())
        .collect();
    [
        ("_sips._tcp", sip.tls_port, "Phones over TLS", "TLS"),
        ("_sip._udp", sip.port, "Phones over UDP", "UDP"),
        ("_sip._tcp", sip.port, "Phones over TCP", "TCP"),
    ]
    .into_iter()
    .filter(|(_, port, _, _)| *port != 0)
    .map(|(service, port, who, kind)| srv_row(&format!("{service}.{domain}"), port, &hosts, (who, kind), lookup))
    .collect()
}

/// The A and AAAA row of `host`: each family `addresses` lists must have records, all of them those addresses; a
/// family it does not list (IPv6, often) is not checked.
fn address_row(host: &str, addresses: &[IpAddr], lookup: &dyn Lookup) -> DnsRow {
    let listed: Vec<String> = addresses.iter().map(IpAddr::to_string).collect();
    let mut ips = row("A/AAAA", host, listed.join(", "));
    match lookup.ips(host) {
        Ok(found) => {
            ips.found = found.iter().map(IpAddr::to_string).collect::<Vec<_>>().join(", ");
            let mut checked = false;
            let mut ok = true;
            for v6 in [false, true] {
                let family = |ip: &&IpAddr| ip.is_ipv6() == v6;
                let wanted: Vec<&IpAddr> = addresses.iter().filter(family).collect();
                let have: Vec<&IpAddr> = found.iter().filter(family).collect();
                if wanted.is_empty() {
                    if v6 && !have.is_empty() {
                        ips.note = "AAAA records not checked: the edge node lists no IPv6 address".to_string();
                    }
                    continue;
                }
                checked = true;
                ok &= !have.is_empty() && have.iter().all(|ip| wanted.contains(ip));
            }
            ips.ok = checked && ok;
            if addresses.is_empty() {
                ips.note = "The edge node's addresses are not known: set edge_public_addresses there".to_string();
            } else if !ips.ok && !found.is_empty() {
                ips.note = "An address that is not the edge node's, or a family it lists with none".to_string();
            }
        }
        Err(err) => ips.note = err,
    }
    ips
}

/// The client SRV records of `domain`: each tunnel port that is on, every record naming one of the edge node's
/// `hosts` and that port (a record pointing elsewhere would send some clients past the edge node).
fn xmpp_rows(domain: &str, xmpp: &XmppExpected, lookup: &dyn Lookup) -> Vec<DnsRow> {
    let hosts: Vec<String> = xmpp
        .hosts
        .iter()
        .map(|host| bare(host))
        .filter(|host| !host.is_empty())
        .collect();
    [
        ("_xmpp-client._tcp", xmpp.port, "XMPP clients (STARTTLS)", "STARTTLS"),
        (
            "_xmpps-client._tcp",
            xmpp.tls_port,
            "XMPP clients (direct TLS)",
            "direct TLS",
        ),
    ]
    .into_iter()
    .filter(|(_, port, _, _)| *port != 0)
    .map(|(service, port, who, kind)| srv_row(&format!("{service}.{domain}"), port, &hosts, (who, kind), lookup))
    .collect()
}

/// The SRV row of `name`: every record must name one of the edge node's `hosts` and `port` (a record pointing
/// elsewhere would send some clients past the edge node). `who` says whose record it is (`XMPP clients (STARTTLS)`),
/// and `kind` the port's kind (`STARTTLS`).
fn srv_row(name: &str, port: u16, hosts: &[String], (who, kind): (&str, &str), lookup: &dyn Lookup) -> DnsRow {
    let target = hosts.first().map_or("(the edge node's host name)", String::as_str);
    let mut srv = row("SRV", name, format!("0 5 {port} {target}"));
    match lookup.srv(name) {
        Ok(records) => {
            srv.found = records
                .iter()
                .map(|record| {
                    format!(
                        "{} {} {} {}",
                        record.priority,
                        record.weight,
                        record.port,
                        bare(&record.target)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let names_edge = |record: &super::dns::Srv| record.port == port && hosts.contains(&bare(&record.target));
            srv.ok = !records.is_empty() && records.iter().all(names_edge);
            if !srv.ok && !records.is_empty() {
                srv.note = if records.iter().any(names_edge) {
                    format!("A record that is not the edge node's: {who} may go past it")
                } else {
                    format!("Point it at the edge node, port {port} ({kind})")
                };
            } else if records.is_empty() {
                srv.note = format!("{who} find the edge node by this record");
            }
        }
        Err(err) => srv.note = err,
    }
    srv
}

/// The lookups a check makes: [`super::dns`]'s, or a test's.
pub trait Lookup {
    fn mx(&self, domain: &str) -> Result<Vec<(u16, String)>, String>;
    fn txt(&self, name: &str) -> Result<Vec<String>, String>;
    fn ips(&self, host: &str) -> Result<Vec<IpAddr>, String>;
    fn srv(&self, name: &str) -> Result<Vec<super::dns::Srv>, String>;
}

/// The resolver of [`super::dns`], with its fixed records.
struct Resolver;

impl Lookup for Resolver {
    fn mx(&self, domain: &str) -> Result<Vec<(u16, String)>, String> {
        super::dns::mx(domain)
    }
    fn txt(&self, name: &str) -> Result<Vec<String>, String> {
        super::dns::txt(name)
    }
    fn ips(&self, host: &str) -> Result<Vec<IpAddr>, String> {
        super::dns::ips(host)
    }
    fn srv(&self, name: &str) -> Result<Vec<super::dns::Srv>, String> {
        super::dns::srv(name)
    }
}

fn texts(parameters: &BTreeMap<String, Value>, name: &str) -> Vec<String> {
    match parameters.get(name) {
        Some(Value::List(items)) => items.iter().map(to_plain_string).collect(),
        Some(value) if !is_empty(value) => to_plain_string(value).split(',').map(str::to_string).collect(),
        _ => Vec::new(),
    }
    .into_iter()
    .map(|text| text.trim().to_string())
    .filter(|text| !text.is_empty())
    .collect()
}

fn text(parameters: &BTreeMap<String, Value>, name: &str) -> String {
    parameters
        .get(name)
        .map(to_plain_string)
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// `check#EdgeDns`.
pub(crate) fn check_edge_dns(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let domains = texts(parameters, "domains");
    let host_names = texts(parameters, "hostNames");
    let turn_hosts = texts(parameters, "turnHosts");
    // the media address is ip:port, or an address alone
    let media_ip = text(parameters, "mediaAddress");
    let media_ip: Option<IpAddr> = media_ip
        .parse::<std::net::SocketAddr>()
        .map(|address| address.ip())
        .or_else(|_| media_ip.parse())
        .ok();
    let mx_host = text(parameters, "mxHost");
    let mut addresses: Vec<IpAddr> = texts(parameters, "edgeAddresses")
        .iter()
        .filter_map(|ip| ip.parse().ok())
        .collect();
    // the edge node names no addresses: those of its MX host, its own name
    let mut address_note = String::new();
    if addresses.is_empty() && !mx_host.is_empty() && !(host_names.is_empty() && turn_hosts.is_empty()) {
        match super::dns::ips(&mx_host) {
            Ok(found) => addresses = found,
            Err(err) => address_note = err,
        }
    }
    let expected = Expected {
        mx_host,
        addresses,
        dkim_selector: text(parameters, "dkimSelector"),
        dkim_record: text(parameters, "dkimRecord"),
        mta: text(parameters, "mailMta"),
        xmpp: XmppExpected {
            domain: text(parameters, "xmppDomain"),
            hosts: texts(parameters, "xmppHosts"),
            port: text(parameters, "xmppPort").parse().unwrap_or(0),
            tls_port: text(parameters, "xmppTlsPort").parse().unwrap_or(0),
        },
        turn_hosts,
        media_ip,
        sip: SipExpected {
            domain: text(parameters, "sipDomain"),
            hosts: texts(parameters, "sipHosts"),
            port: text(parameters, "sipPort").parse().unwrap_or(0),
            tls_port: text(parameters, "sipTlsPort").parse().unwrap_or(0),
        },
    };
    let rows = check(&domains, &host_names, &expected, &Resolver);
    let passed = rows.iter().all(|row| row.ok);
    let record_list = rows
        .iter()
        .map(|row| {
            let note = if row.kind == "A/AAAA" && row.note.is_empty() && !address_note.is_empty() {
                address_note.clone()
            } else {
                row.note.clone()
            };
            Value::Map(
                [
                    ("kind", Value::string(&row.kind)),
                    ("name", Value::string(&row.name)),
                    ("expected", Value::string(&row.expected)),
                    ("found", Value::string(&row.found)),
                    ("ok", Value::Bool(row.ok)),
                    ("note", Value::string(note)),
                    ("later", Value::Bool(row.later)),
                ]
                .into_iter()
                .map(|(name, value)| (name.to_string(), value))
                .collect(),
            )
        })
        .collect();
    Ok(BTreeMap::from([
        ("recordList".to_string(), Value::List(record_list)),
        ("passed".to_string(), Value::Bool(passed)),
    ]))
}

#[cfg(test)]
#[path = "edge_dns_tests.rs"]
mod tests;
