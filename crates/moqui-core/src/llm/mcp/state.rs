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

//! The `requestState` of a write that waits for its approval (plans/LLM-A2A-Assist.md 4.16, 9 "Approvals"; MCP
//! 2026-07-28 "Multi Round-Trip Requests"). The client echoes it on the retry of the call and must not read it, and
//! the server treats it as attacker-controlled: it is `base64url(json{v, aid, sub, cid, tool, dig, exp, n})` and
//! `.` and `base64url(HMAC-SHA256)` of the first part, under the `mcp request-state-key` (base64), else a key derived
//! from `session_cookie_key`, else a random key of the process (a single node's states then last until it
//! restarts). On a retry the MAC is checked first (in constant time), then the version, the expiry, the principal
//! (`sub`, the user's id) and the request (`tool` and `dig`, the digest of the call's name and arguments); any
//! failure refuses the state and nothing runs. The state names an approval (`aid`) and its run
//! (`cid`); single use is the approval's, server side (Approved moves to Executing once), since a state may be shown
//! again until it expires.

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};
use sha2::Sha256;

use crate::context::ExecutionContextFactory;

type StateMac = Hmac<Sha256>;

/// The version of the payload.
pub const VERSION: u8 = 1;

/// The label the key derived from `session_cookie_key` is made with, so a state's MAC is never a session cookie's.
const DERIVED_LABEL: &str = "moqui mcp requestState v1";

/// The largest state taken, before it is decoded.
const MAX_STATE: usize = 4096;

/// What a state says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    pub v: u8,
    /// The approval the call waits for.
    pub aid: String,
    /// The principal: the user the call was made as.
    pub sub: String,
    /// The run of the call.
    pub cid: String,
    /// The tool's MCP name.
    pub tool: String,
    /// [`digest`] of the call.
    pub dig: String,
    /// Epoch milliseconds after which the state is refused.
    pub exp: i64,
    /// A random nonce, so two states of the same call differ.
    pub n: String,
}

/// Why a state was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Not two base64url parts, too long, or a payload that does not read.
    Malformed,
    /// The MAC does not match: changed, or not made by this server's key.
    Tampered,
    /// A version this server does not make.
    Version,
    Expired,
    /// Made for another user.
    OtherPrincipal,
    /// Made for another tool or other arguments.
    OtherRequest,
}

impl Refused {
    /// What the client is told.
    pub fn message(self) -> &'static str {
        match self {
            Refused::Malformed => "the requestState is not one this server made",
            Refused::Tampered => "the requestState was changed or was not made by this server",
            Refused::Version => "the requestState is of a version this server does not take",
            Refused::Expired => "the requestState has expired; call the tool again for a new approval",
            Refused::OtherPrincipal => "the requestState was made for another user",
            Refused::OtherRequest => "the requestState was made for another tool call (its name or arguments differ)",
        }
    }
}

/// The digest a state binds: SHA-256 over the RFC 8785 canonical JSON of `{method: "tools/call", name, arguments}`,
/// an absent `arguments` as `{}`.
pub fn digest(name: &str, arguments: &Json) -> String {
    let arguments = if arguments.is_null() {
        json!({})
    } else {
        arguments.clone()
    };
    moqui_lib::json::canonical_sha256(&json!({"method": "tools/call", "name": name, "arguments": arguments}))
}

