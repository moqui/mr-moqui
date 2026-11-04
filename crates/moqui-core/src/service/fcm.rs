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

//! Firebase Cloud Messaging (the HTTP v1 API) to the native apps a user registered (plans/Mobile-Desktop.md §5, "Native
//! push"), for `send#WebPush` in [`super::web_push`]:
//!
//! - the server signs in as a Google service account (`fcm_service_account`: its JSON key file's path, or the JSON): an
//!   RS256 JWT for the `firebase.messaging` scope, exchanged at the account's `token_uri` for an OAuth access token,
//!   kept until five minutes before it expires ([`Tokens`]);
//! - each message is a POST to `<fcm_base_url>/v1/projects/<fcm_project_id>/messages:send`: a data message (the app's
//!   own code shows it), high priority for a call's ring, with the push's time to live.
//!
//! An answer of UNREGISTERED or INVALID_ARGUMENT means the token is no good ([`Outcome::Gone`]); 429 and 5xx are tried
//! again a few times with a growing wait. Every request goes through [`crate::outbound::send`], so a spec's HTTP
//! fixtures answer it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use moqui_lib::error::Error;
use moqui_lib::sync::lock;

use crate::outbound::{Method, Request, Response};

pub(crate) const ACCOUNT_PROPERTY: &str = "fcm_service_account";
pub(crate) const PROJECT_PROPERTY: &str = "fcm_project_id";
pub(crate) const BASE_URL_PROPERTY: &str = "fcm_base_url";
const DEFAULT_BASE_URL: &str = "https://fcm.googleapis.com";
const DEFAULT_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
const SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
/// `urn:ietf:params:oauth:grant-type:jwt-bearer`, form-encoded.
const GRANT_TYPE: &str = "urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer";
/// A JWT asks for a token good this long (Google's most).
const JWT_SECONDS: i64 = 3600;
/// A cached token is renewed this long before it expires.
const TOKEN_MARGIN_SECONDS: i64 = 300;
/// Tries of one message: the first and two more after a 429 or 5xx.
const MAX_ATTEMPTS: u32 = 3;
/// The wait before the second try, doubled before the third; a Retry-After of the service is taken up to [`MAX_WAIT`].
const BACKOFF: Duration = Duration::from_millis(500);
const MAX_WAIT: Duration = Duration::from_secs(5);
const TIMEOUT: Duration = Duration::from_secs(10);

/// The service account key's fields the sender needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServiceAccount {
    pub client_email: String,
    pub private_key: String,
    pub token_uri: String,
    pub project_id: String,
}

impl ServiceAccount {
    /// From the JSON key file Google gives for a service account.
    pub(crate) fn parse(json: &str) -> Result<Self, String> {
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|err| format!("the service account is not JSON: {err}"))?;
        let field = |name: &str| value[name].as_str().unwrap_or_default().trim().to_string();
        let account = Self {
            client_email: field("client_email"),
            private_key: field("private_key"),
            token_uri: Some(field("token_uri"))
                .filter(|uri| !uri.is_empty())
                .unwrap_or_else(|| DEFAULT_TOKEN_URI.to_string()),
            project_id: field("project_id"),
        };
        if account.client_email.is_empty() || account.private_key.is_empty() {
            return Err("the service account has no client_email or private_key".to_string());
        }
        Ok(account)
    }
}

/// What the sender is configured with.
#[derive(Debug, Clone)]
pub(crate) struct Config {
    pub account: ServiceAccount,
    pub project_id: String,
    pub base_url: String,
}

impl Config {
    fn send_url(&self) -> String {
        format!("{}/v1/projects/{}/messages:send", self.base_url, self.project_id)
    }
}

