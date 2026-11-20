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

//! The recovery bundle (plans/Sealed-Secrets.md §3.3, Backup of the seal): `seal.json`, which holds only wrapped
//! values, with the database's identity and the keyring id (a placeholder until S1 builds the keyring). A database
//! backup is useless without it, so it goes with the offline backups, made again after each `rekey`, `rotate-root` and
//! bootstrap-secret change. A restore on a clean host (`moqui operator init --from`) puts the bundle's seal in place;
//! the recovery shares then unseal it, without the old host's TPM, and `rekey` wraps it under the new host's method.

use serde_json::{json, Value};

use super::{file, Keeper, MethodKind, SealFile};
use crate::context::Error;

/// The `format` of a bundle.
pub const BUNDLE_FORMAT: &str = "moqui-seal-bundle";

impl Keeper {
    /// The recovery bundle of this runtime's seal, as JSON text.
    ///
    /// # Errors
    ///
    /// The runtime has no seal yet.
    pub fn export_bundle(&self) -> Result<String, Error> {
        let inner = self.inner();
        let seal = inner
            .file
            .as_ref()
            .ok_or_else(|| Error::new("this runtime has no seal yet"))?;
        if seal.method_kind() == MethodKind::Dev.name() {
            // the dev seal's file holds its root key in the clear, and so would the bundle
            if self.conf().production() {
                return Err(Error::new(
                    "the dev seal is refused on a production instance, and so is its bundle",
                ));
            }
            tracing::warn!("The recovery bundle of a dev seal holds the root key in the clear");
        }
        // a member's own edge key stays with that member: a member restored from this bundle makes its own (S4)
        let mut seal = seal.clone();
        seal.bootstrap.remove(super::secrets::EDGE_MEMBER_KEY);
        let seal: Value =
            serde_json::to_value(&seal).map_err(|err| Error::new(format!("seal.json does not serialize: {err}")))?;
        let bundle = json!({
            "format": BUNDLE_FORMAT,
            "version": 1,
            "created": jiff::Timestamp::now().to_string(),
            "seal": seal,
            "database": self.conf().database_identity,
            // the keyring's id once S1 builds it (plans/Sealed-Secrets.md §3.1)
            "keyring_id": Value::Null,
        });
        serde_json::to_string_pretty(&bundle).map_err(|err| Error::new(format!("the bundle does not serialize: {err}")))
    }

    /// Put a bundle's seal in place on a runtime that has none. The server stays sealed: the recovery shares (or the
    /// bundle's method, where it still works) unseal it.
    ///
    /// # Errors
    ///
    /// Text that is not a bundle, or a runtime with a seal already.
    pub fn restore(&self, bundle: &str) -> Result<(), Error> {
        let bundle: Value =
            serde_json::from_str(bundle).map_err(|err| Error::new(format!("not a recovery bundle: {err}")))?;
        if bundle["format"] != BUNDLE_FORMAT {
            return Err(Error::new(
                "not a recovery bundle (its format is not moqui-seal-bundle)",
            ));
        }
        let mut seal = SealFile::parse(&bundle["seal"].to_string())?;
        // another member's edge key, from a bundle made before it was left out: this member makes its own
        seal.bootstrap.remove(super::secrets::EDGE_MEMBER_KEY);
        let kind = MethodKind::parse(seal.method_kind())?;
        if kind == MethodKind::Dev && self.conf().production() {
            return Err(super::dev_refused());
        }
        if kind != MethodKind::Dev && seal.recovery.is_none() {
            return Err(Error::new(
                "the bundle's seal has no recovery part, so its shares cannot open it",
            ));
        }
        let mut inner = self.inner();
        let held = file::lock(&self.path)?;
        if inner.file.is_none() {
            inner.file = file::load(&self.path)?;
        }
        if inner.file.is_some() || inner.root.is_some() {
            return Err(Error::new(
                "this runtime has a seal already; a bundle is restored only on a clean runtime",
            ));
        }
        if !file::stage(&self.path, &seal)?.commit_new(&held)? {
            return Err(Error::new("another process made this runtime's seal at the same time"));
        }
        tracing::warn!(
            "Restored the seal of a recovery bundle (method {}, generation {}): unseal it with the recovery shares",
            seal.method_kind(),
            seal.generation
        );
        inner.file = Some(seal);
        Ok(())
    }
}
