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

//! SSO with an OpenID Connect provider played by the test: discovery, JWKS, an authorize endpoint that returns a code,
//! a token endpoint that checks the client secret and the PKCE verifier and signs an ES256 ID token with the nonce, user
//! info, and end session. The login goes through `/sso/login` and `/sso/callback` on the server, creates the user,
//! maps fields and roles to groups, replaces the session id, and returns to a same-origin `returnTo`. A tampered state is
//! refused, an off-site `returnTo` is dropped, and logout goes to the provider's end-session URL.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum", feature = "sso"))]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use crate::common::{exchange, http_request, merge_cookies, start_with_data};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use base64::Engine;
use moqui_lib::object::{to_plain_string, Value};
use p256::ecdsa::signature::Signer;
use sha2::Digest;

const CLIENT: &str = "moqui-test";
const SECRET: &str = "s3cret";

/// What the provider remembers between the authorize and token calls.
#[derive(Default)]
struct Idp {
    issuer: String,
    codes: HashMap<String, (String, String)>,
    /// The `sub` and `roles` the next login gets.
    roles: Vec<String>,
    /// How long ago the next ID token says the user signed in (`auth_time`): 0 for a sign-in now, more for one answered
    /// from the provider's own session.
    auth_age: u64,
    /// The last authorization request's query.
    authorize_query: HashMap<String, String>,
}

type Shared = Arc<Mutex<Idp>>;

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn signing_key() -> p256::ecdsa::SigningKey {
    p256::ecdsa::SigningKey::from_bytes(&[5u8; 32].into()).unwrap()
}

fn jwt(claims: &serde_json::Value) -> String {
    let header = b64(serde_json::json!({ "alg": "ES256", "kid": "k1", "typ": "JWT" })
        .to_string()
        .as_bytes());
    let body = b64(claims.to_string().as_bytes());
    let input = format!("{header}.{body}");
    let signature: p256::ecdsa::Signature = signing_key().sign(input.as_bytes());
    format!("{input}.{}", b64(&signature.to_bytes()))
}

async fn discovery(State(idp): State<Shared>) -> Json<serde_json::Value> {
    let issuer = idp.lock().unwrap().issuer.clone();
    Json(serde_json::json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "userinfo_endpoint": format!("{issuer}/userinfo"),
        "jwks_uri": format!("{issuer}/jwks"),
        "end_session_endpoint": format!("{issuer}/logout"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["ES256"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic"],
        "code_challenge_methods_supported": ["S256"],
    }))
}

async fn jwks() -> Json<serde_json::Value> {
    let point = signing_key().verifying_key().to_sec1_point(false);
    let bytes = point.as_bytes();
    Json(serde_json::json!({ "keys": [{
        "kty": "EC", "crv": "P-256", "kid": "k1", "use": "sig", "alg": "ES256",
        "x": b64(&bytes[1..33]), "y": b64(&bytes[33..65]),
    }]}))
}

async fn authorize(State(idp): State<Shared>, Query(query): Query<HashMap<String, String>>) -> Redirect {
    assert_eq!(query.get("client_id").map(String::as_str), Some(CLIENT));
    assert_eq!(query.get("code_challenge_method").map(String::as_str), Some("S256"));
    idp.lock().unwrap().authorize_query = query.clone();
    let code = format!("code-{}", idp.lock().unwrap().codes.len());
    idp.lock().unwrap().codes.insert(
        code.clone(),
        (
            query["code_challenge"].clone(),
            query.get("nonce").cloned().unwrap_or_default(),
        ),
    );
    let state = urlencode(&query["state"]);
    Redirect::to(&format!("{}?code={code}&state={state}", query["redirect_uri"]))
}

fn urlencode(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

async fn token(
    State(idp): State<Shared>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    let expected = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{CLIENT}:{SECRET}"))
    );
    if headers.get("authorization").and_then(|value| value.to_str().ok()) != Some(expected.as_str()) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "invalid_client" })),
        );
    }
    let mut idp = idp.lock().unwrap();
    let Some((challenge, nonce)) = idp.codes.remove(&form["code"]) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid_grant" })),
        );
    };
    let verified = b64(&sha2::Sha256::digest(form["code_verifier"].as_bytes()));
    if verified != challenge {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid_grant", "error_description": "PKCE" })),
        );
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let id_token = jwt(&serde_json::json!({
        "iss": idp.issuer, "sub": "idp-user-1", "aud": CLIENT, "exp": now + 300, "iat": now, "auth_time": now - idp.auth_age,
        "nonce": nonce, "preferred_username": "sso.joe", "email": "sso.joe@example.com",
        "given_name": "Joe", "family_name": "Single", "roles": idp.roles,
    }));
    (
        StatusCode::OK,
        Json(
            serde_json::json!({ "access_token": "at-1", "token_type": "Bearer", "expires_in": 300, "id_token": id_token }),
        ),
    )
}

