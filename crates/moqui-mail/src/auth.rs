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

//! Mail authentication (plans/archive/Edge-Node.md §5.4, E5b), on `mail-auth`: SPF, DKIM, DMARC and ARC verified, with an
//! `Authentication-Results` field (RFC 8601) under the node's authserv-id, and DKIM signing. The edge node's MTA uses it
//! for mail from the internet and to the MX; internal mail between the organization's hosted accounts on any node uses
//! it too ("the same standards inside"): signed when it is sent ([`sign_for`]) and verified against the node's own key
//! when it is delivered ([`verify_internal`]).
//!
//! The DKIM key is configuration, not rows: `mail-delivery/dkim@key` is the private key (base64 PKCS#8 DER, or PEM), a secret
//! generated into `runtime/.env` the first time it is needed (RSA-2048), as the other `CHANGEME` secrets are at the
//! start; `mail-delivery/dkim@selector` (default `moqui`); `mail-delivery/dkim@domains` the domains it signs for (comma separated), else
//! the domain of any hosted or shared account. [`dkim_record`] is the TXT record to publish at
//! `<selector>._domainkey.<domain>`.

use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use mail_auth::arc::ArcOutput;
use mail_auth::crypto::{RsaKey, Sha256};
use mail_auth::dkim::DkimSigner;
use mail_auth::dmarc::verify::DmarcParameters;
use mail_auth::dmarc::Policy;
use mail_auth::headers::HeaderWriter;
use mail_auth::spf::verify::SpfParameters;
use mail_auth::{AuthenticatedMessage, AuthenticationResults, DkimResult, DmarcResult, Parameters};
use moqui_lib::sync::lock;

use super::dns::{self, Records};

/// The private DKIM key, a secret conf attribute.
pub const DKIM_KEY: &str = "mail-delivery/dkim@key";
/// The DKIM selector.
pub const DKIM_SELECTOR: &str = "mail-delivery/dkim@selector";
/// The domains signed for.
pub const DKIM_DOMAINS: &str = "mail-delivery/dkim@domains";
/// The header fields signed, as RFC 6376 section 5.4.1 recommends; one that is absent is signed as absent. From,
/// Subject, To and Date are named twice (oversigned, section 8.15), so a second one added later breaks the signature.
const SIGNED_FIELDS: [&str; 16] = [
    "From",
    "From",
    "Subject",
    "To",
    "Date",
    "Reply-To",
    "Subject",
    "Date",
    "To",
    "Cc",
    "Message-ID",
    "In-Reply-To",
    "References",
    "MIME-Version",
    "Content-Type",
    "Content-Transfer-Encoding",
];

/// The node's DKIM key, read from `mail-delivery/dkim@key`.
pub struct DkimKey {
    /// The property text it came from: a change of the property is read again.
    text: String,
    pkcs8: Vec<u8>,
    /// The public key, base64 SubjectPublicKeyInfo DER, the record's `p=`.
    public: String,
}

impl DkimKey {
    /// From the property's text: base64 PKCS#8 DER on one line, or a PEM `PRIVATE KEY`.
    ///
    /// # Errors
    ///
    /// Returns why when it is not an RSA private key.
    pub fn parse(text: &str) -> Result<Self, String> {
        let body: String = text
            .lines()
            .filter(|line| !line.trim_start().starts_with("-----"))
            .map(str::trim)
            .collect();
        let pkcs8 = base64::engine::general_purpose::STANDARD
            .decode(body.as_bytes())
            .map_err(|_| format!("{DKIM_KEY} is not base64 or PEM"))?;
        let pair = aws_lc_rs::rsa::KeyPair::from_pkcs8(&pkcs8)
            .map_err(|err| format!("{DKIM_KEY} is not an RSA private key (PKCS#8): {err}"))?;
        use aws_lc_rs::encoding::AsDer as _;
        let public: aws_lc_rs::encoding::PublicKeyX509Der =
            aws_lc_rs::signature::KeyPair::public_key(&pair)
                .as_der()
                .map_err(|err| format!("the public key of {DKIM_KEY}: {err}"))?;
        Ok(Self {
            text: text.trim().to_string(),
            pkcs8,
            public: base64::engine::general_purpose::STANDARD.encode(public.as_ref()),
        })
    }

    /// A new RSA-2048 key, as the property's text (base64 PKCS#8 DER).
    ///
    /// # Errors
    ///
    /// Returns why when the key could not be made.
    pub fn generate() -> Result<String, String> {
        use aws_lc_rs::encoding::AsDer as _;
        let pair = aws_lc_rs::rsa::KeyPair::generate(aws_lc_rs::rsa::KeySize::Rsa2048)
            .map_err(|err| format!("could not make a DKIM key: {err}"))?;
        let der: aws_lc_rs::encoding::Pkcs8V1Der = pair
            .as_der()
            .map_err(|err| format!("could not write the DKIM key: {err}"))?;
        Ok(base64::engine::general_purpose::STANDARD.encode(der.as_ref()))
    }

    /// The TXT record of the key, for `<selector>._domainkey.<domain>`.
    #[must_use]
    pub fn record(&self) -> String {
        format!("v=DKIM1; k=rsa; p={}", self.public)
    }

    /// `raw` with a `DKIM-Signature` of `domain` under `selector` on top.
    ///
    /// # Errors
    ///
    /// Returns why when it was not signed.
    pub fn sign(&self, raw: &[u8], domain: &str, selector: &str) -> Result<Vec<u8>, String> {
        let der = rustls::pki_types::PrivatePkcs8KeyDer::from(self.pkcs8.as_slice());
        let key = RsaKey::<Sha256>::from_key_der(rustls::pki_types::PrivateKeyDer::Pkcs8(der))
            .map_err(|err| format!("the DKIM key does not sign: {err}"))?;
        let signature = DkimSigner::from_key(key)
            .domain(domain.to_ascii_lowercase())
            .selector(selector)
            .headers(SIGNED_FIELDS)
            .sign(raw)
            .map_err(|err| format!("the message was not DKIM-signed for {domain}: {err}"))?;
        let header = signature.to_header();
        let mut out = Vec::with_capacity(header.len() + raw.len());
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(raw);
        Ok(out)
    }
}

