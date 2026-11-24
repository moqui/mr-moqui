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

//! A bounded connection to an SMTP server for lettre (SEC-MAIL-83).
//!
//! lettre makes its own socket and reads a reply with `read_line`, whose timeout is one read's, and tests a reply
//! line's length only once it ends: a server that sends a byte now and then holds the session's thread for the life of
//! the process, and one that sends a line with no end grows it without bound. lettre gives no handle on its socket, so
//! the session goes through a relay this process holds both ends of: [`Relay::start`] connects to the server itself and
//! listens on 127.0.0.1 for lettre; the relay's thread passes the bytes on and closes both sockets at the session's
//! deadline and once the server has sent more than its byte limit, each logged, which fails lettre's blocked read.
//!
//! The relay takes the first connection from this machine to its port, within a moment of starting: another process
//! of the machine that connects first is passed to the server in lettre's place, and lettre's own connection then
//! waits unanswered until its timeout. A process of the machine can reach the server directly in any case, so this
//! gives it nothing it lacks.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The most bytes a server sends in one session, its replies together: a session's replies are a few KiB.
pub(crate) const MAX_REPLIES: u64 = 1024 * 1024;

/// One session's relay. Dropped, it shuts the server's socket and stops waiting for lettre, which ends its thread.
pub(crate) struct Relay {
    /// Where lettre connects.
    pub(crate) local: SocketAddr,
    /// The connection to the server.
    upstream: TcpStream,
    closed: Arc<AtomicBool>,
    deadline_reached: Arc<AtomicBool>,
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
        let _ = self.upstream.shutdown(Shutdown::Both);
    }
}

impl Relay {
    /// Whether the relay closed the connection for the session's deadline.
    pub(crate) fn deadline_reached(&self) -> bool {
        self.deadline_reached.load(Ordering::Acquire)
    }

    /// Connect to the first of `addresses` that answers within `connect` each, and start the relay to it, which ends
    /// at `deadline` or once the server has sent more than `max_replies` bytes. `name` names the server in the log.
    ///
    /// # Errors
    ///
    /// No address answered (the last error), or the relay's listener or thread did not start.
    pub(crate) fn start(
        addresses: &[SocketAddr],
        connect: Duration,
        deadline: Instant,
        max_replies: u64,
        name: &str,
    ) -> Result<Relay, String> {
        let mut last = None;
        let mut upstream = None;
        for address in addresses {
            // each address waits at most what is left before the deadline
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match TcpStream::connect_timeout(address, connect.min(left)) {
                Ok(stream) => {
                    upstream = Some(stream);
                    break;
                }
                Err(err) => last = Some(err),
            }
        }
        let Some(upstream) = upstream else {
            return Err(last.map_or_else(
                || "no address answered before the deadline".to_string(),
                |err| err.to_string(),
            ));
        };
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|err| format!("the relay: {err}"))?;
        let local = listener.local_addr().map_err(|err| format!("the relay: {err}"))?;
        let (from_server, to_server) = match (upstream.try_clone(), upstream.try_clone()) {
            (Ok(from), Ok(to)) => (from, to),
            (Err(err), _) | (_, Err(err)) => return Err(format!("the relay: {err}")),
        };
        let closed = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&closed);
        let deadline_reached = Arc::new(AtomicBool::new(false));
        let expired = Arc::clone(&deadline_reached);
        let name = name.to_string();
        std::thread::Builder::new()
            .name("smtp-relay".to_string())
            .spawn(move || {
                let session = accept_local(listener, Instant::now() + connect, &stop);
                pass(session, from_server, to_server, deadline, max_replies, &name, &expired);
            })
            .map_err(|err| format!("the relay: {err}"))?;
        Ok(Relay {
            local,
            upstream,
            closed,
            deadline_reached,
        })
    }
}

/// The relay's one connection from this machine, waited for until `until` or until the relay is dropped; the listener
/// goes as soon as it is taken.
fn accept_local(listener: TcpListener, until: Instant, closed: &AtomicBool) -> Option<TcpStream> {
    listener.set_nonblocking(true).ok()?;
    while Instant::now() < until && !closed.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, peer)) if peer.ip().is_loopback() => {
                stream.set_nonblocking(false).ok()?;
                return Some(stream);
            }
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(_) => return None,
        }
    }
    None
}

/// Pass what lettre writes to the server (on a thread of its own) and what the server sends back to lettre, at most
/// `max_replies` bytes and up to `deadline`, then shut both sockets.
fn pass(
    session: Option<TcpStream>,
    mut from_server: TcpStream,
    mut to_server: TcpStream,
    deadline: Instant,
    max_replies: u64,
    name: &str,
    expired: &AtomicBool,
) {
    let Some((mut to_session, mut from_session)) =
        session.and_then(|session| Some((session.try_clone().ok()?, session)))
    else {
        let _ = from_server.shutdown(Shutdown::Both);
        return;
    };
    let writes = std::thread::Builder::new()
        .name("smtp-relay-out".to_string())
        .spawn(move || {
            let _ = std::io::copy(&mut from_session, &mut to_server);
            let _ = to_server.shutdown(Shutdown::Write);
        });
    let mut buffer = [0u8; 16 * 1024];
    let mut sent = 0u64;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            expired.store(true, Ordering::Release);
            tracing::warn!("SMTP {name}: the session passed its deadline and was closed");
            break;
        }
        let _ = from_server.set_read_timeout(Some(left));
        match from_server.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                sent = sent.saturating_add(read as u64);
                if sent > max_replies {
                    tracing::warn!("SMTP {name} sent more than {max_replies} bytes in one session; it was closed");
                    break;
                }
                if to_session.write_all(&buffer[..read]).is_err() {
                    break;
                }
            }
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }
    let _ = from_server.shutdown(Shutdown::Both);
    let _ = to_session.shutdown(Shutdown::Both);
    if let Ok(writes) = writes {
        let _ = writes.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The connect to each address waits at most what is left before the session's deadline, so a server that does not
    /// answer (a blackholed address) costs the deadline, not each address's full timeout; past the deadline nothing is
    /// tried.
    #[test]
    fn a_connect_waits_at_most_until_the_deadline() {
        let blackholed: SocketAddr = "10.255.255.1:25".parse().unwrap();
        let started = Instant::now();
        let deadline = started + Duration::from_millis(300);
        let refused = Relay::start(
            &[blackholed, blackholed],
            Duration::from_secs(30),
            deadline,
            MAX_REPLIES,
            "test",
        );
        assert!(refused.is_err());
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
        let late = Relay::start(
            &[blackholed],
            Duration::from_secs(30),
            Instant::now(),
            MAX_REPLIES,
            "test",
        );
        assert_eq!(late.err().as_deref(), Some("no address answered before the deadline"));
    }
}
