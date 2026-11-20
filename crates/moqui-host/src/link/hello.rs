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

//! The ClientHello of a connection to the HTTPS listener, read before any TLS settings are picked, so a link's ALPN
//! gets the link's settings and a browser the listener's own.

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// The most a ClientHello may take. It is read whole before any TLS settings are picked, and a passthrough tunnel
/// replays it whole.
pub(crate) const MAX_HELLO: usize = 64 * 1024;

/// Read the ClientHello from `stream`: rustls's view of it, and every byte read, which a passthrough tunnel replays.
pub(crate) async fn read_hello<S>(stream: &mut S) -> std::io::Result<(rustls::server::Accepted, Vec<u8>)>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut acceptor = rustls::server::Acceptor::default();
    let mut seen = Vec::with_capacity(2048);
    let mut chunk = [0_u8; 4096];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        let fresh = &chunk[..read];
        seen.extend_from_slice(fresh);
        if seen.len() > MAX_HELLO {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the ClientHello is too long",
            ));
        }
        let mut rest = fresh;
        while !rest.is_empty() {
            if acceptor.read_tls(&mut rest)? == 0 {
                break;
            }
        }
        match acceptor.accept() {
            Ok(Some(accepted)) => return Ok((accepted, seen)),
            Ok(None) => {}
            Err((err, mut alert)) => {
                let mut out = Vec::new();
                if alert.write_all(&mut out).is_ok() {
                    let _ = stream.write_all(&out).await;
                }
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, err));
            }
        }
    }
}
