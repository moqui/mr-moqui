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

//! Connect (plans/design-dev-aide.md 21.2, D20): a group server is a `DevAideServer` row. Connect asks the server what
//! it offers, then signs in there as an installed app does (F17: the server's `/sso/appLogin` in the browser with a PKCE
//! challenge and this instance's own address as the loopback redirect, the code taken at `/sso/appToken` for a login
//! key), or takes a token pasted from the server's Git access page. The app sign-in's key is the account's, so Connect
//! keeps none of it (question 18): it makes a key for git alone (git and the framework's git and hosting REST), one for
//! tasks alone (the task source, an issue) and one for the gateway alone, then ends the account's key, unless the offer
//! names an application (Smithy), whose REST takes only an installed app's key for the Dev AIDE's operations (mr-pass
//! design-smithy 5.5): that key is kept for the application's REST alone. A server that makes no such keys keeps the
//! account's key, as a pasted token is kept. What it then sets, each shown and confirmed: the `myaddons.xml` repository
//! of the server's git URL, a `moqui-rest` task source, the `.env` lines of the LLM gateway (with the key for the gateway
//! alone, never the account's), and git's credential for the server's host. Remove takes back what it set and nothing
//! else: the `.env` lines go back to what they were where they are still Connect's, a `myaddons.xml` repository it did
//! not write stays, and the keys it was given are ended on the server, each by itself.

use std::path::{Path, PathBuf};

use base64::Engine;
use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};

use super::client::{self, field, normalize_base, on_server, query_text, Server, CREDENTIAL, OFFER_PATH, SERVER};
use crate::tier::Tier;
use crate::workspace::{flag, json_value, text, Context};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(get_servers), get_servers)?;
    registry.service_function(moqui_core::service_location!(start_connect), start_connect)?;
    registry.service_function(moqui_core::service_location!(finish_connect), finish_connect)?;
    registry.service_function(moqui_core::service_location!(store_server_token), store_server_token)?;
    registry.service_function(moqui_core::service_location!(refresh_server), refresh_server)?;
    registry.service_function(moqui_core::service_location!(apply_server_setup), apply_server_setup)?;
    registry.service_function(moqui_core::service_location!(remove_server), remove_server)
}

/// How long a sign-in in the browser may take.
const PENDING_MILLIS: i64 = 15 * 60_000;
/// Where the server's sign-in hands the code back: this instance's own page, a loopback address on a laptop.
pub(crate) const SIGNED_IN_PATH: &str = "/qapps/dev/Settings/serverSignedIn";
/// The `.env` lines of the default model endpoint pointed at a gateway, by the environment names of its `url` and
/// `api-key` and of the default model's `name` (docs/Run-Deploy.md).
const ENV_URL: &str = "moqui_conf__llm_facade__endpoint__DEFAULT__url";
const ENV_KEY: &str = "moqui_conf__llm_facade__endpoint__DEFAULT__api_key";
const ENV_MODEL: &str = "moqui_conf__llm_facade__model__DEFAULT__name";

pub(crate) fn current_user() -> String {
    moqui_core::user::current_user_id().ok().flatten().unwrap_or_default()
}

/// A connection is made by a person in a session, never by a program logged in by a header (15, 21.9).
fn session_only() -> Result<(), LibError> {
    if moqui_core::screen::gate::header_authenticated_request() {
        return Err(LibError::new(
            "A group server is connected from a session, not a request logged in by a header",
        ));
    }
    Ok(())
}

/// A short name of a label or host for the `myaddons.xml` repository: letters, digits and `-`.
#[must_use]
pub fn repository_name(label: &str, base: &str) -> String {
    let source = if label.trim().is_empty() {
        base.split_once("://")
            .map_or(base, |(_, rest)| rest)
            .split(['/', ':'])
            .next()
            .unwrap_or("")
    } else {
        label
    };
    let mut name = String::new();
    for ch in source.chars() {
        if ch.is_ascii_alphanumeric() {
            name.push(ch.to_ascii_lowercase());
        } else if !name.ends_with('-') && !name.is_empty() {
            name.push('-');
        }
    }
    let name = name.trim_end_matches('-').to_string();
    if name.is_empty() {
        "group".into()
    } else {
        name
    }
}

