//! The listen ports a server will bind, checked once the conf properties are final and before the framework is built,
//! so a port another process holds is found before the database opens. Each busy port is named with the process
//! holding it when this user can see it (`/proc` on Linux, `lsof` elsewhere).
//!
//! The same list names the listeners of the startup report (`crate::startup_report`): HTTP, HTTPS, cluster and SSH are
//! required, and a busy one stops the start; the others (gRPC, SMTP, call media, TURN, SIP, XMPP, the edge tunnels)
//! are optional, and a busy one is warned about and left out: its start asks [`busy`] and records it skipped instead
//! of trying the port again.

// Without a server boot only the busy list is read (by the optional listeners that are built).
#![cfg_attr(
    not(all(feature = "axum", any(feature = "postgres", feature = "mysql"))),
    allow(dead_code)
)]

use std::net::{SocketAddr, TcpListener, ToSocketAddrs, UdpSocket};
#[cfg(all(feature = "axum", any(feature = "postgres", feature = "mysql")))]
use std::path::{Path, PathBuf};
#[cfg(all(feature = "axum", any(feature = "postgres", feature = "mysql")))]
use std::sync::Arc;
use std::sync::Mutex;

use moqui_lib::error::Error;
#[cfg(all(feature = "axum", any(feature = "postgres", feature = "mysql")))]
use moqui_lib::node::MNode;
use moqui_lib::sync::lock;

#[cfg(all(feature = "axum", any(feature = "postgres", feature = "mysql")))]
use crate::ListenBind;

/// The transport a listener binds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Proto {
    Tcp,
    Udp,
}

/// One port of a listener: the listener's name in the startup report, the property that sets the port, and the host,
/// port and transport it binds.
#[derive(Clone, Debug)]
pub(crate) struct Target {
    pub(crate) listener: &'static str,
    pub(crate) property: &'static str,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) proto: Proto,
    /// A busy port stops the start; else the listener is left out.
    pub(crate) required: bool,
}

impl Target {
    /// A port of an optional listener.
    pub(crate) fn optional(
        listener: &'static str,
        property: &'static str,
        host: impl ToString,
        port: u16,
        proto: Proto,
    ) -> Self {
        Self {
            listener,
            property,
            host: host.to_string(),
            port,
            proto,
            required: false,
        }
    }

    /// `HTTP 8080`, `Call media 10000/udp`.
    fn label(&self) -> String {
        match self.proto {
            Proto::Tcp => format!("{} {}", self.listener, self.port),
            Proto::Udp => format!("{} {}/udp", self.listener, self.port),
        }
    }
}

/// A process listening on a port.
#[derive(Debug, PartialEq, Eq)]
struct Holder {
    pid: u32,
    name: String,
    exe: Option<std::path::PathBuf>,
}

