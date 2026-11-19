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

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use aws_lc_rs::signature::{EcdsaKeyPair, KeyPair, ECDSA_P384_SHA384_FIXED_SIGNING};
use base64::Engine;
use huskarl::grant::authorization_code::{CompleteInput, PendingState};
use moqui_core::sso::{FlowConfig, FlowKind, OauthConfig, OidcConfig, SsoClient, SsoProfile, SsoStart};
use moqui_lib::sync::lock;
use p256::ecdsa::signature::Signer;
use serde_json::{json, Value as Json};

use super::{now_secs, profile, provider, HuskarlSso};

/// Admin P10 review: a flow that may not use the private network sends nothing to an internal address, written out
/// or by name, before any connection.
#[test]
fn a_flow_denied_the_private_network_sends_nothing_to_an_internal_address() {
    let transport = super::ReqwestTransport::new(true).unwrap();
    for url in [
        "https://10.1.2.3/.well-known/openid-configuration",
        "https://localhost:9/x",
    ] {
        let request = http::Request::get(url).body(bytes::Bytes::new()).unwrap();
        let transport = transport.clone();
        let err = crate::edge::block_on(async move { transport.send(request).await })
            .unwrap()
            .unwrap_err();
        assert!(err.contains("internal address"), "{url}: {err}");
    }
}

fn username(kind: FlowKind, client_type: &str, claims: serde_json::Value) -> Option<String> {
    let flow = FlowConfig {
        auth_flow_id: "F".to_string(),
        kind,
        deny_internal: false,
    };
    let serde_json::Value::Object(claims) = claims else {
        unreachable!()
    };
    profile(&flow, claims, provider(client_type).as_ref(), None).username
}

#[test]
fn an_email_is_a_username_only_when_verified_or_the_provider_names_it() {
    let oidc = || FlowKind::Oidc(OidcConfig::default());
    let claims =
        |verified: serde_json::Value| json!({"sub": "s1", "email": "a@example.com", "email_verified": verified});
    assert_eq!(
        username(oidc(), "", claims(json!(true))).as_deref(),
        Some("a@example.com")
    );
    assert_eq!(username(oidc(), "", claims(json!(false))), None);
    assert_eq!(
        username(oidc(), "", json!({"sub": "s1", "email": "a@example.com"})),
        None
    );
    assert_eq!(
        username(
            oidc(),
            "",
            json!({"sub": "s1", "preferred_username": "ann", "email": "a@example.com"})
        )
        .as_deref(),
        Some("ann")
    );
    let oauth = |client_type: &str| {
        FlowKind::Oauth(OauthConfig {
            client_type: client_type.to_string(),
            ..OauthConfig::default()
        })
    };
    // Facebook's username is its email, and it sends no email_verified.
    let facebook = json!({"id": "1", "email": "a@example.com"});
    assert_eq!(
        username(oauth("OctFacebook"), "OctFacebook", facebook).as_deref(),
        Some("a@example.com")
    );
    let unverified = json!({"id": "1", "email": "a@example.com", "email_verified": false});
    assert_eq!(username(oauth("OctGoogle"), "OctGoogle", unverified), None);
    assert_eq!(
        username(
            oauth("OctGitHub"),
            "OctGitHub",
            json!({"id": 1, "email": "a@example.com"})
        ),
        None
    );
}

// An OpenID provider the proofs below play: its discovery document, its JWKS and a token endpoint, on a local listener.

const CLIENT: &str = "proof-client";
const SECRET: &str = "proof-client-secret-of-32-bytes!";
const REDIRECT: &str = "https://app.example.com/sso/callback";

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Answers each request on `listener` with the JSON `answer` gives for its path (404 for none), one connection at a
/// time and each closed after its response; returns the paths asked for, in order.
fn serve(listener: TcpListener, answer: impl Fn(&str) -> Option<String> + Send + 'static) -> Arc<Mutex<Vec<String>>> {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&asked);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Ok(reading) = stream.try_clone() else { continue };
            let mut reader = BufReader::new(reading);
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let path = line
                .split_whitespace()
                .nth(1)
                .unwrap_or("")
                .split('?')
                .next()
                .unwrap_or("")
                .to_string();
            let mut length = 0;
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).is_err() || header.trim_end().is_empty() {
                    break;
                }
                if let Some((name, value)) = header.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                }
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            seen.lock().unwrap().push(path.clone());
            let (status, body) = match answer(&path) {
                Some(json) => ("200 OK", json),
                None => ("404 Not Found", "{}".to_string()),
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    asked
}

