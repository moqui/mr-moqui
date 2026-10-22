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

//! `moqui tui` (plans/archive/Screen-TUI-Impl.md 4.1): the terminal client on this runtime, in process. It boots with no
//! listeners unless `--serve`, logs to the files and the log pane only, and asks for a login unless a dev instance names
//! the user.

use moqui_lib::error::Error;

/// The arguments of `moqui tui`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TuiFlags {
    pub path: String,
    pub user: Option<String>,
    pub ascii: bool,
    pub no_color: bool,
    pub serve: bool,
    /// Print one frame of `path` at this size and exit; with `agent`, the agent's drawing size.
    pub snapshot: Option<(u16, u16)>,
    /// Command lines on stdin, an answer each on stdout (`moqui_tui::Agent`), for a program rather than a person.
    pub agent: bool,
    /// A running server to reach over HTTP rather than this runtime in process.
    pub server: Option<String>,
    /// Remove the login key stored for `server`.
    pub forget: bool,
}

/// # Errors
///
/// Returns an error for an unknown flag or a bad value.
pub fn parse(args: &[String]) -> Result<TuiFlags, Error> {
    let mut flags = TuiFlags {
        path: "/apps".to_string(),
        ..TuiFlags::default()
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) => (name, Some(value.to_string())),
            None => (arg.as_str(), None),
        };
        let mut value = |what: &str| {
            inline
                .clone()
                .or_else(|| args.next().cloned())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| Error::new(format!("moqui tui: {name} needs {what}")))
        };
        match name {
            "--path" => flags.path = value("a screen path")?,
            "--user" => flags.user = Some(value("a username")?),
            "--ascii" => flags.ascii = true,
            "--no-color" => flags.no_color = true,
            "--serve" => flags.serve = true,
            "--agent" => flags.agent = true,
            "--server" => flags.server = Some(value("a url such as https://erp.example.com")?),
            "--forget" => flags.forget = true,
            "--snapshot" => {
                flags.path = value("a screen path")?;
                flags.snapshot.get_or_insert((180, 50));
            }
            "--size" => {
                let size = value("a size such as 180x50")?;
                let parsed = size
                    .split_once('x')
                    .and_then(|(w, h)| Some((w.parse::<u16>().ok()?, h.parse::<u16>().ok()?)))
                    .filter(|(w, h)| *w >= 20 && *h >= 6)
                    .ok_or_else(|| Error::new(format!("moqui tui: --size {size} is not WIDTHxHEIGHT")))?;
                flags.snapshot = Some(parsed);
            }
            other => {
                return Err(Error::new(format!(
                    "moqui tui: unknown flag {other}\nRun 'moqui help tui'."
                )))
            }
        }
    }
    if !flags.path.starts_with('/') {
        flags.path.insert(0, '/');
    }
    if flags.forget && flags.server.is_none() {
        return Err(Error::new(
            "moqui tui: --forget needs --server, the server whose key to forget",
        ));
    }
    if flags.server.is_some() && (flags.user.is_some() || flags.serve) {
        return Err(Error::new(
            "moqui tui: --server reaches a running server, which asks for the login; --user and --serve are for this runtime",
        ));
    }
    Ok(flags)
}

/// Run `moqui tui`: over HTTP with `--server`, else on this runtime.
///
/// # Errors
///
/// As [`run_here`] and [`run_remote`].
pub fn run(flags: &TuiFlags) -> Result<(), Error> {
    match &flags.server {
        Some(server) => run_remote(flags, server),
        None => run_here(flags),
    }
}

/// `--server`: the client of a running server, with no runtime of its own (4.3).
fn run_remote(flags: &TuiFlags, server: &str) -> Result<(), Error> {
    if flags.forget {
        let path = crate::tui_remote::key_file(server)?;
        if crate::tui_remote::forget(server)? {
            println!("Removed the login key in {}", path.display());
        } else {
            println!("No login key stored for {server}");
        }
        return Ok(());
    }
    if flags.snapshot.is_none() || flags.agent {
        crate::logging::install_without_console();
    } else {
        crate::logging::install_with_console(Some(tracing::Level::ERROR));
    }
    let log = crate::logging::tap(500);
    let link = crate::tui_remote::HttpLink::connect(server, Some(crate::tui_remote::key_file(server)?))?;
    present(link, flags, log)
}

/// The client on `link`: the agent's lines, one snapshot, or the terminal.
fn present<L: moqui_tui::ScreenLink + 'static>(
    link: L,
    flags: &TuiFlags,
    log: std::sync::mpsc::Receiver<String>,
) -> Result<(), Error> {
    if flags.agent {
        let (columns, rows) = flags.snapshot.unwrap_or((120, 40));
        return agent_lines(link, &flags.path, columns, rows)
            .map_err(|err| Error::new(format!("moqui tui --agent: {err}")));
    }
    let theme = moqui_tui::Theme::detect(&|name| std::env::var(name).ok(), flags.ascii, flags.no_color);
    if let Some((width, height)) = flags.snapshot {
        println!("{}", moqui_tui::snapshot(link, &flags.path, width, height, theme));
        return Ok(());
    }
    let options = moqui_tui::LocalOptions {
        path: flags.path.clone(),
        theme,
        notes: Some(|note| tracing::warn!("TUI theme: {note}")),
        log: Some(log),
    };
    moqui_tui::run_local(link, options).map_err(|err| Error::new(format!("moqui tui: {err}")))
}