/// The S256 challenge of a PKCE verifier.
fn challenge_of(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// This instance's own origin (`http://127.0.0.1:8080`), from the request; `localhost` as the IP literal, the loopback
/// redirect a server takes (RFC 8252 §8.3).
fn own_origin() -> Option<String> {
    let url = moqui_core::web::request_url()?;
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split('/').next()?;
    (!authority.is_empty()).then(|| redirect_origin(&format!("{scheme}://{authority}")))
}

/// `origin` with a `localhost` host as `127.0.0.1`.
#[must_use]
pub fn redirect_origin(origin: &str) -> String {
    match url::Url::parse(origin) {
        Ok(mut parsed)
            if parsed
                .host_str()
                .is_some_and(|host| host.eq_ignore_ascii_case("localhost")) =>
        {
            if parsed.set_host(Some("127.0.0.1")).is_ok() {
                parsed.as_str().trim_end_matches('/').to_string()
            } else {
                origin.to_string()
            }
        }
        _ => origin.to_string(),
    }
}

/// The user's row of a server by its base URL, made when there is none.
fn server_row(context: &Context, base: &str, label: &str) -> Result<moqui_core::entity::EntityValue, LibError> {
    let user = current_user();
    let found = context
        .facade
        .find(SERVER)?
        .eq("userId", Value::string(&user))
        .eq("baseUrl", Value::string(base))
        .one()?;
    let mut row = match found {
        Some(row) => row,
        None => {
            let mut row = context.facade.make_value(SERVER)?;
            row.set_sequenced_id_primary(&context.facade)?;
            row.set("userId", Value::string(&user));
            row.set("baseUrl", Value::string(base));
            row
        }
    };
    if !label.trim().is_empty() || field(&row, "label").is_empty() {
        row.set(
            "label",
            Value::string(Some(label.trim()).filter(|label| !label.is_empty()).unwrap_or(base)),
        );
    }
    Ok(row)
}

/// The user's own row of a server (not the instance's).
fn own_row(context: &Context, id: &str) -> Result<moqui_core::entity::EntityValue, LibError> {
    context
        .facade
        .find(SERVER)?
        .eq("serverId", Value::string(id))
        .one()?
        .filter(|row| field(row, "userId") == current_user())
        .ok_or_else(|| LibError::new(format!("no group server {id} of yours")))
}

/// Ask the server what it offers (as the user, when there is a token) and keep it on the row, with the state: connected
/// (the token is good), signed-out (it is not), reachable (no token), or unreachable with the error.
fn refresh(context: &Context, row: &mut moqui_core::entity::EntityValue) -> Result<Server, LibError> {
    let mut server = Server::from_row(&context.facade, row);
    match server.get(OFFER_PATH, &[]) {
        Ok(answer) => {
            let offer = answer["offer"].clone();
            let signed_in = offer["user"]["signedIn"].as_bool().unwrap_or(false);
            let state = match (&server.token, signed_in) {
                (Some(_), true) => "connected",
                (Some(_), false) => "signed-out",
                (None, _) => "reachable",
            };
            row.set("offerJson", Value::string(offer.to_string()));
            row.set("offerDate", moqui_core::user::now_value());
            row.set("statusText", Value::string(state));
            row.set("lastError", Value::Null);
            if signed_in {
                row.set(
                    "username",
                    Value::string(offer["user"]["username"].as_str().unwrap_or("")),
                );
                if field(row, "connectedDate").is_empty() {
                    row.set("connectedDate", moqui_core::user::now_value());
                }
            }
            server.offer = offer;
        }
        Err(err) => {
            row.set("statusText", Value::string("unreachable"));
            row.set("lastError", Value::string(err.to_string()));
        }
    }
    row.create_or_update(&context.facade)?;
    server.username = field(row, "username");
    Ok(server)
}

/// Keep the user's token for a server, sent only to its address.
fn store_token(context: &Context, server_id: &str, base: &str, token: &str, username: &str) -> Result<(), LibError> {
    let user = current_user();
    let mut use_for_git = None;
    for old in context
        .facade
        .find(CREDENTIAL)?
        .eq("serverId", Value::string(server_id))
        .eq("userId", Value::string(&user))
        .list()?
        .iter()
    {
        use_for_git = Some(field(old, "useForGit"));
        old.delete(&context.facade)?;
    }
    let mut row = context.facade.make_value(CREDENTIAL)?;
    row.set_sequenced_id_primary(&context.facade)?;
    row.set("serverId", Value::string(server_id));
    row.set("userId", Value::string(&user));
    row.set("username", Value::string(username));
    row.set("token", Value::string(token.trim()));
    row.set(
        "urlPrefix",
        Value::string(crate::vcs::url_prefix(base).unwrap_or_default()),
    );
    if let Some(use_for_git) = use_for_git {
        row.set("useForGit", Value::string(use_for_git));
    }
    row.create(&context.facade)?;
    Ok(())
}

/// A server as the page shows it: its state, what it offers and what Connect set (no token).
fn server_json(context: &Context, server: &Server, row: &moqui_core::entity::EntityValue) -> Json {
    json!({
        "serverId": server.id, "label": server.label, "baseUrl": server.base, "username": server.username,
        "mine": !server.user_id.is_empty(), "hasToken": server.token.is_some(),
        "statusText": field(row, "statusText"), "lastError": field(row, "lastError"),
        "offerDate": field(row, "offerDate"), "connectedDate": field(row, "connectedDate"),
        "offer": server.offer, "plan": plan(context, server),
        "setup": serde_json::from_str::<Json>(&field(row, "setupJson")).unwrap_or(Json::Null),
    })
}

/// `get#Servers`: the user's connections and the instance's, with their state, offer, plan and what Connect set.
fn get_servers(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    _parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let _authz = ec.disable_authz();
    let user = current_user();
    let rows = context.facade.find(SERVER)?.order_by("label", false).list()?;
    let servers: Vec<Json> = rows
        .iter()
        .filter(|row| {
            let owner = field(row, "userId");
            owner.is_empty() || owner == user
        })
        .map(|row| server_json(&context, &Server::from_row(&context.facade, row), row))
        .collect();
    let mut out = Parameters::new();
    out.insert("servers".into(), json_value(&servers));
    out.insert("ownOrigin".into(), own_origin().map_or(Value::Null, Value::string));
    Ok(out)
}

/// `start#Connect`: ask the server at `baseUrl` what it offers, keep the connection, and start the sign-in there: the
/// URL to open in the browser, with a PKCE challenge, a state and this instance's own page as the redirect. A server
/// that offers no app sign-in, or this instance at a non-loopback address the server does not list, is connected by a
/// pasted token instead (`store#ServerToken`).
fn start_connect(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    session_only()?;
    let _authz = ec.disable_authz();
    let base = normalize_base(&text(parameters, "baseUrl"))?;
    let mut row = server_row(&context, &base, &text(parameters, "label"))?;
    let server = refresh(&context, &mut row)?;
    if field(&row, "statusText") == "unreachable" {
        return Err(LibError::new(format!(
            "{base} did not answer what it offers: {}",
            field(&row, "lastError")
        )));
    }
    let origin = own_origin().ok_or_else(|| LibError::new("this request has no address to come back to"))?;
    let redirect = format!("{origin}{SIGNED_IN_PATH}");
    let verifier = moqui_core::user::random_string(64)?;
    let state = moqui_core::user::random_string(32)?;
    row.set("pendingState", Value::string(&state));
    row.set("pendingVerifier", Value::string(&verifier));
    row.set("pendingRedirect", Value::string(&redirect));
    row.set("pendingDate", moqui_core::user::now_value());
    row.update(&context.facade)?;
    let login = Some(server.offered("signIn.appLogin"))
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| "/sso/appLogin".into());
    let username = moqui_core::user::current_username().ok().flatten().unwrap_or_default();
    let device = format!("Dev AIDE ({username} at {origin})");
    let mut query = vec![
        ("codeChallenge", challenge_of(&verifier)),
        ("codeChallengeMethod", "S256".to_string()),
        ("state", state),
        ("redirectUri", redirect.clone()),
        ("deviceName", device),
    ];
    let flow = text(parameters, "authFlowId");
    if !flow.is_empty() {
        query.push(("authFlowId", flow));
    }
    let query: Vec<String> = query
        .iter()
        .map(|(key, value)| format!("{key}={}", query_text(value)))
        .collect();
    let mut out = Parameters::new();
    out.insert("serverId".into(), Value::string(&server.id));
    out.insert(
        "authorizeUrl".into(),
        Value::string(format!("{}?{}", server.url(&login)?, query.join("&"))),
    );
    out.insert("redirectUri".into(), Value::string(redirect));
    out.insert("loopback".into(), Value::Bool(loopback(&origin)));
    Ok(out)
}

