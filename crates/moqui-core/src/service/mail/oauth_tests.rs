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

//! The OAuth sign-in of mail accounts ([`super::oauth`]) against a fake provider on 127.0.0.1 (token endpoint with
//! PKCE, refresh, revocation, user info) and the fake IMAP and SMTP servers with XOAUTH2: the code exchange with its
//! state checks (a forged, replayed, late, another user's or another session's state), the address check and its
//! confirmed change, refresh once at a time, a revoked refresh token marking only its account, XOAUTH2 for the sync,
//! the server test and a send, no token in any service output or the popup's page, revocation on remove and sign-out,
//! and discovery of the provider by domain and MX.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;
use serde_json::json;

use super::fake_servers::{fake_dns_with_mx, fake_http, FakeCode, FakeImap, FakePop, FakeProvider, FakeSmtp};
use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::crypto::CryptConfig;
use crate::entity::{EntityFacade, EntityValue};
use crate::user::session::{session_attribute, set_session_attribute};

const SERVICES: &str = "org.moqui.impl.EmailClientServices";
const PROVIDER: &str = "moqui.basic.email.EmailOAuthProvider";
const ACCOUNT: &str = "moqui.basic.email.EmailAccount";
const SERVER: &str = "moqui.basic.email.EmailServer";
const TOKEN: &str = "moqui.basic.email.EmailServerAccessToken";
const REDIRECT: &str = "https://mail.example.test/emailOAuth/callback";
const CLIENT: &str = "client-1";
const SECRET: &str = "secret-1";
const PENDING: &str = "moqui.email.oauth";

/// The properties a test sets, cleared after it.
struct Properties;

impl Properties {
    fn set() -> Self {
        moqui_lib::env::set_property("email_oauth_redirect_uri", REDIRECT);
        moqui_lib::env::set_property("email_oauth_test_client_id", CLIENT);
        moqui_lib::env::set_property("email_oauth_test_client_secret", SECRET);
        Properties
    }
}

impl Drop for Properties {
    fn drop(&mut self) {
        for name in [
            "email_oauth_redirect_uri",
            "email_oauth_test_client_id",
            "email_oauth_test_client_secret",
        ] {
            moqui_lib::env::clear_property(name);
        }
    }
}

/// One test at a time: the access tokens are kept per process by server id.
fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    lock(&LOCK)
}

fn setup() -> (ExecutionContext, EntityFacade) {
    setup_with("")
}

/// [`setup`] with the conf text `conf`.
fn setup_with(conf: &str) -> (ExecutionContext, EntityFacade) {
    let ctx = ExecutionContextFactory::test_context_in(
        conf,
        crate::service::ServicePools::idle(),
        crate::context::framework_dir(),
    );
    let facade = crate::testing::open();
    facade.assume_db_tables();
    facade.set_crypt(
        CryptConfig {
            pass: "OAuthTestPassword:CHANGEME".to_string(),
            salt: "20201202".to_string(),
            iterations: 10,
            algorithm: "PBEWithHmacSHA256AndAES_128".to_string(),
        },
        Vec::new(),
    );
    ctx.factory().set_entity(facade.clone());
    (ctx, facade)
}

fn install() {
    let framework = crate::context::framework_dir().join("service/org/moqui");
    let paths = vec![
        framework.join("EmailServices.xml"),
        framework.join("impl/EmailServices.xml"),
        framework.join("impl/EmailClientServices.xml"),
        framework.join("impl/ShellServices.xml"),
    ];
    let failures = crate::service::lint(&paths).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
}

fn text(value: &str) -> Value {
    Value::string(value)
}

fn row(facade: &EntityFacade, entity: &str, fields: &[(&str, Value)]) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut row = facade.make_value(entity).unwrap();
    for (field, value) in fields {
        row.set(*field, value.clone());
    }
    row.create(facade).unwrap();
}

fn update(facade: &EntityFacade, entity: &str, key: (&str, &str), fields: &[(&str, Value)]) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut row = facade.make_value(entity).unwrap();
    row.set(key.0, text(key.1));
    for (field, value) in fields {
        row.set(*field, value.clone());
    }
    row.update(facade).unwrap();
}

fn one(facade: &EntityFacade, entity: &str, key: &str, id: &str) -> EntityValue {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    facade
        .find(entity)
        .unwrap()
        .eq(key, text(id))
        .use_cache(false)
        .one()
        .unwrap()
        .unwrap_or_else(|| panic!("no {entity} {id}"))
}

fn field(row: &EntityValue, name: &str) -> String {
    row.get(name).map(to_plain_string).unwrap_or_default()
}

fn user(facade: &EntityFacade, user_id: &str) {
    row(
        facade,
        "moqui.security.UserAccount",
        &[
            ("userId", text(user_id)),
            ("username", text(user_id)),
            ("emailAddress", text(&format!("{user_id}@example.com"))),
        ],
    );
}

fn login(user_id: &str) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    crate::user::internal_login_user(user_id).unwrap();
}

fn call(name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    let name = if name.contains('.') {
        name.to_string()
    } else {
        format!("{SERVICES}.{name}")
    };
    crate::service::sync(&name, &parameters).unwrap_or_else(|err| panic!("{name}: {err}"))
}

fn errors() -> String {
    ExecutionContext::with_current(|ec| {
        let text = ec.message().errors().join("\n");
        ec.message().clear_errors();
        text
    })
    .unwrap_or_default()
}

fn ok(name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let out = call(name, parameters);
    let errors = errors();
    assert!(errors.is_empty(), "{name}: {errors}");
    out
}

fn out_text(out: &BTreeMap<String, Value>, key: &str) -> String {
    out.get(key).map(to_plain_string).unwrap_or_default()
}

/// The fake provider as an `EmailOAuthProvider` (its client from the email_oauth_test_ properties, which win over the
/// row's), with the mail servers of an account made with it.
fn provider_row(facade: &EntityFacade, id: &str, fake: &FakeProvider, servers: (u16, u16), revoke: bool) {
    let mut fields = vec![
        ("oauthProviderEnumId", text(id)),
        ("propertyPrefix", text("email_oauth_test")),
        ("clientId", text("row-client")),
        ("clientSecret", text("row-secret")),
        ("authorizeUrl", text(&fake.url("/authorize"))),
        ("tokenUrl", text(&fake.url("/token"))),
        ("userInfoUrl", text(&fake.url("/userinfo"))),
        ("scope", text("https://mail.google.com/ openid email")),
        ("mailScope", text("https://mail.google.com/")),
        ("authParams", text("access_type=offline&prompt=consent")),
        ("issuer", text("https://accounts.google.com")),
        ("addressClaims", text("email")),
        ("addressDomains", text("gmail.test")),
        ("storeHost", text("127.0.0.1")),
        ("storePort", text(&servers.0.to_string())),
        ("storeProtocol", text("imap")),
        ("smtpHost", text("127.0.0.1")),
        ("smtpPort", text(&servers.1.to_string())),
        ("smtpSsl", text("N")),
        ("smtpStartTls", text("N")),
    ];
    if revoke {
        fields.push(("revokeUrl", text(&fake.url("/revoke"))));
    }
    row(facade, PROVIDER, &fields);
}

/// The query of an authorization URL.
fn query(url: &str) -> HashMap<String, String> {
    let (_, query) = url.split_once('?').expect("a query");
    crate::web::form_pairs(query).into_iter().collect()
}

/// start#OAuthSignIn: the authorization URL's query and the sign-in id.
fn begin(parameters: &[(&str, Value)]) -> (HashMap<String, String>, String) {
    let out = ok("start#OAuthSignIn", parameters);
    let url = out_text(&out, "authUrl");
    (query(&url), out_text(&out, "signInId"))
}

/// The ID token claims of a sign-in as `email`.
fn claims(started: &HashMap<String, String>, email: &str) -> serde_json::Value {
    json!({
        "iss": "https://accounts.google.com", "aud": CLIENT, "sub": "110", "email": email, "email_verified": true,
        "nonce": started["nonce"], "exp": jiff::Timestamp::now().as_second() + 3600,
    })
}

