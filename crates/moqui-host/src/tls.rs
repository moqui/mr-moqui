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

//! Certificates for the public HTTPS listener, looked up by the domain a client asks for (SNI) in `runtime/cert`, and
//! for the SMTP listener's STARTTLS ([`smtp_server_config`]).
//!
//! Layouts, per domain `example.com`:
//! - nginx-proxy: `example.com.crt` (or `.pem`, `.cer`; extra certificates in the file are the chain) and
//!   `example.com.key`. `dhparam.pem` and `example.com.dhparam.pem` are ignored: rustls only does ECDHE.
//! - certbot and acme-companion: `example.com/fullchain.pem` (or `cert.pem`) and `example.com/privkey.pem` (or
//!   `key.pem`).
//! - mkcert: `example.com.pem` and `example.com-key.pem`.
//!
//! A request for `shop.example.com` tries `shop.example.com`, then `_wildcard.example.com` (mkcert's name for
//! `*.example.com`), then `example.com` (nginx-proxy's name for a wildcard), then `default`, which also serves a client
//! that sends no name, such as one connecting by IP address. A wildcard covers one label (RFC 6125 6.4.3), so no
//! higher parent is tried. With none of them the handshake is refused. Keys may be PKCS#8, PKCS#1 or SEC1 PEM.
//!
//! The directory is read again at most every [`RECHECK`], so a renewed or added certificate is served without a
//! restart. A pair whose files changed but do not load keeps serving the one loaded before.
//!
//! In dev (`instance_purpose=dev`) an empty directory gets a self-signed `localhost` certificate and a `default` copy.
//!
//! The SMTP listener looks up the name the gateway asks for, then its own `smtp-server` `hostname` (a gateway that
//! connects by address sends no name), then `default`; or it serves the files `tls-cert-file` and `tls-key-file` name,
//! read again when they change.

// A host built with inbound-smtp but not axum has STARTTLS and no HTTPS listener.
#![cfg_attr(not(feature = "axum"), allow(dead_code))]

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock, TryLockError};
use std::time::{Duration, Instant, SystemTime};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use moqui_lib::error::Error;
use moqui_lib::sync;
use rustls::crypto::aws_lc_rs;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;

const DEFAULT_PORT: u16 = 8443;
/// The port this process listens on. `webapp_https_port` is the port clients see in generated URLs, which differs
/// behind a reverse proxy.
const LISTEN_PORT: &str = "webapp_https_listen_port";
/// How long the directory listing is trusted before it is read again.
const RECHECK: Duration = Duration::from_secs(5);
/// One warning about a name with no certificate in this time; the others go to debug, as scanners send many.
const MISS_WARNING_EVERY: Duration = Duration::from_secs(60);
const CERT_EXTENSIONS: [&str; 3] = ["crt", "pem", "cer"];

/// What `lib.rs` hands to the listener: where to bind and how to answer handshakes.
pub(crate) struct HttpsListen {
    pub bind: SocketAddr,
    pub config: Arc<rustls::ServerConfig>,
    /// The certificates `config` serves, which an edge node's link settings share (`edge_link`).
    pub certs: Arc<CertStore>,
}

/// The HTTPS listener to start on `ip`, or `None` when it is off or, by default, when no certificate exists. A port
/// set explicitly with no certificate is an error.
pub(crate) fn plan(runtime_dir: &Path, ip: IpAddr, use_default_port: bool) -> Result<Option<HttpsListen>, Error> {
    let raw = moqui_lib::env::get_prop_or_env(LISTEN_PORT);
    let raw = raw.trim();
    let Some(port) = listen_port(raw, use_default_port)? else {
        return Ok(None);
    };
    let dir = runtime_dir.join("cert");
    if moqui_lib::env::get_prop_or_env("instance_purpose") == "dev" {
        if let Err(err) = ensure_dev_cert(&dir) {
            tracing::warn!(
                "Could not generate a development certificate in {}: {err}",
                dir.display()
            );
        }
    }
    let store = CertStore::new(dir.clone());
    if store.is_empty() {
        let missing = format!(
            "no usable certificate in {}; add <domain>.crt and <domain>.key there",
            dir.display()
        );
        if !raw.is_empty() {
            return Err(Error::new(format!("{LISTEN_PORT} is {raw}, but there is {missing}")));
        }
        tracing::warn!("The HTTPS listener is not started: {missing}");
        return Ok(None);
    }
    let certs = Arc::new(store);
    Ok(Some(HttpsListen {
        bind: SocketAddr::new(ip, port),
        config: Arc::new(server_config(Arc::clone(&certs))?),
        certs,
    }))
}