/// Whether an origin is this machine's by its IP literal: the redirect a server takes from any installed app.
fn loopback(origin: &str) -> bool {
    match url::Url::parse(origin)
        .ok()
        .and_then(|parsed| parsed.host().map(|host| host.to_owned()))
    {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    }
}

/// The username of the user's token for a server, as the server answered it, where the token was kept without one
/// (an app sign-in): git's credential is the server's account, not this instance's.
fn name_token(context: &Context, server_id: &str, username: &str) -> Result<(), LibError> {
    if username.is_empty() {
        return Ok(());
    }
    for mut credential in context
        .facade
        .find(CREDENTIAL)?
        .eq("serverId", Value::string(server_id))
        .eq("userId", Value::string(current_user()))
        .list()?
        .into_values()
    {
        if field(&credential, "username").is_empty() {
            credential.set("username", Value::string(username));
            credential.update(&context.facade)?;
        }
    }
    Ok(())
}

/// Note on the connection how its token came: `app` (an app sign-in, whose key Remove ends on the server) or `pasted`.
fn note_token_source(
    context: &Context,
    row: &mut moqui_core::entity::EntityValue,
    source: &str,
) -> Result<(), LibError> {
    let mut setup: Json = serde_json::from_str(&field(row, "setupJson")).unwrap_or_else(|_| json!({}));
    setup["token"] = json!(source);
    row.set("setupJson", Value::string(setup.to_string()));
    row.update(&context.facade)?;
    Ok(())
}

/// The keys an app sign-in leaves on a connection (question 18): the git key as the token, the tasks key, the gateway
/// key where the server offers a gateway, and the installed app's key where an application takes only that.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SignInKeys {
    pub token: String,
    pub tasks: Option<String>,
    pub gateway: Option<String>,
    pub app: Option<String>,
    /// The sign-in's own key ends on the server: nothing here takes it.
    pub end_account_key: bool,
    /// Keys the server made that nothing here takes (a git key with no tasks key, or the reverse): ended on the server.
    pub orphans: Vec<String>,
}

/// What a sign-in keeps of the account's key `account` and the scoped keys the server made with it (`made`: git,
/// tasks, gateway, each `None` when the server did not make it). With the git and tasks keys made, the git key is the
/// token and the account's key is ended, unless `application` (an application's REST, which takes only an installed
/// app's key) keeps it for that REST alone; without them (a server that makes no such keys) the account's key stays
/// the token, as before, and the one of the two that was made is ended (`orphans`).
#[must_use]
pub fn sign_in_keys(account: &str, made: [Option<String>; 3], application: bool) -> SignInKeys {
    let [git, tasks, gateway] = made;
    match (git, tasks) {
        (Some(git), Some(tasks)) => SignInKeys {
            token: git,
            tasks: Some(tasks),
            gateway,
            app: application.then(|| account.to_string()),
            end_account_key: !application,
            orphans: Vec::new(),
        },
        (git, tasks) => SignInKeys {
            token: account.to_string(),
            gateway,
            orphans: git.into_iter().chain(tasks).collect(),
            ..SignInKeys::default()
        },
    }
}

/// A key for `scope` alone (`create#UserScopedKey` at the offer's key path), made with the account's key `account`;
/// `None` when the server makes none (an older server, or a gateway it does not run), which is logged.
fn make_scoped_key(server: &Server, account: &str, scope: &str, device: &str) -> Option<String> {
    let key_path = Some(server.offered("gateway.keyPath"))
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| "/rest/s1/moqui/hosting/gatewayKey".into());
    match server
        .with_token(account)
        .post(&key_path, &json!({ "scope": scope, "deviceName": device }))
    {
        Ok(answer) => answer["loginKey"]
            .as_str()
            .filter(|key| !key.is_empty())
            .map(str::to_string),
        Err(err) => {
            tracing::warn!("Dev AIDE: {} made no key for {scope} alone: {err}", server.base);
            None
        }
    }
}