/// A code the fake provider exchanges for the PKCE challenge of `started`.
fn code_for(fake: &FakeProvider, started: &HashMap<String, String>, claims: serde_json::Value, scope: &str) -> String {
    let mut state = lock(&fake.state);
    let code = format!("code-{}", state.codes.len() + state.requests.len() + 1);
    state.codes.insert(
        code.clone(),
        FakeCode {
            challenge: started["code_challenge"].clone(),
            claims,
            scope: scope.to_string(),
            refresh: true,
        },
    );
    code
}

const MAIL_SCOPE: &str = "https://mail.google.com/ openid https://www.googleapis.com/auth/userinfo.email";

fn callback(parameters: &[(&str, &str)]) -> BTreeMap<String, Value> {
    let parameters: Vec<(&str, Value)> = parameters.iter().map(|(name, value)| (*name, text(value))).collect();
    ok("complete#OAuthSignIn", &parameters)
}

/// A whole sign-in for a new account, active at once with the year's sync window (or `account`) as `email`: its outcome.
fn sign_in(fake: &FakeProvider, email: &str, account: Option<&str>) -> BTreeMap<String, Value> {
    let mut parameters = vec![("oauthProviderEnumId", text("EopTest"))];
    match account {
        Some(account) => parameters.push(("emailAccountId", text(account))),
        None => {
            parameters.push(("emailAddress", text(email)));
            parameters.push(("statusId", text("EasActive")));
        }
    }
    let (started, _) = begin(&parameters);
    let code = code_for(fake, &started, claims(&started, email), MAIL_SCOPE);
    callback(&[("state", &started["state"]), ("code", &code)])
}

/// Every token the fake provider gave, so a test can look for them where they must not be.
fn tokens(fake: &FakeProvider) -> Vec<String> {
    let state = lock(&fake.state);
    state
        .access_tokens
        .iter()
        .chain(&state.refresh_tokens)
        .cloned()
        .collect()
}

fn assert_no_token(fake: &FakeProvider, what: &str, shown: &str) {
    for token in tokens(fake) {
        assert!(!shown.contains(&token), "{what} shows token {token}: {shown}");
    }
}