/// The port [`plan`] would bind, without generating a certificate: an explicit port, or by default 8443 when a
/// certificate exists or a dev instance will generate one. None when the listener stays off.
pub(crate) fn planned_port(runtime_dir: &Path, use_default_port: bool) -> Result<Option<u16>, Error> {
    let raw = moqui_lib::env::get_prop_or_env(LISTEN_PORT);
    let raw = raw.trim();
    let Some(port) = listen_port(raw, use_default_port)? else {
        return Ok(None);
    };
    if !raw.is_empty() || moqui_lib::env::get_prop_or_env("instance_purpose") == "dev" {
        return Ok(Some(port));
    }
    Ok((!CertStore::new(runtime_dir.join("cert")).is_empty()).then_some(port))
}

/// `webapp_https_listen_port`: empty is 8443 when `use_default_port` (tests bind their own HTTP address and leave it
/// off so they do not take 8443), `0` is ephemeral, and `off` turns the listener off.
fn listen_port(raw: &str, use_default_port: bool) -> Result<Option<u16>, Error> {
    match raw.trim() {
        "" => Ok(use_default_port.then_some(DEFAULT_PORT)),
        "off" => Ok(None),
        raw => raw
            .parse::<u16>()
            .map(Some)
            .map_err(|_| Error::new(format!("{LISTEN_PORT} {raw} is not a port or off"))),
    }
}

/// The listener's TLS settings on `store`, which may be shared: an edge node's link (ALPN `moqui-edge/1`) has settings
/// of its own on the same certificates, chosen by the ClientHello before the handshake (`edge_link`), so a browser is
/// never asked for a client certificate. ALPN offers `h2` then `http/1.1` ([`web_alpn`]); the connection is served as
/// the protocol the handshake chose (`crate::http::serve`).
pub(crate) fn server_config(store: impl Into<Arc<CertStore>>) -> Result<rustls::ServerConfig, Error> {
    let store: Arc<CertStore> = store.into();
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|err| Error::new(format!("could not set up the HTTPS listener: {err}")))?
        .with_no_client_auth()
        .with_cert_resolver(store);
    config.alpn_protocols = web_alpn(http2_enabled());
    Ok(config)
}

/// `webapp_http2`: HTTP/2 on the HTTPS listener, on unless `false`.
pub(crate) fn http2_enabled() -> bool {
    !moqui_lib::env::get_prop_or_env("webapp_http2")
        .trim()
        .eq_ignore_ascii_case("false")
}

/// The HTTPS listener's ALPN: `h2` first when `http2`, then `http/1.1`. A client that offers none gets HTTP/1.1.
pub(crate) fn web_alpn(http2: bool) -> Vec<Vec<u8>> {
    if http2 {
        vec![b"h2".to_vec(), b"http/1.1".to_vec()]
    } else {
        vec![b"http/1.1".to_vec()]
    }
}

/// The pairs in the directory by lowercase name, as of `read`. Lookups never touch the file system, so a name a
/// client makes up costs no file access and no memory.
struct Index {
    pairs: HashMap<String, Pair>,
    read: Instant,
}

struct Pair {
    cert: PathBuf,
    key: PathBuf,
    stamp: Stamp,
    /// `None` when the files have never loaded.
    loaded: Option<Arc<CertifiedKey>>,
}

/// Modification time and length of the certificate and key files, to notice a renewal.
type Stamp = [Option<(SystemTime, u64)>; 2];

pub(crate) struct CertStore {
    dir: PathBuf,
    index: RwLock<Arc<Index>>,
    /// Held by the one handshake that reads the directory again; the others use the index they have.
    refreshing: Mutex<()>,
    last_miss_warning: Mutex<Option<Instant>>,
}

impl std::fmt::Debug for CertStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertStore")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

impl CertStore {
    pub(crate) fn new(dir: PathBuf) -> Self {
        let index = scan(&dir, None);
        Self {
            dir,
            index: RwLock::new(Arc::new(index)),
            refreshing: Mutex::new(()),
            last_miss_warning: Mutex::new(None),
        }
    }

    /// Whether no pair in the directory loads.
    fn is_empty(&self) -> bool {
        self.index().pairs.values().all(|pair| pair.loaded.is_none())
    }

    /// The certificate for a requested name, or `None` (the handshake then fails).
    fn lookup(&self, server_name: Option<&str>) -> Option<Arc<CertifiedKey>> {
        let index = self.index();
        candidates(server_name)
            .iter()
            .find_map(|name| index.pairs.get(name)?.loaded.clone())
    }