/// `finish#Connect`: the server's sign-in came back with `code` and `state` (or `error`): the code is taken for a login
/// key with the PKCE verifier, the keys for git, tasks and the gateway alone are made with it and kept (the git key as
/// the user's token for the server), the account's key ended where nothing takes it, and the server asked again as
/// the user.
fn finish_connect(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    session_only()?;
    let _authz = ec.disable_authz();
    let state = text(parameters, "state");
    if state.is_empty() {
        return Err(LibError::new("no sign-in state"));
    }
    let found = context
        .facade
        .find(SERVER)?
        .eq("pendingState", Value::string(&state))
        .one()?
        .filter(|row| field(row, "userId") == current_user());
    let Some(mut row) = found else {
        return Err(LibError::new(
            "No group server sign-in of yours is waiting: connect again",
        ));
    };
    let verifier = field(&row, "pendingVerifier");
    let redirect = field(&row, "pendingRedirect");
    let started = row
        .get("pendingDate")
        .and_then(moqui_core::user::value_millis)
        .unwrap_or(0);
    for name in ["pendingState", "pendingVerifier", "pendingRedirect", "pendingDate"] {
        row.set(name, Value::Null);
    }
    row.update(&context.facade)?;
    if moqui_core::user::now_millis() - started > PENDING_MILLIS {
        return Err(LibError::new("The sign-in took too long: connect again"));
    }
    let error = text(parameters, "error");
    if !error.is_empty() {
        return Err(LibError::new(format!("The group server did not sign you in: {error}")));
    }
    let server = Server::from_row(&context.facade, &row);
    let token_path = Some(server.offered("signIn.appToken"))
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| "/sso/appToken".into());
    let form = [
        ("code", text(parameters, "code")),
        ("codeVerifier", verifier),
        ("redirectUri", redirect),
        ("deviceName", "Dev AIDE".to_string()),
    ]
    .iter()
    .map(|(key, value)| format!("{key}={}", query_text(value)))
    .collect::<Vec<_>>()
    .join("&");
    let answer = client::send(
        server.url(&token_path)?,
        moqui_core::outbound::Method::Post,
        vec![
            ("accept".into(), "application/json".into()),
            ("content-type".into(), "application/x-www-form-urlencoded".into()),
        ],
        Some(form),
        &server.base,
    )?;
    let key = answer["loginKey"].as_str().unwrap_or("").to_string();
    if key.is_empty() {
        return Err(LibError::new(format!("{} gave no login key", server.base)));
    }
    // the account's key opens everything there: the keys kept are for one service each (question 18), by what the
    // server offers now (its gateway, an application)
    let mut server = server;
    if let Ok(answer) = server.with_token(&key).get(OFFER_PATH, &[]) {
        server.offer = answer["offer"].clone();
    }
    let origin = own_origin().unwrap_or_default();
    let gateway = server.offer["gateway"]["enabled"].as_bool().unwrap_or(false);
    let made = [
        make_scoped_key(&server, &key, "git", &format!("Dev AIDE git ({origin})")),
        make_scoped_key(&server, &key, "tasks", &format!("Dev AIDE tasks ({origin})")),
        gateway
            .then(|| make_scoped_key(&server, &key, "gateway", &format!("Dev AIDE gateway ({origin})")))
            .flatten(),
    ];
    let keys = sign_in_keys(&key, made, server.application().is_some());
    // keys an earlier sign-in of this connection left are ended: these stand in for them (a pasted token may serve
    // elsewhere, and stays)
    let setup: Json = serde_json::from_str(&field(&row, "setupJson")).unwrap_or(Json::Null);
    if let Some(earlier) = server.token.as_deref().filter(|_| setup["token"] == "app") {
        revoke_on_server(&server, earlier, "the earlier sign-in's key");
    }
    store_token(&context, &server.id, &server.base, &keys.token, "")?;
    for name in ["tasksKey", "gatewayKey", "appKey"] {
        let earlier = field(&row, name);
        if !earlier.is_empty() {
            revoke_on_server(&server, &earlier, name);
        }
    }
    row.set("tasksKey", keys.tasks.as_deref().map_or(Value::Null, Value::string));
    row.set("gatewayKey", keys.gateway.as_deref().map_or(Value::Null, Value::string));
    row.set("appKey", keys.app.as_deref().map_or(Value::Null, Value::string));
    row.update(&context.facade)?;
    if keys.end_account_key {
        revoke_on_server(&server, &key, "the sign-in's account key");
    }
    for orphan in &keys.orphans {
        revoke_on_server(&server, orphan, "a key the sign-in does not keep");
    }
    let server = refresh(&context, &mut row)?;
    name_token(&context, &server.id, &server.username)?;
    note_token_source(&context, &mut row, "app")?;
    tracing::info!("Dev AIDE: {} connected to {}", current_user(), server.base);
    let mut out = Parameters::new();
    out.insert("serverId".into(), Value::string(&server.id));
    out.insert("statusText".into(), Value::string(field(&row, "statusText")));
    Ok(out)
}

/// `store#ServerToken`: an access token made on the server's Git access page, for a server already kept (`serverId`)
/// or a new one (`baseUrl`, `label`); the server is asked again as the user.
fn store_server_token(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    session_only()?;
    let _authz = ec.disable_authz();
    let token = text(parameters, "token");
    if token.trim().is_empty() {
        return Err(LibError::new("a token is required"));
    }
    let mut row = match text(parameters, "serverId") {
        id if !id.is_empty() => own_row(&context, &id)?,
        _ => {
            let row = server_row(
                &context,
                &normalize_base(&text(parameters, "baseUrl"))?,
                &text(parameters, "label"),
            )?;
            row.create_or_update(&context.facade)?;
            row
        }
    };
    let id = field(&row, "serverId");
    store_token(
        &context,
        &id,
        &field(&row, "baseUrl"),
        &token,
        &text(parameters, "username"),
    )?;
    let server = refresh(&context, &mut row)?;
    name_token(&context, &id, &server.username)?;
    note_token_source(&context, &mut row, "pasted")?;
    let mut out = Parameters::new();
    out.insert("serverId".into(), Value::string(id));
    out.insert("statusText".into(), Value::string(field(&row, "statusText")));
    Ok(out)
}

