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

//! Certificates by ACME (plans/Edge-Node2.md §4.8, E15), on real servers in the test's own process.
//!
//! A node with no certificate and no name set up has no HTTPS listener and asks no authority for anything. A name set
//! up on System › Certificates starts the listener, which answers `acme-tls/1` only for a name with a challenge open
//! (with its digest) and refuses every other handshake; the order goes to the authority (a stand-in that only takes
//! the connection, so it fails) and its failure shows on the System home. A file put in `runtime/cert` shows on the
//! screen beside the name set up. Through an edge node, a local node's passthrough name's challenge goes down the
//! tunnel to the local node, which answers it, while the edge node answers its own terminate host's.
//!
//! Real issuance (Let's Encrypt staging, then production) is a hand check (plans/Edge-Node2.md §2.1).

#![cfg(all(unix, feature = "embedded-postgres"))]

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_lib::object::{to_plain_string, Value};

use crate::common::Gate;

const SERVICES: &str = "org.moqui.admin.CertificateServices";

/// A service on `gate`'s server as john.doe, with authorization off: its answer and the errors it added.
fn call(gate: &Gate, name: &str, parameters: &[(&str, Value)]) -> (BTreeMap<String, Value>, Vec<String>) {
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let _authz = context.disable_authz();
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(name, value)| ((*name).to_string(), value.clone()))
        .collect();
    let out = moqui_core::service::sync(name, &parameters).unwrap_or_else(|err| panic!("{name}: {err}"));
    let errors = moqui_core::context::ExecutionContext::with_current(|ec| {
        ec.message().errors().iter().map(ToString::to_string).collect()
    })
    .unwrap_or_default();
    (out, errors)
}

/// An open ACME challenge for `host` on `gate`, with `digest`, as an order writes it.
fn open_challenge(gate: &Gate, host: &str, digest: &[u8; 32]) {
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let facade = factory.entity().unwrap();
    let mut row = facade.make_value("moqui.server.AcmeChallenge").unwrap();
    row.set("hostName", Value::string(host));
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    row.set("keyAuthDigest", Value::string(hex));
    row.set(
        "expireDate",
        moqui_core::user::stamp_value(moqui_core::user::now_millis() + 600_000),
    );
    row.create_or_update(&facade).unwrap();
}

/// A field of the row of `entity` keyed by `key`, or None when there is no row.
fn field(gate: &Gate, entity: &str, key: (&str, &str), name: &str) -> Option<String> {
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    factory
        .entity()
        .unwrap()
        .find(entity)
        .unwrap()
        .eq(key.0, Value::string(key.1))
        .use_cache(false)
        .one()
        .unwrap()
        .map(|row| row.get(name).map(to_plain_string).unwrap_or_default())
}

/// Close `host`'s challenge on `gate`, as an order's end does.
fn close_challenge(gate: &Gate, host: &str) {
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let facade = factory.entity().unwrap();
    if let Some(row) = facade
        .find("moqui.server.AcmeChallenge")
        .unwrap()
        .eq("hostName", Value::string(host))
        .one()
        .unwrap()
    {
        row.delete(&facade).unwrap();
    }
}

fn name_status(gate: &Gate, host: &str) -> String {
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    factory
        .entity()
        .unwrap()
        .find("moqui.server.AcmeName")
        .unwrap()
        .eq("hostName", Value::string(host))
        .one()
        .unwrap()
        .and_then(|row| row.get("statusId").map(to_plain_string))
        .unwrap_or_default()
}