/// The sign-in code exchange, with PKCE and every state check, the address check and its confirmed change, the popup's
/// page, and no token in any output.
#[test]
fn the_code_exchange_checks_state_pkce_scope_nonce_and_address() {
    let _serial = serial();
    let _lock = crate::user::session::cookie_property_lock();
    let _properties = Properties::set();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    user(&facade, "alice");
    user(&facade, "carol");
    login("alice");
    let fake = FakeProvider::start(CLIENT, SECRET, REDIRECT);
    provider_row(&facade, "EopTest", &fake, (993, 465), true);

    let providers = ok("get#OAuthProviders", &[]);
    assert_eq!(out_text(&providers, "redirectUri"), REDIRECT);
    assert!(
        format!("{:?}", providers.get("providerList")).contains("configured\": Bool(true)"),
        "{providers:?}"
    );

    // the authorization request: the deployment's client and redirect URI, PKCE S256, state, nonce, the address
    let (started, sign_in_id) = begin(&[
        ("emailAddress", text("alice@gmail.test")),
        ("accountName", text("Alice Gmail")),
    ]);
    assert_eq!(started["response_type"], "code");
    assert_eq!(started["client_id"], CLIENT);
    assert_eq!(started["redirect_uri"], REDIRECT);
    assert_eq!(started["code_challenge_method"], "S256");
    assert_eq!(started["code_challenge"].len(), 43);
    assert!(started["state"].len() >= 43, "{started:?}");
    assert!(!started["nonce"].is_empty());
    assert_eq!(started["login_hint"], "alice@gmail.test");
    assert_eq!(started["access_type"], "offline");
    assert_eq!(started["prompt"], "consent");
    assert!(started["scope"].contains("https://mail.google.com/"));
    assert!(!started.contains_key("code_verifier"));
    let pending = session_attribute(PENDING).expect("the pending sign-in in the session");
    assert!(pending.contains(&started["state"]));

    // a state this session did not start: refused before any request to the provider
    let forged = callback(&[("state", "forged-state-value"), ("code", "anything")]);
    assert_eq!(out_text(&forged, "status"), "error");
    assert_eq!(out_text(&forged, "errorCode"), "invalidState");
    assert_eq!(lock(&fake.state).count("/token", ""), 0);

    // the user said no at the provider; the state is then used, and a replay with a code is refused
    let denied = callback(&[("state", &started["state"]), ("error", "access_denied")]);
    assert_eq!(out_text(&denied, "errorCode"), "denied");
    assert_eq!(out_text(&denied, "signInId"), sign_in_id);
    let code = code_for(&fake, &started, claims(&started, "alice@gmail.test"), MAIL_SCOPE);
    let replayed = callback(&[("state", &started["state"]), ("code", &code)]);
    assert_eq!(out_text(&replayed, "errorCode"), "invalidState");
    assert_eq!(lock(&fake.state).count("/token", ""), 0);
    let status = ok("get#OAuthStatus", &[("signInId", text(&sign_in_id))]);
    assert_eq!(out_text(&status, "errorCode"), "denied");

    // another user in this session
    let (started, _) = begin(&[("emailAddress", text("alice@gmail.test"))]);
    let code = code_for(&fake, &started, claims(&started, "alice@gmail.test"), MAIL_SCOPE);
    login("carol");
    let other_user = callback(&[("state", &started["state"]), ("code", &code)]);
    assert_eq!(out_text(&other_user, "errorCode"), "wrongUser", "{other_user:?}");
    login("alice");
    assert_eq!(
        out_text(&callback(&[("state", &started["state"]), ("code", &code)]), "errorCode"),
        "invalidState"
    );

    // too late: ten minutes passed
    let (started, late_id) = begin(&[("emailAddress", text("alice@gmail.test"))]);
    let code = code_for(&fake, &started, claims(&started, "alice@gmail.test"), MAIL_SCOPE);
    let mut entries: serde_json::Value = serde_json::from_str(&session_attribute(PENDING).unwrap()).unwrap();
    entries[&late_id]["expires"] = json!(1);
    set_session_attribute(PENDING, Some(&entries.to_string()));
    assert_eq!(
        out_text(&callback(&[("state", &started["state"]), ("code", &code)]), "errorCode"),
        "expired"
    );

    // another session (this one holds no sign-in)
    let (started, _) = begin(&[("emailAddress", text("alice@gmail.test"))]);
    let code = code_for(&fake, &started, claims(&started, "alice@gmail.test"), MAIL_SCOPE);
    let kept = session_attribute(PENDING);
    set_session_attribute(PENDING, None);
    assert_eq!(
        out_text(&callback(&[("state", &started["state"]), ("code", &code)]), "errorCode"),
        "invalidState"
    );
    set_session_attribute(PENDING, kept.as_deref());
    assert_eq!(lock(&fake.state).count("/token", ""), 0);

    // the provider checks the PKCE verifier: a code made for another challenge is refused
    let (started, _) = begin(&[("emailAddress", text("alice@gmail.test"))]);
    let mut other = started.clone();
    other.insert(
        "code_challenge".to_string(),
        super::oauth::challenge("not-the-verifier"),
    );
    let code = code_for(&fake, &other, claims(&started, "alice@gmail.test"), MAIL_SCOPE);
    let pkce = callback(&[("state", &started["state"]), ("code", &code)]);
    assert_eq!(out_text(&pkce, "errorCode"), "exchangeFailed");
    assert!(out_text(&pkce, "message").contains("PKCE"), "{pkce:?}");

    // the mail scope left out, and an ID token of another sign-in (nonce)
    let (started, _) = begin(&[("emailAddress", text("alice@gmail.test"))]);
    let code = code_for(&fake, &started, claims(&started, "alice@gmail.test"), "openid email");
    let scope = callback(&[("state", &started["state"]), ("code", &code)]);
    assert_eq!(out_text(&scope, "errorCode"), "scopeNotGranted");
    let (started, _) = begin(&[("emailAddress", text("alice@gmail.test"))]);
    let mut wrong_nonce = claims(&started, "alice@gmail.test");
    wrong_nonce["nonce"] = json!("another-nonce");
    let code = code_for(&fake, &started, wrong_nonce, MAIL_SCOPE);
    let nonce = callback(&[("state", &started["state"]), ("code", &code)]);
    assert_eq!(out_text(&nonce, "errorCode"), "exchangeFailed");
    assert!(out_text(&nonce, "message").contains("nonce"), "{nonce:?}");

    // the sign-in: a new account on the provider's servers, the refresh token encrypted on its server
    let (started, sign_in_id) = begin(&[
        ("emailAddress", text("alice@gmail.test")),
        ("accountName", text("Alice Gmail")),
    ]);
    let code = code_for(&fake, &started, claims(&started, "alice@gmail.test"), MAIL_SCOPE);
    let before = lock(&fake.state).count("/token", "authorization_code");
    let done = callback(&[("state", &started["state"]), ("code", &code)]);
    assert_eq!(out_text(&done, "status"), "success", "{done:?}");
    assert_eq!(lock(&fake.state).count("/token", "authorization_code"), before + 1);
    {
        let state = lock(&fake.state);
        let (_, form) = state.requests.iter().rev().find(|(path, _)| path == "/token").unwrap();
        assert_eq!(form["redirect_uri"], REDIRECT);
        assert_eq!(form["client_secret"], SECRET);
        assert_eq!(
            super::oauth::challenge(&form["code_verifier"]),
            started["code_challenge"]
        );
    }
    let account_id = out_text(&done, "emailAccountId");
    let account = one(&facade, ACCOUNT, "emailAccountId", &account_id);
    assert_eq!(field(&account, "emailAddress"), "alice@gmail.test");
    assert_eq!(field(&account, "accountName"), "Alice Gmail");
    assert_eq!(field(&account, "ownerUserId"), "alice");
    assert_eq!(field(&account, "ownsEmailServer"), "Y");
    let server_id = field(&account, "emailServerId");
    let server = one(&facade, SERVER, "emailServerId", &server_id);
    assert_eq!(field(&server, "authTypeEnumId"), "EatOAuth2");
    assert_eq!(field(&server, "oauthProviderEnumId"), "EopTest");
    assert_eq!(field(&server, "mailUsername"), "alice@gmail.test");
    assert_eq!(field(&server, "storeProtocol"), "imap");
    assert_eq!(field(&server, "mailPassword"), "");
    let refresh = field(&server, "oauthRefreshToken");
    assert!(refresh.starts_with("rt-"), "{refresh}");
    assert!(!field(&server, "oauthSignInDate").is_empty());
    // stored encrypted: what the row holds is not the token
    let raw = crate::entity::info::run_sql(
        &facade,
        "transactional",
        "SELECT \"email_server_id\", \"oauth_refresh_token\" FROM \"email_server\"",
        0,
    )
    .unwrap();
    let stored = raw
        .records
        .iter()
        .find(|record| to_plain_string(&record[0]) == server_id)
        .map(|record| to_plain_string(&record[1]))
        .expect("the server row");
    assert!(!stored.is_empty() && !stored.contains(&refresh), "{stored}");
    assert_eq!(
        crate::entity::crypto::decrypt(&stored, &facade.crypt().unwrap().0).unwrap(),
        refresh
    );

    // what a client sees has no token
    let status = ok("get#OAuthStatus", &[("signInId", text(&sign_in_id))]);
    assert_eq!(out_text(&status, "status"), "success");
    assert_eq!(out_text(&status, "emailAccountId"), account_id);
    let account_status = ok("get#OAuthStatus", &[("emailAccountId", text(&account_id))]);
    assert_eq!(account_status.get("signedIn"), Some(&Value::Bool(true)));
    assert_eq!(account_status.get("needsSignIn"), Some(&Value::Bool(false)));
    assert_eq!(account_status.get("oauthConfigured"), Some(&Value::Bool(true)));
    let access = ok("get#EmailAccountAccess", &[("emailAccountId", text(&account_id))]);
    let accounts = ok("get#UserEmailAccounts", &[]);
    for (what, shown) in [
        ("complete#OAuthSignIn", format!("{done:?}")),
        ("get#OAuthStatus", format!("{status:?} {account_status:?}")),
        ("get#EmailAccountAccess", format!("{access:?}")),
        ("get#UserEmailAccounts", format!("{accounts:?}")),
        ("the session", session_attribute(PENDING).unwrap_or_default()),
    ] {
        assert_no_token(&fake, what, &shown);
    }

    // the state of a sign-in that worked is not taken twice
    let again = callback(&[("state", &started["state"]), ("code", &code)]);
    assert_eq!(out_text(&again, "errorCode"), "invalidState");

    // signing in again as another address: refused unless the user confirmed the change
    let (started, _) = begin(&[("emailAccountId", text(&account_id))]);
    assert_eq!(started["login_hint"], "alice@gmail.test");
    let code = code_for(&fake, &started, claims(&started, "bob@gmail.test"), MAIL_SCOPE);
    let mismatch = callback(&[("state", &started["state"]), ("code", &code)]);
    assert_eq!(out_text(&mismatch, "errorCode"), "addressMismatch");
    assert_eq!(out_text(&mismatch, "expectedAddress"), "alice@gmail.test");
    assert_eq!(out_text(&mismatch, "emailAddress"), "bob@gmail.test");
    assert_eq!(
        field(&one(&facade, SERVER, "emailServerId", &server_id), "oauthRefreshToken"),
        refresh
    );
    let (started, _) = begin(&[
        ("emailAccountId", text(&account_id)),
        ("allowAddressChange", Value::Bool(true)),
    ]);
    let code = code_for(&fake, &started, claims(&started, "bob@gmail.test"), MAIL_SCOPE);
    let changed = callback(&[("state", &started["state"]), ("code", &code)]);
    assert_eq!(out_text(&changed, "status"), "success", "{changed:?}");
    assert_eq!(
        field(&one(&facade, ACCOUNT, "emailAccountId", &account_id), "emailAddress"),
        "bob@gmail.test"
    );
    let server = one(&facade, SERVER, "emailServerId", &server_id);
    assert_eq!(field(&server, "mailUsername"), "bob@gmail.test");
    assert_ne!(field(&server, "oauthRefreshToken"), refresh);
    let identities = {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        facade
            .find("moqui.basic.email.EmailIdentity")
            .unwrap()
            .eq("emailAccountId", text(&account_id))
            .list()
            .unwrap()
    };
    assert_eq!(field(&identities.values()[0], "emailAddress"), "bob@gmail.test");

    // another user may not start a sign-in on the account
    login("carol");
    let _ = call("start#OAuthSignIn", &[("emailAccountId", text(&account_id))]);
    assert!(errors().contains("not found"));
    login("alice");

    // the popup's page: the outcome posted to this origin and the channel, a CSP with a nonce, no token
    let _web = crate::web::install(BTreeMap::new());
    let (started, sign_in_id) = begin(&[("emailAccountId", text(&account_id))]);
    let code = code_for(&fake, &started, claims(&started, "bob@gmail.test"), MAIL_SCOPE);
    callback(&[("state", &started["state"]), ("code", &code), ("sendPage", "true")]);
    let (status, headers, body, _) = crate::web::take_output();
    let page = String::from_utf8(body).unwrap();
    assert_eq!(status, 200);
    let header = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    };
    let csp = header("Content-Security-Policy");
    assert!(
        csp.contains("script-src 'nonce-") && csp.contains("default-src 'none'"),
        "{csp}"
    );
    assert_eq!(header("Referrer-Policy"), "no-referrer");
    assert!(page.contains("window.opener.postMessage(m,o)") && page.contains("var o=window.location.origin"));
    assert!(page.contains("new BroadcastChannel('moqui-email-oauth')"));
    assert!(
        page.contains(&sign_in_id) && page.contains("\"status\":\"success\""),
        "{page}"
    );
    assert_no_token(&fake, "the popup's page", &page);
    // an error from the provider is shown escaped
    crate::web::clear_output();
    let (started, _) = begin(&[("emailAccountId", text(&account_id))]);
    callback(&[
        ("state", &started["state"]),
        ("error", "server_error"),
        ("errorDescription", "</script><script>alert(1)</script>"),
        ("sendPage", "true"),
    ]);
    let page = String::from_utf8(crate::web::take_output().2).unwrap();
    assert!(!page.contains("<script>alert(1)"), "{page}");
    assert!(page.contains("&lt;/script&gt;"), "{page}");
}