/// The DKIM selector: `mail-delivery/dkim@selector`, default `moqui`.
#[must_use]
pub fn selector() -> String {
    match moqui_core::context::conf_env::setting(DKIM_SELECTOR).as_str() {
        "" => "moqui".to_string(),
        selector => selector.to_ascii_lowercase(),
    }
}

/// The domains named in `mail-delivery/dkim@domains`, in lower case; empty for "the hosted accounts' domains".
#[must_use]
pub fn configured_domains() -> Vec<String> {
    moqui_core::context::conf_env::setting(DKIM_DOMAINS)
        .split([',', ' '])
        .map(|domain| domain.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|domain| !domain.is_empty())
        .collect()
}

/// The server's `runtime/.env`, set by the host at its start ([`enable_key_generation`]): a new `mail-delivery/dkim@key`
/// is made only when it is set, and kept by its server's conf (the seal's store, plans/Sealed-Secrets.md S4) or in it.
/// Without it (a unit test, a tool) no key is made.
fn env_file() -> &'static Mutex<Option<std::path::PathBuf>> {
    static ENV_FILE: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);
    &ENV_FILE
}

/// Let [`key`] make the node's DKIM key when `mail-delivery/dkim@key` is not set ([`keep_new_key`]). The host calls it at
/// its start; the first server of a process names its `runtime/.env`.
pub fn enable_key_generation(env_file_path: std::path::PathBuf) {
    let mut file = lock(env_file());
    if file.is_none() {
        *file = Some(env_file_path);
    }
}

/// The node's DKIM key, or none when `mail-delivery/dkim@key` is not set and this process makes none. When it is still
/// `CHANGEME` or empty on a server, an RSA-2048 key is made and kept, once per process, as the other generated secrets
/// are ([`keep_new_key`]).
///
/// # Errors
///
/// Returns why when the key is not one, or a new one could not be made or kept.
pub fn key() -> Result<Option<Arc<DkimKey>>, String> {
    static KEY: Mutex<Option<Arc<DkimKey>>> = Mutex::new(None);
    let mut kept = lock(&KEY);
    let mut text = moqui_core::context::conf_env::setting(DKIM_KEY);
    if let Some(key) = kept.as_ref().filter(|key| key.text == text) {
        return Ok(Some(Arc::clone(key)));
    }
    if text.is_empty() || text.ends_with("CHANGEME") {
        let Some(env) = lock(env_file()).clone() else {
            return Ok(None);
        };
        text = DkimKey::generate()?;
        keep_new_key(&env, &text)?;
        tracing::info!("Generated secret {DKIM_KEY} (RSA-2048)");
    }
    let key = Arc::new(DkimKey::parse(&text)?);
    *kept = Some(Arc::clone(&key));
    Ok(Some(key))
}

/// Keep a new key: by the running factory's conf when this thread has one or the process booted one (into its seal's
/// store, or a test factory's `runtime/.env`, and set in its live conf), else in `env` and as a process property of the
/// attribute's environment name, which [`moqui_core::context::conf_env::setting`] reads first.
fn keep_new_key(env: &std::path::Path, text: &str) -> Result<(), String> {
    use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
    let factory =
        ExecutionContext::with_current(|ec| Arc::clone(ec.factory())).or_else(ExecutionContextFactory::running);
    if let Some(factory) = factory.filter(|factory| factory.runtime_dir().join(".env") == env) {
        return factory
            .set_conf_value(DKIM_KEY, Some(text), true)
            .map(|_| ())
            .map_err(|err| format!("a new {DKIM_KEY} was not kept: {err}"));
    }
    let name = moqui_lib::node::path::AttrPath::parse(DKIM_KEY)
        .ok()
        .and_then(|path| path.env_name())
        .unwrap_or_default();
    moqui_lib::env::update_dotenv_file(env, &[(name.clone(), text.to_string())], &[])
        .map_err(|err| format!("a new {DKIM_KEY} was not written to {}: {err}", env.display()))?;
    moqui_lib::env::set_property(&name, text);
    Ok(())
}

/// The TXT record to publish for the node's DKIM key at `<selector>._domainkey.<domain>`: `v=DKIM1; k=rsa; p=...`.
///
/// # Errors
///
/// Returns why when there is no key and none could be made.
pub fn dkim_record() -> Result<String, String> {
    match key()? {
        Some(key) => Ok(key.record()),
        None => Err(format!("this node has no DKIM key: {DKIM_KEY} is not set")),
    }
}

/// The domains of this node's hosted and shared accounts, the domains it signs for when `mail-delivery/dkim@domains` names
/// none, by the node's role:
/// - a normal node: its own accounts';
/// - a dedicated edge node: its copies of the local nodes' accounts;
/// - an all-in-one node (plans/archive/Edge-Node.md §10): both, the server's own and its paired node's. Only the list of
///   `get#DkimRecord` takes both; a send signs for one side ([`is_hosted_domain`] for the server's own,
///   [`paired_domains`] for a paired node's).
///
/// # Errors
///
/// Returns why when the accounts could not be read.
pub fn hosted_domains(
    facade: &moqui_core::entity::EntityFacade,
    role: moqui_core::edge::NodeRole,
) -> Result<std::collections::BTreeSet<String>, moqui_lib::error::Error> {
    role_domains(
        role,
        || own_addresses(facade),
        || replicated_copies(facade, None),
    )
}