fn wait_until(limit: Duration, what: &str, check: impl Fn() -> bool) {
    let started = Instant::now();
    while !check() {
        assert!(started.elapsed() < limit, "{what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// The client takes any certificate: the tests look for the challenge's digest in it.
#[derive(Debug)]
pub(crate) struct AnyCertificate;

impl rustls::client::danger::ServerCertVerifier for AnyCertificate {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::aws_lc_rs::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// The leaf certificate `address` serves a handshake for `host` offering `alpn`, or the handshake's error.
fn served(rt: &tokio::runtime::Runtime, address: SocketAddr, host: &str, alpn: &[u8]) -> Result<Vec<u8>, String> {
    let mut config =
        rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AnyCertificate))
            .with_no_client_auth();
    config.alpn_protocols = vec![alpn.to_vec()];
    let name = rustls::pki_types::ServerName::try_from(host.to_string()).unwrap();
    rt.block_on(async move {
        let tcp = tokio::net::TcpStream::connect(address)
            .await
            .map_err(|err| err.to_string())?;
        let tls = tokio::time::timeout(
            Duration::from_secs(5),
            tokio_rustls::TlsConnector::from(Arc::new(config)).connect(name, tcp),
        )
        .await
        .map_err(|_| "timed out".to_string())?
        .map_err(|err| err.to_string())?;
        tls.get_ref()
            .1
            .peer_certificates()
            .and_then(|chain| chain.first())
            .map(|leaf| leaf.as_ref().to_vec())
            .ok_or_else(|| "no certificate".to_string())
    })
}

fn carries(cert: &[u8], digest: &[u8; 32]) -> bool {
    cert.windows(digest.len()).any(|window| window == digest)
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

/// A stand-in for the authority's directory: it takes each connection and closes it, counting them.
fn stand_in_authority() -> (u16, Arc<AtomicUsize>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let count = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&count);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            counted.fetch_add(1, Ordering::SeqCst);
            drop(stream);
        }
    });
    (port, count)
}

