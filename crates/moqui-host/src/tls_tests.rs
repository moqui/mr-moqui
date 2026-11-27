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

pub(crate) fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("moqui-tls-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A pair for `name`, written as `<name>.crt/.key` in nginx-proxy style.
fn write_pair(dir: &Path, name: &str) {
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec![name.to_string()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    std::fs::write(dir.join(format!("{name}.crt")), pem("CERTIFICATE", cert.der())).unwrap();
    std::fs::write(
        dir.join(format!("{name}.key")),
        pem("PRIVATE KEY", &key.serialize_der()),
    )
    .unwrap();
}

/// A rustls client that trusts only the certificate in `cert`, and offers no ALPN.
pub(crate) fn client_trusting(cert: &Path) -> tokio_rustls::TlsConnector {
    client_offering(cert, &[])
}

/// A rustls client that trusts only the certificate in `cert` and offers the ALPN protocols `alpn`.
pub(crate) fn client_offering(cert: &Path, alpn: &[&[u8]]) -> tokio_rustls::TlsConnector {
    let mut roots = rustls::RootCertStore::empty();
    for cert in CertificateDer::pem_file_iter(cert).unwrap() {
        roots.add(cert.unwrap()).unwrap();
    }
    let mut client = rustls::ClientConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    client.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
    tokio_rustls::TlsConnector::from(Arc::new(client))
}

/// The listener is on by its switch, at its written port (plans/Runtime-Config.md §3.2): off keeps it closed whatever
/// the port says; on, the port is digits, 0 for one of the kernel's choosing, and anything else is refused.
#[test]
fn port_rules() {
    let listen = |enabled: &str, port: &str| {
        Listen::of(&|name: &str| match name {
            LISTEN_ENABLED => enabled.to_string(),
            LISTEN_PORT => port.to_string(),
            _ => String::new(),
        })
    };
    assert_eq!(listen("true", "8443").port().unwrap(), Some(8443));
    assert_eq!(listen(" TRUE ", " 9443 ").port().unwrap(), Some(9443));
    assert_eq!(listen("true", "0").port().unwrap(), Some(0));
    assert_eq!(listen("false", "8443").port().unwrap(), None);
    assert_eq!(listen("", "8443").port().unwrap(), None, "a switch not written is off");
    assert_eq!(listen("false", "off").port().unwrap(), None);
    for refused in ["", "off", "-1", "abc"] {
        let err = listen("true", refused).port().unwrap_err().to_string();
        assert!(err.contains("https-listen-enabled"), "{err}");
    }
    // the port check requires the port only while the listener will bind it at the start: on, with a certificate or
    // on a dev instance, which makes one; on with none elsewhere it stands by, binding nothing until ACME
    let dir = temp_dir("planned");
    assert_eq!(planned_port(&dir, &listen("false", "8443")).unwrap(), None);
    assert_eq!(planned_port_on(&dir, &listen("false", "8443"), true).unwrap(), None);
    assert_eq!(planned_port_on(&dir, &listen("true", "8443"), false).unwrap(), None);
    assert_eq!(
        planned_port_on(&dir, &listen("true", "8443"), true).unwrap(),
        Some(8443)
    );
    std::fs::create_dir_all(dir.join("cert")).unwrap();
    write_pair(&dir.join("cert"), "localhost");
    assert_eq!(
        planned_port_on(&dir, &listen("true", "9443"), false).unwrap(),
        Some(9443)
    );
    assert_eq!(planned_port_on(&dir, &listen("false", "9443"), false).unwrap(), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn exact_wildcard_parent_and_default_lookup() {
    let dir = temp_dir("lookup");
    write_pair(&dir, "shop.example.com");
    write_pair(&dir, "_wildcard.example.com");
    write_pair(&dir, "example.org");
    let store = CertStore::new(dir.clone());
    assert!(store.lookup(Some("shop.example.com")).is_some());
    assert!(store.lookup(Some("SHOP.example.com.")).is_some());
    assert!(store.lookup(Some("other.example.com")).is_some(), "mkcert wildcard");
    assert!(store.lookup(Some("www.example.org")).is_some(), "nginx-proxy parent");
    assert!(
        store.lookup(Some("a.b.example.com")).is_none(),
        "a wildcard covers one label"
    );
    assert!(store.lookup(Some("example.net")).is_none());
    assert!(store.lookup(None).is_none());
    write_pair(&dir, "default");
    let store = CertStore::new(dir.clone());
    assert!(store.lookup(Some("example.net")).is_some());
    assert!(store.lookup(None).is_some());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn names_a_client_makes_up_add_nothing() {
    let dir = temp_dir("made-up");
    write_pair(&dir, "a.test");
    let store = CertStore::new(dir.clone());
    for n in 0..100 {
        assert!(store.lookup(Some(&format!("x{n}.random.test"))).is_none());
    }
    assert_eq!(store.index().pairs.len(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn certbot_and_mkcert_layouts_and_dhparam_ignored() {
    let dir = temp_dir("layouts");
    std::fs::write(dir.join("dhparam.pem"), "not a cert").unwrap();
    std::fs::write(dir.join("a.example.dhparam.pem"), "not a cert").unwrap();
    assert!(!has_any_pair(&dir));
    write_pair(&dir, "tmp.test");
    let sub = dir.join("live.test");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::rename(dir.join("tmp.test.crt"), sub.join("fullchain.pem")).unwrap();
    std::fs::rename(dir.join("tmp.test.key"), sub.join("privkey.pem")).unwrap();
    assert!(has_any_pair(&dir));
    write_pair(&dir, "mk.test");
    std::fs::rename(dir.join("mk.test.crt"), dir.join("mk.test.pem")).unwrap();
    std::fs::rename(dir.join("mk.test.key"), dir.join("mk.test-key.pem")).unwrap();
    let store = CertStore::new(dir.clone());
    assert!(store.lookup(Some("live.test")).is_some());
    assert!(store.lookup(Some("mk.test")).is_some());
    assert!(store.lookup(Some("tmp.test")).is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_renewal_that_does_not_load_keeps_the_previous_key() {
    let dir = temp_dir("renewal");
    write_pair(&dir, "a.test");
    let first = scan(&dir, None);
    assert!(first.pairs["a.test"].loaded.is_some());
    std::fs::write(dir.join("a.test.crt"), "half written").unwrap();
    let second = scan(&dir, Some(&first));
    assert!(second.pairs["a.test"].loaded.is_some());
    assert!(scan(&dir, None).pairs["a.test"].loaded.is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dev_cert_is_generated_once_and_loads() {
    let dir = temp_dir("dev").join("cert");
    ensure_dev_cert_with(&dir, None).unwrap();
    let first = std::fs::read(dir.join("localhost.crt")).unwrap();
    ensure_dev_cert_with(&dir, None).unwrap();
    assert_eq!(first, std::fs::read(dir.join("localhost.crt")).unwrap());
    let store = CertStore::new(dir.clone());
    assert!(store.lookup(Some("localhost")).is_some());
    assert!(store.lookup(None).is_some());
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

#[test]
fn mismatched_key_is_not_served() {
    let dir = temp_dir("mismatch");
    write_pair(&dir, "a.test");
    write_pair(&dir, "b.test");
    std::fs::copy(dir.join("b.test.key"), dir.join("a.test.key")).unwrap();
    let store = CertStore::new(dir.clone());
    assert!(store.lookup(Some("a.test")).is_none());
    let _ = std::fs::remove_dir_all(dir);
}

/// A client that trusts only the generated dev certificate gets through for `localhost` and is refused for a
/// domain with no certificate.
#[tokio::test]
async fn handshake_serves_known_domain_and_refuses_unknown() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let dir = temp_dir("handshake").join("cert");
    ensure_dev_cert_with(&dir, None).unwrap();
    // Only `localhost`: drop the default so an unknown name has nothing to fall back to.
    std::fs::remove_file(dir.join("default.crt")).unwrap();
    std::fs::remove_file(dir.join("default.key")).unwrap();
    let config = Arc::new(server_config(CertStore::new(dir.clone())).unwrap());
    let acceptor = tokio_rustls::TlsAcceptor::from(config);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(mut tls) = acceptor.accept(stream).await {
                    let _ = tls.write_all(b"hello").await;
                    let _ = tls.shutdown().await;
                }
            });
        }
    });
    let connector = client_trusting(&dir.join("dev-ca/ca.crt"));
    let connect = |name: &'static str| {
        let connector = connector.clone();
        async move {
            let stream = tokio::net::TcpStream::connect(address).await.unwrap();
            connector
                .connect(rustls::pki_types::ServerName::try_from(name).unwrap(), stream)
                .await
        }
    };
    let mut tls = connect("localhost").await.expect("localhost handshake");
    let mut text = String::new();
    tls.read_to_string(&mut text).await.ok();
    assert_eq!(text, "hello");
    assert!(connect("nope.test").await.is_err(), "no certificate for nope.test");
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

/// The resource readings' certificates: each pair's leaf with the day it expires, a file that is not a certificate
/// left out.
#[test]
fn the_expiry_of_each_pair_is_read_from_its_certificate() {
    let dir = temp_dir("expiry");
    let key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(vec!["soon.example.com".to_string()]).unwrap();
    params.not_after = rcgen::date_time_ymd(2031, 3, 9);
    let cert = params.self_signed(&key).unwrap();
    std::fs::write(dir.join("soon.example.com.crt"), pem("CERTIFICATE", cert.der())).unwrap();
    std::fs::write(
        dir.join("soon.example.com.key"),
        pem("PRIVATE KEY", &key.serialize_der()),
    )
    .unwrap();
    std::fs::write(
        dir.join("broken.example.com.crt"),
        pem("CERTIFICATE", b"not a certificate"),
    )
    .unwrap();
    std::fs::write(dir.join("broken.example.com.key"), "not a key").unwrap();

    let found = certificate_expiries(&dir);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].name, "soon.example.com");
    // 2031-03-09 00:00:00 UTC
    assert_eq!(found[0].not_after, 1_930_780_800);
    assert!(certificate_expiries(&dir.join("none")).is_empty());
}

/// The security proof of OWASP ASVS 5.0.0 v5.0.0-12.1.1 (docs/security/proofs/V12-Secure-Communication.md), in-process:
/// the HTTPS listener's own settings ([`server_config`]) speak TLS 1.3 to a client that offers it and TLS 1.2 to a
/// client that offers only 1.2. Nothing older can be offered: rustls implements no version before 1.2.
#[test]
fn the_https_listener_speaks_tls_13_and_12_only_preferring_13() {
    let dir = temp_dir("versions");
    write_pair(&dir, "localhost");
    let server = Arc::new(server_config(CertStore::new(dir.clone())).unwrap());
    let handshake = |versions: &[&'static rustls::SupportedProtocolVersion]| {
        let mut roots = rustls::RootCertStore::empty();
        for cert in CertificateDer::pem_file_iter(dir.join("localhost.crt")).unwrap() {
            roots.add(cert.unwrap()).unwrap();
        }
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
            .with_protocol_versions(versions)
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
        let mut client = rustls::ClientConnection::new(Arc::new(config), name).unwrap();
        let mut listener = rustls::ServerConnection::new(server.clone()).unwrap();
        while client.is_handshaking() || listener.is_handshaking() {
            let mut sent = Vec::new();
            client.write_tls(&mut sent).unwrap();
            listener.read_tls(&mut sent.as_slice()).unwrap();
            listener.process_new_packets().unwrap();
            let mut answered = Vec::new();
            listener.write_tls(&mut answered).unwrap();
            client.read_tls(&mut answered.as_slice()).unwrap();
            client.process_new_packets().unwrap();
        }
        (client.protocol_version(), listener.protocol_version())
    };
    let tls13 = Some(rustls::ProtocolVersion::TLSv1_3);
    let tls12 = Some(rustls::ProtocolVersion::TLSv1_2);
    assert_eq!(
        handshake(rustls::ALL_VERSIONS),
        (tls13, tls13),
        "a client offering both gets 1.3"
    );
    assert_eq!(
        handshake(&[&rustls::version::TLS12]),
        (tls12, tls12),
        "a client offering 1.2 alone gets it"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// The security proof of OWASP ASVS 5.0.0 v5.0.0-11.2.1 (docs/security/proofs/V11-Cryptography.md), in-process: the
/// cryptography is the validated libraries', not the framework's own. The HTTPS listener's TLS runs on aws-lc-rs's
/// provider, its cipher suites and key exchange groups exactly that provider's; the other primitives are the RustCrypto
/// crates (`aes`, `cbc`, `des`, `md-5`, `sha1`, `sha2`, `hmac`, `hkdf`, `pbkdf2`, `aes-gcm`, `chacha20poly1305`,
/// `p256`, `rsa`), `argon2`, `getrandom`, and `rustls` and `rcgen` on `aws-lc-rs`, each in Cargo.lock from crates.io;
/// and no source file of the workspace or its components holds the constants a hand-made SHA-1, SHA-2, MD5, AES or
/// ChaCha would need (their round constants, initial values or S-box). aws-lc-rs is built without its FIPS feature, so
/// this is not a FIPS 140-3 validated module, and `rsa` (0.10, constant time) signs with blinding. How each primitive
/// is used is not judged here (the field encryption's weaknesses are V11's Gap rows and the `known_gap_` tests).
#[test]
fn cryptography_comes_from_the_validated_libraries_and_none_is_home_made() {
    let dir = temp_dir("provider");
    write_pair(&dir, "localhost");
    let server = server_config(CertStore::new(dir.clone())).unwrap();
    let provider = server.crypto_provider();
    let validated = aws_lc_rs::default_provider();
    let suites = |provider: &rustls::crypto::CryptoProvider| {
        provider
            .cipher_suites
            .iter()
            .map(|suite| suite.suite())
            .collect::<Vec<_>>()
    };
    let groups = |provider: &rustls::crypto::CryptoProvider| {
        provider.kx_groups.iter().map(|group| group.name()).collect::<Vec<_>>()
    };
    assert_eq!(suites(provider), suites(&validated));
    assert_eq!(groups(provider), groups(&validated));
    let _ = std::fs::remove_dir_all(dir);

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).unwrap();
    let packages: Vec<(String, String)> = lock
        .split("[[package]]")
        .filter_map(|block| {
            let field = |key: &str| {
                block
                    .lines()
                    .find_map(|line| line.strip_prefix(&format!("{key} = \"")))
                    .map(|rest| rest.trim_end_matches('"').to_string())
            };
            Some((field("name")?, field("source").unwrap_or_default()))
        })
        .collect();
    for name in [
        "aws-lc-rs",
        "aws-lc-sys",
        "rustls",
        "rcgen",
        "aes",
        "cbc",
        "des",
        "md-5",
        "sha1",
        "sha2",
        "hmac",
        "hkdf",
        "pbkdf2",
        "aes-gcm",
        "chacha20poly1305",
        "p256",
        "rsa",
        "argon2",
        "getrandom",
    ] {
        let sources: Vec<&String> = packages
            .iter()
            .filter(|(package, _)| package == name)
            .map(|(_, source)| source)
            .collect();
        assert!(!sources.is_empty(), "{name} is not in Cargo.lock");
        assert!(
            sources
                .iter()
                .all(|source| source.as_str() == "registry+https://github.com/rust-lang/crates.io-index"),
            "{name} comes from {sources:?}"
        );
    }

    // SHA-256's first round constant (the high half of SHA-512's) and initial value, SHA-1's round constant, MD5's
    // first table value, the SHA-1 and MD5 initial value, the AES S-box's first bytes, and ChaCha's constant; in two
    // halves, so this file does not hold them
    let marks: Vec<String> = [
        ["428a", "2f98"],
        ["6a09", "e667"],
        ["5a82", "7999"],
        ["d76a", "a478"],
        ["6745", "2301"],
        ["0x63,0x7c,", "0x77,0x7b"],
        ["expand32-", "bytek"],
    ]
    .iter()
    .map(|halves| halves.concat())
    .collect();
    let mut walk = vec![root.join("crates"), root.join("runtime/components")];
    let mut read = 0;
    while let Some(dir) = walk.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if path.is_dir() {
                if !matches!(name.as_str(), "target" | "node_modules" | ".git") {
                    walk.push(path);
                }
            } else if name.ends_with(".rs") {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let flat: String = text
                    .to_ascii_lowercase()
                    .chars()
                    .filter(|ch| *ch != '_' && *ch != ' ')
                    .collect();
                for mark in &marks {
                    assert!(!flat.contains(mark.as_str()), "{} holds {mark}", path.display());
                }
                read += 1;
            }
        }
    }
    assert!(read > 1000, "the sources were read: {read}");
}

/// The security proof of OWASP ASVS 5.0.0 v5.0.0-12.3.2 (docs/security/proofs/V12-Secure-Communication.md), in-process,
/// of a Configuration row: the framework's outbound HTTP client (`moqui_lib::http::RestClient`, under the remote
/// services, `https://` locations, webhooks, the OAuth sign-ins and the OTLP export) checks the server's certificate
/// before it sends anything. Against a server whose certificate no public authority signed it refuses the connection
/// and the request never reaches the server; trusting that certificate (`root_certificates_pem`, as a deployment with a
/// private authority does) it takes it for the name it was issued to and refuses it for another, 127.0.0.1. A Postgres
/// datasource's checks are `tls_modes_check_what_sslmode_says` (moqui-drivers `pgwire_tests.rs`; no `sslmode` is
/// verify-full). Not checked unless the deployer says so: an MX's STARTTLS certificate, unless
/// `mail-delivery@mx-require-tls` names the domain or its MTA-STS policy enforces, and a datasource whose `sslmode` is
/// `require` (no check) or `verify-ca` (any name).
#[test]
fn the_outbound_http_client_checks_the_servers_certificate_and_name() {
    use std::io::{Read as _, Write as _};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["localhost".to_string()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let config = Arc::new(
        rustls::ServerConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(cert.der().to_vec())],
                PrivateKeyDer::Pkcs8(key.serialize_der().into()),
            )
            .unwrap(),
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let served = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&served);
    std::thread::spawn(move || {
        for tcp in listener.incoming() {
            let Ok(tcp) = tcp else { return };
            let _ = tcp.set_read_timeout(Some(Duration::from_secs(5)));
            let Ok(connection) = rustls::ServerConnection::new(Arc::clone(&config)) else {
                continue;
            };
            let mut tls = rustls::StreamOwned::new(connection, tcp);
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            let whole = loop {
                match tls.read(&mut buffer) {
                    Ok(0) | Err(_) => break false,
                    Ok(read) => {
                        request.extend_from_slice(&buffer[..read]);
                        if request.windows(4).any(|end| end == b"\r\n\r\n") {
                            break true;
                        }
                    }
                }
            };
            if whole {
                counted.fetch_add(1, Ordering::SeqCst);
                let _ = tls.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 7\r\nconnection: close\r\n\r\nchecked");
                tls.conn.send_close_notify();
                let _ = tls.flush();
            }
        }
    });
    let url = |host: &str| format!("https://{host}:{port}/proof");

    let public_roots = moqui_lib::http::RestClient::new();
    assert!(
        public_roots.get(url("localhost")).send(false).is_err(),
        "no public authority signed it"
    );
    assert_eq!(served.load(Ordering::SeqCst), 0, "nothing was sent to it");

    let trusting =
        moqui_lib::http::RestClient::new().root_certificates_pem(pem("CERTIFICATE", cert.der()).into_bytes());
    let response = trusting
        .get(url("localhost"))
        .send(false)
        .expect("the name it was issued to");
    assert_eq!((response.status(), response.body()), (200, "checked"));
    assert_eq!(served.load(Ordering::SeqCst), 1);
    assert!(
        trusting.get(url("127.0.0.1")).send(false).is_err(),
        "a name it was not issued to"
    );
    assert_eq!(served.load(Ordering::SeqCst), 1);
}

/// A TLS 1.2 ClientHello record for `localhost` offering `suites` (no `supported_versions`, so TLS 1.2), with the EC
/// groups, point formats and signature schemes a client sends.
fn tls12_client_hello(suites: &[u16]) -> Vec<u8> {
    let mut extensions = Vec::new();
    let mut extension = |kind: u16, data: &[u8]| {
        extensions.extend_from_slice(&kind.to_be_bytes());
        extensions.extend_from_slice(&(data.len() as u16).to_be_bytes());
        extensions.extend_from_slice(data);
    };
    let name = b"localhost";
    let mut server_name = ((name.len() + 3) as u16).to_be_bytes().to_vec();
    server_name.push(0);
    server_name.extend_from_slice(&(name.len() as u16).to_be_bytes());
    server_name.extend_from_slice(name);
    extension(0x0000, &server_name);
    extension(0x000a, &[0x00, 0x04, 0x00, 0x1d, 0x00, 0x17]);
    extension(0x000b, &[0x01, 0x00]);
    extension(0x000d, &[0x00, 0x06, 0x04, 0x03, 0x08, 0x04, 0x04, 0x01]);
    extension(0x0017, &[]);
    let mut body = vec![0x03, 0x03];
    body.extend_from_slice(&[0x42; 32]);
    body.push(0); // no session id
    body.extend_from_slice(&((suites.len() * 2) as u16).to_be_bytes());
    for suite in suites {
        body.extend_from_slice(&suite.to_be_bytes());
    }
    body.extend_from_slice(&[1, 0]);
    body.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
    body.extend_from_slice(&extensions);
    let length = (body.len() as u32).to_be_bytes();
    let mut handshake = vec![1, length[1], length[2], length[3]];
    handshake.extend_from_slice(&body);
    let mut record = vec![22, 0x03, 0x01];
    record.extend_from_slice(&(handshake.len() as u16).to_be_bytes());
    record.extend_from_slice(&handshake);
    record
}

/// The security proof of OWASP ASVS 5.0.0 v5.0.0-12.1.2 (docs/security/proofs/V12-Secure-Communication.md), in-process:
/// the HTTPS listener (`server_config`) enables only strong cipher suites. Every suite it has is an AEAD (AES-GCM or
/// ChaCha20-Poly1305) with an ephemeral key exchange, TLS 1.3's or ECDHE in TLS 1.2, so forward secret; a client that
/// offers only suites without forward secrecy (RSA key exchange) or with CBC, 3DES, RC4 or a null cipher gets no
/// handshake (no cipher suite in common), and one that offers those and a strong one gets the strong one. TLS 1.3 is
/// preferred over 1.2 (`the_https_listener_speaks_tls_13_and_12_only_preferring_13`). Among the strong suites the
/// server's order holds (`ignore_client_order`, SEC-WEB-88): a client that lists AES-128 before AES-256 gets AES-256.
#[test]
fn the_https_listener_enables_only_aead_suites_with_forward_secrecy() {
    let dir = temp_dir("suites");
    write_pair(&dir, "localhost");
    let config = Arc::new(server_config(CertStore::new(dir.clone())).unwrap());
    let suites = &config.crypto_provider().cipher_suites;
    assert!(!suites.is_empty());
    for suite in suites {
        let name = format!("{:?}", suite.suite());
        match suite {
            rustls::SupportedCipherSuite::Tls13(_) => {
                assert!(name.contains("GCM") || name.contains("CHACHA20_POLY1305"), "{name}")
            }
            rustls::SupportedCipherSuite::Tls12(_) => assert!(
                name.starts_with("TLS_ECDHE_") && (name.contains("_GCM_") || name.contains("CHACHA20_POLY1305")),
                "{name}"
            ),
        }
    }

    let answer = |offered: &[u16]| {
        let mut server = rustls::ServerConnection::new(Arc::clone(&config)).unwrap();
        server.read_tls(&mut tls12_client_hello(offered).as_slice()).unwrap();
        match server.process_new_packets() {
            Err(err) => Err(err),
            Ok(_) => {
                let mut sent = Vec::new();
                server.write_tls(&mut sent).unwrap();
                assert_eq!((sent[0], sent[5]), (22, 2), "a ServerHello");
                let at = 9 + 2 + 32;
                let at = at + 1 + usize::from(sent[at]);
                Ok(u16::from_be_bytes([sent[at], sent[at + 1]]))
            }
        }
    };
    // RSA key exchange (CBC, GCM, 3DES, RC4), ECDHE with CBC, ECDHE with no cipher, and finite-field DHE
    let weak = [0x002f, 0x009d, 0x000a, 0x0005, 0xc023, 0xc006, 0x009e];
    assert_eq!(
        answer(&weak).unwrap_err(),
        rustls::Error::PeerIncompatible(rustls::PeerIncompatible::NoCipherSuitesInCommon)
    );
    let mut offered = weak.to_vec();
    offered.push(0xc02b);
    assert_eq!(
        answer(&offered).unwrap(),
        0xc02b,
        "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256"
    );
    assert!(config.ignore_client_order);
    assert_eq!(
        answer(&[0xc02b, 0xc02c]).unwrap(),
        0xc02c,
        "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384 though the client listed AES-128 first"
    );
    let _ = std::fs::remove_dir_all(dir);
}
