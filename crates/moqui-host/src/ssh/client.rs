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

//! The SFTP client behind `moqui_core::sftp`: a `russh` connection with the `sftp` subsystem fed to `russh-sftp`. The
//! calls run on the edge runtime and the caller's thread waits.
//!
//! Host keys: a pinned `SHA256:` fingerprint must match. Without one the key is checked against
//! `runtime/conf/ssh_known_hosts` (OpenSSH format, never `~/.ssh`); a host seen for the first time is recorded, and a
//! host whose key changed is refused with both fingerprints in the error.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moqui_core::sftp::{SftpAttributes, SftpConfig, SftpConnector, SftpEntry, SftpKind, SftpSession};
use moqui_lib::error::Error;
use moqui_lib::sync::lock;
use russh::client;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::fs::File;
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// How long a connection, a login, or one protocol call may take.
const TIMEOUT: Duration = Duration::from_secs(120);

/// Serializes reads and writes of the known-hosts file.
static KNOWN_HOSTS_LOCK: Mutex<()> = Mutex::new(());

/// Opens russh sessions; installed into core at startup.
pub struct RusshConnector {
    known_hosts: PathBuf,
}

impl RusshConnector {
    /// A connector that records first-use host keys in `known_hosts`.
    #[must_use]
    pub fn new(known_hosts: PathBuf) -> Self {
        Self { known_hosts }
    }
}

impl SftpConnector for RusshConnector {
    fn connect(&self, config: &SftpConfig) -> Result<Box<dyn SftpSession>, Error> {
        let config = config.clone();
        let known_hosts = self.known_hosts.clone();
        let session = crate::edge::block_on(async move { open(config, known_hosts).await })??;
        Ok(Box::new(session))
    }
}

/// Checks the server key, and keeps why it refused one for the error.
struct KeyCheck {
    host: String,
    port: u16,
    pinned: Option<String>,
    known_hosts: PathBuf,
    refused: Arc<Mutex<Option<String>>>,
}

impl KeyCheck {
    fn verdict(&self, key: &PublicKey) -> Result<(), String> {
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        if let Some(pinned) = &self.pinned {
            return if same_fingerprint(pinned, &fingerprint) {
                Ok(())
            } else {
                Err(format!(
                    "SSH host key of {}:{} is {fingerprint}, not the pinned {pinned}",
                    self.host, self.port
                ))
            };
        }
        let _lock = lock(&KNOWN_HOSTS_LOCK);
        match russh::keys::check_known_hosts_path(&self.host, self.port, key, &self.known_hosts) {
            Ok(true) => Ok(()),
            Ok(false) => {
                russh::keys::known_hosts::learn_known_hosts_path(&self.host, self.port, key, &self.known_hosts)
                    .map_err(|err| format!("could not record the host key in {}: {err}", self.known_hosts.display()))?;
                tracing::info!(
                    "SSH host {}:{} seen for the first time; recorded key {fingerprint} in {}",
                    self.host,
                    self.port,
                    self.known_hosts.display()
                );
                Ok(())
            }
            Err(russh::keys::Error::KeyChanged { line }) => {
                let recorded = russh::keys::known_hosts::known_host_keys_path(&self.host, self.port, &self.known_hosts)
                    .ok()
                    .and_then(|keys| keys.into_iter().find(|(at, _)| *at == line))
                    .map_or_else(
                        || "unknown".to_string(),
                        |(_, old)| old.fingerprint(HashAlg::Sha256).to_string(),
                    );
                Err(format!(
                    "SSH host key of {}:{} changed: it is {fingerprint}, and {} line {line} has {recorded}; refusing to connect",
                    self.host,
                    self.port,
                    self.known_hosts.display()
                ))
            }
            Err(err) => Err(format!(
                "could not check the host key in {}: {err}",
                self.known_hosts.display()
            )),
        }
    }
}