/// The sender's configuration from the properties; `None` when `fcm_service_account` is not set. The account is the
/// JSON itself when the value starts with `{`, else the path of its file. `fcm_project_id` defaults to the account's
/// `project_id`.
pub(crate) fn config() -> Result<Option<Config>, String> {
    let given = moqui_lib::env::get_prop_or_env(ACCOUNT_PROPERTY);
    let given = given.trim();
    if given.is_empty() {
        return Ok(None);
    }
    let json = if given.starts_with('{') {
        given.to_string()
    } else {
        std::fs::read_to_string(given).map_err(|err| format!("{ACCOUNT_PROPERTY} file {given} not read: {err}"))?
    };
    let account = ServiceAccount::parse(&json)?;
    let project_id = Some(moqui_lib::env::get_prop_or_env(PROJECT_PROPERTY).trim().to_string())
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| account.project_id.clone());
    if project_id.is_empty() {
        return Err(format!(
            "{PROJECT_PROPERTY} is not set and the service account names no project_id"
        ));
    }
    let base_url = Some(moqui_lib::env::get_prop_or_env(BASE_URL_PROPERTY).trim().to_string())
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    Ok(Some(Config {
        account,
        project_id,
        base_url: base_url.trim_end_matches('/').to_string(),
    }))
}

/// Whether the properties name a service account (not whether it works): for the clients' feature list.
pub(crate) fn configured() -> bool {
    !moqui_lib::env::get_prop_or_env(ACCOUNT_PROPERTY).trim().is_empty()
}

static UNSET_LOGGED: AtomicBool = AtomicBool::new(false);

/// Log once in the process that FCM is not configured, so a server with native devices and no Firebase project says
/// why they get nothing, without a line for every push.
pub(crate) fn note_unset() {
    if !UNSET_LOGGED.swap(true, Ordering::Relaxed) {
        tracing::info!("FCM push not sent: {ACCOUNT_PROPERTY} is not set; native app devices get no push");
    }
}

/// The signed JWT of the OAuth 2 JWT bearer grant (RFC 7523) for the account, at `now` (Unix seconds).
pub(crate) fn jwt(account: &ServiceAccount, now: i64) -> Result<String, String> {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
    let claims = serde_json::json!({
        "iss": account.client_email,
        "scope": SCOPE,
        "aud": account.token_uri,
        "iat": now,
        "exp": now + JWT_SECONDS,
    });
    let claims = URL_SAFE_NO_PAD.encode(claims.to_string());
    let signing_input = format!("{header}.{claims}");
    // the key file's private_key is PKCS#8 PEM with its line breaks as \n, which the JSON parse turned into newlines
    let signer = moqui_lib::signer::SimpleSigner::from_pem_with(
        ACCOUNT_PROPERTY,
        &account.private_key,
        None,
        Some("SHA256withRSA"),
    );
    let signature = signer.sign(&signing_input).map_err(|err| err.message().to_string())?;
    let signature = base64::engine::general_purpose::STANDARD
        .decode(signature)
        .map_err(|err| format!("the signature is not base64: {err}"))?;
    Ok(format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(signature)))
}

/// How a request goes out: [`crate::outbound::send`] on a server, a stand-in in the unit tests.
pub(crate) type Send<'a> = &'a dyn Fn(Request) -> Result<Response, Error>;

struct Cached {
    /// The account and token endpoint the token is for: another account in the properties takes a new token.
    owner: String,
    token: String,
    expires_at: i64,
}

/// The OAuth access token, cached until shortly before it expires.
pub(crate) struct Tokens {
    slot: Mutex<Option<Cached>>,
}

impl Tokens {
    pub(crate) const fn new() -> Self {
        Self { slot: Mutex::new(None) }
    }

