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

//! The SFTP client of the `moqui-sftp` component, `org.moqui.sftp.SftpClient`. Core has no SSH stack: the host
//! installs an [`SftpConnector`] (russh and russh-sftp) on each factory that does the protocol calls, and [`SftpClient`]
//! keeps the Java behavior on top of them: `host:port` parsing (port 22 by default, where Java used 115), the
//! directory-or-file target of `put`, `createDir`, `moveFile`, `renameFile`, charsets, and the messages a script sees.
//!
//! Java accepted any host key. Here a connection checks the remote's pinned SHA-256 fingerprint when one is given,
//! else trusts the key on first use and refuses a key that changed (`runtime/conf/ssh_known_hosts`); the host does
//! the check.

use std::sync::{Arc, RwLock};

use moqui_lib::error::Error;
use moqui_lib::sync::{read, write};

use crate::context::{ExecutionContext, ExecutionContextFactory};

/// Java's `SftpClient.SFTP_PORT`. Java made it the default port, but 115 is the old Simple File Transfer Protocol, not
/// SSH; the default here is [`SSH_PORT`].
pub const SFTP_PORT: u16 = 115;
/// `SftpClient.SSH_PORT`, the default when the host string has no port.
pub const SSH_PORT: u16 = 22;

/// What a caller sees when it uses SFTP and the host installed no connector.
pub const SFTP_UNAVAILABLE: &str = "SFTP is not available in this build";

/// How to reach and log in to one server. `Debug` leaves out the password and the private keys.
#[derive(Clone, Default)]
pub struct SftpConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Option<String>,
    /// Private keys in PEM (OpenSSH, PKCS#1, PKCS#8, or PuTTY), each with its public key when one was given. Tried in
    /// order when there is no password.
    pub keys: Vec<SftpKey>,
    /// A pinned `SHA256:...` fingerprint of the server's host key. Empty trusts on first use.
    pub host_key_fingerprint: Option<String>,
}

#[derive(Clone)]
pub struct SftpKey {
    pub private_key: String,
    pub public_key: Option<String>,
}

const REDACTED: &str = "<redacted>";

impl std::fmt::Debug for SftpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SftpConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| REDACTED))
            .field("keys", &self.keys)
            .field("host_key_fingerprint", &self.host_key_fingerprint)
            .finish()
    }
}

impl std::fmt::Debug for SftpKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SftpKey")
            .field("private_key", &REDACTED)
            .field("public_key", &self.public_key)
            .finish()
    }
}

impl SftpConfig {
    /// `who@host:port`, for messages.
    #[must_use]
    pub fn target(&self) -> String {
        format!("{}@{}:{}", self.username, self.host, self.port)
    }
}

/// The kind of a remote directory entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SftpKind {
    File,
    Directory,
    Other,
}

/// What `stat` or a directory listing reports for one name. Times are seconds since the epoch, as SSH carries them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SftpAttributes {
    pub kind: SftpKind,
    pub size: Option<u64>,
    pub access_time: Option<u32>,
    pub modified_time: Option<u32>,
}

/// One entry of a remote directory. `.` and `..` are left out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SftpEntry {
    pub name: String,
    pub attributes: SftpAttributes,
}

