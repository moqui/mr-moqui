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

//! A trunk host's targets from fixed records: no DNS query is made.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;

use moqui_lib::sync::lock;

use super::*;

/// Fixed answers, and the SRV names asked for.
#[derive(Default)]
pub(crate) struct Fixed {
    pub srv: HashMap<String, Result<Vec<Srv>, String>>,
    pub hosts: HashMap<String, Vec<IpAddr>>,
    pub asked: Mutex<Vec<String>>,
}

impl Fixed {
    pub fn srv(mut self, name: &str, records: &[(u16, u16, u16, &str)]) -> Fixed {
        let records = records
            .iter()
            .map(|&(priority, weight, port, target)| Srv {
                priority,
                weight,
                port,
                target: target.to_string(),
            })
            .collect();
        self.srv.insert(name.to_string(), Ok(records));
        self
    }

    pub fn host(mut self, host: &str, ips: &[&str]) -> Fixed {
        self.hosts
            .insert(host.to_string(), ips.iter().map(|ip| ip.parse().unwrap()).collect());
        self
    }
}

#[async_trait::async_trait]
impl Resolver for Fixed {
    async fn srv(&self, name: &str) -> Result<Vec<Srv>, String> {
        lock(&self.asked).push(name.to_string());
        self.srv.get(name).cloned().unwrap_or(Ok(Vec::new()))
    }

    async fn addresses(&self, host: &str) -> Vec<IpAddr> {
        self.hosts.get(host).cloned().unwrap_or_default()
    }
}

fn addrs(list: &[&str]) -> Vec<SocketAddr> {
    list.iter().map(|a| a.parse().unwrap()).collect()
}

#[test]
fn the_srv_name_follows_the_transport() {
    assert_eq!(
        srv_name(Transport::Tls, "sip.example.com"),
        "_sips._tcp.sip.example.com"
    );
    assert_eq!(srv_name(Transport::Tcp, "sip.example.com"), "_sip._tcp.sip.example.com");
    assert_eq!(srv_name(Transport::Udp, "sip.example.com"), "_sip._udp.sip.example.com");
}

#[test]
fn records_go_by_priority_then_weight_and_a_dot_is_left_out() {
    let record = |priority, weight, target: &str| Srv {
        priority,
        weight,
        port: 5061,
        target: target.to_string(),
    };
    let ordered = order(vec![
        record(20, 100, "c.example.com"),
        record(10, 10, "b.example.com"),
        record(10, 60, "a.example.com"),
        record(10, 10, "aa.example.com"),
        record(5, 0, ""),
    ]);
    let targets: Vec<&str> = ordered.iter().map(|r| r.target.as_str()).collect();
    assert_eq!(
        targets,
        ["a.example.com", "aa.example.com", "b.example.com", "c.example.com"]
    );
}

#[tokio::test]
async fn srv_targets_are_tried_in_order_each_at_its_port() {
    let fixed = Fixed::default()
        .srv(
            "_sips._tcp.trunk.example.com",
            &[(20, 0, 5071, "backup.example.com"), (10, 0, 5061, "edge.example.com")],
        )
        .host("edge.example.com", &["192.0.2.1", "2001:db8::1"])
        .host("backup.example.com", &["192.0.2.9"])
        .host("trunk.example.com", &["198.51.100.1"]);
    let found = targets(&fixed, "Trunk.Example.com.", None, Transport::Tls).await;
    assert_eq!(
        found,
        addrs(&["192.0.2.1:5061", "[2001:db8::1]:5061", "192.0.2.9:5071"])
    );
}

#[tokio::test]
async fn no_srv_records_or_a_failed_lookup_is_the_host_at_the_default_port() {
    let fixed = Fixed::default().host("pbx.example.com", &["192.0.2.5"]);
    assert_eq!(
        targets(&fixed, "pbx.example.com", None, Transport::Udp).await,
        addrs(&["192.0.2.5:5060"])
    );
    assert_eq!(
        targets(&fixed, "pbx.example.com", None, Transport::Tls).await,
        addrs(&["192.0.2.5:5061"])
    );
    let mut failing = Fixed::default().host("pbx.example.com", &["192.0.2.5"]);
    failing
        .srv
        .insert("_sip._tcp.pbx.example.com".to_string(), Err("SERVFAIL".to_string()));
    assert_eq!(
        targets(&failing, "pbx.example.com", None, Transport::Tcp).await,
        addrs(&["192.0.2.5:5060"])
    );
    // an unknown host has no target (the trunk is logged by name and still loaded)
    assert!(targets(&fixed, "nowhere.example.com", None, Transport::Udp)
        .await
        .is_empty());
}

#[tokio::test]
async fn records_that_say_not_offered_leave_no_target() {
    let fixed = Fixed::default()
        .srv("_sip._udp.trunk.example.com", &[(0, 0, 0, "")])
        .host("trunk.example.com", &["192.0.2.1"]);
    assert!(targets(&fixed, "trunk.example.com", None, Transport::Udp)
        .await
        .is_empty());
}

#[tokio::test]
async fn an_explicit_port_or_an_address_skips_srv() {
    let fixed = Fixed::default()
        .srv("_sip._udp.trunk.example.com", &[(10, 0, 5099, "edge.example.com")])
        .host("edge.example.com", &["192.0.2.1"])
        .host("trunk.example.com", &["192.0.2.7"]);
    assert_eq!(
        targets(&fixed, "trunk.example.com", Some(5080), Transport::Udp).await,
        addrs(&["192.0.2.7:5080"])
    );
    assert_eq!(
        targets(&fixed, "198.51.100.4", None, Transport::Tls).await,
        addrs(&["198.51.100.4:5061"])
    );
    assert_eq!(
        targets(&fixed, "[2001:db8::4]", Some(5070), Transport::Tcp).await,
        addrs(&["[2001:db8::4]:5070"])
    );
    assert!(lock(&fixed.asked).is_empty(), "{:?}", lock(&fixed.asked));
}
