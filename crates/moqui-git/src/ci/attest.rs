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

//! The attestation of a deployment (plans/Dogfood.md §8.6, DG10; docs/tools/Component-Deploy.md "The gate"): the
//! source server says that a commit of a repository passed an environment's pipeline, as a JSON text signed with
//! HMAC-SHA256 under the secret the target issued (the environment's token, the target's `component-deploy`
//! `attestation-secret`). The target verifies the signature over the exact text it was sent, then its freshness, that
//! it is addressed to the target itself (`audience`, the URL the source posts to), and deploys only that commit of
//! that repository, with the data types the attestation names. The secret itself is never sent. Both ends use this module: the
//! source's deploy step ([`super::deploy`]) signs, the target's `deploy#AttestedComponent` (moqui-devkit) verifies.

use hmac::{KeyInit, Mac};
use serde::{Deserialize, Serialize};

/// The version of the attestation's format: 2 added `audience` and `dataTypes` (v1 is not taken).
pub const VERSION: u32 = 2;

/// What the signature covers: this prefix, then the attestation's text.
const DOMAIN: &str = "moqui-deploy-attestation-v2\n";

/// The `kind` of an attestation.
pub const KIND: &str = "moqui-deploy-attestation";

/// The longest attestation text a target reads.
pub const MAX_TEXT: usize = 8 * 1024;

/// The shortest secret either end takes: a secret a person could guess signs nothing (`openssl rand -hex 32` makes
/// 64 characters).
pub const MIN_SECRET: usize = 32;

/// How far ahead of the target's clock an attestation may be issued (the two clocks differ).
const SKEW_MILLIS: i64 = 60_000;

/// A deployment's attestation, as JSON with these names.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attestation {
    pub kind: String,
    /// [`VERSION`].
    pub v: u32,
    /// The target's base URL the source posts to (its environment's `targetUrl`): a target takes only one addressed to
    /// one of its own public URLs, so an attestation for one target is not taken by another that shares the secret.
    pub audience: String,
    /// The URL the target fetches the repository from.
    pub repository_url: String,
    /// The branch, a full ref (`refs/heads/main`).
    pub ref_name: String,
    pub commit_id: String,
    /// The name in the commit's `component.xml`.
    pub component: String,
    /// The environment's pipeline, which passed for the commit.
    pub pipeline: String,
    pub pipeline_run_id: String,
    pub run_num: i64,
    /// Always `success`: nothing else is attested.
    pub run_status: String,
    pub environment: String,
    pub deployment_id: String,
    /// When the source signed it, in milliseconds.
    pub issued_at: i64,
    /// The source server's public URL, for the target's log.
    pub source: String,
    /// The data types the target loads of the component's changed data files (the environment's `dataTypes`); empty
    /// for the target's defaults, as its own Deploy and Update use.
    pub data_types: Vec<String>,
}

impl Attestation {
    /// The branch name of [`Self::ref_name`].
    #[must_use]
    pub fn branch(&self) -> &str {
        self.ref_name.strip_prefix("refs/heads/").unwrap_or(&self.ref_name)
    }
}

/// What the target is sent: the attestation's text and its signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed {
    pub text: String,
    /// `sha256=` and 64 hex digits.
    pub signature: String,
}

fn mac(secret: &str) -> hmac::Hmac<sha2::Sha256> {
    let mut mac = <hmac::Hmac<sha2::Sha256> as KeyInit>::new_from_slice(secret.as_bytes())
        .expect("HMAC takes a key of any length");
    mac.update(DOMAIN.as_bytes());
    mac
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| text.get(at..at + 2).and_then(|pair| u8::from_str_radix(pair, 16).ok()))
        .collect()
}

/// A base URL as two ends compare it: the scheme and the host in lower case, no default port, no trailing slash.
#[must_use]
pub fn base_url(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let scheme = scheme.to_ascii_lowercase();
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, format!("/{path}")),
        None => (rest, String::new()),
    };
    let authority = authority.to_ascii_lowercase();
    let default_port = match scheme.as_str() {
        "https" => ":443",
        "http" => ":80",
        _ => "",
    };
    let authority = match authority.strip_suffix(default_port) {
        Some(host) if !default_port.is_empty() => host.to_string(),
        _ => authority,
    };
    format!("{scheme}://{authority}{path}")
}