/// An open SFTP session: the protocol calls the host makes. Paths are the server's.
pub trait SftpSession: Send {
    /// # Errors
    ///
    /// Every call returns the server's error, or that the connection closed.
    fn read_dir(&mut self, path: &str) -> Result<Vec<SftpEntry>, Error>;
    /// `None` when nothing is at `path`.
    ///
    /// # Errors
    ///
    /// Returns an error other than "no such file".
    fn stat(&mut self, path: &str) -> Result<Option<SftpAttributes>, Error>;
    /// # Errors
    ///
    /// See [`SftpSession::read_dir`].
    fn read(&mut self, path: &str) -> Result<Vec<u8>, Error>;
    /// Create or truncate the file and write `bytes`.
    ///
    /// # Errors
    ///
    /// See [`SftpSession::read_dir`].
    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error>;
    /// Set the permission bits, as SSHJ's preserve-attributes does after an upload.
    ///
    /// # Errors
    ///
    /// See [`SftpSession::read_dir`].
    fn set_permissions(&mut self, path: &str, mode: u32) -> Result<(), Error>;
    /// # Errors
    ///
    /// See [`SftpSession::read_dir`].
    fn make_dir(&mut self, path: &str) -> Result<(), Error>;
    /// # Errors
    ///
    /// See [`SftpSession::read_dir`].
    fn rename(&mut self, from: &str, to: &str) -> Result<(), Error>;
    /// # Errors
    ///
    /// See [`SftpSession::read_dir`].
    fn remove_file(&mut self, path: &str) -> Result<(), Error>;
    /// Remove an empty directory.
    ///
    /// # Errors
    ///
    /// See [`SftpSession::read_dir`].
    fn remove_dir(&mut self, path: &str) -> Result<(), Error>;
    /// Open a file for streaming. Writing creates or truncates it. Returns a handle for the chunk calls.
    ///
    /// # Errors
    ///
    /// See [`SftpSession::read_dir`].
    fn open(&mut self, path: &str, write: bool) -> Result<u64, Error>;
    /// Up to `max` bytes; empty at the end of the file.
    ///
    /// # Errors
    ///
    /// See [`SftpSession::read_dir`].
    fn read_chunk(&mut self, handle: u64, max: usize) -> Result<Vec<u8>, Error>;
    /// # Errors
    ///
    /// See [`SftpSession::read_dir`].
    fn write_chunk(&mut self, handle: u64, bytes: &[u8]) -> Result<(), Error>;
    /// # Errors
    ///
    /// See [`SftpSession::read_dir`].
    fn close_handle(&mut self, handle: u64) -> Result<(), Error>;
    /// End the session and the SSH connection.
    fn close(&mut self);
    /// False once the connection closed.
    fn is_connected(&self) -> bool;
}

/// Opens sessions. The host installs one on each factory.
pub trait SftpConnector: Send + Sync {
    /// Connect, check the host key, authenticate, and start the `sftp` subsystem.
    ///
    /// # Errors
    ///
    /// Returns an error when the connection, the host key check, or the login fails.
    fn connect(&self, config: &SftpConfig) -> Result<Box<dyn SftpSession>, Error>;
}

/// The factory's connector. A connector carries the runtime's own state (its `ssh_known_hosts`), so each factory
/// has its own.
#[derive(Default)]
struct Slot {
    connector: RwLock<Option<Arc<dyn SftpConnector>>>,
}

/// Use `connector` for the SFTP connections made under `factory`.
pub fn install_sftp_connector(factory: &ExecutionContextFactory, connector: Arc<dyn SftpConnector>) {
    let slot = factory.extension(|_| Slot::default());
    *write(&slot.connector) = Some(connector);
}

/// Remove the connector installed on `factory`.
pub fn clear_sftp_connector(factory: &ExecutionContextFactory) {
    let slot = factory.extension(|_| Slot::default());
    *write(&slot.connector) = None;
}

/// The connector of the entered context's factory.
fn connector() -> Result<Arc<dyn SftpConnector>, Error> {
    ExecutionContext::with_current(|ec| {
        let slot = ec.factory().extension(|_| Slot::default());
        let found = read(&slot.connector).clone();
        found
    })
    .flatten()
    .ok_or_else(|| Error::new(SFTP_UNAVAILABLE))
}

/// `host`, `host:port`, `[v6]`, or `[v6]:port`. A host with more than one colon and no brackets is an IPv6 address
/// without a port.
fn split_host_port(text: &str, default_port: u16) -> Result<(String, u16), Error> {
    let parse_port = |port: &str| {
        port.trim()
            .parse::<u16>()
            .map_err(|_| Error::new(format!("SFTP port {port} in {text} is not a number")))
    };
    if let Some(rest) = text.strip_prefix('[') {
        let Some((address, after)) = rest.split_once(']') else {
            return Err(Error::new(format!("SFTP host {text} has no closing bracket")));
        };
        return match after.strip_prefix(':') {
            Some(port) => Ok((address.to_string(), parse_port(port)?)),
            None if after.is_empty() => Ok((address.to_string(), default_port)),
            None => Err(Error::new(format!("SFTP host {text} has text after the bracket"))),
        };
    }
    match text.split_once(':') {
        Some((host, port)) if !host.is_empty() && !port.contains(':') => Ok((host.to_string(), parse_port(port)?)),
        _ => Ok((text.to_string(), default_port)),
    }
}

