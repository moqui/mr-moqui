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

//! The DNS of mail (plans/Edge-Node.md §5.4, E5b): the resolver that SPF, DKIM, DMARC and ARC verification query
//! ([`super::auth`]), and the MX, A and AAAA lookups of the edge node's MX delivery ([`super::mx`]). E10's DNS checks
//! ([`super::edge_dns`]) use [`txt`], [`mx`] and [`ips`] too.
//!
//! The resolver is hickory's, built from the system's resolver configuration (Cloudflare when there is none), with its
//! answers kept in a [`Records`] cache until their TTL. It runs on a small Tokio runtime of this crate's own, so the
//! blocking callers here ([`run`]) and the host's async ones ([`on_runtime`]) share it. A [`Records`] can also hold
//! fixed records, which win over DNS: a test's, and the node's own DKIM key for internal mail. `mail_dns_offline`
//! (testing only) makes the resolver's cache answer every other name "not found" without a query.

use std::borrow::Borrow;
use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use mail_auth::dkim::DomainKey;
use mail_auth::dmarc::DmarcRecord;
use mail_auth::dns::TxtRecordParser;
use mail_auth::spf::SpfRecord;
use mail_auth::{DnsCache, MessageAuthenticator, Mx, Negative, RecordSet, ResolverCache, TxtRecord};
use moqui_lib::sync::lock;

/// How long one DNS query may take, per attempt.
const QUERY_TIMEOUT: Duration = Duration::from_secs(5);
/// The most entries one cache keeps; past it the expired ones go, and if that is not enough, all of them.
const MAX_ENTRIES: usize = 20_000;
/// The expiry of a fixed record: never, in practice.
const FIXED: Duration = Duration::from_secs(100 * 365 * 24 * 3600);

/// One record type's answers, each until it expires; `missing`, when set, is the answer for a name not kept (an offline
/// cache's "not found"), so no query is made.
pub struct Answers<K, V> {
    kept: Mutex<HashMap<K, (V, Instant)>>,
    missing: Option<V>,
}

impl<K, V> Default for Answers<K, V> {
    fn default() -> Self {
        Self {
            kept: Mutex::new(HashMap::new()),
            missing: None,
        }
    }
}

impl<K, V> Answers<K, V> {
    fn offline(missing: V) -> Self {
        Self {
            kept: Mutex::new(HashMap::new()),
            missing: Some(missing),
        }
    }
}

impl<K: Hash + Eq, V: Clone> ResolverCache<K, V> for Answers<K, V> {
    fn get<Q>(&self, name: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let map = lock(&self.kept);
        match map.get(name) {
            Some((value, expires)) if *expires > Instant::now() => Some(value.clone()),
            _ => self.missing.clone(),
        }
    }

    fn remove<Q>(&self, name: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        lock(&self.kept).remove(name).map(|(value, _)| value)
    }

    fn insert(&self, key: K, value: V, valid_until: Instant) {
        let mut map = lock(&self.kept);
        if map.len() >= MAX_ENTRIES {
            let now = Instant::now();
            map.retain(|_, (_, expires)| *expires > now);
            if map.len() >= MAX_ENTRIES {
                map.clear();
            }
        }
        map.insert(key, (value, valid_until));
    }
}

type Answer<T> = Result<RecordSet<T>, Negative>;

/// The DNS answers mail authentication and delivery read, by record type: the resolver's, kept until their TTL, and
/// fixed ones that never expire.
#[derive(Default)]
pub struct Records {
    txt: Answers<Box<str>, TxtRecord>,
    mx: Answers<Box<str>, Answer<Mx>>,
    ipv4: Answers<Box<str>, Answer<Ipv4Addr>>,
    ipv6: Answers<Box<str>, Answer<Ipv6Addr>>,
    ptr: Answers<IpAddr, Answer<Box<str>>>,
    /// The fixed TXT records' text, by name, which [`txt`] answers with.
    raw_txt: Mutex<HashMap<Box<str>, Vec<String>>>,
}

