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

//! Who holds a name, address, number or bridge (plans/Edge-Node.md §10, E12). Ownership is exclusive: each item
//! belongs to the server itself or to one paired node. [`own_claim`] finds the server's own holder of an item and
//! [`replica_claim`] the paired node whose copies (its replicated sets, its Configure) hold it. Configure and
//! replication refuse what the server holds, and the server's own setup refuses what a paired node holds, each with
//! [`claimed_message`] naming the item.
//!
//! The server's host names ([`own_host_names`]) are what a dedicated edge node already reserves (§5.7); an all-in-one
//! node adds its `xmpp_domain` and `sip_domain`.

use std::collections::BTreeMap;

use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};

use crate::context::ExecutionContextFactory;
use crate::entity::{Condition, EntityFacade, EntityValue};

/// What kind of item a claim is of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimKind {
    /// A public host name (an `EdgeHost` of a paired node), or a subdomain of the server's own.
    HostName,
    /// A mail address an account receives at: an `EmailAccount`'s or an `EmailIdentity`'s `emailAddress`.
    MailAddress,
    /// A `ChatBridge` by its `chatBridgeId`.
    ChatBridge,
    /// An XMPP domain: the server's `xmpp_domain`, a paired node's Configure `xmppDomain`.
    XmppDomain,
    /// A phone number in E.164: the server's `PhoneNumber`, a paired node's replicated `PhoneNumberFallback`.
    PhoneNumber,
    /// A SIP device domain (registrar realm): the server's `sip_domain`, a paired node's Configure `sipDomain`.
    SipDomain,
}

impl ClaimKind {
    /// The item's name in a message.
    pub fn label(self) -> &'static str {
        match self {
            Self::HostName => "host name",
            Self::MailAddress => "mail address",
            Self::ChatBridge => "chat bridge",
            Self::XmppDomain => "XMPP domain",
            Self::PhoneNumber => "phone number",
            Self::SipDomain => "SIP domain",
        }
    }

    /// The kind's stable id, as a row keeps it (`EdgeReplicaConflict.claimKind`) and a service takes it.
    pub fn id(self) -> &'static str {
        match self {
            Self::HostName => "HostName",
            Self::MailAddress => "MailAddress",
            Self::ChatBridge => "ChatBridge",
            Self::XmppDomain => "XmppDomain",
            Self::PhoneNumber => "PhoneNumber",
            Self::SipDomain => "SipDomain",
        }
    }

    /// The kind of an [`id`](Self::id); `None` for another text.
    pub fn from_id(id: &str) -> Option<Self> {
        [
            Self::HostName,
            Self::MailAddress,
            Self::ChatBridge,
            Self::XmppDomain,
            Self::PhoneNumber,
            Self::SipDomain,
        ]
        .into_iter()
        .find(|kind| kind.id() == id.trim())
    }
}

/// The item a replicated record claims, by its entity: an `EmailAccount`'s or `EmailIdentity`'s address, a
/// `ChatBridge`'s id, a `PhoneNumberFallback`'s number. `None` for a record that claims nothing (an `EmailServer`, a
/// trunk, a group), or one with no key.
pub fn copy_claim(
    entity_name: &str,
    pk: &serde_json::Value,
    fields: &serde_json::Value,
) -> Option<(ClaimKind, String)> {
    let of = |value: &serde_json::Value, name: &str| {
        value[name]
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    match entity_name {
        ACCOUNT | IDENTITY => of(fields, "emailAddress").map(|key| (ClaimKind::MailAddress, key)),
        BRIDGE => of(pk, "chatBridgeId").map(|key| (ClaimKind::ChatBridge, key)),
        FALLBACK => of(pk, "phoneNumber").map(|key| (ClaimKind::PhoneNumber, key)),
        _ => None,
    }
}

/// An item of a paired node's Configure that the server holds, so it was taken out of the setup kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    /// The setup's field it was in (`chatBridgeIds`, `mailDomains`, ..).
    pub field: &'static str,
    pub kind: ClaimKind,
    pub key: String,
    /// The server's holder, for the log ("the server's ChatBridge B1"); the paired node is told only the item.
    pub holder: String,
}

/// What [`check_setup_claims`] found in a paired node's Configure.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SetupClaims {
    /// The items refused, each taken out of the setup.
    pub refused: Vec<Refused>,
    /// The mail domains kept that the server also has receiving addresses in, with how many: the domain is split
    /// between the sides by address (§10), which is allowed and noted.
    pub shared_domains: Vec<(String, u64)>,
}