#[test]
fn https_starts_for_a_name_set_up_with_acme_and_answers_only_its_challenge() {
    if !crate::common::own_process() {
        return;
    }
    let https_port = crate::common::free_port();
    let (authority_port, asked) = stand_in_authority();
    let directory = format!("https://127.0.0.1:{authority_port}/directory");
    let https_port_text = https_port.to_string();
    let gate = crate::common::start_with_conf_properties(
        "acme",
        None,
        "",
        &[
            ("webapp_https_listen_port", &https_port_text),
            ("acme_directory_url", &directory),
        ],
    );
    let https = SocketAddr::from(([127, 0, 0, 1], https_port));
    let rt = runtime();

    // no certificate and no name set up: no listener, and nobody is asked for anything
    assert!(gate.server.https_address().is_none());
    std::thread::sleep(Duration::from_millis(300));
    assert!(std::net::TcpStream::connect(https).is_err(), "no HTTPS listener");
    assert_eq!(asked.load(Ordering::SeqCst), 0, "no request to the authority");

    // a name that is not a DNS name, and terms not agreed, are refused
    let (out, errors) = call(
        &gate,
        &format!("{SERVICES}.setup#AcmeName"),
        &[
            ("hostName", Value::string("not a name")),
            ("termsAgreed", Value::Bool(true)),
        ],
    );
    assert_eq!(to_plain_string(&out["reasonCode"]), "badName", "{errors:?}");
    let (out, _) = call(
        &gate,
        &format!("{SERVICES}.setup#AcmeName"),
        &[("hostName", Value::string("acme.example.test"))],
    );
    assert_eq!(to_plain_string(&out["reasonCode"]), "termsNotAgreed");
    // the other refusals: an authority that is not https, a contact that is not an address, another tool's directory,
    // a name whose DNS has no address (the check), and renewing a name not set up
    let refused = |parameters: &[(&str, Value)]| {
        let mut all = vec![("termsAgreed", Value::Bool(true))];
        all.extend_from_slice(parameters);
        let (out, errors) = call(&gate, &format!("{SERVICES}.setup#AcmeName"), &all);
        assert!(!errors.is_empty());
        to_plain_string(&out["reasonCode"])
    };
    let name = ("hostName", Value::string("acme.example.test"));
    let skip = ("skipCheck", Value::Bool(true));
    assert_eq!(
        refused(&[
            name.clone(),
            skip.clone(),
            ("directoryUrl", Value::string("http://ca.test/dir"))
        ]),
        "badDirectory"
    );
    assert_eq!(
        refused(&[
            name.clone(),
            skip.clone(),
            ("contactEmail", Value::string("not an address"))
        ]),
        "badContact"
    );
    std::fs::create_dir_all(gate.runtime.join("cert/certbot.example.test")).unwrap();
    assert_eq!(
        refused(&[("hostName", Value::string("certbot.example.test")), skip.clone()]),
        "otherTool"
    );
    assert_eq!(
        refused(&[("hostName", Value::string("nowhere.invalid"))]),
        "checkFailed"
    );
    let (out, _) = call(
        &gate,
        &format!("{SERVICES}.renew#AcmeName"),
        &[("hostName", Value::string("never.example.test"))],
    );
    assert_eq!(to_plain_string(&out["reasonCode"]), "notSetUp");
    assert!(std::net::TcpStream::connect(https).is_err(), "still no HTTPS listener");
    assert_eq!(asked.load(Ordering::SeqCst), 0, "still no request to the authority");

    // set up: the listener starts, and the order goes to the authority
    let (out, errors) = call(
        &gate,
        &format!("{SERVICES}.setup#AcmeName"),
        &[
            ("hostName", Value::string("ACME.example.test")),
            ("termsAgreed", Value::Bool(true)),
            ("skipCheck", Value::Bool(true)),
        ],
    );
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(to_plain_string(&out["hostName"]), "acme.example.test");
    wait_until(Duration::from_secs(5), "the HTTPS listener", || {
        std::net::TcpStream::connect(https).is_ok()
    });
    wait_until(
        Duration::from_secs(10),
        "the order asks the authority and fails",
        || name_status(&gate, "acme.example.test") == "AcmeFailed",
    );
    assert!(asked.load(Ordering::SeqCst) >= 1);

    // the listener answers acme-tls/1 only for a name with a challenge open, and refuses every other handshake
    let digest = [0x5a_u8; 32];
    open_challenge(&gate, "acme.example.test", &digest);
    let answered = served(&rt, https, "acme.example.test", b"acme-tls/1").unwrap();
    assert!(carries(&answered, &digest), "the challenge's digest");
    assert!(served(&rt, https, "other.example.test", b"acme-tls/1").is_err());
    assert!(
        served(&rt, https, "acme.example.test", b"http/1.1").is_err(),
        "no certificate yet, so no HTTPS for the name"
    );

    // the failure shows on the System home
    let (attention, _) = call(&gate, "moqui.tools.SystemHomeServices.get#SystemAttention", &[]);
    let Some(Value::List(items)) = attention.get("items") else {
        panic!("{attention:?}")
    };
    let acme_item = items.iter().find_map(|item| match item {
        Value::Map(item) if item.get("key").map(to_plain_string).as_deref() == Some("acmeRenewal") => Some(item),
        _ => None,
    });
    let acme_item = acme_item.unwrap_or_else(|| panic!("{items:?}"));
    assert_eq!(to_plain_string(&acme_item["severity"]), "critical");
    assert!(to_plain_string(&acme_item["text"]).contains("acme.example.test"));

    // a file put in runtime/cert shows on the screen beside the name set up
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["files.example.test".to_string()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let pem = |label: &str, der: &[u8]| {
        use base64::Engine as _;
        let text = base64::engine::general_purpose::STANDARD.encode(der);
        let lines: Vec<&str> = text
            .as_bytes()
            .chunks(64)
            .map(|line| std::str::from_utf8(line).unwrap())
            .collect();
        format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n"))
    };
    let dir = gate.runtime.join("cert");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("files.example.test.crt"), pem("CERTIFICATE", cert.der())).unwrap();
    std::fs::write(
        dir.join("files.example.test.key"),
        pem("PRIVATE KEY", &key.serialize_der()),
    )
    .unwrap();
    let admin = crate::common::sign_in(gate.server.address(), "john.doe", "moqui");
    let request = crate::common::http_request("GET", "/System/Certificates", &[("Cookie", &admin)], "");
    let (status, _, page) = crate::common::exchange(gate.server.address(), &request);
    assert_eq!(status, 200, "{page}");
    for shown in [
        "files.example.test",
        "cert/files.example.test.crt",
        "acme.example.test",
        "Failed",
        "127.0.0.1 · renewed before it runs out",
        "Valid",
        "HTTPS for this name is refused",
        "What an ACME certificate is and is not",
    ] {
        assert!(page.contains(shown), "{shown}: {page}");
    }
}