/// An EC public key as a JWK, from its uncompressed SEC1 point.
fn ec_jwk(crv: &str, alg: &str, kid: &str, point: &[u8]) -> Json {
    let half = (point.len() - 1) / 2;
    json!({
        "kty": "EC", "crv": crv, "alg": alg, "kid": kid, "use": "sig",
        "x": b64(&point[1..=half]), "y": b64(&point[half + 1..]),
    })
}

fn es256_key(seed: u8) -> p256::ecdsa::SigningKey {
    p256::ecdsa::SigningKey::from_bytes(&[seed; 32].into()).unwrap()
}

fn es256_jwk(key: &p256::ecdsa::SigningKey, kid: &str) -> Json {
    ec_jwk(
        "P-256",
        "ES256",
        kid,
        key.verifying_key().to_sec1_point(false).as_bytes(),
    )
}

fn es256_sign(key: &p256::ecdsa::SigningKey, input: &[u8]) -> Vec<u8> {
    let signature: p256::ecdsa::Signature = key.sign(input);
    signature.to_bytes().to_vec()
}

/// A compact JWS of `header` and `claims`, signed by `sign` over its signing input.
fn jws(header: &Json, claims: &Json, sign: impl FnOnce(&[u8]) -> Vec<u8>) -> String {
    let input = format!(
        "{}.{}",
        b64(header.to_string().as_bytes()),
        b64(claims.to_string().as_bytes())
    );
    let signature = sign(input.as_bytes());
    format!("{input}.{}", b64(&signature))
}

/// The provider's signing keys, both in its JWKS: `k1` (ES256) and `k2` (ES384).
struct Keys {
    es256: p256::ecdsa::SigningKey,
    es384: EcdsaKeyPair,
}

impl Keys {
    fn new() -> Self {
        Self {
            es256: es256_key(5),
            es384: EcdsaKeyPair::generate(&ECDSA_P384_SHA384_FIXED_SIGNING).unwrap(),
        }
    }

    fn es384_sign(&self, input: &[u8]) -> Vec<u8> {
        self.es384
            .sign(&aws_lc_rs::rand::SystemRandom::new(), input)
            .unwrap()
            .as_ref()
            .to_vec()
    }
}

/// A provider whose discovery lists `algs` as its ID token algorithms and whose token endpoint answers any code with
/// the ID token the test last put in `id_token`.
struct Idp {
    issuer: String,
    id_token: Arc<Mutex<String>>,
    /// What its user info endpoint answers: the ID token's subject unless a test says otherwise.
    user_info: Arc<Mutex<String>>,
}