/// The optional listeners whose port was busy at the last check, with the port.
static BUSY: Mutex<Vec<(&'static str, u16)>> = Mutex::new(Vec::new());

/// Why `listener` is left out, when one of its ports was busy at the start check: its start records this as skipped
/// and does not bind.
pub(crate) fn busy(listener: &str) -> Option<String> {
    lock(&BUSY)
        .iter()
        .find(|(name, _)| *name == listener)
        .map(|(_, port)| format!("port {port} was busy at the start check"))
}

/// The [`moqui_core::context::BootOptions::before_init`] hook for a server that will listen on `bind`.
#[cfg(all(feature = "axum", any(feature = "postgres", feature = "mysql")))]
pub(crate) fn hook(bind: ListenBind, runtime_dir: PathBuf) -> moqui_core::context::BeforeInit {
    // The message alone: a converted error keeps itself as its source and prints twice.
    let plain = |err: Error| moqui_core::context::Error::new(err.message());
    Arc::new(move |conf: &MNode| {
        let targets = targets(&bind, &runtime_dir, conf).map_err(plain)?;
        *lock(&BUSY) = check(&targets).map_err(plain)?;
        Ok(())
    })
}

/// An attribute of the merged conf by element path (`telemetry/receiver`), expanded and trimmed, as
/// `ExecutionContextFactory::conf_setting` reads it once the factory is built.
#[cfg(all(feature = "axum", any(feature = "postgres", feature = "mysql")))]
fn conf_setting(conf: &MNode, path: &str, name: &str) -> String {
    let mut node = Some(conf.clone());
    for step in path.split('/') {
        node = node.and_then(|node| node.first_named(step));
    }
    node.and_then(|node| node.attribute(name))
        .map(|raw| moqui_lib::env::expand(&raw).unwrap_or(raw).trim().to_string())
        .unwrap_or_default()
}

/// Every listener's ports, resolved as each listener resolves its own: HTTP, HTTPS, cluster and SSH (from the merged
/// `conf`, so not while `ssh-server` `enabled` is false), required; then the optional ones that are configured. The
/// on-demand ranges (TURN relays, SIP RTP legs) are not here.
#[cfg(all(feature = "axum", any(feature = "postgres", feature = "mysql")))]
#[cfg_attr(not(feature = "ssh"), allow(unused_variables))]
fn targets(bind: &ListenBind, runtime_dir: &Path, conf: &MNode) -> Result<Vec<Target>, Error> {
    let mut targets = Vec::new();
    let required = |listener: &'static str, property: &'static str, host: String, port: u16| Target {
        listener,
        property,
        host,
        port,
        proto: Proto::Tcp,
        required: true,
    };
    // a node reached in process only has no HTTP, HTTPS, cluster or gRPC port
    let http = crate::listen_address(bind)?;
    #[cfg_attr(not(feature = "grpc"), allow(unused_variables))]
    let mut https = false;
    if let Some(http) = http {
        targets.push(required("HTTP", "webapp_http_port", http.ip().to_string(), http.port()));
        if let Some(port) = crate::tls::planned_port(runtime_dir, matches!(bind, ListenBind::Property))? {
            https = true;
            targets.push(required(
                "HTTPS",
                "webapp_https_listen_port",
                http.ip().to_string(),
                port,
            ));
        }
        targets.push(required(
            "Cluster",
            "cluster_https_port",
            crate::cluster_edge::bind_address()?.to_string(),
            crate::cluster_edge::listen_port(&moqui_lib::env::get_prop_or_env("cluster_https_port"))?,
        ));
    }
    #[cfg(feature = "ssh")]
    if let Some((host, port)) = crate::ssh::server::listen_target(conf)? {
        targets.push(required("SSH", "ssh_port", host, port));
    }
    // the optional listeners: a setting that does not parse is left to the listener's own start to report
    #[cfg(feature = "grpc")]
    if let Some(http) = http {
        let setting = |path: &str, name: &str| conf_setting(conf, path, name);
        if let Some(address) = crate::grpc_listen::bind_address_by(&setting, http, https) {
            targets.push(Target::optional(
                crate::grpc_listen::LISTENER,
                "grpc-port",
                address.ip(),
                address.port(),
                Proto::Tcp,
            ));
        }
    }
    #[cfg(feature = "inbound-smtp")]
    if let Ok(Some((host, port))) = crate::smtp::listen_target_by(&|name| conf_setting(conf, "smtp-server", name)) {
        targets.push(Target::optional(
            crate::smtp::LISTENER,
            "smtp_listen_port",
            host,
            port,
            Proto::Tcp,
        ));
    }
    #[cfg(feature = "rtc")]
    targets.extend(crate::rtc::listen_targets());
    #[cfg(feature = "sip")]
    targets.extend(crate::sip::listen_targets());
    #[cfg(feature = "xmpp")]
    targets.extend(crate::xmpp::listen_targets());
    #[cfg(feature = "edge")]
    targets.extend(crate::edge_link::tunnel_targets());
    Ok(targets)
}

/// Bind each target and let it go. Every busy required port is in the one error; a busy optional one is a WARN, and
/// its listener and port are returned for [`BUSY`]. Port 0 is ephemeral and always free.
fn check(targets: &[Target]) -> Result<Vec<(&'static str, u16)>, Error> {
    let mut busy_required = Vec::new();
    let mut busy_optional = Vec::new();
    let mut free = Vec::new();
    for target in targets {
        if target.port == 0 {
            continue;
        }
        let Err(err) = bind(&target.host, target.port, target.proto) else {
            free.push(target);
            continue;
        };
        let text = format!(
            "{}{} ({}) on {}: {err}{}",
            target.port,
            if target.proto == Proto::Udp { "/udp" } else { "" },
            target.property,
            target.host,
            holder_text(target.port, target.proto)
        );
        if target.required {
            busy_required.push(text);
        } else {
            tracing::warn!("Port {text}; {} will not start", target.listener);
            busy_optional.push((target.listener, target.port));
        }
    }
    if !busy_required.is_empty() {
        return Err(Error::new(format!(
            "Ports not available, not starting: {}",
            busy_required.join("; ")
        )));
    }
    // a listener left out has none of its ports in the list, though some of them were free
    let free: Vec<String> = free
        .into_iter()
        .filter(|target| !busy_optional.iter().any(|(listener, _)| *listener == target.listener))
        .map(Target::label)
        .collect();
    if !free.is_empty() {
        tracing::info!("Ports checked free: {}", free.join(", "));
    }
    Ok(busy_optional)
}

fn bind(host: &str, port: u16, proto: Proto) -> std::io::Result<()> {
    let addresses: Vec<SocketAddr> = (host, port).to_socket_addrs()?.collect();
    match proto {
        Proto::Tcp => TcpListener::bind(&addresses[..]).map(drop),
        Proto::Udp => UdpSocket::bind(&addresses[..]).map(drop),
    }
}

/// `, held by pid 12 name (exe)` for each process found, else what is known of the holder.
fn holder_text(port: u16, proto: Proto) -> String {
    let lookup = holders(port, proto);
    if !lookup.holders.is_empty() {
        let each: Vec<String> = lookup
            .holders
            .iter()
            .map(|holder| match &holder.exe {
                Some(exe) => format!("pid {} {} ({})", holder.pid, holder.name, exe.display()),
                None => format!("pid {} {}", holder.pid, holder.name),
            })
            .collect();
        return format!(", held by {}", each.join(", "));
    }
    match lookup.uids.first() {
        Some(uid) => format!(", held by a process of uid {uid} this user cannot see"),
        None => String::new(),
    }
}

#[derive(Default)]
struct Lookup {
    holders: Vec<Holder>,
    /// Owners of the listening sockets, for a holder this user cannot see.
    uids: Vec<u32>,
}

#[cfg(target_os = "linux")]
fn holders(port: u16, proto: Proto) -> Lookup {
    let mut sockets = Vec::new();
    let tables = match proto {
        Proto::Tcp => ["/proc/net/tcp", "/proc/net/tcp6"],
        Proto::Udp => ["/proc/net/udp", "/proc/net/udp6"],
    };
    for table in tables {
        if let Ok(text) = std::fs::read_to_string(table) {
            sockets.extend(listening_sockets(&text, port, proto));
        }
    }
    if sockets.is_empty() {
        return Lookup::default();
    }
    let mut lookup = Lookup {
        holders: Vec::new(),
        uids: sockets.iter().map(|socket| socket.uid).collect(),
    };
    lookup.uids.sort_unstable();
    lookup.uids.dedup();
    let links: Vec<String> = sockets
        .iter()
        .map(|socket| format!("socket:[{}]", socket.inode))
        .collect();
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return lookup;
    };
    for entry in procs.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else {
            continue;
        };
        let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        let holds = fds.flatten().any(|fd| {
            std::fs::read_link(fd.path())
                .is_ok_and(|target| links.iter().any(|link| target.as_os_str() == link.as_str()))
        });
        if holds {
            let name = std::fs::read_to_string(entry.path().join("comm")).unwrap_or_default();
            lookup.holders.push(Holder {
                pid,
                name: name.trim().to_string(),
                exe: std::fs::read_link(entry.path().join("exe")).ok(),
            });
        }
    }
    lookup
}