/// A new account made by a sign-in waits paused for the app to finish its setup, with the year's sync window, so the job
/// does not bring in the whole mailbox before; active at once only with a window; an account signing in again keeps its
/// status and window.
#[test]
fn a_new_account_from_a_sign_in_starts_paused_with_a_sync_window() {
    let _serial = serial();
    let _lock = crate::user::session::cookie_property_lock();
    let _properties = Properties::set();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    user(&facade, "alice");
    login("alice");
    let fake = FakeProvider::start(CLIENT, SECRET, REDIRECT);
    provider_row(&facade, "EopTest", &fake, (993, 465), true);
    let finish = |parameters: &[(&str, Value)], email: &str| {
        let (started, _) = begin(parameters);
        let code = code_for(&fake, &started, claims(&started, email), MAIL_SCOPE);
        let done = callback(&[("state", &started["state"]), ("code", &code)]);
        assert_eq!(out_text(&done, "status"), "success", "{done:?}");
        out_text(&done, "emailAccountId")
    };

    // the default: paused, a year back
    let paused = finish(&[("emailAddress", text("alice@gmail.test"))], "alice@gmail.test");
    let account = one(&facade, ACCOUNT, "emailAccountId", &paused);
    assert_eq!(field(&account, "statusId"), "EasPaused");
    assert_eq!(field(&account, "syncWindowDays"), "365");
    // the job leaves it, and a sync asked for says it is paused
    {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let job = ok("sync#DueEmailAccounts", &[]);
        assert_eq!(out_text(&job, "syncedCount"), "0", "{job:?}");
    }
    let out = call("sync#EmailAccount", &[("emailAccountId", text(&paused))]);
    assert_eq!(out_text(&out, "outcome"), "paused", "{out:?}");
    let _ = errors();
    // signing in again keeps it paused with its window
    let again = sign_in(&fake, "alice@gmail.test", Some(&paused));
    assert_eq!(out_text(&again, "status"), "success", "{again:?}");
    let account = one(&facade, ACCOUNT, "emailAccountId", &paused);
    assert_eq!(field(&account, "statusId"), "EasPaused");
    assert_eq!(field(&account, "syncWindowDays"), "365");
    // the app's setup done: active
    ok(
        "update#EmailAccount",
        &[("emailAccountId", text(&paused)), ("statusId", text("EasActive"))],
    );
    assert_eq!(
        field(&one(&facade, ACCOUNT, "emailAccountId", &paused), "statusId"),
        "EasActive"
    );

    // active at once with a window; all mail only while paused
    let active = finish(
        &[
            ("emailAddress", text("bob@gmail.test")),
            ("statusId", text("EasActive")),
            ("syncWindowDays", Value::I64(30)),
        ],
        "bob@gmail.test",
    );
    let account = one(&facade, ACCOUNT, "emailAccountId", &active);
    assert_eq!(field(&account, "statusId"), "EasActive");
    assert_eq!(field(&account, "syncWindowDays"), "30");
    let all = finish(
        &[
            ("emailAddress", text("carol@gmail.test")),
            ("syncWindowDays", Value::I64(0)),
        ],
        "carol@gmail.test",
    );
    let account = one(&facade, ACCOUNT, "emailAccountId", &all);
    assert_eq!(field(&account, "statusId"), "EasPaused");
    assert_eq!(field(&account, "syncWindowDays"), "");

    // refused at the start: active with all mail, another status, a window that is not a number of days
    for (parameters, expected) in [
        (
            vec![("statusId", text("EasActive")), ("syncWindowDays", Value::I64(0))],
            "needs a sync window",
        ),
        (vec![("statusId", text("EasRemoved"))], "Paused or Active"),
        (vec![("syncWindowDays", Value::I64(-1))], "number of days"),
    ] {
        let mut parameters = parameters;
        parameters.push(("emailAddress", text("dee@gmail.test")));
        let out = call("start#OAuthSignIn", &parameters);
        assert!(out_text(&out, "authUrl").is_empty(), "{out:?}");
        let errors = errors();
        assert!(errors.contains(expected), "{expected}: {errors}");
    }
    // create#EmailAccount takes the status too
    let made = call(
        "create#EmailAccount",
        &[
            ("emailAddress", text("eve@example.test")),
            ("statusId", text("EasRemoved")),
        ],
    );
    assert!(made.get("emailAccountId").is_none_or(Value::is_null), "{made:?}");
    assert!(errors().contains("Active or Paused"));
}

/// The refresh: once at a time for concurrent callers, again when the token is about to expire, a rotated refresh token
/// kept; a refresh token the provider refuses marks only its account, which the job then leaves while it syncs the
/// others, and a new sign-in makes it active again.
#[test]
fn refresh_once_at_a_time_and_a_revoked_sign_in_marks_only_its_account() {
    let _serial = serial();
    let _lock = crate::user::session::cookie_property_lock();
    let _properties = Properties::set();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    user(&facade, "alice");
    login("alice");
    let fake = FakeProvider::start(CLIENT, SECRET, REDIRECT);
    let imap = FakeImap::start(&["UIDPLUS"]);
    lock(&imap.state).user = "alice@gmail.test".to_string();
    provider_row(&facade, "EopTest", &fake, (imap.port, 2525), true);
    let done = sign_in(&fake, "alice@gmail.test", None);
    assert_eq!(out_text(&done, "status"), "success", "{done:?}");
    let account_id = out_text(&done, "emailAccountId");
    let server_id = field(&one(&facade, ACCOUNT, "emailAccountId", &account_id), "emailServerId");

    // four callers at once with no token kept: one refresh
    super::oauth::forget(&facade, &server_id);
    lock(&fake.state).refresh_delay_ms = 300;
    let got: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let facade = facade.clone();
                let server_id = server_id.clone();
                scope.spawn(move || super::oauth::access_token(&facade, &server_id, None).unwrap())
            })
            .collect();
        handles.into_iter().map(|handle| handle.join().unwrap()).collect()
    });
    assert_eq!(lock(&fake.state).count("/token", "refresh_token"), 1);
    assert!(got.iter().all(|token| *token == got[0]), "{got:?}");
    lock(&fake.state).refresh_delay_ms = 0;
    // kept: no refresh; about to expire: refreshed
    assert_eq!(super::oauth::access_token(&facade, &server_id, None).unwrap(), got[0]);
    assert_eq!(lock(&fake.state).count("/token", "refresh_token"), 1);
    super::oauth::age_cached(&facade, &server_id);
    let renewed = super::oauth::access_token(&facade, &server_id, None).unwrap();
    assert_ne!(renewed, got[0]);
    assert_eq!(lock(&fake.state).count("/token", "refresh_token"), 2);
    // a token a server refused is replaced
    let replaced = super::oauth::access_token(&facade, &server_id, Some(&renewed)).unwrap();
    assert_ne!(replaced, renewed);

    // a provider that rotates refresh tokens: the new one is stored and used next
    lock(&fake.state).rotate = true;
    let first_refresh = field(&one(&facade, SERVER, "emailServerId", &server_id), "oauthRefreshToken");
    super::oauth::forget(&facade, &server_id);
    super::oauth::access_token(&facade, &server_id, None).unwrap();
    let rotated = field(&one(&facade, SERVER, "emailServerId", &server_id), "oauthRefreshToken");
    assert_ne!(rotated, first_refresh);
    lock(&fake.state).refresh_tokens.retain(|token| *token != first_refresh);
    lock(&fake.state).rotate = false;
    super::oauth::forget(&facade, &server_id);
    super::oauth::access_token(&facade, &server_id, None).unwrap();

    // a second account with a password, on the same IMAP server
    lock(&imap.state).password = "app-password".to_string();
    let other = out_text(
        &ok(
            "create#EmailAccount",
            &[
                ("emailAddress", text("alice@gmail.test")),
                ("accountName", text("Alice by password")),
                ("storeHost", text("127.0.0.1")),
                ("storePort", text(&imap.port.to_string())),
                ("storeProtocol", text("imap")),
                ("smtpHost", text("127.0.0.1")),
                ("mailUsername", text("alice@gmail.test")),
                ("mailPassword", text("app-password")),
            ],
        ),
        "emailAccountId",
    );

    // the sign-in is revoked at the provider: the sync marks the account to sign in again, and only it
    lock(&fake.state).refresh_tokens.clear();
    super::oauth::forget(&facade, &server_id);
    let out = call("sync#EmailAccount", &[("emailAccountId", text(&account_id))]);
    assert_eq!(out_text(&out, "outcome"), "authFailed", "{out:?}");
    assert!(errors().contains("sign in again"));
    let account = one(&facade, ACCOUNT, "emailAccountId", &account_id);
    assert_eq!(field(&account, "statusId"), "EasAuthFailed");
    assert!(
        field(&account, "lastSyncError").contains("sign in again"),
        "{account:?}"
    );
    let status = ok("get#OAuthStatus", &[("emailAccountId", text(&account_id))]);
    assert_eq!(status.get("needsSignIn"), Some(&Value::Bool(true)));
    // the job leaves it and syncs the other one
    {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let job = ok("sync#DueEmailAccounts", &[]);
        assert_eq!(out_text(&job, "syncedCount"), "1", "{job:?}");
    }
    assert_eq!(
        field(&one(&facade, ACCOUNT, "emailAccountId", &other), "statusId"),
        "EasActive"
    );
    // a send through it says why it did not go, and is not tried with a password
    let refused = super::oauth::access_token(&facade, &server_id, None);
    assert!(
        matches!(refused, Err(super::oauth::TokenError::SignIn(_))),
        "{refused:?}"
    );

    // signing in again (the Inbox rail's Fix) makes it active
    let again = sign_in(&fake, "alice@gmail.test", Some(&account_id));
    assert_eq!(out_text(&again, "status"), "success", "{again:?}");
    assert_eq!(
        field(&one(&facade, ACCOUNT, "emailAccountId", &account_id), "statusId"),
        "EasActive"
    );
}

