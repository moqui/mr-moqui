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
//! through the file services as that user (`moqui_dav::dav::files::tree`: a write is an upload and a new version, a
//! removal goes to the trash). A client path is normalized against `/`; a `..` that would leave the root is refused.
//! Each call runs on a request worker: on the root with authorization off, on the drives logged in as the user.
//!
//! A file opened for reading is opened as a windowed source (`ResourceFacade::open`, through the drive adapter for the
//! drives) and read a window at a time as the client asks (plans/archive/High-Level-Review.md HL-010); one opened for writing
//! is kept in memory and written on close, because a `dbresource` file is a row. `ssh-server` `sftp-max-file` bytes
//! (default 256 MiB) bound either file, and the bytes all the open files of one user hold together, over all of that
//! user's channels and connections (a reader holds its window; [`Held`], SEC-FIL-61). A channel has at most
//! [`MAX_HANDLES`] handles open.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};

use moqui_core::context::{Error as CoreError, ExecutionContextFactory, Opened, ResourceStat};
use moqui_core::resource::source::{ByteSource, READ_WINDOW};
use moqui_dav::dav::files::tree;
use russh_sftp::protocol::{Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version};
use russh_sftp::server::StatusReply;

use crate::worker::{on_pool, on_pool_without_authz};

/// Open file and directory handles on one channel.
const MAX_HANDLES: usize = 64;

/// The file bytes each user's open SFTP handles hold, by username, over every channel and connection of that user.
static HELD: LazyLock<Mutex<HashMap<String, usize>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// One channel's share of the bytes its user's open files hold ([`HELD`]): a file opened for writing is in memory
/// until it is closed, so the bound is per user, not per channel, or a client could hold `sftp-max-file` on each of
/// many channels (SEC-FIL-61). The share is given back when the channel ends.
pub(super) struct Held {
    user: String,
    mine: usize,
}

impl Held {
    pub(super) fn new(user: &str) -> Self {
        Self {
            // a username logs in in any case
            user: user.to_ascii_lowercase(),
            mine: 0,
        }
    }

    /// The bytes the user's open files hold on all of their channels.
    fn total(&self) -> usize {
        moqui_lib::sync::lock(&HELD).get(&self.user).copied().unwrap_or(0)
    }

    fn add(&mut self, bytes: usize) {
        self.set(self.mine.saturating_add(bytes));
    }

    fn sub(&mut self, bytes: usize) {
        self.set(self.mine.saturating_sub(bytes));
    }

    fn set(&mut self, mine: usize) {
        let mut held = moqui_lib::sync::lock(&HELD);
        let total = held.entry(self.user.clone()).or_insert(0);
        *total = total.saturating_sub(self.mine).saturating_add(mine);
        if *total == 0 {
            held.remove(&self.user);
        }
        self.mine = mine;
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.set(0);
    }
}

/// An `sftp` subsystem counted in its connection's session channels while it runs (`ssh/server.rs`).
pub(super) struct OpenChannel(Arc<AtomicUsize>);

impl OpenChannel {
    pub(super) fn new(count: &Arc<AtomicUsize>) -> Self {
        count.fetch_add(1, Ordering::AcqRel);
        Self(Arc::clone(count))
    }
}

impl Drop for OpenChannel {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// One open handle.
enum Open {
    /// A file read from its source: `window` is its bytes from `window_at`.
    Read {
        source: Arc<dyn ByteSource>,
        size: u64,
        window_at: u64,
        window: Vec<u8>,
    },
    Write {
        location: String,
        bytes: Vec<u8>,
    },
    Dir {
        entries: Option<Vec<File>>,
    },
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

