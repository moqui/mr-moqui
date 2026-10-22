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

//! Sign-in with Google or Microsoft for a mail account (IronStone M5e, `docs/Mail-OAuth.md`): the OAuth 2.0
//! authorization code grant with PKCE in a popup, the refresh token stored encrypted on the account's `EmailServer`,
//! and the access tokens the sync, the IDLE workers, the server test and the SMTP send sign in with (XOAUTH2).
//!
//! 1. `start#OAuthSignIn` keeps a pending sign-in in the user's session (state, PKCE verifier, nonce, the user, the
//!    account or the address being set up; ten minutes) and answers the provider's authorization URL. The redirect URI
//!    is the deployment's (`email_oauth_redirect_uri`, else the conf's webroot URL), never the request's.
//! 2. The provider sends the browser back to the webroot transition `/emailOAuth/callback`, which calls
//!    `complete#OAuthSignIn`: the state is taken once (a second use, another session or user, or a late one is
//!    refused), the code is exchanged with the verifier, the granted scopes and the ID token (audience, issuer, expiry,
//!    nonce; it comes straight from the token endpoint over TLS, so its signature is not checked, OpenID Connect Core
//!    3.1.3.7) are checked, its address must be the account's unless the user said a change is fine, and the refresh
//!    token is stored. The page posts the outcome (never a token) to the opener on this page's own origin and on a
//!    BroadcastChannel, and the outcome stays in the session for `get#OAuthStatus`.
//! 3. [`access_token`] gives a current access token, refreshing it (one refresh at a time per server, across a cluster
//!    too: the EmailServer row is locked for update) a minute before it expires. A refresh token the provider refuses
//!    (revoked, expired: `invalid_grant`) marks the server's accounts EasAuthFailed ("sign in again"), logged once; any
//!    other failure is temporary. Access tokens are kept in memory and encrypted on `EmailServerAccessToken`, where the
//!    other servers of a cluster find them.
//! 4. Removing an account or signing out revokes the refresh token at the provider when it has a revocation endpoint,
//!    after the commit, best effort and logged, unless another server here holds a sign-in of the same address (the
//!    provider revokes the whole grant), and deletes it here.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

#[cfg(test)]
use base64::engine::general_purpose::STANDARD;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use indexmap::IndexMap;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};

use super::implementation::{find_one, force_new, note_error};
use super::inbound::stamp_millis;
use super::sync::{value_millis, value_number, ACCOUNT, SERVER};
use crate::context::ExecutionContext;
use crate::entity::{Condition, EntityFacade, EntityValue};
use crate::service::util::{field_text, now_millis, param_text, take_errors_string};
use crate::user::session::{current_user_id, session_attribute, update_session_attribute};

const PROVIDER: &str = "moqui.basic.email.EmailOAuthProvider";
const ENUMERATION: &str = "moqui.basic.Enumeration";
const IDENTITY: &str = "moqui.basic.email.EmailIdentity";
/// The stored access token of an EatOAuth2 server, shared by the servers of a cluster.
const TOKEN: &str = "moqui.basic.email.EmailServerAccessToken";
const SERVICES: &str = "org.moqui.impl.EmailClientServices";
/// The authentication type of a server that signs in with OAuth.
pub(super) const OAUTH: &str = "EatOAuth2";
/// The session attribute that holds the pending sign-ins, by sign-in id.
const PENDING: &str = "moqui.email.oauth";
const SIGN_IN_MILLIS: i64 = 10 * 60 * 1000;
/// How long an outcome stays for `get#OAuthStatus` after the sign-in expired.
const KEEP_MILLIS: i64 = 10 * 60 * 1000;
const MAX_PENDING: usize = 8;
/// How far back a new account's first sync reads by default, in days: a year, the add-account wizard's default.
const NEW_SYNC_WINDOW_DAYS: i64 = 365;
/// An access token is refreshed this long before it expires.
const EARLY_MILLIS: i64 = 60 * 1000;
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
/// The `type` of the callback page's message, and the name of its BroadcastChannel (`moqui.oauthPopup`).
const MESSAGE_TYPE: &str = "moqui-email-oauth";
const DECRYPT_FAILED: &str = crate::entity::field_rw::DECRYPT_FAILED;

// ---- providers ----

/// One `EmailOAuthProvider` with the deployment's client. `Debug` leaves out the secret.
#[derive(Clone, Default)]
pub(super) struct Provider {
    pub(super) id: String,
    pub(super) name: String,
    client_id: String,
    client_secret: String,
    authorize_url: String,
    token_url: String,
    revoke_url: String,
    user_info_url: String,
    scope: String,
    mail_scope: String,
    /// Asked for too, and to be granted, for an account that reads its mail over POP3.
    pop_scope: String,
    auth_params: String,
    refresh_params: String,
    issuer: String,
    address_claims: Vec<String>,
    pub(super) address_domains: Vec<String>,
    pub(super) host_suffixes: Vec<String>,
    pub(super) mx_suffixes: Vec<String>,
    pub(super) store_host: String,
    pub(super) store_port: String,
    /// imaps (the default) or imap.
    pub(super) store_protocol: String,
    /// The POP3 server of an account made to read its mail over POP3; empty when the provider has none.
    pub(super) pop_host: String,
    pub(super) pop_port: String,
    /// pop3s (the default) or pop3.
    pub(super) pop_protocol: String,
    pub(super) smtp_host: String,
    pub(super) smtp_port: String,
    pub(super) smtp_ssl: String,
    pub(super) smtp_start_tls: String,
    disabled: bool,
}

impl fmt::Debug for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Provider")
            .field("id", &self.id)
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &if self.client_secret.is_empty() {
                    ""
                } else {
                    "<redacted>"
                },
            )
            .field("authorize_url", &self.authorize_url)
            .field("token_url", &self.token_url)
            .field("disabled", &self.disabled)
            .finish_non_exhaustive()
    }
}

fn comma_list(text: &str) -> Vec<String> {
    text.split(',')
        .map(|part| part.trim().to_ascii_lowercase())
        .filter(|part| !part.is_empty())
        .collect()
}

impl Provider {
    fn from_row(row: &EntityValue, name: String) -> Self {
        let prefix = field_text(row, "propertyPrefix");
        let property = |suffix: &str| {
            if prefix.is_empty() {
                String::new()
            } else {
                moqui_lib::env::get_prop_or_env(&format!("{prefix}_{suffix}"))
                    .trim()
                    .to_string()
            }
        };
        let pick = |own: String, field: &str| if own.is_empty() { field_text(row, field) } else { own };
        let mut client_secret = pick(property("client_secret"), "clientSecret");
        let id = field_text(row, "oauthProviderEnumId");
        if client_secret == DECRYPT_FAILED {
            tracing::error!(
                "EmailOAuthProvider {id}: the clientSecret cannot be decrypted; the provider is not offered"
            );
            client_secret.clear();
        }
        let tenant = match pick(property("tenant"), "tenant") {
            tenant if tenant.is_empty() => "common".to_string(),
            tenant => tenant,
        };
        let fill = |field: &str| field_text(row, field).trim().replace("{tenant}", &tenant);
        Self {
            name: if name.is_empty() { id.clone() } else { name },
            id,
            client_id: pick(property("client_id"), "clientId"),
            client_secret,
            authorize_url: fill("authorizeUrl"),
            token_url: fill("tokenUrl"),
            revoke_url: fill("revokeUrl"),
            user_info_url: fill("userInfoUrl"),
            scope: field_text(row, "scope"),
            mail_scope: field_text(row, "mailScope"),
            pop_scope: field_text(row, "popScope"),
            auth_params: field_text(row, "authParams"),
            refresh_params: field_text(row, "refreshParams"),
            issuer: fill("issuer"),
            address_claims: field_text(row, "addressClaims")
                .split(',')
                .map(|claim| claim.trim().to_string())
                .filter(|claim| !claim.is_empty())
                .collect(),
            address_domains: comma_list(&field_text(row, "addressDomains")),
            host_suffixes: comma_list(&field_text(row, "hostSuffixes")),
            mx_suffixes: comma_list(&field_text(row, "mxSuffixes")),
            store_host: field_text(row, "storeHost"),
            store_port: field_text(row, "storePort"),
            store_protocol: match field_text(row, "storeProtocol") {
                protocol if protocol.is_empty() => "imaps".to_string(),
                protocol => protocol,
            },
            pop_host: field_text(row, "popHost"),
            pop_port: field_text(row, "popPort"),
            pop_protocol: match field_text(row, "popProtocol") {
                protocol if protocol.is_empty() => "pop3s".to_string(),
                protocol => protocol,
            },
            smtp_host: field_text(row, "smtpHost"),
            smtp_port: field_text(row, "smtpPort"),
            smtp_ssl: field_text(row, "smtpSsl"),
            smtp_start_tls: field_text(row, "smtpStartTls"),
            disabled: field_text(row, "disabled") == "Y",
        }
    }

    /// The deployment registered a client for it and the redirect URI is known.
    pub(super) fn configured(&self) -> bool {
        !self.disabled
            && !self.client_id.is_empty()
            && !self.client_secret.is_empty()
            && !self.authorize_url.is_empty()
            && !self.token_url.is_empty()
            && !redirect_uri().is_empty()
    }

    fn has_revocation(&self) -> bool {
        !self.revoke_url.is_empty()
    }

    /// The scopes asked for: with `pop` (the account reads its mail over POP3) the POP scopes too.
    fn scope_for(&self, pop: bool) -> String {
        join_scopes(&self.scope, pop.then_some(self.pop_scope.as_str()))
    }

    /// The scopes that must be granted for mail.
    fn mail_scope_for(&self, pop: bool) -> String {
        join_scopes(&self.mail_scope, pop.then_some(self.pop_scope.as_str()))
    }
}

fn join_scopes(base: &str, more: Option<&str>) -> String {
    let mut scopes: Vec<&str> = base.split_whitespace().collect();
    for scope in more.unwrap_or_default().split_whitespace() {
        if !scopes.contains(&scope) {
            scopes.push(scope);
        }
    }
    scopes.join(" ")
}

/// Whether a server row reads its mail over POP3.
fn pop_server(server: &EntityValue) -> bool {
    matches!(
        field_text(server, "storeProtocol").to_ascii_lowercase().as_str(),
        "pop3" | "pop3s"
    )
}

fn enum_description(facade: &EntityFacade, id: &str) -> String {
    find_one(facade, ENUMERATION, "enumId", id)
        .ok()
        .flatten()
        .map(|row| field_text(&row, "description"))
        .unwrap_or_default()
}

