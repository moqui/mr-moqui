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

//! Attaching a local node (plans/archive/Edge-Node.md §5.7, E10), the edge node's side: what it offers ([`offer`], the link's
//! GetOffer, and the area names of its Hello), and the local node's Configure ([`configure`]): its host names applied
//! with E2's host code, only this node's and never a name another node holds, and the rest of its setup kept on its
//! `moqui.edge.EdgeClient` row. Everything a local node sends is checked as input from any client (§5.8,
//! `moqui_core::edge::setup::check_setup`). The conversions between the messages and `moqui_core::edge::setup` are
//! here too, for the local node's dialer.

use std::net::IpAddr;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::edge::media::SipTrunkState;
use moqui_core::edge::setup::{
    self, CallsOffer, ChatOffer, Configured, EdgeOffer, EdgeSetup, HostResult, MailOffer, PassOffer, SipOffer,
    XmppOffer,
};
use moqui_core::edge::{EdgeHostInfo, HOST_PASSTHROUGH, HOST_TERMINATE};
use moqui_lib::error::Error;

use super::proto::{
    CallsOffer as ProtoCalls, ChatOffer as ProtoChat, ConfigureReply, ConfigureRequest, HostSetting,
    MailOffer as ProtoMail, Offer, PassOffer as ProtoPass, SipOffer as ProtoSip, SipTrunk, WebOffer,
    XmppOffer as ProtoXmpp, XmppSetup,
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
    // the tunnel ports (edge_xmpp_port, edge_xmpp_tls_port), never the server's own XMPP ports: on an all-in-one node a
    // tunnel port that is one of them is not listened on (hook.rs), so it is not offered (§10)
    let all_in_one = moqui_core::edge::settings(factory).all_in_one();
    let tunnel = |bind: Option<std::net::SocketAddr>| {
        bind.filter(|bind| !all_in_one || super::tunnel::own_xmpp_clash(*bind).is_none())
    };
    let (xmpp_tunnel, xmpp_tls_tunnel) = (tunnel(config.xmpp_tunnel), tunnel(config.xmpp_tls_tunnel));
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
            available: xmpp_tunnel.is_some() || xmpp_tls_tunnel.is_some(),
            port: xmpp_tunnel.map_or(0, |address| u32::from(address.port())),
            tls_port: xmpp_tls_tunnel.map_or(0, |address| u32::from(address.port())),
            other_node: false,
            needs_node: false,
        }),
        // the media node as rtc::start advertises it (E7), the address and URLs its Hello carries
        calls: Some(
            match moqui_core::edge::media::advert(factory).filter(|advert| !advert.media_address.is_empty()) {
                Some(advert) => ProtoCalls {
                    available: true,
                    media_address: advert.media_address,
                    turn_urls: advert.turn_urls,
                    other_nodes: false,
                },
                None => ProtoCalls::default(),
            },
        ),
        // the SIP engine as it advertises itself (E8), and the trunks it loaded (E10 phones)
        sip: Some(match moqui_core::edge::media::sip_advert(factory) {
            Some(advert) => ProtoSip {
                available: true,
                sip_address: advert.sip_address,
                tls_address: advert.tls_address,
                domain: advert.domain,
                registrar: advert.registrar,
                trunks: moqui_core::edge::media::sip_trunks(factory)
                    .into_iter()
                    .map(|trunk| SipTrunk {
                        voice_trunk_id: trunk.voice_trunk_id,
                        name: trunk.name,
                        state: trunk.state,
                        error: trunk.error,
                    })
                    .collect(),
                fallbacks: 0,
                other_nodes: false,
            },
            None => ProtoSip::default(),
        }),
        not_available: setup::NOT_AVAILABLE.iter().map(|area| (*area).to_string()).collect(),
        addresses,
        node_hosts: Vec::new(),
    }
}

/// The areas this edge node offers, by name, for its Hello: web always, and mail, chat, pass, xmpp, calls and sip when
/// they are set up. Calls and sip are named as of now; the Hello adds them as the media socket or the SIP engine comes
/// up after the link (`accept.rs` `hello`).
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
    if offer.calls.as_ref().is_some_and(|calls| calls.available) {
        areas.push(CALLS_AREA.to_string());
    }
    if offer.sip.as_ref().is_some_and(|sip| sip.available) {
        areas.push(SIP_AREA.to_string());
    }
    areas
}

/// The area name of the edge node as media node (E7) in its Hello.
pub(crate) const CALLS_AREA: &str = "calls";
/// The area name of the edge node's SIP engine (E8, E10 phones) in its Hello.
pub(crate) const SIP_AREA: &str = "sip";

