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

//! SSO for an installed app (plans/Mobile-Desktop.md §4 "Remote applications and authentication", RFC 8252): the
//! desktop or mobile client opens the system browser at `/sso/appLogin` with a PKCE challenge, its `state`, its
//! registered redirect URI and its name; the browser signs in at the identity provider as `/sso/login` does, and the
//! callback marks this app sign-in done by that user. `/sso/appCode` then asks the user in the browser to let the app
//! in, and on that confirmation (a POST with the session's token) hands the app a single-use code by its redirect URI.
//! The app posts the code with its PKCE verifier and its redirect URI to `/sso/appToken` and gets a device key, as a
//! password sign-in with `createLoginKey` and `deviceName` does; no session is logged in or out for it.
//!
//! A code is good once, for a minute, only with the verifier of its challenge. The pending sign-in is the browser
//! session's, good for 15 minutes, and only after its own provider round trip: a browser that was signed in already
//! still goes through the provider and the confirmation. A failure after the redirect URI is checked goes back to the
//! app with `error=` and its state. MR Moqui stays a relying party of the provider: this is a handoff to its own
//! client, not an authorization server.
//!
//! The redirect URIs an app may give are the property `webapp-list/webapp[webroot]@app-redirect-uris` (spaces or commas between them),
//! by default the desktop app's `org.moqui.desktop:/sso` and the mobile app's `org.moqui.mobile:/sso`, and any
//! loopback one (RFC 8252 §7.3 and §8.3: `http://127.0.0.1:<port>/...` or `http://[::1]:<port>/...`, any port; not
//! `localhost`, which a hosts file or a resolver may send elsewhere), unless `webapp-list/webapp[webroot]@app-loopback-redirect` is false (on
//! by default: a dev instance signs in to its group server so, with its own address as the redirect,
//! plans/design-dev-aide.md 21.2, F17). Any program on the user's computer can listen on a loopback port, so the
//! confirmation names the port the code goes to and says the app's name is the app's own word.
//!
//! With no `authFlowId` the browser signs in at this server's own login instead of an identity provider (a server
//! with none). A session signed in already stays signed in: the confirmation then asks for the account's password (and
//! its second factor) again (`reauthenticate`), as it needs a sign-in after the start (the session's last proven
//! sign-in, `last_auth_millis`) as the provider round trip is needed otherwise.
//!
//! The pages of the browser's side are never shown in a frame (`frame-ancestors 'none'`, `X-Frame-Options: DENY`), so
//! no other site can put the confirmation under a click of its own.

use std::collections::BTreeMap;

use base64::Engine;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{to_plain_string, Value};
use sha2::Digest;

use super::util::param_text;
use crate::entity::EntityFacade;
use crate::transaction::TransactionAttribute;
use crate::user::session::{session_attribute, set_session_attribute};

const APP_CODE: &str = "moqui.security.UserAppCode";
/// The session's app sign-in under way: `{challenge, state, redirectUri, device, startedAt}`, and once the provider's
/// callback signed the browser in, `completedBy` and `completedAt`.
pub(super) const PENDING_APP: &str = "moquiAppAuthPending";
/// Where the provider's callback goes for an app sign-in.
pub(super) const APP_CODE_PATH: &str = "/sso/appCode";
/// The redirect URIs an app may give when the property names none.
const DEFAULT_REDIRECTS: &str = "org.moqui.desktop:/sso org.moqui.mobile:/sso";
/// How long a code is good.
const CODE_MILLIS: i64 = 60_000;
/// How long an app sign-in may take in the browser.
const PENDING_MILLIS: i64 = 15 * 60_000;

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, get_app_providers)?;
    rust_service!(map, start_app_login)?;
    rust_service!(map, confirm_app_login)?;
    rust_service!(map, issue_app_code)?;
    rust_service!(map, exchange_app_code)?;
    Ok(())
}

/// `get#AppProviders`: the identity providers an app can offer (the AuthFlows the login page shows): `authFlowList` of
/// `{authFlowId, description, iconName}`.
fn get_app_providers(
    _parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let flows = facade
        .find("moqui.security.sso.AuthFlow")?
        .disable_authz()
        .order_by("sequenceNum", false)
        .list()?;
    let list: Vec<Value> = flows
        .iter()
        .filter(|flow| {
            let flag = |name: &str| flow.get(name).map(to_plain_string).as_deref() == Some("Y");
            !flag("inbound") && !flag("disabled")
        })
        .map(|flow| {
            let mut map = indexmap::IndexMap::default();
            for field in ["authFlowId", "description", "iconName"] {
                map.insert(field.to_string(), flow.get(field).cloned().unwrap_or(Value::Null));
            }
            Value::Map(map)
        })
        .collect();
    Ok(BTreeMap::from([("authFlowList".to_string(), Value::List(list))]))
}