/// The Java client: build it, `connect`, call, `close`.
pub struct SftpClient {
    config: SftpConfig,
    preserve_attributes: bool,
    session: Option<Box<dyn SftpSession>>,
}

impl SftpClient {
    /// `SftpClient(host, username, port)`: a `host:port` string wins over `port`, which defaults to [`SSH_PORT`]. An
    /// IPv6 address takes a port as `[address]:port`; one without brackets is all host.
    ///
    /// # Errors
    ///
    /// Returns an error when the port after the colon is not a number.
    pub fn new(host: &str, username: &str, port: Option<u16>) -> Result<Self, Error> {
        let (host, port) = split_host_port(host, port.unwrap_or(SSH_PORT))?;
        Ok(Self {
            config: SftpConfig {
                host,
                port,
                username: username.to_string(),
                ..SftpConfig::default()
            },
            preserve_attributes: true,
            session: None,
        })
    }

    /// Authenticate with a password.
    pub fn password(&mut self, password: &str) -> &mut Self {
        self.config.password = Some(password.to_string());
        self
    }

    /// Authenticate with a private key (PEM: PKCS#8, PKCS#1, OpenSSH, or PuTTY) and its public key. May be called
    /// more than once; the keys are tried in order.
    pub fn public_key_pkcs8(&mut self, private_key: &str, public_key: Option<&str>) -> &mut Self {
        self.config.keys.push(SftpKey {
            private_key: private_key.to_string(),
            public_key: public_key.filter(|text| !text.is_empty()).map(str::to_string),
        });
        self
    }

    /// False skips setting the file attributes after a put, for servers that refuse it.
    pub fn preserve_attributes(&mut self, preserve: bool) -> &mut Self {
        self.preserve_attributes = preserve;
        self
    }

    /// Pin the server's host key by its `SHA256:...` fingerprint.
    pub fn host_key_fingerprint(&mut self, fingerprint: &str) -> &mut Self {
        let fingerprint = fingerprint.trim();
        self.config.host_key_fingerprint = (!fingerprint.is_empty()).then(|| fingerprint.to_string());
        self
    }

    #[must_use]
    pub fn config(&self) -> &SftpConfig {
        &self.config
    }

    /// Connect, authenticate, and start SFTP.
    ///
    /// # Errors
    ///
    /// Java's messages for a missing host, username, or credential; else the connector's error.
    pub fn connect(&mut self) -> Result<&mut Self, Error> {
        // nothing goes out under the read fence or in a sim preview, nor in sim mode, as remote messages do
        crate::entity::fence::check_effect(
            crate::entity::fence::Effect::RemoteCall,
            &format!("SFTP {}", self.config.target()),
        )?;
        let sim = ExecutionContext::with_current(|ec| ec.factory().entity())
            .flatten()
            .is_some_and(|facade| facade.sim_active());
        if sim {
            return Err(Error::new(format!(
                "SFTP to {} denied because sim mode is active",
                self.config.target()
            )));
        }
        if self.config.host.is_empty() || self.config.username.is_empty() {
            return Err(Error::new(format!(
                "Host or username not specified, cannot connect to {}",
                self.config.target()
            )));
        }
        let has_password = self.config.password.as_deref().is_some_and(|text| !text.is_empty());
        if !has_password && self.config.keys.is_empty() {
            return Err(Error::new(format!(
                "No password or KeyProvider specified, cannot connect to {}",
                self.config.target()
            )));
        }
        if !has_password {
            self.config.password = None;
        }
        if let Some(mut old) = self.session.take() {
            old.close();
        }
        self.session = Some(connector()?.connect(&self.config)?);
        Ok(self)
    }

