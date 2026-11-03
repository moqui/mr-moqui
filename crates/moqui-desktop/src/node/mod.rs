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

//! The app's node, in one of two modes compiled into the one app (plans/Mobile-Desktop.md, configurations 1 and 2): in
//! the app's own process ([`in_process`], the default), or the `moqui` binary as a supervised child ([`sidecar`]).
//! Both listen on a loopback port only, tell the app the same [`Event`]s, and stop the node and its Postgres when the
//! app quits.

mod in_process;
mod sidecar;

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// What the node tells the app.
pub(crate) enum Event {
    /// Listening at this address. A sidecar that restarted in place says it again.
    Started(SocketAddr),
    /// Did not start, or a sidecar ended by itself, with the error.
    Failed(String),
    /// Stopped by itself: a termination signal, or a restart the node asked for (true).
    Stopped { restart: bool },
}

/// Where the node runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// On a thread of the app (`moqui_host::start_embedded`).
    InProcess,
    /// The `moqui` binary, a child of the app.
    Sidecar,
    /// No node here: a client of a remote server (`src/client/`, plans/Mobile-Desktop.md configuration 3).
    Client,
}

/// The file in the app's config directory that keeps the mode chosen from the tray.
pub(crate) const MODE_FILE: &str = "node-mode";

impl Mode {
    /// `sidecar`, `in-process` or `client`, as in `MOQUI_DESKTOP_NODE` and the mode file.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "sidecar" => Some(Self::Sidecar),
            "in-process" => Some(Self::InProcess),
            "client" => Some(Self::Client),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::InProcess => "in-process",
            Self::Sidecar => "sidecar",
            Self::Client => "client",
        }
    }

    /// The other node mode, which the tray's node item switches to; from the client, the node in process.
    pub(crate) fn other(self) -> Self {
        match self {
            Self::InProcess => Self::Sidecar,
            Self::Sidecar | Self::Client => Self::InProcess,
        }
    }

    /// The tray item's text.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::InProcess => "Node: in process",
            Self::Sidecar => "Node: sidecar",
            Self::Client => "Client of a server",
        }
    }

    /// The mode of this run: `env` (`MOQUI_DESKTOP_NODE`), else `saved` (the mode file), else in process. A value
    /// that is neither mode is logged and passed over.
    pub(crate) fn choose(env: Option<&str>, saved: Option<&str>) -> Self {
        for (source, text) in [("MOQUI_DESKTOP_NODE", env), (MODE_FILE, saved)] {
            let Some(text) = text.filter(|text| !text.trim().is_empty()) else {
                continue;
            };
            match Self::parse(text) {
                Some(mode) => return mode,
                None => tracing::warn!("{source} is {text:?}, not sidecar, in-process or client; it is passed over"),
            }
        }
        Self::InProcess
    }
}

/// The mode of this run, from `MOQUI_DESKTOP_NODE` and the mode file in `config_dir` ([`Mode::choose`]).
pub(crate) fn mode_of_run(config_dir: &Path) -> Mode {
    let env = std::env::var("MOQUI_DESKTOP_NODE").ok();
    Mode::choose(env.as_deref(), saved_mode(&config_dir.join(MODE_FILE)).as_deref())
}

/// Keep the other mode than `current` in the mode file in `config_dir`, for the next start: the tray's mode item.
///
/// # Errors
///
/// Returns an error when the mode file cannot be written.
pub(crate) fn switch_mode(config_dir: &Path, current: Mode) -> std::io::Result<Mode> {
    switch_to(config_dir, current.other())
}

/// Keep `next` in the mode file in `config_dir`, for the next start: the tray's client item.
///
/// # Errors
///
/// Returns an error when the mode file cannot be written.
pub(crate) fn switch_to(config_dir: &Path, next: Mode) -> std::io::Result<Mode> {
    save_mode(&config_dir.join(MODE_FILE), next)?;
    if std::env::var_os("MOQUI_DESKTOP_NODE").is_some_and(|value| !value.is_empty()) {
        tracing::warn!("MOQUI_DESKTOP_NODE is set, and the next start takes it over the {MODE_FILE} file");
    }
    Ok(next)
}