impl Idp {
    fn start(algs: &[&str], keys: &Keys) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let id_token = Arc::new(Mutex::new(String::new()));
        let user_info = Arc::new(Mutex::new(json!({"sub": "proof-user"}).to_string()));
        let discovery = json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/authorize"),
            "token_endpoint": format!("{issuer}/token"),
            "userinfo_endpoint": format!("{issuer}/userinfo"),
            "jwks_uri": format!("{issuer}/jwks"),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": algs,
            "token_endpoint_auth_methods_supported": ["client_secret_basic"],
            "code_challenge_methods_supported": ["S256"],
        })
        .to_string();
        let jwks = json!({ "keys": [
            es256_jwk(&keys.es256, "k1"),
            ec_jwk("P-384", "ES384", "k2", keys.es384.public_key().as_ref()),
        ]})
        .to_string();
        let next = Arc::clone(&id_token);
        let info = Arc::clone(&user_info);
        serve(listener, move |path| match path {
            "/.well-known/openid-configuration" => Some(discovery.clone()),
            "/jwks" => Some(jwks.clone()),
            "/userinfo" => Some(info.lock().unwrap().clone()),
            "/token" => Some(
                json!({
                    "access_token": "proof-access-token", "token_type": "Bearer", "expires_in": 300,
                    "id_token": *next.lock().unwrap(),
                })
                .to_string(),
            ),
            _ => None,
        });
        Self {
            issuer,
            id_token,
            user_info,
        }
    }

    fn flow(&self, auth_flow_id: &str) -> FlowConfig {
        FlowConfig {
            auth_flow_id: auth_flow_id.to_string(),
            kind: FlowKind::Oidc(OidcConfig {
                client_type: "OctOther".to_string(),
                client_id: CLIENT.to_string(),
                secret: SECRET.to_string(),
                discovery_uri: format!("{}/.well-known/openid-configuration", self.issuer),
                preferred_jws_algorithm: "ES256".to_string(),
                use_nonce: true,
                ..OidcConfig::default()
            }),
            // the provider is on loopback
            deny_internal: false,
        }
    }

    /// The start of a sign-in through the relying party's real flow, and the callback the provider sends, with a code
    /// for which the token endpoint answers the ID token `make` builds from the claims a valid one carries (this
    /// issuer, this client, the start's nonce, in date).
    fn begin(
        &self,
        sso: &HuskarlSso,
        flow: &FlowConfig,
        make: impl FnOnce(&Json) -> String,
    ) -> (SsoStart, BTreeMap<String, String>) {
        let started = sso.start(flow, REDIRECT).unwrap();
        let url = reqwest::Url::parse(&started.authorize_url).unwrap();
        let query: HashMap<String, String> = url.query_pairs().into_owned().collect();
        let now = now_secs();
        let claims = json!({
            "iss": self.issuer, "sub": "proof-user", "aud": CLIENT, "iat": now, "exp": now + 300,
            "nonce": query["nonce"], "preferred_username": "proof.user",
        });
        *self.id_token.lock().unwrap() = make(&claims);
        let callback = BTreeMap::from([
            ("code".to_string(), "proof-code".to_string()),
            ("state".to_string(), query["state"].clone()),
        ]);
        (started, callback)
    }

    /// One sign-in through the relying party's real flow, start and callback.
    fn sign_in(&self, sso: &HuskarlSso, flow: &FlowConfig, make: impl FnOnce(&Json) -> String) -> SsoProfile {
        let (started, callback) = self.begin(sso, flow, make);
        sso.complete(flow, &started.pending, &callback)
            .expect("the ID token is taken")
    }

    /// A sign-in whose ID token the relying party refuses: the real callback is refused at the ID token's validation,
    /// and the reason is read from the error chain of the same grant completing the same callback again.
    fn refusal(&self, sso: &HuskarlSso, flow: &FlowConfig, make: impl FnOnce(&Json) -> String) -> String {
        let (started, callback) = self.begin(sso, flow, make);
        let err = sso
            .complete(flow, &started.pending, &callback)
            .expect_err("the ID token is refused");
        assert!(err.to_string().ends_with("validating ID token"), "{err}");
        let grant = Arc::clone(&lock(&sso.inner.grants)[&flow.auth_flow_id].grant);
        let pending: PendingState = serde_json::from_str(&started.pending).unwrap();
        let input: CompleteInput = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(callback.iter())
            .finish()
            .parse()
            .unwrap();
        let err = crate::edge::block_on(async move { grant.complete(&pending, input).await.map(|_| ()) })
            .unwrap()
            .expect_err("the same callback is refused again");
        let mut reason = err.to_string();
        let mut source = std::error::Error::source(&err);
        while let Some(cause) = source {
            reason = format!("{reason}: {cause}");
            source = cause.source();
        }
        reason
    }
}