    /// True between `connect` and `close` while the connection is up.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.session.as_ref().is_some_and(|session| session.is_connected())
    }

    fn session(&mut self) -> Result<&mut Box<dyn SftpSession>, Error> {
        match self.session.as_mut() {
            Some(session) if session.is_connected() => Ok(session),
            _ => Err(Error::new("SFTP Client not connected")),
        }
    }

    /// Names of the regular files in a directory.
    ///
    /// # Errors
    ///
    /// Not connected, or the server's error.
    pub fn ls_files(&mut self, path: &str) -> Result<Vec<String>, Error> {
        Ok(self
            .session()?
            .read_dir(path)?
            .into_iter()
            .filter(|entry| entry.attributes.kind == SftpKind::File)
            .map(|entry| entry.name)
            .collect())
    }

    /// Files and directories in a directory, with each one's `path` (`dir/name`, as SSHJ gives it).
    ///
    /// # Errors
    ///
    /// Not connected, or the server's error.
    pub fn ls(&mut self, path: &str) -> Result<Vec<(String, SftpEntry)>, Error> {
        Ok(self
            .session()?
            .read_dir(path)?
            .into_iter()
            .filter(|entry| entry.attributes.kind != SftpKind::Other)
            .map(|entry| (child_path(path, &entry.name), entry))
            .collect())
    }

    /// # Errors
    ///
    /// Not connected, or the server's error.
    pub fn get_binary(&mut self, path: &str) -> Result<Vec<u8>, Error> {
        self.session()?.read(path)
    }

    /// The file decoded in `charset`, UTF-8 when empty.
    ///
    /// # Errors
    ///
    /// Not connected, an unknown charset, or the server's error.
    pub fn get_text(&mut self, path: &str, charset: &str) -> Result<String, Error> {
        let encoding = encoding_for(charset)?;
        let bytes = self.session()?.read(path)?;
        Ok(decode(encoding, &bytes))
    }

    /// Put text encoded in `charset`. `path` is a directory (the file is `path/filename`) or the file's own path.
    /// `create_dir` first makes the parent of `path`, as Java does.
    ///
    /// # Errors
    ///
    /// Not connected, an unknown charset, or the server's error.
    pub fn put_text(
        &mut self,
        path: &str,
        filename: &str,
        text: &str,
        charset: &str,
        create_dir: bool,
    ) -> Result<String, Error> {
        let bytes = encode(encoding_for(charset)?, text);
        if create_dir {
            let parent = parent_of(path);
            if !parent.is_empty() {
                self.mkdirs(&parent)?;
            }
        }
        self.put_bytes(path, filename, &bytes)
    }

    /// Put bytes, to `path/filename` when `path` is a directory, else to `path`. Returns the file written.
    ///
    /// # Errors
    ///
    /// Not connected, or the server's error.
    pub fn put_bytes(&mut self, path: &str, filename: &str, bytes: &[u8]) -> Result<String, Error> {
        let preserve = self.preserve_attributes;
        let session = self.session()?;
        let target = match session.stat(path)? {
            Some(found) if found.kind == SftpKind::Directory => child_path(path, filename),
            _ => path.to_string(),
        };
        session.write(&target, bytes)?;
        if preserve {
            session.set_permissions(&target, 0o644)?;
        }
        Ok(target)
    }

    /// Create a directory and its missing parents.
    ///
    /// # Errors
    ///
    /// Not connected, a file in the path, or the server's error.
    pub fn mkdirs(&mut self, path: &str) -> Result<(), Error> {
        let session = self.session()?;
        let absolute = path.starts_with('/');
        let mut current = String::new();
        for part in path.split('/').filter(|part| !part.is_empty() && *part != ".") {
            if current.is_empty() {
                if absolute {
                    current.push('/');
                }
            } else {
                current.push('/');
            }
            current.push_str(part);
            match session.stat(&current)? {
                Some(found) if found.kind == SftpKind::Directory => {}
                Some(_) => return Err(Error::new(format!("{current} exists and is not a directory"))),
                None => session.make_dir(&current)?,
            }
        }
        Ok(())
    }

    /// Move a file into another directory, creating the directory when `create_dir`. Returns the new path.
    ///
    /// # Errors
    ///
    /// Not connected, or the server's error.
    pub fn move_file(&mut self, file_path: &str, dest_dir: &str, create_dir: bool) -> Result<String, Error> {
        if create_dir {
            self.mkdirs(dest_dir)?;
        }
        let name = file_path.rsplit_once('/').map_or(file_path, |(_, name)| name);
        let mut new_path = dest_dir.to_string();
        if !new_path.ends_with('/') {
            new_path.push('/');
        }
        new_path.push_str(name);
        self.session()?.rename(file_path, &new_path)?;
        Ok(new_path)
    }

    /// Rename a file in its own directory. Returns the new path.
    ///
    /// # Errors
    ///
    /// Not connected, or the server's error.
    pub fn rename_file(&mut self, file_path: &str, new_name: &str) -> Result<String, Error> {
        let new_path = match file_path.rsplit_once('/') {
            Some((dir, _)) if !dir.is_empty() => format!("{dir}/{new_name}"),
            _ => new_name.to_string(),
        };
        self.session()?.rename(file_path, &new_path)?;
        Ok(new_path)
    }

    /// # Errors
    ///
    /// Not connected, or the server's error.
    pub fn rename(&mut self, from: &str, to: &str) -> Result<String, Error> {
        self.session()?.rename(from, to)?;
        Ok(to.to_string())
    }

    /// # Errors
    ///
    /// Not connected, or the server's error.
    pub fn rm(&mut self, path: &str) -> Result<(), Error> {
        self.session()?.remove_file(path)
    }

    /// `stat` of one path. `None` when nothing is there.
    ///
    /// # Errors
    ///
    /// Not connected, or the server's error.
    pub fn stat(&mut self, path: &str) -> Result<Option<SftpAttributes>, Error> {
        self.session()?.stat(path)
    }

    /// Stream a local file to `remote_path` and, when attributes are preserved, set mode `0600`.
    ///
    /// # Errors
    ///
    /// Not connected, the local file cannot be read, or the server's error.
    pub fn put_file(&mut self, remote_path: &str, local: &std::path::Path) -> Result<(), Error> {
        let mut file = std::fs::File::open(local)
            .map_err(|err| Error::new(format!("read {}", local.display())).with_source(err))?;
        let handle = self.put_stream(remote_path)?;
        let mut buf = [0u8; 32 * 1024];
        loop {
            let read = std::io::Read::read(&mut file, &mut buf)
                .map_err(|err| Error::new(format!("read {}", local.display())).with_source(err));
            let n = match read {
                Ok(0) => break,
                Ok(n) => n,
                Err(err) => {
                    let _ = self.close_handle(handle);
                    return Err(err);
                }
            };
            if let Err(err) = self.write_chunk(handle, &buf[..n]) {
                let _ = self.close_handle(handle);
                return Err(err);
            }
        }
        self.close_handle(handle)?;
        if self.preserve_attributes {
            self.session()?.set_permissions(remote_path, 0o600)?;
        }
        Ok(())
    }

    /// Upload a directory tree. Directories are mode `0700` and files mode `0600` when attributes are preserved.
    ///
    /// # Errors
    ///
    /// Not connected, a local path cannot be read, or the server's error.
    pub fn put_tree(&mut self, remote_dir: &str, local: &std::path::Path) -> Result<(), Error> {
        self.mkdirs(remote_dir)?;
        if self.preserve_attributes {
            self.session()?.set_permissions(remote_dir, 0o700)?;
        }
        let entries =
            std::fs::read_dir(local).map_err(|err| Error::new(format!("read {}", local.display())).with_source(err))?;
        for entry in entries {
            let entry = entry.map_err(|err| Error::new(format!("read {}", local.display())).with_source(err))?;
            let path = entry.path();
            let remote = child_path(remote_dir, &entry.file_name().to_string_lossy());
            if path.is_dir() {
                self.put_tree(&remote, &path)?;
            } else {
                self.put_file(&remote, &path)?;
            }
        }
        Ok(())
    }

    /// Remove a file, or a directory and everything under it.
    ///
    /// # Errors
    ///
    /// Not connected, or the server's error.
    pub fn rm_tree(&mut self, path: &str) -> Result<(), Error> {
        match self.stat(path)? {
            Some(found) if found.kind == SftpKind::Directory => {
                let children = self.ls(path)?;
                for (child, _) in children {
                    self.rm_tree(&child)?;
                }
                self.session()?.remove_dir(path)
            }
            Some(_) => self.rm(path),
            None => Ok(()),
        }
    }

    /// Open a file to read in chunks. See [`SftpSession::open`].
    ///
    /// # Errors
    ///
    /// Not connected, or the server's error.
    pub fn open_stream(&mut self, path: &str) -> Result<u64, Error> {
        self.session()?.open(path, false)
    }

    /// Open a file to write in chunks, created or truncated.
    ///
    /// # Errors
    ///
    /// Not connected, or the server's error.
    pub fn put_stream(&mut self, path: &str) -> Result<u64, Error> {
        self.session()?.open(path, true)
    }

    /// # Errors
    ///
    /// Not connected, or the server's error.
    pub fn read_chunk(&mut self, handle: u64, max: usize) -> Result<Vec<u8>, Error> {
        self.session()?.read_chunk(handle, max)
    }

    /// # Errors
    ///
    /// Not connected, or the server's error.
    pub fn write_chunk(&mut self, handle: u64, bytes: &[u8]) -> Result<(), Error> {
        self.session()?.write_chunk(handle, bytes)
    }

    /// # Errors
    ///
    /// Not connected, or the server's error.
    pub fn close_handle(&mut self, handle: u64) -> Result<(), Error> {
        self.session()?.close_handle(handle)
    }

    /// Close the connection. Calling it again does nothing.
    pub fn close(&mut self) {
        if let Some(mut session) = self.session.take() {
            session.close();
        }
    }
}