/// The access token stored for the other servers of a cluster: encrypted; a server with none in memory uses it with no
/// refresh; one a server refused is refreshed; with a provider that rotates refresh tokens the new one is on the
/// EmailServer and its digest on the stored token; forgetting the sign-in deletes it.
#[test]
fn the_access_token_is_stored_for_the_other_servers_of_a_cluster() {
    let _serial = serial();
    let _lock = crate::user::session::cookie_property_lock();
    let _properties = Properties::set();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    user(&facade, "alice");
    login("alice");
    let fake = FakeProvider::start(CLIENT, SECRET, REDIRECT);
    provider_row(&facade, "EopTest", &fake, (993, 465), true);
    let done = sign_in(&fake, "alice@gmail.test", None);
    assert_eq!(out_text(&done, "status"), "success", "{done:?}");
    let account_id = out_text(&done, "emailAccountId");
    let server_id = field(&one(&facade, ACCOUNT, "emailAccountId", &account_id), "emailServerId");
    let refreshes = || lock(&fake.state).count("/token", "refresh_token");

    // a refresh stores the token, encrypted
    super::oauth::forget(&facade, &server_id);
    let first = super::oauth::access_token(&facade, &server_id, None).unwrap();
    assert_eq!(refreshes(), 1);
    let raw = crate::entity::info::run_sql(
        &facade,
        "transactional",
        "SELECT \"email_server_id\", \"access_token\" FROM \"email_server_access_token\"",
        0,
    )
    .unwrap();
    let stored = raw
        .records
        .iter()
        .find(|record| to_plain_string(&record[0]) == server_id)
        .map(|record| to_plain_string(&record[1]))
        .expect("the stored token");
    assert!(!stored.is_empty() && !stored.contains(&first), "{stored}");
    assert_eq!(
        crate::entity::crypto::decrypt(&stored, &facade.crypt().unwrap().0).unwrap(),
        first
    );

    // another server of the cluster, with nothing in memory: the stored token, no refresh
    super::oauth::forget_local(&server_id);
    assert_eq!(super::oauth::access_token(&facade, &server_id, None).unwrap(), first);
    assert_eq!(refreshes(), 1);

    // a token a server refused is refreshed, though stored, and the new one stored
    super::oauth::forget_local(&server_id);
    let replaced = super::oauth::access_token(&facade, &server_id, Some(&first)).unwrap();
    assert_ne!(replaced, first);
    assert_eq!(refreshes(), 2);
    super::oauth::forget_local(&server_id);
    assert_eq!(super::oauth::access_token(&facade, &server_id, None).unwrap(), replaced);
    assert_eq!(refreshes(), 2);

    // a provider that rotates refresh tokens: the new one on the EmailServer, its digest on the stored token
    lock(&fake.state).rotate = true;
    let used = field(&one(&facade, SERVER, "emailServerId", &server_id), "oauthRefreshToken");
    super::oauth::age_cached(&facade, &server_id);
    let renewed = super::oauth::access_token(&facade, &server_id, None).unwrap();
    assert_ne!(renewed, replaced);
    assert_eq!(refreshes(), 3);
    lock(&fake.state).rotate = false;
    let rotated = field(&one(&facade, SERVER, "emailServerId", &server_id), "oauthRefreshToken");
    assert!(rotated.starts_with("rt-") && rotated != used, "{rotated}");
    let row = one(&facade, TOKEN, "emailServerId", &server_id);
    assert_eq!(field(&row, "accessToken"), renewed);
    let digest = moqui_lib::codec::hex_encode(&<sha2::Sha256 as sha2::Digest>::digest(rotated.as_bytes())[..16]);
    assert_eq!(field(&row, "refreshTokenDigest"), digest);
    // another server uses it with the rotated refresh token, and no refresh
    super::oauth::forget_local(&server_id);
    assert_eq!(super::oauth::access_token(&facade, &server_id, None).unwrap(), renewed);
    assert_eq!(refreshes(), 3);

    // forgetting the sign-in deletes the stored token
    super::oauth::forget(&facade, &server_id);
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let gone = facade
        .find(TOKEN)
        .unwrap()
        .eq("emailServerId", text(&server_id))
        .use_cache(false)
        .one()
        .unwrap();
    assert!(gone.is_none(), "{gone:?}");
}

