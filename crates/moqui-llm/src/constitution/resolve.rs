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

//! Which Constitution applies (plans/LLM-A2A-Assist.md 7.10, "Ownership and resolution"): an organization's own
//! document, else the instance customization (`runtime/conf/Constitution.json`), else the shipped default
//! (`framework/docs/Constitution.json`). One whole document; nothing is merged.
//!
//! Organizations are a component's: the framework asks the services named by `constitution-provider` elements of the
//! `llm-facade` conf (Mantle's `mantle.party.ConstitutionServices.get#ScopedConstitution`), each an implementation of
//! `org.moqui.admin.ConstitutionServices.get#ScopedConstitution`, under the read fence. A source that cannot be read
//! or is invalid is logged by source and organization and passed over with a diagnostic; a provider failure is never
//! taken for "no organization document", and no other organization is chosen in its place.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use moqui_lib::object::{to_plain_string, Value as MValue};
use serde_json::Value;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::fence;

use super::document;

/// The interface every `constitution-provider` service implements.
pub const PROVIDER_SERVICE: &str = "org.moqui.admin.ConstitutionServices.get#ScopedConstitution";

/// The shipped default, under the framework directory.
pub const SHIPPED_FILE: &str = "docs/Constitution.json";

/// The instance customization, under the runtime directory.
pub const INSTANCE_FILE: &str = "conf/Constitution.json";

/// Where a document came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Organization,
    Instance,
    Shipped,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Organization => "organization",
            Self::Instance => "instance",
            Self::Shipped => "shipped",
        }
    }
}

/// Which document to resolve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope<'a> {
    /// The global document (instance, else shipped), as for a job with no organization.
    Global,
    /// The shipped default alone.
    Shipped,
    /// The document of a user's active internal organization, else the global one.
    ActiveOf(&'a str),
    /// The document of a given organization the user may read, else the global one.
    Organization { user_id: &'a str, party_id: &'a str },
}

/// The document that applies, with where it came from.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub document: Arc<Value>,
    pub source: Source,
    pub organization_id: Option<String>,
    pub organization_name: Option<String>,
    pub hash: String,
    /// The sources passed over and why (shown to readers and editors).
    pub diagnostics: Vec<String>,
}

/// The shipped default's path.
pub fn shipped_path(factory: &ExecutionContextFactory) -> PathBuf {
    factory.framework_dir().join(SHIPPED_FILE)
}

/// The instance customization's path.
pub fn instance_path(factory: &ExecutionContextFactory) -> PathBuf {
    factory.runtime_dir().join(INSTANCE_FILE)
}

/// A valid document read from a file: `Ok(None)` when there is no file.
///
/// # Errors
///
/// Returns why the file cannot be used (unreadable, not JSON, invalid).
pub fn read_file(path: &Path) -> Result<Option<(Arc<Value>, String)>, String> {
    type Cached = (SystemTime, u64, Arc<Value>, String);
    static CACHE: OnceLock<Mutex<BTreeMap<PathBuf, Cached>>> = OnceLock::new();
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("it could not be read: {error}")),
    };
    let stamp = (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len());
    let cache = CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    if let Ok(cache) = cache.lock() {
        if let Some((modified, len, document, hash)) = cache.get(path) {
            if (*modified, *len) == stamp {
                return Ok(Some((Arc::clone(document), hash.clone())));
            }
        }
    }
    let text = std::fs::read_to_string(path).map_err(|error| format!("it could not be read: {error}"))?;
    let (document, hash) = checked(&text)?;
    if let Ok(mut cache) = cache.lock() {
        cache.insert(path.to_path_buf(), (stamp.0, stamp.1, Arc::clone(&document), hash.clone()));
    }
    Ok(Some((document, hash)))
}

/// A valid document from its text, with its hash.
///
/// # Errors
///
/// Returns the problems, joined.
pub fn checked(text: &str) -> Result<(Arc<Value>, String), String> {
    let document = document::parse(text).map_err(|error| error.describe())?;
    let errors = document::validate(&document);
    if !errors.is_empty() {
        let shown: Vec<String> = errors.iter().take(5).map(document::FieldError::describe).collect();
        return Err(format!("it is not a valid Constitution ({})", shown.join("; ")));
    }
    let hash = document::hash(&document);
    Ok((Arc::new(document), hash))
}

/// What a provider said about a user's organization.
#[derive(Clone, Debug, Default)]
struct Answer {
    status: String,
    party_id: Option<String>,
    name: Option<String>,
    text: Option<String>,
}

