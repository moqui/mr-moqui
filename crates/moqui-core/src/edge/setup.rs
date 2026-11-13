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

//! Attaching a local node to its edge node (plans/archive/Edge-Node.md §5.7, E10): what the edge node offers ([`EdgeOffer`]),
//! the local node's part it sends back on the link's Configure ([`EdgeSetup`], checked on the edge node by
//! [`check_setup`]), the local rows and settings the wizard's Apply step writes ([`apply_local`]), and the Test step's
//! round trips over the queue ([`start_echo`], [`receive_echo`], [`wait_echo`]). The host's link carries the offer and
//! Configure; the rest needs no network.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;

use super::queue::{self, NewItem};
use super::{EdgeHostInfo, HOST_PASSTHROUGH, HOST_TERMINATE};
use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::EntityFacade;
use crate::transaction::TransactionAttribute;

/// The queue kind of the Test step's round trips: the local node queues one to the edge node, which checks what the
/// payload asks and queues the answer back.
pub const ECHO_KIND: &str = "edgeEcho";
/// The `EmailServer` Apply makes for the accounts that send through the edge node.
pub const EDGE_SERVER_ID: &str = "EDGE";
/// The `EmailServer` the framework's own mail (the email templates) goes through.
pub const SYSTEM_SERVER_ID: &str = "SYSTEM";
/// The `smtpHost` of a server whose mail the local node queues for its edge node (plans/archive/Edge-Node.md §5.4, E5a).
pub const EDGE_SMTP_HOST: &str = "edge:";
/// The local node's address for chat providers and MMS files, which on a paired node is the edge node's (E6).
pub const CHAT_PUBLIC_URL: &str = "channels_public_url";
/// The most names of each kind a Configure carries.
pub const MAX_NAMES: usize = 64;
/// The areas an edge node does not offer yet: none since phones have their area ([`SipOffer`]), as XMPP and calls
/// have ([`XmppOffer`], [`CallsOffer`]). An area added later and not built yet is listed here.
pub const NOT_AVAILABLE: [&str; 0] = [];

const SERVER: &str = "moqui.basic.email.EmailServer";
const ACCOUNT: &str = "moqui.basic.email.EmailAccount";
/// The field of an `EmailServer` that keeps the `smtpHost` Apply replaced, and of an `EmailAccount` the server it
/// named, for [`restore_local`] at unpairing.
const PRIOR_HOST: &str = "edgePriorSmtpHost";
const PRIOR_SERVER: &str = "edgePriorServerId";
/// What a prior field holds for "there was none".
const PRIOR_NONE: &str = "none:";

/// The edge node's mail (E5a, E5b).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MailOffer {
    pub available: bool,
    /// `moqui` or `postfix`.
    pub mta: String,
    pub listener: bool,
    pub mx_host: String,
    pub bounce_domain: String,
    pub dkim_selector: String,
    /// The TXT record of the edge node's DKIM key; empty when Postfix and Rspamd sign.
    pub dkim_record: String,
}

/// The edge node's chat webhooks (E6).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChatOffer {
    pub webhooks: bool,
    pub public_url: String,
}

/// mr-pass on the edge node (E4).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PassOffer {
    pub available: bool,
    /// The client id the local node registers the edge node as.
    pub client_id: String,
}

/// The edge node's XMPP tunnel (E9): its ports that pipe clients down the link to this node's XMPP server.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct XmppOffer {
    /// A tunnel port is on.
    pub available: bool,
    /// `edge_xmpp_port` (STARTTLS); 0 while off.
    pub port: u16,
    /// `edge_xmpp_tls_port` (direct TLS); 0 while off.
    pub tls_port: u16,
    /// `edge_xmpp_node` names another local node, so the tunnels do not come here.
    pub other_node: bool,
    /// `edge_xmpp_node` is empty and this node is not the only one linked, so the tunnels go to no node until it
    /// names one.
    pub needs_node: bool,
}

/// The edge node as media node of the local node's calls, with its TURN (E7): what its media node advertises
/// ([`super::media::advert`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CallsOffer {
    /// Its media socket is up.
    pub available: bool,
    /// The address browsers send the media of its calls to.
    pub media_address: String,
    /// Its embedded TURN listeners as `turn:` and `turns:` URLs; empty without TURN.
    pub turn_urls: Vec<String>,
    /// Other local nodes are linked too, so the edge node takes call messages from none of them (decision 1, E7).
    pub other_nodes: bool,
}