/// `refresh#Server` (Reconnect's check): ask the server again what it offers and whether the token is good.
fn refresh_server(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let _authz = ec.disable_authz();
    let mut row = own_row(&context, &text(parameters, "serverId"))?;
    let server = refresh(&context, &mut row)?;
    let mut out = Parameters::new();
    out.insert("server".into(), json_value(&server_json(&context, &server, &row)));
    Ok(out)
}

// ---- what Connect sets ----

/// Where `myaddons.xml` is: beside `addons.xml` above the runtime (a checkout), else in the runtime directory.
pub(crate) fn myaddons_path(runtime: &Path) -> PathBuf {
    match runtime.parent() {
        Some(parent) if parent.join("addons.xml").is_file() || parent.join("moq").is_file() => {
            parent.join("myaddons.xml")
        }
        _ => runtime.join("myaddons.xml"),
    }
}

/// `text` (a `myaddons.xml`, or none) with the git repository `name` at `url` (`${name}` in it), one element per line
/// as `moq` reads it, replacing one of that name; with `default`, `default-repository` set to it when the file names
/// none.
///
/// # Errors
///
/// When the file has no `</addons>` to add before.
pub fn with_repository(text: Option<&str>, name: &str, url: &str, default: bool) -> Result<String, String> {
    let attr = crate::workspace::xml_attr;
    let block = format!(
        "    <repository name=\"{}\">\n        <location type=\"git\" url=\"{}\"/>\n    </repository>\n",
        attr(name),
        attr(url)
    );
    let Some(text) = text else {
        let root = if default {
            format!("<addons default-repository=\"{}\">", attr(name))
        } else {
            "<addons>".into()
        };
        return Ok(format!("{root}\n{block}</addons>\n"));
    };
    let mut text = without_repository(text, name, false).0;
    let Some(close) = text.rfind("</addons>") else {
        return Err("myaddons.xml has no </addons> to add the repository before".into());
    };
    let line_start = text[..close].rfind('\n').map_or(0, |at| at + 1);
    let at = if text[line_start..close].trim().is_empty() {
        line_start
    } else {
        close
    };
    text.insert_str(at, &block);
    if default {
        if let Some(open) = text.find("<addons") {
            let tag_end = text[open..].find('>').map_or(text.len(), |end| open + end);
            if !text[open..tag_end].contains("default-repository") {
                text.insert_str(
                    open + "<addons".len(),
                    &format!(" default-repository=\"{}\"", attr(name)),
                );
            }
        }
    }
    Ok(text)
}

/// The `url` of the git location of repository `name` in a `myaddons.xml`, as the file has it (attribute text).
#[must_use]
pub fn repository_url(text: &str, name: &str) -> Option<String> {
    let open = format!("<repository name=\"{}\"", crate::workspace::xml_attr(name));
    let start = text.find(&open)?;
    let end = text[start..]
        .find("</repository>")
        .map_or(text.len(), |end| start + end);
    let block = &text[start..end];
    let at = block.find(" url=\"")? + " url=\"".len();
    block[at..].split_once('"').map(|(url, _)| url.to_string())
}

/// The `default-repository` a `myaddons.xml` names, if any.
#[must_use]
pub fn default_repository(text: &str) -> Option<String> {
    let open = text.find("<addons")?;
    let tag = &text[open..open + text[open..].find('>')?];
    let at = tag.find("default-repository=\"")? + "default-repository=\"".len();
    tag[at..].split_once('"').map(|(name, _)| name.to_string())
}

/// `text` without the repository `name`, and whether it had it; with `drop_default`, a `default-repository` naming it
/// goes too.
#[must_use]
pub fn without_repository(text: &str, name: &str, drop_default: bool) -> (String, bool) {
    let attr = crate::workspace::xml_attr(name);
    let mut text = text.to_string();
    let mut had = false;
    let open = format!("<repository name=\"{attr}\"");
    if let Some(start) = text.find(&open) {
        if let Some(end) = text[start..]
            .find("</repository>")
            .map(|end| start + end + "</repository>".len())
        {
            let line_start = text[..start].rfind('\n').map_or(0, |at| at + 1);
            let line_end = if text[end..].starts_with('\n') { end + 1 } else { end };
            let from = if text[line_start..start].trim().is_empty() {
                line_start
            } else {
                start
            };
            text.replace_range(from..line_end, "");
            had = true;
        }
    }
    let default = format!(" default-repository=\"{attr}\"");
    if drop_default {
        if let Some(at) = text.find(&default) {
            text.replace_range(at..at + default.len(), "");
        }
    }
    (text, had)
}

/// The REST path of the user's tasks: the server's offer, else Mantle's.
fn tasks_path(server: &Server) -> String {
    Some(server.offered("tasks.path"))
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| "/rest/s1/mantle/my/tasks".into())
}

