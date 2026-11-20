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

//! What the root key keeps in `seal.json` besides itself (plans/Sealed-Secrets.md §3.1, §6 "What the root key wraps
//! before S1"): the bootstrap secrets, the secrets needed before the database opens, each sealed under
//! HKDF(root, "bootstrap") with its name and generation as associated data, so a value moved to another name or
//! another generation does not open; and the one data key of `SecretValue` rows, wrapped by the root key. S4 fills
//! both through this API; `rotate-root` rewraps them.

use moqui_vault::kdf::hkdf_sha256;
use moqui_vault::keys::Key32;
use moqui_vault::rng::OsRng;
use zeroize::Zeroizing;

use super::file::SealedValue;
use super::{decode, encode, file, Keeper, RootKey, SealFile};
use crate::context::Error;

/// The key the bootstrap secrets are sealed under.
fn bootstrap_key(root: &RootKey) -> Key32 {
    hkdf_sha256(root.expose(), b"moqui-seal", b"bootstrap")
}

/// The root key as a vault key, to wrap the `SecretValue` data key.
fn root_as_key(root: &RootKey) -> Key32 {
    Key32::from_bytes(*root.expose())
}

fn bootstrap_ad(name: &str, generation: u64) -> [Vec<u8>; 3] {
    [
        b"moqui-seal bootstrap".to_vec(),
        name.as_bytes().to_vec(),
        generation.to_be_bytes().to_vec(),
    ]
}

fn secret_value_ad(generation: u64) -> [Vec<u8>; 2] {
    [
        b"moqui-seal secret-value-key".to_vec(),
        generation.to_be_bytes().to_vec(),
    ]
}

fn open_bootstrap(key: &Key32, name: &str, sealed: &SealedValue) -> Result<Zeroizing<Vec<u8>>, Error> {
    let ad = bootstrap_ad(name, sealed.generation);
    let parts: Vec<&[u8]> = ad.iter().map(Vec::as_slice).collect();
    moqui_vault::aead::open(key, &parts, &decode(&sealed.sealed)?)
        .map_err(|_| Error::new(format!("the bootstrap secret {name} does not open under this root key")))
}

fn seal_bootstrap(key: &Key32, name: &str, generation: u64, value: &[u8]) -> SealedValue {
    let ad = bootstrap_ad(name, generation);
    let parts: Vec<&[u8]> = ad.iter().map(Vec::as_slice).collect();
    SealedValue {
        generation,
        sealed: encode(&moqui_vault::aead::seal(key, &parts, value, &mut OsRng)),
    }
}

fn open_secret_value_key(root: &RootKey, sealed: &SealedValue) -> Result<Key32, Error> {
    let ad = secret_value_ad(sealed.generation);
    let parts: Vec<&[u8]> = ad.iter().map(Vec::as_slice).collect();
    let opened = moqui_vault::aead::open(&root_as_key(root), &parts, &decode(&sealed.sealed)?)
        .map_err(|_| Error::new("the SecretValue data key does not open under this root key"))?;
    Key32::from_slice(&opened).map_err(|err| Error::new(format!("the SecretValue data key: {err}")))
}

/// The bootstrap secrets a new seal starts with: a random crypt pass (32 bytes, base64), so a bundle exported right after
/// `init` carries the key of the encrypted fields. A conf that gives its own crypt pass leaves this one unused.
pub(super) fn initial(root: &RootKey) -> Result<std::collections::BTreeMap<String, SealedValue>, Error> {
    let pass = super::secrets::random_secret()?;
    let name = super::secrets::CRYPT_PASS;
    Ok(std::collections::BTreeMap::from([(
        name.to_string(),
        seal_bootstrap(&bootstrap_key(root), name, 1, pass.as_bytes()),
    )]))
}

pub(super) fn seal_secret_value_key(root: &RootKey, generation: u64, key: &Key32) -> SealedValue {
    let ad = secret_value_ad(generation);
    let parts: Vec<&[u8]> = ad.iter().map(Vec::as_slice).collect();
    SealedValue {
        generation,
        sealed: encode(&moqui_vault::aead::seal(
            &root_as_key(root),
            &parts,
            key.as_bytes(),
            &mut OsRng,
        )),
    }
}

/// Seal `old`'s bootstrap secrets and data key, open under `old_root`, into `new` under `new_root`.
pub(crate) fn rewrap(old: &SealFile, old_root: &RootKey, new: &mut SealFile, new_root: &RootKey) -> Result<(), Error> {
    let (old_key, new_key) = (bootstrap_key(old_root), bootstrap_key(new_root));
    for (name, sealed) in &old.bootstrap {
        let value = open_bootstrap(&old_key, name, sealed)?;
        new.bootstrap
            .insert(name.clone(), seal_bootstrap(&new_key, name, sealed.generation, &value));
    }
    if let Some(sealed) = &old.secret_value_key {
        let key = open_secret_value_key(old_root, sealed)?;
        new.secret_value_key = Some(seal_secret_value_key(new_root, sealed.generation, &key));
    }
    Ok(())
}