/// The edge node's SIP engine (E8): its addresses, its registrar, the trunks it loaded from the asking node's copy and
/// the fallbacks it holds for that node (plans/archive/Edge-Node.md §5.7, E10 phones).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SipOffer {
    /// Its SIP engine runs.
    pub available: bool,
    /// For UDP and TCP, and for TLS, `host:port`; empty when that listener is off.
    pub sip_address: String,
    pub tls_address: String,
    /// The realm its registrar challenges phones with (`sip_domain`), and whether the registrar is on.
    pub domain: String,
    pub registrar: bool,
    /// The trunks its engine has loaded now, with their state.
    pub trunks: Vec<super::media::SipTrunkState>,
    /// The asking node's numbers with a fallback in its copy of `voiceFallbacks`.
    pub fallbacks: u64,
    /// Other local nodes are active too, so its engine reads no node's copies (decision 1).
    pub other_nodes: bool,
}

/// What an edge node offers a local node.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EdgeOffer {
    /// The names the edge node answers for itself.
    pub public_names: Vec<String>,
    /// The modes a local node's host names may have.
    pub host_modes: Vec<String>,
    pub mail: MailOffer,
    pub chat: ChatOffer,
    pub pass: PassOffer,
    pub xmpp: XmppOffer,
    pub calls: CallsOffer,
    pub sip: SipOffer,
    /// Areas not built yet: none now.
    pub not_available: Vec<String>,
    /// The edge node's public addresses, when it names them (`edge_public_addresses`).
    pub addresses: Vec<String>,
    /// The asking node's host names on the edge node now.
    pub node_hosts: Vec<EdgeHostInfo>,
}

/// The local node's part of its setup, which the link's Configure sends.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EdgeSetup {
    /// Its host names and their modes; with `set_hosts`, the whole list.
    pub hosts: Vec<EdgeHostInfo>,
    pub set_hosts: bool,
    pub mail_domains: Vec<String>,
    pub chat_bridge_ids: Vec<String>,
    pub pass_sites: Vec<String>,
    /// The local node's XMPP domain (`xmpp_domain`), when its clients come through the edge node's tunnel ports;
    /// empty when not. Its `rooms.` and `upload.` names are the domain's and are not sent.
    pub xmpp_domain: String,
}

/// A host name of a Configure, and why the edge node did not take it (empty when it did).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostResult {
    pub host_name: String,
    pub mode_enum_id: String,
    pub error: String,
}

/// The edge node's answer to a Configure.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Configured {
    pub offer: EdgeOffer,
    pub hosts: Vec<HostResult>,
}

/// A DNS name's normal form: trimmed, lower case, with no trailing dot, or `None` when it is not one (letters,
/// digits, `-` and `.`, at most 253 bytes, at least one dot, no empty label).
pub fn normalize_domain(name: &str) -> Option<String> {
    let name = name.trim();
    let name = name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase();
    let valid = name.len() <= 253
        && name.contains('.')
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        });
    valid.then_some(name)
}

/// A host name a local node's setup may name: its normal form ([`normalize_domain`]), or why not. An IP address,
/// a name with one label, `localhost` and its subdomains, and a name whose last label is all digits are refused.
///
/// # Errors
///
/// Returns why the name is not one.
pub fn check_host_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    let bare = trimmed.trim_start_matches('[').trim_end_matches(']');
    if bare.parse::<std::net::IpAddr>().is_ok() {
        return Err("an IP address is not a host name".to_string());
    }
    let Some(name) = normalize_domain(trimmed) else {
        return Err("not a host name with a domain".to_string());
    };
    if name == "localhost" || name.ends_with(".localhost") {
        return Err("localhost is not a public name".to_string());
    }
    if name
        .rsplit('.')
        .next()
        .is_some_and(|label| label.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err("not a host name with a domain".to_string());
    }
    Ok(name)
}

/// Whether `name` is one of `reserved` or a subdomain of one.
pub fn is_reserved(name: &str, reserved: &[String]) -> bool {
    reserved.iter().any(|own| {
        let own = own.trim().trim_end_matches('.').to_ascii_lowercase();
        !own.is_empty() && (name == own || name.ends_with(&format!(".{own}")))
    })
}

/// Whether this local node is paired with an edge node now: `edge_node_url` is set, or this process's link
/// replicates (its dialer runs). Mail for `edge:` is queued only then.
pub fn is_paired(facade: &EntityFacade) -> bool {
    !moqui_lib::env::get_prop_or_env(super::NODE_URL).trim().is_empty() || super::replica::replica(facade).is_some()
}

/// Whether `id` can be a record id the setup names (a chat bridge, a PASS site, a PASS client): 1 to 63 bytes of
/// letters, digits, `_`, `-` and `.`.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 63
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

