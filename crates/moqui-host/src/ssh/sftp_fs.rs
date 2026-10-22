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

//! The SFTP subsystem of the built-in SSH server: for a user with `SSH_SFTP`, a view of one resource location
//! (`ssh-server` `sftp-root`, a `runtime://`, `file:`, `component://` or `dbresource://` directory), never of the host
//! disk outside it; for any other user (`dav-server` `sftp-drives`), the drives that user may open, one folder each,
//! through the file services as that user (`moqui_core::dav::files::tree`: a write is an upload and a new version, a
//! removal goes to the trash). A client path is normalized against `/`; a `..` that would leave the root is refused.
//! Each call runs on a request worker: on the root with authorization off, on the drives logged in as the user.
//!
//! A file opened for reading is read whole; one opened for writing is kept in memory and written on close, because a
//! `dbresource` file is a row. `ssh-server` `sftp-max-file` bytes (default 256 MiB) bound either, and also the bytes all the
//! open files of one channel hold together. A channel has at most [`MAX_HANDLES`] handles open.

use std::collections::HashMap;
use std::sync::Weak;

use moqui_core::context::{Error as CoreError, ExecutionContextFactory, ResourceStat};
use moqui_core::dav::files::tree;
use russh_sftp::protocol::{Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version};
use russh_sftp::server::StatusReply;

use super::{on_pool, on_pool_without_authz};

/// Open file and directory handles on one channel.
const MAX_HANDLES: usize = 64;

/// One open handle.
enum Open {
    Read { bytes: Vec<u8> },
    Write { location: String, bytes: Vec<u8> },
    Dir { entries: Option<Vec<File>> },
}

/// What one channel serves.
#[derive(Clone, Debug)]
pub(super) enum Backend {
    /// A resource location, for a user with `SSH_SFTP`.
    Root(String),
    /// The drives of this user.
    Drives { username: String },
}

/// The storage calls of a channel, on a request worker.
struct Store<'a> {
    factory: &'a ExecutionContextFactory,
    drives: bool,
}

impl Store<'_> {
    fn stat(&self, target: &str) -> Result<Option<ResourceStat>, CoreError> {
        if self.drives {
            tree::stat(target)
        } else {
            self.factory.resource().stat(target)
        }
    }

    fn list(&self, target: &str) -> Result<Vec<(String, ResourceStat)>, CoreError> {
        if self.drives {
            return tree::list(target);
        }
        let resources = self.factory.resource();
        let mut files = Vec::new();
        for entry in resources.list(target)? {
            let stat = resources.stat(&entry.location)?.unwrap_or(ResourceStat {
                is_file: entry.is_file,
                size: 0,
                last_modified: None,
            });
            files.push((entry.name, stat));
        }
        Ok(files)
    }

    fn read(&self, target: &str) -> Result<Vec<u8>, CoreError> {
        if self.drives {
            tree::read(target)
        } else {
            self.factory.resource().get(target)?.read_bytes()
        }
    }

    fn write(&self, target: &str, bytes: Vec<u8>) -> Result<(), CoreError> {
        if self.drives {
            tree::write(target, bytes)
        } else {
            self.factory.resource().write_bytes(target, &bytes)
        }
    }

    /// Remove what is at `target` when it is a file (`want_file`) or a folder; false when nothing is there.
    fn remove(&self, target: &str, want_file: bool) -> Result<bool, CoreError> {
        match self.stat(target)? {
            Some(stat) if stat.is_file == want_file => {
                if self.drives {
                    tree::remove(target)
                } else {
                    self.factory.resource().delete(target)
                }
            }
            Some(_) if want_file => Err(CoreError::new(format!("is a directory: {target}"))),
            Some(_) => Err(CoreError::new(format!("not a directory: {target}"))),
            None => Ok(false),
        }
    }

    fn make_directory(&self, target: &str) -> Result<(), CoreError> {
        if self.stat(target)?.is_some() {
            return Err(CoreError::new(format!("exists: {target}")));
        }
        if self.drives {
            tree::make_directory(target)
        } else {
            self.factory.resource().make_directory(target)
        }
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), CoreError> {
        if self.stat(to)?.is_some() {
            return Err(CoreError::new(format!("exists: {to}")));
        }
        if self.drives {
            tree::rename(from, to)
        } else {
            self.factory.resource().rename(from, to)
        }
    }
}

/// The handler for one `sftp` channel.
pub(super) struct SftpFs {
    factory: Weak<ExecutionContextFactory>,
    backend: Backend,
    max_file: usize,
    handles: HashMap<String, Open>,
    /// The file bytes the open handles hold, at most `max_file`.
    held: usize,
    next: u64,
}