/// `start#AppLogin`: check the app's challenge, state and redirect URI, keep them in the session, and start the
/// provider's login as `login#User` does, coming back to `/sso/appCode`. A refusal is an error page, not a redirect to
/// a URI that was not checked.
fn start_app_login(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let challenge = param_text(parameters, "codeChallenge");
    let state = param_text(parameters, "state");
    let redirect = param_text(parameters, "redirectUri");
    let method = param_text(parameters, "codeChallengeMethod");
    // what was left of an earlier one is not this one
    set_session_attribute(PENDING_APP, None);
    if let Err(reason) = check_start(&challenge, &method, &state, &redirect, &Redirects::configured()) {
        tracing::warn!("App SSO sign-in refused: {reason}");
        crate::web::send_json_error(400, &reason);
        return Ok(BTreeMap::new());
    }
    let device: String = param_text(parameters, "deviceName").chars().take(255).collect();
    let flow = param_text(parameters, "authFlowId");
    let local = flow.is_empty();
    let pending = serde_json::json!({ "challenge": challenge, "state": state, "redirectUri": redirect, "device": device,
        "startedAt": crate::user::now_millis(), "local": local });
    set_session_attribute(PENDING_APP, Some(&pending.to_string()));
    if local {
        // the server's own login, after this start; a browser signed in already stays so, and confirms who it is on the
        // confirmation instead (a sign-out here would end what the user has open in other tabs)
        if crate::user::current_user_id()?.is_some_and(|id| !id.is_empty()) {
            crate::web::send_redirect(APP_CODE_PATH);
        } else {
            crate::web::send_redirect(&format!("/Login?returnTo={}", percent(APP_CODE_PATH)));
        }
        return Ok(BTreeMap::new());
    }
    let mut login = BTreeMap::new();
    login.insert("authFlowId".to_string(), Value::string(flow));
    login.insert("returnTo".to_string(), Value::string(APP_CODE_PATH));
    super::sso::login_user_for_app(&login, facade)
}

/// The callback signed the browser in as `user_id` (`sso.rs`): when this login was an app's, it is done by that user.
pub(super) fn callback_done(user_id: &str) {
    let Some(mut pending) = pending() else { return };
    pending["completedBy"] = serde_json::Value::String(user_id.to_string());
    pending["completedAt"] = serde_json::json!(crate::user::now_millis());
    set_session_attribute(PENDING_APP, Some(&pending.to_string()));
}

/// A login or callback of an app sign-in failed after its redirect URI was checked: the browser goes back to the app
/// with `error` and its state, and true; false when no app sign-in is under way.
pub(super) fn failed_to_app(error: &str) -> bool {
    let Some(pending) = pending() else { return false };
    set_session_attribute(PENDING_APP, None);
    let redirect = pending["redirectUri"].as_str().unwrap_or("");
    if !Redirects::configured().allows(redirect) {
        return false;
    }
    send_app_redirect(
        redirect,
        &[("error", error), ("state", pending["state"].as_str().unwrap_or(""))],
    );
    true
}

/// The session's app sign-in, if one is under way and not stale.
fn pending() -> Option<serde_json::Value> {
    let stored = session_attribute(PENDING_APP)?;
    let pending: serde_json::Value = serde_json::from_str(&stored).ok()?;
    let started = pending["startedAt"].as_i64()?;
    if crate::user::now_millis() - started > PENDING_MILLIS {
        set_session_attribute(PENDING_APP, None);
        return None;
    }
    Some(pending)
}

/// Where an app sign-in in this browser stands for the confirmation.
enum Ready {
    /// Its callback was done by the user signed in now (or, at this server's own login, the user signed in after the
    /// start): the user may let the app in.
    Confirm(serde_json::Value),
    /// At this server's own login, a browser signed in before the start: the user confirms who they are first.
    Reauthenticate(serde_json::Value),
}