/// XOAUTH2 for the sync (no LOGIN), the server test, and a send through the account; an access token the IMAP server
/// refuses is replaced by a new one once.
#[test]
fn xoauth2_for_the_sync_the_server_test_and_a_send() {
    let _serial = serial();
    let _lock = crate::user::session::cookie_property_lock();
    let _properties = Properties::set();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    user(&facade, "alice");
    login("alice");
    let fake = FakeProvider::start(CLIENT, SECRET, REDIRECT);
    let imap = FakeImap::start(&["UIDPLUS", "MOVE"]);
    let smtp = FakeSmtp::start("alice@gmail.test");
    lock(&imap.state).user = "alice@gmail.test".to_string();
    imap.add(
        "INBOX",
        "Message-ID: <x1@example.com>\nFrom: bob@example.com\nTo: alice@gmail.test\nSubject: Hello\nDate: Mon, 19 Oct 2026 10:00:00 +0000\n\nHi\n",
        &[],
    );
    provider_row(&facade, "EopTest", &fake, (imap.port, smtp.port), true);
    let done = sign_in(&fake, "alice@gmail.test", None);
    assert_eq!(out_text(&done, "status"), "success", "{done:?}");
    let account_id = out_text(&done, "emailAccountId");
    let access = lock(&fake.state).access_tokens.last().cloned().unwrap();
    lock(&imap.state).oauth_tokens.push(access.clone());
    lock(&smtp.state).tokens.push(access.clone());

    let synced = ok("sync#EmailAccount", &[("emailAccountId", text(&account_id))]);
    assert_eq!(out_text(&synced, "outcome"), "synced", "{synced:?}");
    assert_eq!(out_text(&synced, "newCount"), "1");
    let commands = imap.commands();
    assert!(
        commands.iter().any(|command| command == "AUTHENTICATE XOAUTH2"),
        "{commands:?}"
    );
    assert!(!commands.iter().any(|command| command == "LOGIN"), "{commands:?}");
    assert_no_token(&fake, "the IMAP log", &format!("{commands:?}"));

    let tested = ok("test#EmailAccount", &[("emailAccountId", text(&account_id))]);
    assert_eq!(tested.get("success"), Some(&Value::Bool(true)), "{tested:?}");
    assert_eq!(lock(&smtp.state).auths, vec!["XOAUTH2 ok".to_string()]);

    // a send through the account
    row(
        &facade,
        "moqui.basic.email.EmailMessage",
        &[
            ("emailMessageId", text("OAUTH_SEND")),
            ("emailAccountId", text(&account_id)),
            ("statusId", text("ES_READY")),
            ("fromAddress", text("alice@gmail.test")),
            ("toAddresses", text("bob@example.com")),
            ("subject", text("Sent with a token")),
            ("bodyText", text("Hello Bob")),
        ],
    );
    let sent = ok(
        "org.moqui.impl.EmailServices.send#EmailMessage",
        &[("emailMessageId", text("OAUTH_SEND"))],
    );
    assert!(!out_text(&sent, "messageId").is_empty(), "{sent:?}");
    {
        let state = lock(&smtp.state);
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].0, "alice@gmail.test");
        assert_eq!(state.messages[0].1, vec!["bob@example.com".to_string()]);
        assert!(state.messages[0].2.contains("Sent with a token"));
    }

    // the servers stop taking the token (revoked early): the sync and a send get a new one and go on
    let next = format!("at-{}", lock(&fake.state).next_number());
    {
        let mut state = lock(&imap.state);
        state.oauth_tokens.clear();
        state.oauth_tokens.push(next.clone());
    }
    imap.clear_log();
    let synced = ok("sync#EmailAccount", &[("emailAccountId", text(&account_id))]);
    assert_eq!(out_text(&synced, "outcome"), "synced", "{synced:?}");
    let renewed = lock(&fake.state).access_tokens.last().cloned().unwrap();
    assert_eq!(renewed, next, "the fake's next access token");
    assert_eq!(
        imap.commands()
            .iter()
            .filter(|command| *command == "AUTHENTICATE XOAUTH2")
            .count(),
        2
    );
    // the SMTP server refuses the kept token (535): the send gets a new one and goes
    let next = format!("at-{}", lock(&fake.state).next_number());
    {
        let mut state = lock(&smtp.state);
        state.tokens.clear();
        state.tokens.push(next.clone());
        state.auths.clear();
    }
    row(
        &facade,
        "moqui.basic.email.EmailMessage",
        &[
            ("emailMessageId", text("OAUTH_SEND_2")),
            ("emailAccountId", text(&account_id)),
            ("statusId", text("ES_READY")),
            ("fromAddress", text("alice@gmail.test")),
            ("toAddresses", text("carol@example.com")),
            ("subject", text("Second")),
            ("bodyText", text("Hello Carol")),
        ],
    );
    let sent = ok(
        "org.moqui.impl.EmailServices.send#EmailMessage",
        &[("emailMessageId", text("OAUTH_SEND_2"))],
    );
    assert!(!out_text(&sent, "messageId").is_empty(), "{sent:?}");
    assert_eq!(lock(&fake.state).access_tokens.last(), Some(&next));
    let state = lock(&smtp.state);
    assert_eq!(
        state.auths,
        vec!["XOAUTH2 refused".to_string(), "XOAUTH2 ok".to_string()]
    );
    assert_eq!(state.messages.len(), 2);
}

/// A POP3 account signs in with AUTH XOAUTH2 (no USER or PASS): its sign-in asks for the provider's POP scope and
/// refuses a grant without it, its refreshes ask for it too, the server test signs in the same way, and a token the server
/// refuses is replaced by a new one once.
#[test]
fn pop3_signs_in_with_xoauth2_and_the_pop_scope() {
    const POP_SCOPE: &str = "https://mail.example.test/pop";
    let _serial = serial();
    let _lock = crate::user::session::cookie_property_lock();
    let _properties = Properties::set();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    user(&facade, "alice");
    login("alice");
    let fake = FakeProvider::start(CLIENT, SECRET, REDIRECT);
    let imap = FakeImap::start(&[]);
    let smtp = FakeSmtp::start("alice@gmail.test");
    let pop = FakePop::start();
    lock(&pop.oauth).0 = "alice@gmail.test".to_string();
    pop.add(
        "u1",
        "Message-ID: <p1@example.com>\nFrom: bob@example.com\nTo: alice@gmail.test\nSubject: One\nDate: Mon, 19 Oct 2026 10:00:00 +0000\n\nHi\n",
    );
    provider_row(&facade, "EopTest", &fake, (imap.port, smtp.port), true);
    update(
        &facade,
        PROVIDER,
        ("oauthProviderEnumId", "EopTest"),
        &[("popScope", text(POP_SCOPE)), ("refreshParams", text("scope={scope}"))],
    );
    // an account signed in (over IMAP), whose server then reads POP3
    let done = sign_in(&fake, "alice@gmail.test", None);
    assert_eq!(out_text(&done, "status"), "success", "{done:?}");
    let account_id = out_text(&done, "emailAccountId");
    ok(
        "update#EmailAccount",
        &[
            ("emailAccountId", text(&account_id)),
            ("storeHost", text("127.0.0.1")),
            ("storePort", text(&pop.port.to_string())),
            ("storeProtocol", text("pop3")),
        ],
    );
    // its sign-in asks for the POP scope, and a grant without it is refused
    let parameters = [
        ("oauthProviderEnumId", text("EopTest")),
        ("emailAccountId", text(&account_id)),
    ];
    let (started, _) = begin(&parameters);
    assert!(
        started["scope"].split(' ').any(|scope| scope == POP_SCOPE),
        "{started:?}"
    );
    let code = code_for(&fake, &started, claims(&started, "alice@gmail.test"), MAIL_SCOPE);
    let refused = callback(&[("state", &started["state"]), ("code", &code)]);
    assert_eq!(out_text(&refused, "errorCode"), "scopeNotGranted", "{refused:?}");
    let (started, _) = begin(&parameters);
    let granted = format!("{MAIL_SCOPE} {POP_SCOPE}");
    let code = code_for(&fake, &started, claims(&started, "alice@gmail.test"), &granted);
    let done = callback(&[("state", &started["state"]), ("code", &code)]);
    assert_eq!(out_text(&done, "status"), "success", "{done:?}");
    let access = lock(&fake.state).access_tokens.last().cloned().unwrap();
    lock(&pop.oauth).1.push(access);

    let synced = ok("sync#EmailAccount", &[("emailAccountId", text(&account_id))]);
    assert_eq!(out_text(&synced, "outcome"), "synced", "{synced:?}");
    assert_eq!(out_text(&synced, "newCount"), "1");
    let log = lock(&pop.log).clone();
    assert!(log.iter().any(|line| line == "AUTH XOAUTH2"), "{log:?}");
    assert!(log.iter().any(|line| line == "XOAUTH2 ok"), "{log:?}");
    assert!(
        !log.iter()
            .any(|line| line.starts_with("USER") || line.starts_with("PASS")),
        "{log:?}"
    );
    assert_no_token(&fake, "the POP3 log", &format!("{log:?}"));

    let tested = ok(
        "test#EmailAccount",
        &[("emailAccountId", text(&account_id)), ("testSmtp", Value::Bool(false))],
    );
    assert_eq!(tested.get("success"), Some(&Value::Bool(true)), "{tested:?}");
    assert!(format!("{tested:?}").contains("with XOAUTH2"), "{tested:?}");

    // the server stops taking the token: a new one (asking for the POP scope) and the sync goes on
    let next = format!("at-{}", lock(&fake.state).next_number());
    {
        let mut oauth = lock(&pop.oauth);
        oauth.1.clear();
        oauth.1.push(next.clone());
    }
    pop.add(
        "u2",
        "Message-ID: <p2@example.com>\nFrom: bob@example.com\nTo: alice@gmail.test\nSubject: Two\nDate: Mon, 19 Oct 2026 11:00:00 +0000\n\nHi again\n",
    );
    let synced = ok("sync#EmailAccount", &[("emailAccountId", text(&account_id))]);
    assert_eq!(out_text(&synced, "outcome"), "synced", "{synced:?}");
    assert_eq!(out_text(&synced, "newCount"), "1");
    assert_eq!(lock(&fake.state).access_tokens.last(), Some(&next));
    let refresh_scope = lock(&fake.state)
        .requests
        .iter()
        .rev()
        .find(|(_, form)| form.get("grant_type").map(String::as_str) == Some("refresh_token"))
        .and_then(|(_, form)| form.get("scope").cloned())
        .unwrap_or_default();
    assert!(
        refresh_scope.split(' ').any(|scope| scope == POP_SCOPE),
        "{refresh_scope}"
    );
    let log = lock(&pop.log).clone();
    assert!(log.iter().any(|line| line == "XOAUTH2 refused"), "{log:?}");
}