/// What Connect would set from a server's offer, each with whether it is there already.
fn plan(context: &Context, server: &Server) -> Json {
    let name = repository_name(&server.label, &server.base);
    let file = myaddons_path(context.factory.runtime_dir());
    let present = std::fs::read_to_string(&file)
        .map(|text| text.contains(&format!("<repository name=\"{name}\"")))
        .unwrap_or(false);
    let task_source = context
        .facade
        .find("moqui.dev.DevAideTaskSource")
        .and_then(|find| {
            find.eq("serverId", Value::string(&server.id))
                .eq("userId", Value::string(current_user()))
                .count()
        })
        .unwrap_or(0)
        > 0;
    let gateway = server.offer["gateway"]["enabled"].as_bool().unwrap_or(false);
    let instance_files = context.tier == Tier::Full && !context.settings.shared;
    let use_for_git = context
        .facade
        .find(CREDENTIAL)
        .and_then(|find| {
            find.eq("serverId", Value::string(&server.id))
                .eq("userId", Value::string(current_user()))
                .eq("useForGit", Value::string("Y"))
                .count()
        })
        .unwrap_or(0)
        > 0;
    json!({
        "addons": {
            "available": !server.offered("git.httpsUrlTemplate").is_empty(), "repositoryName": name,
            "url": server.offered("git.httpsUrlTemplate"), "sshUrl": server.offered("git.sshUrlTemplate"),
            "file": file.display().to_string(), "present": present, "writable": instance_files,
        },
        "taskSource": { "available": server.token.is_some(), "path": tasks_path(server), "present": task_source },
        "gateway": {
            "available": gateway, "url": server.offered("gateway.url"), "alias": "default", "writable": instance_files,
            "lines": [format!("{ENV_URL}={}", server.offered("gateway.url")),
                format!("{ENV_KEY}=(a key for the gateway alone, made by the server)"), format!("{ENV_MODEL}=default")],
            // the server's require-client, shown; the key made for the gateway alone is its own client under it
            "requireClient": server.offer["gateway"]["requireClient"].as_bool().unwrap_or(false),
        },
        "gitCredential": {
            "prefix": crate::vcs::url_prefix(&server.base).unwrap_or_default(),
            "mode": if use_for_git { "stored" } else { "helper" },
        },
    })
}

/// `apply#ServerSetup`: what the user confirmed of the plan: the `myaddons.xml` repository (and as the default one),
/// the task source (made the user's current one), the gateway's `.env` lines, and git's credential for the server's
/// host (git's own helpers, or the stored token). The instance's files (`myaddons.xml`, `.env`) are the full tier's,
/// and not on a shared server. What it set is kept on the connection for Remove.
fn apply_server_setup(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    session_only()?;
    let _authz = ec.disable_authz();
    let mut row = own_row(&context, &text(parameters, "serverId"))?;
    let server = Server::from_row(&context.facade, &row);
    let mut setup: Json = serde_json::from_str(&field(&row, "setupJson")).unwrap_or_else(|_| json!({}));
    let mut done: Vec<String> = Vec::new();
    let instance_files = || -> Result<(), LibError> {
        if context.tier != Tier::Full || context.settings.shared {
            return Err(LibError::new(
                "myaddons.xml and .env are the instance's: the full tier sets them, and not on a shared server",
            ));
        }
        Ok(())
    };
    if flag(parameters, "addons") {
        instance_files()?;
        let template = server.offered("git.httpsUrlTemplate");
        if template.is_empty() {
            return Err(LibError::new(format!("{} offers no git URL", server.base)));
        }
        // the server's own git, https, with no user part: what moq clones from
        on_server(&server, &template.replace("${name}", "x"))?;
        let name = Some(text(parameters, "repositoryName"))
            .filter(|name| !name.trim().is_empty())
            .map_or_else(
                || repository_name(&server.label, &server.base),
                |name| repository_name(&name, ""),
            );
        let file = myaddons_path(context.factory.runtime_dir());
        let before = std::fs::read_to_string(&file).ok();
        let ours = setup["addons"]["repositoryName"].as_str() == Some(name.as_str());
        let existing = before.as_deref().and_then(|text| repository_url(text, &name));
        let template_attr = crate::workspace::xml_attr(&template);
        // a repository of that name the developer wrote is theirs: not replaced, and not Connect's to remove
        if existing.as_ref().is_some_and(|url| *url != template_attr) && !ours {
            return Err(LibError::new(format!(
                "{} already has a repository {name} of its own: give the server's another name",
                file.display()
            )));
        }
        let default_before = before.as_deref().and_then(default_repository);
        let after = with_repository(
            before.as_deref(),
            &name,
            &template,
            flag(parameters, "defaultRepository"),
        )
        .map_err(LibError::new)?;
        crate::workspace::write_atomic(&file, after.as_bytes())?;
        let set_default = default_before.is_none() && default_repository(&after).as_deref() == Some(name.as_str());
        // what was there before Connect stays the developer's (an entry of the same URL Connect only kept)
        let written = existing.is_none() || ours;
        setup["addons"] = json!({
            "file": file.display().to_string(), "repositoryName": name, "url": template, "written": written,
            "setDefault": set_default || (ours && setup["addons"]["setDefault"].as_bool() == Some(true)),
        });
        done.push(format!("the {name} repository in {}", file.display()));
    }
    if flag(parameters, "taskSource") {
        if server.token.is_none() {
            return Err(LibError::new(
                "connect first: the task source reads your tasks with your token",
            ));
        }
        let user = current_user();
        let existing = context
            .facade
            .find("moqui.dev.DevAideTaskSource")?
            .eq("serverId", Value::string(&server.id))
            .eq("userId", Value::string(&user))
            .one()?;
        let mut source = match existing {
            Some(source) => source,
            None => {
                let mut source = context.facade.make_value("moqui.dev.DevAideTaskSource")?;
                source.set_sequenced_id_primary(&context.facade)?;
                source.set("userId", Value::string(&user));
                source.set("kind", Value::string("moqui-rest"));
                source.set("serverId", Value::string(&server.id));
                source
            }
        };
        source.set("label", Value::string(&server.label));
        source.set("baseUrl", Value::string(&server.base));
        source.set("username", Value::string(&server.username));
        source.set("tasksPath", Value::string(tasks_path(&server)));
        source.create_or_update(&context.facade)?;
        let id = field(&source, "taskSourceId");
        moqui_core::user::set_preference(crate::tasks::TASK_SOURCE, Some(id.as_str()))?;
        setup["taskSource"] = json!({ "taskSourceId": id });
        done.push("the task source".into());
    }
    if flag(parameters, "gateway") {
        instance_files()?;
        let url = server.offered("gateway.url");
        if server.token.is_none() {
            return Err(LibError::new(
                "connect first: the gateway takes a key the server gives you",
            ));
        }
        if url.is_empty() {
            return Err(LibError::new(format!("{} offers no LLM gateway", server.base)));
        }
        // the gateway is the server's own, https (or this machine by its IP literal)
        let url = on_server(&server, &url)?;
        let alias = Some(text(parameters, "gatewayAlias"))
            .filter(|alias| !alias.trim().is_empty())
            .unwrap_or_else(|| "default".into());
        // a key for the gateway alone: the account's key never goes where a model client reads it. The app sign-in
        // made it; a pasted token makes it now
        let key = match Some(field(&row, "gatewayKey")).filter(|key| !key.is_empty()) {
            Some(key) => key,
            None => {
                let device = format!("Dev AIDE gateway ({})", own_origin().unwrap_or_default());
                let token = server.token.clone().unwrap_or_default();
                make_scoped_key(&server, &token, "gateway", &device)
                    .ok_or_else(|| LibError::new(format!("{} gave no key for its gateway", server.base)))?
            }
        };
        let env_file = context.factory.runtime_dir().join(".env");
        let names = [ENV_URL, ENV_KEY, ENV_MODEL];
        // the developer's own lines, from the file itself, kept to put back at Remove; a connection that wrote them
        // before keeps what was there before it
        if field(&row, "envPriorJson").is_empty() || setup["gateway"].is_null() {
            let lines = moqui_lib::env::parse_dotenv(&std::fs::read_to_string(&env_file).unwrap_or_default());
            let prior: serde_json::Map<String, Json> = lines
                .into_iter()
                .filter(|(name, _)| names.contains(&name.as_str()))
                .map(|(name, value)| (name, Json::String(value)))
                .collect();
            row.set("envPriorJson", Value::string(Json::Object(prior).to_string()));
        }
        let previous_key = field(&row, "gatewayKey");
        let set = vec![
            (ENV_URL.to_string(), url.clone()),
            (ENV_KEY.to_string(), key.clone()),
            (ENV_MODEL.to_string(), alias.trim().to_string()),
        ];
        crate::workspace::unfenced(moqui_core::entity::fence::Effect::Resource, ".env")?;
        moqui_lib::env::update_dotenv_file(&env_file, &set, &[])?;
        row.set("gatewayKey", Value::string(&key));
        if !previous_key.is_empty() && previous_key != key {
            revoke_on_server(&server, &previous_key, "the gateway's earlier key");
        }
        setup["gateway"] = json!({ "url": url, "alias": alias.trim(), "names": names });
        done.push("the gateway's .env lines, with a key for the gateway alone (a restart applies them)".into());
    }
    let git = text(parameters, "gitCredential");
    if git == "stored" || git == "helper" {
        let user = current_user();
        let mut rows = context
            .facade
            .find(CREDENTIAL)?
            .eq("serverId", Value::string(&server.id))
            .eq("userId", Value::string(&user))
            .list()?
            .into_values();
        if rows.is_empty() && git == "stored" {
            return Err(LibError::new("connect first: git would get your token"));
        }
        for credential in &mut rows {
            credential.set("useForGit", Value::string(if git == "stored" { "Y" } else { "N" }));
            credential.update(&context.facade)?;
        }
        setup["gitCredential"] = json!(git);
        done.push(if git == "stored" {
            "git's credential for the server's host: your token".to_string()
        } else {
            "git's credential for the server's host: git's own helpers".to_string()
        });
    }
    row.set("setupJson", Value::string(setup.to_string()));
    row.update(&context.facade)?;
    crate::audit::record(
        &context.facade,
        &crate::audit::Write {
            action: "DAA_GIT",
            root: "",
            path: "",
            detail: Some(json!({ "command": "connect", "server": server.base, "set": done })),
            ..crate::audit::Write::default()
        },
    );
    let mut out = Parameters::new();
    out.insert("done".into(), json_value(&done));
    Ok(out)
}