async fn userinfo(headers: HeaderMap) -> impl IntoResponse {
    if headers.get("authorization").and_then(|value| value.to_str().ok()) != Some("Bearer at-1") {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({})));
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({ "sub": "idp-user-1", "zoneinfo": "America/Chicago" })),
    )
}

/// The provider on its own thread and runtime.
fn start_idp() -> (SocketAddr, Shared) {
    let shared: Shared = Arc::default();
    let state = Arc::clone(&shared);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            state.lock().unwrap().issuer = format!("http://{address}");
            let app = Router::new()
                .route("/.well-known/openid-configuration", get(discovery))
                .route("/jwks", get(jwks))
                .route("/authorize", get(authorize))
                .route("/token", post(token))
                .route("/userinfo", get(userinfo))
                .with_state(state);
            tx.send(address).unwrap();
            axum::serve(listener, app).await.unwrap();
        });
    });
    (rx.recv().unwrap(), shared)
}

fn data(idp: SocketAddr) -> String {
    format!(
        r#"<entity-facade-xml type="seed">
    <moqui.security.UserGroup userGroupId="SSO_DEFAULT" description="SSO default"/>
    <moqui.security.UserGroup userGroupId="SSO_STAFF" description="SSO staff"/>
    <moqui.security.sso.AuthFlow authFlowId="MOCK" authFlowTypeEnumId="AftOidc" description="Mock IdP" sequenceNum="1"
        defaultUserGroupId="SSO_DEFAULT"/>
    <moqui.security.sso.OidcFlow authFlowId="MOCK" clientTypeEnumId="OctOther" clientId="{CLIENT}" secret="{SECRET}"
        discoveryUri="http://{idp}/.well-known/openid-configuration" preferredJwsAlgorithmEnumId="OjaES256" useNonce="Y"/>
    <moqui.security.sso.AuthFlowRoleMap authFlowId="MOCK" roleName="staff" userGroupId="SSO_STAFF"/>
    <moqui.security.sso.AuthFlowFieldMap authFlowId="MOCK" ruleSeqId="01" srcFieldName="email" dstFieldName="emailAddress"/>
    <moqui.security.sso.AuthFlowFieldMap authFlowId="MOCK" ruleSeqId="02" dstFieldName="userFullName"
        dstFieldExpression="given_name + ' ' + family_name"/>
    <moqui.security.sso.AuthFlowFieldMap authFlowId="MOCK" ruleSeqId="03" srcFieldName="zoneinfo" dstFieldName="timeZone"/>
    <moqui.security.sso.AuthFlow authFlowId="OFF" authFlowTypeEnumId="AftOidc" description="Disabled flow" disabled="Y"/>
</entity-facade-xml>"#
    )
}

fn location(headers: &[(String, String)]) -> String {
    crate::common::header(headers, "location").unwrap_or_default()
}

/// One browser login through the server and the provider; returns the cookie after the callback and its response.
fn sso_login(server: SocketAddr, return_to: &str, tamper: bool) -> (String, u16, Vec<(String, String)>, String) {
    sso_login_with(server, return_to, tamper, "")
}

/// [`sso_login`] with more form parameters (`extra`, as `&name=value`).
fn sso_login_with(
    server: SocketAddr,
    return_to: &str,
    tamper: bool,
    extra: &str,
) -> (String, u16, Vec<(String, String)>, String) {
    let body = format!("authFlowId=MOCK&returnTo={}{extra}", urlencode(return_to));
    let (status, headers, _) = exchange(
        server,
        &http_request(
            "POST",
            "/sso/login",
            &[("Content-Type", "application/x-www-form-urlencoded")],
            &body,
        ),
    );
    assert_eq!(status, 302, "{headers:?}");
    let cookie = merge_cookies("", &headers);
    let to_idp = location(&headers);
    assert!(
        to_idp.contains("/authorize?") && to_idp.contains("code_challenge="),
        "{to_idp}"
    );
    // The provider answers with the callback.
    let idp_url = to_idp.trim_start_matches("http://");
    let (host, path) = idp_url.split_once('/').unwrap();
    let (status, headers, _) = exchange(
        host.parse().unwrap(),
        &http_request("GET", &format!("/{path}"), &[], ""),
    );
    assert_eq!(status, 303, "{headers:?}");
    let mut callback = location(&headers);
    if tamper {
        callback = callback.replace("state=", "state=x");
    }
    let callback_path = callback.split_once("://").unwrap().1.split_once('/').unwrap().1;
    let (status, headers, text) = exchange(
        server,
        &http_request("GET", &format!("/{callback_path}"), &[("Cookie", &cookie)], ""),
    );
    let after = merge_cookies(&cookie, &headers);
    (after, status, headers, text)
}