pub(super) fn load_provider(facade: &EntityFacade, id: &str) -> Result<Option<Provider>, LibError> {
    Ok(find_one(facade, PROVIDER, "oauthProviderEnumId", id)?
        .map(|row| Provider::from_row(&row, enum_description(facade, id))))
}

pub(super) fn load_providers(facade: &EntityFacade) -> Result<Vec<Provider>, LibError> {
    let rows = facade.find(PROVIDER)?.order_by("oauthProviderEnumId", false).list()?;
    Ok(rows
        .iter()
        .map(|row| {
            let id = field_text(row, "oauthProviderEnumId");
            Provider::from_row(row, enum_description(facade, &id))
        })
        .collect())
}

/// The redirect URI registered at the providers: the conf's `email-client` `oauth-redirect-uri`
/// (`email_oauth_redirect_uri`), else the webroot webapp's URL from the conf with `/emailOAuth/callback`. Never from a
/// request.
pub(super) fn redirect_uri() -> String {
    let mut configured = super::client_setting("oauth-redirect-uri");
    if configured.is_empty() {
        configured = moqui_lib::env::get_prop_or_env("email_oauth_redirect_uri");
    }
    if !configured.trim().is_empty() {
        return configured.trim().to_string();
    }
    let root = ExecutionContext::with_current(|ec| ec.factory().webapp_root_url("webroot")).unwrap_or_default();
    if root.is_empty() {
        return String::new();
    }
    format!("{}/emailOAuth/callback", root.trim_end_matches('/'))
}

/// `host` is `suffix` or a name under it.
fn host_under(host: &str, suffix: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    !suffix.is_empty() && (host == suffix || host.ends_with(&format!(".{suffix}")))
}

/// The provider whose mail an address is: by its domain, by the servers found for it, or by the domain's MX hosts (a
/// company domain at Google Workspace or Microsoft 365).
pub(super) fn detect(providers: &[Provider], domain: &str, hosts: &[String], mx: &[String]) -> Option<Provider> {
    let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    let by = |test: &dyn Fn(&Provider) -> bool| providers.iter().find(|provider| test(provider)).cloned();
    by(&|provider| provider.address_domains.contains(&domain))
        .or_else(|| {
            by(&|provider| {
                hosts
                    .iter()
                    .any(|host| provider.host_suffixes.iter().any(|suffix| host_under(host, suffix)))
            })
        })
        .or_else(|| {
            by(&|provider| {
                mx.iter()
                    .any(|host| provider.mx_suffixes.iter().any(|suffix| host_under(host, suffix)))
            })
        })
}

// ---- small helpers ----

fn random_bytes(count: usize) -> Result<Vec<u8>, LibError> {
    let mut buffer = vec![0u8; count];
    getrandom::fill(&mut buffer).map_err(|err| LibError::new(format!("no random bytes: {err}")))?;
    Ok(buffer)
}

fn random_text(count: usize) -> Result<String, LibError> {
    Ok(URL_SAFE_NO_PAD.encode(random_bytes(count)?))
}

/// The PKCE S256 challenge of a verifier (RFC 7636).
pub(super) fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn form(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(name, value)| format!("{}={}", encode(name), encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// `name=value&name=value`, as written in `authParams` and `refreshParams` (not encoded).
fn extra_pairs(text: &str, scope: &str) -> Vec<(String, String)> {
    text.split('&')
        .filter_map(|pair| {
            let (name, value) = pair.split_once('=')?;
            let name = name.trim();
            (!name.is_empty()).then(|| (name.to_string(), value.trim().replace("{scope}", scope)))
        })
        .collect()
}

/// Equal without telling by timing how much of it was.
fn same_secret(left: &str, right: &str) -> bool {
    left.len() == right.len() && left.bytes().zip(right.bytes()).fold(0u8, |diff, (a, b)| diff | (a ^ b)) == 0
}

fn digest(text: &str) -> String {
    moqui_lib::codec::hex_encode(&Sha256::digest(text.as_bytes())[..16])
}

fn json_text(value: Option<&Json>) -> String {
    match value {
        Some(Json::String(text)) => text.trim().to_string(),
        Some(Json::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}

/// No provider call: a sim session, or the read fence (which keeps the refusal).
fn sim() -> bool {
    ExecutionContext::with_current(|ec| ec.factory().entity().is_some_and(|facade| facade.sim_active()))
        .unwrap_or(false)
        || crate::entity::fence::refuses(crate::entity::fence::Effect::RemoteCall, "a mail sign-in provider")
}

/// The answer of a provider endpoint: its status and JSON body.
struct Answer {
    status: u16,
    json: Json,
}

impl Answer {
    fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// `error` and `error_description` of a refusal (RFC 6749 5.2), never the rest of the body.
    fn refusal(&self) -> (String, String) {
        (
            json_text(self.json.get("error")),
            json_text(self.json.get("error_description"))
                .chars()
                .take(300)
                .collect(),
        )
    }
}

fn http() -> &'static moqui_lib::http::RestClient {
    static CLIENT: OnceLock<moqui_lib::http::RestClient> = OnceLock::new();
    CLIENT.get_or_init(|| moqui_lib::http::RestClient::new().timeout(HTTP_TIMEOUT))
}

fn post_form(url: &str, pairs: &[(String, String)]) -> Result<Answer, String> {
    let response = http()
        .post(url)
        .header("Accept", "application/json")
        .text("application/x-www-form-urlencoded", form(pairs))
        .timeout(HTTP_TIMEOUT)
        .send(sim())
        .map_err(|err| err.to_string())?;
    Ok(Answer {
        status: response.status(),
        json: serde_json::from_slice(response.bytes()).unwrap_or(Json::Null),
    })
}

fn get_bearer(url: &str, token: &str) -> Result<Answer, String> {
    let response = http()
        .get(url)
        .header("Accept", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .timeout(HTTP_TIMEOUT)
        .send(sim())
        .map_err(|err| err.to_string())?;
    Ok(Answer {
        status: response.status(),
        json: serde_json::from_slice(response.bytes()).unwrap_or(Json::Null),
    })
}

// ---- access tokens ----

/// An access token in memory or on `EmailServerAccessToken`, with the refresh token it came from (by digest) and when
/// it expires.
struct Cached {
    token: String,
    expires: i64,
    refresh: String,
}

impl Cached {
    fn new(token: &str, expires_in: i64, refresh: &str) -> Self {
        let expires_in = if expires_in > 0 { expires_in } else { 3600 };
        Self {
            token: token.to_string(),
            expires: now_millis() + expires_in.saturating_mul(1000),
            refresh: digest(refresh),
        }
    }

    /// Current for the refresh token `refresh` (not about to expire) and not the `rejected` one.
    fn usable(&self, refresh: &str, rejected: Option<&str>) -> bool {
        self.refresh == digest(refresh)
            && self.expires - EARLY_MILLIS > now_millis()
            && rejected.is_none_or(|rejected| rejected != self.token)
    }
}

fn cache() -> &'static Mutex<HashMap<String, Cached>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// One refresh at a time per server in this process; across a cluster the refresh locks the EmailServer row too.
fn server_lock(server_id: &str) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    Arc::clone(lock(locks).entry(server_id.to_string()).or_default())
}

fn remember(server_id: &str, cached: Cached) {
    lock(cache()).insert(server_id.to_string(), cached);
}

/// Drop the access token of a server: the one kept in memory and the stored one.
pub(super) fn forget(facade: &EntityFacade, server_id: &str) {
    forget_local(server_id);
    let deleted = facade
        .find(TOKEN)
        .and_then(|find| find.eq("emailServerId", Value::string(server_id)).delete_all());
    if let Err(err) = deleted {
        tracing::error!("Email server {server_id}: could not delete its stored OAuth access token: {err}");
    }
}

/// Drop the access token kept in memory for a server, as another server of the cluster that has none.
pub(super) fn forget_local(server_id: &str) {
    lock(cache()).remove(server_id);
}

/// Make the access token of a server, in memory and stored, one that expires now, as an hour later.
#[cfg(test)]
pub(super) fn age_cached(facade: &EntityFacade, server_id: &str) {
    if let Some(found) = lock(cache()).get_mut(server_id) {
        found.expires = now_millis();
    }
    facade
        .find(TOKEN)
        .and_then(|find| {
            find.eq("emailServerId", Value::string(server_id))
                .update_all([("expiresDate".to_string(), stamp_millis(now_millis()))])
        })
        .unwrap();
}

fn cached(server_id: &str, refresh: &str, rejected: Option<&str>) -> Option<String> {
    let cache = lock(cache());
    let found = cache.get(server_id)?;
    found.usable(refresh, rejected).then(|| found.token.clone())
}

/// The stored access token of a server when it is usable; a row that will not decrypt is as none.
fn stored_row(row: &EntityValue, server_id: &str, refresh: &str, rejected: Option<&str>) -> Option<Cached> {
    let stored = Cached {
        token: field_text(row, "accessToken"),
        expires: value_millis(row.get("expiresDate"))?,
        refresh: field_text(row, "refreshTokenDigest"),
    };
    if stored.token.is_empty() || !stored.usable(refresh, rejected) {
        return None;
    }
    if stored.token == DECRYPT_FAILED {
        tracing::warn!("Email server {server_id}: the stored OAuth access token cannot be decrypted; it is refreshed");
        return None;
    }
    Some(stored)
}

/// The stored access token, kept in memory too, when another server of the cluster (or this one before a restart)
/// refreshed it.
fn stored(facade: &EntityFacade, server_id: &str, refresh: &str, rejected: Option<&str>) -> Option<String> {
    let row = match facade.find(TOKEN).and_then(|find| {
        find.eq("emailServerId", Value::string(server_id))
            .use_cache(false)
            .one()
    }) {
        Ok(row) => row?,
        Err(err) => {
            tracing::warn!("Email server {server_id}: could not read its stored OAuth access token: {err}");
            return None;
        }
    };
    let found = stored_row(&row, server_id, refresh, rejected)?;
    let token = found.token.clone();
    remember(server_id, found);
    Some(token)
}

/// Why there is no access token.
#[derive(Debug)]
pub(super) enum TokenError {
    /// The user must sign in again: no refresh token, or the provider refused it. The accounts are marked already.
    SignIn(String),
    /// Not now: the provider could not be reached, the client is not set up, or it was refused for another reason.
    Unavailable(String),
}

impl fmt::Display for TokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SignIn(text) | Self::Unavailable(text) => f.write_str(text),
        }
    }
}

