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

use std::sync::{Arc, Weak};

use super::challenge::{certificate, parse_digest, Challenge};
use super::order::{account_key, due};
use super::store::{self, NameRow};
use super::{accept_with, Acme};

const DAY: i64 = 24 * 60 * 60 * 1000;

/// The acmeIdentifier extension (1.3.6.1.5.5.7.1.31), critical, holding the digest as an OCTET STRING (RFC 8737 §3).
fn acme_identifier(digest: &[u8; 32]) -> Vec<u8> {
    let mut expected = vec![
        0x06, 0x08, 0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x01, 0x1f, 0x01, 0x01, 0xff, 0x04, 0x22, 0x04, 0x20,
    ];
    expected.extend_from_slice(digest);
    expected
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

#[test]
fn the_challenge_certificate_names_the_host_and_carries_the_digest_critically() {
    let digest = [7_u8; 32];
    let (cert, _) = certificate("shop.example.com", &digest).unwrap();
    assert!(contains(cert.as_ref(), &acme_identifier(&digest)));
    assert!(contains(cert.as_ref(), b"shop.example.com"));
    let hex = crate::tls_pin::hex(&digest);
    assert_eq!(parse_digest(&hex), Some(digest));
    assert_eq!(parse_digest("xyz"), None);
    assert_eq!(parse_digest(&hex[..62]), None);
}

/// A listener with no certificate at all: a handshake for any name is refused, and one that offers `acme-tls/1` is
/// answered only for a name with a challenge open, with that challenge's digest.
#[tokio::test]
async fn the_listener_answers_acme_tls_only_for_an_open_challenge() {
    let dir = crate::tls::tests::temp_dir("acme-listener").join("cert");
    std::fs::create_dir_all(&dir).unwrap();
    let http = Arc::new(crate::tls::server_config(crate::tls::CertStore::new(dir.clone())).unwrap());
    let acme = Arc::new(Acme::default());
    let digest = [42_u8; 32];
    acme.insert(
        "open.example.com",
        Challenge::new(digest, moqui_core::user::now_millis() + 60_000),
    );
    acme.insert(
        "gone.example.com",
        Challenge::new([1_u8; 32], moqui_core::user::now_millis() - 1),
    );
    assert!(
        acme.config("open.example.com").is_some(),
        "the open challenge's settings"
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let served = Arc::clone(&acme);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let (http, acme) = (Arc::clone(&http), Arc::clone(&served));
            tokio::spawn(async move {
                // no connection is ever served: the store has no certificate, and a challenge ends at its handshake
                assert!(accept_with(stream, http, &Weak::new(), Some(acme)).await.is_err());
            });
        }
    });
    let answered = super::check::served(address, "open.example.com").await.unwrap();
    assert!(
        contains(&answered, &acme_identifier(&digest)),
        "the open challenge's digest"
    );
    for refused in ["gone.example.com", "other.example.com"] {
        assert!(
            super::check::served(address, refused).await.is_err(),
            "{refused} has no challenge open"
        );
    }
    // a browser, for the name whose challenge is open: no certificate, so no HTTPS
    let mut browser =
        rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(super::check::AnyCertificate(Arc::new(
                rustls::crypto::aws_lc_rs::default_provider(),
            ))))
            .with_no_client_auth();
    browser.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let tcp = tokio::net::TcpStream::connect(address).await.unwrap();
    let name = rustls::pki_types::ServerName::try_from("open.example.com").unwrap();
    assert!(tokio_rustls::TlsConnector::from(Arc::new(browser))
        .connect(name, tcp)
        .await
        .is_err());
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

fn name(host: &str) -> NameRow {
    NameRow {
        host: host.to_string(),
        renewing: true,
        status: store::ISSUED.to_string(),
        ..NameRow::default()
    }
}