/// The security proof of OWASP ASVS 5.0.0 v5.0.0-9.1.2 (docs/security/proofs/V9-Self-contained-Tokens.md), in-process:
/// the OIDC relying party takes an ID token only signed with an algorithm of its allowlist, which is the flow's preferred
/// algorithm when it names one, else the algorithms the provider's discovery document lists
/// (`id_token_signing_alg_values_supported`) among the asymmetric ones huskarl's verifier implements (SEC-WEB-93); `none`
/// is refused whatever the list says.
/// Against a provider the test plays, through the real start and callback: a token signed ES256 is taken; an unsigned
/// one is refused though the provider lists `none`, and so is one naming `NONE`; one signed ES384 with the provider's
/// own key from its JWKS is refused as off the list, refused from a provider that lists ES384 by a flow that prefers
/// ES256, and taken there by a flow that prefers none; one signed HS256 with the client secret is refused as off the
/// list, even from a provider that lists HS256, so a symmetric algorithm cannot borrow a secret.
#[test]
fn an_id_token_is_taken_only_signed_with_an_algorithm_of_the_allowlist_never_none() {
    let keys = Keys::new();
    let sso = HuskarlSso::default();
    let es256 = |claims: &Json| {
        jws(&json!({"alg": "ES256", "kid": "k1", "typ": "JWT"}), claims, |input| {
            es256_sign(&keys.es256, input)
        })
    };
    let es384 = |claims: &Json| {
        jws(&json!({"alg": "ES384", "kid": "k2", "typ": "JWT"}), claims, |input| {
            keys.es384_sign(input)
        })
    };
    let unsigned =
        |alg: &'static str| move |claims: &Json| jws(&json!({"alg": alg, "typ": "JWT"}), claims, |_| Vec::new());
    let hs256 = |claims: &Json| {
        jws(&json!({"alg": "HS256", "typ": "JWT"}), claims, |input| {
            aws_lc_rs::hmac::sign(
                &aws_lc_rs::hmac::Key::new(aws_lc_rs::hmac::HMAC_SHA256, SECRET.as_bytes()),
                input,
            )
            .as_ref()
            .to_vec()
        })
    };

    let narrow = Idp::start(&["ES256", "none"], &keys);
    let flow = narrow.flow("PROOF_ALG");
    let taken = narrow.sign_in(&sso, &flow, es256);
    assert_eq!(taken.id, "proof-user");
    assert_eq!(taken.username.as_deref(), Some("proof.user"));
    let reason = narrow.refusal(&sso, &flow, unsigned("none"));
    assert!(reason.ends_with("the token is unsigned"), "{reason}");
    let reason = narrow.refusal(&sso, &flow, unsigned("NONE"));
    assert!(reason.contains("disallowed algorithm 'NONE'"), "{reason}");
    let reason = narrow.refusal(&sso, &flow, es384);
    assert!(reason.contains("disallowed algorithm 'ES384'"), "{reason}");
    let reason = narrow.refusal(&sso, &flow, hs256);
    assert!(reason.contains("disallowed algorithm 'HS256'"), "{reason}");

    // SEC-WEB-93: the flow's preferred algorithm is the only one taken, whatever the provider lists
    let wide = Idp::start(&["ES256", "ES384", "HS256"], &keys);
    let pinned = wide.flow("PROOF_ALG_PINNED");
    let reason = wide.refusal(&sso, &pinned, es384);
    assert!(reason.contains("disallowed algorithm 'ES384'"), "{reason}");
    // with none preferred, the provider's list held to the asymmetric algorithms
    let mut flow = wide.flow("PROOF_ALG_WIDE");
    if let FlowKind::Oidc(config) = &mut flow.kind {
        config.preferred_jws_algorithm.clear();
    }
    let taken = wide.sign_in(&sso, &flow, es384);
    assert_eq!(
        taken.id, "proof-user",
        "the same ES384 token, where the provider lists ES384 and the flow prefers none"
    );
    let reason = wide.refusal(&sso, &flow, hs256);
    assert!(reason.contains("disallowed algorithm 'HS256'"), "{reason}");
    let reason = wide.refusal(&sso, &flow, unsigned("none"));
    assert!(reason.ends_with("the token is unsigned"), "{reason}");
}