    /// The certificate for a requested name, else for `fallback`, else `default`.
    #[cfg(feature = "inbound-smtp")]
    fn lookup_or(&self, server_name: Option<&str>, fallback: &str) -> Option<Arc<CertifiedKey>> {
        let index = self.index();
        let mut names = candidates(server_name);
        names.retain(|name| name != "default");
        names.extend(candidates(Some(fallback)));
        names.iter().find_map(|name| index.pairs.get(name)?.loaded.clone())
    }

    fn index(&self) -> Arc<Index> {
        let index = Arc::clone(&sync::read(&self.index));
        if index.read.elapsed() < RECHECK {
            return index;
        }
        let _refreshing = match self.refreshing.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return index,
        };
        // Another handshake may have just read it.
        let index = Arc::clone(&sync::read(&self.index));
        if index.read.elapsed() < RECHECK {
            return index;
        }
        let fresh = Arc::new(scan(&self.dir, Some(&index)));
        *sync::write(&self.index) = Arc::clone(&fresh);
        fresh
    }

    fn log_miss(&self, name: &str) {
        let mut last = sync::lock(&self.last_miss_warning);
        if last.is_some_and(|at| at.elapsed() < MISS_WARNING_EVERY) {
            tracing::debug!(
                "No certificate in {} for {name}; refusing the TLS handshake",
                self.dir.display()
            );
        } else {
            *last = Some(Instant::now());
            tracing::warn!(
                "No certificate in {} for {name}; refusing the TLS handshake (others in the next minute are logged at debug)",
                self.dir.display()
            );
        }
    }
}

impl ResolvesServerCert for CertStore {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let name = hello.server_name();
        let found = self.lookup(name);
        if found.is_none() {
            self.log_miss(name.unwrap_or("(no server name)"));
        }
        found
    }
}

/// The SMTP listener's TLS (STARTTLS): the explicit `cert_file` and `key_file` (absolute, or under the runtime
/// directory), or else `runtime/cert` by the name the client asks for or `hostname`. No ALPN. With no certificate it is
/// an error, and the listener does not start: STARTTLS that cannot finish would leave the mail unprotected.
#[cfg(feature = "inbound-smtp")]
pub(crate) fn smtp_server_config(
    runtime_dir: &Path,
    hostname: &str,
    cert_file: &str,
    key_file: &str,
) -> Result<rustls::ServerConfig, Error> {
    let resolver: Arc<dyn ResolvesServerCert> = if cert_file.is_empty() && key_file.is_empty() {
        let dir = runtime_dir.join("cert");
        if moqui_lib::env::get_prop_or_env("instance_purpose") == "dev" {
            if let Err(err) = ensure_dev_cert(&dir) {
                tracing::warn!(
                    "Could not generate a development certificate in {}: {err}",
                    dir.display()
                );
            }
        }
        let store = CertStore::new(dir.clone());
        if store.lookup_or(None, hostname).is_none() {
            return Err(Error::new(format!(
                "no certificate for {hostname} or default in {}; add {hostname}.crt and {hostname}.key there, or set \
                 smtp-server tls-cert-file and tls-key-file",
                dir.display()
            )));
        }
        Arc::new(SmtpCerts {
            store,
            hostname: hostname.to_string(),
        })
    } else {
        if cert_file.is_empty() || key_file.is_empty() {
            return Err(Error::new(
                "smtp-server tls-cert-file and tls-key-file are set together or not at all",
            ));
        }
        let path = |file: &str| {
            let file = file.strip_prefix("runtime://").unwrap_or(file);
            let path = Path::new(file);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                runtime_dir.join(path)
            }
        };
        Arc::new(FilePair::new(path(cert_file), path(key_file))?)
    };
    rustls::ServerConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|err| Error::new(format!("could not set up the SMTP listener's TLS: {err}")))
        .map(|builder| builder.with_no_client_auth().with_cert_resolver(resolver))
}

/// `runtime/cert` for the SMTP listener: the name asked for, then the listener's hostname.
#[cfg(feature = "inbound-smtp")]
#[derive(Debug)]
struct SmtpCerts {
    store: CertStore,
    hostname: String,
}

#[cfg(feature = "inbound-smtp")]
impl ResolvesServerCert for SmtpCerts {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let name = hello.server_name();
        let found = self.store.lookup_or(name, &self.hostname);
        if found.is_none() {
            self.store.log_miss(name.unwrap_or(&self.hostname));
        }
        found
    }
}

/// A certificate and key by path, read again (at most every [`RECHECK`]) when a file changed; a change that does not
/// load keeps serving the pair loaded before.
#[cfg(feature = "inbound-smtp")]
#[derive(Debug)]
struct FilePair {
    cert: PathBuf,
    key: PathBuf,
    loaded: RwLock<Loaded>,
}

