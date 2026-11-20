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
//! In dev (`moqui_conf__instance__purpose=dev`) an empty directory gets a self-signed `localhost` certificate and a `default` copy.
//!
//! A key need not be a file (plans/Sealed-Secrets.md §3.1, S4): a certificate whose key file is not there is served
//! with a key held in memory, an ACME certificate's from its encrypted row ([`hold_key`]) or one an operator put with
//! `moqui operator secret put tls-key/<name>`; the development certificate's key is put there too. A key file that is
//! there still serves, and is named once in a warning: it is a usable key on disk.
//!
//! The SMTP listener looks up the name the gateway asks for, then its own `smtp-server` `hostname` (a gateway that
//! connects by address sends no name), then `default`; or it serves the files `tls-cert-file` and `tls-key-file` name,
//! read again when they change.

// A host built with inbound-smtp but not axum has STARTTLS and no HTTPS listener.
#![cfg_attr(not(feature = "axum"), allow(dead_code))]

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, RwLock, TryLockError};
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
use sha2::Digest as _;

const DEFAULT_PORT: u16 = 8443;
/// What sets the port this process listens on, for its messages: the webroot webapp's `https-listen-port`. Its
/// `https-port` is the port clients see in generated URLs, which differs behind a reverse proxy.
const LISTEN_PORT: &str = "https-listen-port";
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
    /// No certificate loads: the listener binds only once a name is set up with ACME, and then answers only its
    /// challenges until a certificate is there (`crate::acme`). A local node's passthrough tunnel uses `config` the
    /// same way.
    pub standby: bool,
}

/// The HTTPS listener to start on `ip`, or `None` when it is off. With no certificate it stands by
/// ([`HttpsListen::standby`]): a name with no certificate gets no HTTPS, and with none at all nothing listens until a
/// name is set up with ACME.
pub(crate) fn plan(
    runtime_dir: &Path,
    ip: IpAddr,
    use_default_port: bool,
    listen_port_text: &str,
) -> Result<Option<HttpsListen>, Error> {
    let raw = listen_port_text.trim();
    let Some(port) = listen_port(raw, use_default_port)? else {
        return Ok(None);
    };
    let dir = runtime_dir.join("cert");
    if moqui_core::context::conf_env::instance_purpose() == "dev" {
        if let Err(err) = ensure_dev_cert(&dir) {
            tracing::warn!(
                "Could not generate a development certificate in {}: {err}",
                dir.display()
            );
        }
    }
    let store = CertStore::new(dir.clone());
    let standby = store.is_empty();
    if standby {
        tracing::warn!(
            "The HTTPS listener is not started: no usable certificate in {}; add <domain>.crt and <domain>.key there, \
             or set a name up with ACME on System › Certificates, which starts it",
            dir.display()
        );
    }
    let certs = Arc::new(store);
    Ok(Some(HttpsListen {
        bind: SocketAddr::new(ip, port),
        config: Arc::new(server_config(Arc::clone(&certs))?),
        certs,
        standby,
    }))
}

/// The port [`plan`] would bind, without generating a certificate: an explicit port, or by default 8443 when a
/// certificate exists or a dev instance will generate one. None when the listener stays off.
pub(crate) fn planned_port(
    runtime_dir: &Path,
    use_default_port: bool,
    listen_port_text: &str,
) -> Result<Option<u16>, Error> {
    let raw = listen_port_text.trim();
    let Some(port) = listen_port(raw, use_default_port)? else {
        return Ok(None);
    };
    if !raw.is_empty() || moqui_core::context::conf_env::instance_purpose() == "dev" {
        return Ok(Some(port));
    }
    Ok((!CertStore::new(runtime_dir.join("cert")).is_empty()).then_some(port))
}

/// `webapp-list/webapp[webroot]@https-listen-port`: empty is 8443 when `use_default_port` (tests bind their own HTTP address and leave it
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

