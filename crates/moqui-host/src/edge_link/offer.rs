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

//! Attaching a local node (plans/Edge-Node.md §5.7, E10), the edge node's side: what it offers ([`offer`], the link's
//! GetOffer, and the area names of its Hello), and the local node's Configure ([`configure`]): its host names applied
//! with E2's host code, only this node's and never a name another node holds, and the rest of its setup kept on its
//! `moqui.edge.EdgeClient` row. Everything a local node sends is checked as input from any client (§5.8,
//! `moqui_core::edge::setup::check_setup`). The conversions between the messages and `moqui_core::edge::setup` are
//! here too, for the local node's dialer.

use std::net::IpAddr;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::edge::setup::{
    self, ChatOffer, Configured, EdgeOffer, EdgeSetup, HostResult, MailOffer, PassOffer, XmppOffer,
};
use moqui_core::edge::{EdgeHostInfo, HOST_PASSTHROUGH, HOST_TERMINATE};
use moqui_lib::error::Error;

use super::proto::{
    ChatOffer as ProtoChat, ConfigureReply, ConfigureRequest, HostSetting, MailOffer as ProtoMail, Offer,
    PassOffer as ProtoPass, WebOffer, XmppOffer as ProtoXmpp, XmppSetup,
};
use super::{hosts, store, EdgeConfig};

fn property(name: &str) -> String {
    moqui_lib::env::get_prop_or_env(name).trim().to_string()
}

fn on(name: &str) -> bool {
    matches!(property(name).to_ascii_lowercase().as_str(), "true" | "y" | "yes" | "1")
}

/// The edge node's DKIM key record and selector, which sign its local nodes' mail with the MTA in moqui; none in a
/// build without the mail client, or with no key.
#[cfg(feature = "imap")]
fn dkim() -> (String, String) {
    match moqui_mail::auth::dkim_record() {
        Ok(record) => (moqui_mail::auth::selector(), record),
        Err(err) => {
            tracing::warn!("Edge offer: no DKIM record: {err}");
            (moqui_mail::auth::selector(), String::new())
        }
    }
}

#[cfg(not(feature = "imap"))]
fn dkim() -> (String, String) {
    (String::new(), String::new())
}

/// What this edge node offers, without a node's host names (those are [`offer`]'s).
fn base_offer(factory: &ExecutionContextFactory, config: &EdgeConfig) -> Offer {
    let listener = cfg!(feature = "inbound-smtp") && !property("smtp_listen_port").is_empty();
    let mta = if property("edge_mail_mta") == "moqui" {
        "moqui"
    } else {
        "postfix"
    };
    let (dkim_selector, dkim_record) = if listener && mta == "moqui" {
        dkim()
    } else {
        (String::new(), String::new())
    };
    let mx_host = match property("smtp_hostname") {
        name if !name.is_empty() => name,
        _ => config.public_names.first().cloned().unwrap_or_default(),
    };
    let webhooks = cfg!(feature = "channels") && on("channels_webhooks_enabled");
    let public_url = match property(setup::CHAT_PUBLIC_URL) {
        url if !url.is_empty() => url.trim_end_matches('/').to_string(),
        _ => config
            .public_names
            .first()
            .map(|name| format!("https://{name}"))
            .unwrap_or_default(),
    };
    let addresses = property("edge_public_addresses")
        .split([',', ' '])
        .filter(|text| !text.is_empty())
        .filter_map(|text| match text.parse::<IpAddr>() {
            Ok(ip) => Some(ip.to_string()),
            Err(_) => {
                tracing::error!("edge_public_addresses: {text} is not an IP address; it is left out");
                None
            }
        })
        .collect();
    Offer {
        web: Some(WebOffer {
            public_names: config.public_names.clone(),
            modes: vec![HOST_PASSTHROUGH.to_string(), HOST_TERMINATE.to_string()],
        }),
        mail: Some(ProtoMail {
            available: listener,
            mta: mta.to_string(),
            listener,
            mx_host,
            bounce_domain: property("edge_mail_bounce_domain"),
            dkim_selector,
            dkim_record,
        }),
        chat: Some(ProtoChat { webhooks, public_url }),
        pass: Some(ProtoPass {
            available: factory.components().iter().any(|component| component.name == "mr-pass"),
            client_id: moqui_core::pass_authority::DEFAULT_EDGE_CLIENT.to_string(),
        }),
        xmpp: Some(ProtoXmpp {
            available: config.xmpp_tunnel.is_some() || config.xmpp_tls_tunnel.is_some(),
            port: config.xmpp_tunnel.map_or(0, |address| u32::from(address.port())),
            tls_port: config.xmpp_tls_tunnel.map_or(0, |address| u32::from(address.port())),
            other_node: false,
            needs_node: false,
        }),
        not_available: setup::NOT_AVAILABLE.iter().map(|area| (*area).to_string()).collect(),
        addresses,
        node_hosts: Vec::new(),
    }
}

