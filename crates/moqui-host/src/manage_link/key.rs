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

//! The link key (plans/archive/Vault-Instances.md §4.4): a 32-byte seed from which the node's Ed25519 TLS key (its link
//! identity, pinned by the other end) and the X25519 key its bootstrap bundle is sealed to are both derived. It must work
//! while the instance is sealed, so it is not under the root key: `runtime/db/manage_link_key`, mode 0600, or with
//! `manage@key-store="systemd-creds"` that file holds a credential `systemd-creds` made with the host's TPM2 (the seal's
//! `tpm` helper), which opens only on that host. The same key is the node's on both ends: an instance that is also a
//! manager presents it to its own instances.

use std::path::Path;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use moqui_vault::hpke;
use moqui_vault::rng::Rng as _;
use zeroize::Zeroizing;

use crate::link::Identity;

/// Where the key's file keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyStore {
    /// The seed itself, mode 0600.
    File,
    /// A credential of `systemd-creds encrypt --with-key=tpm2`, run as `command` (empty: the first there is).
    SystemdCreds { command: String },
}

impl KeyStore {
    /// From `manage@key-store` and `seal@systemd-creds-command`, as `setting` reads them. A value that is neither
    /// `file` nor `systemd-creds` is refused, never taken as the plain file.
    pub fn from_settings(setting: super::Settings<'_>) -> Result<Self, String> {
        match setting("manage@key-store").trim() {
            "systemd-creds" => Ok(Self::SystemdCreds {
                command: setting("seal@systemd-creds-command").trim().to_string(),
            }),
            "" | "file" => Ok(Self::File),
            other => Err(format!(
                "manage@key-store=\"{other}\" is not file or systemd-creds: the manager link is not started"
            )),
        }
    }
}

/// The name `systemd-creds` binds into the credential.
const CREDENTIAL_NAME: &str = "moqui-manage-link";

/// A node's link key. No `Debug` of the seed.
pub struct LinkKey {
    seed: Zeroizing<[u8; 32]>,
    identity: Identity,
    bundle: hpke::SecretKey,
}

impl std::fmt::Debug for LinkKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkKey")
            .field("fingerprint", &self.identity.fingerprint)
            .finish_non_exhaustive()
    }
}

impl LinkKey {
    /// The key of `seed`; its certificate names `name`.
    pub fn from_seed(seed: [u8; 32], name: &str) -> Result<Self, String> {
        let seed = Zeroizing::new(seed);
        let encoded = Zeroizing::new(STANDARD.encode(seed.as_slice()));
        let identity = Identity::from_key(&encoded, name)?;
        let ikm = moqui_vault::kdf::hkdf_sha256(seed.as_slice(), b"moqui-manage", b"bundle-key");
        let bundle = hpke::SecretKey::derive(ikm.as_bytes());
        Ok(Self { seed, identity, bundle })
    }

    /// A new key, from the OS's randomness.
    pub fn generate(name: &str) -> Result<Self, String> {
        let mut seed = Zeroizing::new([0_u8; 32]);
        moqui_vault::rng::OsRng.fill(seed.as_mut_slice());
        Self::from_seed(*seed, name)
    }

    /// The key kept in `file`, or a new one written there first (mode 0600, or a TPM-bound credential).
    pub fn load_or_create(file: &Path, store: &KeyStore, name: &str) -> Result<Self, String> {
        match std::fs::read(file) {
            Ok(kept) => {
                let kept = Zeroizing::new(kept);
                let seed = match store {
                    KeyStore::File => Zeroizing::new(kept.to_vec()),
                    KeyStore::SystemdCreds { command } => {
                        moqui_core::seal::methods::tpm::decrypt_credential(command, CREDENTIAL_NAME, &kept)
                            .map_err(|err| format!("the manager link key {} does not open: {err}", file.display()))?
                    }
                };
                let seed: [u8; 32] = seed.as_slice().try_into().map_err(|_| {
                    format!(
                        "{} is not a manager link key (32 bytes): remove it to make a new one, and pair again",
                        file.display()
                    )
                })?;
                Self::from_seed(seed, name)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let key = Self::generate(name)?;
                let kept = match store {
                    KeyStore::File => Zeroizing::new(key.seed.to_vec()),
                    KeyStore::SystemdCreds { command } => moqui_core::seal::methods::tpm::encrypt_credential(
                        command,
                        CREDENTIAL_NAME,
                        key.seed.as_slice(),
                    )
                    .map_err(|err| format!("the manager link key was not made: {err}"))?,
                };
                write_private(file, &kept)?;
                tracing::info!(
                    "Manager link: made the link key {} ({})",
                    file.display(),
                    key.identity.fingerprint
                );
                Ok(key)
            }
            Err(err) => Err(format!("could not read the manager link key {}: {err}", file.display())),
        }
    }

    pub(crate) fn identity(&self) -> &Identity {
        &self.identity
    }

    /// SHA-256 of the TLS public key, hex: what the other end pins.
    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.identity.fingerprint
    }

    /// The X25519 public key the bootstrap bundle is sealed to.
    pub fn bundle_public(&self) -> Result<[u8; 32], String> {
        self.bundle
            .public()
            .map(|public| *public.as_bytes())
            .map_err(|err| format!("the bundle key: {err}"))
    }

    pub(crate) fn bundle_secret(&self) -> &hpke::SecretKey {
        &self.bundle
    }

    /// The link key as the Ed25519 key it is (its seed is the TLS key's), which signs the instance's share deliveries
    /// and its unseal keys (M6): a holder checks it against the fingerprint the manager pinned
    /// (`moqui_vault::managed::link_fingerprint`).
    #[must_use]
    pub fn signing_key(&self) -> moqui_vault::sign::SigningKey {
        moqui_vault::sign::SigningKey::from_seed(&moqui_vault::Key32::from_bytes(*self.seed))
    }
}

/// Write `bytes` to `file`, readable by its owner alone, by a rename so a reader never sees half of it.
pub(crate) fn write_private(file: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write as _;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|err| format!("could not make {}: {err}", dir.display()))?;
    }
    let temporary = file.with_extension("tmp");
    // made new, so its mode is the one set here
    let _ = std::fs::remove_file(&temporary);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let written = options
        .open(&temporary)
        .and_then(|mut out| out.write_all(bytes).and_then(|()| out.sync_all()))
        .and_then(|()| std::fs::rename(&temporary, file));
    if let Err(err) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("could not write {}: {err}", file.display()));
    }
    Ok(())
}