/// What a refresh under the EmailServer lock came to.
enum Refresh {
    /// A new token, or the one another server of the cluster refreshed meanwhile.
    Token(Cached),
    /// The user must sign in again: the text and the detail logged. The accounts are marked after the transaction.
    SignIn(String, String),
    Failed(TokenError),
}

/// A current access token for an EatOAuth2 server, refreshed when it expires within a minute (or is `rejected`, the
/// one a server just refused): the one kept in memory, else the stored one (refreshed by another server of the
/// cluster), else a refresh. One refresh at a time per server, in this process and across the cluster (the EmailServer
/// row locked for update); a new refresh token the provider sends replaces the stored one in the same transaction.
pub(super) fn access_token(
    facade: &EntityFacade,
    server_id: &str,
    rejected: Option<&str>,
) -> Result<String, TokenError> {
    let read = || -> Result<EntityValue, TokenError> {
        facade
            .find(SERVER)
            .and_then(|find| {
                find.eq("emailServerId", Value::string(server_id))
                    .use_cache(false)
                    .one()
            })
            .map_err(|err| TokenError::Unavailable(format!("Email server {server_id}: {err}")))?
            .ok_or_else(|| TokenError::Unavailable(format!("Email server {server_id} not found")))
    };
    let server = read()?;
    let refresh = field_text(&server, "oauthRefreshToken");
    if !refresh.is_empty() && refresh != DECRYPT_FAILED {
        if let Some(token) =
            cached(server_id, &refresh, rejected).or_else(|| stored(facade, server_id, &refresh, rejected))
        {
            return Ok(token);
        }
    }
    let guard = server_lock(server_id);
    let _turn = lock(&guard);
    // another thread may have refreshed while this one waited
    let server = read()?;
    let refresh = field_text(&server, "oauthRefreshToken");
    let provider_id = field_text(&server, "oauthProviderEnumId");
    let provider = load_provider(facade, &provider_id)
        .map_err(|err| TokenError::Unavailable(format!("OAuth provider {provider_id}: {err}")))?
        .ok_or_else(|| TokenError::Unavailable(format!("Email server {server_id} names no OAuth provider")))?;
    if refresh.is_empty() {
        let text = format!("Not signed in with {}; sign in to sync this account", provider.name);
        mark_sign_in_needed(facade, server_id, &text, "no refresh token is stored");
        return Err(TokenError::SignIn(text));
    }
    if refresh == DECRYPT_FAILED {
        return Err(undecryptable(server_id));
    }
    if let Some(token) = cached(server_id, &refresh, rejected) {
        return Ok(token);
    }
    if !provider.configured() {
        return Err(TokenError::Unavailable(format!(
            "Sign-in with {} is not set up on this server (its OAuth client)",
            provider.name
        )));
    }
    // a token the POST got, kept when the transaction that stores it fails
    let mut got: Option<Cached> = None;
    let refreshed = force_new(facade, || {
        refresh_locked(facade, server_id, &provider, rejected, &mut got)
    });
    match refreshed {
        Ok(Refresh::Token(fresh)) => {
            let token = fresh.token.clone();
            remember(server_id, fresh);
            Ok(token)
        }
        Ok(Refresh::SignIn(text, detail)) => {
            // after the transaction: the marking is a transaction of its own, which waits on this one's lock
            mark_sign_in_needed(facade, server_id, &text, &detail);
            Err(TokenError::SignIn(text))
        }
        Ok(Refresh::Failed(err)) => Err(err),
        Err(err) => match got {
            Some(fresh) => {
                tracing::error!("Email server {server_id}: could not store the refreshed OAuth access token: {err}");
                let token = fresh.token.clone();
                remember(server_id, fresh);
                Ok(token)
            }
            None => Err(TokenError::Unavailable(format!(
                "Could not refresh the {} sign-in of email server {server_id}: {err}",
                provider.name
            ))),
        },
    }
}

fn undecryptable(server_id: &str) -> TokenError {
    tracing::error!("Email server {server_id}: the stored OAuth refresh token cannot be decrypted");
    TokenError::Unavailable(format!(
        "The sign-in of email server {server_id} cannot be decrypted (entity_ds_crypt_pass changed?)"
    ))
}

/// The refresh, in the caller's transaction: the EmailServer row locked for update (one refresh at a time across the
/// cluster), the stored token used when another server refreshed it meanwhile, else the POST, and the token and a new
/// refresh token stored. The token the POST got is put in `got` before it is stored.
fn refresh_locked(
    facade: &EntityFacade,
    server_id: &str,
    provider: &Provider,
    rejected: Option<&str>,
    got: &mut Option<Cached>,
) -> Result<Refresh, LibError> {
    let Some(server) = facade
        .find(SERVER)?
        .eq("emailServerId", Value::string(server_id))
        .use_cache(false)
        .for_update(true)
        .one()?
    else {
        return Ok(Refresh::Failed(TokenError::Unavailable(format!(
            "Email server {server_id} not found"
        ))));
    };
    let refresh = field_text(&server, "oauthRefreshToken");
    if refresh.is_empty() {
        return Ok(Refresh::SignIn(
            format!("Not signed in with {}; sign in to sync this account", provider.name),
            "no refresh token is stored".to_string(),
        ));
    }
    if refresh == DECRYPT_FAILED {
        return Ok(Refresh::Failed(undecryptable(server_id)));
    }
    if let Some(row) = facade
        .find(TOKEN)?
        .eq("emailServerId", Value::string(server_id))
        .use_cache(false)
        .one()?
    {
        if let Some(found) = stored_row(&row, server_id, &refresh, rejected) {
            return Ok(Refresh::Token(found));
        }
    }
    let mut pairs = vec![
        ("grant_type".to_string(), "refresh_token".to_string()),
        ("refresh_token".to_string(), refresh.clone()),
        ("client_id".to_string(), provider.client_id.clone()),
        ("client_secret".to_string(), provider.client_secret.clone()),
    ];
    pairs.extend(extra_pairs(
        &provider.refresh_params,
        &provider.scope_for(pop_server(&server)),
    ));
    let answer = match post_form(&provider.token_url, &pairs) {
        Ok(answer) => answer,
        Err(err) => {
            return Ok(Refresh::Failed(TokenError::Unavailable(format!(
                "Could not refresh the {} sign-in of email server {server_id}: {err}",
                provider.name
            ))))
        }
    };
    if !answer.ok() {
        let (code, description) = answer.refusal();
        let detail = if description.is_empty() {
            format!("{code} (HTTP {})", answer.status)
        } else {
            format!("{code}: {description}")
        };
        if code == "invalid_grant" {
            let text = format!(
                "The sign-in with {} ended (revoked or expired); sign in again to sync this account",
                provider.name
            );
            return Ok(Refresh::SignIn(text, detail));
        }
        if matches!(code.as_str(), "invalid_client" | "unauthorized_client") {
            tracing::error!(
                "Email server {server_id}: {} refused this server's OAuth client ({detail}); check the client id and secret",
                provider.name
            );
        }
        return Ok(Refresh::Failed(TokenError::Unavailable(format!(
            "{} refused to refresh the sign-in of email server {server_id}: {detail}",
            provider.name
        ))));
    }
    let token = json_text(answer.json.get("access_token"));
    if token.is_empty() {
        return Ok(Refresh::Failed(TokenError::Unavailable(format!(
            "{} sent no access token for email server {server_id}",
            provider.name
        ))));
    }
    let expires_in = value_number(Some(&moqui_lib::json::from_json(
        answer.json.get("expires_in").unwrap_or(&Json::Null),
    )))
    .unwrap_or(3600);
    *got = Some(Cached::new(&token, expires_in, &refresh));
    // a provider that rotates refresh tokens (Microsoft): keep the new one; the row is locked, so the one used is still
    // the stored one
    let rotated = json_text(answer.json.get("refresh_token"));
    let current = if !rotated.is_empty() && rotated != refresh {
        let mut update = facade.make_value(SERVER)?;
        update.set("emailServerId", Value::string(server_id));
        update.set("oauthRefreshToken", Value::string(&rotated));
        update.update(facade)?;
        rotated
    } else {
        refresh
    };
    let fresh = Cached::new(&token, expires_in, &current);
    let mut row = facade.make_value(TOKEN)?;
    row.set("emailServerId", Value::string(server_id));
    row.set("accessToken", Value::string(&fresh.token));
    row.set("expiresDate", stamp_millis(fresh.expires));
    row.set("refreshTokenDigest", Value::string(&fresh.refresh));
    row.create_or_update(facade)?;
    Ok(Refresh::Token(fresh))
}

/// The server's active accounts need a new sign-in: EasAuthFailed with `text`, logged once (when an account changes).
fn mark_sign_in_needed(facade: &EntityFacade, server_id: &str, text: &str, detail: &str) {
    let marked = force_new(facade, || {
        facade
            .find(ACCOUNT)?
            .eq("emailServerId", Value::string(server_id))
            .condition(Condition::or(vec![
                Condition::is_null("statusId"),
                Condition::is_in("statusId", vec![Value::string("EasActive"), Value::string("EasError")]),
            ]))
            .update_all([
                ("statusId".to_string(), Value::string("EasAuthFailed")),
                ("lastSyncError".to_string(), Value::string(text)),
            ])
    });
    match marked {
        Ok(count) if count > 0 => {
            tracing::error!(server = %server_id, "Email server {server_id}: the OAuth sign-in needs renewing ({detail}); {count} account(s) marked to sign in again");
            publish_counts_of_server(facade, server_id);
        }
        Ok(_) => tracing::debug!("Email server {server_id}: the OAuth sign-in needs renewing ({detail})"),
        Err(err) => tracing::error!("Email server {server_id}: could not mark its accounts to sign in again: {err}"),
    }
}

fn publish_counts_of_server(facade: &EntityFacade, server_id: &str) {
    let ids: Vec<Value> = facade
        .find(ACCOUNT)
        .and_then(|find| find.eq("emailServerId", Value::string(server_id)).list())
        .map(|rows| {
            rows.iter()
                .map(|row| Value::string(field_text(row, "emailAccountId")))
                .collect()
        })
        .unwrap_or_default();
    if ids.is_empty() {
        return;
    }
    let mut parameters = BTreeMap::new();
    parameters.insert("emailAccountIdList".to_string(), Value::List(ids));
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    if let Err(err) = crate::service::sync(&format!("{SERVICES}.publish#AccountCounts"), &parameters) {
        tracing::debug!("Could not publish the counts of email server {server_id}: {err}");
    }
}

// ---- pending sign-ins (the session) ----

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Outcome {
    status: String,
    #[serde(default)]
    error_code: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    email_account_id: String,
    #[serde(default)]
    email_address: String,
    #[serde(default)]
    expected_address: String,
}