/// The areas this edge node offers, by name, for its Hello: web always, and mail, chat, pass and xmpp when they are
/// set up.
pub(crate) fn areas(factory: &ExecutionContextFactory, config: &EdgeConfig) -> Vec<String> {
    let offer = base_offer(factory, config);
    let mut areas = vec!["web".to_string()];
    if offer.mail.as_ref().is_some_and(|mail| mail.available) {
        areas.push("mail".to_string());
    }
    if offer.chat.as_ref().is_some_and(|chat| chat.webhooks) {
        areas.push("chat".to_string());
    }
    if offer.pass.as_ref().is_some_and(|pass| pass.available) {
        areas.push("pass".to_string());
    }
    if offer.xmpp.as_ref().is_some_and(|xmpp| xmpp.available) {
        areas.push("xmpp".to_string());
    }
    areas
}

fn host_setting(host: &EdgeHostInfo) -> HostSetting {
    HostSetting {
        host_name: host.host_name.clone(),
        mode: host.mode_enum_id.clone(),
        error: String::new(),
    }
}

/// Where the XMPP tunnels go for local node `node`, by the tunnel ports' own rule (`tunnel::tunnel_node`): to the node
/// `edge_xmpp_node` names, else to the only node linked. Answers (other node, needs a node): `edge_xmpp_node` names
/// another node; or it is empty and `linked` is not exactly `node`, so the tunnels go nowhere until it names one.
pub(crate) fn xmpp_node(config: &EdgeConfig, node: &str, linked: &[String]) -> (bool, bool) {
    let named = config.xmpp_tunnel_node.as_str();
    if named.is_empty() {
        (false, !matches!(linked, [only] if only == node))
    } else {
        (named != node, false)
    }
}

/// What this edge node offers local node `node` now, with its host names here; `linked` is the nodes linked now.
pub(crate) fn offer(
    factory: &ExecutionContextFactory,
    config: &EdgeConfig,
    node: &str,
    linked: &[String],
) -> Result<Offer, Error> {
    let mut offer = base_offer(factory, config);
    offer.node_hosts = hosts::list(factory, Some(node))?.iter().map(host_setting).collect();
    if let Some(xmpp) = offer.xmpp.as_mut() {
        (xmpp.other_node, xmpp.needs_node) = xmpp_node(config, node, linked);
    }
    Ok(offer)
}

/// The text a Configure answers for a name another node holds: which node is not the asker's business.
const HELD: &str = "another local node has this name";
/// The text for a name of the edge node's own.
const OWN: &str = "the edge node answers for this name itself";

/// The host part of an http(s) address, lower case.
pub(crate) fn url_host(url: &str) -> Option<String> {
    let rest = url.trim().split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = host.rsplit_once(':').map_or(host, |(name, port)| {
        if port.bytes().all(|byte| byte.is_ascii_digit()) {
            name
        } else {
            host
        }
    });
    Some(host.to_ascii_lowercase()).filter(|host| !host.is_empty())
}

