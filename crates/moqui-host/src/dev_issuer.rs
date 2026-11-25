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

//! Local development CA. Only the name-constrained intermediate's signing key persists. The root is valid ten years and
//! its key is dropped once it signs the intermediate; the intermediate two years, when `moqui dev-cert --rotate` makes a
//! new pair (and the new root is trusted again). `moqui dev-cert --replace` moves an older self-signed pair aside.
//!
//! The two leaves share one key and differ in their names. `localhost` (served for SNI `localhost`) names only the DNS
//! name: BoringSSL, the TLS of grpcio and so of the Python, Ruby, PHP and C++ OTLP/gRPC exporters, cannot check an IP
//! address against the intermediate's IP constraints and refuses a leaf that carries one, even for `localhost`.
//! `default` (served when a client sends no SNI, as for `https://127.0.0.1`) adds the loopback addresses.
use moqui_lib::{error::Error, private_fs};
use rcgen::{
    BasicConstraints, CertificateParams, CidrSubnet, DistinguishedName, DnType, ExtendedKeyUsagePurpose,
    GeneralSubtree, IsCa, Issuer, KeyPair, KeyUsagePurpose, NameConstraints, SanType,
};
use rustls::pki_types::{pem::PemObject as _, CertificateDer, PrivateKeyDer};
use sha2::{Digest as _, Sha256};
use std::fs;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DAY: u64 = 86400;
/// The development root's validity, in days.
const ROOT_DAYS: u64 = 3652;
/// The intermediate's: its key is on disk, so it is short-lived and rotated.
const ISSUER_DAYS: u64 = 730;
/// The localhost and default stems that [`issue`] manages in `runtime/cert`.
const STEMS: [&str; 2] = ["localhost", "default"];
/// The stems `moqui dev-cert --replace` moved aside: a key the seal still holds under `tls-key/<stem>` belongs to the
/// moved pair and is replaced at the next issue, not a reason to refuse.
const REPLACED_HELD: &str = "replaced-held";

/// What `moqui dev-cert` asks of [`ensure`] beyond a boot's issue.
#[derive(Clone, Copy, Default)]
pub(crate) struct Options {
    /// Make the shared signer from this checkout's (`--shared`, `./moq trust-dev-cert`).
    pub establish: bool,
    /// Move an unmanaged localhost or default pair aside first (`--replace`).
    pub replace: bool,
    /// Replace the root and the intermediate, then the leaves (`--rotate`).
    pub rotate: bool,
}
fn parameters(name: &str, days: u64) -> CertificateParams {
    let now = SystemTime::now();
    let mut params = CertificateParams::default();
    params.not_before = (now - Duration::from_secs(300)).into();
    params.not_after = (now + Duration::from_secs(days * DAY - 300)).into();
    params.distinguished_name = DistinguishedName::new();
    params.distinguished_name.push(DnType::CommonName, name);
    // Python's ssl (3.13 and later) verifies strictly and refuses a certificate under a CA without this extension
    params.use_authority_key_identifier_extension = true;
    params
}

fn authority(intermediate: bool) -> CertificateParams {
    let mut p = parameters(
        if intermediate {
            "MR Moqui localhost issuer"
        } else {
            "MR Moqui development root"
        },
        if intermediate { ISSUER_DAYS } else { ROOT_DAYS },
    );
    p.is_ca = IsCa::Ca(BasicConstraints::Constrained(if intermediate { 0 } else { 1 }));
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    if intermediate {
        p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        p.name_constraints = Some(NameConstraints {
            permitted_subtrees: vec![
                GeneralSubtree::DnsName("localhost".into()),
                GeneralSubtree::DnsName(".localhost".into()),
                GeneralSubtree::IpAddress(CidrSubnet::from_addr_prefix(Ipv4Addr::new(127, 0, 0, 0).into(), 8)),
                GeneralSubtree::IpAddress(CidrSubnet::from_addr_prefix(Ipv6Addr::LOCALHOST.into(), 128)),
            ],
            excluded_subtrees: vec![],
        });
    }
    p
}

fn lock(dir: &Path) -> io::Result<fs::File> {
    private_fs::create_dir_all(dir)?;
    let file = private_fs::file_options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(".dev-ca.lock"))?;
    file.lock()?;
    Ok(file)
}

fn key_pem(key: &KeyPair) -> zeroize::Zeroizing<String> {
    zeroize::Zeroizing::new(super::pem("PRIVATE KEY", &key.serialize_der()))
}