impl Keeper {
    /// A bootstrap secret by name, while unsealed; `None` when there is none of that name.
    ///
    /// # Errors
    ///
    /// The server is sealed, or the value does not open (moved, changed, or of another root key).
    pub fn bootstrap_secret(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, Error> {
        let inner = self.inner();
        let (Some(root), Some(seal)) = (inner.root.as_ref(), inner.file.as_ref()) else {
            return Err(Error::new("sealed: the bootstrap secrets open only once unsealed"));
        };
        seal.bootstrap
            .get(name)
            .map(|sealed| open_bootstrap(&bootstrap_key(root), name, sealed))
            .transpose()
    }

    /// Keep `value` as the bootstrap secret `name`, its generation one more than before, and write `seal.json`.
    ///
    /// # Errors
    ///
    /// The server is sealed, has no seal file (an unattended boot's root in memory only), or the file cannot be written.
    pub fn set_bootstrap_secret(&self, name: &str, value: &[u8]) -> Result<(), Error> {
        let mut inner = self.inner();
        if inner.root.is_none() || inner.file.is_none() {
            return Err(Error::new(
                "sealed, or no seal yet: a bootstrap secret is kept once unsealed",
            ));
        }
        // under the lock, on the file as it is on disk, so another process's change is not lost
        let held = file::lock(&self.path)?;
        self.refresh(&mut inner, &held)?;
        let (Some(root), Some(seal)) = (inner.root.as_ref(), inner.file.as_ref()) else {
            return Err(Error::new(
                "sealed, or no seal yet: a bootstrap secret is kept once unsealed",
            ));
        };
        let mut seal = seal.clone();
        let generation = seal.bootstrap.get(name).map_or(1, |old| old.generation + 1);
        seal.bootstrap.insert(
            name.to_string(),
            seal_bootstrap(&bootstrap_key(root), name, generation, value),
        );
        file::stage(&self.path, &seal)?.commit()?;
        inner.file = Some(seal);
        Ok(())
    }

    /// The bootstrap secret `name`, or `make`'s value kept as it when there is none, under the seal file's lock, so two
    /// processes that make one at once keep the first.
    ///
    /// # Errors
    ///
    /// As [`Self::set_bootstrap_secret`], or `make`'s error.
    pub fn bootstrap_or_init(
        &self,
        name: &str,
        make: impl FnOnce() -> Result<Vec<u8>, Error>,
    ) -> Result<Zeroizing<Vec<u8>>, Error> {
        let mut inner = self.inner();
        if inner.root.is_none() || inner.file.is_none() {
            return Err(Error::new(
                "sealed, or no seal yet: a bootstrap secret is kept once unsealed",
            ));
        }
        let held = file::lock(&self.path)?;
        self.refresh(&mut inner, &held)?;
        let (Some(root), Some(seal)) = (inner.root.as_ref(), inner.file.as_ref()) else {
            return Err(Error::new(
                "sealed, or no seal yet: a bootstrap secret is kept once unsealed",
            ));
        };
        let key = bootstrap_key(root);
        if let Some(sealed) = seal.bootstrap.get(name) {
            return open_bootstrap(&key, name, sealed);
        }
        let value = Zeroizing::new(make()?);
        let mut seal = seal.clone();
        seal.bootstrap
            .insert(name.to_string(), seal_bootstrap(&key, name, 1, &value));
        file::stage(&self.path, &seal)?.commit()?;
        inner.file = Some(seal);
        Ok(value)
    }

    /// Remove the bootstrap secret `name` from `seal.json`: false when there was none.
    ///
    /// # Errors
    ///
    /// The server is sealed, has no seal file, or the file cannot be written.
    pub fn remove_bootstrap_secret(&self, name: &str) -> Result<bool, Error> {
        let mut inner = self.inner();
        if inner.root.is_none() || inner.file.is_none() {
            return Err(Error::new(
                "sealed, or no seal yet: a bootstrap secret is removed once unsealed",
            ));
        }
        let held = file::lock(&self.path)?;
        self.refresh(&mut inner, &held)?;
        let Some(seal) = inner.file.as_ref() else {
            return Ok(false);
        };
        if !seal.bootstrap.contains_key(name) {
            return Ok(false);
        }
        let mut seal = seal.clone();
        seal.bootstrap.remove(name);
        file::stage(&self.path, &seal)?.commit()?;
        inner.file = Some(seal);
        Ok(true)
    }

    /// The bootstrap secrets' names and generations, never their values.
    ///
    /// # Errors
    ///
    /// The server is sealed.
    pub fn bootstrap_entries(&self) -> Result<Vec<(String, u64)>, Error> {
        let inner = self.inner();
        if inner.root.is_none() {
            return Err(Error::new("sealed: the bootstrap secrets are listed once unsealed"));
        }
        Ok(inner
            .file
            .as_ref()
            .map(|seal| {
                seal.bootstrap
                    .iter()
                    .map(|(name, sealed)| (name.clone(), sealed.generation))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// The data key of `SecretValue` rows, made and kept in `seal.json` on first use.
    ///
    /// # Errors
    ///
    /// The server is sealed, has no seal file, or the file cannot be written.
    pub fn secret_value_key(&self) -> Result<Key32, Error> {
        let mut inner = self.inner();
        if inner.root.is_none() || inner.file.is_none() {
            return Err(Error::new(
                "sealed, or no seal yet: the SecretValue key opens once unsealed",
            ));
        }
        let held = file::lock(&self.path)?;
        self.refresh(&mut inner, &held)?;
        let (Some(root), Some(seal)) = (inner.root.as_ref(), inner.file.as_ref()) else {
            return Err(Error::new(
                "sealed, or no seal yet: the SecretValue key opens once unsealed",
            ));
        };
        if let Some(sealed) = &seal.secret_value_key {
            return open_secret_value_key(root, sealed);
        }
        let key = Key32::random(&mut OsRng);
        let mut seal = seal.clone();
        seal.secret_value_key = Some(seal_secret_value_key(root, 1, &key));
        file::stage(&self.path, &seal)?.commit()?;
        inner.file = Some(seal);
        Ok(key)
    }
}