/// The hosted and shared accounts' addresses among the copies of paired node `node` (every paired node's with none),
/// with the copies of their servers, so an account with a store server elsewhere is left out.
fn replicated_copies(
    facade: &moqui_core::entity::EntityFacade,
    node: Option<&str>,
) -> Result<Vec<String>, moqui_lib::error::Error> {
    let list = |entity: &str| {
        moqui_core::edge::replica::list_values(facade, node, super::edge_mail::RECIPIENT_SET, Some(entity), 1_000_000)
    };
    let mut values = list(super::sync::ACCOUNT)?;
    values.extend(list(super::implementation::SERVER)?);
    Ok(replicated_addresses(&values))
}

/// The domains of paired node `node`'s hosted and shared accounts, from its copies: what its `mailOut` items are signed
/// for, on a dedicated edge node and an all-in-one node alike, so a paired node never sends signed as another side.
///
/// # Errors
///
/// Returns why when the copies could not be read.
pub fn paired_domains(
    facade: &moqui_core::entity::EntityFacade,
    node: &str,
) -> Result<std::collections::BTreeSet<String>, moqui_lib::error::Error> {
    Ok(domains_of(&replicated_copies(facade, Some(node))?))
}

/// The role whose domains this node's own sends are signed for: an all-in-one node's are the server's own, as a normal
/// node's; its paired node's domains are only for that node's mail ([`paired_domains`]).
fn sending_role(role: moqui_core::edge::NodeRole) -> moqui_core::edge::NodeRole {
    if role.all_in_one() {
        moqui_core::edge::NodeRole::Normal
    } else {
        role
    }
}

/// The domains of a node of `role`: of the addresses `own` reads unless a dedicated edge node, and of those
/// `replicated` reads when it takes links.
fn role_domains(
    role: moqui_core::edge::NodeRole,
    own: impl FnOnce() -> Result<Vec<String>, moqui_lib::error::Error>,
    replicated: impl FnOnce() -> Result<Vec<String>, moqui_lib::error::Error>,
) -> Result<std::collections::BTreeSet<String>, moqui_lib::error::Error> {
    let mut addresses: Vec<String> = Vec::new();
    if !role.edge_node() {
        addresses.extend(own()?);
    }
    if role.takes_links() {
        addresses.extend(replicated()?);
    }
    Ok(domains_of(&addresses))
}

/// The addresses of the server's own hosted and shared accounts.
fn own_addresses(facade: &moqui_core::entity::EntityFacade) -> Result<Vec<String>, moqui_lib::error::Error> {
    use moqui_lib::object::Value;
    let text = moqui_core::service::util::field_text;
    let rows = facade
        .find(super::sync::ACCOUNT)?
        .condition(moqui_core::entity::Condition::is_in(
            "accountKindEnumId",
            vec![Value::string("EakHosted"), Value::string("EakShared")],
        ))
        .select_field("emailAddress")
        .select_field("emailServerId")
        .disable_authz()
        .list()?;
    // delivered here: no server, or a server with no incoming host; an account of a store elsewhere is a user's own
    // (only a mail administrator makes one delivered here, SEC-MAIL-01), the rule `implementation::client_signs` keeps
    let stored: std::collections::HashSet<String> = facade
        .find(super::implementation::SERVER)?
        .condition(moqui_core::entity::Condition::not_equals("storeHost", Value::string("")))
        .condition(moqui_core::entity::Condition::is_not_null("storeHost"))
        .select_field("emailServerId")
        .disable_authz()
        .list()?
        .values()
        .iter()
        .map(|row| text(row, "emailServerId"))
        .collect();
    Ok(rows
        .values()
        .iter()
        .filter(|row| !stored.contains(&text(row, "emailServerId")))
        .map(|row| text(row, "emailAddress"))
        .collect())
}

/// The addresses of the hosted and shared accounts delivered here among a paired node's copies of `EmailAccount`:
/// those whose copied server (`EmailServer`, in the same values) has no incoming host (SEC-MAIL-82).
fn replicated_addresses(values: &[moqui_core::edge::replica::ReplicaValue]) -> Vec<String> {
    let field = |json: &serde_json::Value, name: &str| json[name].as_str().unwrap_or_default().to_string();
    let stored: std::collections::HashSet<(String, String)> = values
        .iter()
        .filter(|value| value.entity_name == super::implementation::SERVER)
        .filter(|value| !field(&value.fields, "storeHost").trim().is_empty())
        .map(|value| (value.paired_node_id.clone(), field(&value.pk, "emailServerId")))
        .collect();
    values
        .iter()
        .filter(|value| value.entity_name == super::sync::ACCOUNT)
        .filter(|value| {
            matches!(
                value.fields["accountKindEnumId"].as_str(),
                Some("EakHosted" | "EakShared")
            )
        })
        .filter(|value| !stored.contains(&(value.paired_node_id.clone(), field(&value.fields, "emailServerId"))))
        .map(|value| field(&value.fields, "emailAddress"))
        .collect()
}

/// The domains of `addresses`, in lower case and without a trailing dot.
fn domains_of(addresses: &[String]) -> std::collections::BTreeSet<String> {
    addresses
        .iter()
        .filter_map(|address| address.trim().rsplit_once('@'))
        .map(|(_, domain)| domain.trim_end_matches('.').to_ascii_lowercase())
        .filter(|domain| !domain.is_empty())
        .collect()
}

/// `get#DkimRecord`: the TXT record of the node's key, its selector and the domains it signs for.
pub(super) fn get_dkim_record(
    _parameters: &std::collections::BTreeMap<String, moqui_lib::object::Value>,
    facade: &moqui_core::entity::EntityFacade,
) -> Result<std::collections::BTreeMap<String, moqui_lib::object::Value>, moqui_lib::error::Error> {
    use moqui_lib::object::Value;
    let factory = moqui_core::context::ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
        .ok_or_else(|| moqui_lib::error::Error::new("get#DkimRecord needs an execution context"))?;
    let record = dkim_record().map_err(moqui_lib::error::Error::new)?;
    let mut domains = configured_domains();
    if domains.is_empty() {
        domains = hosted_domains(facade, moqui_core::edge::settings(&factory).role)?
            .into_iter()
            .collect();
    }
    Ok(std::collections::BTreeMap::from([
        ("selector".to_string(), Value::string(selector())),
        ("record".to_string(), Value::string(record)),
        (
            "domainList".to_string(),
            Value::List(domains.into_iter().map(Value::string).collect()),
        ),
    ]))
}