// The root and issuer are published together. Loss of the root key is intentional.
fn create_issuer(dir: &Path) -> io::Result<()> {
    let parent = dir
        .parent()
        .ok_or_else(|| io::Error::other("missing issuer directory parent"))?;
    let temp = private_fs::new_dir_in(parent, ".dev-ca")?;
    let result = (|| {
        let root = authority(false);
        let rk = KeyPair::generate().map_err(io::Error::other)?;
        let root_cert = root.self_signed(&rk).map_err(io::Error::other)?;
        let issuer_key = KeyPair::generate().map_err(io::Error::other)?;
        let issuer_cert = authority(true)
            .signed_by(&issuer_key, &Issuer::new(root, rk))
            .map_err(io::Error::other)?;
        fs::write(temp.join("ca.crt"), super::pem("CERTIFICATE", root_cert.der()))?;
        fs::write(temp.join("issuer.crt"), super::pem("CERTIFICATE", issuer_cert.der()))?;
        super::write_private(&temp.join("issuer.key"), &key_pem(&issuer_key))
            .map_err(|e| io::Error::other(e.to_string()))?;
        fs::rename(&temp, dir)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(temp);
    }
    result
}

/// Establish the first shared signer from this checkout if it already issued an identity; the checkout's own copy of
/// the signing key is removed once the shared one is in place, so one key signs.
fn promote(local: &Path, shared: &Path) -> io::Result<()> {
    let source = local.join("signer");
    if !source.is_dir() || shared.join("signer").exists() {
        return Ok(());
    }
    private_fs::create_dir_all(shared)?;
    let staged = private_fs::new_dir_in(shared, ".dev-ca")?;
    let result = (|| {
        for name in ["ca.crt", "issuer.crt", "issuer.key"] {
            fs::copy(source.join(name), staged.join(name))?;
        }
        private_fs::restrict(&staged.join("issuer.key"))?;
        fs::rename(&staged, shared.join("signer"))
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(staged);
        return result;
    }
    fs::remove_dir_all(source)
}

/// Move the unmanaged localhost and default pairs of `dir` aside into `dir/replaced-<seconds>`, and name their stems
/// in [`REPLACED_HELD`]. Returns the directory, or `None` when there was nothing to move.
fn replace(dir: &Path, local: &Path, at: SystemTime) -> io::Result<Option<PathBuf>> {
    let known = read_optional(&local.join("managed-leaves"))?.unwrap_or_default();
    let managed = |path: &Path| {
        read_optional(path)
            .ok()
            .flatten()
            .is_some_and(|cert| known.lines().any(|hash| hash == fingerprint(&cert)))
    };
    let seconds = at.duration_since(UNIX_EPOCH).map_err(io::Error::other)?.as_secs();
    let aside = dir.join(format!("replaced-{seconds}"));
    let mut moved = Vec::new();
    for stem in STEMS {
        let crt = dir.join(format!("{stem}.crt"));
        if crt.is_file() && managed(&crt) {
            continue;
        }
        let mut paths: Vec<PathBuf> = vec![crt, dir.join(format!("{stem}.key")), dir.join(stem)];
        if let Some((cert, key)) = super::find_pair(dir, stem) {
            paths.push(cert);
            if let super::KeyFrom::File(key) = key {
                paths.push(key);
            }
        }
        paths.sort();
        paths.dedup();
        for path in paths.into_iter().filter(|path| fs::symlink_metadata(path).is_ok()) {
            let Some(name) = path.file_name() else { continue };
            private_fs::create_dir_all(&aside)?;
            fs::rename(&path, aside.join(name))?;
            if !moved.contains(&stem) {
                moved.push(stem);
            }
        }
    }
    if moved.is_empty() {
        return Ok(None);
    }
    private_fs::create_dir_all(local)?;
    let mut held: Vec<String> = read_optional(&local.join(REPLACED_HELD))?
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    held.extend(moved.iter().map(|stem| (*stem).to_string()));
    held.sort();
    held.dedup();
    publish(&local.join(REPLACED_HELD), &held.join("\n"))?;
    Ok(Some(aside))
}

fn publish(path: &Path, text: &str) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("missing certificate directory"))?;
    let temp = private_fs::new_dir_in(parent, ".dev-ca-write")?;
    let stage = temp.join("new");
    let result = super::write_private(&stage, text)
        .map_err(|e| io::Error::other(e.to_string()))
        .and_then(|()| fs::rename(stage, path));
    let cleanup = fs::remove_dir_all(temp);
    result.and(cleanup)
}

