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

//! The origins trusted for reading Constitution references in the app's dialog (plans/LLM-A2A-Assist.md 7.10,
//! "Trusted reference viewer"): instance policy in `runtime/conf/ConstitutionOrigins.json`, `https://grok-the-rock.org`
//! when there is no file. Exact HTTPS origins on the default port, no wildcards. It is separate from the Constitution
//! (a reset of the document leaves it), from CORS and from the canvas's external content setting; an organization's
//! document cannot add to it.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// The origins trusted when there is no policy file.
pub const DEFAULT_ORIGINS: &[&str] = &["https://grok-the-rock.org"];

/// The policy file, under the runtime directory.
pub const FILE: &str = "conf/ConstitutionOrigins.json";

/// The trusted origins in effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub origins: Vec<String>,
    /// The hash of the origin list, the expected revision of a change.
    pub revision: String,
    /// Whether the instance file sets it (else the default).
    pub customized: bool,
    /// Why the file was not used, when it is unreadable or invalid (nothing is trusted then).
    pub problem: Option<String>,
}

impl Policy {
    fn of(origins: Vec<String>, customized: bool, problem: Option<String>) -> Self {
        let revision = moqui_lib::json::canonical_sha256(&json!({ "origins": origins }));
        Self { origins, revision, customized, problem }
    }

    /// Whether `origin` (as [`super::document::check_url`] gives it) is trusted.
    pub fn allows(&self, origin: &str) -> bool {
        self.origins.iter().any(|trusted| trusted == origin)
    }
}

/// `text` as an origin: `https://host`, the host in lower case, no port but the default, no path, query, fragment or
/// credentials.
///
/// # Errors
///
/// Returns a message saying what is wrong.
pub fn normalize_origin(text: &str) -> Result<String, String> {
    let text = text.trim().trim_end_matches('/');
    if text.contains('*') {
        return Err(format!("{text}: wildcards are not allowed; name each origin"));
    }
    let parsed = url::Url::parse(text).map_err(|error| format!("{text}: not an origin ({error})"))?;
    if parsed.scheme() != "https" {
        return Err(format!("{text}: a trusted origin uses https"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(format!("{text}: an origin carries no credentials"));
    }
    if parsed.port().is_some() {
        return Err(format!("{text}: a trusted origin uses the default port"));
    }
    if parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(format!("{text}: an origin has no path, query or fragment"));
    }
    let host = parsed.host_str().filter(|host| !host.is_empty()).ok_or_else(|| format!("{text}: no host"))?;
    Ok(format!("https://{}", host.to_ascii_lowercase()))
}

/// The policy file of the runtime directory.
pub fn path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(FILE)
}

/// The policy in effect: the instance file's, else the default. A file that cannot be read or has an invalid origin
/// trusts nothing (logged, with the problem on the policy): a broken policy never widens trust.
pub fn load(runtime_dir: &Path) -> Policy {
    let file = path(runtime_dir);
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Policy::of(DEFAULT_ORIGINS.iter().map(|origin| (*origin).to_string()).collect(), false, None);
        }
        Err(error) => return broken(&file, format!("it could not be read: {error}")),
    };
    let origins = serde_json::from_str::<Value>(&text)
        .map_err(|error| format!("it is not valid JSON: {error}"))
        .and_then(|value| {
            value
                .get("origins")
                .and_then(Value::as_array)
                .ok_or_else(|| "it has no origins list".to_string())?
                .iter()
                .map(|item| item.as_str().ok_or_else(|| "an origin is not text".to_string()).and_then(normalize_origin))
                .collect::<Result<Vec<String>, String>>()
        });
    match origins {
        Ok(origins) => Policy::of(origins, true, None),
        Err(problem) => broken(&file, problem),
    }
}

fn broken(file: &Path, problem: String) -> Policy {
    tracing::error!("the Constitution reference origins in {} trust nothing: {problem}", file.display());
    Policy::of(Vec::new(), true, Some(problem))
}

/// Replace the policy with `origins` when `expected_revision` is the current one; written whole, through a temporary
/// file and a rename.
///
/// # Errors
///
/// Returns a message for a stale revision, an invalid origin or a failed write; the policy is then unchanged.
pub fn store(runtime_dir: &Path, origins: &[String], expected_revision: &str) -> Result<Policy, String> {
    let current = load(runtime_dir);
    if current.revision != expected_revision {
        return Err("the trusted origins changed since they were read; reload and try again".to_string());
    }
    let mut normalized: Vec<String> = Vec::new();
    for origin in origins.iter().filter(|origin| !origin.trim().is_empty()) {
        let origin = normalize_origin(origin)?;
        if !normalized.contains(&origin) {
            normalized.push(origin);
        }
    }
    let text = serde_json::to_string_pretty(&json!({ "origins": normalized })).unwrap_or_default() + "\n";
    super::write_atomic(&path(runtime_dir), &text)?;
    Ok(Policy::of(normalized, true, None))
}