/// The domain of the first address in a message's `From` field, in lower case.
#[must_use]
pub fn from_domain(raw: &[u8]) -> Option<String> {
    let address = from_address(raw)?;
    let (_, domain) = address.rsplit_once('@')?;
    let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    (!domain.is_empty()).then_some(domain)
}

/// The first address in a message's `From` field, trimmed; none without a field or an address with an `@`.
#[must_use]
pub fn from_address(raw: &[u8]) -> Option<String> {
    let (head, _) = super::delivery::split_head(raw);
    let from = super::delivery::header_fields(head)
        .into_iter()
        .map(super::delivery::field_name_value)
        .find(|(name, _)| name.eq_ignore_ascii_case("From"))?
        .1;
    // the address in angle brackets, else the first word with an @
    let address = match from.rsplit_once('<') {
        Some((_, rest)) => rest.split('>').next().unwrap_or_default().to_string(),
        None => from
            .split([',', ' '])
            .find(|word| word.contains('@'))
            .unwrap_or_default()
            .to_string(),
    };
    let address = address.trim();
    address.contains('@').then(|| address.to_string())
}

/// The domain to sign `raw` for: its From domain, when it is one of `mail-delivery/dkim@domains`, or with none named, one that
/// `hosted` says is a hosted or shared account's.
#[must_use]
pub fn signing_domain(raw: &[u8], hosted: impl Fn(&str) -> bool) -> Option<String> {
    let domain = from_domain(raw)?;
    let configured = configured_domains();
    let signs = if configured.is_empty() {
        hosted(&domain)
    } else {
        configured.contains(&domain)
    };
    signs.then_some(domain)
}

/// The domain to sign paired node mail `raw` for: its From domain when it is one of `paired` (the paired node's own,
/// [`paired_domains`]) and, when `mail-delivery/dkim@domains` names any, also one of those. The list narrows what a
/// paired node's mail is signed for and never widens it, so on a node with several pairings one pairing's mail is never
/// signed for another's domain (SEC-MAIL-90).
#[must_use]
pub fn paired_signing_domain(raw: &[u8], paired: &std::collections::BTreeSet<String>) -> Option<String> {
    let domain = from_domain(raw)?;
    paired_signs(&domain, paired, &configured_domains()).then_some(domain)
}

/// Whether a paired node's mail from `domain` is signed: `domain` is the paired node's, and in `configured` when that
/// names any.
fn paired_signs(domain: &str, paired: &std::collections::BTreeSet<String>, configured: &[String]) -> bool {
    paired.contains(domain) && (configured.is_empty() || configured.iter().any(|listed| listed == domain))
}

/// How long [`is_hosted_domain`] keeps the hosted accounts' domains before reading them again.
const HOSTED_TTL: std::time::Duration = std::time::Duration::from_secs(60);

/// Whether `domain` is a hosted or shared account's on this node (see [`hosted_domains`]), as the current context's
/// factory reads it, for the node's own sends: on an all-in-one node the server's own accounts only, never its paired
/// node's. False with no context or when the accounts cannot be read. The domains are kept a minute, so a send does
/// not read every account (an account added is signed for within the minute).
#[must_use]
pub fn is_hosted_domain(facade: &moqui_core::entity::EntityFacade, domain: &str) -> bool {
    let role = sending_role(
        moqui_core::context::ExecutionContext::with_current(|ec| moqui_core::edge::settings(ec.factory()).role)
            .unwrap_or_default(),
    );
    match kept_hosted_domains(facade, role) {
        Ok(domains) => domains.contains(domain),
        Err(err) => {
            tracing::warn!("The hosted accounts' domains were not read: {err}");
            false
        }
    }
}

/// [`hosted_domains`] of `role`, kept a minute ([`HOSTED_TTL`]), so a send, or each item of a paired node's outbound
/// pass (SEC-MAIL-90), does not read every account and copy (a domain added is seen within the minute).
///
/// # Errors
///
/// Returns why when the accounts could not be read.
pub fn kept_hosted_domains(
    facade: &moqui_core::entity::EntityFacade,
    role: moqui_core::edge::NodeRole,
) -> Result<Arc<std::collections::BTreeSet<String>>, moqui_lib::error::Error> {
    type Kept = Vec<(
        moqui_core::edge::NodeRole,
        std::time::Instant,
        Arc<std::collections::BTreeSet<String>>,
    )>;
    // by role: an edge node's, an all-in-one node's and a normal node's domains differ for one database
    static KEPT: Mutex<Kept> = Mutex::new(Vec::new());
    if let Some((_, _, domains)) = lock(&KEPT)
        .iter()
        .find(|(kept_role, at, _)| *kept_role == role && at.elapsed() < HOSTED_TTL)
    {
        return Ok(Arc::clone(domains));
    }
    let domains = Arc::new(hosted_domains(facade, role)?);
    let mut kept = lock(&KEPT);
    kept.retain(|(kept_role, _, _)| *kept_role != role);
    kept.push((role, std::time::Instant::now(), Arc::clone(&domains)));
    Ok(domains)
}

/// The ports this process's SMTP listeners are bound to.
fn own_listeners() -> &'static Mutex<Vec<u16>> {
    static PORTS: Mutex<Vec<u16>> = Mutex::new(Vec::new());
    &PORTS
}

/// The host's SMTP listener is bound to `port`: a send to it on a loopback address is mail to this node itself.
pub fn note_own_listener(port: u16) {
    let mut ports = lock(own_listeners());
    if !ports.contains(&port) {
        ports.push(port);
    }
}

