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

//! The operator socket (plans/Sealed-Secrets.md §3.3), opened by the gate before anything else and kept for the life
//! of the process, through every seal and unseal. One JSON request a line, one JSON answer a line; the commands run off
//! the socket's thread, since a passphrase's Argon2id and the `systemd-creds` call take a while.
//!
//! Where the socket is, and which group may use it, come only from root. Under systemd the socket unit
//! (`systemd/moqui-operator.socket`) makes it in a directory of root's and hands it over (`LISTEN_FDS`, named
//! `operator`); in a container the entrypoint, still root, makes it with `moqui operator bind` and passes it on
//! (`MOQUI_OPERATOR_FD`). Only a socket file of root's, in a directory of root's that nobody else can write, grants
//! its group. With neither, the server binds `seal@socket` (default `db/operator.sock` in the runtime) itself, mode
//! 0600: a development setup, and no protection boundary, since the server's own user can replace it.
//!
//! While sealed, SIGINT and SIGTERM give up the wait; unsealed but not yet serving, they end the process.

use std::collections::HashMap;
use std::os::fd::FromRawFd as _;
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use moqui_core::context::Error;
use moqui_core::seal::Keeper;
use moqui_lib::sync::lock;
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _};

use super::peer::Settings;

/// The longest request line: a few shares and a bundle fit.
const MAX_LINE: u64 = 1 << 20;

/// The socket this process serves for each runtime directory, once opened (a process that hosts two runtimes, as a
/// test does, serves one for each; the socket root hands over goes to the first).
static SERVED: LazyLock<Mutex<HashMap<PathBuf, PathBuf>>> = LazyLock::new(Mutex::default);

/// Whether the HTTP listener serves, so a signal before it does ends the process ([`abandon_on_signal`]).
static SERVING: AtomicBool = AtomicBool::new(false);

/// Say whether the HTTP listener serves.
pub(crate) fn set_serving(serving: bool) {
    SERVING.store(serving, Ordering::SeqCst);
}

/// Serve the operator socket for `keeper`, unless this process serves one already (a server that booted again after
/// `seal`): the one root handed over, else one the server binds at `seal@socket`. The path it serves.
///
/// # Errors
///
/// A socket handed over that is not one, another process answering on the path, or a path that is not a socket of
/// this user's.
pub(crate) fn serve(runtime_dir: &Path, conf_socket: &str, keeper: Arc<Keeper>) -> Result<PathBuf, Error> {
    let mut served = lock(&SERVED);
    if let Some(path) = served.get(runtime_dir) {
        return Ok(path.clone());
    }
    let (listener, path, settings) = match inherited()? {
        Some(listener) => {
            let path = listener
                .local_addr()
                .ok()
                .and_then(|address| address.as_pathname().map(Path::to_path_buf))
                .unwrap_or_default();
            let group = root_group(&path);
            tracing::info!(
                "Operator socket {} handed over by root{}",
                path.display(),
                group.map_or_else(String::new, |gid| format!(", group {gid}"))
            );
            (listener, path, Settings { group })
        }
        None => {
            let path = if conf_socket.is_empty() {
                runtime_dir.join("db").join("operator.sock")
            } else {
                runtime_dir.join(conf_socket)
            };
            let listener = bind_own(&path)?;
            tracing::warn!(
                "Operator socket {} bound by the server itself, in a directory its own user can write: it is not a \
                 protection boundary; a production server takes it from systemd/moqui-operator.socket or `moqui \
                 operator bind` (docs/Run-Deploy.md, Sealed start)",
                path.display()
            );
            (listener, path, Settings::default())
        }
    };
    listener
        .set_nonblocking(true)
        .map_err(|err| Error::new("could not set the operator socket non-blocking").with_source(err))?;
    served.insert(runtime_dir.to_path_buf(), path.clone());
    let shown = path.display().to_string();
    std::thread::Builder::new()
        .name("moqui-operator".to_string())
        .spawn(move || run(listener, keeper, settings, &shown))
        .map_err(|err| Error::new("the operator socket's thread did not start").with_source(err))?;
    Ok(path)
}