/// A sign-in started in this session. The verifier and nonce never leave the server.
#[derive(Clone, Serialize, Deserialize)]
struct Pending {
    state: String,
    verifier: String,
    nonce: String,
    user: String,
    provider: String,
    #[serde(default)]
    account: String,
    #[serde(default)]
    address: String,
    #[serde(default)]
    allow_change: bool,
    #[serde(default)]
    account_name: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    account_kind: String,
    /// The account reads its mail over POP3: the provider's POP scopes are asked for and must be granted.
    #[serde(default)]
    pop: bool,
    /// A new account's sync window in days, 0 for all mail.
    #[serde(default)]
    sync_window_days: i64,
    /// A new account's status: EasPaused (also when empty) or EasActive.
    #[serde(default)]
    account_status: String,
    created: i64,
    expires: i64,
    #[serde(default)]
    used: bool,
    #[serde(default)]
    outcome: Option<Outcome>,
}

fn read_pending(text: Option<String>) -> BTreeMap<String, Pending> {
    text.and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_pending(entries: &BTreeMap<String, Pending>) -> Option<String> {
    if entries.is_empty() {
        None
    } else {
        serde_json::to_string(entries).ok()
    }
}

/// Drop the sign-ins whose outcome no one will ask for any more, then the oldest past `MAX_PENDING`.
fn prune(entries: &mut BTreeMap<String, Pending>, now: i64) {
    entries.retain(|_, entry| entry.expires + KEEP_MILLIS > now);
    while entries.len() > MAX_PENDING {
        let oldest = entries
            .iter()
            .min_by_key(|(_, entry)| entry.created)
            .map(|(id, _)| id.clone());
        match oldest {
            Some(id) => entries.remove(&id),
            None => break,
        };
    }
}

/// Take the sign-in of `state` once: the entry as it was, marked used in the session. A used one comes back `used`.
fn take_pending(state: &str) -> Option<(String, Pending)> {
    let mut taken = None;
    update_session_attribute(PENDING, |current| {
        let mut entries = read_pending(current);
        if let Some((id, entry)) = entries.iter_mut().find(|(_, entry)| same_secret(&entry.state, state)) {
            taken = Some((id.clone(), entry.clone()));
            entry.used = true;
        }
        write_pending(&entries)
    });
    taken
}

fn record_outcome(sign_in_id: &str, outcome: &Outcome) {
    update_session_attribute(PENDING, |current| {
        let mut entries = read_pending(current);
        if let Some(entry) = entries.get_mut(sign_in_id) {
            entry.outcome = Some(outcome.clone());
        }
        write_pending(&entries)
    });
}

// ---- services ----

fn flag(parameters: &BTreeMap<String, Value>, name: &str) -> bool {
    match parameters.get(name) {
        Some(Value::Bool(on)) => *on,
        Some(value) => matches!(to_plain_string(value).to_ascii_lowercase().as_str(), "true" | "y"),
        None => false,
    }
}

fn user_id() -> String {
    current_user_id().ok().flatten().unwrap_or_default()
}

/// The access level of the current user to an account (get#EmailAccountAccess), with the account's map.
fn account_access(account_id: &str) -> Result<(i64, IndexMap<String, Value>), LibError> {
    let mut parameters = BTreeMap::new();
    parameters.insert("emailAccountId".to_string(), Value::string(account_id));
    let out = crate::service::sync(&format!("{SERVICES}.get#EmailAccountAccess"), &parameters)
        .map_err(|err| LibError::new(err.to_string()))?;
    let level = value_number(out.get("level")).unwrap_or(0);
    let account = match out.get("account") {
        Some(Value::Map(map)) => map.clone(),
        _ => IndexMap::new(),
    };
    Ok((level, account))
}

fn text_out(out: &mut BTreeMap<String, Value>, name: &str, value: &str) {
    out.insert(
        name.to_string(),
        if value.is_empty() {
            Value::Null
        } else {
            Value::string(value)
        },
    );
}

/// `start#OAuthSignIn`.
pub(super) fn start_sign_in(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    if facade.sim_active() {
        note_error("Email sign-in is disabled in LLM sim session");
        return Ok(out);
    }
    crate::entity::fence::check_effect(crate::entity::fence::Effect::RemoteCall, "start#OAuthSignIn")?;
    let user = user_id();
    if user.is_empty() {
        note_error("Sign in to the app first");
        return Ok(out);
    }
    let account_id = param_text(parameters, "emailAccountId");
    let mut provider_id = param_text(parameters, "oauthProviderEnumId");
    let mut pending = Pending {
        state: random_text(32)?,
        verifier: random_text(48)?,
        nonce: random_text(24)?,
        user,
        provider: String::new(),
        account: account_id.clone(),
        address: String::new(),
        allow_change: flag(parameters, "allowAddressChange"),
        account_name: param_text(parameters, "accountName"),
        display_name: param_text(parameters, "displayName"),
        account_kind: String::new(),
        pop: false,
        sync_window_days: 0,
        account_status: String::new(),
        created: now_millis(),
        expires: now_millis() + SIGN_IN_MILLIS,
        used: false,
        outcome: None,
    };
    if account_id.is_empty() {
        let address = param_text(parameters, "emailAddress").trim().to_string();
        let Some((local, domain)) = address.rsplit_once('@') else {
            note_error("An email address or an account is needed to sign in");
            return Ok(out);
        };
        if local.is_empty() || !domain.contains('.') {
            note_error(format!("{address} is not an email address"));
            return Ok(out);
        }
        let kind = match param_text(parameters, "accountKindEnumId") {
            kind if kind.is_empty() => "EakExternal".to_string(),
            kind => kind,
        };
        if !matches!(kind.as_str(), "EakExternal" | "EakShared") {
            note_error(format!("An account of kind {kind} does not sign in with a provider"));
            return Ok(out);
        }
        // the new account waits for the app to finish its setup (paused), or syncs at once within a window, so the
        // job does not bring in the whole mailbox a minute after the sign-in
        let window = match parameters.get("syncWindowDays") {
            None | Some(Value::Null) => Some(NEW_SYNC_WINDOW_DAYS),
            Some(value) if to_plain_string(value).trim().is_empty() => Some(NEW_SYNC_WINDOW_DAYS),
            Some(value) => value_number(Some(value)).filter(|days| *days >= 0),
        };
        let Some(window) = window else {
            note_error("The sync window is a number of days (0 for all mail)");
            return Ok(out);
        };
        let status = match param_text(parameters, "statusId") {
            status if status.is_empty() => "EasPaused".to_string(),
            status => status,
        };
        if !matches!(status.as_str(), "EasPaused" | "EasActive") {
            note_error(format!("A new account is Paused or Active, not {status}"));
            return Ok(out);
        }
        if status == "EasActive" && window == 0 {
            note_error(
                "An account that syncs at once needs a sync window (syncWindowDays), or it starts paused (EasPaused) until its setup is done",
            );
            return Ok(out);
        }
        // the new account reads its mail over IMAP (the default) or POP3, on the provider's server of that kind
        pending.pop = match param_text(parameters, "storeProtocol")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "" | "imap" | "imaps" => false,
            "pop3" | "pop3s" | "pop" => true,
            other => {
                note_error(format!("A new account reads its mail over imap or pop3, not {other}"));
                return Ok(out);
            }
        };
        pending.sync_window_days = window;
        pending.account_status = status;
        if provider_id.is_empty() {
            let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
            if let Some(found) = detect(&load_providers(facade)?, domain, &[], &[]) {
                provider_id = found.id;
            }
        }
        pending.address = address;
        pending.account_kind = kind;
    } else {
        let (level, account) = account_access(&account_id)?;
        if crate::service::util::has_error() {
            return Ok(out);
        }
        if level < 1 {
            note_error(format!("Email account {account_id} not found"));
            return Ok(out);
        }
        if level < 3 {
            note_error(format!("You may not change email account {account_id}"));
            return Ok(out);
        }
        if account.get("isInternal") == Some(&Value::Bool(true)) {
            note_error("The Internal account has no server to sign in to");
            return Ok(out);
        }
        let server_id = account.get("emailServerId").map(to_plain_string).unwrap_or_default();
        let owns = account.get("ownsEmailServer") == Some(&Value::Bool(true));
        if !server_id.is_empty() && !owns {
            note_error(format!(
                "Email account {account_id} uses a shared server, which is not changed through the account"
            ));
            return Ok(out);
        }
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        if let Some(server) = find_one(facade, SERVER, "emailServerId", &server_id)? {
            if provider_id.is_empty() {
                provider_id = field_text(&server, "oauthProviderEnumId");
            }
            pending.pop = pop_server(&server);
        }
        pending.address = account.get("emailAddress").map(to_plain_string).unwrap_or_default();
        if provider_id.is_empty() {
            if let Some((_, domain)) = pending.address.rsplit_once('@') {
                if let Some(found) = detect(&load_providers(facade)?, domain, &[], &[]) {
                    provider_id = found.id;
                }
            }
        }
    }
    if provider_id.is_empty() {
        note_error("Choose the provider to sign in with (oauthProviderEnumId)");
        return Ok(out);
    }
    let provider = {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        load_provider(facade, &provider_id)?
    };
    let Some(provider) = provider else {
        note_error(format!("No OAuth provider {provider_id}"));
        return Ok(out);
    };
    if !provider.configured() {
        note_error(format!(
            "Sign-in with {} is not set up on this server; use a password or an app password",
            provider.name
        ));
        return Ok(out);
    }
    if pending.account.is_empty() && pending.pop && provider.pop_host.is_empty() {
        note_error(format!(
            "{} has no POP server set up here; read the mail over IMAP",
            provider.name
        ));
        return Ok(out);
    }
    pending.provider = provider.id.clone();
    let scope = provider.scope_for(pending.pop);
    let mut query = vec![
        ("response_type".to_string(), "code".to_string()),
        ("client_id".to_string(), provider.client_id.clone()),
        ("redirect_uri".to_string(), redirect_uri()),
        ("scope".to_string(), scope.clone()),
        ("state".to_string(), pending.state.clone()),
        ("code_challenge".to_string(), challenge(&pending.verifier)),
        ("code_challenge_method".to_string(), "S256".to_string()),
        ("nonce".to_string(), pending.nonce.clone()),
    ];
    if !pending.address.is_empty() {
        query.push(("login_hint".to_string(), pending.address.clone()));
    }
    query.extend(extra_pairs(&provider.auth_params, &scope));
    let separator = if provider.authorize_url.contains('?') { '&' } else { '?' };
    let auth_url = format!("{}{separator}{}", provider.authorize_url, form(&query));
    let sign_in_id = moqui_lib::codec::hex_encode(&random_bytes(16)?);
    let entry = pending.clone();
    let id = sign_in_id.clone();
    update_session_attribute(PENDING, move |current| {
        let mut entries = read_pending(current);
        prune(&mut entries, now_millis());
        entries.insert(id, entry);
        prune(&mut entries, now_millis());
        write_pending(&entries)
    });
    tracing::info!(
        "Email sign-in {sign_in_id} with {} started by user {} for {}",
        provider.name,
        pending.user,
        if pending.account.is_empty() {
            format!("a new account {}", pending.address)
        } else {
            format!("account {}", pending.account)
        }
    );
    out.insert("authUrl".into(), Value::string(auth_url));
    out.insert("signInId".into(), Value::string(sign_in_id));
    out.insert("oauthProviderEnumId".into(), Value::string(&provider.id));
    out.insert("expiresIn".into(), Value::I64(SIGN_IN_MILLIS / 1000));
    Ok(out)
}