fn read_optional(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn until(cert: &str) -> io::Result<u64> {
    let der = CertificateDer::from_pem_slice(cert.as_bytes()).map_err(io::Error::other)?;
    moqui_lib::certificate::not_after(&der)
        .and_then(|n| u64::try_from(n).ok())
        .ok_or_else(|| io::Error::other("invalid certificate expiry"))
}

fn fingerprint(cert: &str) -> String {
    crate::tls_pin::hex(&Sha256::digest(cert.as_bytes()).into())
}

fn issue(dir: &Path, source: &Path, establish: bool, at: SystemTime) -> io::Result<()> {
    let local = dir.join("dev-ca");
    private_fs::create_dir_all(source)?;
    let signer_dir = source.join("signer");
    if !signer_dir.exists() {
        create_issuer(&signer_dir)?;
    }
    let root = fs::read_to_string(signer_dir.join("ca.crt"))?;
    let issuer_cert = fs::read_to_string(signer_dir.join("issuer.crt"))?;
    if read_optional(&source.join("ca.crt"))?.as_deref() != Some(&root) {
        publish(&source.join("ca.crt"), &root)?;
    }
    if source != local {
        private_fs::create_dir_all(&local)?;
        if read_optional(&local.join("ca.crt"))?.as_deref() != Some(&root) {
            publish(&local.join("ca.crt"), &root)?;
        }
    }
    let now = at.duration_since(UNIX_EPOCH).map_err(io::Error::other)?.as_secs();
    let end = until(&issuer_cert)?.min(until(&root)?);
    if end <= now + 30 * DAY {
        return Err(io::Error::other(
            "the dev issuer expires within 30 days: run moqui dev-cert --rotate, then ./moq trust-dev-cert to trust \
             the new root",
        ));
    }
    let marker = local.join("managed-leaves");
    let known = read_optional(&marker)?.unwrap_or_default();
    let replaced_held = read_optional(&local.join(REPLACED_HELD))?.unwrap_or_default();
    let mut existing = Vec::new();
    let mut unmanaged = Vec::new();
    for stem in STEMS {
        let path = dir.join(format!("{stem}.crt"));
        let cert = read_optional(&path)?;
        let disk_key = read_optional(&dir.join(format!("{stem}.key")))?.map(zeroize::Zeroizing::new);
        // a held key of a pair --replace moved aside is replaced below, not refused
        let held = super::held_key(dir, stem).filter(|_| !replaced_held.lines().any(|s| s == stem));
        let unknown_key = [disk_key.as_deref(), held.as_deref()].into_iter().flatten().any(|key| {
            let id = format!("key {}", fingerprint(key));
            !known.lines().any(|hash| hash == id)
        });
        let unknown_cert = cert
            .as_ref()
            .is_some_and(|c| !known.lines().any(|hash| hash == fingerprint(c)));
        if unknown_key || unknown_cert || super::find_pair(dir, stem).is_some_and(|(found, _)| found != path) {
            unmanaged.push(stem);
        }
        existing.push(cert);
    }
    if !unmanaged.is_empty() {
        let names = unmanaged.join(", ");
        return Err(io::Error::other(format!(
            "Unmanaged TLS identities in {}: {names}. Run moqui dev-cert --replace to move them aside (into \
             cert/replaced-<time>) and issue a managed development certificate, or back up and move them aside \
             yourself.",
            dir.display()
        )));
    }
    if existing.iter().all(|c| {
        c.as_ref()
            .is_some_and(|c| c.ends_with(&issuer_cert) && until(c).is_ok_and(|t| t > now + 30 * DAY))
    }) && STEMS
        .iter()
        .all(|stem| super::find_pair(dir, stem).is_some_and(|(c, k)| super::read_certified_key(&c, &k).is_ok()))
    {
        return Ok(());
    }
    if !known.is_empty()
        && STEMS.iter().any(|stem| {
            dir.join(format!("{stem}.crt")).is_file()
                && !dir.join(format!("{stem}.key")).is_file()
                && super::held_key(dir, stem).is_none()
        })
    {
        let message =
            "A managed TLS key is unavailable offline. Start the server with its seal unlocked to renew the identity.";
        if establish {
            eprintln!("{message}");
            return Ok(());
        }
        return Err(io::Error::other(message));
    }
    let key = KeyPair::generate().map_err(io::Error::other)?;
    let issuer_pem = zeroize::Zeroizing::new(fs::read(signer_dir.join("issuer.key"))?);
    let der = PrivateKeyDer::from_pem_slice(&issuer_pem).map_err(io::Error::other)?;
    let signer = Issuer::new(authority(true), KeyPair::try_from(&der).map_err(io::Error::other)?);
    let mut chains = Vec::with_capacity(STEMS.len());
    for stem in STEMS {
        let mut params = parameters("localhost", 397);
        params.not_after = params.not_after.min((UNIX_EPOCH + Duration::from_secs(end)).into());
        params.subject_alt_names = vec![SanType::DnsName("localhost".try_into().map_err(io::Error::other)?)];
        if stem == "default" {
            params.subject_alt_names.extend([
                SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                SanType::IpAddress(IpAddr::V6(Ipv6Addr::LOCALHOST)),
            ]);
        }
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let cert = params.signed_by(&key, &signer).map_err(io::Error::other)?;
        chains.push(super::pem("CERTIFICATE", cert.der()) + &issuer_cert);
    }
    let key = key_pem(&key);
    let mut hash: Vec<String> = chains.iter().map(|chain| fingerprint(chain)).collect();
    hash.push(format!("key {}", fingerprint(&key)));
    let hash = hash.join("\n");
    publish(&marker, &format!("{known}\n{hash}"))?;
    let mut sealed = true;
    for (stem, chain) in STEMS.into_iter().zip(&chains) {
        let name = format!("{}{stem}", moqui_core::seal::secrets::TLS_KEY_PREFIX);
        let disk_key = dir.join(format!("{stem}.key"));
        if moqui_core::seal::secrets::put_named(&name, &key).is_err() {
            sealed = false;
            publish(&disk_key, &key)?;
        } else if disk_key.is_file() {
            fs::remove_file(&disk_key)?;
        }
        publish(&dir.join(format!("{stem}.crt")), chain)?;
    }
    if sealed && !replaced_held.is_empty() {
        // the seal now holds this issue's key under each stem
        fs::remove_file(local.join(REPLACED_HELD))?;
    }
    publish(&marker, &hash)
}

/// Remove the signer at `source` (`--rotate`), so the next issue makes a new root and intermediate.
fn rotate(source: &Path) -> io::Result<()> {
    match fs::remove_dir_all(source.join("signer")) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

pub(super) fn ensure(dir: &Path, shared: Option<PathBuf>, establish: bool) -> Result<(), Error> {
    ensure_with(
        dir,
        shared,
        Options {
            establish,
            ..Options::default()
        },
    )
}

/// [`ensure`] with the `moqui dev-cert` options.
pub(super) fn ensure_with(dir: &Path, shared: Option<PathBuf>, options: Options) -> Result<(), Error> {
    let Options {
        establish,
        replace: replacing,
        rotate: rotating,
    } = options;
    let run = || -> io::Result<()> {
        private_fs::create_dir_all(dir)?;
        let dir = dir.canonicalize()?;
        let _local = lock(&dir)?;
        let local = dir.join("dev-ca");
        if replacing {
            if let Some(aside) = replace(&dir, &local, SystemTime::now())? {
                eprintln!("Moved the unmanaged TLS identities aside into {}", aside.display());
            }
        }
        let shared = shared
            .map(|s| s.join("cert/dev-ca"))
            .filter(|s| s != &local && s.parent().and_then(|p| p.canonicalize().ok()).as_ref() != Some(&dir));
        if let Some(shared) = shared.filter(|s| establish || s.join("signer").is_dir()) {
            let parent = shared
                .parent()
                .ok_or_else(|| io::Error::other("missing shared runtime"))?;
            let _shared = lock(parent)?;
            if establish {
                promote(&local, &shared)?;
            }
            if rotating {
                rotate(&shared)?;
            }
            return issue(&dir, &shared, establish, SystemTime::now());
        }
        if rotating {
            rotate(&local)?;
        }
        issue(&dir, &local, establish, SystemTime::now())
    };
    run().map_err(|e| Error::new(format!("dev certificate in {}: {e}", dir.display())))
}

#[cfg(test)]
#[path = "dev_issuer_tests.rs"]
mod tests;
