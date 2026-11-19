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

//! The backup key (plans/Security-Review.md SEC-CLI-114): an age X25519 identity the app makes once and keeps in the
//! OS key store, as the device keys are (`client::keystore::Secrets`, the entry [`ENTRY`]). Each backup, the node's
//! nightly one and the tray's, is sealed to its public key as it is written (`database-backup@recipient`, `moqui
//! save-db --recipient`), so the backup folder, which the user's own backup or a cloud may copy, holds no plain copy of
//! the data. A restore on this computer opens a backup with the key from the store; on another, with the key file the
//! tray's "Save backup key…" wrote. Without the key a backup does not restore, which the notice and the tray say.

use std::path::Path;

use jiff::Timestamp;
use moqui_lib::age::Identity;

/// The key store entry of the backup key.
pub(crate) const ENTRY: &str = "backup-key";

/// A store of secrets: [`crate::client::keystore::Secrets`], or a test's.
pub(crate) trait SecretStore {
    /// The secret `name`, none when there is none; an error when the store does not answer.
    fn read(&self, name: &str) -> Result<Option<String>, String>;
    /// Keep `value` as `name`.
    fn write(&self, name: &str, value: &str) -> Result<(), String>;
}

impl SecretStore for crate::client::keystore::Secrets {
    fn read(&self, name: &str) -> Result<Option<String>, String> {
        Self::read(self, name)
    }

    fn write(&self, name: &str, value: &str) -> Result<(), String> {
        Self::write(self, name, value)
    }
}

/// The backup key in `store`, made and kept the first time. A store that does not answer is an error, never a reason
/// to make a new key: that would leave every backup sealed before without the key that opens it.
///
/// # Errors
///
/// Returns why when the store does not answer, its key does not read, or a new one is not kept.
pub(crate) fn key_in(store: &dyn SecretStore) -> Result<Identity, String> {
    if let Some(text) = store.read(ENTRY)? {
        return identity_of(&text).map_err(|err| format!("the backup key in the key store does not read: {err}"));
    }
    let identity = Identity::generate().map_err(|err| err.to_string())?;
    store
        .write(ENTRY, &identity.secret_text())
        .map_err(|err| format!("a new backup key was not kept: {err}"))?;
    tracing::info!(
        "A backup key was made; the backups are sealed to {}",
        identity.recipient()
    );
    Ok(identity)
}

/// The backup key of the app: in the OS key store, or the file store under the app's config directory.
///
/// # Errors
///
/// Returns why when there is no config directory or [`key_in`] fails.
pub(crate) fn app_key(app: &tauri::AppHandle) -> Result<Identity, String> {
    use tauri::Manager;
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|err| err.to_string())?
        .join("backup-key");
    key_in(&crate::client::keystore::Secrets::open(&dir))
}

/// The identity in `text`, an age identity file: its `AGE-SECRET-KEY-1...` line, `#` comments and blank lines passed
/// over.
///
/// # Errors
///
/// Returns why when there is no identity line or it does not parse.
pub(crate) fn identity_of(text: &str) -> Result<Identity, String> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or("");
    Identity::parse(line).map_err(|err| err.to_string())
}

/// The text of a backup key file, as `age-keygen` writes one (`age -d -i <file>` reads it), with what it is for.
pub(crate) fn key_file_text(identity: &Identity, made: Timestamp) -> String {
    format!(
        "# MR Moqui backup key: it opens the backups of the MR Moqui app that made it. Keep it apart from the\n\
         # backups, somewhere safe: without it they do not restore, and whoever has both reads all the data.\n\
         # created: {}\n# public key: {}\n{}\n",
        made.strftime("%Y-%m-%dT%H:%M:%SZ"),
        identity.recipient(),
        identity.secret_text()
    )
}

/// Write the key file of `identity` at `path`, readable by this user only.
///
/// # Errors
///
/// Returns why when it is not written.
pub(crate) fn save_key_file(identity: &Identity, path: &Path) -> Result<(), String> {
    use std::io::Write as _;
    let mut file = moqui_lib::private_fs::file_options()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|err| format!("{}: {err}", path.display()))?;
    moqui_lib::private_fs::restrict(path).map_err(|err| format!("{}: {err}", path.display()))?;
    file.write_all(key_file_text(identity, Timestamp::now()).as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|err| format!("{}: {err}", path.display()))
}

/// Whether `identity` opens the sealed backup at `dump`: its header, which names the key it was sealed to, is read
/// and checked, not the whole file.
pub(crate) fn opens(identity: &Identity, dump: &Path) -> bool {
    std::fs::File::open(dump)
        .map_err(|err| err.to_string())
        .and_then(|file| {
            moqui_lib::age::Decryptor::new(std::io::BufReader::new(file), identity).map_err(|err| err.to_string())
        })
        .is_ok()
}

/// Whether `name` is a sealed backup (`.dump.age`).
pub(crate) fn is_sealed(name: &str) -> bool {
    name.ends_with(".dump.age")
}

#[cfg(test)]
#[path = "backup_key_tests.rs"]
mod tests;