impl client::Handler for KeyCheck {
    type Error = russh::Error;

    async fn check_server_key(&mut self, server_public_key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        match self.verdict(&server_public_key.public_key()) {
            Ok(()) => Ok(true),
            Err(reason) => {
                *lock(&self.refused) = Some(reason);
                Ok(false)
            }
        }
    }
}

/// `SHA256:abc` and `abc` are the same, and base64 padding does not matter.
pub(crate) fn same_fingerprint(pinned: &str, actual: &str) -> bool {
    let clean = |text: &str| {
        text.trim()
            .trim_start_matches("SHA256:")
            .trim_end_matches('=')
            .to_string()
    };
    clean(pinned) == clean(actual)
}

async fn open(config: SftpConfig, known_hosts: PathBuf) -> Result<RusshSession, Error> {
    let target = config.target();
    let refused = Arc::new(Mutex::new(None));
    let check = KeyCheck {
        host: config.host.clone(),
        port: config.port,
        pinned: config.host_key_fingerprint.clone(),
        known_hosts,
        refused: Arc::clone(&refused),
    };
    let ssh_config = Arc::new(client::Config {
        inactivity_timeout: Some(Duration::from_secs(600)),
        ..client::Config::default()
    });
    let connecting = client::connect(ssh_config, (config.host.as_str(), config.port), check);
    let mut handle = match tokio::time::timeout(TIMEOUT, connecting).await {
        Ok(Ok(handle)) => handle,
        Ok(Err(err)) => {
            let reason = lock(&refused).take();
            return Err(Error::new(
                reason.unwrap_or_else(|| format!("could not connect to {target}: {err}")),
            ));
        }
        Err(_) => return Err(Error::new(format!("timed out connecting to {target}"))),
    };
    let authenticated = authenticate(&mut handle, &config).await;
    if let Err(err) = authenticated {
        tracing::error!("Authentication failed for {target}: {err}");
        let _ = handle.disconnect(russh::Disconnect::ByApplication, "", "").await;
        return Err(err);
    }
    let channel = handle
        .channel_open_session()
        .await
        .map_err(|err| Error::new(format!("could not open an SSH channel to {target}: {err}")))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|err| Error::new(format!("{target} refused the sftp subsystem: {err}")))?;
    let sftp = russh_sftp::client::SftpSession::new(channel.into_stream())
        .await
        .map_err(|err| Error::new(format!("could not start SFTP with {target}: {err}")))?;
    sftp.set_timeout(TIMEOUT.as_secs());
    Ok(RusshSession {
        inner: Arc::new(Inner {
            handle: tokio::sync::Mutex::new(Some(handle)),
            sftp,
            files: tokio::sync::Mutex::new(HashMap::new()),
            target,
        }),
        next_handle: 0,
        open: true,
    })
}

async fn authenticate(handle: &mut client::Handle<KeyCheck>, config: &SftpConfig) -> Result<(), Error> {
    let target = config.target();
    if let Some(password) = &config.password {
        let result = handle
            .authenticate_password(config.username.clone(), password.clone())
            .await
            .map_err(|err| Error::new(format!("password login to {target} failed: {err}")))?;
        return if result.success() {
            Ok(())
        } else {
            Err(Error::new(format!("Authentication by password failed for {target}")))
        };
    }
    let rsa_hash = handle
        .best_supported_rsa_hash()
        .await
        .map_err(|err| Error::new(format!("{target}: {err}")))?
        .flatten();
    for (index, key) in config.keys.iter().enumerate() {
        let private = russh::keys::decode_secret_key(&key.private_key, None)
            .map_err(|err| Error::new(format!("private key {} for {target} does not decode: {err}", index + 1)))?;
        let with_hash = PrivateKeyWithHashAlg::new(Arc::new(private), rsa_hash);
        let result = handle
            .authenticate_publickey(config.username.clone(), with_hash)
            .await
            .map_err(|err| Error::new(format!("public key login to {target} failed: {err}")))?;
        if result.success() {
            return Ok(());
        }
    }
    Err(Error::new(format!("Authentication by public key failed for {target}")))
}

