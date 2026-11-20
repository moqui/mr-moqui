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

#[test]
fn an_endpoint_is_a_grpc_url_or_the_edge_link() {
    assert_eq!(parse_endpoint("edge:").unwrap(), Endpoint::Edge);
    assert_eq!(
        parse_endpoint("grpcs://authority.example.com:4443").unwrap(),
        Endpoint::Direct {
            host: "authority.example.com".to_string(),
            port: 4443,
            tls: true
        }
    );
    assert_eq!(
        parse_endpoint(" grpc://127.0.0.1/ ").unwrap(),
        Endpoint::Direct {
            host: "127.0.0.1".to_string(),
            port: 4317,
            tls: false
        }
    );
    assert_eq!(
        parse_endpoint("grpc://[::1]:9000").unwrap(),
        Endpoint::Direct {
            host: "::1".to_string(),
            port: 9000,
            tls: false
        }
    );
    for bad in ["https://authority.example.com", "edge", "grpc://", ""] {
        assert!(parse_endpoint(bad).is_err(), "{bad}");
    }
}

/// The client secret goes only to the authority the settings name, and in clear text only to loopback unless
/// `pass@backend-insecure` is true (SEC-OUT-83): a script's endpoint to another host gets nothing.
#[test]
fn the_client_secret_goes_only_to_the_configured_authority() {
    let to = |endpoint: &str| parse_endpoint(endpoint).unwrap();
    // the endpoint pass@backend-grpc-url names, its host in any case
    secret_may_go_to(
        &to("grpcs://Authority.example.com:4443"),
        "grpcs://authority.example.com:4443",
        "",
        false,
    )
    .unwrap();
    secret_may_go_to(&to("grpc://127.0.0.1:9000"), "grpc://127.0.0.1:9000", "", false).unwrap();
    // with none, pass@backend-url's host on port 4317
    secret_may_go_to(
        &to("grpcs://authority.example.com"),
        "",
        "https://authority.example.com/rest",
        false,
    )
    .unwrap();
    // another host, port or scheme is refused
    for endpoint in [
        "grpc://host.example:4317",
        "grpcs://authority.example.com:4444",
        "grpc://authority.example.com:4443",
    ] {
        let refused = secret_may_go_to(&to(endpoint), "grpcs://authority.example.com:4443", "", false).unwrap_err();
        assert!(refused.message().contains("goes only to"), "{endpoint}: {refused}");
    }
    assert!(secret_may_go_to(&to("grpc://host.example:4317"), "", "", false).is_err());
    // clear text to another host only when pass@backend-insecure says so
    let plain = to("grpc://authority.internal:4317");
    let refused = secret_may_go_to(&plain, "grpc://authority.internal:4317", "", false).unwrap_err();
    assert!(refused.message().contains("clear text"), "{refused}");
    secret_may_go_to(&plain, "grpc://authority.internal:4317", "", true).unwrap();
    secret_may_go_to(&plain, "", "http://authority.internal", true).unwrap();
    // the edge link carries no secret
    secret_may_go_to(&Endpoint::Edge, "", "", false).unwrap();
}

#[test]
fn a_download_reads_its_chunks_in_order_and_ends_or_fails_with_the_stream() {
    let (sender, receiver) = tokio::sync::mpsc::channel(4);
    sender.try_send(Ok(Bytes::from_static(b"hello "))).unwrap();
    sender.try_send(Ok(Bytes::from_static(b"world"))).unwrap();
    drop(sender);
    let mut reader = ChannelReader {
        receiver,
        current: Bytes::new(),
    };
    let mut text = String::new();
    reader.read_to_string(&mut text).unwrap();
    assert_eq!(text, "hello world");

    let (sender, receiver) = tokio::sync::mpsc::channel(4);
    sender.try_send(Ok(Bytes::from_static(b"part"))).unwrap();
    sender
        .try_send(Err("the PASS authority's download stalled".to_string()))
        .unwrap();
    let mut reader = ChannelReader {
        receiver,
        current: Bytes::new(),
    };
    let mut bytes = Vec::new();
    let err = reader.read_to_end(&mut bytes).unwrap_err();
    assert!(err.to_string().contains("stalled"), "{err}");
    assert_eq!(bytes, b"part");
}

