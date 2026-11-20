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

//! `rekey` and `rotate-root` (plans/Sealed-Secrets.md §3.3), both authorized by the recovery shares. `rekey` keeps the
//! root key and wraps it under another method (or the same with new input); only it changes the method. `rotate-root`
//! makes a new root key: the method wrap, the recovery wrap (under the same recovery key, so the shares stay), the
//! bootstrap secrets and the `SecretValue` data key are written beside the old ones in a new `seal.json`, which
//! replaces the old file in one rename. A crash before the rename leaves the old root in use. Either one, given the
//! shares, also opens a sealed server, as `unseal --recovery` does.

use zeroize::Zeroizing;

use super::{bootstrap, check_root, file, lock, recovery, Inner, Issued, Keeper, MethodKind, RootKey, SealFile};
use crate::context::Error;

/// What `rekey` changes to.
pub struct Rekey {
    /// The recovery shares, at least the threshold.
    pub shares: Vec<Zeroizing<String>>,
    pub method: MethodKind,
    /// The new passphrase of the `passphrase` method.
    pub passphrase: Option<Zeroizing<String>>,
}

/// What `rotate-root` takes.
pub struct RotateRoot {
    /// The recovery shares, at least the threshold.
    pub shares: Vec<Zeroizing<String>>,
    /// The passphrase of a `passphrase` seal, which wraps the new root key.
    pub passphrase: Option<Zeroizing<String>>,
}

impl Keeper {
    /// The root key and the recovery key from the recovery shares; a wrong share is counted as at `unseal`.
    fn recover(
        &self,
        inner: &mut Inner,
        shares: &[Zeroizing<String>],
    ) -> Result<(RootKey, moqui_vault::keys::Key32), Error> {
        if let Some(wait) = inner
            .retry_at
            .and_then(|at| at.checked_duration_since(std::time::Instant::now()))
        {
            return Err(Error::new(format!(
                "wait {:.1} s before the next attempt (after a wrong entry)",
                wait.as_secs_f64()
            )));
        }
        let conf = self.conf();
        let seal = inner
            .file
            .as_ref()
            .ok_or_else(|| Error::new("this runtime has no seal yet"))?;
        let recovery = seal
            .recovery
            .clone()
            .ok_or_else(|| Error::new("this seal has no recovery shares (the dev seal)"))?;
        let Some(key) = recovery::key_from(shares, &recovery)? else {
            return Err(self.wrong_entry(inner, &conf, "recovery share"));
        };
        let root = recovery::open_root(&key, &recovery)?;
        check_root(seal, &root)?;
        Ok((root, key))
    }

    /// Wrap the root key under another method, authorized by the recovery shares (plans/Sealed-Secrets.md §3.3). The
    /// recovery shares and the root key stay; the method's own shares are issued for `shamir`.
    ///
    /// # Errors
    ///
    /// Too few or wrong shares, the `dev` seal on a production instance, or the new method cannot wrap.
    pub fn rekey(&self, request: Rekey) -> Result<Issued, Error> {
        let Some(_attempt) = moqui_lib::sync::try_lock(&self.attempt) else {
            return Err(Error::new("another unseal attempt is running; try again when it ends"));
        };
        let conf = self.conf();
        if request.method == MethodKind::Dev && conf.production() {
            return Err(super::dev_refused());
        }
        let mut inner = lock(&self.inner);
        let (root, _) = self.recover(&mut inner, &request.shares)?;
        let (method, issued) = self.wrap_method(
            &root,
            request.method,
            request.passphrase.as_deref().map(String::as_str),
            &conf,
        )?;
        let mut seal = inner
            .file
            .clone()
            .ok_or_else(|| Error::new("this runtime has no seal yet"))?;
        let old = seal.method_kind().to_string();
        seal.method = method;
        seal.generation += 1;
        file::stage(&self.path, &seal)?.commit()?;
        inner.file = Some(seal);
        self.opened(&mut inner, root);
        tracing::warn!(
            "Rekeyed: the root key is wrapped by the {} method now, in place of {old}",
            request.method.name()
        );
        Ok(issued)
    }

    /// The new seal file and root key of a `rotate-root`, not yet written.
    pub(super) fn rotated(
        &self,
        inner: &mut Inner,
        request: &RotateRoot,
    ) -> Result<(SealFile, RootKey, Issued), Error> {
        let conf = self.conf();
        let (old_root, recovery_key) = self.recover(inner, &request.shares)?;
        let old = inner
            .file
            .clone()
            .ok_or_else(|| Error::new("this runtime has no seal yet"))?;
        let kind = MethodKind::parse(old.method_kind())?;
        let root = RootKey::random()?;
        let (method, issued) =
            self.wrap_method(&root, kind, request.passphrase.as_deref().map(String::as_str), &conf)?;
        let mut seal = old.clone();
        seal.method = method;
        seal.generation += 1;
        seal.root_id = root.id();
        if let Some(recovery) = seal.recovery.as_mut() {
            recovery.wrapped_root = recovery::wrap_root(&recovery_key, &root);
        }
        bootstrap::rewrap(&old, &old_root, &mut seal, &root)?;
        Ok((seal, root, issued))
    }

    /// Make a new root key and rewrap everything under it, authorized by the recovery shares (plans/Sealed-Secrets.md
    /// §3.3); the new `seal.json` replaces the old one in one rename.
    ///
    /// # Errors
    ///
    /// Too few or wrong shares, a `passphrase` seal with no passphrase, or the file cannot be written.
    pub fn rotate_root(&self, request: RotateRoot) -> Result<Issued, Error> {
        let Some(_attempt) = moqui_lib::sync::try_lock(&self.attempt) else {
            return Err(Error::new("another unseal attempt is running; try again when it ends"));
        };
        let mut inner = lock(&self.inner);
        let (seal, root, issued) = self.rotated(&mut inner, &request)?;
        file::stage(&self.path, &seal)?.commit()?;
        inner.file = Some(seal);
        self.opened(&mut inner, root);
        tracing::warn!("Rotated the root key: seal.json holds the new wraps; export the recovery bundle again");
        Ok(issued)
    }
}