impl DnsCache for Records {
    type Txt = Answers<Box<str>, TxtRecord>;
    type Mx = Answers<Box<str>, Answer<Mx>>;
    type Ipv4 = Answers<Box<str>, Answer<Ipv4Addr>>;
    type Ipv6 = Answers<Box<str>, Answer<Ipv6Addr>>;
    type Ptr = Answers<IpAddr, Answer<Box<str>>>;

    fn txt(&self) -> Option<&Self::Txt> {
        Some(&self.txt)
    }

    fn mx(&self) -> Option<&Self::Mx> {
        Some(&self.mx)
    }

    fn ipv4(&self) -> Option<&Self::Ipv4> {
        Some(&self.ipv4)
    }

    fn ipv6(&self) -> Option<&Self::Ipv6> {
        Some(&self.ipv6)
    }

    fn ptr(&self) -> Option<&Self::Ptr> {
        Some(&self.ptr)
    }
}

/// A name as the cache keys it: lower case with the trailing dot.
fn fqdn(name: &str) -> Box<str> {
    let mut name = name.trim().trim_end_matches('.').to_ascii_lowercase();
    name.push('.');
    name.into_boxed_str()
}

impl Records {
    /// A cache that makes no query: a name not fixed here does not exist. For checks against known records only (the
    /// node's own key, a test's records).
    #[must_use]
    pub fn offline() -> Self {
        let code = hickory_resolver::proto::op::ResponseCode::NXDomain;
        let negative = Negative { code };
        Self {
            txt: Answers::offline(TxtRecord::Error(mail_auth::Error::Dns(
                mail_auth::DnsError::RecordNotFound(code),
            ))),
            mx: Answers::offline(Err(negative)),
            ipv4: Answers::offline(Err(negative)),
            ipv6: Answers::offline(Err(negative)),
            ptr: Answers::offline(Err(negative)),
            raw_txt: Mutex::new(HashMap::new()),
        }
    }

    /// A fixed TXT record at `name`: an SPF (`v=spf1`), DMARC (`v=DMARC1`) or DKIM key (`v=DKIM1`, or any other text at a
    /// `_domainkey` name) record, parsed as its kind.
    ///
    /// # Errors
    ///
    /// Returns why when the text does not parse as its kind.
    pub fn fix_txt(&self, name: &str, text: &str) -> Result<(), String> {
        let bytes = text.as_bytes();
        let lower = text.trim_start().to_ascii_lowercase();
        let record: TxtRecord = if lower.starts_with("v=spf1") {
            SpfRecord::parse(bytes).map(TxtRecord::from)
        } else if lower.starts_with("v=dmarc1") {
            DmarcRecord::parse(bytes).map(TxtRecord::from)
        } else {
            DomainKey::parse(bytes).map(TxtRecord::from)
        }
        .map_err(|err| format!("the TXT record of {name} does not parse: {err}"))?;
        self.txt.insert(fqdn(name), record, Instant::now() + FIXED);
        let mut raw = lock(&self.raw_txt);
        let texts = raw.entry(fqdn(name)).or_default();
        if !texts.iter().any(|kept| kept == text) {
            texts.push(text.to_string());
        }
        Ok(())
    }

    /// Fixed MX records of `name`: (preference, host); `.` alone is a null MX.
    pub fn fix_mx(&self, name: &str, hosts: &[(u16, &str)]) {
        let mut groups: Vec<Mx> = Vec::new();
        for (preference, host) in hosts {
            match groups.iter_mut().find(|group| group.preference == *preference) {
                Some(group) => {
                    let mut exchanges = group.exchanges.to_vec();
                    exchanges.push((*host).into());
                    group.exchanges = exchanges.into_boxed_slice();
                }
                None => groups.push(Mx {
                    exchanges: vec![Box::<str>::from(*host)].into_boxed_slice(),
                    preference: *preference,
                }),
            }
        }
        groups.sort_by_key(|group| group.preference);
        self.mx
            .insert(fqdn(name), Ok(record_set(groups)), Instant::now() + FIXED);
    }

