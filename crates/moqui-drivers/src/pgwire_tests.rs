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

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

use super::{tag_rows, Endpoint, Wire};

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// One backend message: its tag, then its length (which counts itself) and body.
fn message(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    out.extend_from_slice(&(u32::try_from(body.len() + 4).unwrap()).to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// A frontend message after startup: its tag and body.
fn read_message(stream: &mut impl Read) -> (u8, Vec<u8>) {
    let mut head = [0u8; 5];
    stream.read_exact(&mut head).unwrap();
    let len = u32::from_be_bytes([head[1], head[2], head[3], head[4]]) as usize;
    let mut body = vec![0u8; len - 4];
    stream.read_exact(&mut body).unwrap();
    (head[0], body)
}

/// A server that takes the client through `SSLRequest` and a TLS handshake, trusts it, and answers one query.
#[test]
fn a_tls_connection_negotiates_starts_and_answers_a_query() {
    let issued = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let cert = issued.cert.der().clone();
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(issued.signing_key.serialize_der().into());
    let server_config = rustls::ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert.clone()], key)
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut tcp, _) = listener.accept().unwrap();
        let mut request = [0u8; 8];
        tcp.read_exact(&mut request).unwrap();
        assert_eq!(request, [0, 0, 0, 8, 4, 210, 22, 47], "SSLRequest");
        tcp.write_all(b"S").unwrap();
        let connection = rustls::ServerConnection::new(Arc::new(server_config)).unwrap();
        let mut tls = rustls::StreamOwned::new(connection, tcp);
        let mut len = [0u8; 4];
        tls.read_exact(&mut len).unwrap();
        let mut startup = vec![0u8; u32::from_be_bytes(len) as usize - 4];
        tls.read_exact(&mut startup).unwrap();
        assert!(
            String::from_utf8_lossy(&startup).contains("moqui"),
            "the database in the startup"
        );
        let mut reply = message(b'R', &0_i32.to_be_bytes());
        reply.extend(message(b'K', &[0, 0, 0, 7, 0, 0, 0, 9]));
        reply.extend(message(b'Z', b"I"));
        tls.write_all(&reply).unwrap();
        let (tag, body) = read_message(&mut tls);
        assert_eq!(tag, b'Q');
        assert!(body.starts_with(b"SELECT 1"));
        let mut reply = message(b'C', b"SELECT 1\0");
        reply.extend(message(b'Z', b"I"));
        tls.write_all(&reply).unwrap();
        let (tag, _) = read_message(&mut tls);
        assert_eq!(tag, b'X', "Terminate");
    });
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert).unwrap();
    let client_config = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let endpoint = Endpoint::Tcp {
        host: "localhost".to_string(),
        port,
        tls: Some(Arc::new(client_config)),
    };
    let mut wire = Wire::connect(&endpoint, "moqui", "moqui", None, None).unwrap();
    assert_eq!(wire.cancel_key(), (7, 9));
    wire.simple("SELECT 1", "query").unwrap();
    drop(wire);
    server.join().unwrap();
}

/// The server's side of a session that sends `noise` bytes of notices before it reads the client's query, as a
/// server does that cannot send the replies of requests sent before: the query, of as many bytes, goes out while
/// the notices are read. Without that, each side waits on the other's write.
fn notices_then_query(stream: &mut (impl Read + Write), noise: usize) {
    let mut len = [0u8; 4];
    stream.read_exact(&mut len).unwrap();
    let mut startup = vec![0u8; u32::from_be_bytes(len) as usize - 4];
    stream.read_exact(&mut startup).unwrap();
    let mut reply = message(b'R', &0_i32.to_be_bytes());
    reply.extend(message(b'K', &[0, 0, 0, 7, 0, 0, 0, 9]));
    reply.extend(message(b'Z', b"I"));
    stream.write_all(&reply).unwrap();
    let mut body = b"SNOTICE\0VNOTICE\0C00000\0M".to_vec();
    body.extend(std::iter::repeat_n(b'n', 1000));
    body.extend(b"\0\0");
    let notice = message(b'N', &body);
    let mut notices = Vec::with_capacity(noise + notice.len());
    while notices.len() < noise {
        notices.extend_from_slice(&notice);
    }
    stream.write_all(&notices).unwrap();
    let (tag, query) = read_message(stream);
    assert_eq!(tag, b'Q');
    assert_eq!(query.len(), noise + 1, "the query and its NUL");
    let mut reply = message(b'C', b"SELECT 1\0");
    reply.extend(message(b'Z', b"I"));
    stream.write_all(&reply).unwrap();
    let (tag, _) = read_message(stream);
    assert_eq!(tag, b'X', "Terminate");
}