/// The names no local node's setup may claim, nor their subdomains: the edge node's public names and web host, its
/// SMTP (MX) host, its bounce domain, the host of its chat address, and its mr-pass sites' hosts. A name the operator
/// gives a node on Edge Clients is not checked against them.
fn reserved_names(factory: &ExecutionContextFactory, config: &EdgeConfig) -> Vec<String> {
    let mut names: Vec<String> = config.public_names.clone();
    for name in ["webapp_http_host", "smtp_hostname", "edge_mail_bounce_domain"] {
        names.push(property(name));
    }
    names.extend(url_host(&property(setup::CHAT_PUBLIC_URL)));
    names.extend(moqui_core::screen::public_site_hosts(factory));
    names.retain(|name| !name.trim().is_empty());
    names
}

/// Apply local node `node`'s Configure: with `set_hosts` its host names become the request's (a name of another
/// node refused, one of its own not listed removed), each name on its own; the rest of the setup is kept on its
/// client row. The caller loads the host table again after.
///
/// # Errors
///
/// Returns why the request is refused whole (not input), or the database failed.
pub(crate) fn configure(
    factory: &ExecutionContextFactory,
    config: &EdgeConfig,
    node: &str,
    request: ConfigureRequest,
    linked: &[String],
) -> Result<ConfigureReply, Error> {
    let checked = setup::check_setup(&setup_of(request))?;
    let mut results = Vec::new();
    if checked.set_hosts {
        let all = hosts::list(factory, None)?;
        let reserved = reserved_names(factory, config);
        let mut kept: Vec<String> = Vec::new();
        for host in &checked.hosts {
            let mut result = host_setting(host);
            let name = match setup::check_host_name(&host.host_name) {
                Ok(name) => name,
                Err(why) => {
                    result.error = why;
                    results.push(result);
                    continue;
                }
            };
            result.host_name = name.clone();
            if setup::is_reserved(&name, &reserved) {
                tracing::warn!("Edge node: local node {node} asked for {name}, a name of this edge node; refused");
                result.error = OWN.to_string();
                results.push(result);
                continue;
            }
            if all
                .iter()
                .any(|held| held.host_name == name && held.paired_node_id != node)
            {
                result.error = HELD.to_string();
                results.push(result);
                continue;
            }
            let saved = hosts::add(
                factory,
                &EdgeHostInfo {
                    host_name: name.clone(),
                    paired_node_id: node.to_string(),
                    mode_enum_id: host.mode_enum_id.clone(),
                },
            );
            match saved {
                Ok(_) => kept.push(name),
                Err(err) => {
                    tracing::warn!("Edge node: local node {node}'s host {name} was not taken: {err}");
                    // a race with another node's add: its message names that node
                    result.error = if err.message().contains("is a host of local node") {
                        HELD.to_string()
                    } else {
                        "the edge node could not take this name".to_string()
                    };
                }
            }
            results.push(result);
        }
        for old in all.iter().filter(|held| held.paired_node_id == node) {
            if kept.contains(&old.host_name) || checked.hosts.iter().any(|host| host.host_name == old.host_name) {
                continue;
            }
            match hosts::remove(factory, &old.host_name) {
                Ok(_) => tracing::info!(
                    "Edge node: host {} of local node {node} removed by its setup",
                    old.host_name
                ),
                Err(err) => tracing::error!(
                    "Edge node: host {} of local node {node} was not removed: {err}",
                    old.host_name
                ),
            }
        }
    }
    let kept_setup = serde_json::json!({
        "mailDomains": checked.mail_domains,
        "chatBridgeIds": checked.chat_bridge_ids,
        "passSites": checked.pass_sites,
        "xmppDomain": checked.xmpp_domain,
    });
    match xmpp_node(config, node, linked) {
        _ if checked.xmpp_domain.is_empty() => {}
        (true, _) => tracing::warn!(
            "Edge node: local node {node} set up XMPP for {}, but edge_xmpp_node names {}, so its clients are not \
             tunneled there",
            checked.xmpp_domain,
            config.xmpp_tunnel_node
        ),
        (_, true) => tracing::warn!(
            "Edge node: local node {node} set up XMPP for {}, but edge_xmpp_node is empty and it is not the only node \
             linked, so XMPP clients are refused: set edge_xmpp_node",
            checked.xmpp_domain
        ),
        _ => {}
    }
    store::save_setup(factory, node, &kept_setup.to_string())?;
    tracing::info!(
        "Edge node: local node {node} set up: {} host names, {} mail domains, {} chat bridges, {} PASS sites, XMPP {}",
        if checked.set_hosts {
            checked.hosts.len().to_string()
        } else {
            "unchanged".to_string()
        },
        checked.mail_domains.len(),
        checked.chat_bridge_ids.len(),
        checked.pass_sites.len(),
        if checked.xmpp_domain.is_empty() {
            "off"
        } else {
            checked.xmpp_domain.as_str()
        }
    );
    Ok(ConfigureReply {
        offer: Some(offer(factory, config, node, linked)?),
        hosts: results,
    })
}

