//! The listen ports a server will bind, checked once the conf properties are final and before the framework is built,
//! so a port another process holds stops the start before the database opens. Each busy port is named with the
//! process holding it when this user can see it (`/proc` on Linux, `lsof` elsewhere).

use std::net::{SocketAddr, TcpListener, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use moqui_lib::error::Error;

use crate::ListenBind;

/// One listener: the property that sets its port, and the host and port it binds.
struct Target {
    name: &'static str,
    host: String,
    port: u16,
}

/// A process listening on a port.
#[derive(Debug, PartialEq, Eq)]
struct Holder {
    pid: u32,
    name: String,
    exe: Option<PathBuf>,
}

/// The [`moqui_core::context::BootOptions::before_init`] hook for a server that will listen on `bind`.
pub(crate) fn hook(bind: ListenBind, runtime_dir: PathBuf) -> moqui_core::context::BeforeInit {
    // The message alone: a converted error keeps itself as its source and prints twice.
    let plain = |err: Error| moqui_core::context::Error::new(err.message());
    Arc::new(move || {
        let targets = targets(&bind, &runtime_dir).map_err(plain)?;
        check(&targets).map_err(plain)?;
        let ports: Vec<String> = targets.iter().map(|target| target.port.to_string()).collect();
        tracing::info!("Ports available: {}", ports.join(", "));
        Ok(())
    })
}

/// The HTTP, HTTPS, cluster and SSH listeners, resolved as each listener resolves its own. SMTP is not here: a
/// listener that cannot start is logged and the server goes on.
fn targets(bind: &ListenBind, runtime_dir: &Path) -> Result<Vec<Target>, Error> {
    let http = crate::listen_address(bind)?;
    let mut targets = vec![Target {
        name: "webapp_http_port",
        host: http.ip().to_string(),
        port: http.port(),
    }];
    if let Some(port) = crate::tls::planned_port(runtime_dir, matches!(bind, ListenBind::Property))? {
        targets.push(Target {
            name: "webapp_https_listen_port",
            host: http.ip().to_string(),
            port,
        });
    }
    targets.push(Target {
        name: "cluster_https_port",
        host: crate::cluster_edge::bind_address()?.to_string(),
        port: crate::cluster_edge::listen_port(&moqui_lib::env::get_prop_or_env("cluster_https_port"))?,
    });
    #[cfg(feature = "ssh")]
    if let Some((host, port)) = crate::ssh::server::listen_target()? {
        targets.push(Target {
            name: "ssh_port",
            host,
            port,
        });
    }
    Ok(targets)
}

/// Bind each target and let it go. Every busy port is in the one error. Port 0 is ephemeral and always free.
fn check(targets: &[Target]) -> Result<(), Error> {
    let mut busy = Vec::new();
    for target in targets.iter().filter(|target| target.port != 0) {
        if let Err(err) = bind(&target.host, target.port) {
            busy.push(format!(
                "{} ({}) on {}: {err}{}",
                target.port,
                target.name,
                target.host,
                holder_text(target.port)
            ));
        }
    }
    if busy.is_empty() {
        return Ok(());
    }
    Err(Error::new(format!(
        "Ports not available, not starting: {}",
        busy.join("; ")
    )))
}

fn bind(host: &str, port: u16) -> std::io::Result<()> {
    let addresses: Vec<SocketAddr> = (host, port).to_socket_addrs()?.collect();
    TcpListener::bind(&addresses[..]).map(drop)
}

/// `, held by pid 12 name (exe)` for each process found, else what is known of the holder.
fn holder_text(port: u16) -> String {
    let lookup = holders(port);
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
fn holders(port: u16) -> Lookup {
    let mut sockets = Vec::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        if let Ok(text) = std::fs::read_to_string(table) {
            sockets.extend(listening_sockets(&text, port));
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
fn holders(port: u16) -> Lookup {
    let output = std::process::Command::new("lsof")
        .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-Fpc"])
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
fn holders(_port: u16) -> Lookup {
    Lookup::default()
}

/// A LISTEN row of `/proc/net/tcp` or `/proc/net/tcp6`.
#[derive(Debug, PartialEq, Eq)]
struct Socket {
    uid: u32,
    inode: u64,
}

/// The rows of a `/proc/net/tcp` table in state LISTEN (`0A`) on `port`. Columns: `sl local rem st tx:rx tr:tm retrnsmt
/// uid timeout inode`, with the local port in hex after the colon.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn listening_sockets(text: &str, port: u16) -> Vec<Socket> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let columns: Vec<&str> = line.split_whitespace().collect();
            let (_, local_port) = columns.get(1)?.rsplit_once(':')?;
            if *columns.get(3)? != "0A" || u16::from_str_radix(local_port, 16).ok()? != port {
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
#[cfg_attr(target_os = "linux", allow(dead_code))]
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
mod tests {
    use super::*;

    fn target(name: &'static str, port: u16) -> Target {
        Target {
            name,
            host: "127.0.0.1".to_string(),
            port,
        }
    }

    #[test]
    fn a_held_port_is_named_and_a_free_one_passes() {
        let held = TcpListener::bind("127.0.0.1:0").unwrap();
        let held_port = held.local_addr().unwrap().port();
        let free_port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let targets = [target("webapp_http_port", held_port), target("ssh_port", free_port)];
        let err = check(&targets).unwrap_err().to_string();
        assert!(err.contains(&format!("{held_port} (webapp_http_port) on 127.0.0.1")), "{err}");
        assert!(!err.contains("ssh_port"), "{err}");
        #[cfg(target_os = "linux")]
        assert!(err.contains(&format!("held by pid {}", std::process::id())), "{err}");
        drop(held);
        check(&targets).unwrap();
    }

    #[test]
    fn port_zero_is_not_checked() {
        check(&[target("cluster_https_port", 0)]).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn holders_finds_this_process() {
        let held = TcpListener::bind("127.0.0.1:0").unwrap();
        let lookup = holders(held.local_addr().unwrap().port());
        let me = lookup
            .holders
            .iter()
            .find(|holder| holder.pid == std::process::id())
            .expect("this process holds the port");
        assert!(!me.name.is_empty());
        assert!(me.exe.is_some());
    }

    #[test]
    fn listening_sockets_reads_listen_rows_of_the_port() {
        let text = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 4242 1 0000000000000000 100 0 0 10 0
   1: 0100007F:1F90 0100007F:D431 01 00000000:00000000 00:00000000 00000000  1000        0 4343 1 0000000000000000 20 4 30 10 -1
   2: 0100007F:08AE 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 4444 1 0000000000000000 100 0 0 10 0
";
        assert_eq!(listening_sockets(text, 8080), vec![Socket { uid: 1000, inode: 4242 }]);
        assert_eq!(listening_sockets(text, 2222), vec![Socket { uid: 0, inode: 4444 }]);
        let v6 = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000000000000:20FB 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 5151 1 0000000000000000 100 0 0 10 0
";
        assert_eq!(listening_sockets(v6, 8443), vec![Socket { uid: 1000, inode: 5151 }]);
    }

    #[test]
    fn lsof_holders_pairs_pid_and_command() {
        let holders = lsof_holders("p123\ncjava\np456\ncnc\n");
        assert_eq!(
            holders,
            vec![
                Holder {
                    pid: 123,
                    name: "java".to_string(),
                    exe: None
                },
                Holder {
                    pid: 456,
                    name: "nc".to_string(),
                    exe: None
                },
            ]
        );
    }
}
