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

use std::net::SocketAddr;

use super::*;

async fn parse(bytes: &[u8]) -> (Result<Option<SocketAddr>, ProxyError>, Vec<u8>) {
    let mut reader = bytes;
    let read = read(&mut reader).await;
    (read, reader.to_vec())
}

#[tokio::test]
async fn a_written_v2_header_reads_back_and_leaves_what_follows() {
    let source: SocketAddr = "203.0.113.9:40000".parse().unwrap();
    let destination: SocketAddr = "192.0.2.1:443".parse().unwrap();
    let mut bytes = v2(source, destination);
    assert_eq!(bytes.len(), 16 + 12);
    bytes.extend_from_slice(b"\x16\x03\x01hello");
    let (read, rest) = parse(&bytes).await;
    assert_eq!(read, Ok(Some(source)));
    assert_eq!(rest, b"\x16\x03\x01hello");

    let six: SocketAddr = "[2001:db8::7]:5000".parse().unwrap();
    let (read, rest) = parse(&v2(six, destination)).await;
    assert_eq!(read, Ok(Some(six)));
    assert!(rest.is_empty());
}

#[tokio::test]
async fn v1_local_and_bad_headers() {
    let (read, rest) = parse(b"PROXY TCP4 192.0.2.10 127.0.0.1 40000 25\r\nEHLO").await;
    assert_eq!(read, Ok(Some("192.0.2.10:40000".parse().unwrap())));
    assert_eq!(rest, b"EHLO");
    assert_eq!(parse(b"PROXY UNKNOWN\r\n").await.0, Ok(None));
    let mut local = V2_SIGNATURE.to_vec();
    local.extend_from_slice(&[0x20, 0x00, 0, 0]);
    assert_eq!(parse(&local).await.0, Ok(None));
    assert!(matches!(
        parse(b"GET / HTTP/1.1\r\n").await.0,
        Err(ProxyError::Invalid(_))
    ));
    assert!(matches!(
        parse(b"\x16\x03\x01\x02\x00\x01\x00\x01").await.0,
        Err(ProxyError::Invalid(_))
    ));
    let long = format!("PROXY TCP4 {}\r\n", "1".repeat(200));
    assert!(matches!(parse(long.as_bytes()).await.0, Err(ProxyError::Invalid(_))));
    let mut short = V2_SIGNATURE.to_vec();
    short.extend_from_slice(&[0x21, 0x11, 0, 4, 1, 2, 3, 4]);
    assert!(matches!(parse(&short).await.0, Err(ProxyError::Invalid(_))));
    assert_eq!(parse(b"PROXY TC").await.0, Err(ProxyError::Closed));
}