/// The webroot webapp's `http2`: HTTP/2 on the HTTPS listener, on unless `false`.
pub(crate) fn http2_enabled() -> bool {
    !moqui_core::context::conf_env::webroot_setting("http2").eq_ignore_ascii_case("false")
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
    /// The key file; empty for a key held in memory.
    key: PathBuf,
    stamp: Stamp,
    /// SHA-256 of a key held in memory, to notice a new one.
    held: Option<[u8; 32]>,
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
        if moqui_core::context::conf_env::instance_purpose() == "dev" {
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
        let loaded = read_certified_key(&cert, &KeyFrom::File(key.clone()))?;
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
                match read_certified_key(&self.cert, &KeyFrom::File(self.key.clone())) {
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
        let Some((cert, from)) = find_pair(dir, &name) else {
            continue;
        };
        let (key, held) = match &from {
            KeyFrom::File(key) => {
                warn_key_file(key, &name);
                (key.clone(), None)
            }
            KeyFrom::Held(pem) => (PathBuf::new(), Some(sha2::Sha256::digest(pem.as_bytes()).into())),
        };
        let stamp = [stamp(&cert), stamp(&key)];
        let name = name.to_ascii_lowercase();
        let before = previous.and_then(|index| index.pairs.get(&name));
        let loaded = match before {
            Some(before)
                if before.stamp == stamp && before.cert == cert && before.key == key && before.held == held =>
            {
                before.loaded.clone()
            }
            _ => match read_certified_key(&cert, &from) {
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
                held,
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

/// The names in `dir` that may have a pair: each `<name>.key`, `<name>-key.pem`, certificate file and subdirectory.
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
                .or_else(|| {
                    CERT_EXTENSIONS
                        .iter()
                        .find_map(|extension| file.strip_suffix(&format!(".{extension}")))
                })
                .map(str::to_string)
        })
        .collect::<HashSet<String>>()
        .into_iter()
        .collect()
}

/// The private keys held in memory by certificate directory and lower-case name: an ACME certificate's, from its
/// encrypted row, which is never written to the directory.
static HELD: LazyLock<RwLock<HeldKeys>> = LazyLock::new(RwLock::default);

/// The held keys' PEMs by directory and name.
type HeldKeys = HashMap<(PathBuf, String), zeroize::Zeroizing<String>>;

/// The slot of `dir`'s pair `name` in [`HELD`]: the directory as the file system resolves it (one spelling may go through
/// a link, as `/var` does on macOS), the name in lower case.
fn held_slot(dir: &Path, name: &str) -> (PathBuf, String) {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    (dir, name.to_ascii_lowercase())
}

/// Drop every key held in memory: the seal drops the keys (plans/Sealed-Secrets.md §3.5); the next unsealed start's
/// ACME sync holds them again from their rows.
pub(crate) fn drop_held_keys() {
    sync::write(&HELD).clear();
}

/// Hold `key_pem` as the key of `dir`'s pair `name` (whose certificate is a file there), or with `None` drop it.
pub(crate) fn hold_key(dir: &Path, name: &str, key_pem: Option<&str>) {
    let slot = held_slot(dir, name);
    match key_pem {
        Some(pem) => {
            sync::write(&HELD).insert(slot, zeroize::Zeroizing::new(pem.to_string()));
        }
        None => {
            sync::write(&HELD).remove(&slot);
        }
    }
}

/// The key of `dir`'s pair `name` that is not a file: held ([`hold_key`]), else the secret `tls-key/<name>` an operator
/// put.
fn held_key(dir: &Path, name: &str) -> Option<zeroize::Zeroizing<String>> {
    let slot = held_slot(dir, name);
    if let Some(held) = sync::read(&HELD).get(&slot) {
        return Some(held.clone());
    }
    moqui_core::seal::secrets::named(&format!("{}{name}", moqui_core::seal::secrets::TLS_KEY_PREFIX))
}

/// Where a pair's key is.
pub(crate) enum KeyFrom {
    File(PathBuf),
    Held(zeroize::Zeroizing<String>),
}

impl KeyFrom {
    /// The key's PEM.
    pub(crate) fn read(&self) -> Result<zeroize::Zeroizing<Vec<u8>>, Error> {
        match self {
            Self::File(key) => std::fs::read(key)
                .map(zeroize::Zeroizing::new)
                .map_err(|err| Error::new(format!("could not read {}: {err}", key.display()))),
            Self::Held(pem) => Ok(zeroize::Zeroizing::new(pem.as_bytes().to_vec())),
        }
    }
}

/// The certificate and key of `name` in `dir`: a pair of files in any layout ([`find_files`]), else a certificate file
/// whose key is held in memory ([`held_key`]).
pub(crate) fn find_pair(dir: &Path, name: &str) -> Option<(PathBuf, KeyFrom)> {
    if let Some((cert, key)) = find_files(dir, name) {
        return Some((cert, KeyFrom::File(key)));
    }
    let sub = dir.join(name);
    let cert = ["fullchain.pem", "cert.pem"]
        .iter()
        .map(|file| sub.join(file))
        .chain(
            CERT_EXTENSIONS
                .iter()
                .map(|extension| dir.join(format!("{name}.{extension}"))),
        )
        .find(|path| path.is_file())?;
    Some((cert, KeyFrom::Held(held_key(dir, name)?)))
}

/// Name a key file once in a warning: a usable key on disk, which `operator secret put` takes in its place.
fn warn_key_file(key: &Path, name: &str) {
    static WARNED: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(Mutex::default);
    if sync::lock(&WARNED).insert(key.to_path_buf()) {
        tracing::warn!(
            "TLS key file {} is a usable key on disk: put it with `moqui operator secret put {}{name} --file {}` and \
             remove the file (plans/Sealed-Secrets.md S4)",
            key.display(),
            moqui_core::seal::secrets::TLS_KEY_PREFIX,
            key.display()
        );
    }
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

fn read_certified_key(cert: &Path, key: &KeyFrom) -> Result<CertifiedKey, Error> {
    let cert_bytes =
        std::fs::read(cert).map_err(|err| Error::new(format!("could not read {}: {err}", cert.display())))?;
    let chain = CertificateDer::pem_slice_iter(&cert_bytes)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| Error::new(format!("{} has no usable PEM certificate: {err}", cert.display())))?;
    if chain.is_empty() {
        return Err(Error::new(format!("{} has no PEM certificate", cert.display())));
    }
    let key_bytes = key.read()?;
    let der = PrivateKeyDer::from_pem_slice(&key_bytes).map_err(|err| {
        Error::new(format!(
            "the key of {} has no usable PEM private key: {err}",
            cert.display()
        ))
    })?;
    CertifiedKey::from_der(chain, der, &aws_lc_rs::default_provider())
        .map_err(|err| Error::new(format!("{} does not match its key: {err}", cert.display())))
}

/// The leaf certificate of each pair in `dir` and when it expires, for the resource readings
/// (`moqui_core::context::resources`): a pair whose certificate does not read, or has no date this reads, is left out.
pub(crate) fn certificate_expiries(dir: &Path) -> Vec<moqui_core::context::resources::Certificate> {
    let mut found = Vec::new();
    for name in pair_names(dir) {
        let Some((cert, _)) = find_pair(dir, &name) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(&cert) else {
            continue;
        };
        let leaf = CertificateDer::pem_slice_iter(&bytes).next().and_then(Result::ok);
        if let Some(not_after) = leaf.and_then(|leaf| moqui_lib::certificate::not_after(&leaf)) {
            found.push(moqui_core::context::resources::Certificate { name, not_after });
        }
    }
    found
}

/// The pair in `dir` a handshake asking for `name` would be served, for System › Certificates: which pair (the name
/// itself, its wildcard or `default`), its files and its leaf, or why it does not load.
#[cfg(feature = "axum")]
pub(crate) struct Served {
    /// The pair's name: `name` itself, `_wildcard.<parent>` or `<parent>`, or `default`.
    pub pair: String,
    pub cert: PathBuf,
    pub key: PathBuf,
    /// The leaf certificate, when the pair loads.
    pub leaf: Option<CertificateDer<'static>>,
    /// Why the pair does not load, when it does not; the listener then refuses the name.
    pub problem: Option<String>,
}

#[cfg(feature = "axum")]
pub(crate) fn served_for(dir: &Path, name: &str) -> Option<Served> {
    candidates(Some(name)).into_iter().find_map(|pair| {
        let (cert, from) = find_pair(dir, &pair)?;
        let (leaf, problem) = match read_certified_key(&cert, &from) {
            Ok(loaded) => (loaded.cert.first().cloned(), None),
            Err(err) => (None, Some(err.to_string())),
        };
        let key = match from {
            KeyFrom::File(key) => key,
            KeyFrom::Held(_) => PathBuf::new(),
        };
        Some(Served {
            pair,
            cert,
            key,
            leaf,
            problem,
        })
    })
}

/// The names in `dir` with a pair in any layout.
#[cfg(feature = "axum")]
pub(crate) fn pairs_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = pair_names(dir)
        .into_iter()
        .filter(|name| find_pair(dir, name).is_some())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Whether the directory holds a certificate and key pair in any layout.
fn has_any_pair(dir: &Path) -> bool {
    pair_names(dir).iter().any(|name| find_pair(dir, name).is_some())
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
    moqui_lib::private_fs::create_dir_all(dir).map_err(|err| fail("could not create the cert directory", &err))?;
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
    let key_pem = zeroize::Zeroizing::new(pem("PRIVATE KEY", &key.serialize_der()));
    for stem in ["localhost", "default"] {
        // the key in the seal's store when the server has one (plans/Sealed-Secrets.md S4), else a file beside it
        let name = format!("{}{stem}", moqui_core::seal::secrets::TLS_KEY_PREFIX);
        if let Err(err) = moqui_core::seal::secrets::put_named(&name, &key_pem) {
            tracing::debug!("The development key is written as a file: {err}");
            write_private(&dir.join(format!("{stem}.key")), &key_pem)?;
        }
        std::fs::write(dir.join(format!("{stem}.crt")), &cert_pem)
            .map_err(|err| fail("could not write the certificate", &err))?;
    }
    tracing::info!(
        "Generated a self-signed development certificate for localhost in {}",
        dir.display()
    );
    Ok(())
}

/// Write a private key to `path` as a new file of mode `0600` (`moqui_lib::private_fs`). A file or a symbolic link
/// already there (a staging file a stopped write left, or one planted) is removed first, never written into: the
/// mode applies only to a file the open creates, and `create_new` does not follow a link (SEC-OUT-85).
pub(crate) fn write_private(path: &Path, text: &str) -> Result<(), Error> {
    use std::io::Write as _;
    let fail = |err: std::io::Error| Error::new(format!("could not write {}: {err}", path.display()));
    match std::fs::remove_file(path) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(fail(err)),
        _ => {}
    }
    let mut file = moqui_lib::private_fs::file_options()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(fail)?;
    file.write_all(text.as_bytes()).map_err(fail)
}

pub(crate) fn pem(label: &str, der: &[u8]) -> String {
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
