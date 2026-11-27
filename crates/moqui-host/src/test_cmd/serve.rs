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
//! a server starts on the same runtime and cluster (the scratch one, or with `--db existing` the runtime itself), for
//! client specs or a person to use. With `--db existing` and no specs to run (each component named has none) no test
//! factory boots: the server starts at once on the database as it is. It serves until SIGINT or SIGTERM, or until its standard input closes when that is
//! a pipe (the one `moqui test-client --serve` holds, so the server also stops when the client dies), then a scratch
//! runtime goes unless `--keep`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moqui_lib::error::Error;

use super::TestFixture;

/// What `--serve` takes before the test factory boots: the standard input watch, and the ports the user named.
pub(super) struct Setup {
    stdin_closed: Arc<AtomicBool>,
    /// The ports and switches of [`other_ports`] set by a property or the environment before the boot, which stay as
    /// they are.
    user_ports: Vec<&'static str>,
}

/// The HTTPS listener's switch and port ([`moqui_core::listeners::SWITCHES`], plans/Runtime-Config.md §3.2).
fn https() -> &'static moqui_core::listeners::ListenerSwitch {
    moqui_core::listeners::switch("HTTPS").expect("HTTPS is one of the listeners with a switch")
}

/// The listeners with a switch besides HTTP that bind a fixed port by default or by the conf (the dev conf's SSH on
/// 2222, HTTPS on 8443, gRPC on 4317), by their rows' names in [`moqui_core::listeners::SWITCHES`].
const OTHER_SWITCHES: [&str; 3] = ["gRPC", "SSH", "HTTPS"];
/// The cluster's port, 8444 when empty: it has no switch and is always on.
const CLUSTER_PORT: &str = "cluster-member@https-port";

/// Each port besides HTTP's that the server of a test run binds a free port for instead, so it starts beside a running
/// server: its selector and its switch's, `None` for the cluster port.
fn other_ports() -> impl Iterator<Item = (&'static str, Option<&'static str>)> {
    OTHER_SWITCHES
        .iter()
        .filter_map(|name| moqui_core::listeners::switch(name))
        .flat_map(|entry| entry.ports.iter().map(move |port| (*port, Some(entry.enabled))))
        .chain([(CLUSTER_PORT, None)])
}

/// The name that sets a port of [`other_ports`]: a selector's environment name, else the property.
fn port_name(name: &str) -> String {
    if name.contains('@') {
        moqui_lib::node::path::AttrPath::parse(name)
            .ok()
            .and_then(|path| path.env_name())
            .unwrap_or_default()
    } else {
        name.to_string()
    }
}

impl Setup {
    /// Watch standard input from the start, so a client that dies during the specs gets no server, and note the ports
    /// the user set before the conf sets any.
    pub(super) fn before_boot() -> Self {
        // The advertised origin of this explicit HTTP listener must also stay HTTP for redirects and login.
        moqui_lib::env::set_property(&port_name("webapp-list/webapp[webroot]@https-enabled"), "false");
        let named = |selector: &str| user_named(selector);
        let user_ports: Vec<&'static str> = other_ports()
            .flat_map(|(port, switch)| [Some(port), switch])
            .flatten()
            .filter(|name| named(name))
            .collect();
        // HTTPS stays closed unless the user names it: this server's HTTP is on an address of its own, and the conf's
        // 8443 is a running server's (plans/Runtime-Config.md §8.2)
        let https = https();
        if !https.ports.iter().any(|port| named(port)) && !named(https.enabled) {
            moqui_lib::env::set_property(&port_name(https.enabled), "false");
        }
        Self {
            stdin_closed: watch_stdin(),
            user_ports,
        }
    }
}

