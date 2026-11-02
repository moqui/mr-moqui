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

//! Mail authentication (plans/Edge-Node.md §5.4, E5b), on `mail-auth`: SPF, DKIM, DMARC and ARC verified, with an
//! `Authentication-Results` field (RFC 8601) under the node's authserv-id, and DKIM signing. The edge node's MTA uses it
//! for mail from the internet and to the MX; internal mail between the organization's hosted accounts on any node uses
//! it too ("the same standards inside"): signed when it is sent ([`sign_for`]) and verified against the node's own key
//! when it is delivered ([`verify_internal`]).
//!
//! The DKIM key is configuration, not rows: `mail_dkim_key` is the private key (base64 PKCS#8 DER, or PEM), a secret
//! generated into `runtime/.env` the first time it is needed (RSA-2048), as the other `CHANGEME` secrets are at the
//! start; `mail_dkim_selector` (default `moqui`); `mail_dkim_domains` the domains it signs for (comma separated), else
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

/// The private DKIM key property.
pub const DKIM_KEY: &str = "mail_dkim_key";
/// The DKIM selector property.
pub const DKIM_SELECTOR: &str = "mail_dkim_selector";
/// The domains signed for.
pub const DKIM_DOMAINS: &str = "mail_dkim_domains";
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

/// The node's DKIM key, read from `mail_dkim_key`.
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

/// The DKIM selector: `mail_dkim_selector`, default `moqui`.
#[must_use]
pub fn selector() -> String {
    match moqui_lib::env::get_prop_or_env(DKIM_SELECTOR).trim() {
        "" => "moqui".to_string(),
        selector => selector.to_ascii_lowercase(),
    }
}

/// The domains named in `mail_dkim_domains`, in lower case; empty for "the hosted accounts' domains".
#[must_use]
pub fn configured_domains() -> Vec<String> {
    moqui_lib::env::get_prop_or_env(DKIM_DOMAINS)
        .split([',', ' '])
        .map(|domain| domain.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|domain| !domain.is_empty())
        .collect()
}

/// Where a new `mail_dkim_key` is written: the server's `runtime/.env`, set by the host at its start
/// ([`enable_key_generation`]). Without it (a unit test, a tool) no key is made.
fn env_file() -> &'static Mutex<Option<std::path::PathBuf>> {
    static ENV_FILE: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);
    &ENV_FILE
}

/// Let [`key`] make the node's DKIM key when `mail_dkim_key` is not set, written to `env_file` (`runtime/.env`). The
/// host calls it at its start; the first server of a process names the file.
pub fn enable_key_generation(env_file_path: std::path::PathBuf) {
    let mut file = lock(env_file());
    if file.is_none() {
        *file = Some(env_file_path);
    }
}