    /// Fixed A and AAAA records of `name`.
    pub fn fix_ips(&self, name: &str, ips: &[IpAddr]) {
        let v4: Vec<Ipv4Addr> = ips
            .iter()
            .filter_map(|ip| match ip {
                IpAddr::V4(ip) => Some(*ip),
                IpAddr::V6(_) => None,
            })
            .collect();
        let v6: Vec<Ipv6Addr> = ips
            .iter()
            .filter_map(|ip| match ip {
                IpAddr::V6(ip) => Some(*ip),
                IpAddr::V4(_) => None,
            })
            .collect();
        let until = Instant::now() + FIXED;
        self.ipv4.insert(fqdn(name), Ok(record_set(v4)), until);
        self.ipv6.insert(fqdn(name), Ok(record_set(v6)), until);
    }
}

fn record_set<T>(records: Vec<T>) -> RecordSet<T> {
    RecordSet {
        records: records.into(),
        dnssec_status: mail_auth::DnssecStatus::Indeterminate,
    }
}

/// This crate's Tokio runtime, which the resolver runs on: four workers (inbound checks and MX lookups share it, each
/// waiting on the network, not the CPU), started at first use.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .thread_name("moqui-mail-dns")
            .enable_all()
            .build()
            .expect("a Tokio runtime for the mail resolver")
    })
}

/// Run `future` on this crate's runtime and wait for it, from a thread that is not a Tokio worker (a request worker, the
/// MX delivery thread).
///
/// # Errors
///
/// Returns why when the task panicked (a library's bug on hostile input): the caller treats it as not checked.
pub fn run<T: Send + 'static>(future: impl Future<Output = T> + Send + 'static) -> Result<T, String> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let task = runtime().spawn(future);
    runtime().spawn(async move {
        let _ = sender.send(task.await.map_err(|err| format!("a mail DNS task failed: {err}")));
    });
    receiver
        .recv()
        .map_err(|_| "the mail resolver's runtime stopped".to_string())?
}

/// Run `future` on this crate's runtime from async code on another runtime (the host's listener), so the resolver's
/// connections stay on the runtime that made them.
///
/// # Errors
///
/// Returns why when the task panicked.
pub async fn on_runtime<T: Send + 'static>(future: impl Future<Output = T> + Send + 'static) -> Result<T, String> {
    runtime()
        .spawn(future)
        .await
        .map_err(|err| format!("a mail DNS task failed: {err}"))
}

