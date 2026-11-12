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

//! The backup mirror as objects on a store: S3 (`sendUrl` `s3://bucket`), or files over SFTP when the archives are
//! encrypted. Each archive goes up once, as one object: a dump as `<name>`, a physical backup directory as
//! `<name>.tar.gz` with its contents at the top of the tar. Encrypted, each is sealed to an age recipient (`age1...`) on
//! the way up and gets `.age` on its name; the host holds no key that decrypts them. A promotion is a rename on the
//! store (S3: a copy and a delete), and an archive the ladder dropped is deleted there. The `backup-s3` feature;
//! without it `backup_store_off.rs` stands in and reports that it is not built.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{self, Write};

use moqui_lib::age::{Encryptor, Recipient};
use moqui_lib::error::Error as LibError;
use moqui_lib::s3::{S3Client, S3Config, S3Object};

use crate::backup::{self, BackupOutcome, KeptArchive};
use crate::sftp::{SftpClient, SftpKind};

const AGE: &str = ".age";
const TAR_GZ: &str = ".tar.gz";

/// Where the archives go: what [`mirror`] needs of a store.
pub(super) trait ObjectStore {
    fn list(&self, prefix: &str) -> Result<Vec<S3Object>, LibError>;
    /// Make `key` of what `fill` writes; nothing is left when it fails.
    fn put_with(&self, key: &str, fill: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>) -> Result<u64, LibError>;
    /// Move `from` (of `size` bytes) to `to`.
    fn rename(&self, from: &str, to: &str, size: u64) -> Result<(), LibError>;
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

    fn rename(&self, from: &str, to: &str, size: u64) -> Result<(), LibError> {
        S3Client::copy(self, from, to, size)?;
        S3Client::delete(self, from)
    }

    fn delete(&self, key: &str) -> Result<(), LibError> {
        S3Client::delete(self, key)
    }
}

/// An open SFTP connection as a store: keys are paths, the prefix the directory.
struct SftpStore<'a>(RefCell<&'a mut SftpClient>);

impl ObjectStore for SftpStore<'_> {
    fn list(&self, prefix: &str) -> Result<Vec<S3Object>, LibError> {
        let mut client = self.0.borrow_mut();
        Ok(client
            .ls(prefix.trim_end_matches('/'))?
            .into_iter()
            .filter(|(_, entry)| entry.attributes.kind == SftpKind::File)
            .map(|(_, entry)| S3Object {
                key: format!("{prefix}{}", entry.name),
                size: entry.attributes.size.unwrap_or(0),
            })
            .collect())
    }

    fn put_with(&self, key: &str, fill: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>) -> Result<u64, LibError> {
        self.0.borrow_mut().put_with(key, fill)
    }

    fn rename(&self, from: &str, to: &str, _size: u64) -> Result<(), LibError> {
        self.0.borrow_mut().rename(from, to).map(|_| ())
    }

    fn delete(&self, key: &str) -> Result<(), LibError> {
        self.0.borrow_mut().rm(key)
    }
}

/// The S3 remote: `send_url` `s3://bucket?region=...&endpoint=...`, the access key and secret, and the key prefix
/// (the message type's `sendPath`).
pub(super) struct S3Remote<'a> {
    pub send_url: &'a str,
    pub access_key: &'a str,
    pub secret_key: &'a str,
    pub prefix: &'a str,
}

/// Mirror the keep set to S3, encrypted to `recipient` when there is one. `Y` when every step went through.
pub(super) fn mirror_s3(
    remote: &S3Remote<'_>,
    recipient: Option<&str>,
    outcome: &BackupOutcome,
) -> Result<String, LibError> {
    crate::entity::fence::check_effect(crate::entity::fence::Effect::RemoteCall, "S3 backup mirror")?;
    let sim = crate::context::ExecutionContext::with_current(|ec| ec.factory().entity())
        .flatten()
        .is_some_and(|facade| facade.sim_active());
    if sim {
        return Err(LibError::new("S3 backup mirror denied because sim mode is active"));
    }
    if remote.access_key.is_empty() || remote.secret_key.is_empty() {
        return Err(LibError::new(
            "an S3 backup mirror needs the access key as username and the secret key as password",
        ));
    }
    let recipient = recipient.map(parse_recipient).transpose()?;
    let config = S3Config::from_url(remote.send_url, remote.access_key, remote.secret_key)?;
    let shown = format!("s3://{}/{}", config.bucket, remote.prefix);
    let client = S3Client::new(config);
    mirror(&client, &shown, &key_prefix(remote.prefix), recipient.as_ref(), outcome)
}

