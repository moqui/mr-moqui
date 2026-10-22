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

//! `discover#EmailServer`: the incoming and outgoing servers for an address, for the MailAccounts wizard.
//!
//! In order, each within a few seconds so the wizard never waits long:
//!
//! 1. Thunderbird autoconfig: `https://autoconfig.<domain>/mail/config-v1.1.xml`, the domain's
//!    `/.well-known/autoconfig/mail/config-v1.1.xml`, then the ISPDB (`https://autoconfig.thunderbird.net/v1.1/<domain>`),
//!    fetched at once; the first of them in that order that answers wins. `email-client` `autoconfig-urls` replaces the list
//!    (templates with `{domain}` and `{email}`), for a deployment with its own database and for tests.
//! 2. DNS SRV records (RFC 6186): `_imaps._tcp`, `_imap._tcp`, `_pop3s._tcp`, `_submissions._tcp`, `_submission._tcp`,
//!    asked of the first nameserver in `/etc/resolv.conf` (or `email-client` `dns-server`) over UDP; the framework has no
//!    resolver library, so this is a small query of its own.
//! 3. Guesses: `imap.<domain>` on 993 and `smtp.<domain>` on 587 with STARTTLS, for the wizard's test step to try.
//!
//! It also says which OAuth provider (`EmailOAuthProvider`, [`super::oauth`]) the mail is at, and whether this server
//! is set up to sign in with it: by the address's domain (gmail.com, outlook.com and the like), by the servers found
//! (imap.gmail.com, outlook.office365.com), else by the domain's MX hosts (a company domain at Google Workspace or
//! Microsoft 365: `*.google.com`, `*.protection.outlook.com`), asked with the SRV records. For such a domain the
//! provider's servers replace the guesses.
//!
//! The domain's own URLs are fetched only to public addresses (an address cannot point the server at its own network).

use std::collections::BTreeMap;
use std::net::UdpSocket;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use moqui_lib::error::Error as LibError;
use moqui_lib::node::MNode;
use moqui_lib::object::{to_plain_string, Value};

use super::implementation::note_error;
use crate::entity::EntityFacade;
use crate::service::util::param_text;

const HTTP_BUDGET: Duration = Duration::from_millis(3500);
const DNS_BUDGET: Duration = Duration::from_millis(1500);

const DEFAULT_AUTOCONFIG: &[&str] = &[
    "https://autoconfig.{domain}/mail/config-v1.1.xml?emailaddress={email}",
    "https://{domain}/.well-known/autoconfig/mail/config-v1.1.xml?emailaddress={email}",
    "https://autoconfig.thunderbird.net/v1.1/{domain}",
];

/// One server of a suggestion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Choice {
    /// imap, pop3 or smtp.
    pub(super) kind: String,
    pub(super) host: String,
    pub(super) port: u16,
    /// SSL (TLS from the start), STARTTLS or plain.
    pub(super) socket: String,
    pub(super) username: String,
    /// password, OAuth2, or what the config said.
    pub(super) auth: String,
}

impl Choice {
    fn to_map(&self) -> Value {
        let mut map = IndexMap::new();
        map.insert("kind".to_string(), Value::string(&self.kind));
        map.insert("host".to_string(), Value::string(&self.host));
        map.insert("port".to_string(), Value::I64(i64::from(self.port)));
        map.insert("socketType".to_string(), Value::string(&self.socket));
        map.insert("username".to_string(), Value::string(&self.username));
        map.insert("authentication".to_string(), Value::string(&self.auth));
        Value::Map(map)
    }

    /// Lower is better: IMAP before POP3, TLS before STARTTLS before plain, a password before OAuth only.
    fn rank(&self) -> u32 {
        let kind = match self.kind.as_str() {
            "imap" | "smtp" => 0,
            _ => 10,
        };
        let socket = match self.socket.as_str() {
            "SSL" => 0,
            "STARTTLS" => 1,
            _ => 5,
        };
        let auth = if self.auth.eq_ignore_ascii_case("OAuth2") { 3 } else { 0 };
        kind + socket + auth
    }
}

#[derive(Debug, Default)]
pub(super) struct Suggestion {
    pub(super) source: String,
    pub(super) display_name: String,
    pub(super) incoming: Vec<Choice>,
    pub(super) outgoing: Vec<Choice>,
}

