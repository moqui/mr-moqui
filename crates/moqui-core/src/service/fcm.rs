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
//! A 404, UNREGISTERED, SENDER_ID_MISMATCH or a 400 about `message.token` means the token is no good
//! ([`Outcome::Gone`]); another 400 is the message's fault, logged as an error. 429 and 5xx are tried again a few times
//! with a growing wait, never past a ring's end; after that, as with no access token, FCM is taken to be down and the
//! rest of the send is not tried ([`Outcome::Unsent`]), with no device counted against. Every request goes through
//! [`crate::outbound::send`], so a spec's HTTP fixtures answer it.

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
/// A failed token request is answered again, without asking, for this long.
const FAILURE_SECONDS: i64 = 30;
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

/// A token request that failed, kept for [`FAILURE_SECONDS`] so a push to many devices, or the next push, does not ask
/// the token endpoint again at once.
struct Failure {
    owner: String,
    until: i64,
    reason: String,
}

/// The OAuth access token, cached until shortly before it expires, and a failure to get one, for half a minute. The
/// token is read under a short lock; one fetch at a time runs under its own lock, so a slow token endpoint holds up the
/// threads that need a new token only, and they take the token that fetch got.
pub(crate) struct Tokens {
    slot: Mutex<Option<Cached>>,
    failure: Mutex<Option<Failure>>,
    fetch: Mutex<()>,
}

impl Tokens {
    pub(crate) const fn new() -> Self {
        Self {
            slot: Mutex::new(None),
            failure: Mutex::new(None),
            fetch: Mutex::new(()),
        }
    }

    fn cached(&self, owner: &str, now: i64) -> Option<Result<String, String>> {
        if let Some(cached) = lock(&self.slot).as_ref() {
            if cached.owner == owner && now < cached.expires_at - TOKEN_MARGIN_SECONDS {
                return Some(Ok(cached.token.clone()));
            }
        }
        lock(&self.failure)
            .as_ref()
            .filter(|failure| failure.owner == owner && now < failure.until)
            .map(|failure| Err(failure.reason.clone()))
    }

    /// A token for `account` at `now` (Unix seconds): the cached one, or a new one from its token endpoint. A failure
    /// is answered again for half a minute without a request.
    pub(crate) fn get(&self, account: &ServiceAccount, now: i64, send: Send<'_>) -> Result<String, String> {
        let owner = format!("{} {}", account.client_email, account.token_uri);
        if let Some(answer) = self.cached(&owner, now) {
            return answer;
        }
        let _fetching = lock(&self.fetch);
        // another thread may have fetched while this one waited
        if let Some(answer) = self.cached(&owner, now) {
            return answer;
        }
        let fetched = self.fetch_token(account, now, send);
        match &fetched {
            Ok((token, expires_at)) => {
                *lock(&self.failure) = None;
                *lock(&self.slot) = Some(Cached {
                    owner,
                    token: token.clone(),
                    expires_at: *expires_at,
                });
            }
            Err(reason) => {
                *lock(&self.failure) = Some(Failure {
                    owner,
                    until: now + FAILURE_SECONDS,
                    reason: reason.clone(),
                });
            }
        }
        fetched.map(|(token, _)| token)
    }

