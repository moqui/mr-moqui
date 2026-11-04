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

//! Sign-in through the server's identity providers for an installed app (RFC 8252; the server's side is
//! `crates/moqui-core/src/service/sso_app.rs`, plans/Mobile-Desktop.md "Remote applications and authentication"). The
//! app makes a PKCE verifier and a state, opens the system browser at the server's `/sso/appLogin` ([`start`]), and the
//! server hands the browser back to the app's redirect URI with a single-use code and the state; the app checks the
//! state ([`callback`]) and takes the code with its verifier for a device key ([`exchange`]). The verifier never leaves
//! the app before the exchange, so a code another app caught is no use to it.

use std::time::{Duration, Instant};

use base64::Engine;
use moqui_lib::error::Error;
use sha2::Digest;

use crate::http::{client, form_body, reason};

/// The desktop app's redirect URI, as the server allows it by default (`webapp_app_redirect_uris`).
pub const DESKTOP_REDIRECT: &str = "org.moqui.desktop:/sso";

/// The mobile app's redirect URI (its Android intent filter), allowed by the server by default too.
pub const MOBILE_REDIRECT: &str = "org.moqui.mobile:/sso";

/// How long a sign-in waits for the browser to come back.
const PENDING_FOR: Duration = Duration::from_secs(15 * 60);

/// An identity provider the server offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    pub id: String,
    pub description: String,
    pub icon: String,
}

/// A sign-in under way in the browser: what the app keeps until the server hands the code back.
#[derive(Debug, Clone)]
pub struct Pending {
    pub server: String,
    pub verifier: String,
    pub state: String,
    pub redirect: String,
    started: Instant,
}

impl Pending {
    /// Whether the browser took too long: the sign-in is started again.
    pub fn stale(&self) -> bool {
        self.started.elapsed() > PENDING_FOR
    }
}

/// The server's identity providers (`/sso/appProviders`).
///
/// # Errors
///
/// Returns the transport's error, or the status of a refusal.
pub fn providers(base: &str) -> Result<Vec<Provider>, Error> {
    let url = format!("{base}/sso/appProviders");
    let response = client()?
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .map_err(|err| Error::new(format!("{url}: {err}")))?;
    let status = response.status().as_u16();
    let value: serde_json::Value = serde_json::from_str(&response.text().unwrap_or_default()).unwrap_or_default();
    if status >= 400 {
        return Err(Error::new(reason(&value, status)));
    }
    let text = |flow: &serde_json::Value, name: &str| flow[name].as_str().unwrap_or("").to_string();
    Ok(value["authFlowList"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|flow| Provider {
                    id: text(flow, "authFlowId"),
                    description: text(flow, "description"),
                    icon: text(flow, "iconName"),
                })
                .filter(|provider| !provider.id.is_empty())
                .collect()
        })
        .unwrap_or_default())
}

/// A new sign-in at `base` through the provider `auth_flow_id`, for the app named `device` (which the browser shows when
/// it asks the user to let the app in): what to keep, and the URL to open in the system browser.
///
/// # Errors
///
/// Returns an error when the system random source fails.
pub fn start(base: &str, auth_flow_id: &str, redirect: &str, device: &str) -> Result<(Pending, String), Error> {
    let verifier = random_text(48)?;
    let state = random_text(24)?;
    let query = form_body(&[
        ("authFlowId", auth_flow_id),
        ("codeChallenge", &challenge_of(&verifier)),
        ("codeChallengeMethod", "S256"),
        ("state", &state),
        ("redirectUri", redirect),
        ("deviceName", device),
    ]);
    let url = format!("{base}/sso/appLogin?{query}");
    let pending = Pending {
        server: base.to_string(),
        verifier,
        state,
        redirect: redirect.to_string(),
        started: Instant::now(),
    };
    Ok((pending, url))
}

/// The state of a hand-back link (`state=`), to find its sign-in.
pub fn link_state(link: &str) -> Option<String> {
    let url = reqwest::Url::parse(link).ok()?;
    url.query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
}

/// The code of the server's hand-back to the app (`<redirect>?code=…&state=…`), when its state is the pending
/// sign-in's.
///
/// # Errors
///
/// Returns the provider's or server's error (`error=`), or an error for a state that is not the pending one (a hand-back
/// this app did not start) or a link with no code.
pub fn callback(link: &str, pending: &Pending) -> Result<String, Error> {
    let url = reqwest::Url::parse(link).map_err(|err| Error::new(format!("{link}: {err}")))?;
    let value = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    if value("state").as_deref() != Some(pending.state.as_str()) {
        return Err(Error::new("the sign-in came back for another request; start it again"));
    }
    if let Some(error) = value("error") {
        return Err(Error::new(format!("the sign-in was not completed: {error}")));
    }
    value("code")
        .filter(|code| !code.is_empty())
        .ok_or_else(|| Error::new("the sign-in came back with no code"))
}

/// Take the code, with the pending sign-in's verifier, for a device key named `device` (`/sso/appToken`).
///
/// # Errors
///
/// Returns the server's reason for a refused code, or the transport's error.
pub fn exchange(pending: &Pending, code: &str, device: &str) -> Result<String, Error> {
    let url = format!("{}/sso/appToken", pending.server);
    let response = client()?
        .post(&url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_body(&[
            ("code", code),
            ("codeVerifier", pending.verifier.as_str()),
            ("redirectUri", pending.redirect.as_str()),
            ("deviceName", device),
        ]))
        .send()
        .map_err(|err| Error::new(format!("{url}: {err}")))?;
    let status = response.status().as_u16();
    let value: serde_json::Value = serde_json::from_str(&response.text().unwrap_or_default()).unwrap_or_default();
    match value["loginKey"].as_str() {
        Some(key) if status < 400 && !key.is_empty() => Ok(key.to_string()),
        _ => Err(Error::new(reason(&value, status))),
    }
}

/// The S256 challenge of a verifier (RFC 7636): base64url of its SHA-256, with no padding.
pub fn challenge_of(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()))
}

/// `bytes` random bytes as base64url, no padding: a verifier of 48 is 64 characters, inside RFC 7636's 43 to 128.
fn random_text(bytes: usize) -> Result<String, Error> {
    let mut buffer = vec![0_u8; bytes];
    getrandom::fill(&mut buffer).map_err(|err| Error::new(format!("random source failed: {err}")))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buffer))
}

#[cfg(test)]
#[path = "sso_tests.rs"]
mod tests;