/// `discover#EmailServer`.
pub(super) fn discover_server(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    let address = param_text(parameters, "emailAddress").trim().to_string();
    let Some((local, domain)) = address.rsplit_once('@') else {
        note_error(format!("{address} is not an email address"));
        return Ok(out);
    };
    let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    if local.is_empty() || !valid_domain(&domain) {
        note_error(format!("{address} is not an email address"));
        return Ok(out);
    }
    let providers = {
        let _authz = crate::context::ExecutionContext::with_current(crate::context::ExecutionContext::disable_authz);
        super::oauth::load_providers(facade).unwrap_or_else(|err| {
            tracing::error!("discover#EmailServer: the OAuth providers could not be read: {err}");
            Vec::new()
        })
    };
    let by_domain = super::oauth::detect(&providers, &domain, &[], &[]);
    let offline = facade.sim_active()
        || crate::entity::fence::refuses(crate::entity::fence::Effect::RemoteCall, "discover#EmailServer");
    let mut suggestion = if offline {
        None
    } else {
        autoconfig(&address, local, &domain)
    };
    let mut mx = Vec::new();
    let want_mx = by_domain.is_none() && !providers.is_empty();
    let mut asked_mx = false;
    if suggestion.is_none() {
        let (found, hosts) = dns_lookup(&domain, &address, true, want_mx);
        suggestion = found;
        mx = hosts;
        asked_mx = want_mx;
    }
    let mut suggestion = suggestion.unwrap_or_else(|| guess(&domain, &address));
    let hosts: Vec<String> = suggestion
        .incoming
        .iter()
        .chain(&suggestion.outgoing)
        .map(|choice| choice.host.clone())
        .collect();
    let mut provider = by_domain.or_else(|| super::oauth::detect(&providers, &domain, &hosts, &[]));
    if provider.is_none() && want_mx && !asked_mx {
        // a company domain with servers of its own name may still be at Google or Microsoft
        mx = mx_lookup(&domain);
    }
    if provider.is_none() && !mx.is_empty() {
        provider = super::oauth::detect(&providers, &domain, &[], &mx);
    }
    if let Some(found) = provider.as_ref().filter(|found| !found.store_host.is_empty()) {
        // a company domain at the provider has no servers of its own: the provider's, not guesses
        if suggestion.source == "guess" {
            suggestion = provider_suggestion(found, &address);
        }
    }
    suggestion.incoming.sort_by_key(Choice::rank);
    suggestion.outgoing.sort_by_key(Choice::rank);
    out.insert(
        "oauthProviderEnumId".into(),
        provider.as_ref().map_or(Value::Null, |found| Value::string(&found.id)),
    );
    out.insert(
        "oauthProviderName".into(),
        provider
            .as_ref()
            .map_or(Value::Null, |found| Value::string(&found.name)),
    );
    out.insert(
        "oauthConfigured".into(),
        Value::Bool(provider.as_ref().is_some_and(super::oauth::Provider::configured)),
    );
    out.insert("source".into(), Value::string(&suggestion.source));
    out.insert("displayName".into(), Value::string(&suggestion.display_name));
    if let Some(incoming) = suggestion.incoming.first() {
        let protocol = match (incoming.kind.as_str(), incoming.socket.as_str()) {
            ("pop3", "SSL") => "pop3s",
            ("pop3", _) => "pop3",
            (_, "SSL") => "imaps",
            _ => "imap",
        };
        out.insert("storeHost".into(), Value::string(&incoming.host));
        out.insert("storePort".into(), Value::string(incoming.port.to_string()));
        out.insert("storeProtocol".into(), Value::string(protocol));
        out.insert("mailUsername".into(), Value::string(&incoming.username));
        out.insert(
            "oauthOnly".into(),
            Value::Bool(
                suggestion
                    .incoming
                    .iter()
                    .all(|choice| choice.auth.eq_ignore_ascii_case("OAuth2")),
            ),
        );
    }
    if let Some(outgoing) = suggestion.outgoing.first() {
        out.insert("smtpHost".into(), Value::string(&outgoing.host));
        out.insert("smtpPort".into(), Value::string(outgoing.port.to_string()));
        out.insert(
            "smtpSsl".into(),
            Value::string(if outgoing.socket == "SSL" { "Y" } else { "N" }),
        );
        out.insert(
            "smtpStartTls".into(),
            Value::string(if outgoing.socket == "STARTTLS" { "Y" } else { "N" }),
        );
        match out.get("mailUsername").map(to_plain_string) {
            None => {
                out.insert("mailUsername".into(), Value::string(&outgoing.username));
            }
            // an outgoing server that signs in as another user: its own SMTP sign-in
            Some(incoming) if !outgoing.username.is_empty() && !outgoing.username.eq_ignore_ascii_case(&incoming) => {
                out.insert("smtpUsername".into(), Value::string(&outgoing.username));
            }
            Some(_) => {}
        }
    }
    out.insert(
        "incomingList".into(),
        Value::List(suggestion.incoming.iter().map(Choice::to_map).collect()),
    );
    out.insert(
        "outgoingList".into(),
        Value::List(suggestion.outgoing.iter().map(Choice::to_map).collect()),
    );
    Ok(out)
}