/// SEC-WEB-93: the discovery document, and the keys and endpoints it names, are https but for this machine on an
/// instance that is not production.
#[test]
fn a_discovery_document_and_its_endpoints_are_https_but_on_this_machine() {
    use super::secure_url;
    assert!(secure_url("https://idp.example.com/.well-known/openid-configuration", "discovery").is_ok());
    assert!(secure_url("http://127.0.0.1:9000/jwks", "jwks_uri").is_ok());
    assert!(secure_url("http://idp.example.com/jwks", "jwks_uri").is_err());
    assert!(secure_url("ftp://idp.example.com/jwks", "jwks_uri").is_err());
    // a document on loopback that names a jwks_uri elsewhere in clear is refused when the grant is built
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    let discovery = json!({
        "issuer": issuer, "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"), "jwks_uri": "http://keys.example.com/jwks",
        "response_types_supported": ["code"], "id_token_signing_alg_values_supported": ["ES256"],
    })
    .to_string();
    serve(listener, move |path| {
        (path == "/.well-known/openid-configuration").then(|| discovery.clone())
    });
    let flow = FlowConfig {
        auth_flow_id: "PROOF_HTTP_JWKS".to_string(),
        kind: FlowKind::Oidc(OidcConfig {
            client_type: "OctOther".to_string(),
            client_id: CLIENT.to_string(),
            secret: SECRET.to_string(),
            discovery_uri: format!("{issuer}/.well-known/openid-configuration"),
            ..OidcConfig::default()
        }),
        deny_internal: false,
    };
    let err = HuskarlSso::default().start(&flow, REDIRECT).unwrap_err();
    assert!(err.to_string().contains("must be https"), "{err}");
    // an OAuth 2 flow's endpoints are held to the same rule
    let oauth = |token_url: &str| FlowConfig {
        auth_flow_id: format!("PROOF_OAUTH_{}", token_url.len()),
        kind: FlowKind::Oauth(OauthConfig {
            client_type: "OctOauth20".to_string(),
            client_id: CLIENT.to_string(),
            secret: SECRET.to_string(),
            authorize_url: "https://idp.example.com/authorize".to_string(),
            token_url: token_url.to_string(),
            ..OauthConfig::default()
        }),
        deny_internal: false,
    };
    let err = HuskarlSso::default()
        .start(&oauth("http://idp.example.com/token"), REDIRECT)
        .unwrap_err();
    assert!(err.to_string().contains("must be https"), "{err}");
    assert!(HuskarlSso::default()
        .start(&oauth("https://idp.example.com/token"), REDIRECT)
        .is_ok());
}

/// The security proof of OWASP ASVS 5.0.0 v5.0.0-9.1.3 (docs/security/proofs/V9-Self-contained-Tokens.md), in-process:
/// the keys that verify an ID token come only from the JWKS at the `jwks_uri` of the configured provider's discovery
/// document, never from the token. Against a provider the test plays, through the real start and callback, a token
/// signed by another key is refused whether its header points at that key's JWKS (`jku`), at a certificate for it
/// (`x5u`), or carries the key itself (`jwk`): with a `kid` naming that key, for want of a key; with the provider's
/// `kid`, as the provider's key does not verify it. The host those headers point at is never asked, and the same claims
/// signed with the provider's key are taken.
#[test]
fn the_keys_of_an_id_token_come_only_from_the_providers_jwks_never_its_header() {
    let keys = Keys::new();
    let sso = HuskarlSso::default();
    let idp = Idp::start(&["ES256"], &keys);
    let flow = idp.flow("PROOF_KEYS");

    let attacker = es256_key(9);
    let attacker_jwk = es256_jwk(&attacker, "evil");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let elsewhere = format!("http://{}", listener.local_addr().unwrap());
    let answer = json!({ "keys": [attacker_jwk] }).to_string();
    let asked_elsewhere = serve(listener, move |_| Some(answer.clone()));

    let jku = format!("{elsewhere}/jwks");
    let x5u = format!("{elsewhere}/cert.pem");
    let cases = [
        (
            json!({"alg": "ES256", "typ": "JWT", "kid": "evil", "jku": jku}),
            "no matching key",
        ),
        (
            json!({"alg": "ES256", "typ": "JWT", "kid": "evil", "x5u": x5u}),
            "no matching key",
        ),
        (
            json!({"alg": "ES256", "typ": "JWT", "kid": "evil", "jwk": attacker_jwk}),
            "no matching key",
        ),
        (
            json!({"alg": "ES256", "typ": "JWT", "kid": "evil", "jku": jku, "x5u": x5u, "jwk": attacker_jwk}),
            "no matching key",
        ),
        (
            json!({"alg": "ES256", "typ": "JWT", "kid": "k1", "jku": jku, "jwk": es256_jwk(&attacker, "k1")}),
            "signature mismatch",
        ),
    ];
    for (header, why) in &cases {
        let reason = idp.refusal(&sso, &flow, |claims| {
            jws(header, claims, |input| es256_sign(&attacker, input))
        });
        assert!(reason.ends_with(why), "{header}: {reason}");
    }
    assert!(
        asked_elsewhere.lock().unwrap().is_empty(),
        "the host the headers name is never asked: {:?}",
        asked_elsewhere.lock().unwrap()
    );

    let taken = idp.sign_in(&sso, &flow, |claims| {
        jws(&json!({"alg": "ES256", "typ": "JWT", "kid": "k1"}), claims, |input| {
            es256_sign(&keys.es256, input)
        })
    });
    assert_eq!(taken.id, "proof-user");
}