    /// A file opened to read a window at a time; `None` when nothing is there.
    fn open(&self, target: &str) -> Result<Option<Opened>, CoreError> {
        if self.drives {
            tree::open(target).map(Some)
        } else {
            self.factory.resource().open(target)
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

/// What [`SftpFs::open`] found at a path.
enum Found {
    Nothing,
    /// Writing with truncate: nothing is read.
    Truncated,
    /// Writing: the bytes there, to change in memory.
    Bytes(Vec<u8>),
    /// Writing, but the bytes there would pass the channel's room.
    NoRoom,
    /// Reading: the source, read as the client asks.
    Source(Opened),
}

/// The handler for one `sftp` channel.
pub(super) struct SftpFs {
    factory: Weak<ExecutionContextFactory>,
    backend: Backend,
    max_file: usize,
    handles: HashMap<String, Open>,
    /// The file bytes the open handles hold; with the user's other channels, at most `max_file`.
    held: Held,
    next: u64,
    /// The channel's place in its connection's count, given back when the subsystem ends.
    _channel: Option<OpenChannel>,
}

impl SftpFs {
    /// A channel of `held`'s user.
    pub(super) fn new(factory: Weak<ExecutionContextFactory>, backend: Backend, max_file: usize, held: Held) -> Self {
        let backend = match backend {
            Backend::Root(root) => Backend::Root(root.trim_end_matches('/').to_string()),
            drives => drives,
        };
        Self {
            factory,
            backend,
            max_file,
            handles: HashMap::new(),
            held,
            next: 0,
            _channel: None,
        }
    }

    /// Count the subsystem in its connection's session channels while it runs.
    pub(super) fn counted(mut self, channel: OpenChannel) -> Self {
        self._channel = Some(channel);
        self
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

    /// The bytes open files may still take before the user's channels hold `ssh-server` `sftp-max-file`.
    fn room(&self) -> usize {
        self.max_file.saturating_sub(self.held.total())
    }

    fn too_much_held() -> StatusReply {
        StatusCode::Failure
            .with_message("the open files of this user would hold more than ssh-server sftp-max-file bytes")
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
                    // the channel's login borrowed for one operation: its end writes nothing to the account, where
                    // a logout would sign the person out everywhere (hasLoggedOut)
                    moqui_core::user::internal_login_user(&username)?;
                    let result = job(&Store { factory, drives: true });
                    let _ = moqui_core::user::logout_local();
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

    /// `len` bytes from `offset` of a file open for reading: from its window, or a new window read from its source
    /// (with authorization off: the access was checked when it was opened). A window is at most [`READ_WINDOW`]
    /// whatever `len` the client asks, which answers short (SFTP allows it), and is counted in the user's bytes held
    /// (SEC-FIL-84).
    async fn read_window(&mut self, id: u32, handle: &str, offset: u64, len: u32) -> Result<Data, StatusReply> {
        let (source, size, held_now) = match self.handles.get(handle) {
            Some(Open::Read {
                source,
                size,
                window_at,
                window,
            }) => {
                if offset >= *size {
                    return Err(StatusCode::Eof.into());
                }
                if offset >= *window_at && offset < window_at.saturating_add(window.len() as u64) {
                    let start = usize::try_from(offset - window_at).unwrap_or(usize::MAX);
                    let stop = start.saturating_add(len as usize).min(window.len());
                    return Ok(Data {
                        id,
                        data: window[start..stop].to_vec(),
                    });
                }
                (Arc::clone(source), *size, window.len())
            }
            _ => return Err(StatusCode::Failure.with_message("not a file handle")),
        };
        let want = usize::try_from(size - offset).unwrap_or(usize::MAX).min(READ_WINDOW);
        // the new window takes the old one's place, and what it needs more must fit the user's room
        if want.saturating_sub(held_now) > self.room() {
            return Err(Self::too_much_held());
        }
        let fetched = self
            .run(move |_store| {
                let _authz = moqui_core::context::ExecutionContext::with_current(
                    moqui_core::context::ExecutionContext::disable_authz,
                );
                source
                    .read_at(offset, want)
                    .map_err(|err| CoreError::new(err.message()))
            })
            .await
            .map_err(failure)?;
        if fetched.is_empty() {
            return Err(StatusCode::Eof.into());
        }
        let Some(Open::Read { window_at, window, .. }) = self.handles.get_mut(handle) else {
            return Err(StatusCode::Failure.with_message("not a file handle"));
        };
        let mine = self
            .held
            .mine
            .saturating_sub(window.len())
            .saturating_add(fetched.len());
        self.held.set(mine);
        *window_at = offset;
        *window = fetched;
        let stop = (len as usize).min(window.len());
        Ok(Data {
            id,
            data: window[..stop].to_vec(),
        })
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
            Some(Open::Read { size, .. }) => {
                attrs.size = Some(*size);
                attrs.permissions = Some(0o100_644);
            }
            Some(Open::Write { bytes, .. }) => {
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
        let found = self
            .run(move |store| match store.stat(&read_location)? {
                Some(stat) if !stat.is_file => {
                    Err(moqui_core::context::Error::new(format!("not a file: {read_location}")))
                }
                Some(stat) if usize::try_from(stat.size).unwrap_or(usize::MAX) > max => Err(
                    moqui_core::context::Error::new(format!("{read_location} is larger than ssh-server sftp-max-file")),
                ),
                Some(_) if writing && truncate => Ok(Found::Truncated),
                Some(stat) if writing && usize::try_from(stat.size).unwrap_or(usize::MAX) > room => Ok(Found::NoRoom),
                Some(_) if writing => store.read(&read_location).map(Found::Bytes),
                Some(_) => Ok(store.open(&read_location)?.map_or(Found::Nothing, Found::Source)),
                None => Ok(Found::Nothing),
            })
            .await
            .map_err(failure)?;
        let existing = match found {
            Found::NoRoom => return Err(Self::too_much_held()),
            Found::Source(opened) if !writing => {
                let open = Open::Read {
                    size: opened.stat.size,
                    source: opened.source,
                    window_at: 0,
                    window: Vec::new(),
                };
                let handle = self.handle_id();
                self.handles.insert(handle.clone(), open);
                return Ok(Handle { id, handle });
            }
            Found::Source(_) | Found::Nothing => None,
            Found::Truncated => Some(Vec::new()),
            Found::Bytes(bytes) => Some(bytes),
        };
        // The file may have grown between the stat and the read.
        let size = existing.as_ref().map_or(0, Vec::len);
        if size > self.room() {
            return Err(Self::too_much_held());
        }
        let open = match (writing, existing) {
            (false, _) => return Err(StatusCode::NoSuchFile.into()),
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
        if let Open::Write { bytes, .. } = &open {
            self.held.add(bytes.len());
        }
        let handle = self.handle_id();
        self.handles.insert(handle.clone(), open);
        Ok(Handle { id, handle })
    }

    async fn read(&mut self, id: u32, handle: String, offset: u64, len: u32) -> Result<Data, Self::Error> {
        let bytes = match self.handles.get(&handle) {
            Some(Open::Read { .. }) => return self.read_window(id, &handle, offset, len).await,
            Some(Open::Write { bytes, .. }) => bytes,
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
            self.held.add(grown);
        }
        bytes[start..end].copy_from_slice(&data);
        Ok(ok(id))
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        let open = self.handles.remove(&handle);
        if let Some(Open::Read { window: bytes, .. } | Open::Write { bytes, .. }) = &open {
            self.held.sub(bytes.len());
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
#[path = "sftp_fs_tests.rs"]
mod tests;