/// A DNS name with a dot, not an address literal.
fn valid_domain(domain: &str) -> bool {
    domain.len() <= 253
        && domain.contains('.')
        && !domain.starts_with(['.', '-'])
        && !domain.ends_with('-')
        && domain
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '-')
        && !domain.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
        && !domain.contains("..")
}

fn percent(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

// ---- Thunderbird autoconfig ----

fn autoconfig(address: &str, local: &str, domain: &str) -> Option<Suggestion> {
    let configured = super::client_setting("autoconfig-urls");
    let (templates, own_list): (Vec<String>, bool) = if configured.trim().is_empty() {
        (DEFAULT_AUTOCONFIG.iter().map(|url| (*url).to_string()).collect(), false)
    } else {
        (
            configured
                .split(',')
                .map(|url| url.trim().to_string())
                .filter(|url| !url.is_empty())
                .collect(),
            true,
        )
    };
    let urls: Vec<String> = templates
        .iter()
        .map(|template| {
            template
                .replace("{domain}", domain)
                .replace("{email}", &percent(address))
        })
        .collect();
    let (sender, receiver) = mpsc::channel::<(usize, Option<String>)>();
    for (index, url) in urls.iter().enumerate() {
        let thread_sender = sender.clone();
        let url = url.clone();
        // the ISPDB is Thunderbird's; the domain's own URLs only to public addresses
        let deny_internal = !own_list && !url.starts_with("https://autoconfig.thunderbird.net/");
        let spawned = std::thread::Builder::new()
            .name("moqui-email-autoconfig".to_string())
            .spawn(move || {
                let _ = thread_sender.send((index, fetch_text(&url, deny_internal, HTTP_BUDGET)));
            });
        if spawned.is_err() {
            let _ = sender.send((index, None));
        }
    }
    drop(sender);
    let deadline = Instant::now() + HTTP_BUDGET + Duration::from_millis(200);
    let mut answers: Vec<Option<Option<String>>> = vec![None; urls.len()];
    while answers.iter().any(Option::is_none) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match receiver.recv_timeout(left) {
            Ok((index, body)) => answers[index] = Some(body),
            Err(_) => break,
        }
        // the first in order that answered with a config wins as soon as those before it are done
        if let Some(found) = first_in_order(&answers, local, domain, address) {
            return Some(found);
        }
    }
    first_in_order(&answers, local, domain, address)
}

fn first_in_order(answers: &[Option<Option<String>>], local: &str, domain: &str, address: &str) -> Option<Suggestion> {
    for (index, answer) in answers.iter().enumerate() {
        match answer {
            None => return None,
            Some(None) => continue,
            Some(Some(body)) => {
                if let Some(mut suggestion) = parse_config(body, local, domain, address) {
                    suggestion.source = if index + 1 == answers.len() && answers.len() == DEFAULT_AUTOCONFIG.len() {
                        "ispdb".to_string()
                    } else {
                        "autoconfig".to_string()
                    };
                    return Some(suggestion);
                }
            }
        }
    }
    None
}

#[cfg(feature = "http")]
fn fetch_text(url: &str, deny_internal: bool, timeout: Duration) -> Option<String> {
    let client = moqui_lib::http::RestClient::new()
        .deny_internal_addresses(deny_internal)
        .timeout(timeout);
    match client.get(url).timeout(timeout).send(false) {
        Ok(response) if response.status() == 200 => Some(response.body().to_string()),
        Ok(response) => {
            tracing::debug!(
                "Mail autoconfig {}: HTTP {}",
                moqui_lib::http::redacted(url),
                response.status()
            );
            None
        }
        Err(err) => {
            tracing::debug!("Mail autoconfig {}: {err}", moqui_lib::http::redacted(url));
            None
        }
    }
}