/// A new account from a sign-in with storeProtocol pop3: the sign-in asks for the POP scope, nothing is made until it
/// succeeds (a grant without the POP scope makes nothing), and the account is on the provider's POP3 server, where the
/// sync signs in with XOAUTH2. A provider with no POP server refuses it; an unknown protocol is refused.
#[test]
fn a_new_pop3_account_from_a_sign_in_is_made_on_the_providers_pop_server() {
    const POP_SCOPE: &str = "https://mail.example.test/pop";
    let _serial = serial();
    let _lock = crate::user::session::cookie_property_lock();
    let _properties = Properties::set();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    user(&facade, "alice");
    login("alice");
    let fake = FakeProvider::start(CLIENT, SECRET, REDIRECT);
    let imap = FakeImap::start(&[]);
    let smtp = FakeSmtp::start("alice@gmail.test");
    let pop = FakePop::start();
    lock(&pop.oauth).0 = "alice@gmail.test".to_string();
    pop.add(
        "u1",
        "Message-ID: <np1@example.com>\nFrom: bob@example.com\nTo: alice@gmail.test\nSubject: One\nDate: Mon, 19 Oct 2026 10:00:00 +0000\n\nHi\n",
    );
    provider_row(&facade, "EopTest", &fake, (imap.port, smtp.port), true);
    let start = |protocol: &str| {
        vec![
            ("oauthProviderEnumId", text("EopTest")),
            ("emailAddress", text("alice@gmail.test")),
            ("statusId", text("EasActive")),
            ("storeProtocol", text(protocol)),
        ]
    };
    // no POP server on the provider: refused before the popup opens
    call("start#OAuthSignIn", &start("pop3"));
    assert!(errors().contains("no POP server"), "a provider without popHost");
    call("start#OAuthSignIn", &start("smtp"));
    assert!(errors().contains("imap or pop3"));
    update(
        &facade,
        PROVIDER,
        ("oauthProviderEnumId", "EopTest"),
        &[
            ("popScope", text(POP_SCOPE)),
            ("popHost", text("127.0.0.1")),
            ("popPort", text(&pop.port.to_string())),
            ("popProtocol", text("pop3")),
        ],
    );
    let providers = ok("get#OAuthProviders", &[]);
    let listed = format!("{providers:?}");
    assert!(listed.contains(&pop.port.to_string()), "{listed}");
    let accounts = || {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        facade
            .find(ACCOUNT)
            .unwrap()
            .eq("emailAddress", text("alice@gmail.test"))
            .use_cache(false)
            .list()
            .unwrap()
    };
    let (started, _) = begin(&start("pop3"));
    assert!(
        started["scope"].split(' ').any(|scope| scope == POP_SCOPE),
        "{started:?}"
    );
    assert!(accounts().is_empty(), "nothing is made before the sign-in");
    // a grant without the POP scope is refused and makes nothing
    let code = code_for(&fake, &started, claims(&started, "alice@gmail.test"), MAIL_SCOPE);
    let refused = callback(&[("state", &started["state"]), ("code", &code)]);
    assert_eq!(out_text(&refused, "errorCode"), "scopeNotGranted", "{refused:?}");
    assert!(accounts().is_empty(), "a refused sign-in makes no account");
    let (started, _) = begin(&start("pop3"));
    let code = code_for(
        &fake,
        &started,
        claims(&started, "alice@gmail.test"),
        &format!("{MAIL_SCOPE} {POP_SCOPE}"),
    );
    let done = callback(&[("state", &started["state"]), ("code", &code)]);
    assert_eq!(out_text(&done, "status"), "success", "{done:?}");
    let account_id = out_text(&done, "emailAccountId");
    assert_eq!(accounts().len(), 1);
    let account = one(&facade, ACCOUNT, "emailAccountId", &account_id);
    let server = one(&facade, SERVER, "emailServerId", &field(&account, "emailServerId"));
    assert_eq!(field(&server, "storeProtocol"), "pop3");
    assert_eq!(field(&server, "storeHost"), "127.0.0.1");
    assert_eq!(field(&server, "storePort"), pop.port.to_string());
    assert_eq!(field(&server, "smtpPort"), smtp.port.to_string());
    assert_eq!(field(&server, "authTypeEnumId"), "EatOAuth2");
    let access = lock(&fake.state).access_tokens.last().cloned().unwrap();
    lock(&pop.oauth).1.push(access);
    let synced = ok("sync#EmailAccount", &[("emailAccountId", text(&account_id))]);
    assert_eq!(out_text(&synced, "outcome"), "synced", "{synced:?}");
    assert_eq!(out_text(&synced, "newCount"), "1");
    let log = lock(&pop.log).clone();
    assert!(log.iter().any(|line| line == "XOAUTH2 ok"), "{log:?}");
}