#[test]
fn an_oidc_login_creates_the_user_maps_groups_and_rotates_the_session() {
    if !crate::common::own_process() {
        return;
    }
    let (idp, provider) = start_idp();
    provider.lock().unwrap().roles = vec!["staff".to_string(), "unmapped".to_string()];
    let gate = start_with_data("sso", &data(idp));
    let server = gate.server.address();

    // The login page offers the enabled flow, not the disabled one.
    let (status, _, page) = exchange(server, &http_request("GET", "/Login", &[], ""));
    assert_eq!(status, 200);
    assert!(page.contains("/sso/login") && page.contains("Mock IdP"), "{page}");
    assert!(!page.contains("Disabled flow"));

    let (cookie, status, headers, text) = sso_login(server, "/qapps/AppList", false);
    assert_eq!(status, 302, "{text}");
    assert_eq!(location(&headers), "/qapps/AppList");
    let before = crate::common::cookies_of(&headers);
    assert!(
        before.contains("moqui.session="),
        "the callback set a new session cookie: {headers:?}"
    );

    let factory = Arc::clone(gate.server.factory());
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let facade = factory.entity().unwrap();
    // OidcFlow.secret is encrypted: the login above checked the client secret, read back decrypted.
    assert!(facade.crypt().is_some(), "field encryption is on");
    let account = facade
        .find("moqui.security.UserAccount")
        .unwrap()
        .eq("username", Value::string("sso.joe"))
        .one()
        .unwrap()
        .expect("the SSO user");
    let field = |name: &str| account.get(name).map(to_plain_string).unwrap_or_default();
    assert_eq!(field("externalUserId"), "idp-user-1");
    assert_eq!(field("emailAddress"), "sso.joe@example.com");
    assert_eq!(field("userFullName"), "Joe Single");
    assert_eq!(field("timeZone"), "America/Chicago");
    let groups = |facade: &moqui_core::entity::EntityFacade| {
        let mut groups: Vec<(String, bool)> = facade
            .find("moqui.security.UserGroupMember")
            .unwrap()
            .eq("userId", Value::string(field("userId")))
            .list()
            .unwrap()
            .values()
            .iter()
            .map(|row| {
                (
                    row.get("userGroupId").map(to_plain_string).unwrap_or_default(),
                    row.get("thruDate").is_none_or(Value::is_null),
                )
            })
            .collect();
        groups.sort();
        groups
    };
    assert_eq!(groups(&facade), [("SSO_STAFF".to_string(), true)]);

    // The session is logged in as sso.joe.
    let (status, _, apps) = exchange(
        server,
        &http_request("GET", "/qapps/AppList", &[("Cookie", &cookie)], ""),
    );
    assert_eq!(status, 200, "{apps}");

    // A provider's fresh sign-in (auth_time now) is a recent sign-in, one answered from the provider's own session (an old
    // auth_time) is not; a re-authentication asks the provider for a new sign-in (prompt=login, max_age=0) and counts only
    // when its auth_time comes after the request, whatever the provider answers.
    let recent = |cookie: &str| {
        let (status, _, json) = exchange(server, &http_request("GET", "/ReAuth/check", &[("Cookie", cookie)], ""));
        assert_eq!(status, 200, "{json}");
        let json: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(json["method"], "sso", "{json}");
        json["recent"].as_bool().unwrap()
    };
    assert!(recent(&cookie), "a fresh SSO sign-in is recent");
    assert!(!provider.lock().unwrap().authorize_query.contains_key("prompt"));
    provider.lock().unwrap().auth_age = 3600;
    let (silent, status, _, text) = sso_login(server, "/qapps", false);
    assert_eq!(status, 302, "{text}");
    assert!(
        !recent(&silent),
        "a sign-in the provider answered from its session an hour old is not recent"
    );
    let (ignored, status, _, text) = sso_login_with(server, "/qapps", false, "&reauth=true");
    assert_eq!(status, 302, "{text}");
    let query = provider.lock().unwrap().authorize_query.clone();
    assert_eq!(query.get("prompt").map(String::as_str), Some("login"), "{query:?}");
    assert_eq!(query.get("max_age").map(String::as_str), Some("0"), "{query:?}");
    assert!(
        !recent(&ignored),
        "a re-authentication the provider did not do is not recent"
    );
    provider.lock().unwrap().auth_age = 0;
    let (reauthenticated, status, _, text) = sso_login_with(server, "/qapps", false, "&reauth=true");
    assert_eq!(status, 302, "{text}");
    assert!(
        recent(&reauthenticated),
        "a re-authentication the provider did is recent"
    );

    // The next login without the staff role ends that membership and adds the default group.
    provider.lock().unwrap().roles = Vec::new();
    let (_, status, _, text) = sso_login(server, "/qapps", false);
    assert_eq!(status, 302, "{text}");
    assert_eq!(
        groups(&facade),
        [("SSO_DEFAULT".to_string(), true), ("SSO_STAFF".to_string(), false)]
    );

    // A tampered state is refused and the browser goes back to /Login.
    let (_, status, headers, _) = sso_login(server, "/qapps", true);
    assert_eq!(status, 302);
    assert!(location(&headers).ends_with("/Login"), "{headers:?}");

    // An off-site returnTo is not followed.
    let (cookie, status, headers, _) = sso_login(server, "https://evil.example/steal", false);
    assert_eq!(status, 302);
    assert!(!location(&headers).contains("evil.example"), "{headers:?}");

    // Logout ends the local session and goes to the provider's end session with the ID token.
    let (status, headers, _) = exchange(server, &http_request("GET", "/sso/logout", &[("Cookie", &cookie)], ""));
    assert_eq!(status, 302);
    let to = location(&headers);
    assert!(to.starts_with(&format!("http://{idp}/logout?id_token_hint=")), "{to}");
    assert!(to.contains("post_logout_redirect_uri="), "{to}");
}