#[cfg(not(feature = "http"))]
fn fetch_text(_url: &str, _deny_internal: bool, _timeout: Duration) -> Option<String> {
    None
}

/// A `clientConfig` document (config-v1.1.xml).
pub(super) fn parse_config(xml: &str, local: &str, domain: &str, address: &str) -> Option<Suggestion> {
    let root = MNode::parse_text("config-v1.1.xml", xml).ok()?;
    if root.name() != "clientConfig" {
        return None;
    }
    let provider = root.first_named("emailProvider")?;
    let expand = |text: String| {
        text.replace("%EMAILADDRESS%", address)
            .replace("%EMAILLOCALPART%", local)
            .replace("%EMAILDOMAIN%", domain)
    };
    let text_of = |node: &MNode, name: &str| {
        node.first_named(name)
            .and_then(|child| child.text())
            .unwrap_or_default()
    };
    let choice = |node: &MNode| -> Option<Choice> {
        let kind = node.attribute("type").unwrap_or_default().to_ascii_lowercase();
        if !matches!(kind.as_str(), "imap" | "pop3" | "smtp") {
            return None;
        }
        let host = expand(text_of(node, "hostname")).trim().to_string();
        if host.is_empty() {
            return None;
        }
        let socket = match text_of(node, "socketType").trim().to_ascii_uppercase().as_str() {
            "SSL" | "TLS" => "SSL",
            "STARTTLS" => "STARTTLS",
            _ => "plain",
        }
        .to_string();
        let default_port = match (kind.as_str(), socket.as_str()) {
            ("imap", "SSL") => 993,
            ("imap", _) => 143,
            ("pop3", "SSL") => 995,
            ("pop3", _) => 110,
            (_, "SSL") => 465,
            _ => 587,
        };
        let auths: Vec<String> = node
            .children_named("authentication")
            .iter()
            .filter_map(MNode::text)
            .map(|text| text.trim().to_string())
            .collect();
        let auth = if auths.iter().any(|auth| auth.starts_with("password")) {
            "password".to_string()
        } else {
            auths.first().cloned().unwrap_or_else(|| "password".to_string())
        };
        Some(Choice {
            kind,
            host,
            port: text_of(node, "port").trim().parse().unwrap_or(default_port),
            socket,
            username: expand(text_of(node, "username")).trim().to_string(),
            auth,
        })
    };
    let incoming: Vec<Choice> = provider
        .children_named("incomingServer")
        .iter()
        .filter_map(choice)
        .collect();
    let outgoing: Vec<Choice> = provider
        .children_named("outgoingServer")
        .iter()
        .filter_map(choice)
        .collect();
    if incoming.is_empty() && outgoing.is_empty() {
        return None;
    }
    Some(Suggestion {
        source: String::new(),
        display_name: text_of(&provider, "displayName"),
        incoming,
        outgoing,
    })
}

// ---- DNS SRV (RFC 6186) ----

const SRV_QUERIES: &[(&str, &str, &str)] = &[
    ("_imaps._tcp", "imap", "SSL"),
    ("_imap._tcp", "imap", "STARTTLS"),
    ("_pop3s._tcp", "pop3", "SSL"),
    ("_submissions._tcp", "smtp", "SSL"),
    ("_submission._tcp", "smtp", "STARTTLS"),
];

fn nameserver() -> Option<String> {
    let configured = super::client_setting("dns-server");
    if !configured.trim().is_empty() {
        return Some(configured.trim().to_string());
    }
    let text = std::fs::read_to_string("/etc/resolv.conf").ok()?;
    let server = text.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        (words.next() == Some("nameserver"))
            .then(|| words.next().map(str::to_string))
            .flatten()
    })?;
    Some(if server.contains(':') && !server.starts_with('[') {
        format!("[{server}]:53")
    } else {
        format!("{server}:53")
    })
}

/// A DNS query for one SRV name.
pub(super) fn srv_query(id: u16, name: &str) -> Vec<u8> {
    dns_query(id, name, 33)
}