/// Removing an account revokes its refresh token at the provider and deletes it here; not when another server here
/// signs in as the same address (the provider revokes the whole grant); a provider with no revocation endpoint is not
/// asked; signing out and going back to a password revoke too.
#[test]
fn remove_sign_out_and_a_password_revoke_the_sign_in() {
    let _serial = serial();
    let _lock = crate::user::session::cookie_property_lock();
    let _properties = Properties::set();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    user(&facade, "alice");
    login("alice");
    let fake = FakeProvider::start(CLIENT, SECRET, REDIRECT);
    provider_row(&facade, "EopTest", &fake, (993, 465), true);
    let server_of = |account: &str| field(&one(&facade, ACCOUNT, "emailAccountId", account), "emailServerId");
    let refresh_of = |account: &str| {
        field(
            &one(&facade, SERVER, "emailServerId", &server_of(account)),
            "oauthRefreshToken",
        )
    };

    // two accounts signed in as the same address: removing one does not revoke the other's grant
    let first = out_text(&sign_in(&fake, "alice@gmail.test", None), "emailAccountId");
    let second = out_text(&sign_in(&fake, "alice@gmail.test", None), "emailAccountId");
    let first_refresh = refresh_of(&first);
    ok("delete#EmailAccount", &[("emailAccountId", text(&first))]);
    assert!(lock(&fake.state).revoked.is_empty());
    assert_eq!(refresh_of(&first), "");
    assert_eq!(
        field(&one(&facade, ACCOUNT, "emailAccountId", &first), "statusId"),
        "EasRemoved"
    );
    assert!(!first_refresh.is_empty());

    // the last one: revoked at the provider after the commit, and deleted here with the local copies
    let second_refresh = refresh_of(&second);
    let second_server = server_of(&second);
    ok(
        "delete#EmailAccount",
        &[
            ("emailAccountId", text(&second)),
            ("deleteLocalCopies", Value::Bool(true)),
        ],
    );
    assert_eq!(lock(&fake.state).revoked, vec![second_refresh.clone()]);
    assert_eq!(lock(&fake.state).count("/revoke", ""), 1);
    {
        let state = lock(&fake.state);
        let (_, form) = state.requests.iter().find(|(path, _)| path == "/revoke").unwrap();
        assert_eq!(form["token_type_hint"], "refresh_token");
        assert_eq!(form["client_id"], CLIENT);
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    assert!(facade
        .find(SERVER)
        .unwrap()
        .eq("emailServerId", text(&second_server))
        .one()
        .unwrap()
        .is_none());
    drop(_authz);
    assert!(super::oauth::access_token(&facade, &second_server, None).is_err());

    // signing out: revoked there, deleted here, the account waits for a new sign-in
    let third = out_text(&sign_in(&fake, "alice@gmail.test", None), "emailAccountId");
    let third_refresh = refresh_of(&third);
    let out = ok("revoke#OAuthSignIn", &[("emailAccountId", text(&third))]);
    assert_eq!(out.get("revoked"), Some(&Value::Bool(true)));
    assert!(lock(&fake.state).revoked.contains(&third_refresh));
    assert_eq!(refresh_of(&third), "");
    let account = one(&facade, ACCOUNT, "emailAccountId", &third);
    assert_eq!(field(&account, "statusId"), "EasAuthFailed");
    assert!(field(&account, "lastSyncError").contains("Signed out"));
    let status = ok("get#OAuthStatus", &[("emailAccountId", text(&third))]);
    assert_eq!(status.get("signedIn"), Some(&Value::Bool(false)));
    assert_eq!(status.get("needsSignIn"), Some(&Value::Bool(true)));

    // back to a password: revoked and deleted
    let again = sign_in(&fake, "alice@gmail.test", Some(&third));
    assert_eq!(out_text(&again, "status"), "success");
    let fourth_refresh = refresh_of(&third);
    ok(
        "update#EmailAccount",
        &[
            ("emailAccountId", text(&third)),
            ("authTypeEnumId", text("EatPassword")),
            ("mailPassword", text("app-password")),
        ],
    );
    assert!(lock(&fake.state).revoked.contains(&fourth_refresh));
    let server = one(&facade, SERVER, "emailServerId", &server_of(&third));
    assert_eq!(field(&server, "oauthRefreshToken"), "");
    assert_eq!(field(&server, "authTypeEnumId"), "EatPassword");

    // a provider with no revocation endpoint (Microsoft): deleted here only
    update(
        &facade,
        PROVIDER,
        ("oauthProviderEnumId", "EopTest"),
        &[("revokeUrl", Value::Null)],
    );
    let fifth = out_text(&sign_in(&fake, "alice@gmail.test", None), "emailAccountId");
    let revokes = lock(&fake.state).count("/revoke", "");
    ok("delete#EmailAccount", &[("emailAccountId", text(&fifth))]);
    assert_eq!(lock(&fake.state).count("/revoke", ""), revokes);
    assert_eq!(refresh_of(&fifth), "");
    // the internal service is for internal callers only
    let _ = call("revoke#OAuthToken", &[("emailServerId", text("anything"))]);
    assert!(errors().contains("internal"));
}

/// discover#EmailServer names the provider by the address's domain, by the domain's MX hosts (a company domain), and
/// says whether this server is set up for it; the provider's servers replace the guesses.
#[test]
fn discover_names_the_provider_by_domain_or_mx() {
    let _serial = serial();
    let _lock = crate::user::session::cookie_property_lock();
    let _properties = Properties::set();
    let http = fake_http(HashMap::new());
    let mut mx = HashMap::new();
    mx.insert("acme.test".to_string(), vec!["aspmx.l.google.com".to_string()]);
    mx.insert(
        "contoso.test".to_string(),
        vec!["contoso-test.mail.protection.outlook.com".to_string()],
    );
    let dns = fake_dns_with_mx(HashMap::new(), mx);
    let (ctx, facade) = setup_with(&format!(
        r#"<moqui-conf><email-client autoconfig-urls="http://127.0.0.1:{http}/{{domain}}/config-v1.1.xml"
            dns-server="127.0.0.1:{dns}"/></moqui-conf>"#
    ));
    let _entered = ctx.enter();
    install();
    user(&facade, "alice");
    login("alice");
    let fake = FakeProvider::start(CLIENT, SECRET, REDIRECT);
    provider_row(&facade, "EopTest", &fake, (993, 465), true);
    update(
        &facade,
        PROVIDER,
        ("oauthProviderEnumId", "EopTest"),
        &[
            ("storeHost", text("imap.gmail.test")),
            ("smtpHost", text("smtp.gmail.test")),
            ("mxSuffixes", text("google.com,googlemail.com")),
        ],
    );
    row(
        &facade,
        PROVIDER,
        &[
            ("oauthProviderEnumId", text("EopOther")),
            ("authorizeUrl", text("https://login.example.test/authorize")),
            ("tokenUrl", text("https://login.example.test/token")),
            ("mxSuffixes", text("protection.outlook.com")),
            ("storeHost", text("outlook.office365.test")),
        ],
    );

    let started = Instant::now();
    let gmail = ok("discover#EmailServer", &[("emailAddress", text("ann@gmail.test"))]);
    assert_eq!(out_text(&gmail, "oauthProviderEnumId"), "EopTest", "{gmail:?}");
    assert_eq!(gmail.get("oauthConfigured"), Some(&Value::Bool(true)));
    assert_eq!(out_text(&gmail, "source"), "provider");
    assert_eq!(out_text(&gmail, "storeHost"), "imap.gmail.test");
    let workspace = ok("discover#EmailServer", &[("emailAddress", text("bo@acme.test"))]);
    assert_eq!(out_text(&workspace, "oauthProviderEnumId"), "EopTest", "{workspace:?}");
    assert_eq!(out_text(&workspace, "smtpHost"), "smtp.gmail.test");
    let microsoft = ok("discover#EmailServer", &[("emailAddress", text("cy@contoso.test"))]);
    assert_eq!(out_text(&microsoft, "oauthProviderEnumId"), "EopOther", "{microsoft:?}");
    assert_eq!(microsoft.get("oauthConfigured"), Some(&Value::Bool(false)));
    let plain = ok("discover#EmailServer", &[("emailAddress", text("dee@none.test"))]);
    assert_eq!(out_text(&plain, "oauthProviderEnumId"), "");
    assert_eq!(out_text(&plain, "source"), "guess");
    assert!(started.elapsed() < Duration::from_secs(15), "{:?}", started.elapsed());

    // with no network: the providers' domains and suffixes, and find#OAuthProvider by domain or host (MX only when asked)
    let providers = ok("get#OAuthProviders", &[]);
    let list = format!("{:?}", providers.get("providerList"));
    for expected in [
        "addressDomainList",
        "\"gmail.test\"",
        "\"google.com\"",
        "\"protection.outlook.com\"",
    ] {
        assert!(list.contains(expected), "{expected} in {list}");
    }
    assert!(!list.contains("secret"), "{list}");
    let find = |parameters: &[(&str, Value)]| ok("find#OAuthProvider", parameters);
    let by_domain = find(&[("emailAddress", text("Ann@Gmail.test"))]);
    assert_eq!(out_text(&by_domain, "oauthProviderEnumId"), "EopTest", "{by_domain:?}");
    assert_eq!(out_text(&by_domain, "matchedBy"), "domain");
    assert_eq!(by_domain.get("configured"), Some(&Value::Bool(true)));
    let by_host = find(&[
        ("emailAddress", text("cy@contoso.test")),
        ("storeHost", text("outlook.office365.test")),
    ]);
    assert_eq!(out_text(&by_host, "oauthProviderEnumId"), "", "{by_host:?}");
    update(
        &facade,
        PROVIDER,
        ("oauthProviderEnumId", "EopOther"),
        &[("hostSuffixes", text("office365.test"))],
    );
    let by_host = find(&[
        ("emailAddress", text("cy@contoso.test")),
        ("storeHost", text("Outlook.Office365.test")),
    ]);
    assert_eq!(out_text(&by_host, "oauthProviderEnumId"), "EopOther", "{by_host:?}");
    assert_eq!(out_text(&by_host, "matchedBy"), "host");
    assert_eq!(by_host.get("configured"), Some(&Value::Bool(false)));
    let no_mx = find(&[("emailAddress", text("bo@acme.test"))]);
    assert_eq!(out_text(&no_mx, "oauthProviderEnumId"), "", "{no_mx:?}");
    let by_mx = find(&[("emailAddress", text("bo@acme.test")), ("lookupMx", Value::Bool(true))]);
    assert_eq!(out_text(&by_mx, "oauthProviderEnumId"), "EopTest", "{by_mx:?}");
    assert_eq!(out_text(&by_mx, "matchedBy"), "mx");
    let none = find(&[("emailAddress", text("dee@none.test")), ("lookupMx", Value::Bool(true))]);
    assert_eq!(out_text(&none, "oauthProviderEnumId"), "", "{none:?}");
    assert_eq!(none.get("configured"), Some(&Value::Bool(false)));

    // a provider without a client is not offered at the start
    let _ = call(
        "start#OAuthSignIn",
        &[
            ("emailAddress", text("cy@contoso.test")),
            ("oauthProviderEnumId", text("EopOther")),
        ],
    );
    assert!(errors().contains("not set up"));
}