struct Inner {
    handle: tokio::sync::Mutex<Option<client::Handle<KeyCheck>>>,
    sftp: russh_sftp::client::SftpSession,
    files: tokio::sync::Mutex<HashMap<u64, File>>,
    target: String,
}

/// One connected session.
pub struct RusshSession {
    inner: Arc<Inner>,
    next_handle: u64,
    open: bool,
}

fn failed(path: &str, err: &SftpError) -> Error {
    Error::new(format!("SFTP {path}: {err}"))
}

fn not_found(err: &SftpError) -> bool {
    matches!(err, SftpError::Status(status) if status.status_code == StatusCode::NoSuchFile)
}

fn attributes(attrs: &FileAttributes) -> SftpAttributes {
    let kind = if attrs.is_dir() {
        SftpKind::Directory
    } else if attrs.is_regular() {
        SftpKind::File
    } else {
        SftpKind::Other
    };
    SftpAttributes {
        kind,
        size: attrs.size,
        access_time: attrs.atime,
        modified_time: attrs.mtime,
    }
}

impl RusshSession {
    fn run<T, F>(&self, work: impl FnOnce(Arc<Inner>) -> F) -> Result<T, Error>
    where
        F: std::future::Future<Output = Result<T, Error>> + Send + 'static,
        T: Send + 'static,
    {
        if !self.open {
            return Err(Error::new("SFTP Client not connected"));
        }
        crate::edge::block_on(work(Arc::clone(&self.inner)))?
    }
}

impl SftpSession for RusshSession {
    fn read_dir(&mut self, path: &str) -> Result<Vec<SftpEntry>, Error> {
        let path = path.to_string();
        self.run(|inner| async move {
            let entries = inner
                .sftp
                .read_dir(path.clone())
                .await
                .map_err(|err| failed(&path, &err))?;
            Ok(entries
                .map(|entry| SftpEntry {
                    name: entry.file_name(),
                    attributes: attributes(&entry.metadata()),
                })
                .collect())
        })
    }

    fn stat(&mut self, path: &str) -> Result<Option<SftpAttributes>, Error> {
        let path = path.to_string();
        self.run(|inner| async move {
            match inner.sftp.metadata(path.clone()).await {
                Ok(attrs) => Ok(Some(attributes(&attrs))),
                Err(err) if not_found(&err) => Ok(None),
                Err(err) => Err(failed(&path, &err)),
            }
        })
    }

    fn read(&mut self, path: &str) -> Result<Vec<u8>, Error> {
        let path = path.to_string();
        self.run(|inner| async move { inner.sftp.read(path.clone()).await.map_err(|err| failed(&path, &err)) })
    }

    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error> {
        let path = path.to_string();
        let bytes = bytes.to_vec();
        self.run(|inner| async move {
            let mut file = inner
                .sftp
                .create(path.clone())
                .await
                .map_err(|err| failed(&path, &err))?;
            file.write_all(&bytes)
                .await
                .map_err(|err| Error::new(format!("SFTP {path}: {err}")))?;
            file.shutdown()
                .await
                .map_err(|err| Error::new(format!("SFTP {path}: {err}")))
        })
    }

    fn set_permissions(&mut self, path: &str, mode: u32) -> Result<(), Error> {
        let path = path.to_string();
        self.run(|inner| async move {
            let attrs = FileAttributes {
                permissions: Some(mode),
                ..FileAttributes::empty()
            };
            inner
                .sftp
                .set_metadata(path.clone(), attrs)
                .await
                .map_err(|err| failed(&path, &err))
        })
    }