/// The client's side: a query of `noise` bytes, which must finish (a deadlock would hang, so it is waited for).
fn send_big_query(endpoint: Endpoint, noise: usize) {
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut wire = Wire::connect(&endpoint, "moqui", "moqui", None, None).unwrap();
        let sql = "x".repeat(noise);
        let result = wire.simple(&sql, "query").map_err(|err| err.to_string());
        drop(wire);
        let _ = done.send(result);
    });
    let result = finished
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the query finished: the client read the notices while its write waited");
    result.unwrap();
}

/// Over a Unix socket, whose buffers are 8 KB each way on macOS.
#[cfg(unix)]
#[test]
fn a_write_reads_what_the_server_sends_while_it_waits() {
    let dir = std::env::temp_dir().join(format!("pgwire-{}-{}", std::process::id(), line!()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(".s.PGSQL.5432");
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    let noise = 2 * 1024 * 1024;
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        notices_then_query(&mut stream, noise);
    });
    send_big_query(Endpoint::Unix(path), noise);
    server.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Over TLS, where the records waiting are read through the connection, not the stream that is writing.
#[test]
fn a_tls_write_reads_what_the_server_sends_while_it_waits() {
    let issued = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let cert = issued.cert.der().clone();
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(issued.signing_key.serialize_der().into());
    let server_config = rustls::ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert.clone()], key)
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    // More than loopback TCP buffers hold, so the server is still writing when the client writes.
    let noise = 16 * 1024 * 1024;
    let server = std::thread::spawn(move || {
        let (mut tcp, _) = listener.accept().unwrap();
        let mut request = [0u8; 8];
        tcp.read_exact(&mut request).unwrap();
        tcp.write_all(b"S").unwrap();
        let connection = rustls::ServerConnection::new(Arc::new(server_config)).unwrap();
        let mut tls = rustls::StreamOwned::new(connection, tcp);
        notices_then_query(&mut tls, noise);
    });
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert).unwrap();
    let client_config = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let endpoint = Endpoint::Tcp {
        host: "localhost".to_string(),
        port,
        tls: Some(Arc::new(client_config)),
    };
    send_big_query(endpoint, noise);
    server.join().unwrap();
}

/// A server for `connections` TLS sessions in turn with `issued` as its certificate: each takes the startup and
/// waits for the client to go. A session whose handshake the client refuses ends there.
fn tls_server(issued: &rcgen::CertifiedKey<rcgen::KeyPair>, connections: usize) -> (u16, std::thread::JoinHandle<()>) {
    let cert = issued.cert.der().clone();
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(issued.signing_key.serialize_der().into());
    let server_config = Arc::new(
        rustls::ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .unwrap(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        for _ in 0..connections {
            let (mut tcp, _) = listener.accept().unwrap();
            let mut request = [0u8; 8];
            tcp.read_exact(&mut request).unwrap();
            tcp.write_all(b"S").unwrap();
            let connection = rustls::ServerConnection::new(Arc::clone(&server_config)).unwrap();
            let mut tls = rustls::StreamOwned::new(connection, tcp);
            let mut len = [0u8; 4];
            if tls.read_exact(&mut len).is_err() {
                continue;
            }
            let mut startup = vec![0u8; u32::from_be_bytes(len) as usize - 4];
            tls.read_exact(&mut startup).unwrap();
            let mut reply = message(b'R', &0_i32.to_be_bytes());
            reply.extend(message(b'K', &[0, 0, 0, 7, 0, 0, 0, 9]));
            reply.extend(message(b'Z', b"I"));
            tls.write_all(&reply).unwrap();
            let mut rest = Vec::new();
            let _ = tls.read_to_end(&mut rest);
        }
    });
    (port, server)
}

fn tls_connect(host: &str, port: u16, options: &crate::pg_tls::TlsOptions) -> Result<Wire, moqui_lib::error::Error> {
    let endpoint = Endpoint::Tcp {
        host: host.to_string(),
        port,
        tls: Some(crate::pg_tls::client_config(options)?),
    };
    Wire::connect(&endpoint, "moqui", "moqui", None, None)
}