/// The listening socket root handed to this process: systemd's socket activation (`LISTEN_PID`, `LISTEN_FDS`,
/// `LISTEN_FDNAMES` with `operator`), or `moqui operator bind`'s (`MOQUI_OPERATOR_PID`, `MOQUI_OPERATOR_FD`). Taken
/// once; the variables are removed and the descriptor is closed on exec, so no child gets it.
pub(crate) fn inherited() -> Result<Option<UnixListener>, Error> {
    let own = std::process::id().to_string();
    let var = |name: &str| std::env::var(name).ok().map(|value| value.trim().to_string());
    let fd = if var("LISTEN_PID").as_deref() == Some(own.as_str()) {
        let count: usize = var("LISTEN_FDS").and_then(|count| count.parse().ok()).unwrap_or(0);
        let names = var("LISTEN_FDNAMES").unwrap_or_default();
        let index = names
            .split(':')
            .position(|name| name == "operator")
            .or((count == 1).then_some(0))
            .filter(|index| *index < count);
        for name in ["LISTEN_PID", "LISTEN_FDS", "LISTEN_FDNAMES"] {
            std::env::remove_var(name);
        }
        index.and_then(|index| i32::try_from(index).ok()).map(|index| 3 + index)
    } else if var("MOQUI_OPERATOR_PID").as_deref() == Some(own.as_str()) {
        let fd = var("MOQUI_OPERATOR_FD").and_then(|fd| fd.parse().ok());
        for name in ["MOQUI_OPERATOR_PID", "MOQUI_OPERATOR_FD"] {
            std::env::remove_var(name);
        }
        fd
    } else {
        None
    };
    fd.map(adopt).transpose()
}

/// Whether descriptor `fd` of this process is open on a socket, read from the file system so no descriptor is
/// claimed before it is known to be one.
fn is_open_socket(fd: i32) -> bool {
    if cfg!(target_os = "linux") {
        std::fs::read_link(format!("/proc/self/fd/{fd}"))
            .is_ok_and(|target| target.to_string_lossy().starts_with("socket:"))
    } else {
        std::fs::metadata(format!("/dev/fd/{fd}")).is_ok_and(|meta| meta.file_type().is_socket())
    }
}

/// The listener on inherited descriptor `fd`.
fn adopt(fd: i32) -> Result<UnixListener, Error> {
    if fd < 3 || !is_open_socket(fd) {
        return Err(Error::new(format!(
            "the operator socket handed over (descriptor {fd}) is not an open socket"
        )));
    }
    // SAFETY: `fd` is open on a socket (checked above), it was handed to this process for the operator socket alone
    // (the variables naming it carry this process's id and are removed, so it is adopted once), and nothing else in
    // the process owns it.
    let listener = unsafe { UnixListener::from_raw_fd(fd) };
    listener.local_addr().map_err(|err| {
        Error::new(format!(
            "the operator socket handed over (descriptor {fd}) is not a Unix socket"
        ))
        .with_source(err)
    })?;
    rustix::io::fcntl_setfd(&listener, rustix::io::FdFlags::CLOEXEC)
        .map_err(|err| Error::new("could not close the operator socket on exec").with_source(err))?;
    Ok(listener)
}

/// The group a socket file grants: its group, when root owns it and its directory, and nobody else can write that.
fn root_group(path: &Path) -> Option<u32> {
    let file = std::fs::symlink_metadata(path).ok()?;
    let dir = std::fs::symlink_metadata(path.parent()?).ok()?;
    let granted = granted_group(file.uid(), file.gid(), dir.uid(), dir.mode());
    if granted.is_none() {
        tracing::warn!(
            "The operator socket {} or its directory is not root's alone: only root may run operator commands",
            path.display()
        );
    }
    granted
}

/// The group of a socket file owned by `file_uid` with group `file_gid`, in a directory owned by `dir_uid` with
/// `dir_mode`: granted only when root owns both and the directory is not writable by its group or others.
pub(crate) fn granted_group(file_uid: u32, file_gid: u32, dir_uid: u32, dir_mode: u32) -> Option<u32> {
    (file_uid == 0 && dir_uid == 0 && dir_mode & 0o022 == 0).then_some(file_gid)
}