/// The PASS part of the security proof of OWASP ASVS 5.0.0 v5.0.0-12.3.2 (mr-pass docs/ASVS-Matrix.md), in-process, of
/// a Configuration row: the frontend's gRPC client of its authority (`grpcs://`, `ec.service.passAuthority`) checks the
/// authority's certificate before it sends anything. Against an authority whose certificate no public authority
/// signed, a call is refused at the TLS handshake, by name or by address, and the server gets not one byte of
/// HTTP/2, so neither the call nor the client secret (request metadata, after the connection) ever leaves. The client
/// trusts only the WebPKI roots, so no test can hold a certificate it takes; the control is the same transport
/// (`grpc_client::dial`) with the same client settings plus the test's root: it takes the certificate for the name it
/// was issued to, and refuses it for another (127.0.0.1). The REST transport (`pass@backend-url` over https) is
/// `RestClient`, proved by `the_outbound_http_client_checks_the_servers_certificate_and_name` (tls_tests.rs). Clear text
/// (`grpc://`, `http://`) to another host is refused by mr-pass's Transport unless `pass@backend-insecure` is true
/// (mr-pass `test/Transport.spec.js`, "a transport in clear text to another host is refused unless
/// pass@backend-insecure says so"), and by this client too (`secret_may_go_to`), which sends the secret only to the
/// authority the settings name.
#[test]
fn the_authority_client_checks_the_authoritys_certificate_and_name() {
    use std::io::Write as _;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["localhost".to_string()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let mut server_config =
        rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![rustls::pki_types::CertificateDer::from(cert.der().to_vec())],
                rustls::pki_types::PrivateKeyDer::Pkcs8(key.serialize_der().into()),
            )
            .unwrap();
    server_config.alpn_protocols = vec![b"h2".to_vec()];
    let server_config = Arc::new(server_config);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    // the bytes of HTTP/2 each connection sent after its handshake
    let received = Arc::new(Mutex::new(Vec::new()));
    let connections = Arc::new(AtomicUsize::new(0));
    let (kept, counted) = (Arc::clone(&received), Arc::clone(&connections));
    std::thread::spawn(move || {
        for tcp in listener.incoming() {
            let Ok(tcp) = tcp else { return };
            counted.fetch_add(1, Ordering::SeqCst);
            let _ = tcp.set_read_timeout(Some(Duration::from_secs(5)));
            let Ok(connection) = rustls::ServerConnection::new(Arc::clone(&server_config)) else {
                continue;
            };
            let mut tls = rustls::StreamOwned::new(connection, tcp);
            let mut buffer = [0_u8; 4096];
            let read = tls.read(&mut buffer).unwrap_or(0);
            lock(&kept).push(buffer[..read].to_vec());
            if read > 0 {
                // an empty SETTINGS frame, the server's side of the connection preface
                let _ = tls.write_all(&[0, 0, 0, 4, 0, 0, 0, 0, 0]);
                let _ = tls.flush();
            }
        }
    });

    let sender = Sender::default();
    for host in ["localhost", "127.0.0.1"] {
        let err = sender
            .call(&format!("grpcs://{host}:{port}"), Call::default())
            .expect_err("no public authority signed it");
        let err = err.to_string();
        assert!(
            err.contains("TLS with the PASS authority") && err.contains("UnknownIssuer"),
            "{host}: {err}"
        );
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while lock(&received).len() < 2 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(*lock(&received), vec![Vec::<u8>::new(); 2], "nothing was sent");

    let trusting = |host: &'static str| {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        roots
            .add(rustls::pki_types::CertificateDer::from(cert.der().to_vec()))
            .unwrap();
        let mut config =
            rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(roots)
                .with_no_client_auth();
        config.alpn_protocols = vec![b"h2".to_vec()];
        let name = rustls::pki_types::ServerName::try_from(host).unwrap();
        crate::edge::block_on(async move {
            crate::grpc_client::dial(crate::grpc_client::Dial {
                what: "the PASS authority",
                host,
                port,
                tls: Some((Arc::new(config), name)),
                timeout: Duration::from_secs(5),
                keep_alive: Duration::from_secs(30),
                keep_alive_timeout: Duration::from_secs(20),
            })
            .await
            .map(|_| ())
            .map_err(|err| err.to_string())
        })
        .unwrap()
    };
    let wrong_name = trusting("127.0.0.1").expect_err("a name it was not issued to");
    assert!(
        wrong_name.contains("TLS with the PASS authority") && wrong_name.contains("not valid for name"),
        "{wrong_name}"
    );
    assert_eq!(trusting("localhost"), Ok(()), "the name it was issued to");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while lock(&received).len() < 4 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let received = lock(&received).clone();
    assert_eq!(received.len(), 4, "{received:?}");
    assert!(received[2].is_empty(), "nothing was sent past the refused name");
    assert!(
        received[3].starts_with(b"PRI * HTTP/2.0"),
        "the control's connection preface"
    );
}