/// Sign `attestation` with `secret`.
///
/// # Errors
///
/// When the secret is shorter than [`MIN_SECRET`].
pub fn sign(secret: &str, attestation: &Attestation) -> Result<Signed, String> {
    if secret.len() < MIN_SECRET {
        return Err(format!(
            "the environment's token is shorter than {MIN_SECRET} characters"
        ));
    }
    let text = serde_json::to_string(attestation).map_err(|err| err.to_string())?;
    let mut mac = mac(secret);
    mac.update(text.as_bytes());
    let signature = format!("sha256={}", hex(&mac.finalize().into_bytes()));
    Ok(Signed { text, signature })
}

/// Why a target refuses an attestation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// No secret here, or a short one.
    NoSecret,
    /// Signed with this secret, but addressed to another target than this one (`audience`), or this target does not
    /// know its own public URL to compare.
    Audience,
    /// The signature is not this secret's over this text: forged, altered, or signed for another target.
    Signature,
    /// Signed, but not an attestation of a passed run this version reads.
    Malformed(String),
    /// Issued more than the target's `max-age-seconds` ago.
    Stale,
    /// Issued later than the target's clock allows.
    Future,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSecret => write!(
                f,
                "this server takes no attested deploys (component-deploy attestation-secret)"
            ),
            Self::Signature => write!(f, "the attestation's signature does not verify"),
            Self::Audience => write!(
                f,
                "the attestation is addressed to another server (its audience is not one of this server's public URLs, \
                 component-deploy public-urls)"
            ),
            Self::Malformed(why) => write!(f, "the attestation is not one this server reads: {why}"),
            Self::Stale => write!(f, "the attestation is too old (component-deploy max-age-seconds)"),
            Self::Future => write!(f, "the attestation is issued in the future"),
        }
    }
}

fn hex_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Verify `text` and its `signature` with `secret`, as of `now` (milliseconds): the signature first, over the exact
/// text, compared in constant time; then the attestation itself: its kind and version, a run that passed, a commit
/// id, a branch, a component, and an issue time no older than `max_age_millis` and not ahead of the clock.
///
/// # Errors
///
/// The [`Refusal`].
pub fn verify(
    secret: &str,
    text: &str,
    signature: &str,
    now: i64,
    max_age_millis: i64,
) -> Result<Attestation, Refusal> {
    if secret.len() < MIN_SECRET {
        return Err(Refusal::NoSecret);
    }
    let Some(digest) = signature.trim().strip_prefix("sha256=").and_then(unhex) else {
        return Err(Refusal::Signature);
    };
    if text.len() > MAX_TEXT {
        return Err(Refusal::Signature);
    }
    let mut mac = mac(secret);
    mac.update(text.as_bytes());
    mac.verify_slice(&digest).map_err(|_| Refusal::Signature)?;
    let attestation: Attestation = serde_json::from_str(text).map_err(|err| Refusal::Malformed(err.to_string()))?;
    let malformed = |why: &str| Err(Refusal::Malformed(why.to_string()));
    if attestation.kind != KIND || attestation.v != VERSION {
        return malformed("its kind or version");
    }
    if attestation.run_status != "success" {
        return malformed("its run did not pass");
    }
    if !hex_id(&attestation.commit_id) {
        return malformed("its commit id");
    }
    if !attestation.ref_name.starts_with("refs/heads/") || attestation.branch().is_empty() {
        return malformed("its ref is not a branch");
    }
    if attestation.component.is_empty() || attestation.repository_url.is_empty() || attestation.deployment_id.is_empty()
    {
        return malformed("its component, repository URL or deployment");
    }
    if attestation.audience.trim().is_empty() {
        return malformed("its audience");
    }
    if attestation.issued_at > now + SKEW_MILLIS {
        return Err(Refusal::Future);
    }
    if now - attestation.issued_at > max_age_millis {
        return Err(Refusal::Stale);
    }
    Ok(attestation)
}

/// Whether `attestation` is addressed to this target, whose public base URLs are `own` ([`base_url`] compared).
///
/// # Errors
///
/// [`Refusal::Audience`] when it is not, or when `own` is empty (the target cannot tell).
pub fn check_audience(attestation: &Attestation, own: &[String]) -> Result<(), Refusal> {
    let audience = base_url(&attestation.audience);
    if own
        .iter()
        .any(|url| !url.trim().is_empty() && base_url(url) == audience)
    {
        Ok(())
    } else {
        Err(Refusal::Audience)
    }
}

#[cfg(test)]
#[path = "attest_tests.rs"]
mod tests;