/// For the SIP offer to local node `node`: (its numbers with a fallback in its copy of `voiceFallbacks`, other local
/// nodes are active too). The engine reads the copies only of the only active node (`sip/edge.rs` `paired_node`).
fn sip_copies(factory: &ExecutionContextFactory, node: &str) -> Result<(u32, bool), Error> {
    let Some(facade) = factory.entity() else {
        return Ok((0, false));
    };
    let fallbacks = facade
        .find(moqui_core::edge::replica::VALUE)?
        .eq("pairedNodeId", moqui_lib::object::Value::string(node))
        .eq(
            "setName",
            moqui_lib::object::Value::string(moqui_core::edge::sip::FALLBACKS_SET),
        )
        .disable_authz()
        .count()?;
    let others = facade
        .find("moqui.edge.EdgeClient")?
        .eq("statusId", moqui_lib::object::Value::string("EdgeClientActive"))
        .condition(moqui_core::entity::Condition::not_equals(
            "pairedNodeId",
            moqui_lib::object::Value::string(node),
        ))
        .disable_authz()
        .count()?;
    Ok((u32::try_from(fallbacks).unwrap_or(0), others > 0))
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
    // the media driver keys rooms by call alone, so it takes call messages only while one local node is linked (E7)
    if let Some(calls) = offer.calls.as_mut().filter(|calls| calls.available) {
        calls.other_nodes = linked.iter().any(|other| other != node);
    }
    if let Some(sip) = offer.sip.as_mut().filter(|sip| sip.available) {
        // a count that does not read leaves the rest of the offer: no fallbacks said, other nodes as the links say
        (sip.fallbacks, sip.other_nodes) = sip_copies(factory, node).unwrap_or_else(|err| {
            tracing::error!("Edge offer to {node}: the SIP copies were not counted: {err}");
            (0, linked.iter().any(|other| other != node))
        });
    }
    Ok(offer)
}

/// The text a Configure answers for a name another node holds: which node is not the asker's business.
const HELD: &str = "another local node has this name";
/// The text for a name of the edge node's own.
const OWN: &str = "the edge node answers for this name itself";

/// The names no local node's setup may claim, nor their subdomains: the edge node's public names and the server's
/// own host names (`moqui_core::edge::claims::own_host_names`: its web host, SMTP (MX) host, bounce domain, the host
/// of its chat address and its mr-pass sites' hosts; on an all-in-one node its XMPP and SIP domains too). A name the
/// operator gives a node on Edge Clients is not checked against them.
fn reserved_names(factory: &ExecutionContextFactory, config: &EdgeConfig) -> Vec<String> {
    let mut names: Vec<String> = config.public_names.clone();
    names.extend(moqui_core::edge::claims::own_host_names(factory));
    names.retain(|name| !name.trim().is_empty());
    names
}

/// The item names of a Configure's setup fields, for a refusal.
fn item_label(field: &str) -> &'static str {
    match field {
        "mailDomains" => "mail domain",
        "chatBridgeIds" => "chat bridge",
        "xmppDomain" => "XMPP domain",
        "sipDomain" => "SIP domain",
        _ => "item",
    }
}