/// The fields of a kept setup (`EdgeClient.setupJson`) that name items one side holds, by kind: each a string or a
/// list of strings. `mailDomains` is checked by [`check_setup_claims`] on its own rule. A Configure names no phone
/// numbers: a paired node's numbers are its replicated `voiceFallbacks`, checked as replica conflicts.
const SETUP_ITEMS: [(&str, ClaimKind); 3] = [
    ("chatBridgeIds", ClaimKind::ChatBridge),
    ("xmppDomain", ClaimKind::XmppDomain),
    ("sipDomain", ClaimKind::SipDomain),
];

/// On an all-in-one node, take out of a paired node's setup (the JSON kept on its `EdgeClient`) each item the server
/// holds ([`own_claim`]): a chat bridge, its XMPP domain, its SIP domain, and a mail domain the server holds whole. A
/// mail domain is split by address (§10): the server holds one whole only as its own bounce domain
/// (`edge_mail_bounce_domain`, or a subdomain of it), and one it merely has addresses in is kept and noted in
/// [`SetupClaims::shared_domains`]. An item whose check fails is refused, logged, as the server cannot tell.
pub fn check_setup_claims(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    setup: &mut serde_json::Value,
) -> SetupClaims {
    let mut found = SetupClaims::default();
    let mut refuse = |field: &'static str, kind: ClaimKind, key: &str| -> bool {
        let holder = match own_claim(factory, facade, kind, key) {
            Ok(Some(holder)) => holder,
            Ok(None) => return false,
            Err(error) => {
                tracing::error!(
                    "{} {key}: whether the server holds it could not be read, so it is refused: {error}",
                    kind.label()
                );
                "the server (the check failed)".to_string()
            }
        };
        found.refused.push(Refused {
            field,
            kind,
            key: key.trim().to_string(),
            holder,
        });
        true
    };
    for (field, kind) in SETUP_ITEMS {
        match &mut setup[field] {
            serde_json::Value::String(key) if !key.trim().is_empty() => {
                if refuse(field, kind, key) {
                    key.clear();
                }
            }
            serde_json::Value::Array(keys) => {
                keys.retain(|key| !key.as_str().is_some_and(|key| refuse(field, kind, key)))
            }
            _ => {}
        }
    }
    let bounce = domain_property("edge_mail_bounce_domain");
    if let serde_json::Value::Array(domains) = &mut setup["mailDomains"] {
        domains.retain(|domain| {
            let Some(name) = domain
                .as_str()
                .map(|name| name.trim().trim_end_matches('.').to_ascii_lowercase())
            else {
                return true;
            };
            if super::setup::is_reserved(&name, std::slice::from_ref(&bounce)) {
                found.refused.push(Refused {
                    field: "mailDomains",
                    kind: ClaimKind::MailAddress,
                    key: name,
                    holder: format!("the server's bounce domain {bounce}"),
                });
                return false;
            }
            match own_domain_addresses(facade, &name) {
                Ok(0) => {}
                Ok(count) => found.shared_domains.push((name, count)),
                Err(error) => {
                    tracing::error!("mail domain {name}: the server's addresses in it could not be counted: {error}")
                }
            }
            true
        });
    }
    found
}

/// How many of the server's own hosted or shared accounts, not removed, have an address in `domain`.
fn own_domain_addresses(facade: &EntityFacade, domain: &str) -> Result<u64, Error> {
    let count = facade
        .find(ACCOUNT)?
        .condition(Condition::ignore_case(Condition::like(
            "emailAddress",
            Value::string(format!("%@{domain}")),
        )))
        .condition(Condition::is_in(
            "accountKindEnumId",
            RECEIVING_KINDS.iter().map(|kind| Value::string(*kind)).collect(),
        ))
        .condition(Condition::or(vec![
            Condition::is_null("statusId"),
            Condition::not_equals("statusId", Value::string("EasRemoved")),
        ]))
        .disable_authz()
        .count()?;
    Ok(u64::try_from(count).unwrap_or(0))
}

/// The replicated set of a paired node's mail recipients (MoquiDefaultConf.xml `<edge>`).
pub const MAIL_SET: &str = "mailRecipients";
/// The replicated set of a paired node's chat bridges.
pub const CHAT_SET: &str = "chatBridges";
/// The replicated set of a paired node's numbers' fallbacks: the numbers the replica knows of.
pub const FALLBACKS_SET: &str = "voiceFallbacks";

