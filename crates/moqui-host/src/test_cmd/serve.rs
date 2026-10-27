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

//! `moqui test --serve [address]` (plans/archive/ClientTestHarness.md 6): once the specs pass, the test factory shuts down and
//! a server starts on the same scratch runtime and cluster, for client specs or a person to use. It serves until
//! SIGINT or SIGTERM, or until its standard input closes when that is a pipe (the one `moqui test-client --serve`
//! holds, so the server also stops when the client dies), then the scratch runtime goes unless `--keep`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moqui_lib::error::Error;

use super::TestFixture;

/// What `--serve` takes before the test factory boots: the standard input watch, and the ports the user named.
pub(super) struct Setup {
    stdin_closed: Arc<AtomicBool>,
    /// The ports of [`OTHER_PORTS`] set by a property or the environment before the boot, which stay as they are.
    user_ports: Vec<&'static str>,
}

/// The listeners besides HTTP that bind a fixed port by default or by the conf (the cluster on 8444, the dev conf's SSH
/// on 2222). The server of a test run binds a free port for each instead, so it starts beside a running server.
const OTHER_PORTS: [&str; 3] = ["cluster_https_port", "ssh_port", "webapp_https_listen_port"];

impl Setup {
    /// Watch standard input from the start, so a client that dies during the specs gets no server, and note the ports
    /// the user set before the conf sets any.
    pub(super) fn before_boot() -> Self {
        Self {
            stdin_closed: watch_stdin(),
            user_ports: OTHER_PORTS
                .into_iter()
                .filter(|name| !moqui_lib::env::get_prop_or_env(name).is_empty())
                .collect(),
        }
    }
}

/// A flag set once standard input closes. Never set when standard input is not a pipe: a terminal, or the
/// `/dev/null` of a job started in the background, does not stop the server.
fn watch_stdin() -> Arc<AtomicBool> {
    let closed = Arc::new(AtomicBool::new(false));
    if stdin_is_pipe() {
        let flag = Arc::clone(&closed);
        let started = std::thread::Builder::new()
            .name("moqui-serve-stdin".to_string())
            .spawn(move || {
                use std::io::Read as _;
                let mut stdin = std::io::stdin();
                let mut buf = [0_u8; 256];
                while matches!(stdin.read(&mut buf), Ok(read) if read > 0) {}
                flag.store(true, Ordering::Relaxed);
            });
        if let Err(err) = started {
            tracing::warn!("Not watching standard input, the server stops only on a signal: {err}");
        }
    }
    closed
}

#[cfg(unix)]
fn stdin_is_pipe() -> bool {
    use std::os::fd::AsFd as _;
    use std::os::unix::fs::FileTypeExt as _;
    std::io::stdin()
        .as_fd()
        .try_clone_to_owned()
        .map(std::fs::File::from)
        .and_then(|file| file.metadata())
        .is_ok_and(|meta| meta.file_type().is_fifo() || meta.file_type().is_socket())
}

#[cfg(not(unix))]
fn stdin_is_pipe() -> bool {
    use std::io::IsTerminal as _;
    !std::io::stdin().is_terminal()
}

/// Removes the scratch runtime when the server is done, however it ends.
struct Scratch(Option<PathBuf>);

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Some(dir) = self.0.take() {
            if let Err(err) = std::fs::remove_dir_all(&dir) {
                tracing::warn!("could not remove the scratch runtime {}: {err}", dir.display());
            }
        }
    }
}

/// Serve the fixture's runtime on `address` until stopped.
pub(super) fn serve(
    fixture: TestFixture,
    framework: &Path,
    address: std::net::SocketAddr,
    ready_file: Option<&Path>,
    setup: &Setup,
) -> Result<(), Error> {
    let stdin_closed = &setup.stdin_closed;
    let runtime = fixture.runtime_dir.clone();
    let scratch = Scratch(fixture.close_keeping_runtime());
    if stdin_closed.load(Ordering::Relaxed) {
        println!("Not serving: standard input closed");
        return Ok(());
    }
    for name in OTHER_PORTS {
        let set = moqui_lib::env::get_prop_or_env(name);
        // an empty cluster port is 8444; the others are off while empty
        if !setup.user_ports.contains(&name) && (name == "cluster_https_port" || !set.is_empty()) {
            if !set.is_empty() && set != "0" {
                tracing::info!("{name} is {set} in the conf; the test server binds a free port instead");
            }
            moqui_lib::env::set_property(name, "0");
        }
    }
    let server = crate::start_server(&runtime, framework, runtime.join("db/embedded"), address)?;
    let url = format!("http://{}", server.address());
    println!("Serving {url} (Ctrl-C to stop)");
    if let Some(path) = ready_file {
        // Written whole and renamed, so a reader never sees part of the URL.
        let partial = path.with_extension("partial");
        std::fs::write(&partial, format!("{url}\n"))
            .and_then(|()| std::fs::rename(&partial, path))
            .map_err(|err| Error::new(format!("could not write {}", path.display())).with_source(err))?;
    }
    while !server.is_stopped() && !stdin_closed.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(200));
    }
    if stdin_closed.load(Ordering::Relaxed) {
        tracing::info!("Standard input closed, stopping the server");
    }
    drop(server);
    if let Some(path) = ready_file {
        let _ = std::fs::remove_file(path);
    }
    if scratch.0.is_none() {
        println!("Test runtime kept: {}", runtime.display());
    }
    drop(scratch);
    println!("Stopped");
    Ok(())
}