/// An order against an authority that checks the challenge as a real one does: the certificate is stored and written
/// in certbot's layout, the listener serves it, and the renewal job orders it again when it is near its end.
#[test]
fn an_order_gets_a_certificate_the_listener_serves_and_the_job_renews_it() {
    if !crate::common::own_process() {
        return;
    }
    let rt = runtime();
    let https_port = crate::common::free_port();
    let https = SocketAddr::from(([127, 0, 0, 1], https_port));
    let authority = crate::acme_authority::start(&rt, https);
    let https_port_text = https_port.to_string();
    let gate = crate::common::start_with_conf_files(
        "acme-order",
        None,
        "",
        &[
            ("webapp_https_listen_port", &https_port_text),
            ("acme_directory_url", &authority.directory),
            ("acme_directory_ca", "acme-root.pem"),
        ],
        &[("acme-root.pem", &authority.root_pem)],
    );
    const HOST: &str = "www.acme.test";
    let (_, errors) = call(
        &gate,
        &format!("{SERVICES}.setup#AcmeName"),
        &[
            ("hostName", Value::string(HOST)),
            ("contactEmail", Value::string("ops@acme.test")),
            ("termsAgreed", Value::Bool(true)),
            ("skipCheck", Value::Bool(true)),
        ],
    );
    assert!(errors.is_empty(), "{errors:?}");
    wait_until(Duration::from_secs(10), "the certificate", || {
        let status = name_status(&gate, HOST);
        assert_ne!(status, "AcmeFailed", "the order failed");
        status == "AcmeIssued"
    });
    let dir = gate.runtime.join("cert").join(HOST);
    for file in ["fullchain.pem", "privkey.pem", ".moqui-acme"] {
        assert!(dir.join(file).is_file(), "{file}");
    }
    // the listener serves it once it reads the directory again
    wait_until(Duration::from_secs(8), "the listener serves the certificate", || {
        served(&rt, https, HOST, b"http/1.1").is_ok_and(|leaf| leaf.windows(HOST.len()).any(|w| w == HOST.as_bytes()))
    });
    assert_eq!(authority.issued.load(Ordering::SeqCst), 1);

    // it lasts until midnight, so the job renews it now
    let (out, errors) = call(&gate, &format!("{SERVICES}.renew#AcmeCertificates"), &[]);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(to_plain_string(&out["renewed"]), "1", "{out:?}");
    assert_eq!(to_plain_string(&out["failed"]), "0");
    assert_eq!(authority.orders.load(Ordering::SeqCst), 2);
    assert_eq!(authority.issued.load(Ordering::SeqCst), 2);
    assert_eq!(name_status(&gate, HOST), "AcmeIssued");

    // stopped, it is not renewed though it is due
    let (_, errors) = call(
        &gate,
        &format!("{SERVICES}.update#AcmeName"),
        &[("hostName", Value::string(HOST)), ("renewing", Value::Bool(false))],
    );
    assert!(errors.is_empty(), "{errors:?}");
    let (out, _) = call(&gate, &format!("{SERVICES}.renew#AcmeCertificates"), &[]);
    assert_eq!(to_plain_string(&out["renewed"]), "0");
    assert_eq!(authority.orders.load(Ordering::SeqCst), 2);

    // removed: its rows and files go, and the job pauses with the last name
    const JOB: (&str, &str) = ("jobName", "renew_AcmeCertificates_daily");
    assert_eq!(
        field(&gate, "moqui.service.job.ServiceJob", JOB, "paused").as_deref(),
        Some("N")
    );
    let (out, errors) = call(
        &gate,
        &format!("{SERVICES}.remove#AcmeName"),
        &[("hostName", Value::string(HOST))],
    );
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(out["removed"], Value::Bool(true));
    for entity in ["moqui.server.AcmeName", "moqui.server.AcmeCertificate"] {
        assert_eq!(field(&gate, entity, ("hostName", HOST), "hostName"), None, "{entity}");
    }
    assert!(!dir.exists(), "its files");
    assert_eq!(
        field(&gate, "moqui.service.job.ServiceJob", JOB, "paused").as_deref(),
        Some("Y")
    );
}

