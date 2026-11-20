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

//! The unseal methods (plans/Sealed-Secrets.md §3.3). An automatic method (`dev`, `tpm`, and in S11 `kms` and
//! `transit`) wraps and unwraps the root key with no operator and is a [`Method`]; the built-in ones are here, and S11
//! (and a managed instance's manager link, Vault-Instances M6) adds its own with [`register`]. The operator methods,
//! `passphrase` and `shamir`, take what an operator enters on the socket and have functions of their own.

use std::sync::{Arc, LazyLock, RwLock};

use serde_json::{Map, Value};

use super::{RootKey, SealConf};
use crate::context::Error;

pub mod dev;
pub mod passphrase;
pub mod shamir;
pub mod tpm;

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
        MethodKind::Kms | MethodKind::Transit => Err(Error::new(format!(
            "the {} unseal method is not built yet (plans/Sealed-Secrets.md S11)",
            kind.name()
        ))),
        MethodKind::Passphrase | MethodKind::Shamir => Err(Error::new(format!(
            "the {} method is opened by an operator, not automatically",
            kind.name()
        ))),
    }
}

/// A base64 field of a method's metadata.
pub(crate) fn bytes_field(metadata: &Map<String, Value>, name: &str) -> Result<Vec<u8>, Error> {
    let text = metadata
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::new(format!("seal.json's method has no {name}")))?;
    super::decode(text).map_err(|err| Error::new(format!("seal.json's method {name}: {err}")))
}