    /// A token for `account` at `now` (Unix seconds): the cached one, or a new one from its token endpoint.
    pub(crate) fn get(&self, account: &ServiceAccount, now: i64, send: Send<'_>) -> Result<String, String> {
        let owner = format!("{} {}", account.client_email, account.token_uri);
        let mut slot = lock(&self.slot);
        if let Some(cached) = slot.as_ref() {
            if cached.owner == owner && now < cached.expires_at - TOKEN_MARGIN_SECONDS {
                return Ok(cached.token.clone());
            }
        }
        let assertion = jwt(account, now)?;
        // the JWT is base64url and dots, which a form body carries as they are
        let body = format!("grant_type={GRANT_TYPE}&assertion={assertion}");
        let response = send(Request {
            method: Method::Post,
            url: account.token_uri.clone(),
            headers: vec![(
                "content-type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            )],
            body: Some(body),
            timeout: Some(TIMEOUT),
            deny_internal: false,
        })
        .map_err(|err| format!("the token endpoint did not answer: {}", err.message()))?;
        let answer: serde_json::Value = serde_json::from_str(&response.body).unwrap_or_default();
        let token = answer["access_token"].as_str().unwrap_or_default();
        if !(200..300).contains(&response.status) || token.is_empty() {
            // the error and its description only: never the body of a success
            let reason = [answer["error"].as_str(), answer["error_description"].as_str()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(": ");
            return Err(format!("the token endpoint answered {}: {reason}", response.status));
        }
        let expires_in = answer["expires_in"].as_i64().unwrap_or(JWT_SECONDS);
        *slot = Some(Cached {
            owner,
            token: token.to_string(),
            expires_at: now + expires_in,
        });
        Ok(token.to_string())
    }

    /// Forget the token, after FCM refused it.
    pub(crate) fn clear(&self) {
        *lock(&self.slot) = None;
    }
}

/// The process's token.
pub(crate) static TOKENS: Tokens = Tokens::new();

/// A call's ring, for the app to show the incoming call.
#[derive(Debug, Default, Clone)]
pub(crate) struct Ring {
    pub call_session_id: String,
    pub chat_room_id: String,
    pub from: String,
    pub video: bool,
    /// When the ring ends, Unix milliseconds.
    pub expires_at: i64,
}

/// What a push says: what Web Push carries (title, link, topic, tag; never a message's body), and a ring's call.
#[derive(Debug, Default, Clone)]
pub(crate) struct Notice {
    pub title: String,
    pub link: String,
    pub topic: String,
    pub tag: String,
    pub ring: Option<Ring>,
    pub high: bool,
    pub ttl: u32,
}

/// The `messages:send` body for the device with `token`: a data message (every value a string), so the app's own code
/// gets it and shows it, a ring as an incoming call. `kind` is `ring` or `notification`. A tag collapses an earlier
/// push of the same tag that the device has not yet received.
pub(crate) fn message(token: &str, notice: &Notice) -> serde_json::Value {
    let mut data = serde_json::Map::new();
    let kind = if notice.ring.is_some() { "ring" } else { "notification" };
    data.insert("kind".to_string(), kind.into());
    for (name, value) in [
        ("title", &notice.title),
        ("link", &notice.link),
        ("topic", &notice.topic),
        ("tag", &notice.tag),
    ] {
        data.insert(name.to_string(), value.as_str().into());
    }
    if let Some(ring) = &notice.ring {
        data.insert("callSessionId".to_string(), ring.call_session_id.as_str().into());
        data.insert("chatRoomId".to_string(), ring.chat_room_id.as_str().into());
        data.insert("from".to_string(), ring.from.as_str().into());
        data.insert("video".to_string(), ring.video.to_string().into());
        data.insert("expiresAt".to_string(), ring.expires_at.to_string().into());
    }
    let mut android = serde_json::json!({
        "priority": if notice.high { "HIGH" } else { "NORMAL" },
        "ttl": format!("{}s", notice.ttl),
    });
    if !notice.tag.is_empty() {
        android["collapse_key"] = notice.tag.as_str().into();
    }
    serde_json::json!({"message": {"token": token, "data": data, "android": android}})
}

/// What FCM's answer means for the device.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Delivered,
    /// UNREGISTERED or INVALID_ARGUMENT: the token is no good; never send to it again.
    Gone,
    /// Any other answer, or none, after the tries: count a failure against the device.
    Failed,
    /// Not the device's fault (no token from Google, FCM refused the server's credentials): leave the device as it is.
    Unsent,
}

/// What one answer calls for.
#[derive(Debug, PartialEq, Eq)]
enum Answer {
    Done(Outcome),
    Retry(Option<Duration>),
    /// 401: the access token is not taken (revoked, or the account changed); take a new one and try again.
    Reauthorize,
}

fn error_codes(body: &str) -> Vec<String> {
    let value: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    let error = &value["error"];
    let mut codes: Vec<String> = error["status"].as_str().map(str::to_string).into_iter().collect();
    for detail in error["details"].as_array().into_iter().flatten() {
        if let Some(code) = detail["errorCode"].as_str() {
            codes.push(code.to_string());
        }
    }
    codes
}

fn answer(response: &Response) -> Answer {
    let codes = error_codes(&response.body);
    let has = |code: &str| codes.iter().any(|found| found == code);
    match response.status {
        200..=299 => Answer::Done(Outcome::Delivered),
        404 | 400 if has("UNREGISTERED") || has("INVALID_ARGUMENT") => Answer::Done(Outcome::Gone),
        401 => Answer::Reauthorize,
        // a sender id mismatch or a quota is the server's configuration, not the device's
        403 => Answer::Done(Outcome::Unsent),
        429 | 500..=599 => Answer::Retry(retry_after(response)),
        _ => Answer::Done(Outcome::Failed),
    }
}

fn retry_after(response: &Response) -> Option<Duration> {
    response
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
        .and_then(|(_, value)| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

/// Sends messages with one configuration, token cache and transport.
pub(crate) struct Sender<'a> {
    pub config: &'a Config,
    pub tokens: &'a Tokens,
    pub send: Send<'a>,
    /// The wait before the second try ([`BACKOFF`]; none in the unit tests).
    pub backoff: Duration,
}

impl Sender<'_> {
    /// Send `body` ([`message`]), trying again after a 429 or 5xx and once after a refused access token. A failure is
    /// logged by `device` (the registration's id), never by its token.
    pub(crate) fn deliver(&self, device: &str, body: &serde_json::Value, now: i64) -> Outcome {
        let mut reauthorized = false;
        let mut wait = self.backoff;
        for attempt in 1..=MAX_ATTEMPTS {
            let token = match self.tokens.get(&self.config.account, now, self.send) {
                Ok(token) => token,
                Err(reason) => {
                    tracing::warn!(subscription_id = %device, "FCM push not sent: {reason}");
                    return Outcome::Unsent;
                }
            };
            let sent = (self.send)(Request {
                method: Method::Post,
                url: self.config.send_url(),
                headers: vec![
                    ("authorization".to_string(), format!("Bearer {token}")),
                    (
                        "content-type".to_string(),
                        "application/json; charset=UTF-8".to_string(),
                    ),
                ],
                body: Some(body.to_string()),
                timeout: Some(TIMEOUT),
                deny_internal: false,
            });
            let next = match &sent {
                Ok(response) => answer(response),
                // a lost answer is tried again, as a 5xx
                Err(err) => {
                    tracing::info!(subscription_id = %device, "FCM did not answer: {}", err.message());
                    Answer::Retry(None)
                }
            };
            match next {
                Answer::Done(outcome) => {
                    if let (Outcome::Failed | Outcome::Unsent, Ok(response)) = (&outcome, &sent) {
                        tracing::warn!(
                            subscription_id = %device,
                            "FCM push failed: {} {}",
                            response.status,
                            error_codes(&response.body).join(" ")
                        );
                    }
                    return outcome;
                }
                Answer::Reauthorize if !reauthorized => {
                    reauthorized = true;
                    self.tokens.clear();
                }
                Answer::Reauthorize => {
                    tracing::warn!(subscription_id = %device, "FCM refused the server's access token");
                    return Outcome::Unsent;
                }
                Answer::Retry(after) if attempt < MAX_ATTEMPTS => {
                    std::thread::sleep(after.unwrap_or(wait).min(MAX_WAIT));
                    wait *= 2;
                }
                Answer::Retry(_) => {
                    tracing::warn!(subscription_id = %device, "FCM push failed after {MAX_ATTEMPTS} tries");
                    return Outcome::Failed;
                }
            }
        }
        Outcome::Failed
    }
}

/// The production sender: the configuration, the process's token and [`crate::outbound::send`] (refused under the read
/// fence, and answered by a spec's HTTP fixtures).
pub(crate) fn sender(config: &Config) -> Sender<'_> {
    Sender {
        config,
        tokens: &TOKENS,
        send: &crate::outbound::send,
        backoff: BACKOFF,
    }
}

#[cfg(test)]
#[path = "fcm_tests.rs"]
mod tests;