/// Two members of a cluster on one database: a name set up on one starts the other's challenge-only listener, and a
/// challenge one writes, changes or closes is what the other answers within a second, however many `acme-tls/1`
/// handshakes come in between.
#[test]
fn a_challenge_another_member_writes_or_closes_is_answered_as_it_is_now() {
    if !crate::common::own_process() {
        return;
    }
    const HOST: &str = "member.example.test";
    let gate = crate::common::start_with_properties("acme-cluster", None, &[]);
    {
        let factory = gate.server.factory();
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let facade = factory.entity().unwrap();
        let mut row = facade.make_value("moqui.server.AcmeName").unwrap();
        for (name, value) in [
            ("hostName", HOST),
            ("directoryUrl", "https://ca.example.test/dir"),
            ("statusId", "AcmeIssued"),
            ("renewing", "N"),
        ] {
            row.set(name, Value::string(value));
        }
        row.create(&facade).unwrap();
    }
    let port = crate::common::free_port();
    let port_text = port.to_string();
    let (_member, _member_runtime) = crate::common::start_member(
        "acme-member",
        &gate.cluster_dir,
        &[("webapp_https_listen_port", &port_text)],
    );
    let member = SocketAddr::from(([127, 0, 0, 1], port));
    wait_until(
        Duration::from_secs(5),
        "the member's listener for the name set up",
        || std::net::TcpStream::connect(member).is_ok(),
    );
    let rt = runtime();
    // acme-tls/1 handshakes for other names all along, which read the rows as often as they may
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let noise = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let rt = runtime();
            while !stop.load(Ordering::SeqCst) {
                let _ = served(&rt, member, "noise.example.test", b"acme-tls/1");
                std::thread::sleep(Duration::from_millis(50));
            }
        })
    };
    let seen = Duration::from_millis(1200);
    let first = [0x61_u8; 32];
    let second = [0x62_u8; 32];
    open_challenge(&gate, HOST, &first);
    std::thread::sleep(seen);
    assert!(carries(&served(&rt, member, HOST, b"acme-tls/1").unwrap(), &first));
    open_challenge(&gate, HOST, &second);
    std::thread::sleep(seen);
    let now = served(&rt, member, HOST, b"acme-tls/1").unwrap();
    assert!(
        carries(&now, &second) && !carries(&now, &first),
        "the changed challenge"
    );
    close_challenge(&gate, HOST);
    std::thread::sleep(seen);
    assert!(
        served(&rt, member, HOST, b"acme-tls/1").is_err(),
        "the closed challenge"
    );
    stop.store(true, Ordering::SeqCst);
    noise.join().unwrap();
}

/// A test certificate authority's certificate for `names` (PEM), with the root.
#[cfg(all(feature = "edge", feature = "ssh"))]
fn issued(names: &[&str]) -> (rustls::pki_types::CertificateDer<'static>, String, String) {
    let authority = crate::edge_web::authority();
    let (cert, key) = crate::edge_web::issue(&authority, names);
    (authority.root, cert, key)
}

