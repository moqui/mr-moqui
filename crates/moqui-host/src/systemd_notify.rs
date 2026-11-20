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

//! systemd readiness, without libsystemd.
//!
//! When `NOTIFY_SOCKET` is set, [`notify`] sends one datagram (`READY=1`, `STOPPING=1`). A host unit
//! with `Type=notify` and a Podman quadlet with `Notify=true` both wait on that. No socket means no
//! message, so a normal start is unchanged. A send that fails is ignored: a stale socket must not
//! stop the server.

use std::io::Error;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set when `moqui operator seal` stops the server (plans/Sealed-Secrets.md §3.3): the stop says `STATUS=sealed`, not
/// `STOPPING=1`, since the process stays and waits sealed, and the binary's server boots again into the gate.
static SEALED_STOP: AtomicBool = AtomicBool::new(false);

/// Mark the server's stop as a seal's.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn mark_sealed_stop() {
    SEALED_STOP.store(true, Ordering::SeqCst);
}

/// Whether the server's stop is a seal's.
pub(crate) fn sealed_stop() -> bool {
    SEALED_STOP.load(Ordering::SeqCst)
}

/// Whether the server stopped for a seal, clearing the mark.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn take_sealed_stop() -> bool {
    SEALED_STOP.swap(false, Ordering::SeqCst)
}

/// Tell systemd `state` (`READY=1`, `STOPPING=1`). A missing newline is added. Nothing happens when
/// `NOTIFY_SOCKET` is unset or empty.
pub fn notify(state: &str) {
    let Ok(address) = std::env::var("NOTIFY_SOCKET") else {
        return;
    };
    if let Err(err) = send(address.trim(), state) {
        tracing::debug!("systemd notify {state} was not delivered: {err}");
    }
}

/// Tell systemd a restart has begun. The restarted binary keeps this PID and its `READY=1` ends the
/// reload. systemd 253 and newer want the `CLOCK_MONOTONIC` time with `RELOADING=1`.
pub fn notify_reloading() {
    notify(&reloading(monotonic_usec()));
}

fn reloading(usec: u64) -> String {
    format!("RELOADING=1\nMONOTONIC_USEC={usec}")
}

#[cfg(unix)]
fn monotonic_usec() -> u64 {
    let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `now` is a valid `timespec` the call only writes.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) } != 0 {
        return 0;
    }
    let secs = u64::try_from(now.tv_sec).unwrap_or(0);
    let nanos = u64::try_from(now.tv_nsec).unwrap_or(0);
    secs.saturating_mul(1_000_000).saturating_add(nanos / 1_000)
}

#[cfg(not(unix))]
fn monotonic_usec() -> u64 {
    0
}

/// Send `state` to a systemd notify address: a filesystem path, or `@name` for the abstract namespace.
pub(crate) fn send(address: &str, state: &str) -> Result<(), Error> {
    if address.is_empty() {
        return Ok(());
    }
    let payload = payload(state)?;
    #[cfg(unix)]
    {
        let socket = std::os::unix::net::UnixDatagram::unbound()?;
        if let Some(name) = address.strip_prefix('@') {
            #[cfg(any(target_os = "linux", target_os = "android"))]
            {
                use std::os::linux::net::SocketAddrExt as _;
                let addr = std::os::unix::net::SocketAddr::from_abstract_name(name)?;
                socket.send_to_addr(&payload, &addr)?;
                return Ok(());
            }
            #[cfg(not(any(target_os = "linux", target_os = "android")))]
            {
                let _ = name;
                return Err(Error::new(
                    std::io::ErrorKind::Unsupported,
                    "abstract notify sockets are Linux only",
                ));
            }
        }
        socket.send_to(&payload, address)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = payload;
        Ok(())
    }
}

fn payload(state: &str) -> Result<Vec<u8>, Error> {
    let bytes = state.as_bytes();
    if bytes.len() > 256 {
        return Err(Error::new(
            std::io::ErrorKind::InvalidInput,
            "systemd notify message is too long",
        ));
    }
    let mut payload = bytes.to_vec();
    if !payload.ends_with(b"\n") {
        payload.push(b'\n');
    }
    Ok(payload)
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::net::UnixDatagram;
    use std::time::Duration;

    use super::{monotonic_usec, reloading, send};

    #[test]
    fn a_filesystem_socket_receives_ready_with_a_newline() {
        let dir = std::env::temp_dir().join(format!("moqui-notify-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("notify.sock");
        let listener = UnixDatagram::bind(&path).unwrap();
        listener.set_read_timeout(Some(Duration::from_secs(2))).unwrap();

        send(&path.display().to_string(), "READY=1").unwrap();

        let mut buf = [0_u8; 64];
        let read = listener.recv(&mut buf).unwrap();
        assert_eq!(&buf[..read], b"READY=1\n");
        drop(listener);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn an_abstract_socket_receives_stopping() {
        use std::os::linux::net::SocketAddrExt as _;
        let name = format!("moqui-notify-{}", std::process::id());
        let addr = std::os::unix::net::SocketAddr::from_abstract_name(&name).unwrap();
        let listener = UnixDatagram::bind_addr(&addr).unwrap();
        listener.set_read_timeout(Some(Duration::from_secs(2))).unwrap();

        send(&format!("@{name}"), "STOPPING=1\n").unwrap();

        let mut buf = [0_u8; 64];
        let read = listener.recv(&mut buf).unwrap();
        assert_eq!(&buf[..read], b"STOPPING=1\n");
    }

    #[test]
    fn reloading_carries_the_monotonic_time() {
        assert!(monotonic_usec() > 0);
        assert_eq!(reloading(42), "RELOADING=1\nMONOTONIC_USEC=42");
    }

    #[test]
    fn an_empty_address_sends_nothing() {
        send("", "READY=1").unwrap();
    }
}