/// What a sign-in ended with, for the page and the opener.
fn failed(code: &str, message: impl Into<String>) -> Outcome {
    Outcome {
        status: "error".to_string(),
        error_code: code.to_string(),
        message: message.into(),
        ..Outcome::default()
    }
}

/// A sign-in that ended with an error, as the error of a step.
type Refused = Box<Outcome>;

fn refused(code: &str, message: impl Into<String>) -> Refused {
    Box::new(failed(code, message))
}

fn outcome_out(sign_in_id: &str, outcome: &Outcome) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    text_out(&mut out, "status", &outcome.status);
    text_out(&mut out, "errorCode", &outcome.error_code);
    text_out(&mut out, "message", &outcome.message);
    text_out(&mut out, "signInId", sign_in_id);
    text_out(&mut out, "emailAccountId", &outcome.email_account_id);
    text_out(&mut out, "emailAddress", &outcome.email_address);
    text_out(&mut out, "expectedAddress", &outcome.expected_address);
    out
}

/// `complete#OAuthSignIn`: the callback. Every outcome is answered (and with `sendPage` the popup's page is sent);
/// none is a service error, so the transition always renders the page.
pub(super) fn complete_sign_in(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let (sign_in_id, outcome) = finish_sign_in(parameters, facade);
    if outcome.status == "success" {
        tracing::info!(
            "Email sign-in {sign_in_id}: account {} signed in as {}",
            outcome.email_account_id,
            outcome.email_address
        );
    } else {
        tracing::warn!(
            "Email sign-in {}: {} ({})",
            if sign_in_id.is_empty() { "?" } else { &sign_in_id },
            outcome.message,
            outcome.error_code
        );
    }
    if !sign_in_id.is_empty() {
        record_outcome(&sign_in_id, &outcome);
    }
    if flag(parameters, "sendPage") {
        send_page(&sign_in_id, &outcome);
    }
    Ok(outcome_out(&sign_in_id, &outcome))
}

fn finish_sign_in(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> (String, Outcome) {
    let state = param_text(parameters, "state");
    let invalid = || {
        failed(
            "invalidState",
            "This sign-in is not known here: it was used already, it was started in another browser or session, or the page was reloaded. Start the sign-in again.",
        )
    };
    if state.is_empty() {
        return (String::new(), invalid());
    }
    let Some((sign_in_id, pending)) = take_pending(&state) else {
        return (String::new(), invalid());
    };
    if pending.used {
        // a replay: refused, and the outcome of the first use stays as it was
        return (String::new(), invalid());
    }
    let mut outcome = finish_pending(parameters, facade, &pending);
    if outcome.expected_address.is_empty() {
        outcome.expected_address = pending.address.clone();
    }
    (sign_in_id, outcome)
}

fn finish_pending(parameters: &BTreeMap<String, Value>, facade: &EntityFacade, pending: &Pending) -> Outcome {
    let user = user_id();
    if user.is_empty() || user != pending.user {
        return failed(
            "wrongUser",
            "The sign-in was started by another user, or you are signed out of the app. Sign in to the app and start again.",
        );
    }
    if now_millis() > pending.expires {
        return failed("expired", "The sign-in took too long. Start it again.");
    }
    let provider_error = param_text(parameters, "error");
    if !provider_error.is_empty() {
        if provider_error == "access_denied" {
            return failed(
                "denied",
                "The access was not allowed, so the account was not connected.",
            );
        }
        let description: String = param_text(parameters, "errorDescription").chars().take(300).collect();
        return failed(
            "providerError",
            if description.is_empty() {
                format!("The provider refused the sign-in: {provider_error}")
            } else {
                format!("The provider refused the sign-in: {provider_error}: {description}")
            },
        );
    }
    let code = param_text(parameters, "code");
    if code.is_empty() {
        return failed("providerError", "The provider sent no authorization code.");
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let provider = match load_provider(facade, &pending.provider) {
        Ok(Some(provider)) if provider.configured() => provider,
        Ok(_) => {
            return failed(
                "notConfigured",
                "Sign-in with this provider is not set up on this server.",
            )
        }
        Err(err) => return failed("failed", format!("The provider could not be read: {err}")),
    };
    let tokens = match exchange(&provider, &code, &pending.verifier, pending.pop) {
        Ok(tokens) => tokens,
        Err(outcome) => return *outcome,
    };
    let address = match signed_in_address(&provider, &tokens, &pending.nonce) {
        Ok(address) => address,
        Err(outcome) => return *outcome,
    };
    let expected = pending.address.trim();
    if !expected.is_empty() && !expected.eq_ignore_ascii_case(&address) && !pending.allow_change {
        return Outcome {
            expected_address: expected.to_string(),
            email_address: address.clone(),
            ..failed(
                "addressMismatch",
                format!(
                    "You signed in as {address}, but the account is {expected}. Sign in as {expected}, or confirm that the account becomes {address}."
                ),
            )
        };
    }
    if facade.crypt().is_none() {
        tracing::error!("Email sign-in refused: field encryption (entity_ds_crypt_pass) is not configured");
        return failed(
            "storeFailed",
            "This server does not encrypt stored secrets (entity_ds_crypt_pass), so the sign-in is not kept.",
        );
    }
    let stored = if pending.account.is_empty() {
        store_new_account(facade, pending, &provider, &tokens, &address)
    } else {
        store_on_account(facade, pending, &provider, &tokens, &address)
    };
    match stored {
        Ok((account_id, server_id)) => {
            remember(
                &server_id,
                Cached::new(&tokens.access, tokens.expires_in, &tokens.refresh),
            );
            Outcome {
                status: "success".to_string(),
                message: format!("Signed in with {} as {address}.", provider.name),
                email_account_id: account_id,
                email_address: address,
                expected_address: expected.to_string(),
                ..Outcome::default()
            }
        }
        Err(text) => failed("storeFailed", text),
    }
}

/// What the token endpoint gave. `Debug` is not derived, so no token can reach a log.
struct Tokens {
    access: String,
    refresh: String,
    expires_in: i64,
    id_token: String,
}

fn exchange(provider: &Provider, code: &str, verifier: &str, pop: bool) -> Result<Tokens, Refused> {
    let pairs = vec![
        ("grant_type".to_string(), "authorization_code".to_string()),
        ("code".to_string(), code.to_string()),
        ("redirect_uri".to_string(), redirect_uri()),
        ("client_id".to_string(), provider.client_id.clone()),
        ("client_secret".to_string(), provider.client_secret.clone()),
        ("code_verifier".to_string(), verifier.to_string()),
    ];
    let answer = post_form(&provider.token_url, &pairs).map_err(|err| {
        refused(
            "exchangeFailed",
            format!("{} could not be reached: {err}", provider.name),
        )
    })?;
    if !answer.ok() {
        let (error, description) = answer.refusal();
        return Err(refused(
            "exchangeFailed",
            format!(
                "{} refused the sign-in: {}{}",
                provider.name,
                if error.is_empty() {
                    format!("HTTP {}", answer.status)
                } else {
                    error
                },
                if description.is_empty() {
                    String::new()
                } else {
                    format!(": {description}")
                }
            ),
        ));
    }
    let access = json_text(answer.json.get("access_token"));
    let refresh = json_text(answer.json.get("refresh_token"));
    if access.is_empty() {
        return Err(refused(
            "exchangeFailed",
            format!("{} sent no access token.", provider.name),
        ));
    }
    if refresh.is_empty() {
        return Err(refused(
            "noRefreshToken",
            format!(
                "{} did not allow offline access, so mail cannot sync while you are away. Remove this app's access in your {} account and sign in again.",
                provider.name, provider.name
            ),
        ));
    }
    if let Some(Json::String(granted)) = answer.json.get("scope") {
        let wanted_scopes = provider.mail_scope_for(pop);
        let missing: Vec<&str> = wanted_scopes
            .split_whitespace()
            .filter(|wanted| !scope_granted(granted, wanted))
            .collect();
        if !missing.is_empty() {
            return Err(refused(
                "scopeNotGranted",
                format!(
                    "The access to mail was not allowed ({}). Sign in again and allow access to your mail.",
                    missing.join(", ")
                ),
            ));
        }
    }
    Ok(Tokens {
        access,
        refresh,
        expires_in: value_number(Some(&moqui_lib::json::from_json(
            answer.json.get("expires_in").unwrap_or(&Json::Null),
        )))
        .unwrap_or(3600),
        id_token: json_text(answer.json.get("id_token")),
    })
}

/// A granted scope list has `wanted`, as it is or by its last part (Microsoft may answer `IMAP.AccessAsUser.All` for
/// `https://outlook.office.com/IMAP.AccessAsUser.All`).
fn scope_granted(granted: &str, wanted: &str) -> bool {
    let tail = |scope: &str| scope.rsplit('/').next().unwrap_or(scope).to_ascii_lowercase();
    granted
        .split_whitespace()
        .any(|scope| scope.eq_ignore_ascii_case(wanted) || (!tail(wanted).is_empty() && tail(scope) == tail(wanted)))
}

/// The claims of a JWT's payload, not verified.
pub(super) fn jwt_claims(token: &str) -> Option<serde_json::Map<String, Json>> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    match serde_json::from_slice(&bytes).ok()? {
        Json::Object(map) => Some(map),
        _ => None,
    }
}

/// `actual` is the issuer `pattern`: the same with or without `https://`, `{tenantid}` standing for one path part.
fn issuer_matches(pattern: &str, actual: &str) -> bool {
    if pattern.is_empty() {
        return true;
    }
    let bare = |text: &str| {
        text.trim()
            .trim_start_matches("https://")
            .trim_end_matches('/')
            .to_string()
    };
    let (pattern, actual) = (bare(pattern), bare(actual));
    match pattern.split_once("{tenantid}") {
        None => pattern == actual,
        Some((before, after)) => {
            actual.len() > before.len() + after.len()
                && actual.starts_with(before)
                && actual.ends_with(after)
                && !actual[before.len()..actual.len() - after.len()].contains('/')
        }
    }
}

fn check_id_token(claims: &serde_json::Map<String, Json>, provider: &Provider, nonce: &str) -> Result<(), String> {
    let audience_ok = match claims.get("aud") {
        Some(Json::String(aud)) => *aud == provider.client_id,
        Some(Json::Array(list)) => list.iter().any(|aud| aud.as_str() == Some(provider.client_id.as_str())),
        _ => false,
    };
    if !audience_ok {
        return Err("the ID token is not for this server's client".to_string());
    }
    if !issuer_matches(&provider.issuer, &json_text(claims.get("iss"))) {
        return Err("the ID token is not from the provider".to_string());
    }
    let expires = value_number(Some(&moqui_lib::json::from_json(
        claims.get("exp").unwrap_or(&Json::Null),
    )));
    if expires.is_none_or(|exp| exp * 1000 < now_millis() - 300_000) {
        return Err("the ID token has expired".to_string());
    }
    if !same_secret(&json_text(claims.get("nonce")), nonce) {
        return Err("the ID token is not for this sign-in (nonce)".to_string());
    }
    Ok(())
}

/// The address in claims, by the provider's address claims; an `email_verified` of false refuses it.
fn claim_address(claims: &serde_json::Map<String, Json>, provider: &Provider) -> Result<Option<String>, Refused> {
    let unverified = match claims.get("email_verified") {
        Some(Json::Bool(verified)) => !verified,
        Some(Json::String(text)) => text.eq_ignore_ascii_case("false"),
        _ => false,
    };
    if unverified {
        return Err(refused(
            "noAddress",
            format!("{} has not verified the address of this account.", provider.name),
        ));
    }
    Ok(provider
        .address_claims
        .iter()
        .map(|claim| json_text(claims.get(claim)))
        .find(|text| text.contains('@') && !text.contains(char::is_whitespace)))
}

fn signed_in_address(provider: &Provider, tokens: &Tokens, nonce: &str) -> Result<String, Refused> {
    if !tokens.id_token.is_empty() {
        let claims = jwt_claims(&tokens.id_token)
            .ok_or_else(|| refused("exchangeFailed", "The provider's ID token is not readable."))?;
        check_id_token(&claims, provider, nonce)
            .map_err(|text| refused("exchangeFailed", format!("Refused: {text}.")))?;
        if let Some(address) = claim_address(&claims, provider)? {
            return Ok(address);
        }
    }
    if !provider.user_info_url.is_empty() {
        let answer = get_bearer(&provider.user_info_url, &tokens.access)
            .map_err(|err| refused("noAddress", format!("The account's address could not be read: {err}")))?;
        if answer.ok() {
            if let Json::Object(claims) = &answer.json {
                if let Some(address) = claim_address(claims, provider)? {
                    return Ok(address);
                }
            }
        }
    }
    Err(refused(
        "noAddress",
        format!("{} did not say the address of the account.", provider.name),
    ))
}

/// The server fields of a sign-in.
fn sign_in_fields(provider: &Provider, tokens: &Tokens, address: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("authTypeEnumId", Value::string(OAUTH)),
        ("oauthProviderEnumId", Value::string(&provider.id)),
        ("oauthRefreshToken", Value::string(&tokens.refresh)),
        ("oauthSignInDate", stamp_millis(now_millis())),
        ("mailUsername", Value::string(address)),
        ("mailPassword", Value::Null),
    ]
}