/// A Configure as the edge node takes it: everything from a local node is input from any client (§5.8). Names and
/// domains are normalized, lists capped at [`MAX_NAMES`], duplicates dropped. A bad domain (the XMPP domain by
/// [`check_host_name`]), id or mode is refused whole; a bad host name is kept, for its own error in the answer.
///
/// # Errors
///
/// Returns why the request is refused.
pub fn check_setup(setup: &EdgeSetup) -> Result<EdgeSetup, Error> {
    let too_many = |what: &str, count: usize| {
        (count > MAX_NAMES).then(|| Error::new(format!("{count} {what} is more than the {MAX_NAMES} a setup may name")))
    };
    for (what, count) in [
        ("host names", setup.hosts.len()),
        ("mail domains", setup.mail_domains.len()),
        ("chat bridges", setup.chat_bridge_ids.len()),
        ("PASS sites", setup.pass_sites.len()),
    ] {
        if let Some(err) = too_many(what, count) {
            return Err(err);
        }
    }
    let mut hosts: Vec<EdgeHostInfo> = Vec::new();
    for host in &setup.hosts {
        if host.mode_enum_id != HOST_PASSTHROUGH && host.mode_enum_id != HOST_TERMINATE {
            return Err(Error::new(format!(
                "the mode of a host name must be {HOST_PASSTHROUGH} or {HOST_TERMINATE}"
            )));
        }
        let name = host.host_name.trim().trim_end_matches('.').to_ascii_lowercase();
        if !hosts.iter().any(|kept| kept.host_name == name) {
            hosts.push(EdgeHostInfo {
                host_name: name,
                paired_node_id: String::new(),
                mode_enum_id: host.mode_enum_id.clone(),
            });
        }
    }
    let mut mail_domains: Vec<String> = Vec::new();
    for domain in &setup.mail_domains {
        let Some(domain) = normalize_domain(domain) else {
            return Err(Error::new(format!(
                "{} is not a mail domain",
                domain.chars().take(80).collect::<String>()
            )));
        };
        if !mail_domains.contains(&domain) {
            mail_domains.push(domain);
        }
    }
    let ids = |what: &str, list: &[String]| -> Result<Vec<String>, Error> {
        let mut out: Vec<String> = Vec::new();
        for id in list {
            let id = id.trim();
            if !valid_id(id) {
                return Err(Error::new(format!(
                    "{} is not a {what} id",
                    id.chars().take(80).collect::<String>()
                )));
            }
            if !out.iter().any(|kept| kept == id) {
                out.push(id.to_string());
            }
        }
        Ok(out)
    };
    let xmpp_domain = match setup.xmpp_domain.trim() {
        "" => String::new(),
        domain => check_host_name(domain).map_err(|why| {
            Error::new(format!(
                "{} is not an XMPP domain: {why}",
                domain.chars().take(80).collect::<String>()
            ))
        })?,
    };
    Ok(EdgeSetup {
        hosts,
        set_hosts: setup.set_hosts,
        mail_domains,
        chat_bridge_ids: ids("chat bridge", &setup.chat_bridge_ids)?,
        pass_sites: ids("PASS site", &setup.pass_sites)?,
        xmpp_domain,
    })
}

/// What the local node chose in the wizard, for [`apply_local`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalChoices {
    /// The edge node's address (`edge_node_url`), the chat address when the offer names none.
    pub edge_url: String,
    pub mail: Option<MailChoice>,
    /// The chat bridges whose webhooks go to the edge node; `None` leaves chat as it is.
    pub chat: Option<Vec<String>>,
    pub pass: Option<PassChoice>,
}

/// Mail through the edge node: `SYSTEM`'s and the chosen accounts' outgoing mail.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MailChoice {
    pub system: bool,
    pub account_ids: Vec<String>,
}

/// The sites the edge node serves as a PASS client, and its SSO callback.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PassChoice {
    pub sites: Vec<String>,
    pub callback: String,
}

/// One thing Apply did, or could not do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SetupLine {
    /// web, mail, chat, pass, xmpp, calls, sip or link.
    pub area: String,
    pub ok: bool,
    pub text: String,
}

impl SetupLine {
    pub fn new(area: &str, ok: bool, text: impl Into<String>) -> Self {
        Self {
            area: area.to_string(),
            ok,
            text: text.into(),
        }
    }
}

