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

#[test]
fn port_rules() {
    assert_eq!(listen_port("", true).unwrap(), Some(8443));
    assert_eq!(listen_port("", false).unwrap(), None);
    assert_eq!(listen_port("0", false).unwrap(), Some(0));
    assert_eq!(listen_port(" 9443 ", false).unwrap(), Some(9443));
    assert_eq!(listen_port("off", true).unwrap(), None);
    assert!(listen_port("-1", true).is_err());
    assert!(listen_port("abc", true).is_err());
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
    ensure_dev_cert(&dir).unwrap();
    let first = std::fs::read(dir.join("localhost.crt")).unwrap();
    ensure_dev_cert(&dir).unwrap();
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
    ensure_dev_cert(&dir).unwrap();
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
    let connector = client_trusting(&dir.join("localhost.crt"));
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