    fn make_dir(&mut self, path: &str) -> Result<(), Error> {
        let path = path.to_string();
        self.run(|inner| async move {
            inner
                .sftp
                .create_dir(path.clone())
                .await
                .map_err(|err| failed(&path, &err))
        })
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), Error> {
        let (from, to) = (from.to_string(), to.to_string());
        self.run(|inner| async move {
            inner
                .sftp
                .rename(from.clone(), to.clone())
                .await
                .map_err(|err| failed(&format!("{from} to {to}"), &err))
        })
    }

    fn remove_file(&mut self, path: &str) -> Result<(), Error> {
        let path = path.to_string();
        self.run(|inner| async move {
            inner
                .sftp
                .remove_file(path.clone())
                .await
                .map_err(|err| failed(&path, &err))
        })
    }

    fn remove_dir(&mut self, path: &str) -> Result<(), Error> {
        let path = path.to_string();
        self.run(|inner| async move {
            inner
                .sftp
                .remove_dir(path.clone())
                .await
                .map_err(|err| failed(&path, &err))
        })
    }

    fn open(&mut self, path: &str, write: bool) -> Result<u64, Error> {
        self.next_handle += 1;
        let id = self.next_handle;
        let path = path.to_string();
        self.run(move |inner| async move {
            // Java opens a write stream with READ, WRITE, CREAT and TRUNC.
            let flags = if write {
                OpenFlags::READ | OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE
            } else {
                OpenFlags::READ
            };
            let file = inner
                .sftp
                .open_with_flags(path.clone(), flags)
                .await
                .map_err(|err| failed(&path, &err))?;
            inner.files.lock().await.insert(id, file);
            Ok(id)
        })
    }

    fn read_chunk(&mut self, handle: u64, max: usize) -> Result<Vec<u8>, Error> {
        self.run(move |inner| async move {
            let mut files = inner.files.lock().await;
            let file = files
                .get_mut(&handle)
                .ok_or_else(|| Error::new("SFTP stream is closed"))?;
            let mut buffer = vec![0u8; max.clamp(1, 1 << 20)];
            let read = file
                .read(&mut buffer)
                .await
                .map_err(|err| Error::new(format!("SFTP read: {err}")))?;
            buffer.truncate(read);
            Ok(buffer)
        })
    }

    fn write_chunk(&mut self, handle: u64, bytes: &[u8]) -> Result<(), Error> {
        let bytes = bytes.to_vec();
        self.run(move |inner| async move {
            let mut files = inner.files.lock().await;
            let file = files
                .get_mut(&handle)
                .ok_or_else(|| Error::new("SFTP stream is closed"))?;
            file.write_all(&bytes)
                .await
                .map_err(|err| Error::new(format!("SFTP write: {err}")))
        })
    }

    fn close_handle(&mut self, handle: u64) -> Result<(), Error> {
        self.run(move |inner| async move {
            let file = inner.files.lock().await.remove(&handle);
            match file {
                Some(mut file) => file
                    .shutdown()
                    .await
                    .map_err(|err| Error::new(format!("SFTP close: {err}"))),
                None => Ok(()),
            }
        })
    }

    fn close(&mut self) {
        if !self.open {
            return;
        }
        self.open = false;
        let inner = Arc::clone(&self.inner);
        let _ = crate::edge::block_on(async move {
            for (_, mut file) in inner.files.lock().await.drain() {
                let _ = file.shutdown().await;
            }
            let _ = inner.sftp.close().await;
            if let Some(handle) = inner.handle.lock().await.take() {
                let _ = handle.disconnect(russh::Disconnect::ByApplication, "", "").await;
            }
            tracing::debug!("closed SFTP session to {}", inner.target);
        });
    }

    fn is_connected(&self) -> bool {
        self.open
            && self
                .inner
                .handle
                .try_lock()
                .map_or(true, |handle| handle.as_ref().is_some_and(|handle| !handle.is_closed()))
    }
}

impl Drop for RusshSession {
    fn drop(&mut self) {
        self.close();
    }
}
