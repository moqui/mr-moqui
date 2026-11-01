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

//! The PROXY protocol header (HAProxy's v1 text and v2 binary forms) that names the client of a proxied connection:
//! read by the SMTP listener behind a mail gateway (`smtp.rs`) and by a local node on an edge link's passthrough tunnel
//! (`edge_link/down.rs`, plans/Edge-Node.md §5.3), and written by the edge node. Nothing reads it on an open socket.

use std::net::{IpAddr, SocketAddr};

use tokio::io::{AsyncRead, AsyncReadExt as _};

/// The PROXY v2 signature.
pub(crate) const V2_SIGNATURE: [u8; 12] = [0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A];
/// The longest v1 line after `PROXY `, with its CRLF (107 bytes in all).
const MAX_V1: usize = 101;
/// The longest v2 address block taken.
const MAX_V2: usize = 4096;

/// Why a header was not read.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ProxyError {
    /// The connection closed before the header ended.
    Closed,
    Io(String),
    /// Not a header, or one that does not parse.
    Invalid(String),
}

impl std::fmt::Display for ProxyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => f.write_str("closed before the PROXY header ended"),
            Self::Io(text) | Self::Invalid(text) => f.write_str(text),
        }
    }
}

fn io(err: &std::io::Error) -> ProxyError {
    if err.kind() == std::io::ErrorKind::UnexpectedEof {
        ProxyError::Closed
    } else {
        ProxyError::Io(err.to_string())
    }
}

fn invalid(text: &str) -> ProxyError {
    ProxyError::Invalid(text.to_string())
}

/// Read a v1 or v2 header from the start of `reader`, and no byte after it: the client's address (port 0 when v1
/// names none that parses), or `None` for a proxy's own connection (v1 UNKNOWN, v2 LOCAL, or an address family other
/// than IPv4 and IPv6).
pub(crate) async fn read<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Option<SocketAddr>, ProxyError> {
    let mut start = [0_u8; 6];
    reader.read_exact(&mut start).await.map_err(|err| io(&err))?;
    if &start == b"PROXY " {
        return read_v1(reader).await;
    }
    if start != V2_SIGNATURE[..6] {
        return Err(invalid("no PROXY header"));
    }
    let mut rest = [0_u8; 10];
    reader.read_exact(&mut rest).await.map_err(|err| io(&err))?;
    if rest[..6] != V2_SIGNATURE[6..] || rest[6] >> 4 != 2 {
        return Err(invalid("a PROXY v2 header that does not parse"));
    }
    let command = rest[6] & 0x0F;
    let family = rest[7] >> 4;
    let length = usize::from(u16::from_be_bytes([rest[8], rest[9]]));
    if length > MAX_V2 {
        return Err(invalid("a PROXY v2 header too long"));
    }
    let mut block = vec![0_u8; length];
    reader.read_exact(&mut block).await.map_err(|err| io(&err))?;
    if command == 0 {
        return Ok(None);
    }
    match family {
        1 => match block.get(..12) {
            Some(&[a, b, c, d, _, _, _, _, p1, p2, _, _]) => Ok(Some(SocketAddr::new(
                IpAddr::from([a, b, c, d]),
                u16::from_be_bytes([p1, p2]),
            ))),
            _ => Err(invalid("a PROXY v2 address block too short")),
        },
        2 => match (block.get(..16), block.get(32..34)) {
            (Some(source), Some(&[p1, p2])) => {
                let mut octets = [0_u8; 16];
                octets.copy_from_slice(source);
                Ok(Some(SocketAddr::new(
                    IpAddr::from(octets),
                    u16::from_be_bytes([p1, p2]),
                )))
            }
            _ => Err(invalid("a PROXY v2 address block too short")),
        },
        _ => Ok(None),
    }
}

/// The rest of a v1 line, read a byte at a time so nothing after it is taken.
async fn read_v1<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Option<SocketAddr>, ProxyError> {
    let mut line = Vec::with_capacity(MAX_V1);
    loop {
        let byte = reader.read_u8().await.map_err(|err| io(&err))?;
        line.push(byte);
        if byte == b'\n' {
            break;
        }
        if line.len() >= MAX_V1 {
            return Err(invalid("a PROXY v1 header too long or not ended"));
        }
    }
    if !line.ends_with(b"\r\n") {
        return Err(invalid("a PROXY v1 header too long or not ended"));
    }
    let text = String::from_utf8_lossy(&line);
    let parts: Vec<&str> = text.split_whitespace().collect();
    match parts.as_slice() {
        ["UNKNOWN", ..] => Ok(None),
        ["TCP4" | "TCP6", source, _, port, _] => {
            let ip: IpAddr = source
                .parse()
                .map_err(|_| ProxyError::Invalid(format!("PROXY v1 source {source} is not an address")))?;
            Ok(Some(SocketAddr::new(ip, port.parse().unwrap_or(0))))
        }
        _ => Err(invalid("a PROXY v1 header that does not parse")),
    }
}

/// A v2 PROXY header for a TCP connection from `source` to `destination`. A pair of mixed families is sent as IPv6.
pub(crate) fn v2(source: SocketAddr, destination: SocketAddr) -> Vec<u8> {
    let mut header = V2_SIGNATURE.to_vec();
    // version 2, PROXY command
    header.push(0x21);
    match (source.ip(), destination.ip()) {
        (IpAddr::V4(from), IpAddr::V4(to)) => {
            // TCP over IPv4
            header.extend_from_slice(&[0x11, 0, 12]);
            header.extend_from_slice(&from.octets());
            header.extend_from_slice(&to.octets());
        }
        (from, to) => {
            let six = |ip: IpAddr| match ip {
                IpAddr::V4(v4) => v4.to_ipv6_mapped(),
                IpAddr::V6(v6) => v6,
            };
            // TCP over IPv6
            header.extend_from_slice(&[0x21, 0, 36]);
            header.extend_from_slice(&six(from).octets());
            header.extend_from_slice(&six(to).octets());
        }
    }
    header.extend_from_slice(&source.port().to_be_bytes());
    header.extend_from_slice(&destination.port().to_be_bytes());
    header
}

#[cfg(test)]
#[path = "proxy_header_tests.rs"]
mod tests;