/// Whether a Configure is input this edge node takes, before anything is changed.
///
/// # Errors
///
/// Returns why it is refused.
pub(crate) fn request_check(request: &ConfigureRequest) -> Result<(), Error> {
    setup::check_setup(&setup_of(request.clone())).map(|_| ())
}

fn setup_of(request: ConfigureRequest) -> EdgeSetup {
    EdgeSetup {
        hosts: request
            .hosts
            .into_iter()
            .map(|host| EdgeHostInfo {
                host_name: host.host_name,
                paired_node_id: String::new(),
                mode_enum_id: host.mode,
            })
            .collect(),
        set_hosts: request.set_hosts,
        mail_domains: request.mail_domains,
        chat_bridge_ids: request.chat_bridge_ids,
        pass_sites: request.pass_sites,
        xmpp_domain: request.xmpp.map(|xmpp| xmpp.domain).unwrap_or_default(),
    }
}

/// A local node's Configure, from its setup.
pub(crate) fn request_of(setup: &EdgeSetup) -> ConfigureRequest {
    ConfigureRequest {
        hosts: setup.hosts.iter().map(host_setting).collect(),
        mail_domains: setup.mail_domains.clone(),
        chat_bridge_ids: setup.chat_bridge_ids.clone(),
        pass_sites: setup.pass_sites.clone(),
        set_hosts: setup.set_hosts,
        xmpp: (!setup.xmpp_domain.is_empty()).then(|| XmppSetup {
            domain: setup.xmpp_domain.clone(),
        }),
    }
}

/// An offer as the local node's services see it.
pub(crate) fn offer_of(offer: Offer) -> EdgeOffer {
    let web = offer.web.unwrap_or_default();
    let mail = offer.mail.unwrap_or_default();
    let chat = offer.chat.unwrap_or_default();
    let pass = offer.pass.unwrap_or_default();
    let xmpp = offer.xmpp.unwrap_or_default();
    // a port that is not one (0, or past u16 from a newer peer) is off
    let port = |port: u32| u16::try_from(port).unwrap_or(0);
    EdgeOffer {
        public_names: web.public_names,
        host_modes: web.modes,
        mail: MailOffer {
            available: mail.available,
            mta: mail.mta,
            listener: mail.listener,
            mx_host: mail.mx_host,
            bounce_domain: mail.bounce_domain,
            dkim_selector: mail.dkim_selector,
            dkim_record: mail.dkim_record,
        },
        chat: ChatOffer {
            webhooks: chat.webhooks,
            public_url: chat.public_url,
        },
        pass: PassOffer {
            available: pass.available,
            client_id: pass.client_id,
        },
        xmpp: XmppOffer {
            available: xmpp.available,
            port: port(xmpp.port),
            tls_port: port(xmpp.tls_port),
            other_node: xmpp.other_node,
            needs_node: xmpp.needs_node,
        },
        not_available: offer.not_available,
        addresses: offer.addresses,
        node_hosts: offer
            .node_hosts
            .into_iter()
            .map(|host| EdgeHostInfo {
                host_name: host.host_name,
                paired_node_id: String::new(),
                mode_enum_id: host.mode,
            })
            .collect(),
    }
}

/// The edge node's answer to a Configure, as the local node's services see it.
pub(crate) fn configured_of(reply: ConfigureReply) -> Configured {
    Configured {
        offer: offer_of(reply.offer.unwrap_or_default()),
        hosts: reply
            .hosts
            .into_iter()
            .map(|host| HostResult {
                host_name: host.host_name,
                mode_enum_id: host.mode,
                error: host.error,
            })
            .collect(),
    }
}