#[cfg(feature = "inbound-smtp")]
#[derive(Debug)]
struct Loaded {
    stamp: Stamp,
    checked: Instant,
    key: Arc<CertifiedKey>,
}

#[cfg(feature = "inbound-smtp")]
impl FilePair {
    fn new(cert: PathBuf, key: PathBuf) -> Result<Self, Error> {
        let stamp = [stamp(&cert), stamp(&key)];
        let loaded = read_certified_key(&cert, &key)?;
        Ok(Self {
            cert,
            key,
            loaded: RwLock::new(Loaded {
                stamp,
                checked: Instant::now(),
                key: Arc::new(loaded),
            }),
        })
    }

    fn current(&self) -> Arc<CertifiedKey> {
        {
            let loaded = sync::read(&self.loaded);
            if loaded.checked.elapsed() < RECHECK {
                return Arc::clone(&loaded.key);
            }
        }
        let mut loaded = sync::write(&self.loaded);
        if loaded.checked.elapsed() >= RECHECK {
            loaded.checked = Instant::now();
            let stamp = [stamp(&self.cert), stamp(&self.key)];
            if stamp != loaded.stamp {
                loaded.stamp = stamp;
                match read_certified_key(&self.cert, &self.key) {
                    Ok(key) => {
                        tracing::info!(
                            "The SMTP listener serves the changed certificate {}",
                            self.cert.display()
                        );
                        loaded.key = Arc::new(key);
                    }
                    Err(err) => tracing::warn!(
                        "Certificate {} is not usable: {err}; still serving the one loaded before",
                        self.cert.display()
                    ),
                }
            }
        }
        Arc::clone(&loaded.key)
    }
}

#[cfg(feature = "inbound-smtp")]
impl ResolvesServerCert for FilePair {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.current())
    }
}

/// The names to try for a requested host, best first.
fn candidates(server_name: Option<&str>) -> Vec<String> {
    let mut names = Vec::new();
    let host = server_name
        .map(|name| name.trim_end_matches('.').to_ascii_lowercase())
        .unwrap_or_default();
    if !host.is_empty() {
        let parent = host
            .split_once('.')
            .map(|(_, parent)| parent.to_string())
            .filter(|parent| parent.contains('.'));
        names.push(host);
        if let Some(parent) = parent {
            names.push(format!("_wildcard.{parent}"));
            names.push(parent);
        }
    }
    names.push("default".to_string());
    names
}

/// The pairs in `dir`. One whose files are unchanged since `previous` keeps its loaded key without a read, and one
/// whose files changed but do not load keeps the key loaded before.
fn scan(dir: &Path, previous: Option<&Index>) -> Index {
    let mut pairs = HashMap::new();
    for name in pair_names(dir) {
        let Some((cert, key)) = find_files(dir, &name) else {
            continue;
        };
        let stamp = [stamp(&cert), stamp(&key)];
        let name = name.to_ascii_lowercase();
        let before = previous.and_then(|index| index.pairs.get(&name));
        let loaded = match before {
            Some(before) if before.stamp == stamp && before.cert == cert && before.key == key => before.loaded.clone(),
            _ => match read_certified_key(&cert, &key) {
                Ok(loaded) => Some(Arc::new(loaded)),
                Err(err) => {
                    let kept = before.and_then(|before| before.loaded.clone());
                    let serving = if kept.is_some() {
                        "; still serving the one loaded before"
                    } else {
                        ""
                    };
                    tracing::warn!("Certificate {} is not usable: {err}{serving}", cert.display());
                    kept
                }
            },
        };
        pairs.insert(
            name,
            Pair {
                cert,
                key,
                stamp,
                loaded,
            },
        );
    }
    Index {
        pairs,
        read: Instant::now(),
    }
}

fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

/// The names in `dir` that may have a pair: each `<name>.key`, `<name>-key.pem` and subdirectory.
fn pair_names(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let file = entry.file_name().into_string().ok()?;
            if entry.path().is_dir() {
                return Some(file);
            }
            file.strip_suffix(".key")
                .or_else(|| file.strip_suffix("-key.pem"))
                .map(str::to_string)
        })
        .collect()
}