/// Bind `path` for this server, mode 0600. Something already there is replaced only when it is a socket of this
/// user's that nobody answers on (left by a process that ended).
pub(crate) fn bind_own(path: &Path) -> Result<UnixListener, Error> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|err| Error::new(format!("could not make {}", dir.display())).with_source(err))?;
    }
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        let own = rustix::process::geteuid().as_raw();
        if !meta.file_type().is_socket() || meta.uid() != own {
            return Err(Error::new(format!(
                "{} is there and is not a socket of this user's: the operator socket is not put in its place",
                path.display()
            )));
        }
        if std::os::unix::net::UnixStream::connect(path).is_ok() {
            return Err(Error::new(format!(
                "another server answers on the operator socket {}",
                path.display()
            )));
        }
        std::fs::remove_file(path)
            .map_err(|err| Error::new(format!("could not remove {}", path.display())).with_source(err))?;
    }
    let listener = UnixListener::bind(path)
        .map_err(|err| Error::new(format!("could not open the operator socket {}", path.display())).with_source(err))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|err| Error::new(format!("could not set the mode of {}", path.display())).with_source(err))?;
    Ok(listener)
}

/// The socket's thread: accept, and on SIGINT or SIGTERM give up the wait or end the process.
fn run(listener: UnixListener, keeper: Arc<Keeper>, settings: Settings, shown: &str) {
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(err) => {
            tracing::error!("The operator socket {shown} has no runtime: {err}");
            return;
        }
    };
    runtime.block_on(async move {
        let listener = match tokio::net::UnixListener::from_std(listener) {
            Ok(listener) => listener,
            Err(err) => {
                tracing::error!("The operator socket {shown} could not be served: {err}");
                return;
            }
        };
        tokio::spawn(abandon_on_signal(Arc::clone(&keeper)));
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    tokio::spawn(connection(stream, Arc::clone(&keeper), settings.clone()));
                }
                Err(err) => {
                    tracing::warn!("The operator socket {shown} did not accept: {err}");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
    });
}

/// A signal while sealed ends the wait of the gate, so the process stops instead of ignoring it. Once unsealed and
/// before the HTTP listener serves (opening the database, loading data), nothing else answers a signal, so it ends
/// the process here. While serving, the listener's own handler stops the server.
async fn abandon_on_signal(keeper: Arc<Keeper>) {
    let Ok(mut term) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) else {
        return;
    };
    let Ok(mut interrupt) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()) else {
        return;
    };
    loop {
        let code = tokio::select! {
            _ = term.recv() => 143,
            _ = interrupt.recv() => 130,
        };
        if !keeper.is_unsealed() {
            tracing::warn!("Stopping while sealed");
            keeper.abandon();
        } else if !SERVING.load(Ordering::SeqCst) {
            tracing::warn!("Stopping on a signal before the listener opened");
            std::process::exit(code);
        }
    }
}

/// One connection: its peer's access, then its requests in turn.
async fn connection(stream: tokio::net::UnixStream, keeper: Arc<Keeper>, settings: Settings) {
    let access = match stream.peer_cred() {
        Ok(cred) => super::peer::access(cred.uid(), cred.gid(), cred.pid(), &settings),
        Err(err) => {
            tracing::warn!("The operator socket could not read a peer's credentials: {err}");
            return;
        }
    };
    let (reader, mut writer) = stream.into_split();
    let mut reader = tokio::io::BufReader::new(reader);
    loop {
        let mut line = zeroize::Zeroizing::new(String::new());
        match (&mut reader).take(MAX_LINE).read_line(&mut line).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if !line.ends_with('\n') && line.len() as u64 >= MAX_LINE {
            let _ = writer
                .write_all(b"{\"ok\":false,\"error\":\"request too long\"}\n")
                .await;
            return;
        }
        let keeper = Arc::clone(&keeper);
        let mut answer = tokio::task::spawn_blocking(move || super::commands::run(&keeper, access, &line))
            .await
            .unwrap_or_else(|_| serde_json::json!({"ok": false, "error": "the command failed"}));
        let mut text = zeroize::Zeroizing::new(answer.to_string());
        super::client::scrub(&mut answer);
        text.push('\n');
        if writer.write_all(text.as_bytes()).await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
#[path = "socket_tests.rs"]
mod tests;