impl SftpFs {
    pub(super) fn new(factory: Weak<ExecutionContextFactory>, backend: Backend, max_file: usize) -> Self {
        let backend = match backend {
            Backend::Root(root) => Backend::Root(root.trim_end_matches('/').to_string()),
            drives => drives,
        };
        Self {
            factory,
            backend,
            max_file,
            handles: HashMap::new(),
            held: 0,
            next: 0,
        }
    }

    /// Refuse one more handle past [`MAX_HANDLES`].
    fn handle_room(&self) -> Result<(), StatusReply> {
        if self.handles.len() >= MAX_HANDLES {
            return Err(StatusCode::Failure.with_message(format!(
                "{MAX_HANDLES} handles are open on this channel; close one first"
            )));
        }
        Ok(())
    }

    /// The bytes open files may still take before this channel holds `ssh-server` `sftp-max-file`.
    fn room(&self) -> usize {
        self.max_file.saturating_sub(self.held)
    }

    fn too_much_held() -> StatusReply {
        StatusCode::Failure
            .with_message("the open files on this channel would hold more than ssh-server sftp-max-file bytes")
    }

    /// The storage target for a client path (a location under the root, or the path in the drives' tree) and the path
    /// as shown, or `PermissionDenied` when it climbs above the root.
    fn location(&self, path: &str) -> Result<(String, String), StatusReply> {
        let shown =
            normalize(path).ok_or_else(|| StatusCode::PermissionDenied.with_message("outside the SFTP root"))?;
        let location = match &self.backend {
            Backend::Root(root) if shown == "/" => root.clone(),
            Backend::Root(root) => format!("{root}{shown}"),
            Backend::Drives { .. } => shown.clone(),
        };
        Ok((location, shown))
    }

    /// Run `job` on a request worker: on the root with authorization off, on the drives as the channel's user.
    async fn run<T, F>(&self, job: F) -> Result<T, CoreError>
    where
        T: Send + 'static,
        F: FnOnce(&Store<'_>) -> Result<T, CoreError> + Send + 'static,
    {
        match self.backend.clone() {
            Backend::Root(_) => {
                on_pool_without_authz(&self.factory, move |factory| job(&Store { factory, drives: false })).await
            }
            Backend::Drives { username } => {
                on_pool(&self.factory, move |factory| {
                    moqui_core::user::internal_login_user(&username)?;
                    let result = job(&Store { factory, drives: true });
                    let _ = moqui_core::user::logout();
                    result
                })
                .await
            }
        }
    }

    fn handle_id(&mut self) -> String {
        self.next += 1;
        format!("h{}", self.next)
    }

    async fn stat_of(&self, location: String) -> Result<Option<ResourceStat>, StatusReply> {
        self.run(move |store| store.stat(&location)).await.map_err(failure)
    }
}

/// `/a/./b/../c` to `/a/c`; `None` when `..` passes the root.
pub(super) fn normalize(path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(format!("/{}", parts.join("/")))
}

fn failure(err: moqui_core::context::Error) -> StatusReply {
    let message = err.message().to_string();
    let lower = message.to_ascii_lowercase();
    let code = if lower.contains("not found") || lower.contains("no such file") || lower.contains("not a directory") {
        StatusCode::NoSuchFile
    } else if lower.contains("not authorized") || lower.contains("permission") {
        StatusCode::PermissionDenied
    } else {
        StatusCode::Failure
    };
    code.with_message(message)
}

fn ok(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Ok".to_string(),
        language_tag: "en-US".to_string(),
    }
}

fn attributes(stat: &ResourceStat) -> FileAttributes {
    let mut attrs = FileAttributes::empty();
    attrs.size = Some(stat.size);
    attrs.permissions = Some(if stat.is_file { 0o100_644 } else { 0o040_755 });
    let seconds = stat.last_modified.and_then(|millis| u32::try_from(millis / 1000).ok());
    attrs.mtime = seconds;
    attrs.atime = seconds;
    attrs
}