/// The app sign-in the current user did in this browser, with its redirect still one an app may give.
fn ready_pending() -> Option<Ready> {
    let mut pending = pending()?;
    let user_id = crate::user::current_user_id()
        .ok()
        .flatten()
        .filter(|id| !id.is_empty())?;
    if !Redirects::configured().allows(pending["redirectUri"].as_str().unwrap_or("")) {
        return None;
    }
    let local = pending["local"].as_bool() == Some(true);
    let signed_in_since = local
        && pending["startedAt"]
            .as_i64()
            .zip(crate::user::last_auth_millis())
            .is_some_and(|(started, signed_in)| signed_in >= started);
    if signed_in_since {
        pending["completedBy"] = serde_json::Value::String(user_id.clone());
    }
    if pending["completedBy"].as_str() == Some(user_id.as_str()) {
        Some(Ready::Confirm(pending))
    } else if local {
        Some(Ready::Reauthenticate(pending))
    } else {
        None
    }
}

/// The port of a loopback redirect, what the confirmation names: the app on this computer that gets the code.
fn redirect_port(redirect: &str) -> Option<String> {
    let rest = redirect
        .strip_prefix("http://")
        .or_else(|| redirect.strip_prefix("https://"))?;
    let authority = rest.split(['/', '?']).next()?;
    let (_, port) = authority
        .rsplit_once(':')
        .filter(|(host, _)| host.ends_with(']') || !host.contains(':'))?;
    Some(port.to_string())
}

/// What the confirmation says of the app: its name as the app gave it, and where the code goes.
fn app_text(pending: &serde_json::Value) -> String {
    let device = pending["device"].as_str().filter(|name| !name.is_empty());
    let redirect = pending["redirectUri"].as_str().unwrap_or("");
    let named = device.map_or_else(
        || "an app".to_string(),
        |name| format!("an app that calls itself \u{201c}{}\u{201d}", escape(name)),
    );
    if is_loopback_redirect(redirect) {
        let port =
            redirect_port(redirect).unwrap_or_else(|| if redirect.starts_with("https") { "443" } else { "80" }.into());
        format!("{named}, on this computer at port {}", escape(&port))
    } else {
        format!("{named} ({})", escape(redirect))
    }
}

/// The form that confirms the signed-in user again, with the session's token: the password, and the code when the
/// account has a second factor (`code_wanted`).
fn reauth_form(token: &str, pending_id: &str, code_wanted: bool) -> String {
    let code = if code_wanted {
        "<p><label>Code <input name=\"code\" autocomplete=\"one-time-code\" inputmode=\"numeric\"></label></p>"
    } else {
        ""
    };
    // an account with no password here (it signs in through an identity provider) finishes by signing in again
    format!(
        "<form method=\"post\" action=\"/sso/appCodeConfirm\">{}\
         <p><label>Password <input type=\"password\" name=\"password\" autocomplete=\"current-password\"></label></p>{code}\
         <button type=\"submit\" name=\"allow\" value=\"true\">Confirm and sign in</button> \
         <button type=\"submit\" name=\"allow\" value=\"false\">Cancel</button></form>\
         <form method=\"post\" action=\"/sso/appCodeConfirm\">{}<p>No password here (you sign in with another \
         provider)? <button type=\"submit\" name=\"signOut\" value=\"true\">Sign out and sign in again</button></p></form>",
        hidden_fields(token, pending_id),
        hidden_fields(token, pending_id)
    )
}

/// The session token and the pending sign-in a form was shown for.
fn hidden_fields(token: &str, pending_id: &str) -> String {
    format!(
        "<input type=\"hidden\" name=\"moquiSessionToken\" value=\"{}\"><input type=\"hidden\" name=\"pendingId\" value=\"{}\">",
        escape(token),
        escape(pending_id)
    )
}

/// The id of a pending sign-in that its page's form carries back: a confirmation answers the sign-in it showed, not
/// one another start put in its place since.
fn pending_id(pending: &serde_json::Value) -> String {
    let text = format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        pending["challenge"].as_str().unwrap_or(""),
        pending["state"].as_str().unwrap_or(""),
        pending["redirectUri"].as_str().unwrap_or(""),
        pending["startedAt"].as_i64().unwrap_or(0)
    );
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(text.as_bytes()))
}

/// The page that asks the signed-in user to confirm who they are before an app is let in.
fn send_reauth_page(pending: &serde_json::Value, note: Option<&str>, code_wanted: bool) -> Result<(), LibError> {
    let username = crate::user::current_username()?.unwrap_or_default();
    let token = crate::user::csrf_token()?;
    let text = format!(
        "{}Sign in to {} as {}? You are signed in already: enter your password to confirm it is you. The app gets its own \
         key, which you can revoke from your account.",
        note.map(|note| format!("{} ", escape(note))).unwrap_or_default(),
        app_text(pending),
        escape(&username)
    );
    send_page(
        200,
        "Confirm it is you",
        &text,
        Some(&reauth_form(&token, &pending_id(pending), code_wanted)),
    );
    Ok(())
}