#[cfg(all(feature = "edge", feature = "ssh"))]
#[test]
fn the_edge_node_passes_a_passthrough_names_challenge_down_the_tunnel() {
    use moqui_core::edge::LinkState;
    use moqui_host::edge_link::{local_web, Backoff, Dialer, LocalConfig};

    if !crate::common::own_process() {
        return;
    }
    const PASS: &str = "pass.localhost";
    const TERM: &str = "term.localhost";
    let (root, edge_cert, edge_key) = issued(&["localhost", TERM]);
    let local_port = crate::common::free_port().to_string();
    let edge_port = crate::common::free_port();
    // the local node has no certificate at all
    let local =
        crate::common::start_with_conf_properties("acme-local", None, "", &[("webapp_https_listen_port", &local_port)]);
    let edge_port_text = edge_port.to_string();
    let edge = crate::common::start_second_with(
        "acme-edge",
        "",
        &[("node_role", "front_edge"), ("webapp_https_listen_port", &edge_port_text)],
        &[("cert/default.crt", &edge_cert), ("cert/default.key", &edge_key)],
    );
    let rt = runtime();
    let url = format!("https://localhost:{edge_port}");
    let paired = crate::edge_web::pair_node(&rt, &edge, &url, "local-acme", &root);
    for (host, mode) in [(PASS, "EdgeHostPassthrough"), (TERM, "EdgeHostTerminate")] {
        crate::edge_web::call(
            &edge,
            "add#EdgeHost",
            &[("hostName", host), ("pairedNodeId", "local-acme"), ("modeEnumId", mode)],
        )
        .unwrap();
    }
    let mut config = LocalConfig::new(&url, &paired.key, &paired.pin, "local-acme");
    config.backoff = Backoff {
        first: Duration::from_millis(100),
        max: Duration::from_millis(500),
    };
    config.connect_timeout = Duration::from_secs(3);
    config.web = local_web(local.server.factory());
    let (_shutdown, shutdown) = tokio::sync::watch::channel(false);
    let dialer = Dialer::start(config, rt.handle(), shutdown, None).unwrap();
    crate::edge_web::wait_status(&dialer, Duration::from_secs(10), "link up", |status| {
        status.state == LinkState::Linked
    });
    wait_until(Duration::from_secs(5), "the down connection", || {
        crate::edge_web::down(&edge, "local-acme")
    });

    // the local node's challenge for its passthrough name; the edge node's own row for that name is never used
    let local_digest = [0x11_u8; 32];
    let edge_digest = [0x22_u8; 32];
    let term_digest = [0x33_u8; 32];
    open_challenge(&local, PASS, &local_digest);
    open_challenge(&edge, PASS, &edge_digest);
    open_challenge(&edge, TERM, &term_digest);
    let edge_https = SocketAddr::from(([127, 0, 0, 1], edge_port));
    let through = served(&rt, edge_https, PASS, b"acme-tls/1").unwrap();
    assert!(
        carries(&through, &local_digest),
        "answered by the local node, down the tunnel"
    );
    assert!(!carries(&through, &edge_digest));
    let own = served(&rt, edge_https, TERM, b"acme-tls/1").unwrap();
    assert!(
        carries(&own, &term_digest),
        "the edge node answers its terminate host itself"
    );
    assert!(
        served(&rt, edge_https, PASS, b"http/1.1").is_err(),
        "the local node has no certificate for the name"
    );
    drop(dialer);
}