#[cfg(all(unix, not(target_os = "linux")))]
fn holders(port: u16, proto: Proto) -> Lookup {
    let args = match proto {
        Proto::Tcp => vec![
            "-nP".to_string(),
            format!("-iTCP:{port}"),
            "-sTCP:LISTEN".to_string(),
            "-Fpc".to_string(),
        ],
        Proto::Udp => vec!["-nP".to_string(), format!("-iUDP:{port}"), "-Fpc".to_string()],
    };
    let output = std::process::Command::new("lsof")
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    match output {
        Ok(output) => Lookup {
            holders: lsof_holders(&String::from_utf8_lossy(&output.stdout)),
            uids: Vec::new(),
        },
        Err(_) => Lookup::default(),
    }
}

#[cfg(not(unix))]
fn holders(_port: u16, _proto: Proto) -> Lookup {
    Lookup::default()
}

/// A LISTEN row of `/proc/net/tcp` or `/proc/net/tcp6`, or a bound row of `/proc/net/udp` or `/proc/net/udp6`.
#[derive(Debug, PartialEq, Eq)]
struct Socket {
    uid: u32,
    inode: u64,
}

/// The rows of a `/proc/net/tcp` table in state LISTEN (`0A`) on `port`, or of a `/proc/net/udp` table in state
/// UNCONN (`07`, a bound socket). Columns: `sl local rem st tx:rx tr:tm retrnsmt uid timeout inode`, with the local port
/// in hex after the colon.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn listening_sockets(text: &str, port: u16, proto: Proto) -> Vec<Socket> {
    let state = match proto {
        Proto::Tcp => "0A",
        Proto::Udp => "07",
    };
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let columns: Vec<&str> = line.split_whitespace().collect();
            let (_, local_port) = columns.get(1)?.rsplit_once(':')?;
            if *columns.get(3)? != state || u16::from_str_radix(local_port, 16).ok()? != port {
                return None;
            }
            Some(Socket {
                uid: columns.get(7)?.parse().ok()?,
                inode: columns.get(9)?.parse().ok()?,
            })
        })
        .filter(|socket| socket.inode != 0)
        .collect()
}

/// The processes in `lsof -F pc` output: a `p<pid>` line, then its `c<command>` line.
/// Used on Unix other than Linux. Linux reads `/proc`, and Windows has no `lsof`.
#[cfg_attr(not(all(unix, not(target_os = "linux"))), allow(dead_code))]
fn lsof_holders(text: &str) -> Vec<Holder> {
    let mut holders: Vec<Holder> = Vec::new();
    for line in text.lines() {
        if let Some(pid) = line.strip_prefix('p').and_then(|pid| pid.parse().ok()) {
            holders.push(Holder {
                pid,
                name: String::new(),
                exe: None,
            });
        } else if let (Some(name), Some(holder)) = (line.strip_prefix('c'), holders.last_mut()) {
            holder.name = name.to_string();
        }
    }
    holders
}

#[cfg(test)]
#[path = "port_check_tests.rs"]
mod tests;