/// `confirm#AppLogin`: after the provider signed the browser in for an app, a page that asks the user to let that app
/// in (its name, the user), with a button that posts to `/sso/appCodeConfirm` with the session's token, and one that
/// does not. A browser with no such sign-in is told so.
fn confirm_app_login(
    _parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let pending = match ready_pending() {
        Some(Ready::Confirm(pending)) => pending,
        Some(Ready::Reauthenticate(pending)) => {
            send_reauth_page(&pending, None, false)?;
            return Ok(BTreeMap::new());
        }
        None => {
            send_page(
                400,
                "Nothing to sign in",
                "No app sign-in is waiting in this browser. Start it again from the app.",
                None,
            );
            return Ok(BTreeMap::new());
        }
    };
    let username = crate::user::current_username()?.unwrap_or_default();
    let token = crate::user::csrf_token()?;
    let text = format!(
        "Sign in to {} as {}? The app gets its own key, which you can revoke from your account.",
        app_text(&pending),
        escape(&username)
    );
    let form = format!(
        "<form method=\"post\" action=\"/sso/appCodeConfirm\">{}\
         <button type=\"submit\" name=\"allow\" value=\"true\">Sign in</button> \
         <button type=\"submit\" name=\"allow\" value=\"false\">Cancel</button></form>",
        hidden_fields(&token, &pending_id(&pending))
    );
    send_page(200, "Sign in to the app", &text, Some(&form));
    Ok(BTreeMap::new())
}

/// `issue#AppCode`: on the user's confirmation, a single-use code for the app, bound to the user, the challenge and
/// the redirect URI, and the browser sent to the app with it and the app's state (`error=access_denied` on Cancel).
fn issue_app_code(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let allow = param_text(parameters, "allow") == "true";
    let ready = ready_pending();
    // the form answers the sign-in it was shown for: another start since (another app, another tab) is not confirmed
    // by it
    let shown = param_text(parameters, "pendingId");
    let current = match &ready {
        Some(Ready::Confirm(pending) | Ready::Reauthenticate(pending)) => pending_id(pending),
        None => String::new(),
    };
    if ready.is_some() && shown != current {
        send_page(
            400,
            "The sign-in changed",
            "Another app sign-in was started in this browser since this page was shown. Start it again from the app.",
            None,
        );
        return Ok(BTreeMap::new());
    }
    // signed in already with no password here: signed out, the sign-in kept, and back through this server's login
    if param_text(parameters, "signOut") == "true" {
        if let Some(Ready::Reauthenticate(pending)) = ready {
            crate::user::logout()?;
            set_session_attribute(PENDING_APP, Some(&pending.to_string()));
            crate::web::send_redirect(&format!("/Login?returnTo={}", percent(APP_CODE_PATH)));
            return Ok(BTreeMap::new());
        }
    }
    let pending = match ready {
        Some(Ready::Confirm(pending)) => pending,
        Some(Ready::Reauthenticate(pending)) if allow => {
            // the signed-in user confirms who they are: the password, then the code of a second factor
            match crate::user::reauthenticate(
                &param_text(parameters, "password"),
                param_text(parameters, "code").trim(),
            ) {
                Ok(crate::user::CodeLogin::LoggedIn) => {
                    let mut pending = pending;
                    pending["completedBy"] =
                        serde_json::Value::String(crate::user::current_user_id()?.unwrap_or_default());
                    pending
                }
                Ok(crate::user::CodeLogin::CodeNeeded { text, .. }) => {
                    send_reauth_page(&pending, Some(&text), true)?;
                    return Ok(BTreeMap::new());
                }
                Err(err) => {
                    send_reauth_page(&pending, Some(err.message()), false)?;
                    return Ok(BTreeMap::new());
                }
            }
        }
        Some(Ready::Reauthenticate(pending)) => pending,
        None => {
            send_page(
                400,
                "Nothing to sign in",
                "No app sign-in is waiting in this browser. Start it again from the app.",
                None,
            );
            return Ok(BTreeMap::new());
        }
    };
    // one code per sign-in
    set_session_attribute(PENDING_APP, None);
    let text = |name: &str| pending[name].as_str().unwrap_or("").to_string();
    let (challenge, state, redirect) = (text("challenge"), text("state"), text("redirectUri"));
    if !allow {
        send_app_redirect(&redirect, &[("error", "access_denied"), ("state", &state)]);
        return Ok(BTreeMap::new());
    }
    let user_id = text("completedBy");
    if user_id.is_empty() {
        return Err(LibError::new("the app sign-in has no user"));
    }
    let code = crate::user::random_string(43)?;
    let now = crate::user::now_millis();
    let _authz = crate::context::ExecutionContext::with_current(crate::context::ExecutionContext::disable_authz);
    // codes that are used or expired are of no more use
    let old = facade
        .find(APP_CODE)?
        .condition(moqui_db::sql::Condition::less(
            "expireDate",
            crate::user::stamp_value(now),
        ))
        .list()?;
    for row in old.iter() {
        if let Err(err) = row.delete(facade) {
            tracing::warn!("An expired app sign-in code was not deleted: {err}");
        }
    }
    let mut row = facade.make_value(APP_CODE)?;
    row.set("codeHash", Value::string(crate::user::single_use_hash(&code)?));
    row.set("userId", Value::string(&user_id));
    row.set("codeChallenge", Value::string(&challenge));
    row.set("redirectUri", Value::string(&redirect));
    row.set("expireDate", crate::user::stamp_value(now + CODE_MILLIS));
    row.create(facade)?;
    send_app_redirect(&redirect, &[("code", &code), ("state", &state)]);
    Ok(BTreeMap::new())
}

