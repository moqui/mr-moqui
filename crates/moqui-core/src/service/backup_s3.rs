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

//! The S3 mirror of `backup#EmbeddedDatabase`, for a `SystemMessageRemote` whose `sendUrl` is `s3://bucket`. Each
//! archive goes up once, encrypted to the remote's `publicKey`, an age recipient (`age1...`): a dump as `<name>.age`, a
//! physical backup directory as `<name>.tar.gz.age`, its
//! contents at the top of the tar. The host holds no key that decrypts them. A promotion is a copy on
//! the store, and an archive the ladder dropped is deleted there. The `backup-s3` feature; without it
//! `backup_s3_off.rs` stands in and reports that S3 is not built.

use std::collections::HashMap;
use std::io::{self, Write};

use moqui_lib::age::{Encryptor, Recipient};
use moqui_lib::error::Error as LibError;
use moqui_lib::s3::{S3Client, S3Config, S3Object};

use crate::backup::{self, BackupOutcome, KeptArchive};

const AGE: &str = ".age";
const TAR_GZ: &str = ".tar.gz";

/// Where the archives go: what [`mirror`] needs of a store.
pub(super) trait ObjectStore {
    fn list(&self, prefix: &str) -> Result<Vec<S3Object>, LibError>;
    /// Make `key` of what `fill` writes; nothing is left when it fails.
    fn put_with(&self, key: &str, fill: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>) -> Result<u64, LibError>;
    fn copy(&self, from: &str, to: &str, size: u64) -> Result<(), LibError>;
    fn delete(&self, key: &str) -> Result<(), LibError>;
}

impl ObjectStore for S3Client {
    fn list(&self, prefix: &str) -> Result<Vec<S3Object>, LibError> {
        S3Client::list(self, prefix)
    }

    fn put_with(&self, key: &str, fill: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>) -> Result<u64, LibError> {
        let mut upload = self.upload(key);
        fill(&mut upload).map_err(|err| LibError::new(format!("could not upload {key}")).with_source(err))?;
        upload.finish()
    }

    fn copy(&self, from: &str, to: &str, size: u64) -> Result<(), LibError> {
        S3Client::copy(self, from, to, size)
    }

    fn delete(&self, key: &str) -> Result<(), LibError> {
        S3Client::delete(self, key)
    }
}

/// The remote's settings: `send_url` `s3://bucket?region=...&endpoint=...`, the access key and secret, the age
/// recipient, and the key prefix (the message type's `sendPath`).
pub(super) struct S3Remote<'a> {
    pub send_url: &'a str,
    pub access_key: &'a str,
    pub secret_key: &'a str,
    pub recipient: &'a str,
    pub prefix: &'a str,
}

/// Mirror the keep set to S3. `Y` when every step went through.
pub(super) fn mirror_s3(remote: &S3Remote<'_>, outcome: &BackupOutcome) -> Result<String, LibError> {
    crate::entity::fence::check_effect(crate::entity::fence::Effect::RemoteCall, "S3 backup mirror")?;
    let sim = crate::context::ExecutionContext::with_current(|ec| ec.factory().entity())
        .flatten()
        .is_some_and(|facade| facade.sim_active());
    if sim {
        return Err(LibError::new("S3 backup mirror denied because sim mode is active"));
    }
    if remote.access_key.is_empty() || remote.secret_key.is_empty() {
        return Err(LibError::new(
            "an s3:// backup mirror needs the access key as username and the secret key as password",
        ));
    }
    let recipient = Recipient::parse(remote.recipient).map_err(|err| {
        LibError::new(format!(
            "an s3:// backup mirror needs publicKey set to an age recipient (age1...), as age-keygen prints: {err}"
        ))
    })?;
    let config = S3Config::from_url(remote.send_url, remote.access_key, remote.secret_key)?;
    let shown = format!("s3://{}/{}", config.bucket, remote.prefix);
    let client = S3Client::new(config);
    mirror(&client, &shown, remote.prefix, &recipient, outcome)
}

