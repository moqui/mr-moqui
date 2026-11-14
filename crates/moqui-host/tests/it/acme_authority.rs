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

//! A minimal ACME authority for the tests of `acme` (RFC 8555 with TLS-ALPN-01, RFC 8737), on HTTPS with a test root:
//! the directory, nonces, accounts, one-name orders, their authorization and challenge, the finalize and the
//! certificate. It checks no JWS signature; it does check the challenge as an authority does, by a TLS connection to
//! the node offering `acme-tls/1` and the key authorization's digest in the certificate served, so a node that
//! answers wrong gets an invalid order. Its certificates last until the next midnight UTC, so the renewal job finds
//! them due.

#![cfg(all(unix, feature = "embedded-postgres"))]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use hyper::body::Incoming;
use hyper::{Request, Response};
use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};

type Body = axum::body::Body;

/// A running authority: its directory URL, its root (PEM) for the node to trust, and what it has done.
pub(crate) struct Authority {
    pub(crate) directory: String,
    pub(crate) root_pem: String,
    /// The orders it took.
    pub(crate) orders: Arc<AtomicUsize>,
    /// The certificates it issued.
    pub(crate) issued: Arc<AtomicUsize>,
}

struct Order {
    host: String,
    token: String,
    thumb: String,
    authz: &'static str,
    error: Option<String>,
    chain: Option<String>,
}

struct State {
    base: String,
    validate_at: SocketAddr,
    issuer: rcgen::Issuer<'static, rcgen::KeyPair>,
    root_pem: String,
    next: AtomicU64,
    accounts: Mutex<HashMap<String, String>>,
    orders: Mutex<HashMap<u64, Order>>,
    taken: Arc<AtomicUsize>,
    issued: Arc<AtomicUsize>,
}

pub(crate) fn pem(label: &str, der: &[u8]) -> String {
    let text = base64::engine::general_purpose::STANDARD.encode(der);
    let lines: Vec<&str> = text
        .as_bytes()
        .chunks(64)
        .map(|line| std::str::from_utf8(line).unwrap())
        .collect();
    format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n"))
}

/// Start the authority on `runtime`; it validates every challenge by connecting to `validate_at`.
pub(crate) fn start(runtime: &tokio::runtime::Runtime, validate_at: SocketAddr) -> Authority {
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let mut name = rcgen::DistinguishedName::new();
    name.push(rcgen::DnType::CommonName, "Test ACME Root");
    params.distinguished_name = name;
    let root = params.self_signed(&ca_key).unwrap();
    let root_pem = pem("CERTIFICATE", root.der());
    let issuer = rcgen::Issuer::new(params, ca_key);
    // its own HTTPS certificate, for localhost
    let server_key = rcgen::KeyPair::generate().unwrap();
    let server_cert = rcgen::CertificateParams::new(vec!["localhost".to_string()])
        .unwrap()
        .signed_by(&server_key, &issuer)
        .unwrap();
    let tls = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![server_cert.der().clone()],
            rustls::pki_types::PrivateKeyDer::Pkcs8(server_key.serialize_der().into()),
        )
        .unwrap();
    let listener = runtime.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("https://localhost:{port}");
    let taken = Arc::new(AtomicUsize::new(0));
    let issued = Arc::new(AtomicUsize::new(0));
    let state = Arc::new(State {
        base: base.clone(),
        validate_at,
        issuer,
        root_pem: root_pem.clone(),
        next: AtomicU64::new(1),
        accounts: Mutex::default(),
        orders: Mutex::default(),
        taken: Arc::clone(&taken),
        issued: Arc::clone(&issued),
    });
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
    runtime.spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let (acceptor, state) = (acceptor.clone(), Arc::clone(&state));
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(stream).await else {
                    return;
                };
                let service = hyper::service::service_fn(move |request| {
                    let state = Arc::clone(&state);
                    async move { Ok::<_, std::convert::Infallible>(handle(&state, request).await) }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(tls), service)
                    .await;
            });
        }
    });
    Authority {
        directory: format!("{base}/dir"),
        root_pem,
        orders: taken,
        issued,
    }
}

fn answer(state: &State, status: u16, location: Option<String>, body: Body, kind: &str) -> Response<Body> {
    let nonce = state.next.fetch_add(1, Ordering::SeqCst);
    let mut builder = Response::builder()
        .status(status)
        .header("Replay-Nonce", format!("nonce{nonce}"))
        .header("Content-Type", kind);
    if let Some(location) = location {
        builder = builder.header("Location", location);
    }
    builder.body(body).unwrap()
}

fn json_answer(state: &State, status: u16, location: Option<String>, value: &Json) -> Response<Body> {
    answer(
        state,
        status,
        location,
        Body::from(value.to_string()),
        "application/json",
    )
}

fn problem(state: &State, detail: &str) -> Response<Body> {
    answer(
        state,
        400,
        None,
        Body::from(json!({"type": "urn:ietf:params:acme:error:malformed", "detail": detail}).to_string()),
        "application/problem+json",
    )
}