/// Write the local rows and settings of the chosen areas (plans/archive/Edge-Node.md §5.7, Apply), each on its own, so one
/// that fails is a line and the others still go:
/// - mail: an `EmailServer` [`EDGE_SERVER_ID`] with `smtpHost` `edge:`; `SYSTEM`'s `smtpHost` set to it; each chosen
///   hosted or shared account sends through it (its own server, when it owns one with no incoming store, gets
///   `edge:`; else the account names [`EDGE_SERVER_ID`]). An account that syncs from another provider is left.
/// - chat: [`CHAT_PUBLIC_URL`] in `env_file`, the edge node's address.
/// - PASS: `pass_edge_client` and `pass_client_<id>_sites` and `_callback` in `env_file`.
///
/// An area that is `None` is left as it is. `env_file` is the server's `runtime/.env`.
pub fn apply_local(
    facade: &EntityFacade,
    env_file: &std::path::Path,
    choices: &LocalChoices,
    offer: &EdgeOffer,
) -> Vec<SetupLine> {
    let mut lines = Vec::new();
    if let Some(mail) = &choices.mail {
        apply_mail(facade, mail, &mut lines);
    }
    if choices.chat.is_some() {
        let url = match offer.chat.public_url.trim() {
            "" => choices.edge_url.trim().trim_end_matches('/'),
            url => url.trim_end_matches('/'),
        };
        if url.starts_with("https://") || url.starts_with("http://") {
            lines.push(set_env(
                "chat",
                env_file,
                &[(CHAT_PUBLIC_URL, url)],
                &format!("Chat providers and MMS files use {url}"),
            ));
        } else {
            lines.push(SetupLine::new(
                "chat",
                false,
                "The edge node names no web address for webhooks, so channels_public_url is left",
            ));
        }
    }
    if let Some(pass) = &choices.pass {
        lines.push(apply_pass(env_file, pass, offer));
    }
    lines
}

/// The PASS choice as Apply writes it: the client id, the sites and the callback, or why it is not one.
///
/// # Errors
///
/// Returns why the choice is refused.
pub fn check_pass(pass: &PassChoice, offer: &EdgeOffer) -> Result<(String, Vec<String>, String), String> {
    let client = match offer.pass.client_id.trim() {
        "" => crate::pass_authority::DEFAULT_EDGE_CLIENT,
        id => id,
    };
    if !valid_id(client) || client.contains('.') {
        return Err(format!("The edge node's PASS client id {client} is not one"));
    }
    let Some(sites) = pass
        .sites
        .iter()
        .map(|site| site.trim())
        .filter(|site| !site.is_empty())
        .map(|site| valid_id(site).then(|| site.to_string()))
        .collect::<Option<Vec<String>>>()
    else {
        return Err("A PASS site id is not one".to_string());
    };
    if sites.is_empty() {
        return Err("Choose the PASS sites the edge node serves".to_string());
    }
    let callback = pass.callback.trim();
    let callback_ok = (callback.starts_with("https://") || callback.starts_with("http://"))
        && callback.split_once("//").is_some_and(|(_, rest)| rest.contains('/'))
        && !callback.contains(['#', '\r', '\n', ' ']);
    if !callback_ok {
        return Err(
            "The SSO callback must be the frontend's https address with its path, such as \
             https://forum.example.com/account/Sso"
                .to_string(),
        );
    }
    Ok((client.to_string(), sites, callback.to_string()))
}

fn apply_pass(env_file: &std::path::Path, pass: &PassChoice, offer: &EdgeOffer) -> SetupLine {
    let (client, sites, callback) = match check_pass(pass, offer) {
        Ok(checked) => checked,
        Err(why) => return SetupLine::new("pass", false, why),
    };
    let (client, callback) = (client.as_str(), callback.as_str());
    let sites_name = format!("pass_client_{client}_sites");
    let callback_name = format!("pass_client_{client}_callback");
    let sites_text = sites.join(",");
    set_env(
        "pass",
        env_file,
        &[
            (crate::pass_authority::EDGE_CLIENT_PROPERTY, client),
            (sites_name.as_str(), sites_text.as_str()),
            (callback_name.as_str(), callback),
        ],
        &format!("The edge node is PASS client {client} for {sites_text}"),
    )
}

/// Write `set` to the dotenv file; the line says so, or why not. A process property of the same name wins over the
/// file, so a value that does not read back is named.
pub(crate) fn set_env(area: &str, env_file: &std::path::Path, set: &[(&str, &str)], done: &str) -> SetupLine {
    let pairs: Vec<(String, String)> = set
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect();
    if let Err(err) = moqui_lib::env::update_dotenv_file(env_file, &pairs, &[]) {
        tracing::error!("Edge setup: {area} settings were not written: {err}");
        return SetupLine::new(area, false, format!("The settings were not written: {err}"));
    }
    let overridden: Vec<&str> = set
        .iter()
        .filter(|(name, value)| moqui_lib::env::get_prop_or_env(name).trim() != value.trim())
        .map(|(name, _)| *name)
        .collect();
    if overridden.is_empty() {
        tracing::info!("Edge setup: {done}");
        SetupLine::new(area, true, done)
    } else {
        SetupLine::new(
            area,
            false,
            format!(
                "Written to runtime/.env, but a property set at start overrides {}: remove it there",
                overridden.join(", ")
            ),
        )
    }
}