/// Whether this process's SMTP listener is bound to `port`.
#[must_use]
pub fn is_own_listener(port: u16) -> bool {
    lock(own_listeners()).contains(&port)
}

/// Whether `mail-delivery/dkim@key` is set (configured, or made earlier): a send by SMTP to another server is signed only then,
/// so a node never sends a signature of a key it has not published.
#[must_use]
pub fn key_configured() -> bool {
    let text = moqui_core::context::conf_env::setting(DKIM_KEY);
    let text = text.trim();
    !text.is_empty() && !text.ends_with("CHANGEME")
}

/// `raw` signed with the node's key when its From domain is one this node signs for (see [`signing_domain`]); as it is
/// when not, or when the node has no key. A key that cannot be had is logged and the message goes unsigned: a message
/// is not held for its signature.
#[must_use]
pub fn sign_for(raw: Vec<u8>, hosted: impl Fn(&str) -> bool) -> Vec<u8> {
    let domain = signing_domain(&raw, hosted);
    sign_as(raw, domain)
}

/// Paired node mail `raw` signed with the node's key when its From domain is the paired node's (see
/// [`paired_signing_domain`]), as [`sign_for`] signs the node's own.
#[must_use]
pub fn sign_for_paired(raw: Vec<u8>, paired: &std::collections::BTreeSet<String>) -> Vec<u8> {
    let domain = paired_signing_domain(&raw, paired);
    sign_as(raw, domain)
}

/// `raw` signed for `domain` with the node's key; as it is with no domain or no key, or when the key fails (logged).
fn sign_as(raw: Vec<u8>, domain: Option<String>) -> Vec<u8> {
    let Some(domain) = domain else {
        return raw;
    };
    let signed = key().and_then(|key| match key {
        Some(key) => key.sign(&raw, &domain, &selector()).map(Some),
        None => Ok(None),
    });
    match signed {
        Ok(Some(signed)) => signed,
        Ok(None) => {
            tracing::debug!("A message from {domain} goes unsigned: this node has no DKIM key");
            raw
        }
        Err(err) => {
            tracing::error!("A message from {domain} goes unsigned: {err}");
            raw
        }
    }
}

/// What a DMARC failure asks of the receiver.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DmarcAction {
    /// Passed, or no policy applies.
    #[default]
    Accept,
    /// Failed with `p=quarantine`: to the Junk folder.
    Quarantine,
    /// Failed with `p=reject`: refused (550).
    Reject,
}

/// The checks of a message from another server.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Verdict {
    /// The `Authentication-Results` field, with its CRLF.
    pub field: String,
    pub dmarc: DmarcAction,
    /// The results in short (`spf=pass dkim=pass dmarc=pass arc=none`), for the log.
    pub summary: String,
    /// The From domain whose DMARC policy applied.
    pub domain: String,
    /// The checks did not finish (a timeout, a failure): the message is not judged now, and the listener answers 451.
    pub temp_error: bool,
    /// Why the message is refused before any policy is asked: its author is not one domain ([`author_refusal`]). Its
    /// `dmarc` is then [`DmarcAction::Reject`] too, and the listener answers 550 with this text.
    pub refused: Option<String>,
}

/// Why a message whose author is not one domain is refused, or none: more than one `From` field, no `From` address,
/// or `From` addresses of more than one domain. DMARC judges one author domain and `mail-auth` passes over such a
/// message (an empty result, no policy), while the stored sender is the first address, so `From: ceo@bank.example,
/// x@evil.example` would come in as the bank's whatever its policy (RFC 7489 section 6.6.1; plans/Security-Review-Medium.md
/// SEC-MAIL-81).
fn author_refusal(raw: &[u8], addresses: &[String]) -> Option<String> {
    let (head, _) = super::delivery::split_head(raw);
    let fields = super::delivery::header_fields(head)
        .into_iter()
        .map(super::delivery::field_name_value)
        .filter(|(name, _)| name.trim().eq_ignore_ascii_case("From"))
        .count();
    if fields > 1 {
        return Some(format!("The message has {fields} From fields: one author is required"));
    }
    let domains: std::collections::BTreeSet<String> = addresses
        .iter()
        .filter_map(|address| address.rsplit_once('@'))
        .map(|(_, domain)| domain.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|domain| !domain.is_empty())
        .collect();
    match domains.len() {
        0 => Some("The message names no From address: one author is required".to_string()),
        1 => None,
        _ => Some("The message's From addresses are of more than one domain: one author domain is required".to_string()),
    }
}

/// The SMTP session a message came in, as the checks read it.
#[derive(Clone, Debug, Default)]
pub struct Session {
    pub client_ip: Option<IpAddr>,
    /// The client's EHLO name.
    pub helo: String,
    /// `MAIL FROM`; empty for the null reverse path.
    pub mail_from: String,
    /// This node's name, SPF's receiving host.
    pub hostname: String,
}

/// The most DKIM signatures of one message checked; the rest are not (each costs DNS queries and a signature check).
const MAX_SIGNATURES: usize = 10;
/// How long the checks of one message may take before they count as a temporary error.
const VERIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