/// End `key` on the server (`hosting/keyRevoke`), signed in by the key itself, which a key made for one service alone
/// may ask for itself alone: whether it ended. A failure is logged; Remove goes on.
fn revoke_on_server(server: &Server, key: &str, what: &str) -> bool {
    let path = Some(server.offered("signIn.keyRevoke"))
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| "/rest/s1/moqui/hosting/keyRevoke".into());
    match server.with_token(key).post(&path, &json!({ "loginKey": key })) {
        Ok(answer) => answer["revoked"].as_bool() == Some(true),
        Err(err) => {
            tracing::warn!("Dev AIDE: {what} was not ended on {}: {err}", server.base);
            false
        }
    }
}

/// What Remove does to the `.env` lines: each still as Connect wrote it (`wrote`) goes back to the value the file had
/// before (`prior`), or goes when it had none; a line the developer changed since stays.
#[must_use]
pub fn env_restore(
    current: &[(String, String)],
    wrote: &[(&str, String)],
    prior: &Json,
) -> (Vec<(String, String)>, Vec<&'static str>) {
    let mut set = Vec::new();
    let mut remove = Vec::new();
    for (name, value) in wrote {
        let now = current
            .iter()
            .rev()
            .find(|(have, _)| have == name)
            .map(|(_, value)| value);
        if value.is_empty() || now != Some(value) {
            continue;
        }
        let name = match *name {
            ENV_URL => ENV_URL,
            ENV_KEY => ENV_KEY,
            _ => ENV_MODEL,
        };
        match prior[name].as_str() {
            Some(before) => set.push((name.to_string(), before.to_string())),
            None => remove.push(name),
        }
    }
    (set, remove)
}