fn text(row: &crate::entity::EntityValue, field: &str) -> String {
    row.get(field).map(to_plain_string).unwrap_or_default()
}

/// Run `body` in a transaction of its own with authorization off: one area's writes, apart from the others.
fn own_transaction<T>(facade: &EntityFacade, body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    facade
        .transactions_for(SERVER)?
        .run(TransactionAttribute::ForceNew, Some(Duration::from_secs(30)), body)
}

/// Set a server's `smtpHost` to `edge:`, making the row when it is not there, and keep the host it replaces in
/// `edgePriorSmtpHost` for unpairing (not again once it is `edge:`); the old host, when it had one.
fn point_server(facade: &EntityFacade, server_id: &str) -> Result<String, Error> {
    let existing = facade
        .find(SERVER)?
        .eq("emailServerId", Value::string(server_id))
        .disable_authz()
        .use_cache(false)
        .one()?;
    let old = existing.as_ref().map(|row| text(row, "smtpHost")).unwrap_or_default();
    let mut row = facade.make_value(SERVER)?;
    row.set("emailServerId", Value::string(server_id));
    row.set("smtpHost", Value::string(EDGE_SMTP_HOST));
    if existing.is_some() && old != EDGE_SMTP_HOST {
        row.set(
            PRIOR_HOST,
            Value::string(if old.is_empty() { PRIOR_NONE } else { old.as_str() }),
        );
    }
    if existing.is_some() {
        row.update(facade)?;
    } else {
        row.create(facade)?;
    }
    Ok(old)
}

fn apply_mail(facade: &EntityFacade, mail: &MailChoice, lines: &mut Vec<SetupLine>) {
    match own_transaction(facade, || point_server(facade, EDGE_SERVER_ID)) {
        Ok(_) => {}
        Err(err) => {
            tracing::error!("Edge setup: the {EDGE_SERVER_ID} mail server was not saved: {err}");
            lines.push(SetupLine::new(
                "mail",
                false,
                format!("The mail server for the edge node was not saved: {err}"),
            ));
            return;
        }
    }
    if mail.system {
        lines.push(
            match own_transaction(facade, || point_server(facade, SYSTEM_SERVER_ID)) {
                Ok(old) if old.is_empty() || old == EDGE_SMTP_HOST => {
                    SetupLine::new("mail", true, "The system's mail goes through the edge node")
                }
                Ok(old) => SetupLine::new(
                    "mail",
                    true,
                    format!("The system's mail goes through the edge node, not {old}"),
                ),
                Err(err) => {
                    tracing::error!("Edge setup: the SYSTEM mail server was not changed: {err}");
                    SetupLine::new(
                        "mail",
                        false,
                        format!("The system's mail server was not changed: {err}"),
                    )
                }
            },
        );
    }
    for account_id in &mail.account_ids {
        let line = match own_transaction(facade, || point_account(facade, account_id)) {
            Ok(line) => line,
            Err(err) => {
                tracing::error!("Edge setup: email account {account_id} was not changed: {err}");
                SetupLine::new("mail", false, format!("Account {account_id} was not changed: {err}"))
            }
        };
        lines.push(line);
    }
}