/// The archive name of a key under `prefix`, and the key's suffix (`.age` or `.tar.gz.age`).
fn archive_of<'a>(prefix: &str, key: &'a str) -> Option<(&'a str, &'a str)> {
    let rest = key.strip_prefix(prefix)?;
    let stem = rest.strip_suffix(AGE)?;
    let name = stem.strip_suffix(TAR_GZ).unwrap_or(stem);
    (!name.contains('/') && backup::is_archive_name(name)).then(|| (name, &rest[name.len()..]))
}

fn key_prefix(prefix: &str) -> String {
    let trimmed = prefix.trim_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}/")
    }
}

pub(super) fn mirror(
    store: &dyn ObjectStore,
    shown: &str,
    prefix: &str,
    recipient: &Recipient,
    outcome: &BackupOutcome,
) -> Result<String, LibError> {
    let prefix = key_prefix(prefix);
    let listed = store.list(&prefix).inspect_err(|err| {
        tracing::error!("backup mirror could not list {shown}: {err}");
    })?;
    // archive name to its key and size
    let mut remote: HashMap<String, (String, u64)> = HashMap::new();
    for object in &listed {
        if let Some((name, _)) = archive_of(&prefix, &object.key) {
            remote.insert(name.to_string(), (object.key.clone(), object.size));
        }
    }
    let remote_names: Vec<String> = remote.keys().cloned().collect();
    let keep_names: Vec<String> = outcome.keep.iter().map(|kept| kept.name.clone()).collect();
    let plan = backup::mirror_plan(&keep_names, &remote_names, &outcome.renames, backup::is_archive_name);
    let mut failed = false;
    for (old, new) in &plan.renames {
        let Some((from, size)) = remote.get(old).cloned() else {
            continue;
        };
        let suffix = archive_of(&prefix, &from).map_or(AGE, |(_, suffix)| suffix);
        let to = format!("{prefix}{new}{suffix}");
        let moved = store.copy(&from, &to, size).and_then(|()| store.delete(&from));
        if let Err(err) = moved {
            tracing::error!("backup mirror could not rename {from} to {to}: {err}");
            failed = true;
        }
    }
    let mut sent = 0_u64;
    for name in &plan.uploads {
        let Some(kept) = outcome.keep.iter().find(|kept| &kept.name == name) else {
            continue;
        };
        match upload(store, &prefix, kept, recipient) {
            Ok(bytes) => sent += bytes,
            Err(err) => {
                tracing::error!("backup mirror could not upload {name}: {err}");
                failed = true;
            }
        }
    }
    for name in &plan.deletes {
        let Some((key, _)) = remote.get(name) else {
            continue;
        };
        if let Err(err) = store.delete(key) {
            tracing::error!("backup mirror could not delete {key}: {err}");
            failed = true;
        }
    }
    tracing::info!(
        "backup mirror {shown}: {} uploaded ({sent} bytes), {} renamed, {} deleted",
        plan.uploads.len(),
        plan.renames.len(),
        plan.deletes.len()
    );
    if failed {
        Err(LibError::new(
            "embedded database backup mirror did not finish; the local archive is in place",
        ))
    } else {
        Ok("Y".to_string())
    }
}

/// A dump file as it is, a backup directory as a gzipped tar, each encrypted on the way up.
fn upload(store: &dyn ObjectStore, prefix: &str, kept: &KeptArchive, recipient: &Recipient) -> Result<u64, LibError> {
    let dir = kept.path.is_dir();
    let key = if dir {
        format!("{prefix}{}{TAR_GZ}{AGE}", kept.name)
    } else {
        format!("{prefix}{}{AGE}", kept.name)
    };
    store.put_with(&key, &mut |out| {
        let mut sealed = Encryptor::new(out, recipient).map_err(io::Error::other)?;
        if dir {
            let gz = flate2::write::GzEncoder::new(&mut sealed, flate2::Compression::default());
            let mut tar = tar::Builder::new(gz);
            tar.follow_symlinks(false);
            // the directory's contents at the top, not under its name, which a promotion changes on the store
            tar.append_dir_all(".", &kept.path)?;
            tar.into_inner()?.finish()?;
        } else {
            let mut file = std::fs::File::open(&kept.path)?;
            io::copy(&mut file, &mut sealed)?;
        }
        sealed.finish().map(|_| ())
    })
}

#[cfg(test)]
#[path = "backup_s3_tests.rs"]
mod tests;
