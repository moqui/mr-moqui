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

use moqui_core::edge::setup::{Configured, EdgeOffer, EdgeSetup, HostResult, XmppOffer};
use moqui_core::edge::{EdgeHostInfo, HOST_PASSTHROUGH, HOST_TERMINATE};

use super::{failed_results, merge_configured, merge_offer, names_of, split_setup};

fn host(name: &str, mode: &str) -> EdgeHostInfo {
    EdgeHostInfo {
        host_name: name.to_string(),
        paired_node_id: "local-1".to_string(),
        mode_enum_id: mode.to_string(),
        sticky_cookie: None,
    }
}

/// An edge node's own sites go to its front as terminate hosts, its local nodes' names in their own modes, each once
/// and in its normal form; a name of its own keeps terminate, and one that is no public name is left out.
#[test]
fn an_edge_node_gives_its_front_its_sites_and_its_local_nodes_names() {
    let own = vec!["Shop.Example.com.".to_string(), "localhost".to_string(), String::new()];
    let forwarded = vec![
        host("erp.example.com", HOST_PASSTHROUGH),
        host("crm.example.com", HOST_TERMINATE),
        host("shop.example.com", HOST_PASSTHROUGH),
    ];
    let names: Vec<(String, String)> = names_of(&own, &forwarded)
        .into_iter()
        .map(|host| (host.host_name, host.mode_enum_id))
        .collect();
    assert_eq!(
        names,
        vec![
            ("crm.example.com".to_string(), HOST_TERMINATE.to_string()),
            ("erp.example.com".to_string(), HOST_PASSTHROUGH.to_string()),
            ("shop.example.com".to_string(), HOST_TERMINATE.to_string()),
        ]
    );
}

/// With two pairings, the host names and the XMPP domain go to the front and the rest to the edge pairing. Each hears
/// every Configure: a cleared XMPP domain reaches the front, and host names set on the front are cleared on the edge
/// pairing, so an edge node does not forward them too.
#[test]
fn a_setup_is_split_between_the_front_and_the_edge_pairing() {
    let setup = EdgeSetup {
        hosts: vec![host("erp.example.com", HOST_PASSTHROUGH)],
        set_hosts: true,
        mail_domains: vec!["example.com".to_string()],
        chat_bridge_ids: vec!["telnyx".to_string()],
        pass_sites: vec!["shop".to_string()],
        xmpp_domain: "example.com".to_string(),
    };
    let (front, edge) = split_setup(&setup);
    assert_eq!(front.hosts, setup.hosts);
    assert!(front.set_hosts);
    assert_eq!(front.xmpp_domain, "example.com");
    assert!(front.mail_domains.is_empty() && front.chat_bridge_ids.is_empty() && front.pass_sites.is_empty());
    assert!(
        edge.hosts.is_empty() && edge.set_hosts,
        "the names cleared on the edge pairing"
    );
    assert!(edge.xmpp_domain.is_empty());
    assert_eq!(edge.mail_domains, setup.mail_domains);
    assert_eq!(edge.chat_bridge_ids, setup.chat_bridge_ids);
    assert_eq!(edge.pass_sites, setup.pass_sites);

    // no web part: the front still hears it, so an XMPP domain cleared here is cleared there
    let no_web = EdgeSetup {
        mail_domains: vec!["example.com".to_string()],
        ..EdgeSetup::default()
    };
    let (front, edge) = split_setup(&no_web);
    assert!(!front.set_hosts && front.xmpp_domain.is_empty() && front.hosts.is_empty());
    assert!(!edge.set_hosts);
    assert_eq!(edge.mail_domains, no_web.mail_domains);
}

/// A part a pairing did not take is answered item by item with why, so the other pairing's answers stand.
#[test]
fn a_part_not_taken_is_answered_item_by_item() {
    let part = EdgeSetup {
        hosts: vec![host("erp.example.com", HOST_TERMINATE)],
        set_hosts: true,
        mail_domains: vec!["example.com".to_string()],
        xmpp_domain: "example.com".to_string(),
        ..EdgeSetup::default()
    };
    let results = failed_results(&part, "front node", "the link is not up");
    let named: Vec<(&str, &str)> = results
        .iter()
        .map(|result| (result.host_name.as_str(), result.mode_enum_id.as_str()))
        .collect();
    assert_eq!(
        named,
        [
            ("erp.example.com", HOST_TERMINATE),
            ("example.com", "mailDomains"),
            ("example.com", "xmppDomain")
        ]
    );
    assert!(results
        .iter()
        .all(|result| result.error == "the front node did not take it: the link is not up"));
    // a part that named nothing still says it failed
    let cleared = failed_results(&EdgeSetup::default(), "edge node", "refused");
    assert_eq!(cleared.len(), 1);
    assert!(cleared[0].error.contains("edge node"));
}

/// The offer of a node with two pairings is the edge pairing's with the front's web access and ports, and the answers
/// of a split Configure are both pairings' host results.
#[test]
fn the_offer_takes_web_access_and_ports_from_the_front() {
    let edge = EdgeOffer {
        public_names: Vec::new(),
        host_modes: Vec::new(),
        not_available: vec!["web".to_string(), "xmpp".to_string()],
        addresses: vec!["203.0.113.5".to_string()],
        ..EdgeOffer::default()
    };
    let mut front = EdgeOffer {
        public_names: vec!["front.example.com".to_string()],
        host_modes: vec![HOST_PASSTHROUGH.to_string(), HOST_TERMINATE.to_string()],
        node_hosts: vec![host("erp.example.com", HOST_PASSTHROUGH)],
        xmpp: XmppOffer {
            available: true,
            port: 5222,
            ..XmppOffer::default()
        },
        not_available: vec!["mail".to_string()],
        ..EdgeOffer::default()
    };
    front.mail.available = false;
    let mut edge_mail = edge.clone();
    edge_mail.mail.available = true;
    let merged = merge_offer(edge_mail, front.clone());
    assert!(merged.mail.available, "mail is the edge pairing's");
    assert_eq!(merged.public_names, front.public_names);
    assert_eq!(merged.host_modes, front.host_modes);
    assert_eq!(merged.node_hosts, front.node_hosts);
    assert_eq!(merged.xmpp.port, 5222);
    assert_eq!(merged.addresses, edge.addresses, "the front named none");
    assert!(merged.not_available.is_empty(), "{:?}", merged.not_available);

    let answered = merge_configured(
        Some(Configured {
            offer: edge,
            hosts: vec![HostResult {
                host_name: "example.com".to_string(),
                mode_enum_id: "mailDomains".to_string(),
                error: String::new(),
            }],
        }),
        Some(Configured {
            offer: front,
            hosts: vec![HostResult {
                host_name: "erp.example.com".to_string(),
                mode_enum_id: HOST_PASSTHROUGH.to_string(),
                error: String::new(),
            }],
        }),
    );
    let names: Vec<&str> = answered.hosts.iter().map(|host| host.host_name.as_str()).collect();
    assert_eq!(names, ["erp.example.com", "example.com"]);
    assert_eq!(answered.offer.public_names, ["front.example.com"]);
}