impl Drop for SftpClient {
    /// Java's `finalize` closes a client the caller forgot, with an error in the log. The close is network I/O with no
    /// time limit, and a script's client is dropped by the QuickJS garbage collector at any allocation, so it is done on
    /// a thread of its own.
    fn drop(&mut self) {
        let Some(mut session) = self.session.take() else {
            return;
        };
        let target = self.config.target();
        if session.is_connected() {
            tracing::error!("SftpClient not closed for host {target}, closing it");
        }
        // Even a session that reads as disconnected may still close its channel when dropped.
        let closer = std::thread::Builder::new()
            .name("sftp-close".to_string())
            .spawn(move || session.close());
        if let Err(err) = closer {
            tracing::error!("could not start a thread to close the SFTP session to {target}: {err}");
        }
    }
}

fn child_path(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// `new File(path).parent`.
fn parent_of(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some(("", _)) => "/".to_string(),
        Some((parent, _)) => parent.to_string(),
        None => String::new(),
    }
}

/// The `encoding_rs` encoding for a Java charset name; UTF-8 when empty.
///
/// # Errors
///
/// Returns an error for a name the WHATWG Encoding Standard does not know.
pub fn encoding_for(charset: &str) -> Result<&'static encoding_rs::Encoding, Error> {
    let charset = charset.trim();
    if charset.is_empty() {
        return Ok(encoding_rs::UTF_8);
    }
    encoding_rs::Encoding::for_label(charset.as_bytes())
        .ok_or_else(|| Error::new(format!("Unsupported charset {charset}")))
}

/// Text to bytes. UTF-16 is written as UTF-16 with no byte order mark, which `encoding_rs` does not encode.
#[must_use]
pub fn encode(encoding: &'static encoding_rs::Encoding, text: &str) -> Vec<u8> {
    if encoding == encoding_rs::UTF_16BE {
        return text.encode_utf16().flat_map(u16::to_be_bytes).collect();
    }
    if encoding == encoding_rs::UTF_16LE {
        return text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    }
    encoding.encode(text).0.into_owned()
}

/// Bytes to text; malformed sequences become U+FFFD, as Java's decoder replaces them.
#[must_use]
pub fn decode(encoding: &'static encoding_rs::Encoding, bytes: &[u8]) -> String {
    encoding.decode_without_bom_handling(bytes).0.into_owned()
}

#[cfg(test)]
#[path = "sftp_tests.rs"]
pub(crate) mod tests;