/// The Constitution for `scope`.
///
/// # Errors
///
/// Returns the diagnostics when not even the shipped default can be used: the Constitution is then unavailable.
pub fn resolve(factory: &ExecutionContextFactory, scope: Scope<'_>) -> Result<Resolved, Vec<String>> {
    let mut diagnostics = Vec::new();
    let mut org: (Option<String>, Option<String>) = (None, None);
    let request = match scope {
        Scope::ActiveOf(user_id) => Some((user_id, None)),
        Scope::Organization { user_id, party_id } => Some((user_id, Some(party_id))),
        Scope::Global | Scope::Shipped => None,
    };
    if let Some((user_id, party_id)) = request {
        let providers = super::super::facade(factory).conf().constitution_providers.clone();
        for service in providers {
            match ask(&service, user_id, party_id) {
                Ok(answer) if answer.status == "none" || answer.status.is_empty() => continue,
                Ok(answer) => {
                    org = (answer.party_id.clone(), answer.name.clone());
                    if answer.status == "override" {
                        let label = answer.party_id.clone().unwrap_or_default();
                        match answer.text.as_deref().map(checked) {
                            Some(Ok((document, hash))) => {
                                return Ok(Resolved {
                                    document,
                                    source: Source::Organization,
                                    organization_id: answer.party_id,
                                    organization_name: answer.name,
                                    hash,
                                    diagnostics,
                                });
                            }
                            Some(Err(problem)) => {
                                tracing::error!("the Constitution of organization {label} is passed over: {problem}");
                                diagnostics.push(format!("The organization's own Constitution is not used: {problem}"));
                            }
                            None => diagnostics.push("The organization's own Constitution is empty.".to_string()),
                        }
                    }
                    break;
                }
                Err(problem) => {
                    tracing::error!("the Constitution provider {service} failed for {user_id}: {problem}");
                    diagnostics.push(format!("The organization's Constitution could not be read: {problem}"));
                    break;
                }
            }
        }
    }
    let with_org = |mut resolved: Resolved, diagnostics: Vec<String>| {
        resolved.organization_id.clone_from(&org.0);
        resolved.organization_name.clone_from(&org.1);
        resolved.diagnostics = diagnostics;
        resolved
    };
    if scope != Scope::Shipped {
        let path = instance_path(factory);
        match read_file(&path) {
            Ok(Some((document, hash))) => {
                let resolved = Resolved {
                    document,
                    source: Source::Instance,
                    organization_id: None,
                    organization_name: None,
                    hash,
                    diagnostics: Vec::new(),
                };
                return Ok(with_org(resolved, diagnostics));
            }
            Ok(None) => {}
            Err(problem) => {
                tracing::error!("the instance Constitution {} is passed over: {problem}", path.display());
                diagnostics.push(format!("The instance customization is not used: {problem}"));
            }
        }
    }
    let path = shipped_path(factory);
    match read_file(&path) {
        Ok(Some((document, hash))) => {
            let resolved = Resolved {
                document,
                source: Source::Shipped,
                organization_id: None,
                organization_name: None,
                hash,
                diagnostics: Vec::new(),
            };
            Ok(with_org(resolved, diagnostics))
        }
        Ok(None) => {
            tracing::error!("the shipped Constitution {} is missing", path.display());
            diagnostics.push("The shipped Constitution is missing, so no Constitution applies.".to_string());
            Err(diagnostics)
        }
        Err(problem) => {
            tracing::error!("the shipped Constitution {} cannot be used: {problem}", path.display());
            diagnostics.push(format!("The shipped Constitution cannot be used ({problem}), so no Constitution applies."));
            Err(diagnostics)
        }
    }
}

/// A provider's answer, called under the read fence.
fn ask(service: &str, user_id: &str, party_id: Option<&str>) -> Result<Answer, String> {
    let guard = fence::enter(format!("constitution provider {service}"));
    let mut parameters = BTreeMap::new();
    parameters.insert("userId".to_string(), MValue::string(user_id));
    if let Some(party_id) = party_id {
        parameters.insert("organizationPartyId".to_string(), MValue::string(party_id));
    }
    let result = moqui_core::service::call::sync_checked(
        service,
        &parameters,
        &moqui_core::service::call::CallOptions::default(),
    );
    let violations = guard.violations();
    drop(guard);
    if !violations.is_empty() {
        let tried: Vec<String> = violations.iter().map(fence::Violation::describe).collect();
        return Err(format!("it tried {}", tried.join(", ")));
    }
    let result = result.map_err(|error| error.message().to_string())?;
    let text = |key: &str| {
        result
            .get(key)
            .filter(|value| !value.is_null())
            .map(to_plain_string)
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
    };
    Ok(Answer {
        status: text("scopeStatus").unwrap_or_default(),
        party_id: text("organizationPartyId"),
        name: text("organizationName"),
        text: text("documentText"),
    })
}