/// Check SPF (the HELO and MAIL FROM identities), DKIM (its first ten signatures), ARC and DMARC of `raw`, which came
/// from `session`, with the node's resolver (or the fixed records `records`, a test's); the `Authentication-Results`
/// field is under `authserv_id`. It runs on the resolver's runtime, so it may be awaited from any. Checks that fail
/// (a panic) or take over 45 seconds are a temporary error ([`Verdict::temp_error`]), which the listener answers 451.
///
/// DMARC is RFC 9989's: `t=y` lowers the policy one level, and there is no `pct`. A failure under `p=reject` or
/// `p=quarantine` is overridden when a passing ARC chain was last sealed by one of `smtp-server@arc-trusted-sealers` (a
/// mailing list or forwarder this node trusts) and that sealer's `ARC-Authentication-Results` records `dmarc=pass` for
/// the From domain; with that property empty there is no ARC exception. A DMARC result of `temperror` (a DNS error), or
/// no resolver, is a temporary error too.
pub async fn verify(raw: Vec<u8>, session: Session, authserv_id: String, records: Option<Arc<Records>>) -> Verdict {
    let id = authserv_id.clone();
    let done = dns::on_runtime(async move {
        let records: &Records = match &records {
            Some(records) => records,
            None => dns::records(),
        };
        tokio::time::timeout(VERIFY_TIMEOUT, verify_with(&raw, &session, &authserv_id, records)).await
    })
    .await;
    let error = match done {
        Ok(Ok(verdict)) => return verdict,
        Ok(Err(_)) => format!("the checks took over {} seconds", VERIFY_TIMEOUT.as_secs()),
        Err(err) => err,
    };
    tracing::error!("Mail authentication failed: {error}");
    Verdict {
        field: format!("Authentication-Results: {id}; none (temporary error)\r\n"),
        summary: "temperror".to_string(),
        temp_error: true,
        ..Verdict::default()
    }
}

/// Whether a DMARC result defers the message (451): it is a temporary error (`temp`) and either the domain's record
/// was not read (`has_record` false) or its policy is reject or quarantine.
fn defers(temp: bool, has_record: bool, policy: Policy) -> bool {
    temp && (!has_record || matches!(policy, Policy::Reject | Policy::Quarantine))
}

/// The trusted sealer whose ARC seal overrides a DMARC failure, or none: `newest` (the newest set's sealer, with the
/// value of its `ARC-Authentication-Results`) is one of `trusted`, and that sealer recorded `dmarc=pass` for `domain`, the
/// message's From domain, when it took the message. A trusted sealer carries a pass across its own change of the message
/// (a list's footer); it never turns a failure it recorded itself into a pass (SEC-MAIL-88).
fn arc_overrides(newest: Option<(String, &[u8])>, domain: &str, trusted: &[String]) -> Option<String> {
    let (sealer, results) = newest?;
    (!sealer.is_empty() && trusted.contains(&sealer) && recorded_dmarc_pass(results, domain)).then_some(sealer)
}

/// Whether an `ARC-Authentication-Results` value records `dmarc=pass` with a `header.from` of `domain`.
fn recorded_dmarc_pass(results: &[u8], domain: &str) -> bool {
    let text = String::from_utf8_lossy(results);
    // RFC 8601's lexical rules: comments out (they may hold anything, `;` too), and a quoted string kept as one word
    // whose `;`, `=`, parentheses and spaces split nothing, so a quoted `smtp.mailfrom` or `smtp.helo` the sealer
    // copied cannot write an entry of its own; then the entries split on `;`
    let mut plain = String::with_capacity(text.len());
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for ch in text.chars() {
        if escaped {
            escaped = false;
            if depth == 0 {
                plain.push(inert(ch));
            }
            continue;
        }
        match ch {
            '\\' if depth > 0 || quoted => escaped = true,
            '"' if depth == 0 => {
                quoted = !quoted;
                plain.push('"');
            }
            ch if quoted => plain.push(inert(ch)),
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            _ if depth > 0 => {}
            ch if ch.is_whitespace() => plain.push(' '),
            ch => plain.push(ch),
        }
    }
    let domain = domain.trim().trim_end_matches('.');
    plain.split(';').any(|entry| {
        let mut words = entry.split_whitespace();
        words.next().is_some_and(|word| word.eq_ignore_ascii_case("dmarc=pass"))
            && words.any(|word| {
                word.split_once('=').is_some_and(|(name, value)| {
                    name.eq_ignore_ascii_case("header.from")
                        && value
                            .trim_matches('"')
                            .trim_end_matches('.')
                            .eq_ignore_ascii_case(domain)
                })
            })
    })
}

/// A character of a quoted string as [`recorded_dmarc_pass`] keeps it: one that would end a word or an entry, start a
/// comment or name a value outside quotes stands as `_`.
fn inert(ch: char) -> char {
    if ch.is_whitespace() || matches!(ch, ';' | '(' | ')' | '=') {
        '_'
    } else {
        ch
    }
}

/// `field` with its `dmarc=fail` result marked, in a comment, as overridden by `sealer`, so a reader of the stored field
/// sees that the failure was let in.
fn overridden_field(field: String, sealer: &str) -> String {
    const ENTRY: &str = ";\r\n\tdmarc=fail";
    match field.find(ENTRY) {
        Some(at) => {
            let (head, rest) = field.split_at(at + ENTRY.len());
            format!("{head} (overridden by the trusted ARC sealer {sealer}){rest}")
        }
        None => field,
    }
}

/// The domains whose ARC seal is trusted to carry a DMARC pass across a forwarder (`smtp-server@arc-trusted-sealers`).
fn trusted_sealers() -> Vec<String> {
    moqui_core::context::conf_env::setting("smtp-server@arc-trusted-sealers")
        .split([',', ' '])
        .map(|domain| domain.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|domain| !domain.is_empty())
        .collect()
}