/// The node's DKIM key, or none when `mail_dkim_key` is not set and this process makes none. When it is still
/// `CHANGEME` or empty on a server, an RSA-2048 key is made and written to `runtime/.env`, once per process, as the
/// other generated secrets are.
///
/// # Errors
///
/// Returns why when the key is not one, or a new one could not be made or kept.
pub fn key() -> Result<Option<Arc<DkimKey>>, String> {
    static KEY: Mutex<Option<Arc<DkimKey>>> = Mutex::new(None);
    let mut kept = lock(&KEY);
    let mut text = moqui_lib::env::get_prop_or_env(DKIM_KEY).trim().to_string();
    if let Some(key) = kept.as_ref().filter(|key| key.text == text) {
        return Ok(Some(Arc::clone(key)));
    }
    if text.is_empty() || text.ends_with("CHANGEME") {
        let Some(env) = lock(env_file()).clone() else {
            return Ok(None);
        };
        text = DkimKey::generate()?;
        moqui_lib::env::update_dotenv_file(&env, &[(DKIM_KEY.to_string(), text.clone())], &[])
            .map_err(|err| format!("a new {DKIM_KEY} was not written to {}: {err}", env.display()))?;
        moqui_lib::env::set_property(DKIM_KEY, text.clone());
        tracing::info!("Generated secret property {DKIM_KEY} (RSA-2048) into {}", env.display());
    }
    let key = Arc::new(DkimKey::parse(&text)?);
    *kept = Some(Arc::clone(&key));
    Ok(Some(key))
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

/// The domains of this node's hosted and shared accounts, the domains it signs for when `mail_dkim_domains` names
/// none: on an edge node from its copies of the local nodes' accounts.
///
/// # Errors
///
/// Returns why when the accounts could not be read.
pub fn hosted_domains(
    facade: &moqui_core::entity::EntityFacade,
    edge_node: bool,
) -> Result<std::collections::BTreeSet<String>, moqui_lib::error::Error> {
    use moqui_lib::object::Value;
    let mut addresses: Vec<String> = Vec::new();
    if edge_node {
        let values = moqui_core::edge::replica::list_values(
            facade,
            None,
            super::edge_mail::RECIPIENT_SET,
            Some(super::sync::ACCOUNT),
            1_000_000,
        )?;
        for value in values {
            if matches!(
                value.fields["accountKindEnumId"].as_str(),
                Some("EakHosted" | "EakShared")
            ) {
                addresses.push(value.fields["emailAddress"].as_str().unwrap_or_default().to_string());
            }
        }
    } else {
        let rows = facade
            .find(super::sync::ACCOUNT)?
            .condition(moqui_core::entity::Condition::is_in(
                "accountKindEnumId",
                vec![Value::string("EakHosted"), Value::string("EakShared")],
            ))
            .select_field("emailAddress")
            .disable_authz()
            .list()?;
        for row in rows.values() {
            addresses.push(moqui_core::service::util::field_text(row, "emailAddress"));
        }
    }
    Ok(addresses
        .iter()
        .filter_map(|address| address.trim().rsplit_once('@'))
        .map(|(_, domain)| domain.trim_end_matches('.').to_ascii_lowercase())
        .filter(|domain| !domain.is_empty())
        .collect())
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
        domains = hosted_domains(facade, moqui_core::edge::settings(&factory).edge_node)?
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
    let (_, domain) = address.trim().rsplit_once('@')?;
    let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    (!domain.is_empty()).then_some(domain)
}

/// The domain to sign `raw` for: its From domain, when it is one of `mail_dkim_domains`, or with none named, one that
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

/// How long [`is_hosted_domain`] keeps the hosted accounts' domains before reading them again.
const HOSTED_TTL: std::time::Duration = std::time::Duration::from_secs(60);

/// Whether `domain` is a hosted or shared account's on this node (see [`hosted_domains`]), as the current context's
/// factory reads it; false with no context or when the accounts cannot be read. The domains are kept a minute, so a
/// send does not read every account (an account added is signed for within the minute).
#[must_use]
pub fn is_hosted_domain(facade: &moqui_core::entity::EntityFacade, domain: &str) -> bool {
    type Kept = Option<(bool, std::time::Instant, std::collections::BTreeSet<String>)>;
    static KEPT: Mutex<Kept> = Mutex::new(None);
    let edge_node =
        moqui_core::context::ExecutionContext::with_current(|ec| moqui_core::edge::settings(ec.factory()).edge_node)
            .unwrap_or(false);
    {
        let kept = lock(&KEPT);
        if let Some((edge, at, domains)) = kept.as_ref() {
            if *edge == edge_node && at.elapsed() < HOSTED_TTL {
                return domains.contains(domain);
            }
        }
    }
    match hosted_domains(facade, edge_node) {
        Ok(domains) => {
            let hosted = domains.contains(domain);
            *lock(&KEPT) = Some((edge_node, std::time::Instant::now(), domains));
            hosted
        }
        Err(err) => {
            tracing::warn!("The hosted accounts' domains were not read: {err}");
            false
        }
    }
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

/// Whether `mail_dkim_key` is set (configured, or made earlier): a send by SMTP to another server is signed only then,
/// so a node never sends a signature of a key it has not published.
#[must_use]
pub fn key_configured() -> bool {
    let text = moqui_lib::env::get_prop_or_env(DKIM_KEY);
    let text = text.trim();
    !text.is_empty() && !text.ends_with("CHANGEME")
}

/// `raw` signed with the node's key when its From domain is one this node signs for (see [`signing_domain`]); as it is
/// when not, or when the node has no key. A key that cannot be had is logged and the message goes unsigned: a message
/// is not held for its signature.
#[must_use]
pub fn sign_for(raw: Vec<u8>, hosted: impl Fn(&str) -> bool) -> Vec<u8> {
    let Some(domain) = signing_domain(&raw, hosted) else {
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
/// `p=quarantine` is overridden when a passing ARC chain was last sealed by one of `mail_arc_trusted_sealers` (a
/// mailing list or forwarder this node trusts); with that property empty there is no ARC exception.
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

/// The domains whose ARC seal is trusted to carry a DMARC pass across a forwarder (`mail_arc_trusted_sealers`).
fn trusted_sealers() -> Vec<String> {
    moqui_lib::env::get_prop_or_env("mail_arc_trusted_sealers")
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
            return Verdict {
                field: format!("Authentication-Results: {authserv_id}; none (no resolver)\r\n"),
                summary: "no resolver".to_string(),
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
    if action != DmarcAction::Accept && arc.result() == &DkimResult::Pass {
        let sealer = arc
            .chain()
            .last()
            .map(|link| link.seal.header.d.to_ascii_lowercase())
            .unwrap_or_default();
        if !sealer.is_empty() && trusted_sealers().contains(&sealer) {
            action = DmarcAction::Accept;
            arc_override = format!(" (DMARC policy overridden by the trusted ARC sealer {sealer})");
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
    Verdict {
        field: results.to_header(),
        dmarc: action,
        summary,
        temp_error: false,
        domain: if dmarc.domain().is_empty() {
            from
        } else {
            dmarc.domain().to_string()
        },
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
