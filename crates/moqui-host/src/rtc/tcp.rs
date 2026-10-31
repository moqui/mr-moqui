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

//! ICE-TCP, passive (plans/Comms-Framework.md §6.5, W4): a TCP listener on `rtc_media_tcp_port` for browsers that cannot
//! reach the UDP media port, tried before a TURN relay. Each connection carries ICE and DTLS-SRTP datagrams framed as
//! RFC 4571 says (a two-byte length before each). A thread per connection reads and hands each datagram to the driver;
//! the driver writes back through the connection's stream. At most [`MAX_CONNECTIONS`] connections.

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moqui_lib::sync::lock;

use super::Command;

/// The most ICE-TCP connections at once.
const MAX_CONNECTIONS: usize = 1_000;
/// The largest datagram in a frame.
const MAX_FRAME: usize = 2_048;

/// The writing half of each connection, by the browser's address.
pub(crate) type Streams = Arc<Mutex<HashMap<SocketAddr, TcpStream>>>;

/// Writes one datagram to the browser at `to`, framed; a connection that fails is dropped.
pub(crate) fn send(streams: &Streams, to: SocketAddr, data: &[u8]) {
    let Ok(len) = u16::try_from(data.len()) else { return };
    let mut all = lock(streams);
    let Some(stream) = all.get_mut(&to) else { return };
    let mut frame = Vec::with_capacity(data.len() + 2);
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(data);
    if stream.write_all(&frame).is_err() {
        all.remove(&to);
    }
}

/// Starts the listener's accept thread. The connections' datagrams go to the driver as [`Command::TcpIn`].
pub(crate) fn start(
    listener: TcpListener,
    streams: Streams,
    commands: SyncSender<Command>,
    stop: Arc<AtomicBool>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    listener.set_nonblocking(true)?;
    let open = Arc::new(AtomicUsize::new(0));
    std::thread::Builder::new()
        .name("moqui-rtc-tcp".to_string())
        .spawn(move || {
            while !stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, peer)) => {
                        if open.load(Ordering::Acquire) >= MAX_CONNECTIONS {
                            continue;
                        }
                        let _ = stream.set_nodelay(true);
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
                        let Ok(writer) = stream.try_clone() else { continue };
                        lock(&streams).insert(peer, writer);
                        open.fetch_add(1, Ordering::AcqRel);
                        let (conn_streams, conn_commands, conn_stop, conn_open) = (
                            Arc::clone(&streams),
                            commands.clone(),
                            Arc::clone(&stop),
                            Arc::clone(&open),
                        );
                        let spawned = std::thread::Builder::new()
                            .name("moqui-rtc-tcp-conn".to_string())
                            .spawn(move || {
                                read_frames(stream, peer, &conn_commands, &conn_stop);
                                lock(&conn_streams).remove(&peer);
                                conn_open.fetch_sub(1, Ordering::AcqRel);
                            });
                        if spawned.is_err() {
                            lock(&streams).remove(&peer);
                            open.fetch_sub(1, Ordering::AcqRel);
                        }
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
                    Err(e) => {
                        tracing::debug!("ICE-TCP accept failed: {e}");
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
            }
        })
}

fn read_frames(mut stream: TcpStream, peer: SocketAddr, commands: &SyncSender<Command>, stop: &AtomicBool) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
    let mut buf: Vec<u8> = Vec::with_capacity(4_096);
    let mut chunk = [0u8; 4_096];
    while !stop.load(Ordering::Acquire) {
        match stream.read(&mut chunk) {
            Ok(0) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) =>
            {
                continue
            }
            Err(_) => return,
        }
        while buf.len() >= 2 {
            let len = usize::from(u16::from_be_bytes([buf[0], buf[1]]));
            if len > MAX_FRAME {
                return;
            }
            if buf.len() < 2 + len {
                break;
            }
            let data: Vec<u8> = buf[2..2 + len].to_vec();
            buf.drain(..2 + len);
            // a full driver drops the datagram, as a full UDP buffer would
            let _ = commands.try_send(Command::TcpIn { from: peer, data });
        }
    }
}
