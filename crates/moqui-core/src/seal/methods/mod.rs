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

//! The unseal methods (plans/Sealed-Secrets.md §3.3). An automatic method (`dev`, `tpm`, `kms` and `transit`) wraps
//! and unwraps the root key with no operator and is a [`Method`]; the built-in ones are here, and a managed instance's
//! manager link (Vault-Instances M6) installs its own transit with [`register`], from the host: core never calls the
//! host. The operator methods, `passphrase` and `shamir`, take what an operator enters on the socket and have functions
//! of their own. `kms` and `transit` call out over HTTP, so a build with no `http` feature has neither.

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, RwLock};

use serde_json::{Map, Value};
use zeroize::Zeroizing;

use super::{RootKey, SealConf};
use crate::context::Error;

pub mod dev;
#[cfg(feature = "http")]
pub mod kms;
pub mod passphrase;
pub mod shamir;
pub mod tpm;
#[cfg(feature = "http")]
pub mod transit;

/// The unseal methods by name, as `seal@method` and `seal.json` name them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MethodKind {
    /// The key in `seal.json` itself: development only, refused on a production instance (§3.8).
    Dev,
    /// Argon2id of a passphrase an operator enters (m=256 MiB, t=3).
    Passphrase,
    /// k of n shares that operators enter, one each.
    Shamir,
    /// `systemd-creds` with the TPM2, bound to its PCRs.
    Tpm,
    /// A cloud KMS key (S11).
    Kms,
    /// OpenBao or Vault Transit, or a managed instance's manager (S11).
    Transit,
}

impl MethodKind {
    /// Every method, in the order the docs list them.
    pub const ALL: [Self; 6] = [
        Self::Tpm,
        Self::Kms,
        Self::Transit,
        Self::Passphrase,
        Self::Shamir,
        Self::Dev,
    ];

    /// The method a name gives.
    ///
    /// # Errors
    ///
    /// A name that is no method.
    pub fn parse(name: &str) -> Result<Self, Error> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.name() == name.trim())
            .ok_or_else(|| {
                Error::new(format!(
                    "{name:?} is not an unseal method: tpm, kms, transit, passphrase, shamir or dev"
                ))
            })
    }

    /// The method's name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Passphrase => "passphrase",
            Self::Shamir => "shamir",
            Self::Tpm => "tpm",
            Self::Kms => "kms",
            Self::Transit => "transit",
        }
    }

    /// Whether it unseals at start with no operator.
    #[must_use]
    pub fn automatic(self) -> bool {
        !matches!(self, Self::Passphrase | Self::Shamir)
    }
}

/// An automatic unseal method: wraps a root key with no operator, into the metadata `seal.json` keeps for it, and
/// unwraps it again at start.
pub trait Method: Send + Sync {
    /// Which method this is.
    fn kind(&self) -> MethodKind;

    /// The method's metadata for `seal.json` with `root` wrapped in it. `kind` is added by the caller.
    ///
    /// # Errors
    ///
    /// The method cannot wrap here (no TPM, no KMS reached).
    fn wrap(&self, root: &RootKey, conf: &SealConf) -> Result<Map<String, Value>, Error>;

    /// The root key from the method's metadata.
    ///
    /// # Errors
    ///
    /// The method cannot unwrap (another host's TPM, a refused KMS call, changed metadata).
    fn unwrap(&self, metadata: &Map<String, Value>, conf: &SealConf) -> Result<RootKey, Error>;
}

/// The methods installed from above (S11's kms and transit, a manager link's transit).
static REGISTERED: LazyLock<RwLock<Vec<Arc<dyn Method>>>> = LazyLock::new(RwLock::default);

/// Install an automatic method, replacing one of the same kind.
pub fn register(method: Arc<dyn Method>) {
    let mut registered = moqui_lib::sync::write(&REGISTERED);
    registered.retain(|other| other.kind() != method.kind());
    registered.push(method);
}

/// The automatic method of `kind`.
///
/// # Errors
///
/// `kind` is an operator method, or one this build has not installed (`kms`, `transit` before S11).
pub fn automatic(kind: MethodKind) -> Result<Arc<dyn Method>, Error> {
    if let Some(method) = moqui_lib::sync::read(&REGISTERED)
        .iter()
        .find(|method| method.kind() == kind)
    {
        return Ok(Arc::clone(method));
    }
    match kind {
        MethodKind::Dev => Ok(Arc::new(dev::Dev)),
        MethodKind::Tpm => Ok(Arc::new(tpm::Tpm)),
        #[cfg(feature = "http")]
        MethodKind::Kms => Ok(Arc::new(kms::Kms)),
        #[cfg(feature = "http")]
        MethodKind::Transit => Ok(Arc::new(transit::Transit)),
        #[cfg(not(feature = "http"))]
        MethodKind::Kms | MethodKind::Transit => Err(Error::new(format!(
            "the {} unseal method calls out over HTTP, which this build has not (the http feature)",
            kind.name()
        ))),
        MethodKind::Passphrase | MethodKind::Shamir => Err(Error::new(format!(
            "the {} method is opened by an operator, not automatically",
            kind.name()
        ))),
    }
}