/// The provider's servers for an account's `EmailServer`: its IMAP server, or with `pop` its POP3 server, and SMTP.
fn provider_server_fields(provider: &Provider, pop: bool) -> Vec<(&'static str, Value)> {
    let text = |value: &str| {
        if value.is_empty() {
            Value::Null
        } else {
            Value::string(value)
        }
    };
    let (store_host, store_port, store_protocol) = if pop {
        (&provider.pop_host, &provider.pop_port, &provider.pop_protocol)
    } else {
        (&provider.store_host, &provider.store_port, &provider.store_protocol)
    };
    vec![
        ("storeHost", text(store_host)),
        ("storePort", text(store_port)),
        (
            "storeProtocol",
            text(if store_host.is_empty() { "" } else { store_protocol }),
        ),
        ("smtpHost", text(&provider.smtp_host)),
        ("smtpPort", text(&provider.smtp_port)),
        (
            "smtpSsl",
            text(if provider.smtp_ssl.is_empty() {
                "N"
            } else {
                &provider.smtp_ssl
            }),
        ),
        (
            "smtpStartTls",
            text(if provider.smtp_start_tls.is_empty() {
                "Y"
            } else {
                &provider.smtp_start_tls
            }),
        ),
    ]
}

fn errors_or(text: &str) -> String {
    take_errors_string().unwrap_or_else(|| text.to_string())
}

/// The sign-in on an account the user manages: its own server gets the sign-in (one is made with the provider's
/// servers when it has none), its address changes when the user allowed it, and a failed sign-in is active again.
fn store_on_account(
    facade: &EntityFacade,
    pending: &Pending,
    provider: &Provider,
    tokens: &Tokens,
    address: &str,
) -> Result<(String, String), String> {
    let account_id = pending.account.clone();
    // the user must still manage it
    let (level, _) = account_access(&account_id).map_err(|err| err.to_string())?;
    if level < 3 {
        return Err(errors_or(&format!("You may not change email account {account_id}")));
    }
    force_new(facade, || {
        let account = facade
            .find(ACCOUNT)?
            .eq("emailAccountId", Value::string(&account_id))
            .for_update(true)
            .one()?
            .ok_or_else(|| LibError::new(format!("Email account {account_id} not found")))?;
        if field_text(&account, "statusId") == "EasRemoved" {
            return Err(LibError::new(format!("Email account {account_id} was removed")));
        }
        let mut server_id = field_text(&account, "emailServerId");
        let mut account_update = facade.make_value(ACCOUNT)?;
        account_update.set("emailAccountId", Value::string(&account_id));
        if server_id.is_empty() {
            let mut server = facade.make_value(SERVER)?;
            server.set_sequenced_id_primary(facade)?;
            for (name, value) in provider_server_fields(provider, pending.pop)
                .into_iter()
                .chain(sign_in_fields(provider, tokens, address))
            {
                server.set(name, value);
            }
            for (name, value) in [("storeDelete", "N"), ("storeMarkSeen", "N"), ("storeSkipSeen", "N")] {
                server.set(name, Value::string(value));
            }
            server.create(facade)?;
            server_id = field_text(&server, "emailServerId");
            account_update.set("emailServerId", Value::string(&server_id));
            account_update.set("ownsEmailServer", Value::string("Y"));
        } else {
            if field_text(&account, "ownsEmailServer") != "Y" {
                return Err(LibError::new(format!(
                    "Email account {account_id} uses a shared server, which is not changed through the account"
                )));
            }
            let current = facade
                .find(SERVER)?
                .eq("emailServerId", Value::string(&server_id))
                .use_cache(false)
                .one()?;
            let mut server = facade.make_value(SERVER)?;
            server.set("emailServerId", Value::string(&server_id));
            let empty_hosts = current
                .as_ref()
                .is_none_or(|row| field_text(row, "storeHost").is_empty() && field_text(row, "smtpHost").is_empty());
            if empty_hosts {
                for (name, value) in provider_server_fields(provider, pending.pop) {
                    server.set(name, value);
                }
            }
            for (name, value) in sign_in_fields(provider, tokens, address) {
                server.set(name, value);
            }
            server.update(facade)?;
        }
        let old_address = field_text(&account, "emailAddress");
        if !old_address.eq_ignore_ascii_case(address) {
            account_update.set("emailAddress", Value::string(address));
            for identity in facade
                .find(IDENTITY)?
                .eq("emailAccountId", Value::string(&account_id))
                .list()?
                .iter()
            {
                if field_text(identity, "emailAddress").eq_ignore_ascii_case(&old_address) {
                    let mut changed = facade.make_value(IDENTITY)?;
                    changed.set(
                        "emailIdentityId",
                        Value::string(field_text(identity, "emailIdentityId")),
                    );
                    changed.set("emailAddress", Value::string(address));
                    changed.update(facade)?;
                }
            }
        }
        if matches!(field_text(&account, "statusId").as_str(), "EasAuthFailed" | "EasError") {
            account_update.set("statusId", Value::string("EasActive"));
            account_update.set("lastSyncError", Value::Null);
        }
        account_update.update(facade)?;
        Ok((account_id.clone(), server_id))
    })
    .map_err(|err| format!("The sign-in could not be kept: {err}"))
}

/// A new account of the user's (create#EmailAccount) on the provider's servers, signed in: paused until the app finishes
/// its setup, or active with a sync window, as start#OAuthSignIn was asked.
fn store_new_account(
    facade: &EntityFacade,
    pending: &Pending,
    provider: &Provider,
    tokens: &Tokens,
    address: &str,
) -> Result<(String, String), String> {
    let made = force_new(facade, || {
        let mut parameters: BTreeMap<String, Value> = BTreeMap::new();
        parameters.insert("emailAddress".into(), Value::string(address));
        parameters.insert("accountKindEnumId".into(), Value::string(&pending.account_kind));
        if !pending.account_name.is_empty() {
            parameters.insert("accountName".into(), Value::string(&pending.account_name));
        }
        if !pending.display_name.is_empty() {
            parameters.insert("displayName".into(), Value::string(&pending.display_name));
        }
        // on the provider's POP3 server when the sign-in was for a POP account (its scope was asked and granted)
        for (name, value) in provider_server_fields(provider, pending.pop) {
            if !value.is_null() {
                parameters.insert(name.to_string(), value);
            }
        }
        parameters.insert("mailUsername".into(), Value::string(address));
        parameters.insert("authTypeEnumId".into(), Value::string(OAUTH));
        parameters.insert("oauthProviderEnumId".into(), Value::string(&provider.id));
        if pending.sync_window_days > 0 {
            parameters.insert("syncWindowDays".into(), Value::I64(pending.sync_window_days));
        }
        // a sign-in started before the status was kept is paused too
        let status = if pending.account_status == "EasActive" {
            "EasActive"
        } else {
            "EasPaused"
        };
        parameters.insert("statusId".into(), Value::string(status));
        let created = crate::service::sync(&format!("{SERVICES}.create#EmailAccount"), &parameters)
            .map_err(|err| LibError::new(err.to_string()))?;
        if let Some(errors) = take_errors_string() {
            return Err(LibError::new(errors));
        }
        let account_id = created.get("emailAccountId").map(to_plain_string).unwrap_or_default();
        let server_id = created.get("emailServerId").map(to_plain_string).unwrap_or_default();
        if account_id.is_empty() || server_id.is_empty() {
            return Err(LibError::new("the account was not made"));
        }
        let mut server = facade.make_value(SERVER)?;
        server.set("emailServerId", Value::string(&server_id));
        for (name, value) in sign_in_fields(provider, tokens, address) {
            server.set(name, value);
        }
        server.update(facade)?;
        Ok((account_id, server_id))
    });
    made.map_err(|err| format!("The account could not be made: {err}"))
}