/// `exchange#AppCode`: the app's code, PKCE verifier and redirect URI for a device key named `deviceName`. Out:
/// `loginKey`. A code that is unknown, used, expired, of another redirect or not of this verifier is refused (400),
/// and a code is used, in a transaction of its own, before anything else is checked, so it cannot be guessed at or
/// taken twice.
fn exchange_app_code(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let code = param_text(parameters, "code");
    let verifier = param_text(parameters, "codeVerifier");
    let redirect = param_text(parameters, "redirectUri");
    let device = param_text(parameters, "deviceName");
    let refused = |why: &str| {
        tracing::info!("An app sign-in code was refused: {why}");
        crate::web::send_json_error(400, "The sign-in code is not valid");
        Ok(BTreeMap::new())
    };
    if code.is_empty() || !verifier_ok(&verifier) {
        return refused("no code, or not a PKCE verifier");
    }
    let hashed = crate::user::single_use_hash(&code)?;
    let _authz = crate::context::ExecutionContext::with_current(crate::context::ExecutionContext::disable_authz);
    let Some(row) = facade.find(APP_CODE)?.eq("codeHash", Value::string(&hashed)).one()? else {
        return refused("unknown");
    };
    let now = crate::user::now_millis();
    // used once: the update that marks it is the one that takes it, kept whatever happens after
    let taken = facade
        .transactions_for(APP_CODE)?
        .run(TransactionAttribute::ForceNew, None, || {
            facade
                .find(APP_CODE)?
                .eq("codeHash", Value::string(&hashed))
                .condition(moqui_db::sql::Condition::is_null("usedDate"))
                .update_all([("usedDate".to_string(), crate::user::stamp_value(now))])
        })?;
    let field = |name: &str| row.get(name).map(to_plain_string).unwrap_or_default();
    let expired = row
        .get("expireDate")
        .and_then(crate::user::value_millis)
        .is_none_or(|expire| now > expire);
    if taken == 0 {
        return refused("used");
    }
    if expired {
        return refused("expired");
    }
    if field("redirectUri") != redirect {
        return refused("another redirect URI");
    }
    if challenge_of(&verifier) != field("codeChallenge") {
        return refused("another verifier");
    }
    let device = if device.trim().is_empty() {
        "App"
    } else {
        device.as_str()
    };
    match crate::user::issue_device_key(&field("userId"), device) {
        Ok(key) => Ok(BTreeMap::from([("loginKey".to_string(), Value::string(key))])),
        Err(err) => refused(&format!("no device key: {}", err.message())),
    }
}

/// The redirect URIs an app may give: those listed, exactly, and with `loopback` any loopback one.
#[derive(Debug, Clone)]
pub(crate) struct Redirects {
    pub(crate) listed: Vec<String>,
    pub(crate) loopback: bool,
}