/// The cloud KMS providers of the `kms` method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KmsProvider {
    Aws,
    Gcp,
    Azure,
}

impl KmsProvider {
    /// The provider's name, as `seal@kms-provider` gives it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Aws => "aws",
            Self::Gcp => "gcp",
            Self::Azure => "azure",
        }
    }
}

/// Where a method's credential comes from (`seal@kms-credentials`, `seal@transit-token`): never the conf itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Credentials {
    /// The host's own identity: the instance role (AWS IMDSv2), the metadata server (GCP), the managed identity
    /// (Azure). `transit` has none.
    #[default]
    Metadata,
    /// A file of root's (an absolute path) that the server's user may read and nobody but its owner may write.
    File(PathBuf),
    /// The unit's systemd credential of that name (`$CREDENTIALS_DIRECTORY/<name>`), looked up when it is read, so a
    /// method that does not use it needs no credentials directory.
    Systemd(String),
}

thread_local! {
    /// A test's credential files are its own user's ([`allow_own_credential_files`]).
    static OWN_FILES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Let this thread read credential files its own user owns, in a directory it may write: a test's, which cannot make a
/// file of root's.
#[doc(hidden)]
pub fn allow_own_credential_files(allow: bool) {
    OWN_FILES.with(|own| own.set(allow));
}

impl Credentials {
    /// Parse `seal@kms-credentials` or `seal@transit-token`: empty or `metadata`, an absolute path, or
    /// `systemd:<name>`. A secret written there itself is refused, so a credential is never a conf literal.
    ///
    /// # Errors
    ///
    /// Anything else, or `systemd:` with a name that is a path.
    pub fn parse(selector: &str, given: &str) -> Result<Self, Error> {
        let given = given.trim();
        if given.is_empty() || given == "metadata" {
            return Ok(Self::Metadata);
        }
        if let Some(name) = given.strip_prefix("systemd:") {
            if name.is_empty() || name.contains('/') || name.starts_with('.') {
                return Err(Error::new(format!(
                    "{selector}: {given:?} is not a systemd credential's name"
                )));
            }
            return Ok(Self::Systemd(name.to_string()));
        }
        if Path::new(given).is_absolute() {
            return Ok(Self::File(PathBuf::from(given)));
        }
        Err(Error::new(format!(
            "{selector} names a file (an absolute path), a systemd credential (systemd:NAME) or metadata, never the \
             secret itself"
        )))
    }

    /// Whether a file holds the credential (the host's own identity otherwise).
    #[must_use]
    pub fn is_file(&self) -> bool {
        !matches!(self, Self::Metadata)
    }

    /// The credential's text, trimmed. A file must be root's, as the docs say: not the server's own user's, in no
    /// directory that user may write, and neither written nor read by others; a systemd credential is systemd's
    /// read-only one.
    ///
    /// # Errors
    ///
    /// No file named, a systemd credential with no `$CREDENTIALS_DIRECTORY`, or a file that cannot be read or that
    /// fails the checks.
    pub fn read(&self, selector: &str) -> Result<Zeroizing<String>, Error> {
        let path = match self {
            Self::Metadata => return Err(Error::new(format!("{selector} names no file"))),
            Self::File(path) => {
                check_root_file(selector, path)?;
                path.clone()
            }
            Self::Systemd(name) => {
                let dir = std::env::var_os("CREDENTIALS_DIRECTORY").ok_or_else(|| {
                    Error::new(format!(
                        "{selector} names the systemd credential {name}, and the service has no CREDENTIALS_DIRECTORY \
                         (LoadCredential= or LoadCredentialEncrypted= in the unit)"
                    ))
                })?;
                Path::new(&dir).join(name)
            }
        };
        let text = Zeroizing::new(
            std::fs::read_to_string(&path)
                .map_err(|err| Error::new(format!("{selector} {}: {err}", path.display())))?,
        );
        Ok(Zeroizing::new(text.trim().to_string()))
    }
}

/// The checks of a credential file of root's ([`Credentials::read`]).
fn check_root_file(selector: &str, path: &Path) -> Result<(), Error> {
    let meta = std::fs::metadata(path).map_err(|err| Error::new(format!("{selector} {}: {err}", path.display())))?;
    if !meta.is_file() {
        return Err(Error::new(format!("{selector} {} is not a file", path.display())));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if meta.mode() & 0o026 != 0 {
            return Err(Error::new(format!(
                "{selector} {} may be written by its group, or read or written by others: make it 0400 or 0440 \
                 (root's, the server's group)",
                path.display()
            )));
        }
        if OWN_FILES.with(std::cell::Cell::get) {
            return Ok(());
        }
        let me = rustix::process::geteuid().as_raw();
        if meta.uid() == me && me != 0 {
            return Err(Error::new(format!(
                "{selector} {} belongs to the server's own user, which could replace it: make it root's",
                path.display()
            )));
        }
        if let Some(dir) = path.parent() {
            let dir_meta =
                std::fs::metadata(dir).map_err(|err| Error::new(format!("{selector} {}: {err}", dir.display())))?;
            if (dir_meta.uid() == me && me != 0) || dir_meta.mode() & 0o022 != 0 {
                return Err(Error::new(format!(
                    "{selector} {} is in a directory the server's own user or others may write ({}): put it in one of \
                     root's",
                    path.display(),
                    dir.display()
                )));
            }
        }
    }
    Ok(())
}

/// Refuse a URL that is not https, but on the loopback (a test's stub, a local proxy): the root key and a credential go
/// to it.
///
/// # Errors
///
/// An `http://` URL to any other host, or no scheme.
#[cfg(feature = "http")]
pub(crate) fn require_https(selector: &str, url: &str) -> Result<(), Error> {
    let url = url.trim();
    if url.starts_with("https://") {
        return Ok(());
    }
    if let Some(rest) = url.strip_prefix("http://") {
        let host = rest.split(['/', '?']).next().unwrap_or_default();
        let host = match host.strip_prefix('[') {
            Some(v6) => v6.split(']').next().unwrap_or_default(),
            None => host.rsplit_once(':').map_or(host, |(name, _)| name),
        };
        if host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
        {
            return Ok(());
        }
    }
    Err(Error::new(format!(
        "{selector} {url:?} is not https: the root key and a credential go to it"
    )))
}

/// The settings of the `kms` and `transit` methods (`seal@kms-*`, `seal@transit-*`), read from the conf files only:
/// they decide where the root key is sent and whose credential goes with it, so an environment name or `runtime/.env`
/// that sets one is ignored ([`SealConf::from_gate`]).
#[derive(Clone, Debug, Default)]
pub struct CloudConf {
    /// `aws`, `gcp` or `azure`.
    pub kms_provider: String,
    /// The key: an AWS key id, ARN or alias; a GCP `projects/.../locations/.../keyRings/.../cryptoKeys/...`; an Azure
    /// key URL (`https://<vault>.vault.azure.net/keys/<name>`).
    pub kms_key: String,
    /// The AWS region.
    pub kms_region: String,
    /// The KMS API's base URL in place of the provider's (a private endpoint; a test's stub).
    pub kms_endpoint: String,
    /// Where tokens come from in place of the provider's: the instance metadata (AWS, Azure IMDS), the metadata server
    /// (GCP) or the sign-in endpoint (Azure's client credentials).
    pub kms_token_endpoint: String,
    pub kms_credentials: Credentials,
    /// The OpenBao or Vault address, its Transit mount (`transit`), key and namespace, and a PEM of the authorities its
    /// HTTPS is checked against in place of the system's.
    pub transit_url: String,
    pub transit_mount: String,
    pub transit_key: String,
    pub transit_namespace: String,
    pub transit_ca_file: String,
    pub transit_token: Credentials,
}

impl CloudConf {
    /// The provider `seal@kms-provider` names.
    ///
    /// # Errors
    ///
    /// None or another is named.
    pub fn kms_provider(&self) -> Result<KmsProvider, Error> {
        match self.kms_provider.trim() {
            "aws" => Ok(KmsProvider::Aws),
            "gcp" => Ok(KmsProvider::Gcp),
            "azure" => Ok(KmsProvider::Azure),
            other => Err(Error::new(format!("seal@kms-provider is {other:?}: aws, gcp or azure"))),
        }
    }

    /// The key `seal@kms-key` names.
    ///
    /// # Errors
    ///
    /// None is named.
    pub fn kms_key(&self) -> Result<&str, Error> {
        Some(self.kms_key.trim())
            .filter(|key| !key.is_empty())
            .ok_or_else(|| Error::new("seal@kms-key is not set (the KMS key that wraps the root key)"))
    }

    /// The base of the token calls: `seal@kms-token-endpoint`, else `default`.
    #[must_use]
    pub fn token_base(&self, default: &str) -> String {
        if self.kms_token_endpoint.is_empty() {
            default.to_string()
        } else {
            self.kms_token_endpoint.trim_end_matches('/').to_string()
        }
    }
}

#[cfg(all(test, feature = "http"))]
#[path = "cloud_tests.rs"]
mod cloud_tests;

/// A base64 field of a method's metadata.
pub(crate) fn bytes_field(metadata: &Map<String, Value>, name: &str) -> Result<Vec<u8>, Error> {
    let text = metadata
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::new(format!("seal.json's method has no {name}")))?;
    super::decode(text).map_err(|err| Error::new(format!("seal.json's method {name}: {err}")))
}