/// A new nonce: 96 random bits, base64url.
///
/// # Errors
///
/// Returns an error when the random source fails.
pub fn nonce() -> Result<String, moqui_lib::error::Error> {
    let mut bytes = [0_u8; 12];
    getrandom::fill(&mut bytes)
        .map_err(|error| moqui_lib::error::Error::new(format!("random source failed: {error}")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// The key states are made with: the conf's `request-state-key` (standard base64, at least 32 bytes), else one
/// derived from `session_cookie_key`, else a random key of this process.
///
/// # Errors
///
/// Returns an error when the conf's key does not read (no state is made and a write is refused), or there is no
/// random source.
pub fn key(factory: &ExecutionContextFactory) -> Result<Vec<u8>, moqui_lib::error::Error> {
    let configured = crate::llm::facade(factory).conf().mcp.request_state_key.clone();
    if let Some(text) = configured.as_deref().map(str::trim).filter(|text| !text.is_empty()) {
        let key = STANDARD
            .decode(text.as_bytes())
            .map_err(|_| moqui_lib::error::Error::new("the mcp request-state-key is not standard base64"))?;
        if key.len() < 32 {
            return Err(moqui_lib::error::Error::new(
                "the mcp request-state-key is shorter than 32 bytes",
            ));
        }
        return Ok(key);
    }
    if let Ok(key) = crate::user::derived_cookie_key(DERIVED_LABEL) {
        return Ok(key);
    }
    process_key()
}

/// A random key of this process, when neither key is configured: a single node's states then last until it restarts
/// (a cluster has `session_cookie_key`, so its nodes share the derived key).
fn process_key() -> Result<Vec<u8>, moqui_lib::error::Error> {
    static KEY: std::sync::OnceLock<Option<Vec<u8>>> = std::sync::OnceLock::new();
    KEY.get_or_init(|| {
        let mut bytes = vec![0_u8; 32];
        match getrandom::fill(&mut bytes) {
            Ok(()) => {
                tracing::warn!(
                    "the MCP requestState is signed with a key of this process (no mcp request-state-key or \
                     session_cookie_key), so a write waiting for its approval must be retried before a restart"
                );
                Some(bytes)
            }
            Err(error) => {
                tracing::error!("no random key for the MCP requestState: {error}");
                None
            }
        }
    })
    .clone()
    .ok_or_else(|| moqui_lib::error::Error::new("no key for the MCP requestState"))
}

fn mac_of(key: &[u8], payload: &str) -> StateMac {
    // an HMAC takes a key of any length
    let mut mac = StateMac::new_from_slice(key).expect("HMAC takes a key of any length");
    mac.update(payload.as_bytes());
    mac
}

/// The state of `claims` under `key`.
pub fn seal(key: &[u8], claims: &Claims) -> String {
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap_or_default());
    let tag = URL_SAFE_NO_PAD.encode(mac_of(key, &payload).finalize().into_bytes());
    format!("{payload}.{tag}")
}

/// What a retry must match.
#[derive(Clone, Copy, Debug)]
pub struct Expect<'a> {
    pub user_id: &'a str,
    pub tool: &'a str,
    /// [`digest`] of the retry.
    pub digest: &'a str,
    /// Epoch milliseconds now.
    pub now: i64,
}

/// The claims of `state`, when it was made under `key` and is good for `expect`.
///
/// # Errors
///
/// Why it is refused: the MAC first, then the version, the expiry, the principal and the request.
pub fn open(key: &[u8], state: &str, expect: &Expect<'_>) -> Result<Claims, Refused> {
    if state.len() > MAX_STATE {
        return Err(Refused::Malformed);
    }
    let (payload, tag) = state.split_once('.').ok_or(Refused::Malformed)?;
    let tag = URL_SAFE_NO_PAD.decode(tag.as_bytes()).map_err(|_| Refused::Malformed)?;
    // verify_slice compares in constant time
    mac_of(key, payload).verify_slice(&tag).map_err(|_| Refused::Tampered)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload.as_bytes())
        .map_err(|_| Refused::Malformed)?;
    let claims: Claims = serde_json::from_slice(&bytes).map_err(|_| Refused::Malformed)?;
    if claims.v != VERSION {
        return Err(Refused::Version);
    }
    if claims.exp <= expect.now {
        return Err(Refused::Expired);
    }
    if claims.sub.is_empty() || claims.sub != expect.user_id {
        return Err(Refused::OtherPrincipal);
    }
    if claims.tool != expect.tool || claims.dig != expect.digest {
        return Err(Refused::OtherRequest);
    }
    Ok(claims)
}