/// The JSON of the page's message: `<`, `>`, `&` and the line separators escaped, so it cannot end the script.
fn script_json(value: &Json) -> String {
    value
        .to_string()
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// The popup's page: the outcome, then a message to the opener on this page's own origin and on the BroadcastChannel
/// (a provider page can cut `window.opener`). It closes itself after a success. No token is in it, and it loads
/// nothing; its CSP allows only its own script and style.
fn page_html(sign_in_id: &str, outcome: &Outcome, nonce: &str) -> String {
    let message = json!({
        "type": MESSAGE_TYPE,
        "signInId": sign_in_id,
        "status": outcome.status,
        "errorCode": outcome.error_code,
        "message": outcome.message,
        "emailAccountId": outcome.email_account_id,
        "emailAddress": outcome.email_address,
        "expectedAddress": outcome.expected_address,
    });
    let success = outcome.status == "success";
    let heading = if success {
        "Signed in"
    } else {
        "The sign-in did not finish"
    };
    format!(
        concat!(
            "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">",
            "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{heading}</title>",
            "<style nonce=\"{nonce}\">body{{font-family:system-ui,-apple-system,'Segoe UI',Roboto,sans-serif;margin:0;",
            "padding:32px 24px;color:#1b1b1f;background:#fdfbff;line-height:1.5}}h1{{font-size:1.25rem;margin:0 0 12px}}",
            "p{{margin:0 0 16px}}.error{{color:#ba1a1a}}button{{font:inherit;padding:8px 20px;border-radius:20px;",
            "border:1px solid #74777f;background:transparent;color:inherit;cursor:pointer}}",
            "@media (prefers-color-scheme: dark){{body{{color:#e3e2e6;background:#1b1b1f}}.error{{color:#ffb4ab}}}}</style>",
            "</head><body><h1>{heading}</h1><p class=\"{class}\">{text}</p>",
            "<p><button id=\"close\" type=\"button\">Close this window</button></p>",
            "<script nonce=\"{nonce}\">(function(){{var m={json};var o=window.location.origin;",
            "try{{if(window.opener&&!window.opener.closed)window.opener.postMessage(m,o)}}catch(e){{}}",
            "try{{if(typeof BroadcastChannel==='function'){{var c=new BroadcastChannel('{channel}');c.postMessage(m);c.close()}}}}catch(e){{}}",
            "document.getElementById('close').addEventListener('click',function(){{window.close()}});",
            "if(m.status==='success')setTimeout(function(){{window.close()}},1500)}})()</script></body></html>"
        ),
        heading = heading,
        nonce = nonce,
        class = if success { "" } else { "error" },
        text = html_escape(&outcome.message),
        json = script_json(&message),
        channel = MESSAGE_TYPE,
    )
}

fn send_page(sign_in_id: &str, outcome: &Outcome) {
    let nonce = random_text(18).unwrap_or_else(|_| "n".to_string());
    let csp = format!(
        "default-src 'none'; script-src 'nonce-{nonce}'; style-src 'nonce-{nonce}'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
    );
    crate::web::send_bytes_with_headers(
        200,
        vec![
            ("Content-Type".to_string(), "text/html;charset=UTF-8".to_string()),
            ("Content-Security-Policy".to_string(), csp),
            ("Referrer-Policy".to_string(), "no-referrer".to_string()),
            ("Cache-Control".to_string(), "no-store".to_string()),
        ],
        page_html(sign_in_id, outcome, &nonce).into_bytes(),
    );
}

/// `get#OAuthStatus`: a sign-in of this session by `signInId` (pending, success or error; unknown when it expired or
/// is another session's), or an account's sign-in state by `emailAccountId`. Never a token.
pub(super) fn sign_in_status(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let sign_in_id = param_text(parameters, "signInId");
    let account_id = param_text(parameters, "emailAccountId");
    let user = user_id();
    if !sign_in_id.is_empty() {
        let entries = read_pending(session_attribute(PENDING));
        let entry = entries.get(&sign_in_id).filter(|entry| entry.user == user);
        let mut out = match entry {
            None => outcome_out(
                &sign_in_id,
                &failed(
                    "unknown",
                    "This sign-in is not known here (expired or another session's).",
                ),
            ),
            Some(entry) => match &entry.outcome {
                Some(outcome) => outcome_out(&sign_in_id, outcome),
                None if entry.expires < now_millis() => outcome_out(
                    &sign_in_id,
                    &failed("expired", "The sign-in took too long. Start it again."),
                ),
                None => {
                    let mut out = outcome_out(
                        &sign_in_id,
                        &Outcome {
                            status: "pending".to_string(),
                            ..Outcome::default()
                        },
                    );
                    text_out(&mut out, "expectedAddress", &entry.address);
                    out
                }
            },
        };
        if let Some(entry) = entry {
            text_out(&mut out, "oauthProviderEnumId", &entry.provider);
        }
        return Ok(out);
    }
    let mut out = BTreeMap::new();
    if account_id.is_empty() {
        note_error("signInId or emailAccountId is needed");
        return Ok(out);
    }
    let (level, account) = account_access(&account_id)?;
    if crate::service::util::has_error() {
        return Ok(out);
    }
    if level < 1 {
        note_error(format!("Email account {account_id} not found"));
        return Ok(out);
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let server_id = account.get("emailServerId").map(to_plain_string).unwrap_or_default();
    let server = find_one(facade, SERVER, "emailServerId", &server_id)?;
    let auth_type = server
        .as_ref()
        .map(|row| field_text(row, "authTypeEnumId"))
        .unwrap_or_default();
    let provider_id = server
        .as_ref()
        .map(|row| field_text(row, "oauthProviderEnumId"))
        .unwrap_or_default();
    let oauth = auth_type == OAUTH;
    let signed_in = oauth
        && server
            .as_ref()
            .is_some_and(|row| !field_text(row, "oauthRefreshToken").is_empty());
    let status = account.get("statusId").map(to_plain_string).unwrap_or_default();
    let provider = if provider_id.is_empty() {
        None
    } else {
        load_provider(facade, &provider_id)?
    };
    text_out(&mut out, "authTypeEnumId", &auth_type);
    text_out(&mut out, "oauthProviderEnumId", &provider_id);
    out.insert("signedIn".into(), Value::Bool(signed_in));
    out.insert(
        "needsSignIn".into(),
        Value::Bool(oauth && (!signed_in || status == "EasAuthFailed")),
    );
    out.insert(
        "oauthConfigured".into(),
        Value::Bool(provider.as_ref().is_some_and(Provider::configured)),
    );
    out.insert(
        "signInDate".into(),
        server
            .as_ref()
            .and_then(|row| row.get("oauthSignInDate").cloned())
            .unwrap_or(Value::Null),
    );
    Ok(out)
}

fn text_list(items: &[String]) -> Value {
    Value::List(items.iter().map(Value::string).collect())
}

/// `get#OAuthProviders`: each provider, whether this server is set up for it, how an address is known to be its mail
/// (address domains, host and MX suffixes), and the redirect URI to register. No secret.
pub(super) fn providers_service(
    _parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let list = load_providers(facade)?
        .into_iter()
        .map(|provider| {
            let mut map = IndexMap::new();
            map.insert("oauthProviderEnumId".to_string(), Value::string(&provider.id));
            map.insert("description".to_string(), Value::string(&provider.name));
            map.insert("configured".to_string(), Value::Bool(provider.configured()));
            map.insert("addressDomainList".to_string(), text_list(&provider.address_domains));
            map.insert("hostSuffixList".to_string(), text_list(&provider.host_suffixes));
            map.insert("mxSuffixList".to_string(), text_list(&provider.mx_suffixes));
            // the servers of an account made with it, IMAP or POP3, for the wizard to show or offer
            for (name, value) in [
                ("storeHost", &provider.store_host),
                ("storePort", &provider.store_port),
                ("storeProtocol", &provider.store_protocol),
                ("popHost", &provider.pop_host),
                ("popPort", &provider.pop_port),
                ("popProtocol", &provider.pop_protocol),
                ("smtpHost", &provider.smtp_host),
                ("smtpPort", &provider.smtp_port),
            ] {
                map.insert(name.to_string(), Value::string(value));
            }
            Value::Map(map)
        })
        .collect();
    let mut out = BTreeMap::new();
    out.insert("providerList".into(), Value::List(list));
    out.insert("redirectUri".into(), Value::string(redirect_uri()));
    Ok(out)
}

/// `find#OAuthProvider`: the provider whose mail an address (or an incoming server) is, with no network: by the
/// address's domain (addressDomains), then the server's host (hostSuffixes); the domain's MX hosts (mxSuffixes, a
/// company domain at Google Workspace or Microsoft 365) only with `lookupMx`, a DNS query of a few seconds at most.
/// Nothing found is no provider, not an error.
pub(super) fn find_provider_service(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    let address = param_text(parameters, "emailAddress").trim().to_ascii_lowercase();
    let domain = address
        .rsplit_once('@')
        .map(|(_, domain)| domain.trim().trim_end_matches('.').to_string())
        .unwrap_or_default();
    let host = param_text(parameters, "storeHost").trim().to_ascii_lowercase();
    let providers = {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        load_providers(facade)?
    };
    let by = |test: &dyn Fn(&Provider) -> bool| providers.iter().find(|provider| test(provider)).cloned();
    let mut found = (!domain.is_empty())
        .then(|| by(&|provider| provider.address_domains.contains(&domain)))
        .flatten()
        .map(|provider| (provider, "domain"));
    if found.is_none() && !host.is_empty() {
        found = by(&|provider| provider.host_suffixes.iter().any(|suffix| host_under(&host, suffix)))
            .map(|provider| (provider, "host"));
    }
    if found.is_none()
        && flag(parameters, "lookupMx")
        && super::discover::lookup_domain(&domain)
        && !facade.sim_active()
        && !crate::entity::fence::refuses(crate::entity::fence::Effect::RemoteCall, "an MX lookup")
    {
        let mx = super::discover::mx_hosts(&domain);
        found = detect(&providers, &domain, &[], &mx).map(|provider| (provider, "mx"));
    }
    match found {
        Some((provider, matched)) => {
            text_out(&mut out, "oauthProviderEnumId", &provider.id);
            text_out(&mut out, "providerName", &provider.name);
            out.insert("configured".into(), Value::Bool(provider.configured()));
            text_out(&mut out, "matchedBy", matched);
        }
        None => {
            out.insert("oauthProviderEnumId".into(), Value::Null);
            out.insert("configured".into(), Value::Bool(false));
        }
    }
    Ok(out)
}

/// Revoke a refresh token at its provider, best effort and logged (RFC 7009).
fn revoke_now(provider: &Provider, token: &str, server_id: &str) {
    let pairs = vec![
        ("token".to_string(), token.to_string()),
        ("token_type_hint".to_string(), "refresh_token".to_string()),
        ("client_id".to_string(), provider.client_id.clone()),
        ("client_secret".to_string(), provider.client_secret.clone()),
    ];
    match post_form(&provider.revoke_url, &pairs) {
        Ok(answer) if answer.ok() => {
            tracing::info!("Email server {server_id}: the {} sign-in was revoked", provider.name);
        }
        Ok(answer) => {
            let (code, description) = answer.refusal();
            tracing::warn!(
                "Email server {server_id}: {} did not revoke the sign-in (HTTP {}, {code} {description}); it is deleted here",
                provider.name,
                answer.status
            );
        }
        Err(err) => tracing::warn!(
            "Email server {server_id}: could not revoke the {} sign-in ({err}); it is deleted here",
            provider.name
        ),
    }
}

/// Revoke a server's sign-in at the provider after the caller's commit (now with no transaction) and delete it here.
/// False when there was nothing to revoke at the provider.
fn revoke_server(facade: &EntityFacade, server_id: &str, clear: bool) -> Result<bool, LibError> {
    let Some(server) = facade
        .find(SERVER)?
        .eq("emailServerId", Value::string(server_id))
        .use_cache(false)
        .one()?
    else {
        return Ok(false);
    };
    forget(facade, server_id);
    let token = field_text(&server, "oauthRefreshToken");
    if clear && !token.is_empty() {
        let mut update = facade.make_value(SERVER)?;
        update.set("emailServerId", Value::string(server_id));
        update.set("oauthRefreshToken", Value::Null);
        update.update(facade)?;
    }
    if token.is_empty() || token == DECRYPT_FAILED {
        return Ok(false);
    }
    let provider_id = field_text(&server, "oauthProviderEnumId");
    let Some(provider) = load_provider(facade, &provider_id)? else {
        return Ok(false);
    };
    if !provider.has_revocation() {
        tracing::info!(
            "Email server {server_id}: {} has no revocation endpoint; the sign-in is deleted here only",
            provider.name
        );
        return Ok(false);
    }
    if provider.client_id.is_empty() || provider.client_secret.is_empty() {
        tracing::warn!(
            "Email server {server_id}: {} is not set up, so the sign-in is deleted here only",
            provider.name
        );
        return Ok(false);
    }
    // the provider revokes the whole grant of the user to this client: not while another server here signs in with it
    let username = field_text(&server, "mailUsername");
    let others = facade
        .find(SERVER)?
        .eq("oauthProviderEnumId", Value::string(&provider_id))
        .condition(Condition::not_equals("emailServerId", Value::string(server_id)))
        .condition(Condition::is_not_null("oauthRefreshToken"))
        .use_cache(false)
        .list()?;
    if others
        .iter()
        .any(|row| field_text(row, "mailUsername").eq_ignore_ascii_case(&username))
    {
        tracing::info!(
            "Email server {server_id}: another server here signs in as {username} with {}, so the grant is not revoked; the sign-in is deleted here",
            provider.name
        );
        return Ok(false);
    }
    let server_id = server_id.to_string();
    let revoke = move || revoke_now(&provider, &token, &server_id);
    match facade.default_transactions() {
        Ok(tx) if tx.in_transaction() => {
            if let Err(err) = tx.on_commit(revoke) {
                tracing::warn!("Could not wait for the commit to revoke an email sign-in: {err}");
            }
        }
        _ => revoke(),
    }
    Ok(true)
}

/// `revoke#OAuthToken`: internal (authorization disabled), for delete#EmailAccount and update#EmailAccount.
pub(super) fn revoke_token_service(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    if !ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(false) {
        note_error("revoke#OAuthToken is internal");
        return Ok(out);
    }
    let server_id = param_text(parameters, "emailServerId");
    let clear = !matches!(parameters.get("clearToken"), Some(Value::Bool(false)))
        && !matches!(
            parameters.get("clearToken").map(to_plain_string).as_deref(),
            Some("false" | "N")
        );
    let revoked = match revoke_server(facade, &server_id, clear) {
        Ok(revoked) => revoked,
        Err(err) => {
            tracing::error!("Email server {server_id}: the sign-in could not be revoked: {err}");
            false
        }
    };
    out.insert("revoked".into(), Value::Bool(revoked));
    Ok(out)
}

/// `revoke#OAuthSignIn`: the user signs an account out of its provider (manage permission). The token is revoked
/// there and deleted here, and the account waits for a new sign-in (EasAuthFailed).
pub(super) fn sign_out_service(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    let account_id = param_text(parameters, "emailAccountId");
    let (level, account) = account_access(&account_id)?;
    if crate::service::util::has_error() {
        return Ok(out);
    }
    if level < 1 {
        note_error(format!("Email account {account_id} not found"));
        return Ok(out);
    }
    if level < 3 {
        note_error(format!("You may not change email account {account_id}"));
        return Ok(out);
    }
    let server_id = account.get("emailServerId").map(to_plain_string).unwrap_or_default();
    if server_id.is_empty() || account.get("ownsEmailServer") != Some(&Value::Bool(true)) {
        note_error(format!("Email account {account_id} has no sign-in of its own"));
        return Ok(out);
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let server = find_one(facade, SERVER, "emailServerId", &server_id)?;
    if server.as_ref().map(|row| field_text(row, "authTypeEnumId")).as_deref() != Some(OAUTH) {
        note_error(format!("Email account {account_id} does not sign in with a provider"));
        return Ok(out);
    }
    let provider_name = server
        .as_ref()
        .map(|row| enum_description(facade, &field_text(row, "oauthProviderEnumId")))
        .unwrap_or_default();
    let revoked = revoke_server(facade, &server_id, true)?;
    let mut update = facade.make_value(ACCOUNT)?;
    update.set("emailAccountId", Value::string(&account_id));
    update.set("statusId", Value::string("EasAuthFailed"));
    update.set(
        "lastSyncError",
        Value::string(format!(
            "Signed out of {provider_name}; sign in again to sync this account"
        )),
    );
    update.update(facade)?;
    tracing::info!("Email account {account_id} signed out of {provider_name}");
    out.insert("revoked".into(), Value::Bool(revoked));
    Ok(out)
}

// ---- XOAUTH2 ----

/// The SASL XOAUTH2 initial response (before base64): `user=` the mailbox, `auth=Bearer` the access token.
pub(super) fn xoauth2(user: &str, token: &str) -> String {
    format!("user={user}\x01auth=Bearer {token}\x01\x01")
}

/// IMAP `AUTHENTICATE XOAUTH2`: the initial response at the first (empty) challenge; an empty answer to the error
/// challenge a server sends on a refused token, so it ends with its tagged NO.
pub(super) struct XOauth2 {
    pub(super) response: String,
}

impl imap::Authenticator for XOauth2 {
    type Response = Vec<u8>;

    fn process(&self, challenge: &[u8]) -> Self::Response {
        if challenge.is_empty() {
            self.response.as_bytes().to_vec()
        } else {
            Vec::new()
        }
    }
}

/// The decoded XOAUTH2 response a server got: the user and the token. For the fake servers of the tests.
#[cfg(test)]
pub(super) fn parse_xoauth2(base64: &str) -> Option<(String, String)> {
    let bytes = STANDARD.decode(base64.trim()).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    let mut user = None;
    let mut token = None;
    for part in text.split('\x01') {
        if let Some(rest) = part.strip_prefix("user=") {
            user = Some(rest.to_string());
        } else if let Some(rest) = part.strip_prefix("auth=Bearer ") {
            token = Some(rest.to_string());
        }
    }
    Some((user?, token?))
}

#[cfg(test)]
mod tests {
    use super::{challenge, issuer_matches, scope_granted, script_json, xoauth2};

    #[test]
    fn pkce_s256_matches_rfc_7636() {
        // RFC 7636 appendix B
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn issuers_scopes_and_the_page_script() {
        assert!(issuer_matches("https://accounts.google.com", "accounts.google.com"));
        assert!(issuer_matches(
            "https://accounts.google.com",
            "https://accounts.google.com"
        ));
        assert!(!issuer_matches(
            "https://accounts.google.com",
            "https://evil.example.com"
        ));
        let microsoft = "https://login.microsoftonline.com/{tenantid}/v2.0";
        assert!(issuer_matches(
            microsoft,
            "https://login.microsoftonline.com/9188040d-6c67-4c5b-b112-36a304b66dad/v2.0"
        ));
        assert!(!issuer_matches(microsoft, "https://login.microsoftonline.com/v2.0"));
        assert!(!issuer_matches(microsoft, "https://login.microsoftonline.com/a/b/v2.0"));
        assert!(scope_granted(
            "openid https://mail.google.com/ email",
            "https://mail.google.com/"
        ));
        assert!(!scope_granted("openid email", "https://mail.google.com/"));
        assert!(scope_granted(
            "IMAP.AccessAsUser.All SMTP.Send",
            "https://outlook.office.com/SMTP.Send"
        ));
        let text = script_json(&serde_json::json!({"message": "</script><b>&\u{2028}"}));
        assert!(
            !text.contains('<') && !text.contains('>') && !text.contains('&'),
            "{text}"
        );
        assert_eq!(xoauth2("a@b.c", "tok"), "user=a@b.c\u{1}auth=Bearer tok\u{1}\u{1}");
    }
}
