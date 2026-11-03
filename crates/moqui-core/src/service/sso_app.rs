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
//! The redirect URIs an app may give are the property `webapp_app_redirect_uris` (spaces or commas between them),
//! by default the desktop app's `org.moqui.desktop:/sso`.

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
const DEFAULT_REDIRECTS: &str = "org.moqui.desktop:/sso";
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
            let mut map = indexmap::IndexMap::new();
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
    if let Err(reason) = check_start(&challenge, &method, &state, &redirect, &allowed_redirects()) {
        tracing::warn!("App SSO sign-in refused: {reason}");
        crate::web::send_json_error(400, &reason);
        return Ok(BTreeMap::new());
    }
    let device: String = param_text(parameters, "deviceName").chars().take(255).collect();
    let pending = serde_json::json!({ "challenge": challenge, "state": state, "redirectUri": redirect, "device": device,
        "startedAt": crate::user::now_millis() });
    set_session_attribute(PENDING_APP, Some(&pending.to_string()));
    let mut login = BTreeMap::new();
    login.insert(
        "authFlowId".to_string(),
        Value::string(param_text(parameters, "authFlowId")),
    );
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
    if !allowed_redirects().iter().any(|uri| uri == redirect) {
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

/// The app sign-in the current user did in this browser, ready for the confirmation: its callback done by the user
/// signed in now, its redirect still one an app may give.
fn completed_pending() -> Option<serde_json::Value> {
    let pending = pending()?;
    let user_id = crate::user::current_user_id()
        .ok()
        .flatten()
        .filter(|id| !id.is_empty())?;
    let redirect = pending["redirectUri"].as_str().unwrap_or("");
    let done_by_this_user = pending["completedBy"].as_str() == Some(user_id.as_str());
    (done_by_this_user && allowed_redirects().iter().any(|uri| uri == redirect)).then_some(pending)
}

/// `confirm#AppLogin`: after the provider signed the browser in for an app, a page that asks the user to let that app
/// in (its name, the user), with a button that posts to `/sso/appCodeConfirm` with the session's token, and one that
/// does not. A browser with no such sign-in is told so.
fn confirm_app_login(
    _parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some(pending) = completed_pending() else {
        send_page(
            400,
            "Nothing to sign in",
            "No app sign-in is waiting in this browser. Start it again from the app.",
            None,
        );
        return Ok(BTreeMap::new());
    };
    let username = crate::user::current_username()?.unwrap_or_default();
    let device = pending["device"]
        .as_str()
        .filter(|name| !name.is_empty())
        .unwrap_or("an app");
    let token = crate::user::csrf_token()?;
    let text = format!(
        "Sign in to {} as {}? The app gets its own key, which you can revoke from your account.",
        escape(device),
        escape(&username)
    );
    let form = format!(
        "<form method=\"post\" action=\"/sso/appCodeConfirm\"><input type=\"hidden\" name=\"moquiSessionToken\" value=\"{}\">\
         <button type=\"submit\" name=\"allow\" value=\"true\">Sign in</button> \
         <button type=\"submit\" name=\"allow\" value=\"false\">Cancel</button></form>",
        escape(&token)
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
    let Some(pending) = completed_pending() else {
        send_page(
            400,
            "Nothing to sign in",
            "No app sign-in is waiting in this browser. Start it again from the app.",
            None,
        );
        return Ok(BTreeMap::new());
    };
    // one code per sign-in
    set_session_attribute(PENDING_APP, None);
    let text = |name: &str| pending[name].as_str().unwrap_or("").to_string();
    let (challenge, state, redirect) = (text("challenge"), text("state"), text("redirectUri"));
    if param_text(parameters, "allow") != "true" {
        send_app_redirect(&redirect, &[("error", "access_denied"), ("state", &state)]);
        return Ok(BTreeMap::new());
    }
    let user_id = text("completedBy");
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

/// The redirect URIs an app may give.
fn allowed_redirects() -> Vec<String> {
    let configured = moqui_lib::env::get_prop_or_env("webapp_app_redirect_uris");
    let text = if configured.trim().is_empty() {
        DEFAULT_REDIRECTS.to_string()
    } else {
        configured
    };
    text.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|uri| !uri.is_empty())
        .map(str::to_string)
        .collect()
}

/// Why a start is refused, if it is: an S256 challenge (43 base64url characters), a state of plain characters, and a
/// redirect URI that is one of `allowed` exactly.
pub(crate) fn check_start(
    challenge: &str,
    method: &str,
    state: &str,
    redirect: &str,
    allowed: &[String],
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
    if !allowed.iter().any(|uri| uri == redirect) {
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
    crate::web::send_bytes_with_headers(
        status,
        vec![("Content-Type".to_string(), "text/html; charset=utf-8".to_string())],
        html.into_bytes(),
    );
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