/// `remove#Server`: the connection, its token and what Connect set (the task source, the `myaddons.xml` repository it
/// wrote, the `.env` lines still as it wrote them, put back as they were, the roots' hosted repositories it resolved),
/// and the keys it was given ended on the server, each by itself: the gateway's, and an app sign-in's (git, tasks and
/// the installed app's).
fn remove_server(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    session_only()?;
    let _authz = ec.disable_authz();
    let row = own_row(&context, &text(parameters, "serverId"))?;
    let id = field(&row, "serverId");
    let server = Server::from_row(&context.facade, &row);
    let setup: Json = serde_json::from_str(&field(&row, "setupJson")).unwrap_or(Json::Null);
    let mut removed: Vec<String> = Vec::new();
    // each part goes on its own: one that fails is logged, and the rest still go
    let mut part = |what: &str, outcome: Result<bool, LibError>| match outcome {
        Ok(true) => removed.push(what.to_string()),
        Ok(false) => {}
        Err(err) => tracing::error!("Dev AIDE: removing {what} of group server {id} failed: {err}"),
    };
    part(
        "the task source",
        (|| {
            let mut any = false;
            for source in context
                .facade
                .find("moqui.dev.DevAideTaskSource")?
                .eq("serverId", Value::string(&id))
                .eq("userId", Value::string(current_user()))
                .list()?
                .iter()
            {
                if moqui_core::user::get_preference(crate::tasks::TASK_SOURCE)
                    .ok()
                    .flatten()
                    .as_deref()
                    == Some(field(source, "taskSourceId").as_str())
                {
                    moqui_core::user::set_preference(crate::tasks::TASK_SOURCE, None)?;
                }
                source.delete(&context.facade)?;
                any = true;
            }
            Ok(any)
        })(),
    );
    part(
        "the myaddons.xml repository",
        (|| {
            let (Some(file), Some(name)) = (
                setup["addons"]["file"].as_str(),
                setup["addons"]["repositoryName"].as_str(),
            ) else {
                return Ok(false);
            };
            // only an entry Connect wrote, still with the URL it wrote: one the developer had or changed stays
            if context.tier != Tier::Full || setup["addons"]["written"].as_bool() == Some(false) {
                return Ok(false);
            }
            let Ok(before) = std::fs::read_to_string(file) else {
                return Ok(false);
            };
            let wrote = setup["addons"]["url"].as_str().map(crate::workspace::xml_attr);
            if wrote.is_some() && repository_url(&before, name) != wrote {
                return Ok(false);
            }
            let set_default = setup["addons"]["setDefault"].as_bool() != Some(false);
            let (after, had) = without_repository(&before, name, set_default);
            if had {
                crate::workspace::write_atomic(Path::new(file), after.as_bytes())?;
            }
            Ok(had)
        })(),
    );
    part(
        "the gateway's .env lines",
        (|| {
            if setup["gateway"].is_null() || context.tier != Tier::Full {
                return Ok(false);
            }
            let env_file = context.factory.runtime_dir().join(".env");
            let current = moqui_lib::env::parse_dotenv(&std::fs::read_to_string(&env_file).unwrap_or_default());
            // the key Connect wrote: the gateway's own, or (a connection made before it had one) the account's token
            let key = Some(field(&row, "gatewayKey"))
                .filter(|key| !key.is_empty())
                .or_else(|| server.token.clone())
                .unwrap_or_default();
            let wrote = [
                (ENV_URL, setup["gateway"]["url"].as_str().unwrap_or("").to_string()),
                (ENV_KEY, key),
                (ENV_MODEL, setup["gateway"]["alias"].as_str().unwrap_or("").to_string()),
            ];
            let prior: Json = serde_json::from_str(&field(&row, "envPriorJson")).unwrap_or(Json::Null);
            let (set, remove) = env_restore(&current, &wrote, &prior);
            if set.is_empty() && remove.is_empty() {
                return Ok(false);
            }
            crate::workspace::unfenced(moqui_core::entity::fence::Effect::Resource, ".env")?;
            moqui_lib::env::update_dotenv_file(&env_file, &set, &remove)?;
            Ok(true)
        })(),
    );
    part("the gateway's key on the server", {
        let key = field(&row, "gatewayKey");
        Ok(!key.is_empty() && revoke_on_server(&server, &key, "the gateway's key"))
    });
    part("the tasks key on the server", {
        let key = field(&row, "tasksKey");
        Ok(!key.is_empty() && revoke_on_server(&server, &key, "the tasks key"))
    });
    part("the installed app's key on the server", {
        let key = field(&row, "appKey");
        Ok(!key.is_empty() && revoke_on_server(&server, &key, "the installed app's key"))
    });
    part("the sign-in's key on the server", {
        // a key the app sign-in made is this connection's alone; a token the user pasted may serve elsewhere
        let token = server.token.clone().unwrap_or_default();
        Ok(setup["token"] == "app" && !token.is_empty() && revoke_on_server(&server, &token, "the sign-in's key"))
    });
    part(
        "the roots' hosted repositories",
        (|| {
            let mut any = false;
            for mut root in context
                .facade
                .find(crate::roots::ROOT_ENTITY)?
                .eq("hostedServerId", Value::string(&id))
                .list()?
                .into_values()
            {
                for name in [
                    "hostedServerId",
                    "hostedRemote",
                    "hostedRepositoryId",
                    "hostedJson",
                    "hostedDate",
                ] {
                    root.set(name, Value::Null);
                }
                root.update(&context.facade)?;
                any = true;
            }
            Ok(any)
        })(),
    );
    part(
        "the token",
        (|| {
            let mut any = false;
            for credential in context
                .facade
                .find(CREDENTIAL)?
                .eq("serverId", Value::string(&id))
                .eq("userId", Value::string(current_user()))
                .list()?
                .iter()
            {
                credential.delete(&context.facade)?;
                any = true;
            }
            Ok(any)
        })(),
    );
    row.delete(&context.facade)?;
    let mut out = Parameters::new();
    out.insert("removed".into(), json_value(&removed));
    Ok(out)
}