/// The security proof of OWASP ASVS 5.0.0 v5.0.0-6.8.2 (docs/security/proofs/V6-Authentication.md), in-process: the
/// OIDC sign-in takes an ID token only when its signature is there and verifies with the provider's key. Against a
/// provider the test plays, through the real start and callback, a token signed with the provider's key is taken, and
/// the same token is refused with its claims changed after signing (another `sub`), with a bit of its signature
/// flipped, with its signature cut short, emptied or left off, and with the provider's own signature of other claims
/// put on it; an unsigned token (`none`) is refused too
/// (`an_id_token_is_taken_only_signed_with_an_algorithm_of_the_allowlist_never_none`). The OAuth 2 flows carry no
/// assertion and there is no SAML. Not covered: the mail client's OAuth sign-in (moqui-mail `oauth.rs`), which reads
/// its ID token's claims without checking the signature, the token coming straight from the provider's token endpoint
/// over TLS (OpenID Connect Core 3.1.3.7).
#[test]
fn an_id_token_is_taken_only_when_its_signature_is_there_and_verifies() {
    let keys = Keys::new();
    let sso = HuskarlSso::default();
    let idp = Idp::start(&["ES256"], &keys);
    let flow = idp.flow("PROOF_SIGNATURE");
    let header = json!({"alg": "ES256", "kid": "k1", "typ": "JWT"});
    let signed = |claims: &Json| jws(&header, claims, |input| es256_sign(&keys.es256, input));
    let parts = |token: &str| -> (String, String, String) {
        let mut parts = token.split('.').map(str::to_string);
        (parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap())
    };
    let other_claims = |claims: &Json| {
        let mut other = claims.clone();
        other["sub"] = json!("someone-else");
        other["preferred_username"] = json!("someone.else");
        other
    };

    let taken = idp.sign_in(&sso, &flow, signed);
    assert_eq!(taken.id, "proof-user");

    /// An ID token the provider's callback answers, made from the claims a valid one carries.
    type MakeToken<'a> = Box<dyn Fn(&Json) -> String + 'a>;
    let mut reasons = Vec::new();
    let cases: Vec<(&str, MakeToken<'_>)> = vec![
        (
            "claims changed after signing",
            Box::new(|claims: &Json| {
                let (head, _, signature) = parts(&signed(claims));
                format!(
                    "{head}.{}.{signature}",
                    b64(other_claims(claims).to_string().as_bytes())
                )
            }),
        ),
        (
            "a bit of the signature flipped",
            Box::new(|claims: &Json| {
                let (head, payload, signature) = parts(&signed(claims));
                let mut bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(signature)
                    .unwrap();
                bytes[10] ^= 0x01;
                format!("{head}.{payload}.{}", b64(&bytes))
            }),
        ),
        (
            "the signature cut short",
            Box::new(|claims: &Json| {
                let (head, payload, signature) = parts(&signed(claims));
                let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(signature)
                    .unwrap();
                format!("{head}.{payload}.{}", b64(&bytes[..32]))
            }),
        ),
        (
            "the signature empty",
            Box::new(|claims: &Json| {
                let (head, payload, _) = parts(&signed(claims));
                format!("{head}.{payload}.")
            }),
        ),
        (
            "another token's signature",
            Box::new(|claims: &Json| {
                let (head, payload, _) = parts(&signed(claims));
                let (_, _, borrowed) = parts(&signed(&other_claims(claims)));
                format!("{head}.{payload}.{borrowed}")
            }),
        ),
    ];
    for (case, make) in &cases {
        let reason = idp.refusal(&sso, &flow, |claims| make(claims));
        reasons.push(format!("{case}: {reason}"));
    }
    // with no signature segment at all the token is not even a JWS: refused at the callback, whatever the stage
    let (started, callback) = idp.begin(&sso, &flow, |claims| {
        let (head, payload, _) = parts(&signed(claims));
        format!("{head}.{payload}")
    });
    assert!(
        sso.complete(&flow, &started.pending, &callback).is_err(),
        "no signature segment"
    );
    for reason in &reasons {
        assert!(
            reason.contains("signature") || reason.contains("malformed") || reason.contains("invalid"),
            "{reasons:?}"
        );
    }

    let again = idp.sign_in(&sso, &flow, signed);
    assert_eq!(again.id, "proof-user", "the provider's signature is taken still");
}