/// Mirror the keep set into `dir` over a connected SFTP client, each archive one file encrypted to `recipient`.
pub(super) fn mirror_sftp_encrypted(
    client: &mut SftpClient,
    dir: &str,
    recipient: &str,
    outcome: &BackupOutcome,
) -> Result<String, LibError> {
    let recipient = parse_recipient(recipient)?;
    client.mkdirs(dir)?;
    let shown = format!("sftp {dir}");
    let prefix = format!("{}/", dir.trim_end_matches('/'));
    mirror(
        &SftpStore(RefCell::new(client)),
        &shown,
        &prefix,
        Some(&recipient),
        outcome,
    )
}

fn parse_recipient(text: &str) -> Result<Recipient, LibError> {
    Recipient::parse(text).map_err(|err| {
        LibError::new(format!(
            "an encrypted backup mirror needs mirrorRecipient set to an age recipient (age1...), as age-keygen \
             prints: {err}"
        ))
    })
}

/// The archive name of a key under `prefix`, and the key's suffix: none or `.tar.gz`, then `.age` when `sealed`.
/// The other mode's objects are not archives here, so turning encryption on or off sends every archive again.
fn archive_of<'a>(prefix: &str, key: &'a str, sealed: bool) -> Option<(&'a str, &'a str)> {
    let rest = key.strip_prefix(prefix)?;
    let stem = if sealed { rest.strip_suffix(AGE)? } else { rest };
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

/// Mirror the keep set to `store` under `prefix` (empty, or ending in `/`).
pub(super) fn mirror(
    store: &dyn ObjectStore,
    shown: &str,
    prefix: &str,
    recipient: Option<&Recipient>,
    outcome: &BackupOutcome,
) -> Result<String, LibError> {
    let sealed = recipient.is_some();
    let listed = store.list(prefix).inspect_err(|err| {
        tracing::error!("backup mirror could not list {shown}: {err}");
    })?;
    // archive name to its key and size
    let mut remote: HashMap<String, (String, u64)> = HashMap::new();
    for object in &listed {
        if let Some((name, _)) = archive_of(prefix, &object.key, sealed) {
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
        let suffix = archive_of(prefix, &from, sealed).map_or("", |(_, suffix)| suffix);
        let to = format!("{prefix}{new}{suffix}");
        if let Err(err) = store.rename(&from, &to, size) {
            tracing::error!("backup mirror could not rename {from} to {to}: {err}");
            failed = true;
        }
    }
    let mut sent = 0_u64;
    for name in &plan.uploads {
        let Some(kept) = outcome.keep.iter().find(|kept| &kept.name == name) else {
            continue;
        };
        match upload(store, prefix, kept, recipient) {
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

/// A dump file as it is, a backup directory as a gzipped tar, each encrypted on the way up when there is a recipient.
fn upload(
    store: &dyn ObjectStore,
    prefix: &str,
    kept: &KeptArchive,
    recipient: Option<&Recipient>,
) -> Result<u64, LibError> {
    let dir = kept.path.is_dir();
    let key = format!(
        "{prefix}{}{}{}",
        kept.name,
        if dir { TAR_GZ } else { "" },
        if recipient.is_some() { AGE } else { "" }
    );
    store.put_with(&key, &mut |out| match recipient {
        Some(recipient) => {
            let mut sealed = Encryptor::new(out, recipient).map_err(io::Error::other)?;
            write_archive(kept, &mut sealed)?;
            sealed.finish().map(|_| ())
        }
        None => write_archive(kept, out),
    })
}

fn write_archive(kept: &KeptArchive, out: &mut dyn Write) -> io::Result<()> {
    if kept.path.is_dir() {
        let gz = flate2::write::GzEncoder::new(out, flate2::Compression::default());
        let mut tar = tar::Builder::new(gz);
        tar.follow_symlinks(false);
        // the directory's contents at the top, not under its name, which a promotion changes on the store
        tar.append_dir_all(".", &kept.path)?;
        tar.into_inner()?.finish()?;
    } else {
        let mut file = std::fs::File::open(&kept.path)?;
        io::copy(&mut file, out)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "backup_store_tests.rs"]
mod tests;