/// An installed app's sign-in (RFC 8252, `crates/moqui-core/src/service/sso_app.rs`): the providers it may offer, a
/// start refused for a redirect URI not registered, the browser through `/sso/appLogin`, the provider and the callback
/// to `/sso/appCode`, which sends it to the app's redirect URI with a code and the app's state, and the code with its
/// verifier at `/sso/appToken` for a device key that signs in.
#[test]
fn an_app_signs_in_in_the_browser_and_takes_its_code_for_a_device_key() {
    if !crate::common::own_process() {
        return;
    }
    let (idp, _provider) = start_idp();
    let gate = start_with_data("sso-app", &data(idp));
    let server = gate.server.address();
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

    let (status, _, providers) = exchange(server, &http_request("GET", "/sso/appProviders", &[], ""));
    assert_eq!(status, 200, "{providers}");
    assert!(
        providers.contains("MOCK") && !providers.contains("Disabled flow"),
        "{providers}"
    );

    let start = |redirect: &str| {
        format!(
            "/sso/appLogin?authFlowId=MOCK&codeChallenge={challenge}&codeChallengeMethod=S256&state=s-1&redirectUri={}&deviceName={}",
            urlencode(redirect),
            urlencode("Joe's laptop")
        )
    };
    let (status, _, text) = exchange(
        server,
        &http_request("GET", &start("https://evil.example/sso"), &[], ""),
    );
    assert_eq!(status, 400, "{text}");

    // a start with no provider round trip gives no code, even in a browser that is signed in already
    let (status, headers, _) = exchange(server, &http_request("GET", &start("org.moqui.desktop:/sso"), &[], ""));
    assert_eq!(status, 302, "{headers:?}");
    let unfinished = merge_cookies("", &headers);
    let (status, _, _) = exchange(
        server,
        &http_request("GET", "/sso/appCode", &[("Cookie", &unfinished)], ""),
    );
    assert_eq!(status, 400, "no callback, no confirmation");

    // a callback that fails goes back to the app with the error and its state, not to the web's /Login
    let (status, headers, _) = exchange(server, &http_request("GET", &start("org.moqui.desktop:/sso"), &[], ""));
    assert_eq!(status, 302, "{headers:?}");
    let failing = merge_cookies("", &headers);
    let to_idp = location(&headers);
    let (host, path) = to_idp.trim_start_matches("http://").split_once('/').unwrap();
    let (_, headers, _) = exchange(
        host.parse().unwrap(),
        &http_request("GET", &format!("/{path}"), &[], ""),
    );
    let tampered = location(&headers).replace("state=", "state=x");
    let tampered_path = tampered.split_once("://").unwrap().1.split_once('/').unwrap().1;
    let (status, headers, _) = exchange(
        server,
        &http_request("GET", &format!("/{tampered_path}"), &[("Cookie", &failing)], ""),
    );
    assert_eq!(status, 302);
    assert_eq!(
        location(&headers),
        "org.moqui.desktop:/sso?error=server_error&state=s-1"
    );

    let (status, headers, _) = exchange(server, &http_request("GET", &start("org.moqui.desktop:/sso"), &[], ""));
    assert_eq!(status, 302, "{headers:?}");
    let cookie = merge_cookies("", &headers);
    let to_idp = location(&headers);
    let (host, path) = to_idp.trim_start_matches("http://").split_once('/').unwrap();
    let (status, headers, _) = exchange(
        host.parse().unwrap(),
        &http_request("GET", &format!("/{path}"), &[], ""),
    );
    assert_eq!(status, 303, "{headers:?}");
    let callback = location(&headers);
    let callback_path = callback.split_once("://").unwrap().1.split_once('/').unwrap().1;
    let (status, headers, text) = exchange(
        server,
        &http_request("GET", &format!("/{callback_path}"), &[("Cookie", &cookie)], ""),
    );
    assert_eq!(status, 302, "{text}");
    assert_eq!(location(&headers), "/sso/appCode");
    let cookie = merge_cookies(&cookie, &headers);

    // the user is asked to let the app in, by its name
    let (status, _, page) = exchange(server, &http_request("GET", "/sso/appCode", &[("Cookie", &cookie)], ""));
    assert_eq!(status, 200, "{page}");
    assert!(page.contains("Joe&#39;s laptop") && page.contains("sso.joe"), "{page}");
    let token = page
        .split("name=\"moquiSessionToken\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("the session token in the form")
        .to_string();
    // a post without the session's token is refused
    let (status, _, _) = exchange(
        server,
        &http_request(
            "POST",
            "/sso/appCodeConfirm",
            &[
                ("Cookie", &cookie),
                ("Content-Type", "application/x-www-form-urlencoded"),
            ],
            "allow=true",
        ),
    );
    assert_ne!(status, 302, "no code without the session's token");
    let (status, headers, text) = exchange(
        server,
        &http_request(
            "POST",
            "/sso/appCodeConfirm",
            &[
                ("Cookie", &cookie),
                ("Content-Type", "application/x-www-form-urlencoded"),
            ],
            &format!("allow=true&moquiSessionToken={token}"),
        ),
    );
    assert_eq!(status, 302, "{text}");
    let to_app = location(&headers);
    let query = to_app
        .strip_prefix("org.moqui.desktop:/sso?")
        .unwrap_or_else(|| panic!("{to_app}"));
    let pairs: HashMap<&str, &str> = query.split('&').filter_map(|pair| pair.split_once('=')).collect();
    assert_eq!(pairs.get("state"), Some(&"s-1"), "{to_app}");
    let code = pairs.get("code").expect("a code").to_string();
    // the browser's sign-in is over: no second code from it
    let (status, _, _) = exchange(server, &http_request("GET", "/sso/appCode", &[("Cookie", &cookie)], ""));
    assert_eq!(status, 400);

    let form = format!(
        "code={code}&codeVerifier={verifier}&redirectUri=org.moqui.desktop%3A%2Fsso&deviceName=Joe%27s%20laptop"
    );
    let (status, _, answer) = exchange(
        server,
        &http_request(
            "POST",
            "/sso/appToken",
            &[("Content-Type", "application/x-www-form-urlencoded")],
            &form,
        ),
    );
    assert_eq!(status, 200, "{answer}");
    let answer: serde_json::Value = serde_json::from_str(&answer).unwrap();
    let key = answer["loginKey"].as_str().expect("a device key");
    let (status, _, config) = exchange(
        server,
        &http_request("GET", "/qapps/clientConfig", &[("login_key", key)], ""),
    );
    assert_eq!(status, 200, "{config}");
    assert!(config.contains("sso.joe"), "{config}");
    let (status, _, _) = exchange(
        server,
        &http_request(
            "POST",
            "/sso/appToken",
            &[("Content-Type", "application/x-www-form-urlencoded")],
            &form,
        ),
    );
    assert_eq!(status, 400, "a code is taken once");
    // the browser that signed in is still signed in: the app's key signed nobody out
    let (status, _, apps) = exchange(
        server,
        &http_request("GET", "/qapps/AppList", &[("Cookie", &cookie)], ""),
    );
    assert_eq!(status, 200, "{apps}");
}