    /// The token endpoint's token and when it expires (Unix seconds).
    fn fetch_token(&self, account: &ServiceAccount, now: i64, send: Send<'_>) -> Result<(String, i64), String> {
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
        Ok((token.to_string(), now + expires_in))
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
    /// The caller's name (`caller`: FCM reserves `from`).
    pub caller: String,
    pub video: bool,
    /// When the ring ends, Unix milliseconds.
    pub expires_at: i64,
}

/// What a push is for.
#[derive(Debug, Default, Clone)]
pub(crate) enum Kind {
    /// A notification, as a Web Push carries it.
    #[default]
    Notification,
    /// A call ringing the user.
    Ring(Ring),
    /// The ring of a call ended (answered, declined, timed out, canceled): the app's ring screen stops. FCM only.
    RingEnded { call_session_id: String, reason: String },
}

/// What a push says: what Web Push carries (title, link, topic, tag; never a message's body), and a ring's call.
#[derive(Debug, Default, Clone)]
pub(crate) struct Notice {
    pub title: String,
    pub link: String,
    pub topic: String,
    pub tag: String,
    pub kind: Kind,
    pub high: bool,
    pub ttl: u32,
}

impl Notice {
    /// When pushing it is no longer worth anything (a ring's end), Unix milliseconds.
    fn deadline(&self) -> Option<i64> {
        match &self.kind {
            Kind::Ring(ring) if ring.expires_at > 0 => Some(ring.expires_at),
            _ => None,
        }
    }
}

/// Keys FCM refuses in a data message ("from", "notification", "message_type", and any starting with "google." or
/// "gcm."): a message with one is answered 400 INVALID_ARGUMENT.
pub(crate) fn reserved_key(key: &str) -> bool {
    matches!(key, "from" | "notification" | "message_type") || key.starts_with("google.") || key.starts_with("gcm.")
}

/// The most UTF-8 bytes each value of a data message takes, so a message is always well under FCM's 4 KB.
const MAX_TITLE_BYTES: usize = 512;
const MAX_LINK_BYTES: usize = 1024;
const MAX_FIELD_BYTES: usize = 128;
/// The whole data map, as JSON, at most: what [`message`] cuts the link and then the title to.
pub(crate) const MAX_DATA_BYTES: usize = 3072;

/// `text` cut to at most `max` UTF-8 bytes, at a character boundary.
pub(crate) fn cut_bytes(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn data_bytes(data: &serde_json::Map<String, serde_json::Value>) -> usize {
    serde_json::Value::Object(data.clone()).to_string().len()
}

/// The `messages:send` body for the device with `token`: a data message (every value a string), so the app's own code
/// gets it and shows it, a ring as an incoming call. `kind` is `notification`, `ring` or `ringEnded`. A tag collapses an
/// earlier push of the same tag that the device has not yet received, so a ring's end replaces a ring not yet given.
pub(crate) fn message(token: &str, notice: &Notice) -> serde_json::Value {
    let mut data = serde_json::Map::new();
    let mut put = |name: &str, value: &str, max: usize| {
        // one reserved key makes FCM refuse the whole message
        debug_assert!(!reserved_key(name), "{name} is reserved in an FCM data message");
        if !reserved_key(name) {
            data.insert(name.to_string(), cut_bytes(value, max).into());
        }
    };
    let kind = match notice.kind {
        Kind::Notification => "notification",
        Kind::Ring(_) => "ring",
        Kind::RingEnded { .. } => "ringEnded",
    };
    put("kind", kind, MAX_FIELD_BYTES);
    put("topic", &notice.topic, MAX_FIELD_BYTES);
    put("tag", &notice.tag, MAX_FIELD_BYTES);
    match &notice.kind {
        Kind::RingEnded {
            call_session_id,
            reason,
        } => {
            put("callSessionId", call_session_id, MAX_FIELD_BYTES);
            put("reason", reason, MAX_FIELD_BYTES);
        }
        Kind::Notification | Kind::Ring(_) => {
            put("title", &notice.title, MAX_TITLE_BYTES);
            put("link", &notice.link, MAX_LINK_BYTES);
        }
    }
    if let Kind::Ring(ring) = &notice.kind {
        put("callSessionId", &ring.call_session_id, MAX_FIELD_BYTES);
        put("chatRoomId", &ring.chat_room_id, MAX_FIELD_BYTES);
        put("caller", &ring.caller, MAX_TITLE_BYTES);
        put("video", if ring.video { "true" } else { "false" }, MAX_FIELD_BYTES);
        put("expiresAt", &ring.expires_at.to_string(), MAX_FIELD_BYTES);
    }
    // JSON escapes can grow a value: cut the link, then the title, until the whole fits
    for name in ["link", "title"] {
        while data_bytes(&data) > MAX_DATA_BYTES {
            let Some(value) = data.get(name).and_then(serde_json::Value::as_str).map(str::to_string) else {
                break;
            };
            if value.is_empty() {
                break;
            }
            let shorter = cut_bytes(&value, value.len() / 2).to_string();
            data.insert(name.to_string(), shorter.into());
        }
    }
    let mut android = serde_json::json!({
        "priority": if notice.high { "HIGH" } else { "NORMAL" },
        "ttl": format!("{}s", notice.ttl),
    });
    if !notice.tag.is_empty() {
        android["collapse_key"] = cut_bytes(&notice.tag, MAX_FIELD_BYTES).into();
    }
    serde_json::json!({"message": {"token": token, "data": data, "android": android}})
}

/// What FCM's answer means for the device.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Delivered,
    /// 404, UNREGISTERED, SENDER_ID_MISMATCH, or a 400 about the token: the token is no good; never send to it again.
    Gone,
    /// Another answer about this message (a 400 not about the token): logged as an error, and the device is left as
    /// it is, since the fault is most likely the server's message, not the device.
    Rejected,
    /// FCM or Google is not taking messages now (no access token, refused credentials, 429 or 5xx after the tries, no
    /// answer): the device is left as it is, and the rest of the send is not tried, as it would fail the same way.
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

/// The `errorCode`s of an FCM error's details, and whether a detail's field violation names `message.token`.
struct ErrorInfo {
    codes: Vec<String>,
    token_field: bool,
}

fn error_info(body: &str) -> ErrorInfo {
    let value: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    let mut info = ErrorInfo {
        codes: Vec::new(),
        token_field: false,
    };
    for detail in value["error"]["details"].as_array().into_iter().flatten() {
        if let Some(code) = detail["errorCode"].as_str() {
            info.codes.push(code.to_string());
        }
        for violation in detail["fieldViolations"].as_array().into_iter().flatten() {
            if violation["field"].as_str() == Some("message.token") {
                info.token_field = true;
            }
        }
    }
    info
}

fn answer(response: &Response) -> Answer {
    let info = error_info(&response.body);
    let has = |code: &str| info.codes.iter().any(|found| found == code);
    match response.status {
        200..=299 => Answer::Done(Outcome::Delivered),
        404 => Answer::Done(Outcome::Gone),
        _ if has("UNREGISTERED") => Answer::Done(Outcome::Gone),
        // the token belongs to another Firebase project's app
        403 if has("SENDER_ID_MISMATCH") => Answer::Done(Outcome::Gone),
        400 if info.token_field => Answer::Done(Outcome::Gone),
        400 => Answer::Done(Outcome::Rejected),
        401 => Answer::Reauthorize,
        429 | 500..=599 => Answer::Retry(retry_after(response)),
        // a 403 with no per-token code (the project's API off, the account's role) and anything else: the server's
        _ => Answer::Done(Outcome::Unsent),
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

/// The clock, Unix milliseconds: for a ring's deadline.
fn now_millis() -> i64 {
    jiff::Timestamp::now().as_millisecond()
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
    /// Send `body` ([`message`]), trying again after a 429 or 5xx (never waiting past `deadline`, a ring's end in Unix
    /// milliseconds) and once after a refused access token. A failure is logged by `device` (the registration's id),
    /// never by its token.
    pub(crate) fn deliver(&self, device: &str, body: &serde_json::Value, now: i64, deadline: Option<i64>) -> Outcome {
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
            let codes = || match &sent {
                Ok(response) => format!("{} {}", response.status, error_info(&response.body).codes.join(" ")),
                Err(err) => err.message().to_string(),
            };
            match next {
                Answer::Done(Outcome::Rejected) => {
                    tracing::error!(subscription_id = %device, "FCM refused the message: {}", codes());
                    return Outcome::Rejected;
                }
                Answer::Done(Outcome::Unsent) => {
                    tracing::warn!(subscription_id = %device, "FCM push not sent: {}", codes());
                    return Outcome::Unsent;
                }
                Answer::Done(outcome) => return outcome,
                Answer::Reauthorize if !reauthorized => {
                    reauthorized = true;
                    self.tokens.clear();
                }
                Answer::Reauthorize => {
                    tracing::warn!(subscription_id = %device, "FCM refused the server's access token");
                    return Outcome::Unsent;
                }
                Answer::Retry(after) if attempt < MAX_ATTEMPTS => {
                    let pause = after.unwrap_or(wait).min(MAX_WAIT);
                    let pause_ms = i64::try_from(pause.as_millis()).unwrap_or(i64::MAX);
                    if deadline.is_some_and(|end| now_millis().saturating_add(pause_ms) >= end) {
                        tracing::info!(subscription_id = %device, "FCM push not tried again: the ring is over by then");
                        return Outcome::Unsent;
                    }
                    std::thread::sleep(pause);
                    wait *= 2;
                }
                Answer::Retry(_) => {
                    tracing::warn!(subscription_id = %device, "FCM push not sent after {MAX_ATTEMPTS} tries: {}", codes());
                    return Outcome::Unsent;
                }
            }
        }
        Outcome::Unsent
    }

    /// [`Self::deliver`] of `notice` to the device with `token`, with the notice's deadline.
    pub(crate) fn deliver_notice(&self, device: &str, token: &str, notice: &Notice, now: i64) -> Outcome {
        self.deliver(device, &message(token, notice), now, notice.deadline())
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