fn decode(part: &Json) -> Vec<u8> {
    URL_SAFE_NO_PAD
        .decode(part.as_str().unwrap_or_default())
        .unwrap_or_default()
}

/// RFC 7638: the SHA-256 of the key's required members in order, base64url.
fn thumbprint(jwk: &Json) -> String {
    let field = |name: &str| jwk[name].as_str().unwrap_or_default().to_string();
    let canonical = format!(
        r#"{{"crv":"{}","kty":"{}","x":"{}","y":"{}"}}"#,
        field("crv"),
        field("kty"),
        field("x"),
        field("y")
    );
    URL_SAFE_NO_PAD.encode(Sha256::digest(canonical.as_bytes()))
}

fn order_json(state: &State, id: u64, order: &Order) -> Json {
    let status = match (order.authz, &order.chain) {
        (_, Some(_)) => "valid",
        ("valid", None) => "ready",
        ("invalid", _) => "invalid",
        _ => "pending",
    };
    let mut value = json!({
        "status": status,
        "identifiers": [{"type": "dns", "value": order.host}],
        "authorizations": [format!("{}/authz/{id}", state.base)],
        "finalize": format!("{}/finalize/{id}", state.base),
    });
    if order.chain.is_some() {
        value["certificate"] = json!(format!("{}/cert/{id}", state.base));
    }
    value
}

fn challenge_json(state: &State, id: u64, order: &Order) -> Json {
    let mut challenge = json!({
        "type": "tls-alpn-01",
        "url": format!("{}/chall/{id}", state.base),
        "token": order.token,
        "status": order.authz,
    });
    if let Some(error) = &order.error {
        challenge["error"] = json!({"type": "urn:ietf:params:acme:error:incorrectResponse", "detail": error});
    }
    challenge
}

async fn handle(state: &State, request: Request<Incoming>) -> Response<Body> {
    let path = request.uri().path().to_string();
    if path == "/dir" {
        let base = &state.base;
        return json_answer(
            state,
            200,
            None,
            &json!({"newNonce": format!("{base}/nonce"), "newAccount": format!("{base}/account"),
                "newOrder": format!("{base}/order"), "revokeCert": format!("{base}/revoke"), "keyChange": format!("{base}/key")}),
        );
    }
    if path == "/nonce" {
        return answer(state, 200, None, Body::empty(), "text/plain");
    }
    let Ok(body) = axum::body::to_bytes(Body::new(request.into_body()), 1 << 20).await else {
        return problem(state, "no body");
    };
    let Ok(jws) = serde_json::from_slice::<Json>(&body) else {
        return problem(state, "not JSON");
    };
    let protected: Json = serde_json::from_slice(&decode(&jws["protected"])).unwrap_or_default();
    let payload: Json = serde_json::from_slice(&decode(&jws["payload"])).unwrap_or_default();
    let id = |prefix: &str| path.strip_prefix(prefix).and_then(|rest| rest.parse::<u64>().ok());
    if path == "/account" {
        let account = format!("{}/acct/{}", state.base, state.next.fetch_add(1, Ordering::SeqCst));
        let thumb = thumbprint(&protected["jwk"]);
        state.accounts.lock().unwrap().insert(account.clone(), thumb);
        return json_answer(state, 201, Some(account), &json!({"status": "valid"}));
    }
    if path.starts_with("/acct/") {
        return json_answer(state, 200, None, &json!({"status": "valid"}));
    }
    let kid = protected["kid"].as_str().unwrap_or_default().to_string();
    let Some(thumb) = state.accounts.lock().unwrap().get(&kid).cloned() else {
        return problem(state, "unknown account");
    };
    if path == "/order" {
        let host = payload["identifiers"][0]["value"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let id = state.next.fetch_add(1, Ordering::SeqCst);
        let token = URL_SAFE_NO_PAD.encode(Sha256::digest(format!("token{id}{host}").as_bytes()));
        let order = Order {
            host,
            token,
            thumb,
            authz: "pending",
            error: None,
            chain: None,
        };
        let value = order_json(state, id, &order);
        state.orders.lock().unwrap().insert(id, order);
        state.taken.fetch_add(1, Ordering::SeqCst);
        return json_answer(state, 201, Some(format!("{}/order/{id}", state.base)), &value);
    }
    if let Some(id) = id("/order/") {
        let orders = state.orders.lock().unwrap();
        let Some(order) = orders.get(&id) else {
            return problem(state, "no such order");
        };
        return json_answer(state, 200, None, &order_json(state, id, order));
    }
    if let Some(id) = id("/authz/") {
        let orders = state.orders.lock().unwrap();
        let Some(order) = orders.get(&id) else {
            return problem(state, "no such authorization");
        };
        let value = json!({"status": order.authz, "identifier": {"type": "dns", "value": order.host},
            "challenges": [challenge_json(state, id, order)]});
        return json_answer(state, 200, None, &value);
    }
    if let Some(id) = id("/chall/") {
        let Some((host, expected)) = state.orders.lock().unwrap().get(&id).map(|order| {
            let key_authorization = format!("{}.{}", order.token, order.thumb);
            (
                order.host.clone(),
                Sha256::digest(key_authorization.as_bytes()).to_vec(),
            )
        }) else {
            return problem(state, "no such challenge");
        };
        let validated = validate(state.validate_at, &host, &expected).await;
        let mut orders = state.orders.lock().unwrap();
        let Some(order) = orders.get_mut(&id) else {
            return problem(state, "no such challenge");
        };
        match validated {
            Ok(()) => order.authz = "valid",
            Err(error) => {
                order.authz = "invalid";
                order.error = Some(error);
            }
        }
        return json_answer(state, 200, None, &challenge_json(state, id, order));
    }
    if let Some(id) = id("/finalize/") {
        let csr = decode(&payload["csr"]);
        let mut orders = state.orders.lock().unwrap();
        let Some(order) = orders.get_mut(&id).filter(|order| order.authz == "valid") else {
            return problem(state, "the order is not ready");
        };
        let Some(chain) = issue(state, &order.host, &csr) else {
            return problem(state, "the request has no P-256 key");
        };
        order.chain = Some(chain);
        state.issued.fetch_add(1, Ordering::SeqCst);
        return json_answer(state, 200, None, &order_json(state, id, order));
    }
    if let Some(id) = id("/cert/") {
        let orders = state.orders.lock().unwrap();
        let Some(chain) = orders.get(&id).and_then(|order| order.chain.clone()) else {
            return problem(state, "no certificate");
        };
        return answer(state, 200, None, Body::from(chain), "application/pem-certificate-chain");
    }
    problem(state, "unknown resource")
}

/// The key of a certificate request: the 65-byte P-256 point of its only `BIT STRING` that starts with 0x04.
struct RequestKey(Vec<u8>);

impl rcgen::PublicKeyData for RequestKey {
    fn der_bytes(&self) -> &[u8] {
        &self.0
    }

    fn algorithm(&self) -> &'static rcgen::SignatureAlgorithm {
        &rcgen::PKCS_ECDSA_P256_SHA256
    }
}