/// The security proof of OWASP ASVS 5.0.0 v5.0.0-9.2.3 (docs/security/proofs/V9-Self-contained-Tokens.md), in-process:
/// the OIDC sign-in takes an ID token only when it is meant for this relying party, its `aud` the flow's client id.
/// Against a provider the test plays, through the real start and callback, signed with the provider's key and valid
/// otherwise, a token for another client is refused, and so are one whose `aud` list leaves this client out and one with
/// no `aud`; one for this client is taken. The mail client's sign-in checks its ID token's `aud` the same way
/// (`a_mail_id_token_for_another_client_is_refused`, moqui-mail `oauth_tests.rs`); a Git LFS token is taken only for
/// the repository it names (`lfs_objects_are_uploaded_downloaded_and_locked_over_the_batch_api`, moqui-git
/// `server/tests.rs`: "a token is for its repository").
#[test]
fn an_id_token_is_taken_only_when_its_audience_is_this_client() {
    let keys = Keys::new();
    let sso = HuskarlSso::default();
    let idp = Idp::start(&["ES256"], &keys);
    let flow = idp.flow("PROOF_AUDIENCE");
    let header = json!({"alg": "ES256", "kid": "k1", "typ": "JWT"});
    let with_audience = |audience: Option<Json>| {
        let header = header.clone();
        let keys = &keys;
        move |claims: &Json| {
            let mut claims = claims.clone();
            match &audience {
                Some(audience) => claims["aud"] = audience.clone(),
                None => {
                    claims.as_object_mut().unwrap().remove("aud");
                }
            }
            jws(&header, &claims, |input| es256_sign(&keys.es256, input))
        }
    };
    for audience in [
        Some(json!("another-client")),
        Some(json!(["another-client", "a-third-client"])),
        Some(json!("")),
        None,
    ] {
        let reason = idp.refusal(&sso, &flow, with_audience(audience.clone()));
        assert!(!reason.is_empty(), "{audience:?}");
    }
    let taken = idp.sign_in(&sso, &flow, with_audience(Some(json!(CLIENT))));
    assert_eq!(taken.id, "proof-user");
}