impl russh_sftp::server::Handler for SftpFs {
    type Error = StatusReply;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported.into()
    }

    async fn init(&mut self, _version: u32, _extensions: HashMap<String, String>) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let (_, shown) = self.location(&path)?;
        Ok(Name {
            id,
            files: vec![File::dummy(shown)],
        })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let (location, _) = self.location(&path)?;
        match self.stat_of(location).await? {
            Some(stat) => Ok(Attrs {
                id,
                attrs: attributes(&stat),
            }),
            None => Err(StatusCode::NoSuchFile.into()),
        }
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.stat(id, path).await
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let mut attrs = FileAttributes::empty();
        match self.handles.get(&handle) {
            Some(Open::Read { bytes } | Open::Write { bytes, .. }) => {
                attrs.size = Some(bytes.len() as u64);
                attrs.permissions = Some(0o100_644);
            }
            Some(Open::Dir { .. }) => attrs.permissions = Some(0o040_755),
            None => return Err(StatusCode::Failure.with_message("unknown handle")),
        }
        Ok(Attrs { id, attrs })
    }

    /// Attributes are not kept: a set succeeds, so a client that sets times or modes after an upload works.
    async fn setstat(&mut self, id: u32, path: String, _attrs: FileAttributes) -> Result<Status, Self::Error> {
        let (location, _) = self.location(&path)?;
        match self.stat_of(location).await? {
            Some(_) => Ok(ok(id)),
            None => Err(StatusCode::NoSuchFile.into()),
        }
    }

    async fn fsetstat(&mut self, id: u32, _handle: String, _attrs: FileAttributes) -> Result<Status, Self::Error> {
        Ok(ok(id))
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        self.handle_room()?;
        let (location, _) = self.location(&path)?;
        let listed = self.run(move |store| store.list(&location)).await.map_err(failure)?;
        let entries = listed
            .into_iter()
            .map(|(name, stat)| File::new(name, attributes(&stat)))
            .collect();
        let handle = self.handle_id();
        self.handles
            .insert(handle.clone(), Open::Dir { entries: Some(entries) });
        Ok(Handle { id, handle })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        match self.handles.get_mut(&handle) {
            Some(Open::Dir { entries }) => match entries.take() {
                Some(files) => Ok(Name { id, files }),
                None => Err(StatusCode::Eof.into()),
            },
            _ => Err(StatusCode::Failure.with_message("not a directory handle")),
        }
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        self.handle_room()?;
        let (location, _) = self.location(&filename)?;
        let writing = pflags.intersects(OpenFlags::WRITE | OpenFlags::APPEND | OpenFlags::CREATE);
        let truncate = pflags.contains(OpenFlags::TRUNCATE);
        let max = self.max_file;
        // A truncated file is not read, so it takes no room.
        let room = if writing && truncate { usize::MAX } else { self.room() };
        let read_location = location.clone();
        let existing = self
            .run(move |store| match store.stat(&read_location)? {
                Some(stat) if !stat.is_file => {
                    Err(moqui_core::context::Error::new(format!("not a file: {read_location}")))
                }
                Some(stat) if usize::try_from(stat.size).unwrap_or(usize::MAX) > max => Err(
                    moqui_core::context::Error::new(format!("{read_location} is larger than ssh-server sftp-max-file")),
                ),
                Some(stat) if usize::try_from(stat.size).unwrap_or(usize::MAX) > room => Ok(Err(())),
                Some(_) if writing && truncate => Ok(Ok(Some(Vec::new()))),
                Some(_) => store.read(&read_location).map(|bytes| Ok(Some(bytes))),
                None => Ok(Ok(None)),
            })
            .await
            .map_err(failure)?
            .map_err(|()| Self::too_much_held())?;
        // The file may have grown between the stat and the read.
        let size = existing.as_ref().map_or(0, Vec::len);
        if size > self.room() {
            return Err(Self::too_much_held());
        }
        let open = match (writing, existing) {
            (false, Some(bytes)) => Open::Read { bytes },
            (false, None) => return Err(StatusCode::NoSuchFile.into()),
            (true, existing) => {
                if existing.is_none() && !pflags.contains(OpenFlags::CREATE) {
                    return Err(StatusCode::NoSuchFile.into());
                }
                if existing.is_some() && pflags.contains(OpenFlags::EXCLUDE) {
                    return Err(StatusCode::Failure.with_message("file exists"));
                }
                let bytes = if truncate {
                    Vec::new()
                } else {
                    existing.unwrap_or_default()
                };
                Open::Write { location, bytes }
            }
        };
        if let Open::Read { bytes } | Open::Write { bytes, .. } = &open {
            self.held += bytes.len();
        }
        let handle = self.handle_id();
        self.handles.insert(handle.clone(), open);
        Ok(Handle { id, handle })
    }

    async fn read(&mut self, id: u32, handle: String, offset: u64, len: u32) -> Result<Data, Self::Error> {
        let bytes = match self.handles.get(&handle) {
            Some(Open::Read { bytes } | Open::Write { bytes, .. }) => bytes,
            _ => return Err(StatusCode::Failure.with_message("not a file handle")),
        };
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        if start >= bytes.len() {
            return Err(StatusCode::Eof.into());
        }
        let end = start.saturating_add(len as usize).min(bytes.len());
        Ok(Data {
            id,
            data: bytes[start..end].to_vec(),
        })
    }

    async fn write(&mut self, id: u32, handle: String, offset: u64, data: Vec<u8>) -> Result<Status, Self::Error> {
        let max = self.max_file;
        let room = self.room();
        let Some(Open::Write { bytes, .. }) = self.handles.get_mut(&handle) else {
            return Err(StatusCode::PermissionDenied.with_message("not opened for writing"));
        };
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        let end = start.saturating_add(data.len());
        if end > max {
            return Err(StatusCode::Failure.with_message("the file is larger than ssh-server sftp-max-file"));
        }
        let grown = end.saturating_sub(bytes.len());
        if grown > room {
            return Err(Self::too_much_held());
        }
        if grown > 0 {
            bytes.resize(end, 0);
            self.held += grown;
        }
        bytes[start..end].copy_from_slice(&data);
        Ok(ok(id))
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        let open = self.handles.remove(&handle);
        if let Some(Open::Read { bytes } | Open::Write { bytes, .. }) = &open {
            self.held = self.held.saturating_sub(bytes.len());
        }
        match open {
            Some(Open::Write { location, bytes }) => {
                self.run(move |store| store.write(&location, bytes))
                    .await
                    .map_err(failure)?;
                Ok(ok(id))
            }
            Some(_) => Ok(ok(id)),
            None => Err(StatusCode::Failure.with_message("unknown handle")),
        }
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        let (location, _) = self.location(&filename)?;
        let removed = self
            .run(move |store| store.remove(&location, true))
            .await
            .map_err(failure)?;
        if removed {
            Ok(ok(id))
        } else {
            Err(StatusCode::NoSuchFile.into())
        }
    }

    async fn mkdir(&mut self, id: u32, path: String, _attrs: FileAttributes) -> Result<Status, Self::Error> {
        let (location, _) = self.location(&path)?;
        self.run(move |store| store.make_directory(&location))
            .await
            .map_err(failure)?;
        Ok(ok(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        let (location, shown) = self.location(&path)?;
        if shown == "/" {
            return Err(StatusCode::PermissionDenied.with_message("the SFTP root stays"));
        }
        let removed = self
            .run(move |store| store.remove(&location, false))
            .await
            .map_err(failure)?;
        if removed {
            Ok(ok(id))
        } else {
            Err(StatusCode::NoSuchFile.into())
        }
    }

    async fn rename(&mut self, id: u32, oldpath: String, newpath: String) -> Result<Status, Self::Error> {
        let (from, shown) = self.location(&oldpath)?;
        let (to, _) = self.location(&newpath)?;
        if shown == "/" {
            return Err(StatusCode::PermissionDenied.with_message("the SFTP root stays"));
        }
        self.run(move |store| store.rename(&from, &to)).await.map_err(failure)?;
        Ok(ok(id))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use russh_sftp::protocol::{FileAttributes, OpenFlags};
    use russh_sftp::server::Handler as _;

    use super::{normalize, Backend, Open, SftpFs, MAX_HANDLES};

    fn message(reply: &russh_sftp::server::StatusReply) -> &str {
        reply.error_message.as_deref().unwrap_or("")
    }

    #[tokio::test]
    async fn open_files_and_handles_are_capped_per_channel() {
        let mut fs = SftpFs::new(Weak::new(), Backend::Root("runtime://sftp".to_string()), 100);
        let (location, bytes) = ("runtime://sftp/a".to_string(), vec![0; 60]);
        fs.handles.insert("h1".to_string(), Open::Write { location, bytes });
        let (location, bytes) = ("runtime://sftp/b".to_string(), Vec::new());
        fs.handles.insert("h2".to_string(), Open::Write { location, bytes });
        fs.held = 60;
        fs.write(1, "h2".to_string(), 0, vec![1; 40]).await.unwrap();
        assert_eq!(fs.held, 100);
        let refused = fs.write(2, "h2".to_string(), 40, vec![1]).await.unwrap_err();
        assert!(message(&refused).contains("would hold more than"), "{refused:?}");
        // Writing over bytes already held needs no room.
        fs.write(3, "h1".to_string(), 0, vec![2; 60]).await.unwrap();
        fs.close(4, "h1".to_string()).await.unwrap_err();
        assert_eq!(fs.held, 40);

        for index in fs.handles.len()..MAX_HANDLES {
            fs.handles.insert(format!("d{index}"), Open::Dir { entries: None });
        }
        let refused = fs
            .open(5, "/c".to_string(), OpenFlags::READ, FileAttributes::empty())
            .await
            .unwrap_err();
        assert!(message(&refused).contains("handles are open"), "{refused:?}");
        let refused = fs.opendir(6, "/".to_string()).await.unwrap_err();
        assert!(message(&refused).contains("handles are open"), "{refused:?}");
    }

    #[test]
    fn paths_stay_under_the_root() {
        assert_eq!(normalize("/").as_deref(), Some("/"));
        assert_eq!(normalize(".").as_deref(), Some("/"));
        assert_eq!(normalize("a/./b//c/").as_deref(), Some("/a/b/c"));
        assert_eq!(normalize("/a/b/../c").as_deref(), Some("/a/c"));
        assert_eq!(normalize("/a/../.."), None);
        assert_eq!(normalize("../etc/passwd"), None);
    }
}