/// The mode file's text, if it reads.
pub(crate) fn saved_mode(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Keep `mode` in the file at `path` for the next run.
///
/// # Errors
///
/// Returns an error when the file or its directory cannot be written.
pub(crate) fn save_mode(path: &Path, mode: Mode) -> std::io::Result<()> {
    path.parent().map_or(Ok(()), std::fs::create_dir_all)?;
    std::fs::write(path, format!("{}\n", mode.as_str()))
}

/// The node, in either mode. [`Node::stop`] stops it and waits for it.
pub(crate) enum Node {
    InProcess(in_process::Node),
    Sidecar(sidecar::Node),
}

impl Node {
    /// Start the node on `runtime` in `mode`, listening on 127.0.0.1 at `port`, with `properties` beside the
    /// [`listen_properties`] (the backup's, `crate::backup::node_properties`). `events` hears when it starts, fails
    /// or stops by itself, on a thread of the node's.
    ///
    /// # Errors
    ///
    /// Returns an error when the node thread, or the sidecar process, does not start.
    pub(crate) fn start(
        mode: Mode,
        runtime: PathBuf,
        port: u16,
        properties: Vec<(&'static str, String)>,
        events: impl Fn(Event) + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        match mode {
            Mode::InProcess => in_process::Node::start(runtime, port, properties, events).map(Self::InProcess),
            Mode::Sidecar => sidecar::Node::start(&runtime, port, &properties, events).map(Self::Sidecar),
            Mode::Client => Err(std::io::Error::other("a client of a server runs no node")),
        }
    }

    /// Stop the node and wait until it and Postgres have stopped.
    pub(crate) fn stop(self) {
        match self {
            Self::InProcess(node) => node.stop(),
            Self::Sidecar(node) => node.stop(),
        }
    }
}

/// The listeners of a node that only this computer reaches: HTTP on 127.0.0.1 at `port`, no public HTTPS, the cluster
/// listener on loopback at a port of the kernel's choosing, and no OpenSearch (search runs on Postgres). The edge link
/// dials out, so nothing listens for it. The node in process sets them as process properties; the sidecar gets them
/// as `-D name=value`.
pub(crate) fn listen_properties(port: u16) -> Vec<(&'static str, String)> {
    vec![
        ("webapp_http_bind", "127.0.0.1".to_string()),
        ("webapp_http_port", port.to_string()),
        ("webapp_https_listen_port", "off".to_string()),
        ("cluster_bind_address", "127.0.0.1".to_string()),
        ("cluster_https_port", "0".to_string()),
        ("opensearch_run", "false".to_string()),
    ]
}

/// The `moqui` binary the sidecar runs, also for `save-db` and `load-save` (`crate::backup`).
///
/// # Errors
///
/// Returns an error, which says where it looked, when there is none.
pub(crate) fn moqui_binary() -> Result<PathBuf, String> {
    sidecar::binary()
}

/// Whether the node at `address` answers `/health` ready.
pub(crate) fn answers_ready(address: SocketAddr) -> bool {
    sidecar::get_health(address).is_some_and(|response| sidecar::health_ready(&response))
}

/// The node's HTTP port: the one it had last time (`saved`) when it is still free, so the window's origin, and with it
/// what the pages keep in local storage, stays the same; otherwise a free one.
///
/// # Errors
///
/// Returns an error when no loopback port can be bound.
pub(crate) fn choose_port(saved: Option<u16>) -> std::io::Result<u16> {
    if let Some(port) = saved.filter(|port| *port != 0) {
        if TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok() {
            return Ok(port);
        }
        tracing::warn!("Port {port} of the last run is taken; the node takes another, and the pages start with empty local storage");
    }
    Ok(TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?.local_addr()?.port())
}

/// The port in the file at `path`, written by [`save_port`].
pub(crate) fn saved_port(path: &Path) -> Option<u16> {
    let text = std::fs::read_to_string(path).ok()?;
    let port: u16 = text.trim().parse().ok()?;
    Some(port)
}

/// Keep `port` in the file at `path` for the next run.
pub(crate) fn save_port(path: &Path, port: u16) {
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(path, format!("{port}\n")));
    if let Err(err) = written {
        tracing::warn!("Could not keep the node's port in {}: {err}", path.display());
    }
}

/// Whether a server already answers on this runtime's own port (`webapp_http_port`, default 8080), as `moqui tui`
/// checks: two nodes on one runtime would share its Postgres. A development run on this repository's runtime only.
pub(crate) fn served_on_runtime_port() -> Option<u16> {
    let port = moqui_core::web::effective_http_port(&moqui_lib::env::get_prop_or_env("webapp_http_port")).ok()?;
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    std::net::TcpStream::connect_timeout(&address, Duration::from_millis(300))
        .ok()
        .map(|_| port)
}

#[cfg(test)]
mod tests;