/// The job renews a name set up and renewing whose certificate is missing or near its end, and leaves the rest.
#[test]
fn the_renewal_job_picks_only_set_up_names_near_expiry() {
    let now = 1_800_000_000_000;
    let ninety = |issued_days_ago: i64| NameRow {
        issued: Some(now - issued_days_ago * DAY),
        not_after: Some(now - issued_days_ago * DAY + 90 * DAY),
        ..name("x")
    };
    let rows = vec![
        NameRow {
            host: "fresh.example.com".into(),
            ..ninety(10)
        },
        NameRow {
            host: "old.example.com".into(),
            ..ninety(61)
        },
        NameRow {
            host: "stopped.example.com".into(),
            renewing: false,
            ..ninety(80)
        },
        NameRow {
            host: "first.example.com".into(),
            status: store::FAILED.into(),
            last_attempt: Some(now - 7 * 60 * 60 * 1000),
            ..name("x")
        },
        NameRow {
            host: "just-failed.example.com".into(),
            status: store::FAILED.into(),
            last_attempt: Some(now - 60 * 1000),
            ..ninety(85)
        },
        NameRow {
            host: "ordering.example.com".into(),
            status: store::ORDERING.into(),
            last_attempt: Some(now - 60 * 1000),
            ..name("x")
        },
        NameRow {
            host: "cut-off.example.com".into(),
            status: store::ORDERING.into(),
            last_attempt: Some(now - 7 * 60 * 60 * 1000),
            ..name("x")
        },
        // its lock lapsed a minute ago: it counts as just failed
        NameRow {
            host: "cut-off-now.example.com".into(),
            status: store::ORDERING.into(),
            last_attempt: Some(now - 16 * 60 * 1000),
            ..name("x")
        },
        NameRow {
            host: "short.example.com".into(),
            issued: Some(now - 5 * DAY),
            not_after: Some(now + DAY),
            ..name("x")
        },
        NameRow {
            host: "undated.example.com".into(),
            not_after: Some(now + 45 * DAY),
            ..name("x")
        },
    ];
    // the one ordering holds its lock; the cut-off ones' locks lapsed
    let locked = std::collections::HashSet::from(["ordering.example.com".to_string()]);
    assert_eq!(
        due(&rows, &locked, now),
        vec![
            "old.example.com",
            "first.example.com",
            "cut-off.example.com",
            "short.example.com"
        ]
    );
}

#[test]
fn the_account_key_is_the_same_for_the_same_seed() {
    use base64::Engine as _;
    let seed = base64::engine::general_purpose::STANDARD.encode([3_u8; 32]);
    let other = base64::engine::general_purpose::STANDARD.encode([4_u8; 32]);
    let (_, first) = account_key(&seed).unwrap();
    let (_, again) = account_key(&seed).unwrap();
    let (_, different) = account_key(&other).unwrap();
    assert_eq!(first.secret_der(), again.secret_der());
    assert_ne!(first.secret_der(), different.secret_der());
    assert!(account_key("not base64!").is_err());
    assert!(account_key("c2hvcnQ=").is_err(), "too short");
}

/// An issued certificate is written in certbot's layout, which the listener serves; another tool's directory is
/// never written or removed, and a name's files go with it.
#[test]
fn the_certificate_is_written_where_the_listener_reads_it() {
    let dir = crate::tls::tests::temp_dir("acme-files").join("cert");
    std::fs::create_dir_all(&dir).unwrap();
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["www.example.com".to_string()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let chain = crate::tls::pem("CERTIFICATE", cert.der());
    let key_pem = crate::tls::pem("PRIVATE KEY", &key.serialize_der());
    store::write_files(&dir, "www.example.com", &chain, &key_pem).unwrap();
    let served = crate::tls::served_for(&dir, "www.example.com").unwrap();
    assert_eq!(served.pair, "www.example.com");
    assert!(served.problem.is_none(), "{:?}", served.problem);
    assert!(served.cert.ends_with("www.example.com/fullchain.pem"));
    // written again unchanged: nothing to do
    store::write_files(&dir, "www.example.com", &chain, &key_pem).unwrap();

    std::fs::create_dir_all(dir.join("certbot.example.com")).unwrap();
    assert!(store::foreign_dir(&dir, "certbot.example.com"));
    assert!(store::write_files(&dir, "certbot.example.com", &chain, &key_pem).is_err());
    store::remove_files(&dir, "certbot.example.com").unwrap();
    assert!(
        dir.join("certbot.example.com").is_dir(),
        "another tool's directory stays"
    );

    store::remove_files(&dir, "www.example.com").unwrap();
    assert!(crate::tls::served_for(&dir, "www.example.com").is_none());
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

#[test]
fn a_host_name_is_a_dns_name_an_authority_issues_for() {
    use super::services::valid_host;
    for good in ["www.example.com", "a-b.example.co.uk", "xn--bcher-kva.example"] {
        assert!(valid_host(good), "{good}");
    }
    for bad in [
        "localhost",
        "*.example.com",
        "192.168.1.10",
        "-a.example.com",
        "a..example.com",
        "exa mple.com",
        "",
    ] {
        assert!(!valid_host(bad), "{bad}");
    }
}