/// `moqui tui` on this runtime.
///
/// # Errors
///
/// Returns an error when the runtime is already served, the boot fails, `--user` is given on an instance that is not
/// for development, or the terminal cannot be drawn.
#[cfg(all(feature = "axum", any(feature = "postgres", feature = "mysql")))]
fn run_here(flags: &TuiFlags) -> Result<(), Error> {
    use crate::tui::HostLink;

    let (runtime, framework) = crate::runtime_framework_dirs();
    let cluster = runtime.join("db/embedded");
    let port = crate::http::http_port(&moqui_lib::env::get_prop_or_env("webapp_http_port")).unwrap_or(8080);
    if !flags.serve && served_on(port) {
        return Err(Error::new(format!(
            "moqui tui: a server already answers on port {port}, likely on this runtime. Stop it first, or reach it \
             with `moqui tui --server http://localhost:{port}`."
        )));
    }
    // Before anything logs: a line on stderr would tear the screen.
    if flags.snapshot.is_none() {
        crate::logging::install_without_console();
    } else {
        crate::logging::install_with_console(Some(tracing::Level::ERROR));
    }
    let log = crate::logging::tap(500);
    let keep_alive: Box<dyn std::any::Any> = if flags.serve {
        let server = crate::start_server_in(
            runtime.clone(),
            framework.clone(),
            cluster.clone(),
            crate::ListenBind::Property,
        )?;
        Box::new(server)
    } else {
        Box::new(crate::start_local(&runtime, &framework, &cluster)?)
    };
    let factory = keep_alive
        .downcast_ref::<crate::LocalRuntime>()
        .map(|local| std::sync::Arc::clone(local.factory()))
        .or_else(|| {
            keep_alive
                .downcast_ref::<crate::Server>()
                .map(|server| std::sync::Arc::clone(server.factory()))
        })
        .ok_or_else(|| Error::new("moqui tui: the runtime has no factory"))?;
    let link = match &flags.user {
        Some(user) => {
            let purpose = moqui_lib::env::get_prop_or_env("instance_purpose");
            if purpose != "dev" {
                return Err(Error::new(format!(
                    "moqui tui: --user logs in without a password, so only an instance_purpose of dev takes it (this \
                     one is {})",
                    if purpose.is_empty() { "production" } else { &purpose }
                )));
            }
            HostLink::as_user(&factory, user.clone())
        }
        None => HostLink::new(&factory),
    };
    let result = present(link, flags, log);
    drop(keep_alive);
    result
}

/// `--agent`: the first screen, then one answer for each command line on stdin, until `close` or the end of input.
fn agent_lines<L: moqui_tui::ScreenLink>(link: L, path: &str, columns: u16, rows: u16) -> std::io::Result<()> {
    use std::io::{BufRead, Write};

    let mut agent = moqui_tui::Agent::open(link, path, columns, rows);
    let mut out = std::io::stdout().lock();
    writeln!(out, "{}", agent.look().text)?;
    out.flush()?;
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        writeln!(out, "{}", agent.run(&line).text)?;
        out.flush()?;
        if agent.closed() {
            break;
        }
    }
    Ok(())
}

#[cfg(not(all(feature = "axum", any(feature = "postgres", feature = "mysql"))))]
fn run_here(_flags: &TuiFlags) -> Result<(), Error> {
    Err(Error::new(
        "moqui tui needs a database and the server pieces to run here; this build has neither. Reach a running server \
         with --server <url>",
    ))
}

/// Something answers on the loopback port: a server holds this runtime's HTTP port.
#[cfg(all(feature = "axum", any(feature = "postgres", feature = "mysql")))]
fn served_on(port: u16) -> bool {
    let address = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
    std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_millis(300)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_string()).collect()
    }

    #[test]
    fn flags_read_and_a_bad_one_is_refused() {
        let flags = parse(&args(&[
            "--path",
            "apps/gallery/List",
            "--user=john.doe",
            "--ascii",
            "--size",
            "100x40",
            "--agent",
        ]))
        .unwrap();
        assert!(flags.agent);
        assert_eq!(flags.path, "/apps/gallery/List");
        assert_eq!(flags.user.as_deref(), Some("john.doe"));
        assert!(flags.ascii);
        assert_eq!(flags.snapshot, Some((100, 40)));
        assert!(parse(&args(&["--size", "wide"])).is_err());
        assert!(parse(&args(&["--frobnicate"])).is_err());
        assert!(parse(&args(&["--user"])).is_err());
        assert_eq!(parse(&args(&[])).unwrap().path, "/apps");
    }
}