/// The certificate and key files for `name`, in any of the layouts.
pub(crate) fn find_files(dir: &Path, name: &str) -> Option<(PathBuf, PathBuf)> {
    let key = dir.join(format!("{name}.key"));
    if key.is_file() {
        return CERT_EXTENSIONS
            .iter()
            .map(|extension| dir.join(format!("{name}.{extension}")))
            .find(|cert| cert.is_file())
            .map(|cert| (cert, key));
    }
    let key = dir.join(format!("{name}-key.pem"));
    let cert = dir.join(format!("{name}.pem"));
    if key.is_file() && cert.is_file() {
        return Some((cert, key));
    }
    let sub = dir.join(name);
    if sub.is_dir() {
        let cert = ["fullchain.pem", "cert.pem"]
            .iter()
            .map(|f| sub.join(f))
            .find(|p| p.is_file())?;
        let key = ["privkey.pem", "key.pem"]
            .iter()
            .map(|f| sub.join(f))
            .find(|p| p.is_file())?;
        return Some((cert, key));
    }
    None
}

fn read_certified_key(cert: &Path, key: &Path) -> Result<CertifiedKey, Error> {
    let cert_bytes =
        std::fs::read(cert).map_err(|err| Error::new(format!("could not read {}: {err}", cert.display())))?;
    let chain = CertificateDer::pem_slice_iter(&cert_bytes)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| Error::new(format!("{} has no usable PEM certificate: {err}", cert.display())))?;
    if chain.is_empty() {
        return Err(Error::new(format!("{} has no PEM certificate", cert.display())));
    }
    let key_bytes = std::fs::read(key).map_err(|err| Error::new(format!("could not read {}: {err}", key.display())))?;
    let der = PrivateKeyDer::from_pem_slice(&key_bytes)
        .map_err(|err| Error::new(format!("{} has no usable PEM private key: {err}", key.display())))?;
    CertifiedKey::from_der(chain, der, &aws_lc_rs::default_provider())
        .map_err(|err| Error::new(format!("{} does not match its key: {err}", cert.display())))
}

/// Whether the directory holds a certificate and key pair in any layout.
fn has_any_pair(dir: &Path) -> bool {
    pair_names(dir).iter().any(|name| find_files(dir, name).is_some())
}

/// The leaf certificate a handshake asking `dir`'s certificates for `name` is served, as the edge link's XMPP probe
/// pins it (`edge_link::probe`); none when no pair there answers for the name.
#[cfg_attr(not(feature = "edge"), allow(dead_code))]
pub(crate) fn leaf_for(dir: &Path, name: &str) -> Option<CertificateDer<'static>> {
    CertStore::new(dir.to_path_buf())
        .lookup(Some(name))?
        .cert
        .first()
        .cloned()
}

/// A self-signed `localhost` certificate plus `default` copies, when the directory has no pair at all.
pub(crate) fn ensure_dev_cert(dir: &Path) -> Result<(), Error> {
    if has_any_pair(dir) {
        return Ok(());
    }
    let fail = |what: &str, err: &dyn std::fmt::Display| Error::new(format!("{what}: {err}"));
    std::fs::create_dir_all(dir).map_err(|err| fail("could not create the cert directory", &err))?;
    let key = rcgen::KeyPair::generate().map_err(|err| fail("could not generate a key", &err))?;
    let mut params = rcgen::CertificateParams::new(vec!["localhost".to_string()])
        .map_err(|err| fail("could not set up the certificate", &err))?;
    params.subject_alt_names.extend([
        rcgen::SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        rcgen::SanType::IpAddress(IpAddr::V6(Ipv6Addr::LOCALHOST)),
    ]);
    let mut name = rcgen::DistinguishedName::new();
    name.push(rcgen::DnType::CommonName, "localhost");
    params.distinguished_name = name;
    let cert = params
        .self_signed(&key)
        .map_err(|err| fail("could not sign the certificate", &err))?;
    let cert_pem = pem("CERTIFICATE", cert.der());
    let key_pem = pem("PRIVATE KEY", &key.serialize_der());
    for stem in ["localhost", "default"] {
        std::fs::write(dir.join(format!("{stem}.crt")), &cert_pem)
            .map_err(|err| fail("could not write the certificate", &err))?;
        write_private(&dir.join(format!("{stem}.key")), &key_pem)?;
    }
    tracing::info!(
        "Generated a self-signed development certificate for localhost in {}",
        dir.display()
    );
    Ok(())
}

fn write_private(path: &Path, text: &str) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|err| Error::new(format!("could not write {}: {err}", path.display())))?;
        file.write_all(text.as_bytes())
            .map_err(|err| Error::new(format!("could not write {}: {err}", path.display())))
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, text).map_err(|err| Error::new(format!("could not write {}: {err}", path.display())))
    }
}

fn pem(label: &str, der: &[u8]) -> String {
    let body = STANDARD.encode(der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for line in body.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(line).unwrap_or_default());
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

#[cfg(test)]
#[path = "tls_tests.rs"]
pub(crate) mod tests;