/// The security proof of OWASP ASVS 5.0.0 v5.0.0-9.2.4 (docs/security/proofs/V9-Self-contained-Tokens.md), in-process:
/// the JWTs the framework signs name their audience. Apple's client secret, an ES256 JWT from the team signed with the
/// key its OidcFlow holds, carries `aud` `https://appleid.apple.com`, its client as `sub`, the team as `iss` and a day's
/// life, and verifies with that key. The others are proved where they are made: the Web Push VAPID token's `aud` is the
/// push service's origin (`the_vapid_header_is_an_es256_token_for_the_endpoint_origin`, moqui-core
/// `notify/push_crypto_tests.rs`), the FCM grant's is Google's token endpoint
/// (`the_jwt_asks_for_the_messaging_scope_at_the_token_endpoint_and_is_signed_rs256`, `service/fcm_tests.rs`); the
/// tokens signed with the server's own keys have a key of their own or one derived for their purpose and name what they
/// are for: an LFS token its repository and operation (`lfs_objects_are_uploaded_downloaded_and_locked_over_the_batch_api`,
/// moqui-git), a script's server grant its purpose (`a_server_grant_checks_for_its_purpose_and_subject_until_it_expires`,
/// moqui-core `script/host/tests.rs`).
#[test]
fn the_apple_client_secret_names_apple_as_its_audience() {
    use p256::ecdsa::signature::Verifier;
    use p256::pkcs8::EncodePrivateKey;

    let key = es256_key(11);
    let pem = key.to_pkcs8_pem(Default::default()).unwrap().to_string();
    let config = OidcConfig {
        client_type: "OctApple".to_string(),
        client_id: "com.example.proof".to_string(),
        secret: pem,
        apple_team_id: "PROOFTEAM1".to_string(),
        apple_key_id: "PROOFKEY1".to_string(),
        ..OidcConfig::default()
    };
    let token = super::apple_client_secret(&config).unwrap();
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(segments.len(), 3, "{token}");
    let decode = |segment: &str| -> Json {
        serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(segment)
                .unwrap(),
        )
        .unwrap()
    };
    assert_eq!(decode(segments[0]), json!({"alg": "ES256", "kid": "PROOFKEY1"}));
    let claims = decode(segments[1]);
    assert_eq!(claims["aud"], "https://appleid.apple.com");
    assert_eq!(claims["sub"], "com.example.proof");
    assert_eq!(claims["iss"], "PROOFTEAM1");
    assert_eq!(
        claims["exp"].as_u64().unwrap() - claims["iat"].as_u64().unwrap(),
        86_400
    );
    let signature = p256::ecdsa::Signature::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(segments[2])
            .unwrap(),
    )
    .unwrap();
    let signed = format!("{}.{}", segments[0], segments[1]);
    assert!(key.verifying_key().verify(signed.as_bytes(), &signature).is_ok());
}

/// SEC-WEB-86: an OpenID user info answer is merged into the profile only when its `sub` is the ID token's.
#[test]
fn a_user_info_answer_is_taken_only_for_the_id_tokens_subject() {
    use super::same_subject;
    let sub = |text: &str| json!(text);
    assert!(same_subject("F", Some(&sub("u1")), Some(&sub("u1"))).is_ok());
    for (token, info) in [
        (Some(sub("u1")), Some(sub("u2"))),
        (Some(sub("u1")), None),
        (None, Some(sub("u1"))),
    ] {
        let err = same_subject("F", token.as_ref(), info.as_ref()).unwrap_err();
        assert!(err.to_string().contains("not for the ID token's subject"), "{err}");
    }
    assert!(same_subject("F", Some(&sub("u1")), Some(&json!(1))).is_err());

    // through the real start and callback: another subject's user info refuses the sign-in, the ID token's own is
    // merged into the profile
    let keys = Keys::new();
    let sso = HuskarlSso::default();
    let idp = Idp::start(&["ES256"], &keys);
    let flow = idp.flow("PROOF_USER_INFO");
    let es256 = |claims: &Json| {
        jws(&json!({"alg": "ES256", "kid": "k1", "typ": "JWT"}), claims, |input| {
            es256_sign(&keys.es256, input)
        })
    };
    *idp.user_info.lock().unwrap() = json!({"sub": "someone-else", "email": "other@example.com"}).to_string();
    let (started, callback) = idp.begin(&sso, &flow, es256);
    let err = sso.complete(&flow, &started.pending, &callback).unwrap_err();
    assert!(err.to_string().contains("not for the ID token's subject"), "{err}");
    *idp.user_info.lock().unwrap() = json!({"sub": "proof-user", "zoneinfo": "Europe/Oslo"}).to_string();
    let taken = idp.sign_in(&sso, &flow, es256);
    assert_eq!(
        taken
            .attributes
            .get("zoneinfo")
            .map(moqui_lib::object::to_plain_string)
            .as_deref(),
        Some("Europe/Oslo")
    );
}
