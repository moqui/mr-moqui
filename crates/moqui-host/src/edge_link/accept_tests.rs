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

use super::*;

/// A client's ClientHello to `name` offering `alpn`, read by the edge node's reader on the other end.
async fn hello_from_client(name: &'static str, alpn: &[u8]) -> std::io::Result<(rustls::server::Accepted, Vec<u8>)> {
    let (client_side, mut server_side) = tokio::io::duplex(1 << 16);
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    config.alpn_protocols = vec![alpn.to_vec()];
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let server_name = rustls::pki_types::ServerName::try_from(name).unwrap();
    // the handshake never finishes: only its first flight is read
    let client = tokio::spawn(async move {
        let _ = connector.connect(server_name, client_side).await;
    });
    let read = read_hello(&mut server_side).await;
    client.abort();
    read
}

#[tokio::test]
async fn the_client_hello_is_read_whole_and_its_bytes_kept() {
    let (accepted, seen) = hello_from_client("pass.example.com", b"http/1.1").await.unwrap();
    let hello = accepted.client_hello();
    assert_eq!(hello.server_name(), Some("pass.example.com"));
    assert!(hello.alpn().unwrap().any(|protocol| protocol == b"http/1.1"));
    // a TLS handshake record, kept from its first byte for a passthrough tunnel to replay
    assert_eq!(seen.first(), Some(&0x16));
    let record = usize::from(u16::from_be_bytes([seen[3], seen[4]]));
    assert!(
        seen.len() >= 5 + record,
        "{} bytes for a {record}-byte record",
        seen.len()
    );
}

#[tokio::test]
async fn bytes_that_are_not_tls_or_end_early_are_refused() {
    let (mut client, mut server) = tokio::io::duplex(1 << 16);
    client.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
    let err = read_hello(&mut server).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

    let (mut client, mut server) = tokio::io::duplex(1 << 16);
    client.write_all(&[0x16, 0x03, 0x01, 0x02, 0x00, 0x01]).await.unwrap();
    drop(client);
    let err = read_hello(&mut server).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
}

#[tokio::test]
async fn a_client_hello_that_never_ends_is_refused() {
    // handshake records of 16 KiB each that never end the ClientHello: the reader stops, at rustls's own buffer limit,
    // which comes before the reader's cap (MAX_HELLO, the backstop)
    let (mut client, mut server) = tokio::io::duplex(1 << 20);
    let writer = tokio::spawn(async move {
        let mut message = vec![0x01, 0x00, 0xFF, 0xFF];
        message.resize(16 * 1024, 0);
        let mut first = vec![0x16, 0x03, 0x01, 0x40, 0x00];
        first.extend_from_slice(&message);
        let _ = client.write_all(&first).await;
        let mut more = vec![0x16, 0x03, 0x01, 0x40, 0x00];
        more.resize(5 + 16 * 1024, 0);
        for _ in 0..8 {
            if client.write_all(&more).await.is_err() {
                break;
            }
        }
        client
    });
    let err = read_hello(&mut server).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    drop(writer.await);
}

/// Pair attempts count by /64 and start again after the window; sources whose window ended are swept once a window,
/// so the table does not keep them (HL-032).
#[test]
fn pair_attempts_count_by_source_in_a_window() {
    let mut attempts = PairAttempts::default();
    let window = Duration::from_secs(600);
    let t0 = Instant::now();
    let a: IpAddr = "2001:db8:7:7::1".parse().unwrap();
    let a2: IpAddr = "2001:db8:7:7::2".parse().unwrap();
    assert!(attempts.allowed(a, t0, window, 2));
    assert!(attempts.allowed(a2, t0, window, 2));
    assert!(!attempts.allowed(a, t0, window, 2), "the third from the /64");
    let v4: IpAddr = "198.51.100.9".parse().unwrap();
    let mapped: IpAddr = "::ffff:198.51.100.9".parse().unwrap();
    assert!(attempts.allowed(v4, t0, window, 1));
    assert!(
        !attempts.allowed(mapped, t0, window, 1),
        "an IPv4-mapped address is its IPv4 one"
    );
    assert_eq!(attempts.sources(), 2);
    let later = t0 + window + Duration::from_secs(1);
    assert!(attempts.allowed(a, later, window, 2), "a new window");
    assert_eq!(attempts.sources(), 1, "the expired source was swept");
}