const ACCOUNT: &str = "moqui.basic.email.EmailAccount";
const IDENTITY: &str = "moqui.basic.email.EmailIdentity";
const SERVER: &str = "moqui.basic.email.EmailServer";
const BRIDGE: &str = "moqui.chat.ChatBridge";
const PHONE_NUMBER: &str = "moqui.call.PhoneNumber";
const FALLBACK: &str = "moqui.call.PhoneNumberFallback";
const EDGE_HOST: &str = "moqui.edge.EdgeHost";
const EDGE_CLIENT: &str = "moqui.edge.EdgeClient";
/// The account kinds the SMTP listener delivers to, as moqui-mail's delivery reads them.
const RECEIVING_KINDS: [&str; 2] = ["EakHosted", "EakShared"];
/// The most copies a scan of a replicated set reads.
const SCAN_LIMIT: usize = 100_000;

/// The message refusing `key` of `kind` because `holder` has it: "mail address ann@example.com is held by the
/// server's EmailAccount 100010".
pub fn claimed_message(kind: ClaimKind, key: &str, holder: &str) -> String {
    format!("{} {} is held by {holder}", kind.label(), key.trim())
}

/// The host part of an http(s) address, lower case.
pub fn url_host(url: &str) -> Option<String> {
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

fn property(name: &str) -> String {
    moqui_lib::env::get_prop_or_env(name).trim().to_string()
}

/// A domain property in its normal form: lower case, no trailing dot; empty when unset.
fn domain_property(name: &str) -> String {
    property(name).trim_end_matches('.').to_ascii_lowercase()
}

/// The server's own host names, which no paired node may claim, nor their subdomains (§5.7): its web host, its SMTP
/// (MX) host, its bounce domain, the host of its chat address and its mr-pass sites' hosts; on an all-in-one node
/// also its `xmpp_domain` and `sip_domain`, and the domains of its own receiving accounts ([`own_mail_domains`]), so
/// a paired node cannot take a mail domain's apex or its `mta-sts.` and `autoconfig.` hosts. The edge link adds the
/// names its Hello gives. A mail domain split by address stays allowed in a setup's `mailDomains`.
pub fn own_host_names(factory: &ExecutionContextFactory) -> Vec<String> {
    let mut names: Vec<String> = ["webapp_http_host", "smtp_hostname", "edge_mail_bounce_domain"]
        .iter()
        .map(|name| property(name))
        .collect();
    names.extend(url_host(&property(super::setup::CHAT_PUBLIC_URL)));
    names.extend(crate::screen::public_site_hosts(factory));
    if super::settings(factory).all_in_one() {
        names.push(domain_property("xmpp_domain"));
        names.push(domain_property("sip_domain"));
        match factory.entity() {
            Some(facade) => match own_mail_domains(&facade) {
                Ok(domains) => names.extend(domains),
                Err(error) => tracing::error!("the server's own mail domains could not be read: {error}"),
            },
            None => tracing::error!("the server's own mail domains could not be read: no entity facade"),
        }
    }
    names.retain(|name| !name.trim().is_empty());
    names.sort();
    names.dedup();
    names
}

/// The domains, lower case, of the server's own accounts that receive mail ([`own_mail_address`]'s rule): hosted or
/// shared, not removed, with no incoming (IMAP or POP) server.
fn own_mail_domains(facade: &EntityFacade) -> Result<Vec<String>, Error> {
    let accounts = facade
        .find(ACCOUNT)?
        .condition(Condition::is_in(
            "accountKindEnumId",
            RECEIVING_KINDS.iter().map(|kind| Value::string(*kind)).collect(),
        ))
        .select_field("emailAddress")
        .select_field("accountKindEnumId")
        .select_field("statusId")
        .select_field("emailServerId")
        .disable_authz()
        .list()?;
    let mut domains = Vec::new();
    for row in accounts.values() {
        let address = text(row, "emailAddress");
        let Some((_, domain)) = address.trim().rsplit_once('@') else {
            continue;
        };
        let domain = domain.trim_end_matches('.').to_ascii_lowercase();
        if domain.is_empty() || domains.contains(&domain) || !receives(facade, row)? {
            continue;
        }
        domains.push(domain);
    }
    Ok(domains)
}

/// Whether account `row` receives mail on the server: a hosted or shared account, not removed, with no incoming (IMAP
/// or POP) server.
fn receives(facade: &EntityFacade, row: &EntityValue) -> Result<bool, Error> {
    if !RECEIVING_KINDS.contains(&text(row, "accountKindEnumId").as_str()) || text(row, "statusId") == "EasRemoved" {
        return Ok(false);
    }
    let server = text(row, "emailServerId");
    if server.is_empty() {
        return Ok(true);
    }
    let store_host = facade
        .find(SERVER)?
        .eq("emailServerId", Value::string(&server))
        .select_field("storeHost")
        .disable_authz()
        .one()?
        .map(|row| text(&row, "storeHost"))
        .unwrap_or_default();
    Ok(store_host.trim().is_empty())
}

fn text(row: &EntityValue, field: &str) -> String {
    row.get(field).map(to_plain_string).unwrap_or_default()
}

/// What of the server's own holds `key` of `kind`, for [`claimed_message`] ("the server's EmailAccount 100010"), or
/// `None` when the server holds no such item. A host name is held when it is one of [`own_host_names`] or a
/// subdomain of one; an XMPP domain when it is the server's `xmpp_domain` or a subdomain (`rooms.`, `upload.`).
///
/// # Errors
///
/// Returns the database's error; the caller refuses the item then, as it cannot tell.
pub fn own_claim(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    kind: ClaimKind,
    key: &str,
) -> Result<Option<String>, Error> {
    let key = key.trim();
    if key.is_empty() {
        return Ok(None);
    }
    match kind {
        ClaimKind::HostName => {
            let name = key.trim_end_matches('.').to_ascii_lowercase();
            Ok(own_host_names(factory)
                .into_iter()
                .find(|own| super::setup::is_reserved(&name, std::slice::from_ref(own)))
                .map(|own| format!("the server's host name {own}")))
        }
        ClaimKind::MailAddress => own_mail_address(facade, key),
        ClaimKind::ChatBridge => Ok(facade
            .find(BRIDGE)?
            .eq("chatBridgeId", Value::string(key))
            .select_field("chatBridgeId")
            .disable_authz()
            .one()?
            .map(|_| format!("the server's ChatBridge {key}"))),
        ClaimKind::XmppDomain => {
            let own = domain_property("xmpp_domain");
            let name = key.trim_end_matches('.').to_ascii_lowercase();
            Ok(super::setup::is_reserved(&name, std::slice::from_ref(&own))
                .then(|| format!("the server's XMPP domain {own}")))
        }
        ClaimKind::PhoneNumber => Ok(facade
            .find(PHONE_NUMBER)?
            .eq("phoneNumber", Value::string(key))
            .select_field("phoneNumber")
            .disable_authz()
            .one()?
            .map(|_| format!("the server's PhoneNumber {key}"))),
        ClaimKind::SipDomain => {
            let own = domain_property("sip_domain");
            let name = key.trim_end_matches('.').to_ascii_lowercase();
            Ok((!own.is_empty() && name == own).then(|| format!("the server's SIP domain {own}")))
        }
    }
}

/// The server's account that receives mail at `address`: a hosted or shared account not removed and with no
/// incoming (IMAP or POP) server, by its address or one of its identities', in any case.
fn own_mail_address(facade: &EntityFacade, address: &str) -> Result<Option<String>, Error> {
    let receives = |row: &EntityValue| receives(facade, row);
    let address_is = || Condition::ignore_case(Condition::equals("emailAddress", Value::string(address)));
    let accounts = facade.find(ACCOUNT)?.condition(address_is()).disable_authz().list()?;
    for row in accounts.values() {
        if receives(row)? {
            return Ok(Some(format!(
                "the server's EmailAccount {}",
                text(row, "emailAccountId")
            )));
        }
    }
    let identities = facade
        .find(IDENTITY)?
        .condition(address_is())
        .select_field("emailIdentityId")
        .select_field("emailAccountId")
        .disable_authz()
        .list()?;
    for identity in identities.values() {
        let account_id = text(identity, "emailAccountId");
        if account_id.is_empty() {
            continue;
        }
        let account = facade
            .find(ACCOUNT)?
            .eq("emailAccountId", Value::string(&account_id))
            .disable_authz()
            .one()?;
        if let Some(account) = account {
            if receives(&account)? {
                return Ok(Some(format!(
                    "the server's EmailIdentity {} of EmailAccount {account_id}",
                    text(identity, "emailIdentityId")
                )));
            }
        }
    }
    Ok(None)
}

/// The paired node (its `pairedNodeId`) whose copies hold `key` of `kind`, or `None`. Read from what the server keeps
/// of each paired node while it is active (`EdgeClientActive`): a revoked node's rows are kept for a re-pair
/// (§5.2 "Unpair and revoke") but claim nothing until then.
/// - a host name: its `EdgeHost` rows (exact name), kept by a revoked node too, as the names stay reserved for it;
/// - a mail address: its replicated `mailRecipients` accounts and identities, in any case;
/// - a chat bridge: its replicated `chatBridges`;
/// - a phone number: its replicated `voiceFallbacks` (a number with nothing to fall back to has no copy);
/// - an XMPP or SIP domain: the `xmppDomain` or `sipDomain` of its last Configure (`EdgeClient.setupJson`).
///
/// # Errors
///
/// Returns the database's error.
pub fn replica_claim(facade: &EntityFacade, kind: ClaimKind, key: &str) -> Result<Option<String>, Error> {
    let key = key.trim();
    if key.is_empty() {
        return Ok(None);
    }
    match kind {
        ClaimKind::HostName => {
            let name = key.trim_end_matches('.').to_ascii_lowercase();
            Ok(facade
                .find(EDGE_HOST)?
                .eq("hostName", Value::string(&name))
                .select_field("pairedNodeId")
                .disable_authz()
                .one()?
                .map(|row| text(&row, "pairedNodeId")))
        }
        ClaimKind::MailAddress => {
            // the copies' fields are encrypted at rest, so the address cannot be a condition: each node's are read
            for node in active_nodes(facade)? {
                for entity in [ACCOUNT, IDENTITY] {
                    let copies = super::replica::list_values(facade, Some(&node), MAIL_SET, Some(entity), SCAN_LIMIT)?;
                    if copies.len() >= SCAN_LIMIT {
                        tracing::error!(
                            "paired node {node}: more than {SCAN_LIMIT} {entity} copies in {MAIL_SET}; whether one \
                             past them holds mail address {key} is not checked"
                        );
                    }
                    let held = copies.iter().any(|copy| {
                        copy.fields["emailAddress"]
                            .as_str()
                            .is_some_and(|held| held.trim().eq_ignore_ascii_case(key))
                    });
                    if held {
                        return Ok(Some(node));
                    }
                }
            }
            Ok(None)
        }
        ClaimKind::ChatBridge => keyed_copy(facade, CHAT_SET, BRIDGE, "chatBridgeId", key),
        ClaimKind::PhoneNumber => keyed_copy(facade, FALLBACKS_SET, FALLBACK, "phoneNumber", key),
        ClaimKind::XmppDomain => setup_claim(facade, "xmppDomain", key),
        ClaimKind::SipDomain => setup_claim(facade, "sipDomain", key),
    }
}

/// The paired nodes that are active (`EdgeClientActive`), in id order.
fn active_nodes(facade: &EntityFacade) -> Result<Vec<String>, Error> {
    Ok(facade
        .find(EDGE_CLIENT)?
        .eq("statusId", Value::string("EdgeClientActive"))
        .select_field("pairedNodeId")
        .order_by("pairedNodeId", false)
        .disable_authz()
        .list()?
        .values()
        .iter()
        .map(|row| text(row, "pairedNodeId"))
        .collect())
}

/// The active node whose copy in `set` of `entity`'s record with the one key field `field` = `key` exists.
fn keyed_copy(facade: &EntityFacade, set: &str, entity: &str, field: &str, key: &str) -> Result<Option<String>, Error> {
    let pk = BTreeMap::from([(field.to_string(), key.to_string())]);
    for node in active_nodes(facade)? {
        if super::replica::get_value(facade, Some(&node), set, entity, &pk)?.is_some() {
            return Ok(Some(node));
        }
    }
    Ok(None)
}

/// The active paired node whose last Configure set `field` (a domain) to `key`, in any case.
fn setup_claim(facade: &EntityFacade, field: &str, key: &str) -> Result<Option<String>, Error> {
    let name = key.trim_end_matches('.').to_ascii_lowercase();
    let clients = facade
        .find(EDGE_CLIENT)?
        .eq("statusId", Value::string("EdgeClientActive"))
        .select_field("pairedNodeId")
        .select_field("setupJson")
        .disable_authz()
        .list()?;
    for client in clients.values() {
        let setup: serde_json::Value = serde_json::from_str(&text(client, "setupJson")).unwrap_or_default();
        let held = setup[field]
            .as_str()
            .map(|held| held.trim().trim_end_matches('.').to_ascii_lowercase());
        if held.is_some_and(|held| !held.is_empty() && held == name) {
            return Ok(Some(text(client, "pairedNodeId")));
        }
    }
    Ok(None)
}

#[cfg(test)]
#[path = "claims_tests.rs"]
mod tests;