/// A certificate for `host` and the request's key, until the next midnight UTC, with the root after it.
fn issue(state: &State, host: &str, csr: &[u8]) -> Option<String> {
    let at = csr.windows(4).position(|window| window == [0x03, 0x42, 0x00, 0x04])?;
    let point = csr.get(at + 3..at + 3 + 65)?.to_vec();
    let today = jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::UTC).date();
    let tomorrow = today.tomorrow().ok()?;
    let yesterday = today.yesterday().ok()?;
    let mut params = rcgen::CertificateParams::new(vec![host.to_string()]).ok()?;
    params.not_before = rcgen::date_time_ymd(
        i32::from(yesterday.year()),
        u8::try_from(yesterday.month()).ok()?,
        u8::try_from(yesterday.day()).ok()?,
    );
    params.not_after = rcgen::date_time_ymd(
        i32::from(tomorrow.year()),
        u8::try_from(tomorrow.month()).ok()?,
        u8::try_from(tomorrow.day()).ok()?,
    );
    let mut name = rcgen::DistinguishedName::new();
    name.push(rcgen::DnType::CommonName, host);
    params.distinguished_name = name;
    let cert = params.signed_by(&RequestKey(point), &state.issuer).ok()?;
    Some(format!("{}{}", pem("CERTIFICATE", cert.der()), state.root_pem))
}

/// The authority's check: a TLS connection to the node for `host` offering `acme-tls/1`, whose certificate carries
/// `expected` (the key authorization's digest).
async fn validate(at: SocketAddr, host: &str, expected: &[u8]) -> Result<(), String> {
    let mut config =
        rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(crate::acme::AnyCertificate))
            .with_no_client_auth();
    config.alpn_protocols = vec![b"acme-tls/1".to_vec()];
    let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|err| err.to_string())?;
    let tcp = tokio::net::TcpStream::connect(at)
        .await
        .map_err(|err| err.to_string())?;
    let tls = tokio_rustls::TlsConnector::from(Arc::new(config))
        .connect(name, tcp)
        .await
        .map_err(|err| format!("the TLS connection to {host} failed: {err}"))?;
    let connection = tls.get_ref().1;
    if connection.alpn_protocol() != Some(b"acme-tls/1".as_slice()) {
        return Err("the server did not agree on acme-tls/1".to_string());
    }
    let leaf = connection
        .peer_certificates()
        .and_then(|chain| chain.first())
        .ok_or_else(|| "no certificate".to_string())?;
    if leaf.as_ref().windows(expected.len()).any(|window| window == expected) {
        Ok(())
    } else {
        Err(format!(
            "the certificate for {host} does not carry the key authorization's digest"
        ))
    }
}