async fn verify_with(raw: &[u8], session: &Session, authserv_id: &str, records: &Records) -> Verdict {
    let resolver = match dns::authenticator() {
        Ok(resolver) => resolver,
        Err(err) => {
            tracing::error!("Mail authentication has no resolver: {err}");
            // not judged is not accepted: the sender tries again (SEC-MAIL-92)
            return Verdict {
                field: format!("Authentication-Results: {authserv_id}; none (no resolver)\r\n"),
                summary: "no resolver".to_string(),
                temp_error: true,
                ..Verdict::default()
            };
        }
    };
    let Some(mut message) = AuthenticatedMessage::parse(raw) else {
        return Verdict {
            field: format!("Authentication-Results: {authserv_id}; none (not a message)\r\n"),
            summary: "not a message".to_string(),
            ..Verdict::default()
        };
    };
    if let Some(why) = author_refusal(raw, message.from_addresses()) {
        return Verdict {
            field: format!("Authentication-Results: {authserv_id}; dmarc=permerror (no single author domain)\r\n"),
            dmarc: DmarcAction::Reject,
            summary: "dmarc=permerror (no single author domain)".to_string(),
            refused: Some(why),
            ..Verdict::default()
        };
    }
    let mut kept = 0;
    message.retain_dkim_signatures(|_| {
        kept += 1;
        if kept <= MAX_SIGNATURES {
            Ok(())
        } else {
            Err(mail_auth::dkim::DkimError::UnsupportedAlgorithm)
        }
    });
    let ip = session.client_ip.unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
    let helo = session.helo.trim().trim_end_matches('.').to_ascii_lowercase();
    let host = session.hostname.as_str();
    let dkim = resolver
        .verify_dkim(Parameters::new(&message).with_cache(records))
        .await;
    let arc: ArcOutput = resolver.verify_arc(Parameters::new(&message).with_cache(records)).await;
    let spf_params = SpfParameters::helo_and_mail_from(ip, &helo, host, &session.mail_from);
    let spf = resolver
        .verify_spf(
            Parameters::new(SpfParameters::helo_and_mail_from(ip, &helo, host, &session.mail_from)).with_cache(records),
        )
        .await;
    let mail_from_domain = match session.mail_from.rsplit_once('@') {
        Some((_, domain)) => domain.to_string(),
        None => helo.clone(),
    };
    let dmarc = resolver
        .verify_dmarc(
            Parameters::new(DmarcParameters::new(&message, &dkim, &mail_from_domain, &spf)).with_cache(records),
        )
        .await;
    // mail-auth judges no author domain when it reads the From addresses as of more than one domain, by its own
    // normalization (IDNA, no trailing dot taken off), which `author_refusal`'s may not match: such a message is
    // refused too, whatever the two made of it (SEC-MAIL-81)
    if dmarc.domain().is_empty() {
        return Verdict {
            field: format!("Authentication-Results: {authserv_id}; dmarc=permerror (no single author domain)\r\n"),
            dmarc: DmarcAction::Reject,
            summary: "dmarc=permerror (no single author domain)".to_string(),
            refused: Some(
                "The message's From addresses are of more than one domain: one author domain is required".to_string(),
            ),
            ..Verdict::default()
        };
    }
    let from = message
        .first_from_address()
        .rsplit_once('@')
        .map(|(_, domain)| domain.to_string())
        .unwrap_or_default();
    let mut results = AuthenticationResults::new(authserv_id)
        .with_dkim_results(&dkim, message.first_from_address())
        .with_spf_result(&spf, &spf_params)
        .with_dmarc_result(&dmarc);
    if !message.arc_seals().is_empty() {
        results = results.with_arc_result(&arc, ip);
    }
    let failed = matches!(dmarc.result(), DmarcResult::Fail(_));
    let mut action = match (failed, dmarc.policy()) {
        (true, Policy::Reject) => DmarcAction::Reject,
        (true, Policy::Quarantine) => DmarcAction::Quarantine,
        _ => DmarcAction::Accept,
    };
    let mut arc_override = String::new();
    let mut field = safe_mailfrom(results.to_header(), &session.mail_from);
    if action != DmarcAction::Accept && arc.result() == &DkimResult::Pass {
        // the newest set: mail-auth lists the chain oldest (instance 1) first
        let newest = arc
            .chain()
            .last()
            .map(|link| (link.seal.header.d.to_ascii_lowercase(), link.results.value));
        if let Some(sealer) = arc_overrides(newest, dmarc.domain(), &trusted_sealers()) {
            action = DmarcAction::Accept;
            arc_override = format!(" (DMARC policy overridden by the trusted ARC sealer {sealer})");
            field = overridden_field(field, &sealer);
        }
    }
    let dkim_word = if dkim.iter().any(|output| output.result() == &DkimResult::Pass) {
        "pass".to_string()
    } else {
        dkim.first()
            .map_or("none".to_string(), |output| result_word(&output.result().to_string()))
    };
    let summary = format!(
        "spf={} dkim={dkim_word} dmarc={} arc={}{arc_override}",
        result_word(&spf.result().to_string()),
        result_word(&dmarc.result().to_string()),
        result_word(&arc.result().to_string()),
    );
    // a DNS error is not a judgment: the sender tries again rather than the message going in unjudged (SEC-MAIL-92),
    // when it matters: the domain's record could not be read, or it asks reject or quarantine. A p=none domain with a
    // selector that does not resolve is accepted as before, not deferred until it bounces
    let temp_error = defers(
        matches!(dmarc.result(), DmarcResult::TempError(_)),
        dmarc.record().is_some(),
        dmarc.policy(),
    );
    Verdict {
        field,
        dmarc: action,
        summary,
        temp_error,
        refused: None,
        domain: if dmarc.domain().is_empty() {
            from
        } else {
            dmarc.domain().to_string()
        },
    }
}