impl Redirects {
    /// `webapp-list/webapp[webroot]@app-redirect-uris` (the desktop and mobile apps' when empty) and `@app-loopback-redirect`.
    fn configured() -> Self {
        let configured = crate::context::conf_env::setting("webapp-list/webapp[webroot]@app-redirect-uris");
        let text = if configured.trim().is_empty() {
            DEFAULT_REDIRECTS.to_string()
        } else {
            configured
        };
        Self {
            listed: text
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|uri| !uri.is_empty())
                .map(str::to_string)
                .collect(),
            loopback: crate::context::conf_env::setting("webapp-list/webapp[webroot]@app-loopback-redirect").trim()
                != "false",
        }
    }

    pub(crate) fn allows(&self, redirect: &str) -> bool {
        self.listed.iter().any(|uri| uri == redirect) || (self.loopback && is_loopback_redirect(redirect))
    }
}

/// Whether `redirect` is a loopback redirect URI (RFC 8252 §7.3): http or https to the IP literal `127.0.0.1` or
/// `[::1]` (§8.3: not `localhost`, which a hosts file or resolver may send elsewhere), on any port, with no user part and
/// no fragment. The code it carries is good only with the app's PKCE verifier.
pub(crate) fn is_loopback_redirect(redirect: &str) -> bool {
    let Some(rest) = redirect
        .strip_prefix("http://")
        .or_else(|| redirect.strip_prefix("https://"))
    else {
        return false;
    };
    if rest.contains('#') || rest.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let authority = rest.split(['/', '?']).next().unwrap_or("");
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.ends_with(':') && !port.contains(']') => (host, Some(port)),
        _ => (authority, None),
    };
    let port_ok =
        port.is_none_or(|port| !port.is_empty() && port.len() <= 5 && port.bytes().all(|b| b.is_ascii_digit()));
    port_ok && matches!(host, "127.0.0.1" | "[::1]")
}

/// Why a start is refused, if it is: an S256 challenge (43 base64url characters), a state of plain characters, and a
/// redirect URI that `allowed` allows.
pub(crate) fn check_start(
    challenge: &str,
    method: &str,
    state: &str,
    redirect: &str,
    allowed: &Redirects,
) -> Result<(), String> {
    if method != "S256" {
        return Err(format!("code_challenge_method {method:?} is not S256"));
    }
    let base64url = |text: &str| text.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if challenge.len() != 43 || !base64url(challenge) {
        return Err("the code challenge is not an S256 challenge".to_string());
    }
    if state.is_empty() || state.len() > 200 || !state.chars().all(|c| c.is_ascii_graphic()) {
        return Err("the state is missing or not plain text".to_string());
    }
    if !allowed.allows(redirect) {
        return Err(format!("{redirect} is not a redirect URI an app may give"));
    }
    Ok(())
}

/// A PKCE verifier (RFC 7636 §4.1): 43 to 128 of its unreserved characters.
fn verifier_ok(verifier: &str) -> bool {
    (43..=128).contains(&verifier.len())
        && verifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~'))
}

/// The S256 challenge of a verifier: base64url of its SHA-256, with no padding.
pub(crate) fn challenge_of(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()))
}

/// Redirect the browser to the app at `redirect` with `pairs` in its query.
fn send_app_redirect(redirect: &str, pairs: &[(&str, &str)]) {
    let query: String = pairs
        .iter()
        .map(|(name, value)| format!("{name}={}", percent(value)))
        .collect::<Vec<_>>()
        .join("&");
    let separator = if redirect.contains('?') { '&' } else { '?' };
    crate::web::send_redirect(&format!("{redirect}{separator}{query}"));
}

/// A small page of the browser's side of an app sign-in: a title, its text, and a form.
fn send_page(status: u16, title: &str, text: &str, form: Option<&str>) {
    let html = format!(
        "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\">\
         <title>{title}</title></head><body style=\"font-family:sans-serif;max-width:32em;margin:12vh auto;padding:0 1em\">\
         <h1 style=\"font-size:1.4em\">{title}</h1><p>{text}</p>{}</body></html>",
        form.unwrap_or("")
    );
    crate::web::send_bytes_with_headers(status, page_headers(), html.into_bytes());
}

/// The headers of the browser's pages of an app sign-in: HTML, never in a frame, never cached.
fn page_headers() -> Vec<(String, String)> {
    [
        ("Content-Type", "text/html; charset=utf-8"),
        ("Content-Security-Policy", "frame-ancestors 'none'"),
        ("X-Frame-Options", "DENY"),
        ("Cache-Control", "no-store"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value.to_string()))
    .collect()
}

/// `text` for HTML.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// `text` percent-encoded for a query value.
fn percent(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (byte as char).to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

#[cfg(test)]
#[path = "sso_app_tests.rs"]
mod tests;