/// `sslmode` and `sslrootcert` against a server whose certificate is for `localhost` and signed by no public
/// authority, as a managed database with its own is.
#[test]
fn tls_modes_check_what_sslmode_says() {
    use crate::pg_tls::TlsOptions;
    use moqui_core::entity::datasource::TlsVerify;

    let issued = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let dir = std::env::temp_dir().join(format!("pg-tls-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.join("root.pem");
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, issued.cert.der());
    let lines: Vec<&str> = encoded
        .as_bytes()
        .chunks(64)
        .map(|line| std::str::from_utf8(line).unwrap())
        .collect();
    let pem = format!(
        "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
        lines.join("\n")
    );
    std::fs::write(&root, pem).unwrap();
    let root_text = root.display().to_string();
    let (port, server) = tls_server(&issued, 6);

    // The public authorities do not know it.
    let err = tls_connect("localhost", port, &TlsOptions::default()).err().unwrap();
    assert!(err.message().contains("TLS handshake with localhost"), "{err}");
    assert!(err.message().contains("UnknownIssuer"), "{err}");
    // With the root file, verify-full passes for the name in the certificate and refuses another.
    let full = TlsOptions::new(TlsVerify::Full, &root_text);
    drop(tls_connect("localhost", port, &full).unwrap());
    let err = tls_connect("127.0.0.1", port, &full).err().unwrap();
    assert!(err.message().contains("TLS handshake with 127.0.0.1"), "{err}");
    // verify-ca checks the authority and not the name.
    drop(tls_connect("127.0.0.1", port, &TlsOptions::new(TlsVerify::Ca, &root_text)).unwrap());
    // require encrypts without checking; with a root file it is verify-ca.
    drop(tls_connect("127.0.0.1", port, &TlsOptions::new(TlsVerify::Encrypt, "")).unwrap());
    drop(tls_connect("127.0.0.1", port, &TlsOptions::new(TlsVerify::Encrypt, &root_text)).unwrap());
    server.join().unwrap();

    let missing = TlsOptions::new(TlsVerify::Full, &dir.join("missing.pem").display().to_string());
    let err = crate::pg_tls::client_config(&missing).err().unwrap();
    assert!(err.message().contains("missing.pem"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_server_that_refuses_tls_is_an_error() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut tcp, _): (TcpStream, _) = listener.accept().unwrap();
        let mut request = [0u8; 8];
        tcp.read_exact(&mut request).unwrap();
        tcp.write_all(b"N").unwrap();
    });
    let config = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    let endpoint = Endpoint::Tcp {
        host: "localhost".to_string(),
        port,
        tls: Some(Arc::new(config)),
    };
    let err = Wire::connect(&endpoint, "moqui", "moqui", None, None).err().unwrap();
    assert!(err.message().contains("does not accept TLS"), "{err}");
    server.join().unwrap();
}

/// An idle connection reads as closed once the server sends `FATAL` and closes, as a restart does; a parameter
/// status that arrives while idle does not close it.
#[test]
fn an_idle_connection_the_server_closed_reads_as_closed() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (sent, sent_rx) = std::sync::mpsc::channel::<()>();
    let (go, go_rx) = std::sync::mpsc::channel::<()>();
    let server = std::thread::spawn(move || {
        let (mut tcp, _) = listener.accept().unwrap();
        let mut len = [0u8; 4];
        tcp.read_exact(&mut len).unwrap();
        let mut startup = vec![0u8; u32::from_be_bytes(len) as usize - 4];
        tcp.read_exact(&mut startup).unwrap();
        let mut reply = message(b'R', &0_i32.to_be_bytes());
        reply.extend(message(b'K', &[0, 0, 0, 7, 0, 0, 0, 9]));
        reply.extend(message(b'Z', b"I"));
        tcp.write_all(&reply).unwrap();
        go_rx.recv().unwrap();
        tcp.write_all(&message(b'S', b"TimeZone\0UTC\0")).unwrap();
        sent.send(()).unwrap();
        go_rx.recv().unwrap();
        tcp.write_all(&message(b'E', b"SFATAL\0C57P01\0Mterminating connection\0\0"))
            .unwrap();
        drop(tcp);
        sent.send(()).unwrap();
    });
    let endpoint = Endpoint::Tcp {
        host: "127.0.0.1".to_string(),
        port,
        tls: None,
    };
    let wire = Wire::connect(&endpoint, "moqui", "moqui", None, None).unwrap();
    std::thread::sleep(super::QUIET_BEFORE_PEEK);
    assert!(!wire.closed_while_idle(), "open and idle");
    go.send(()).unwrap();
    sent_rx.recv().unwrap();
    assert!(!wire.closed_while_idle(), "a parameter status leaves it open");
    go.send(()).unwrap();
    sent_rx.recv().unwrap();
    server.join().unwrap();
    // A completed send does not mean the peer's TCP stack has delivered the
    // final packet yet. Give the nonblocking probe a bounded delivery window.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while !wire.closed_while_idle() {
        assert!(std::time::Instant::now() < deadline, "closed by the server");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn a_command_tag_counts_its_rows() {
    assert_eq!(tag_rows("INSERT 0 5"), 5);
    assert_eq!(tag_rows("UPDATE 3"), 3);
    assert_eq!(tag_rows("SELECT 4"), 4);
    assert_eq!(tag_rows("BEGIN"), 0);
}