/// `field` with its `smtp.mailfrom` written safely. A reverse path whose local part is quoted may hold `;`,
/// parentheses, quotes, backslashes and spaces (RFC 5321); mail-auth quotes it as RFC 8601 says, but a reader that
/// splits the field on `;` would take what the sender wrote inside the quotes for results of this server's own
/// (`dmarc=pass header.from=bank.example`). RFC 8601 lets the property be the domain alone, which is what SPF checked:
/// such a path is written as its domain, and the property is left out when that is not a plain domain name. A path
/// mail-auth writes bare, or quotes only for `=` (an SRS address), is kept as it is.
pub(crate) fn safe_mailfrom(field: String, mail_from: &str) -> String {
    let unsafe_char = |ch: char| ch.is_control() || matches!(ch, ';' | '(' | ')' | '"' | '\\' | ' ');
    if !mail_from.chars().any(unsafe_char) {
        return field;
    }
    // what mail-auth wrote (its push_quoted_pvalue): `"` and `\` escaped, control characters left out
    let mut quoted = String::from(" smtp.mailfrom=\"");
    for ch in mail_from.chars() {
        match ch {
            '"' | '\\' => {
                quoted.push('\\');
                quoted.push(ch);
            }
            '\0'..='\u{1f}' | '\u{7f}'..='\u{9f}' => {}
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    let domain = mail_from.rsplit_once('@').map(|(_, domain)| domain).filter(|domain| {
        !domain.is_empty()
            && domain
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.'))
    });
    let written = domain
        .map(|domain| format!(" smtp.mailfrom={domain}"))
        .unwrap_or_default();
    if field.contains(&quoted) {
        return field.replacen(&quoted, &written, 1);
    }
    // SPF reported the HELO identity: the path is not in the field
    if !field.contains(" smtp.mailfrom=") {
        return field;
    }
    // not as mail-auth writes it: no field is better than one a sender wrote part of
    tracing::warn!("Authentication-Results: the MAIL FROM was not found as written; the field is left with no results");
    match field.split_once(';') {
        Some((head, _)) => format!("{head}; none (the MAIL FROM could not be written)\r\n"),
        None => field,
    }
}

/// The first word of a result as mail-auth shows it (`fail; reason` is `fail`).
fn result_word(text: &str) -> String {
    text.split([';', ' ', '('])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// The `Authentication-Results` field of internal mail: its DKIM signatures of this node's selector, verified against
/// the node's own key with no DNS query, under `authserv_id`. None when none passes: mail from outside, a node with no
/// key, or a signature of another node's key under the same selector (an edge node's, whose own field reports it).
#[must_use]
pub fn verify_internal(raw: &[u8], authserv_id: &str) -> Option<String> {
    let selector = selector();
    let ours: Vec<String> = {
        let message = AuthenticatedMessage::parse(raw)?;
        let mut ours: Vec<String> = message
            .dkim_signatures()
            .iter()
            .filter(|header| header.header.s.eq_ignore_ascii_case(&selector))
            .map(|header| header.header.d.to_ascii_lowercase())
            .collect();
        ours.dedup();
        ours
    };
    if ours.is_empty() {
        return None;
    }
    let key = match key() {
        Ok(Some(key)) => key,
        Ok(None) => return None,
        Err(err) => {
            tracing::warn!("Internal mail is not verified: {err}");
            return None;
        }
    };
    verify_with_key(raw, authserv_id, &key, &selector, ours)
}

/// [`verify_internal`] with `key` under `selector`, for the signatures of `ours` (its domains).
fn verify_with_key(raw: &[u8], authserv_id: &str, key: &DkimKey, selector: &str, ours: Vec<String>) -> Option<String> {
    let selector = selector.to_string();
    let records = Records::offline();
    for domain in &ours {
        if let Err(err) = records.fix_txt(&format!("{selector}._domainkey.{domain}"), &key.record()) {
            tracing::error!("Internal mail is not verified: {err}");
            return None;
        }
    }
    let resolver = match dns::authenticator() {
        Ok(resolver) => resolver,
        Err(err) => {
            tracing::error!("Internal mail is not verified: {err}");
            return None;
        }
    };
    let raw = raw.to_vec();
    let authserv_id = authserv_id.to_string();
    dns::run(async move {
        let mut message = AuthenticatedMessage::parse(&raw)?;
        // only the node's own signatures: any other would be looked up in DNS
        message.retain_dkim_signatures(|signature| {
            if signature.s.eq_ignore_ascii_case(&selector) && ours.contains(&signature.d.to_ascii_lowercase()) {
                Ok(())
            } else {
                Err(mail_auth::dkim::DkimError::UnsupportedAlgorithm)
            }
        });
        let dkim: Vec<_> = resolver
            .verify_dkim(Parameters::new(&message).with_cache(&records))
            .await
            .into_iter()
            .filter(|output| output.signature().is_some() && output.result() == &DkimResult::Pass)
            .collect();
        if dkim.is_empty() {
            tracing::info!("A message with this node's DKIM selector does not verify with this node's key (another node's key, or changed); no Authentication-Results from here");
            return None;
        }
        Some(
            AuthenticationResults::new(&authserv_id)
                .with_dkim_results(&dkim, message.first_from_address())
                .to_header(),
        )
    })
    .unwrap_or_else(|err| {
        tracing::error!("Internal mail is not verified: {err}");
        None
    })
}

/// The authserv-id of an `Authentication-Results` value: its first word before the first `;`, in lower case.
#[must_use]
pub fn authserv_of(value: &str) -> String {
    value
        .split(';')
        .next()
        .and_then(|first| first.split_whitespace().next())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// `raw` without the `Authentication-Results` fields that claim `authserv_id` (from outside, so forged: RFC 8601
/// section 5) and without the fields named in `drop` (the spam verdict fields, which only this node writes), with
/// `fields` (each with its CRLF) on top.
#[must_use]
pub fn rewrite_head(raw: &[u8], authserv_id: &str, drop: &[String], fields: &[String]) -> Vec<u8> {
    let (head, body) = super::delivery::split_head(raw);
    let id = authserv_id.trim().to_ascii_lowercase();
    let mut out = Vec::with_capacity(raw.len() + 512);
    for field in fields {
        out.extend_from_slice(field.as_bytes());
    }
    for field in super::delivery::header_fields(head) {
        let (name, value) = super::delivery::field_name_value(field);
        if name.eq_ignore_ascii_case("Authentication-Results") && authserv_of(&value) == id {
            continue;
        }
        if drop.iter().any(|dropped| dropped.eq_ignore_ascii_case(&name)) {
            continue;
        }
        out.extend_from_slice(field);
        if !field.ends_with(b"\n") {
            out.extend_from_slice(b"\r\n");
        }
    }
    out.extend_from_slice(body);
    out
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