/// On an all-in-one node, take out of local node `node`'s kept setup each item the server holds
/// (`moqui_core::edge::claims::check_setup_claims`: a chat bridge, its XMPP or SIP domain, a mail domain held
/// whole), each answered as a result with the setup field as its mode and an error naming the item; the
/// server's holder is logged, not sent. A mail domain the server also has addresses in is kept and logged.
fn refuse_own_items(
    factory: &ExecutionContextFactory,
    node: &str,
    setup: &mut serde_json::Value,
) -> Result<Vec<HostSetting>, Error> {
    let facade = store::facade(factory)?;
    let found = moqui_core::edge::claims::check_setup_claims(factory, &facade, setup);
    for (domain, count) in &found.shared_domains {
        tracing::info!(
            "Edge node: local node {node} takes mail for {domain}, where the server also has {count} addresses of its \
             own; the domain is split between them by address"
        );
    }
    Ok(found
        .refused
        .into_iter()
        .map(|refused| {
            let label = item_label(refused.field);
            tracing::warn!(
                "Edge node: local node {node} asked for {label} {}, which {} holds; refused",
                refused.key,
                refused.holder
            );
            HostSetting {
                error: format!("{label} {} is the server's own", refused.key),
                host_name: refused.key,
                mode: refused.field.to_string(),
            }
        })
        .collect())
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
    let all_in_one = moqui_core::edge::settings(factory).all_in_one();
    let mut results = Vec::new();
    // E12: the local node's SIP device domain, which only an all-in-one node's registrar takes its phones in
    // (sip/owners.rs); a dedicated edge node's registrar has its own realm (E8), so the field is not kept there. A
    // value that is not a domain is refused on its own, not the whole setup
    let sip_domain = match request.sip_domain.trim() {
        "" => String::new(),
        _ if !all_in_one => String::new(),
        domain => match setup::check_host_name(domain) {
            Ok(domain) => domain,
            Err(why) => {
                tracing::warn!(
                    "Edge node: local node {node} sent SIP domain {domain}, which is not one ({why}); dropped"
                );
                results.push(HostSetting {
                    host_name: domain.to_string(),
                    mode: "sipDomain".to_string(),
                    error: format!("SIP domain {domain} is not a domain: {why}"),
                });
                String::new()
            }
        },
    };
    let checked = setup::check_setup(&setup_of(request))?;
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
    let mut kept_setup = serde_json::json!({
        "mailDomains": checked.mail_domains,
        "chatBridgeIds": checked.chat_bridge_ids,
        "passSites": checked.pass_sites,
        "xmppDomain": checked.xmpp_domain,
        "sipDomain": sip_domain,
    });
    // an all-in-one node keeps its own items: each the server holds is refused by name and left out (§10)
    if all_in_one {
        results.extend(refuse_own_items(factory, node, &mut kept_setup)?);
    }
    // what is kept, after any refusal
    let xmpp_domain = kept_setup["xmppDomain"].as_str().unwrap_or_default().to_string();
    let count = |field: &str| kept_setup[field].as_array().map_or(0, Vec::len);
    match xmpp_node(config, node, linked) {
        _ if xmpp_domain.is_empty() => {}
        (true, _) => tracing::warn!(
            "Edge node: local node {node} set up XMPP for {}, but edge_xmpp_node names {}, so its clients are not \
             tunneled there",
            xmpp_domain,
            config.xmpp_tunnel_node
        ),
        (_, true) => tracing::warn!(
            "Edge node: local node {node} set up XMPP for {}, but edge_xmpp_node is empty and it is not the only node \
             linked, so XMPP clients are refused: set edge_xmpp_node",
            xmpp_domain
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
        count("mailDomains"),
        count("chatBridgeIds"),
        checked.pass_sites.len(),
        if xmpp_domain.is_empty() {
            "off"
        } else {
            xmpp_domain.as_str()
        }
    );
    let mut offer = offer(factory, config, node, linked)?;
    // the registrar takes the kept domain's phones now (sip/mod.rs, `Engine::paired_changed`), so the answer says it
    // rather than the advert, which the SIP engine reads again only every few seconds; the local node keeps its
    // SipNode row's realm from it (dial.rs, `Dialer::note_configured`)
    if all_in_one {
        if let Some(sip) = offer.sip.as_mut().filter(|sip| sip.available) {
            let domain = kept_setup["sipDomain"].as_str().unwrap_or_default().to_string();
            sip.registrar = !domain.is_empty() && registrar_enabled();
            sip.domain = domain;
        }
    }
    Ok(ConfigureReply {
        offer: Some(offer),
        hosts: results,
    })
}

/// Whether this server's SIP registrar is on (`sip_registrar_enabled`, as `sip::phones::registrar_from` reads it).
fn registrar_enabled() -> bool {
    matches!(
        moqui_lib::env::get_prop_or_env("sip_registrar_enabled")
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "true" | "y" | "yes" | "1"
    )
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
        sip_domain: own_sip_domain(),
    }
}

/// E12: the domain this node's phones register to through an all-in-one node (`sip_domain`), as a Configure sends it;
/// empty when unset or not a host name with a domain (logged), as the edge node would refuse it.
fn own_sip_domain() -> String {
    let domain = moqui_lib::env::get_prop_or_env("sip_domain");
    match domain.trim() {
        "" => String::new(),
        domain => setup::check_host_name(domain).unwrap_or_else(|why| {
            tracing::warn!("Edge link: sip_domain {domain} is not a domain ({why}), so a Configure does not send it");
            String::new()
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
    let calls = offer.calls.unwrap_or_default();
    let sip = offer.sip.unwrap_or_default();
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
        calls: CallsOffer {
            available: calls.available && !calls.media_address.is_empty(),
            media_address: calls.media_address,
            turn_urls: calls.turn_urls,
            other_nodes: calls.other_nodes,
        },
        sip: SipOffer {
            available: sip.available && !(sip.sip_address.is_empty() && sip.tls_address.is_empty()),
            sip_address: sip.sip_address,
            tls_address: sip.tls_address,
            domain: sip.domain,
            registrar: sip.registrar,
            trunks: sip
                .trunks
                .into_iter()
                .map(|trunk| SipTrunkState {
                    voice_trunk_id: trunk.voice_trunk_id,
                    name: trunk.name,
                    state: trunk.state,
                    error: trunk.error,
                })
                .collect(),
            fallbacks: u64::from(sip.fallbacks),
            other_nodes: sip.other_nodes,
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
