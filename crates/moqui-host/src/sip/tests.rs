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

fn settings(pairs: &[(&str, &str)]) -> Result<Option<SipSettings>, String> {
    let map: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    settings_from(&|name| map.get(name).cloned().unwrap_or_default())
}

#[test]
fn the_listeners_are_off_until_a_port_is_set_and_take_their_hosts() {
    assert_eq!(settings(&[("sip_rtp_ports", "20000-20999")]), Ok(None));
    let s = settings(&[("sip_port", "5060")]).unwrap().unwrap();
    assert_eq!(s.listen, Some("127.0.0.1:5060".parse().unwrap()));
    assert_eq!(s.tls, None);
    assert_eq!(s.rtp_ports, "20000-20999");
    assert_eq!(s.rtp_host, "127.0.0.1".parse::<IpAddr>().unwrap());
    let s = settings(&[
        ("sip_tls_port", "5061"),
        ("sip_host", "0.0.0.0"),
        ("sip_tls_host", "127.0.0.2"),
        ("sip_rtp_host", "127.0.0.3"),
        ("sip_rtp_ports", "30000-30010"),
        ("sip_public_address", "198.51.100.20"),
        ("sip_tls_ca_file", "/nowhere/ca.pem"),
    ])
    .unwrap()
    .unwrap();
    assert_eq!(s.listen, None);
    assert_eq!(s.tls, Some("127.0.0.2:5061".parse().unwrap()));
    assert_eq!(s.rtp_host, "127.0.0.3".parse::<IpAddr>().unwrap());
    assert_eq!(s.public, Some("198.51.100.20".parse().unwrap()));
    assert_eq!(s.ca_file, Some(PathBuf::from("/nowhere/ca.pem")));
    assert!(settings(&[("sip_port", "fifty")]).is_err());
    assert!(settings(&[("sip_port", "5060"), ("sip_public_address", "sip.example")]).is_err());
}

#[test]
fn a_port_range_hands_out_free_ports_and_wraps() {
    let range = PortRange::parse("41850-41852", "127.0.0.1".parse().unwrap()).unwrap();
    let a = range.take().unwrap();
    let b = range.take().unwrap();
    assert_ne!(a.local_addr().unwrap().port(), b.local_addr().unwrap().port());
    let c = range.take();
    let d = range.take();
    // three ports, so a fourth is refused while the three are held (one may be taken by another process)
    assert!(c.is_none() || d.is_none());
    drop((a, b, c, d));
    assert!(range.take().is_some(), "freed ports come back");
    assert!(PortRange::parse("2000", "127.0.0.1".parse().unwrap()).is_err());
    assert!(PortRange::parse("3000-2000", "127.0.0.1".parse().unwrap()).is_err());
}