/// Whether the user set the conf attribute at `selector` before the boot, by a process property, the environment or
/// the loaded `runtime/.env`, in any spelling of its environment name, as the conf merge takes them (case and the
/// characters between letters and digits aside).
pub(super) fn user_named(selector: &str) -> bool {
    use moqui_core::context::conf_env::{compare_key, env_name_path};
    let Some(wanted) = moqui_lib::node::path::AttrPath::parse(selector)
        .ok()
        .map(|path| compare_key(&path))
    else {
        return false;
    };
    let environment =
        std::env::vars_os().filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)));
    let mut names = moqui_lib::env::property_entries()
        .into_iter()
        .chain(environment)
        .chain(moqui_lib::env::dotenv_entries());
    names
        .any(|(name, value)| !value.is_empty() && env_name_path(&name).is_some_and(|path| compare_key(&path) == wanted))
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
            super::interrupt::untrack(&dir);
            super::interrupt::remove_scratch(&dir);
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
    let runtime = fixture.runtime_dir.clone();
    let on_scratch = fixture.on_scratch;
    let scratch = Scratch(fixture.close_keeping_runtime());
    serve_on(&runtime, framework, scratch, on_scratch, address, ready_file, setup)
}

/// `--serve --db existing` with no specs to run: the runtime's own database served as it is, with no test factory
/// booted before the server. Each component named (`serve_only`) must be one the runtime loads.
pub(super) fn serve_existing(
    runtime: &Path,
    framework: &Path,
    serve_only: &[String],
    address: std::net::SocketAddr,
    ready_file: Option<&Path>,
    setup: &Setup,
) -> Result<(), Error> {
    let runtime = std::fs::canonicalize(runtime)
        .map_err(|err| Error::new(format!("could not resolve {}", runtime.display())).with_source(err))?;
    let framework = std::fs::canonicalize(framework)
        .map_err(|err| Error::new(format!("could not resolve {}", framework.display())).with_source(err))?;
    let loaded =
        moqui_core::context::discover_components(&runtime, &framework).map_err(|err| Error::new(err.to_string()))?;
    if let Some(name) = serve_only
        .iter()
        .find(|name| !loaded.iter().any(|component| &&component.name == name))
    {
        return Err(super::not_loaded(name));
    }
    let _ = crate::logging::attach_files(&runtime, None);
    serve_on(&runtime, &framework, Scratch(None), false, address, ready_file, setup)
}

fn serve_on(
    runtime: &Path,
    framework: &Path,
    scratch: Scratch,
    on_scratch: bool,
    address: std::net::SocketAddr,
    ready_file: Option<&Path>,
    setup: &Setup,
) -> Result<(), Error> {
    let stdin_closed = &setup.stdin_closed;
    if stdin_closed.load(Ordering::Relaxed) {
        println!("Not serving: standard input closed");
        return Ok(());
    }
    // a selector's value as the conf files set it (the dev conf's ssh-server port), or an override
    let conf = moqui_core::context::load_merged_conf(framework, runtime).ok();
    let setting = |name: &str| {
        conf.as_ref()
            .map(|conf| moqui_core::context::conf_env::conf_attribute(conf, name))
            .unwrap_or_default()
    };
    for (name, switch) in other_ports() {
        if setup.user_ports.contains(&name) {
            continue;
        }
        let set = setting(name);
        // a listener with a switch moves only while it is on; the cluster port is always on (an empty one is 8444)
        let on = switch.is_none_or(|switch| moqui_core::context::conf_env::switch_on(&setting(switch)));
        if on {
            if !set.is_empty() && set != "0" {
                tracing::info!("{name} is {set} in the conf; the test server binds a free port instead");
            }
            moqui_lib::env::set_property(&port_name(name), "0");
        }
    }
    // a client spec's requests name their own loopback address (test_client: one per spec)
    crate::http::test_peer::allow();
    let server = crate::start_server(runtime, framework, runtime.join("db/embedded"), address)?;
    // the server stops on a signal now, and the scratch runtime goes after it
    super::interrupt::server_started();
    let url = format!("http://{}", server.address());
    println!("Serving {url} (Ctrl-C to stop)");
    if let Some(path) = ready_file {
        // Written whole and renamed, so a reader never sees part of the URL. The SSH listener, when there is one, is
        // a second line, `ssh <address>`: its port moved to a free one too (test_client/serve.rs reads it).
        let partial = path.with_extension("partial");
        let mut ready = format!("{url}\n");
        if let Some(ssh) = server.ssh_address() {
            ready.push_str(&format!("ssh {ssh}\n"));
        }
        std::fs::write(&partial, ready)
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
    if on_scratch && scratch.0.is_none() {
        println!("Test runtime kept: {}", runtime.display());
    }
    drop(scratch);
    println!("Stopped");
    Ok(())
}