/// The resolver: the system's configuration, else Cloudflare's servers.
///
/// # Errors
///
/// Returns why when no resolver could be made.
pub fn authenticator() -> Result<&'static MessageAuthenticator, String> {
    static AUTHENTICATOR: OnceLock<Result<MessageAuthenticator, String>> = OnceLock::new();
    AUTHENTICATOR
        .get_or_init(|| {
            let _entered = runtime().enter();
            let (config, mut options) = match hickory_resolver::system_conf::read_system_conf() {
                Ok(conf) => conf,
                Err(err) => {
                    tracing::warn!("Mail DNS: the system's resolver configuration was not read ({err}); Cloudflare's servers are used");
                    (
                        hickory_resolver::config::ResolverConfig::udp_and_tcp(&hickory_resolver::config::CLOUDFLARE),
                        hickory_resolver::config::ResolverOpts::default(),
                    )
                }
            };
            options.timeout = QUERY_TIMEOUT;
            MessageAuthenticator::new(config, options).map_err(|err| format!("the mail resolver was not made: {err}"))
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// `mail_dns_offline` (testing only): no DNS query is made; a name not fixed in [`records`] does not exist.
fn offline() -> bool {
    moqui_lib::env::get_prop_or_env("mail_dns_offline").trim() == "true"
}

/// The answers of the resolver, kept until their TTL, with the fixed ones (a test's). Offline when
/// `mail_dns_offline` is `true` at the first use.
#[must_use]
pub fn records() -> &'static Records {
    static RECORDS: OnceLock<Records> = OnceLock::new();
    RECORDS.get_or_init(|| {
        if offline() {
            Records::offline()
        } else {
            Records::default()
        }
    })
}

/// The TXT records of `name`, each with its strings joined: the fixed ones when `name` has any.
///
/// # Errors
///
/// Returns why when the lookup failed; a name with no TXT records is an empty list.
pub fn txt(name: &str) -> Result<Vec<String>, String> {
    if let Some(fixed) = lock(&records().raw_txt).get(&fqdn(name)) {
        return Ok(fixed.clone());
    }
    if offline() {
        return Ok(Vec::new());
    }
    let resolver = authenticator()?;
    let name = name.to_string();
    run(async move {
        let key = hickory_resolver::proto::rr::Name::from_str_relaxed(name.as_str())
            .map_err(|err| format!("{name} is not a DNS name: {err}"))?;
        match resolver.resolver().txt_lookup(key).await {
            Ok(lookup) => Ok(lookup
                .answers()
                .iter()
                .filter_map(|record| match &record.data {
                    hickory_resolver::proto::rr::RData::TXT(txt) => Some(
                        txt.txt_data
                            .iter()
                            .map(|part| String::from_utf8_lossy(part).into_owned())
                            .collect::<String>(),
                    ),
                    _ => None,
                })
                .collect()),
            Err(err) if err.is_no_records_found() => Ok(Vec::new()),
            Err(err) => Err(format!("the TXT records of {name}: {err}")),
        }
    })?
}

/// The MX hosts of `domain`, most preferred first, with their preferences; empty when it has none.
///
/// # Errors
///
/// Returns why when the lookup failed.
pub fn mx(domain: &str) -> Result<Vec<(u16, String)>, String> {
    let resolver = authenticator()?;
    let domain = domain.to_string();
    run(async move { mx_async(resolver, &domain).await })?
}

async fn mx_async(resolver: &MessageAuthenticator, domain: &str) -> Result<Vec<(u16, String)>, String> {
    match resolver.mx_lookup(domain, Some(&records().mx)).await {
        Ok(set) => Ok(set
            .records
            .iter()
            .flat_map(|group| {
                group
                    .exchanges
                    .iter()
                    .map(move |host| (group.preference, host.trim_end_matches('.').to_string()))
            })
            .collect()),
        Err(mail_auth::Error::Dns(mail_auth::DnsError::RecordNotFound(_))) => Ok(Vec::new()),
        Err(err) => Err(format!("the MX records of {domain}: {err}")),
    }
}

/// The addresses of `host`, A then AAAA; empty when it has none.
///
/// # Errors
///
/// Returns why when no address came back and a lookup failed with something other than "not found".
pub fn ips(host: &str) -> Result<Vec<IpAddr>, String> {
    let resolver = authenticator()?;
    let host = host.to_string();
    run(async move {
        let v4 = resolver.ipv4_lookup(host.as_str(), Some(&records().ipv4)).await;
        let v6 = resolver.ipv6_lookup(host.as_str(), Some(&records().ipv6)).await;
        let v4 = v4.map(|set| set.records.iter().map(|ip| IpAddr::V4(*ip)).collect());
        let v6 = v6.map(|set| set.records.iter().map(|ip| IpAddr::V6(*ip)).collect());
        combine_ips(&host, v4, v6)
    })?
}

/// The A and AAAA answers together. A lookup that failed (a timeout, SERVFAIL) is an error when no address came back,
/// so a domain is never refused for good on a transient failure; "not found" for both is an empty list.
pub(crate) fn combine_ips(
    host: &str,
    v4: mail_auth::Result<Vec<IpAddr>>,
    v6: mail_auth::Result<Vec<IpAddr>>,
) -> Result<Vec<IpAddr>, String> {
    let not_found =
        |err: &mail_auth::Error| matches!(err, mail_auth::Error::Dns(mail_auth::DnsError::RecordNotFound(_)));
    let mut out: Vec<IpAddr> = Vec::new();
    let mut failed: Option<String> = None;
    for answer in [v4, v6] {
        match answer {
            Ok(ips) => out.extend(ips),
            Err(err) if not_found(&err) => {}
            Err(err) => failed = Some(format!("the addresses of {host}: {err}")),
        }
    }
    match failed {
        Some(err) if out.is_empty() => Err(err),
        _ => Ok(out),
    }
}