/// System › Certificates on `gate` as john.doe: post `transition` with `fields`, and the errors it left.
#[cfg(all(feature = "edge", feature = "ssh"))]
fn screen_post(gate: &Gate, transition: &str, fields: &[(&str, &str)]) -> String {
    let factory = Arc::clone(gate.server.factory());
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let screens = moqui_core::screen::ScreenTest::new(&factory)
        .root_screen("component://tools/screen/System.xml")
        .render_mode("html");
    let parameters: BTreeMap<String, Value> = fields
        .iter()
        .map(|(name, value)| ((*name).to_string(), Value::string(*value)))
        .collect();
    let page = screens.test_render(&format!("Certificates/{transition}"), parameters, Some("post"));
    page.error_messages.join(" | ")
}

/// System › Certificates on `gate` as john.doe, rendered once in `mode`: its text.
#[cfg(all(feature = "edge", feature = "ssh"))]
fn screen_text(gate: &Gate, mode: &str) -> String {
    let factory = Arc::clone(gate.server.factory());
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let screens = moqui_core::screen::ScreenTest::new(&factory)
        .root_screen("component://tools/screen/System.xml")
        .render_mode(mode);
    let page = screens.test_render("Certificates", BTreeMap::new(), None);
    assert!(page.error_messages.is_empty(), "{:?}", page.error_messages);
    page.text()
}

/// Set `host` up from the screen with the check, or without it where `host` has no address here (*.localhost is
/// loopback by RFC 6761, but not every resolver says so).
#[cfg(all(feature = "edge", feature = "ssh"))]
fn set_up_from_the_screen(gate: &Gate, host: &str, directory: &str) {
    let fields = |skip: &'static str| {
        vec![
            ("otherHostName", host.to_string()),
            ("directoryChoice", directory.to_string()),
            ("contactEmail", "ops@example.com".to_string()),
            ("termsAgreed", "Y".to_string()),
            ("skipCheck", skip.to_string()),
        ]
    };
    let post = |skip: &'static str| {
        let fields = fields(skip);
        let fields: Vec<(&str, &str)> = fields.iter().map(|(name, value)| (*name, value.as_str())).collect();
        screen_post(gate, "setUp", &fields)
    };
    let errors = post("");
    if errors.contains("has no address in DNS") {
        eprintln!("{host}: set up without the check: {errors}");
        assert_eq!(post("Y"), "");
    } else {
        assert_eq!(errors, "", "the check");
    }
}