/// One account's outgoing mail through the edge node.
fn point_account(facade: &EntityFacade, account_id: &str) -> Result<SetupLine, Error> {
    let Some(account) = facade
        .find(ACCOUNT)?
        .eq("emailAccountId", Value::string(account_id))
        .disable_authz()
        .one()?
    else {
        return Ok(SetupLine::new(
            "mail",
            false,
            format!("There is no account {account_id}"),
        ));
    };
    let address = text(&account, "emailAddress");
    let kind = text(&account, "accountKindEnumId");
    if kind != "EakHosted" && kind != "EakShared" {
        return Ok(SetupLine::new(
            "mail",
            false,
            format!("{address} is not hosted here, so it sends through its own provider"),
        ));
    }
    let server_id = text(&account, "emailServerId");
    let server = if server_id.is_empty() {
        None
    } else {
        facade
            .find(SERVER)?
            .eq("emailServerId", Value::string(&server_id))
            .disable_authz()
            .use_cache(false)
            .one()?
    };
    if let Some(server) = &server {
        let store = text(server, "storeHost");
        if !store.is_empty() {
            return Ok(SetupLine::new(
                "mail",
                false,
                format!("{address} syncs from {store}, so it sends through that provider"),
            ));
        }
    }
    let owns =
        matches!(account.get("ownsEmailServer"), Some(Value::Bool(true))) || text(&account, "ownsEmailServer") == "Y";
    if owns && server.is_some() {
        point_server(facade, &server_id)?;
    } else if server_id != EDGE_SERVER_ID {
        let mut update = facade.make_value(ACCOUNT)?;
        update.set("emailAccountId", Value::string(account_id));
        update.set("emailServerId", Value::string(EDGE_SERVER_ID));
        update.set(
            PRIOR_SERVER,
            Value::string(if server_id.is_empty() {
                PRIOR_NONE
            } else {
                server_id.as_str()
            }),
        );
        update.set("ownsEmailServer", Value::string("N"));
        update.update(facade)?;
    }
    Ok(SetupLine::new(
        "mail",
        true,
        format!("{address} sends through the edge node"),
    ))
}

/// Undo Apply's mail rows at unpairing, so mail does not queue for an edge node that is gone: each server Apply set
/// to `edge:` gets its prior `smtpHost` back, and each account Apply pointed at [`EDGE_SERVER_ID`] its prior server.
/// Each row on its own; one that fails is logged and named, and the others go on.
pub fn restore_local(facade: &EntityFacade) -> Vec<SetupLine> {
    let mut lines = Vec::new();
    let found = own_transaction(facade, || {
        let servers = facade
            .find(SERVER)?
            .eq("smtpHost", Value::string(EDGE_SMTP_HOST))
            .disable_authz()
            .use_cache(false)
            .list()?;
        let accounts = facade
            .find(ACCOUNT)?
            .condition(crate::entity::Condition::is_not_null(PRIOR_SERVER))
            .disable_authz()
            .list()?;
        let rows = |list: &[crate::entity::EntityValue], key: &str, prior: &str| -> Vec<(String, String)> {
            list.iter()
                .map(|row| (text(row, key), text(row, prior)))
                .filter(|(_, prior)| !prior.is_empty())
                .collect()
        };
        Ok((
            rows(servers.values(), "emailServerId", PRIOR_HOST),
            rows(accounts.values(), "emailAccountId", PRIOR_SERVER),
        ))
    });
    let (servers, accounts) = match found {
        Ok(found) => found,
        Err(err) => {
            tracing::error!("Edge unpairing: the mail servers to restore were not read: {err}");
            return vec![SetupLine::new(
                "mail",
                false,
                format!("The mail servers were not restored: {err}"),
            )];
        }
    };
    let prior_value = |prior: &str| {
        if prior == PRIOR_NONE {
            Value::Null
        } else {
            Value::string(prior)
        }
    };
    for (server_id, prior) in servers {
        let restored = own_transaction(facade, || {
            let mut row = facade.make_value(SERVER)?;
            row.set("emailServerId", Value::string(&server_id));
            row.set("smtpHost", prior_value(&prior));
            row.set(PRIOR_HOST, Value::Null);
            row.update(facade)
        });
        lines.push(match restored {
            Ok(()) => SetupLine::new(
                "mail",
                true,
                format!("Mail server {server_id} sends as before the edge node"),
            ),
            Err(err) => {
                tracing::error!("Edge unpairing: mail server {server_id} was not restored: {err}");
                SetupLine::new(
                    "mail",
                    false,
                    format!("Mail server {server_id} was not restored: {err}"),
                )
            }
        });
    }
    for (account_id, prior) in accounts {
        let restored = own_transaction(facade, || {
            let mut row = facade.make_value(ACCOUNT)?;
            row.set("emailAccountId", Value::string(&account_id));
            row.set("emailServerId", prior_value(&prior));
            row.set(PRIOR_SERVER, Value::Null);
            row.update(facade)
        });
        lines.push(match restored {
            Ok(()) => SetupLine::new(
                "mail",
                true,
                format!("Account {account_id} uses its server from before"),
            ),
            Err(err) => {
                tracing::error!("Edge unpairing: email account {account_id} was not restored: {err}");
                SetupLine::new("mail", false, format!("Account {account_id} was not restored: {err}"))
            }
        });
    }
    lines
}

/// What a Test step round trip asks the edge node to check.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EchoCheck {
    /// mailIn (an address the edge node takes mail for), mailOut (the edge node takes this node's mail), webhook (the
    /// edge node takes the bridges' webhooks) or link (the queue alone).
    pub check: String,
    pub address: String,
    pub bridge_ids: Vec<String>,
}

