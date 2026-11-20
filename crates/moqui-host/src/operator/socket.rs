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

//! The operator socket (plans/Sealed-Secrets.md §3.3): a Unix socket, mode 0660 with the operator group, opened by the
//! gate before anything else and kept for the life of the process, through every seal and unseal. One JSON request a
//! line, one JSON answer a line. The commands run off the socket's thread, since a passphrase's Argon2id and the
//! `systemd-creds` call take a while. While sealed, SIGINT and SIGTERM give up the wait, so the process ends.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use moqui_core::context::conf_env::conf_attribute;
use moqui_core::context::Error;
use moqui_core::seal::Keeper;
use moqui_lib::node::MNode;
use moqui_lib::sync::{lock, read, write};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _};

use super::peer::Settings;

/// The longest request line: a few shares and a bundle fit.
const MAX_LINE: u64 = 1 << 20;

/// The sockets this process serves, by path.
static SERVED: LazyLock<Mutex<HashMap<PathBuf, Arc<RwLock<Settings>>>>> = LazyLock::new(Mutex::default);

/// The socket's path: `seal@socket` (relative to the runtime), else `moqui.sock` in the unit's `RuntimeDirectory=`
/// (`$RUNTIME_DIRECTORY`, its first entry), else `db/operator.sock` in the runtime.
pub(crate) fn path(runtime_dir: &Path, conf: &MNode) -> PathBuf {
    let set = conf_attribute(conf, "seal@socket");
    if !set.is_empty() {
        return runtime_dir.join(set);
    }
    default_path(runtime_dir)
}

/// The socket's path when `seal@socket` is empty.
pub(crate) fn default_path(runtime_dir: &Path) -> PathBuf {
    match std::env::var("RUNTIME_DIRECTORY") {
        Ok(dirs) if !dirs.trim().is_empty() => {
            let first = dirs.split(':').next().unwrap_or_default();
            PathBuf::from(first).join("moqui.sock")
        }
        _ => runtime_dir.join("db").join("operator.sock"),
    }
}

/// Serve the operator socket at `path` for `keeper`, unless this process serves it already (a server that booted
/// again after `seal`, or after a conf reload): then only its settings change.
///
/// # Errors
///
/// Another process answers on `path`, or it cannot be bound.
pub(crate) fn serve(path: &Path, keeper: Arc<Keeper>, settings: Settings) -> Result<(), Error> {
    let mut served = lock(&SERVED);
    if let Some(held) = served.get(path) {
        *write(held) = settings;
        return Ok(());
    }
    let listener = bind(path, settings.group)?;
    let settings = Arc::new(RwLock::new(settings));
    served.insert(path.to_path_buf(), Arc::clone(&settings));
    let shown = path.display().to_string();
    std::thread::Builder::new()
        .name("moqui-operator".to_string())
        .spawn(move || run(listener, keeper, settings, &shown))
        .map_err(|err| Error::new("the operator socket's thread did not start").with_source(err))?;
    tracing::info!("Operator socket open on {}", path.display());
    Ok(())
}

/// Bind `path`, mode 0660, in the operator group when there is one.
fn bind(path: &Path, group: Option<u32>) -> Result<std::os::unix::net::UnixListener, Error> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|err| Error::new(format!("could not make {}", dir.display())).with_source(err))?;
    }
    if path.exists() {
        if std::os::unix::net::UnixStream::connect(path).is_ok() {
            return Err(Error::new(format!(
                "another server answers on the operator socket {}",
                path.display()
            )));
        }
        // left by a process that ended
        let _ = std::fs::remove_file(path);
    }
    let listener = std::os::unix::net::UnixListener::bind(path)
        .map_err(|err| Error::new(format!("could not open the operator socket {}", path.display())).with_source(err))?;
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660))
        .map_err(|err| Error::new(format!("could not set the mode of {}", path.display())).with_source(err))?;
    if let Some(group) = group {
        if let Err(err) = rustix::fs::chown(path, None, Some(rustix::fs::Gid::from_raw(group))) {
            tracing::warn!(
                "Could not give the operator socket {} to group {group} ({err}): only root may run operator commands",
                path.display()
            );
        }
    }
    listener
        .set_nonblocking(true)
        .map_err(|err| Error::new("could not set the operator socket non-blocking").with_source(err))?;
    Ok(listener)
}

/// The socket's thread: accept, and on SIGINT or SIGTERM while sealed give up the wait.
fn run(listener: std::os::unix::net::UnixListener, keeper: Arc<Keeper>, settings: Arc<RwLock<Settings>>, shown: &str) {
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
                    tokio::spawn(connection(stream, Arc::clone(&keeper), Arc::clone(&settings)));
                }
                Err(err) => {
                    tracing::warn!("The operator socket {shown} did not accept: {err}");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
    });
}

/// A signal while sealed ends the wait of the gate, so the process stops instead of ignoring it.
async fn abandon_on_signal(keeper: Arc<Keeper>) {
    let Ok(mut term) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) else {
        return;
    };
    let Ok(mut interrupt) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()) else {
        return;
    };
    loop {
        tokio::select! {
            _ = term.recv() => {}
            _ = interrupt.recv() => {}
        }
        if !keeper.is_unsealed() {
            tracing::warn!("Stopping while sealed");
            keeper.abandon();
        }
    }
}

/// One connection: its peer's access, then its requests in turn.
async fn connection(stream: tokio::net::UnixStream, keeper: Arc<Keeper>, settings: Arc<RwLock<Settings>>) {
    let access = match stream.peer_cred() {
        Ok(cred) => super::peer::access(cred.uid(), cred.gid(), cred.pid(), &read(&settings)),
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
        let answer = tokio::task::spawn_blocking(move || super::commands::run(&keeper, access, &line))
            .await
            .unwrap_or_else(|_| serde_json::json!({"ok": false, "error": "the command failed"}));
        let mut text = answer.to_string();
        text.push('\n');
        if writer.write_all(text.as_bytes()).await.is_err() {
            return;
        }
        drop(zeroize::Zeroizing::new(text));
    }
}