/// Edge-Node2 §5 E15's scenario, as far as it runs on one machine: an authority of the test's own stands for Let's
/// Encrypt, and loopback for the names' DNS.
#[cfg(all(feature = "edge", feature = "ssh"))]
#[test]
#[ignore = "acceptance: E15, a terminate host's and a passthrough name's certificates by ACME from the screen, renewed by the job"]
fn acceptance_certificates_for_an_edge_node_and_its_local_node() {
    use moqui_core::edge::LinkState;
    use moqui_host::edge_link::{local_web, Backoff, Dialer, LocalConfig};

    if !crate::common::own_process() {
        return;
    }
    const PASS: &str = "pass.localhost";
    const TERM: &str = "term.localhost";
    let rt = runtime();
    let (root, edge_cert, edge_key) = issued(&["localhost"]);
    let local_port = crate::common::free_port().to_string();
    let edge_port = crate::common::free_port();
    let edge_https = SocketAddr::from(([127, 0, 0, 1], edge_port));
    let authority = crate::acme_authority::start(&rt, edge_https);
    let edge_port_text = edge_port.to_string();
    let acme = [
        ("acme_directory_url", authority.directory.as_str()),
        ("acme_directory_ca", "acme-root.pem"),
        ("acme_challenge_port", edge_port_text.as_str()),
    ];
    let mut local_properties = vec![("webapp_https_listen_port", local_port.as_str())];
    local_properties.extend(acme);
    // the local node has no certificate at all
    let local = crate::common::start_with_conf_files(
        "acme-accept-local",
        None,
        "",
        &local_properties,
        &[("acme-root.pem", &authority.root_pem)],
    );
    let mut edge_properties = vec![
        ("node_role", "front_edge"),
        ("webapp_https_listen_port", edge_port_text.as_str()),
    ];
    edge_properties.extend(acme);
    let edge = crate::common::start_second_with(
        "acme-accept-edge",
        "",
        &edge_properties,
        &[
            ("cert/default.crt", &edge_cert),
            ("cert/default.key", &edge_key),
            ("acme-root.pem", &authority.root_pem),
        ],
    );
    let url = format!("https://localhost:{edge_port}");
    let paired = crate::edge_web::pair_node(&rt, &edge, &url, "local-e15", &root);
    for (host, mode) in [(PASS, "EdgeHostPassthrough"), (TERM, "EdgeHostTerminate")] {
        crate::edge_web::call(
            &edge,
            "add#EdgeHost",
            &[("hostName", host), ("pairedNodeId", "local-e15"), ("modeEnumId", mode)],
        )
        .unwrap();
    }
    let mut config = LocalConfig::new(&url, &paired.key, &paired.pin, "local-e15");
    config.backoff = Backoff {
        first: Duration::from_millis(100),
        max: Duration::from_millis(500),
    };
    config.connect_timeout = Duration::from_secs(3);
    config.web = local_web(local.server.factory());
    let (_shutdown, shutdown) = tokio::sync::watch::channel(false);
    let dialer = Dialer::start(config, rt.handle(), shutdown, None).unwrap();
    crate::edge_web::wait_status(&dialer, Duration::from_secs(10), "link up", |status| {
        status.state == LinkState::Linked
    });
    wait_until(Duration::from_secs(5), "the down connection", || {
        crate::edge_web::down(&edge, "local-e15")
    });

    // a name with no certificate gets no HTTPS, and nobody is asked for one
    assert!(served(&rt, edge_https, PASS, b"http/1.1").is_err());
    assert_eq!(authority.orders.load(Ordering::SeqCst), 0);

    // from the screen: the edge node for its terminate host, the local node for its passthrough name
    set_up_from_the_screen(&edge, TERM, &authority.directory);
    set_up_from_the_screen(&local, PASS, &authority.directory);
    for (gate, host) in [(&edge, TERM), (&local, PASS)] {
        wait_until(Duration::from_secs(15), "the certificate", || {
            let status = name_status(gate, host);
            assert_ne!(status, "AcmeFailed", "{host}'s order failed");
            status == "AcmeIssued"
        });
    }
    for host in [TERM, PASS] {
        wait_until(Duration::from_secs(10), "HTTPS for the name", || {
            served(&rt, edge_https, host, b"http/1.1")
                .is_ok_and(|leaf| leaf.windows(host.len()).any(|w| w == host.as_bytes()))
        });
    }

    // a file put in runtime/cert shows beside them
    let (_, files_cert, files_key) = issued(&["files.localhost"]);
    std::fs::write(edge.runtime.join("cert/files.localhost.crt"), files_cert).unwrap();
    std::fs::write(edge.runtime.join("cert/files.localhost.key"), files_key).unwrap();
    let text = screen_text(&edge, "html");
    for shown in [
        TERM,
        "files.localhost",
        "cert/files.localhost.crt",
        "Issued",
        "Test ACME Root",
    ] {
        assert!(text.contains(shown), "{shown}: {text}");
    }
    // and in the terminal
    let text = screen_text(&edge, "tui");
    for shown in [TERM, "files.localhost", "Issued", "Set up with ACME"] {
        assert!(text.contains(shown), "{shown}: {text}");
    }

    // both renew by the job: their certificates last until midnight
    for gate in [&edge, &local] {
        let (out, errors) = call(gate, &format!("{SERVICES}.renew#AcmeCertificates"), &[]);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(to_plain_string(&out["renewed"]), "1", "{out:?}");
    }
    assert_eq!(authority.issued.load(Ordering::SeqCst), 4);
    drop(dialer);
}