/// A DNS query for one name and record type (33 SRV, 15 MX).
pub(super) fn dns_query(id: u16, name: &str, kind: u16) -> Vec<u8> {
    let mut packet = Vec::with_capacity(64);
    packet.extend_from_slice(&id.to_be_bytes());
    packet.extend_from_slice(&[0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    for label in name.trim_end_matches('.').split('.') {
        let bytes = label.as_bytes();
        packet.push(u8::try_from(bytes.len().min(63)).unwrap_or(63));
        packet.extend_from_slice(&bytes[..bytes.len().min(63)]);
    }
    packet.push(0);
    packet.extend_from_slice(&kind.to_be_bytes());
    packet.extend_from_slice(&[0x00, 0x01]);
    packet
}

/// A name at `offset` of a DNS message, following compression pointers; the offset after it.
fn read_name(packet: &[u8], mut offset: usize) -> Option<(String, usize)> {
    let mut labels = Vec::new();
    let mut end = None;
    let mut jumps = 0;
    loop {
        let length = *packet.get(offset)?;
        if length & 0xC0 == 0xC0 {
            let pointer = (usize::from(length & 0x3F) << 8) | usize::from(*packet.get(offset + 1)?);
            end.get_or_insert(offset + 2);
            offset = pointer;
            jumps += 1;
            if jumps > 32 {
                return None;
            }
            continue;
        }
        if length == 0 {
            end.get_or_insert(offset + 1);
            break;
        }
        let start = offset + 1;
        let label = packet.get(start..start + usize::from(length))?;
        labels.push(String::from_utf8_lossy(label).into_owned());
        offset = start + usize::from(length);
    }
    Some((labels.join("."), end?))
}

/// An SRV record: priority, weight, port, target.
pub(super) type SrvRecord = (u16, u16, u16, String);

/// The id and SRV records of a DNS answer.
pub(super) fn srv_answers(packet: &[u8]) -> Option<(u16, Vec<SrvRecord>)> {
    if packet.len() < 12 {
        return None;
    }
    let id = u16::from_be_bytes([packet[0], packet[1]]);
    let questions = u16::from_be_bytes([packet[4], packet[5]]);
    let answers = u16::from_be_bytes([packet[6], packet[7]]);
    let mut offset = 12;
    for _ in 0..questions {
        let (_, next) = read_name(packet, offset)?;
        offset = next + 4;
    }
    let mut out = Vec::new();
    for _ in 0..answers {
        let (_, next) = read_name(packet, offset)?;
        let header = packet.get(next..next + 10)?;
        let kind = u16::from_be_bytes([header[0], header[1]]);
        let length = usize::from(u16::from_be_bytes([header[8], header[9]]));
        let data = next + 10;
        if kind == 33 && length >= 7 {
            let fields = packet.get(data..data + 6)?;
            let (target, _) = read_name(packet, data + 6)?;
            out.push((
                u16::from_be_bytes([fields[0], fields[1]]),
                u16::from_be_bytes([fields[2], fields[3]]),
                u16::from_be_bytes([fields[4], fields[5]]),
                target,
            ));
        }
        offset = data + length;
    }
    Some((id, out))
}

/// The MX hosts of an answer to an MX query, by preference.
pub(super) fn mx_answers(packet: &[u8]) -> Option<(u16, Vec<String>)> {
    if packet.len() < 12 {
        return None;
    }
    let id = u16::from_be_bytes([packet[0], packet[1]]);
    let questions = u16::from_be_bytes([packet[4], packet[5]]);
    let answers = u16::from_be_bytes([packet[6], packet[7]]);
    let mut offset = 12;
    for _ in 0..questions {
        let (_, next) = read_name(packet, offset)?;
        offset = next + 4;
    }
    let mut out = Vec::new();
    for _ in 0..answers {
        let (_, next) = read_name(packet, offset)?;
        let header = packet.get(next..next + 10)?;
        let kind = u16::from_be_bytes([header[0], header[1]]);
        let length = usize::from(u16::from_be_bytes([header[8], header[9]]));
        let data = next + 10;
        if kind == 15 && length >= 3 {
            let preference = packet.get(data..data + 2)?;
            let (host, _) = read_name(packet, data + 2)?;
            out.push((u16::from_be_bytes([preference[0], preference[1]]), host));
        }
        offset = data + length;
    }
    out.sort();
    Some((id, out.into_iter().map(|(_, host)| host).collect()))
}

/// The domain's MX hosts alone, for a domain whose servers came from autoconfig. Empty when DNS does not answer.
fn mx_lookup(domain: &str) -> Vec<String> {
    dns_lookup(domain, "", false, true).1
}

/// The MX hosts of a domain, for `find#OAuthProvider` with `lookupMx`. Empty when DNS does not answer.
pub(super) fn mx_hosts(domain: &str) -> Vec<String> {
    mx_lookup(domain)
}

/// A domain worth a DNS query: a name with a dot, not an address literal.
pub(super) fn lookup_domain(domain: &str) -> bool {
    valid_domain(domain)
}

/// The SRV records (RFC 6186) of a domain as a suggestion, and with `with_mx` its MX hosts, asked at once.
fn dns_lookup(domain: &str, address: &str, with_srv: bool, with_mx: bool) -> (Option<Suggestion>, Vec<String>) {
    let mut mx = Vec::new();
    let Some(server) = nameserver() else {
        return (None, mx);
    };
    let Ok(socket) = UdpSocket::bind(if server.starts_with('[') { "[::]:0" } else { "0.0.0.0:0" }) else {
        return (None, mx);
    };
    if socket.connect(&server).is_err() {
        return (None, mx);
    }
    let mut base = [0u8; 2];
    if getrandom::fill(&mut base).is_err() {
        return (None, mx);
    }
    let base = u16::from_be_bytes(base) & 0xFFF0;
    let mx_id = base + u16::try_from(SRV_QUERIES.len()).unwrap_or(0);
    let mut answered = vec![!with_srv; SRV_QUERIES.len()];
    let mut mx_answered = !with_mx;
    if with_srv {
        for (index, (prefix, _, _)) in SRV_QUERIES.iter().enumerate() {
            let id = base + u16::try_from(index).unwrap_or(0);
            if socket.send(&srv_query(id, &format!("{prefix}.{domain}"))).is_err() {
                return (None, mx);
            }
        }
    }
    if with_mx && socket.send(&dns_query(mx_id, domain, 15)).is_err() {
        mx_answered = true;
    }
    let deadline = Instant::now() + DNS_BUDGET;
    let mut suggestion = Suggestion {
        source: "srv".to_string(),
        ..Suggestion::default()
    };
    let mut buffer = [0u8; 4096];
    while answered.iter().any(|done| !done) || !mx_answered {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || socket.set_read_timeout(Some(left)).is_err() {
            break;
        }
        let Ok(read) = socket.recv(&mut buffer) else {
            break;
        };
        let packet = &buffer[..read];
        if packet.len() >= 2 && u16::from_be_bytes([packet[0], packet[1]]) == mx_id {
            if let Some((_, hosts)) = mx_answers(packet) {
                mx = hosts;
                mx_answered = true;
            }
            continue;
        }
        let Some((id, records)) = srv_answers(packet) else {
            continue;
        };
        let Some(index) = id
            .checked_sub(base)
            .map(usize::from)
            .filter(|index| *index < SRV_QUERIES.len())
        else {
            continue;
        };
        answered[index] = true;
        let (_, kind, socket_type) = SRV_QUERIES[index];
        // the lowest priority, then the highest weight; a target of "." means no such service
        let best = records
            .into_iter()
            .filter(|(_, _, port, target)| !target.is_empty() && target != "." && *port > 0)
            .min_by(|left, right| left.0.cmp(&right.0).then(right.1.cmp(&left.1)));
        if let Some((_, _, port, target)) = best {
            let choice = Choice {
                kind: kind.to_string(),
                host: target.trim_end_matches('.').to_string(),
                port,
                socket: socket_type.to_string(),
                username: address.to_string(),
                auth: "password".to_string(),
            };
            if kind == "smtp" {
                suggestion.outgoing.push(choice);
            } else {
                suggestion.incoming.push(choice);
            }
        }
    }
    ((!suggestion.incoming.is_empty()).then_some(suggestion), mx)
}

/// The servers of an OAuth provider (`EmailOAuthProvider`), for an address whose domain is at it.
fn provider_suggestion(provider: &super::oauth::Provider, address: &str) -> Suggestion {
    let choice = |kind: &str, host: &str, port: &str, default_port: u16, socket: &str| Choice {
        kind: kind.to_string(),
        host: host.to_string(),
        port: port.trim().parse().unwrap_or(default_port),
        socket: socket.to_string(),
        username: address.to_string(),
        auth: "password".to_string(),
    };
    let smtp_socket = if provider.smtp_ssl == "Y" {
        "SSL"
    } else if provider.smtp_start_tls == "N" {
        "plain"
    } else {
        "STARTTLS"
    };
    Suggestion {
        source: "provider".to_string(),
        display_name: provider.name.clone(),
        incoming: vec![if provider.store_protocol == "imap" {
            choice("imap", &provider.store_host, &provider.store_port, 143, "STARTTLS")
        } else {
            choice("imap", &provider.store_host, &provider.store_port, 993, "SSL")
        }],
        outgoing: if provider.smtp_host.is_empty() {
            Vec::new()
        } else {
            vec![choice(
                "smtp",
                &provider.smtp_host,
                &provider.smtp_port,
                if smtp_socket == "SSL" { 465 } else { 587 },
                smtp_socket,
            )]
        },
    }
}

fn guess(domain: &str, address: &str) -> Suggestion {
    Suggestion {
        source: "guess".to_string(),
        display_name: String::new(),
        incoming: vec![Choice {
            kind: "imap".to_string(),
            host: format!("imap.{domain}"),
            port: 993,
            socket: "SSL".to_string(),
            username: address.to_string(),
            auth: "password".to_string(),
        }],
        outgoing: vec![Choice {
            kind: "smtp".to_string(),
            host: format!("smtp.{domain}"),
            port: 587,
            socket: "STARTTLS".to_string(),
            username: address.to_string(),
            auth: "password".to_string(),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_config, srv_answers, srv_query, valid_domain};

    #[test]
    fn domains_and_configs() {
        assert!(valid_domain("example.com"));
        assert!(!valid_domain("localhost"));
        assert!(!valid_domain("10.0.0.1"));
        assert!(!valid_domain("a..b"));
        let xml = r#"<clientConfig version="1.1"><emailProvider id="example.com">
            <domain>example.com</domain><displayName>Example Mail</displayName>
            <incomingServer type="pop3"><hostname>pop.example.com</hostname><port>995</port>
                <socketType>SSL</socketType><username>%EMAILLOCALPART%</username>
                <authentication>password-cleartext</authentication></incomingServer>
            <incomingServer type="imap"><hostname>imap.example.com</hostname><port>993</port>
                <socketType>SSL</socketType><username>%EMAILADDRESS%</username>
                <authentication>OAuth2</authentication><authentication>password-cleartext</authentication></incomingServer>
            <outgoingServer type="smtp"><hostname>smtp.%EMAILDOMAIN%</hostname><port>587</port>
                <socketType>STARTTLS</socketType><username>%EMAILADDRESS%</username></outgoingServer>
            </emailProvider></clientConfig>"#;
        let found = parse_config(xml, "ann", "example.com", "ann@example.com").expect("config");
        assert_eq!(found.display_name, "Example Mail");
        assert_eq!(found.incoming.len(), 2);
        assert_eq!(found.incoming[1].username, "ann@example.com");
        assert_eq!(found.incoming[1].auth, "password");
        assert_eq!(found.outgoing[0].host, "smtp.example.com");
        assert!(parse_config("<html/>", "a", "b.c", "a@b.c").is_none());
    }

    #[test]
    fn srv_packets_round_trip() {
        let query = srv_query(0x1234, "_imaps._tcp.example.com");
        assert_eq!(&query[..2], &[0x12, 0x34]);
        // an answer: the question, then one SRV record whose name points back at it
        let mut answer = query.clone();
        answer[2] = 0x81;
        answer[3] = 0x80;
        answer[7] = 1;
        answer.extend_from_slice(&[0xC0, 0x0C, 0x00, 0x21, 0x00, 0x01, 0, 0, 0, 60]);
        let target = b"\x04mail\x07example\x03com\x00";
        let length = u16::try_from(6 + target.len()).unwrap();
        answer.extend_from_slice(&length.to_be_bytes());
        answer.extend_from_slice(&[0, 10, 0, 5, 0x03, 0xE1]);
        answer.extend_from_slice(target);
        let (id, records) = srv_answers(&answer).expect("answer");
        assert_eq!(id, 0x1234);
        assert_eq!(records, vec![(10, 5, 993, "mail.example.com".to_string())]);
    }
}