/// The edge node's answer to a round trip.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EchoAnswer {
    pub ok: bool,
    pub message: String,
}

/// The answers that came back, by token, with when they came: read once by [`wait_echo`], and dropped after
/// 10 minutes when no one waits for them.
static ECHOES: Mutex<BTreeMap<String, (Instant, EchoAnswer)>> = Mutex::new(BTreeMap::new());

const ECHO_KEPT: Duration = Duration::from_secs(600);

/// Queue a round trip to the edge node, committed at once so it goes now: its token, which [`wait_echo`] waits for.
///
/// # Errors
///
/// Returns why it was not queued (no `edgeEcho` kind in this node's conf, the database).
pub fn start_echo(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    check: &EchoCheck,
) -> Result<String, Error> {
    let token = crate::entity::uuid_v4()?;
    let payload = serde_json::json!({
        "token": token,
        "check": check.check,
        "address": check.address,
        "bridgeIds": check.bridge_ids,
    });
    let item = NewItem {
        kind: ECHO_KIND.to_string(),
        conversation_key: format!("{ECHO_KIND}/{token}"),
        idempotency_key: token.clone(),
        payload: Some(payload.to_string()),
        content: None,
        content_location: None,
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    facade.transactions_for(queue::QUEUE_ITEM)?.run(
        TransactionAttribute::ForceNew,
        Some(Duration::from_secs(30)),
        || queue::enqueue(factory, facade, None, &item),
    )?;
    Ok(token)
}

/// Wait until `deadline` for the answer to the round trip of `token`.
pub fn wait_echo(token: &str, deadline: Instant) -> Option<EchoAnswer> {
    loop {
        if let Some((_, answer)) = lock(&ECHOES).remove(token) {
            return Some(answer);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The handler of the `edgeEcho` queue kind (`org.moqui.edge.EdgeServices.receive#EdgeEcho`). On an edge node it
/// makes the check the payload asks of the node's own copies and settings and queues the answer back, in the
/// transaction of the receipt; on a local node it keeps the answer for [`wait_echo`].
///
/// # Errors
///
/// Returns why the item was not taken (a payload that is not one).
pub fn receive_echo(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    node: &str,
    payload: &str,
) -> Result<(), Error> {
    let payload: serde_json::Value =
        serde_json::from_str(payload).map_err(|err| Error::new(format!("an edgeEcho payload is not JSON: {err}")))?;
    let token = payload["token"].as_str().unwrap_or_default().to_string();
    if token.is_empty() || token.len() > 64 {
        return Err(Error::new("an edgeEcho payload has no token"));
    }
    if !super::settings(factory).takes_links() {
        if payload["back"].as_bool() != Some(true) {
            return Err(Error::new(
                "a local node takes only the edge node's answers to its round trips",
            ));
        }
        let answer = EchoAnswer {
            ok: payload["ok"].as_bool() == Some(true),
            message: payload["message"]
                .as_str()
                .unwrap_or_default()
                .chars()
                .take(500)
                .collect(),
        };
        let mut echoes = lock(&ECHOES);
        echoes.retain(|_, (came, _)| came.elapsed() < ECHO_KEPT);
        echoes.insert(token, (Instant::now(), answer));
        return Ok(());
    }
    let check = EchoCheck {
        check: payload["check"].as_str().unwrap_or_default().to_string(),
        address: payload["address"].as_str().unwrap_or_default().to_string(),
        bridge_ids: payload["bridgeIds"]
            .as_array()
            .map(|ids| {
                ids.iter()
                    .filter_map(serde_json::Value::as_str)
                    .take(MAX_NAMES)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    };
    let answer = if echo_allowed(node) {
        edge_check(facade, node, &check)?
    } else {
        EchoAnswer {
            ok: false,
            message: "Too many checks from this node: wait a minute and test again".to_string(),
        }
    };
    let back = serde_json::json!({"token": token, "back": true, "ok": answer.ok, "message": answer.message});
    let item = NewItem {
        kind: ECHO_KIND.to_string(),
        conversation_key: format!("{ECHO_KIND}/{token}"),
        idempotency_key: format!("{token}/back"),
        payload: Some(back.to_string()),
        content: None,
        content_location: None,
    };
    queue::enqueue(factory, facade, Some(node), &item)?;
    Ok(())
}

/// The checks one node may ask in a minute: each mail-in check reads the node's replicated recipients.
const ECHOES_PER_MINUTE: u32 = 20;

/// Whether `node` may ask another check now (at most [`ECHOES_PER_MINUTE`] a minute).
fn echo_allowed(node: &str) -> bool {
    static ASKED: Mutex<BTreeMap<String, (Instant, u32)>> = Mutex::new(BTreeMap::new());
    let mut asked = lock(&ASKED);
    asked.retain(|_, (since, _)| since.elapsed() < Duration::from_secs(60));
    let entry = asked.entry(node.to_string()).or_insert((Instant::now(), 0));
    entry.1 += 1;
    entry.1 <= ECHOES_PER_MINUTE
}

/// Whether this edge node can send its local nodes' mail: with the MTA in moqui its DKIM key is there; with Postfix
/// beside it the relay port answers.
fn mta_ready() -> (bool, String) {
    let property = |name: &str| moqui_lib::env::get_prop_or_env(name).trim().to_string();
    if property("edge_mail_mta") == "moqui" {
        let key = property("mail_dkim_key");
        if key.is_empty() || key == "CHANGEME" {
            return (
                false,
                "The edge node has no DKIM key yet (mail_dkim_key): read its offer again, which makes one".to_string(),
            );
        }
        return (
            true,
            "The edge node takes this node's mail and sends it, DKIM-signed, to each domain's MX".to_string(),
        );
    }
    let host = match property("edge_mail_relay_host") {
        host if host.is_empty() => "127.0.0.1".to_string(),
        host => host,
    };
    let port = property("edge_mail_relay_port").parse::<u16>().unwrap_or(10025);
    let reached = format!("{host}:{port}")
        .parse::<std::net::SocketAddr>()
        .ok()
        .is_some_and(|address| std::net::TcpStream::connect_timeout(&address, Duration::from_secs(2)).is_ok());
    if reached {
        (
            true,
            format!("The edge node takes this node's mail and relays it to Postfix on {host}:{port}"),
        )
    } else {
        (
            false,
            format!("The edge node's Postfix relay on {host}:{port} does not answer"),
        )
    }
}

fn property_on(name: &str) -> bool {
    matches!(
        moqui_lib::env::get_prop_or_env(name)
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "true" | "y" | "yes" | "1"
    )
}

/// The edge node's side of a round trip, from its copies of `node`'s sets and its own settings.
fn edge_check(facade: &EntityFacade, node: &str, check: &EchoCheck) -> Result<EchoAnswer, Error> {
    let answer = |ok: bool, message: String| Ok(EchoAnswer { ok, message });
    match check.check.as_str() {
        "link" => answer(true, "The edge node took the item and answered".to_string()),
        "mailIn" => {
            if moqui_lib::env::get_prop_or_env("smtp_listen_port").trim().is_empty() {
                return answer(
                    false,
                    "The edge node's SMTP listener is off: set smtp_listen_port there".to_string(),
                );
            }
            let address = check.address.trim().to_ascii_lowercase();
            if address.is_empty() {
                return answer(false, "No address to check".to_string());
            }
            let known = ["moqui.basic.email.EmailAccount", "moqui.basic.email.EmailIdentity"]
                .iter()
                .map(|entity| super::replica::list_values(facade, Some(node), "mailRecipients", Some(entity), 100_000))
                .collect::<Result<Vec<_>, Error>>()?
                .iter()
                .flatten()
                .any(|value| {
                    value.fields["emailAddress"]
                        .as_str()
                        .is_some_and(|known| known.trim().eq_ignore_ascii_case(&address))
                });
            if known {
                answer(true, format!("The edge node takes mail for {address}"))
            } else {
                answer(
                    false,
                    format!("{address} is not in the edge node's copy of this node's accounts yet"),
                )
            }
        }
        "mailOut" => {
            let (ok, message) = mta_ready();
            answer(ok, message)
        }
        "webhook" => {
            if !property_on("channels_webhooks_enabled") {
                return answer(
                    false,
                    "The edge node takes no webhooks: set channels_webhooks_enabled there".to_string(),
                );
            }
            let copies = super::replica::list_values(facade, Some(node), "chatBridges", None, 10_000)?;
            let missing: Vec<&str> = check
                .bridge_ids
                .iter()
                .filter(|id| {
                    !copies
                        .iter()
                        .any(|value| value.pk["chatBridgeId"].as_str() == Some(id.as_str()))
                })
                .map(String::as_str)
                .collect();
            if missing.is_empty() {
                answer(true, "The edge node takes the bridges' webhooks".to_string())
            } else {
                answer(
                    false,
                    format!("The edge node has no copy of bridge {} yet", missing.join(", ")),
                )
            }
        }
        other => answer(
            false,
            format!("{} is not a check", other.chars().take(40).collect::<String>()),
        ),
    }
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
