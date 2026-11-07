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

//! The Constitution each agent run used (plans/LLM-A2A-Assist.md 7.10, "Run scope and caching"). A run resolves its
//! document, organization and locale when it starts and keeps them through its steps, pauses and resume; the document
//! itself is stored once per distinct hash in `AgentConstitutionSnapshot`, which also keeps the before and after of
//! every saved change. The run's key (source, organization, hash, locale) decides when a conversation's frozen prefix
//! is rebuilt: a change applies from the next run on, never in the middle of one.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use moqui_lib::error::Error;
use moqui_lib::object::Value as MValue;
use serde_json::Value;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::EntityFacade;
use moqui_core::transaction::TransactionAttribute;

use super::resolve::{self, Resolved, Scope};
use super::{document, locale, render};
use crate::prompt::Surface;
use crate::text_of;

/// The snapshot entity.
pub const SNAPSHOT: &str = "moqui.agent.AgentConstitutionSnapshot";

/// What a run uses: where its document came from and the locale its section is in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunConstitution {
    /// `organization`, `instance`, `shipped`, or `unavailable`.
    pub source: String,
    pub organization_id: Option<String>,
    /// The document's hash; `None` when unavailable.
    pub hash: Option<String>,
    /// The normalized locale of the run's section.
    pub locale: String,
}

impl RunConstitution {
    /// The key a conversation's frozen prefix is built for.
    pub fn key(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.source,
            self.organization_id.as_deref().unwrap_or(""),
            self.hash.as_deref().unwrap_or(""),
            self.locale
        )
    }

    /// The fields of `AgentRun`.
    pub fn of_run(row: &moqui_core::entity::EntityValue) -> Option<Self> {
        let source = text_of(row, "constitutionSource")?;
        Some(Self {
            source,
            organization_id: text_of(row, "constitutionOrgId"),
            hash: text_of(row, "constitutionHash"),
            locale: text_of(row, "localeCode").unwrap_or_else(|| "en".to_string()),
        })
    }

    /// Set the fields of `AgentRun`.
    pub fn set_on(&self, row: &mut moqui_core::entity::EntityValue) {
        row.set("constitutionSource", MValue::string(&self.source));
        if let Some(org) = &self.organization_id {
            row.set("constitutionOrgId", MValue::string(org));
        }
        if let Some(hash) = &self.hash {
            row.set("constitutionHash", MValue::string(hash));
        }
        row.set("localeCode", MValue::string(&self.locale));
    }
}

/// The scope of a new run: a job (or a run as the agent's own user) uses the global document; the other surfaces, the
/// requesting user's active organization.
pub fn scope_for(surface: Surface, run_as_agent_user: bool, user_id: &str) -> Scope<'_> {
    if surface == Surface::Job || run_as_agent_user {
        Scope::Global
    } else {
        Scope::ActiveOf(user_id)
    }
}

/// The Constitution of a run starting now for `locale_user_id`'s locale (normalized; the document's default when the
/// user has none): resolved, its snapshot kept. One that cannot be resolved is `unavailable`, and the run goes on
/// without the section.
pub fn pick(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    scope: Scope<'_>,
    locale_user_id: &str,
) -> (RunConstitution, Option<Resolved>) {
    let user_locale = user_locale(facade, locale_user_id);
    match resolve::resolve(factory, scope) {
        Ok(resolved) => {
            let locale = user_locale.unwrap_or_else(|| document::default_locale(&resolved.document));
            keep(facade, &resolved.hash, &resolved.document);
            let picked = RunConstitution {
                source: resolved.source.as_str().to_string(),
                organization_id: resolved.organization_id.clone(),
                hash: Some(resolved.hash.clone()),
                locale,
            };
            (picked, Some(resolved))
        }
        Err(diagnostics) => {
            tracing::error!("an agent run starts without a Constitution: {}", diagnostics.join(" "));
            let picked = RunConstitution {
                source: "unavailable".to_string(),
                organization_id: None,
                hash: None,
                locale: user_locale.unwrap_or_else(|| "en".to_string()),
            };
            (picked, None)
        }
    }
}

/// The normalized locale of a user's account, if set.
pub fn user_locale(facade: &EntityFacade, user_id: &str) -> Option<String> {
    let row = crate::run::quietly(|| {
        facade
            .find("moqui.security.UserAccount")
            .and_then(|find| find.eq("userId", MValue::string(user_id)).disable_authz().one())
    });
    row.ok()
        .flatten()
        .and_then(|row| text_of(&row, "locale"))
        .and_then(|tag| locale::normalize(&tag))
}

/// Store `document` under `hash` when it is not stored yet, in a transaction of its own; a failure is logged (the run
/// still has the hash, and its references are then unavailable).
pub fn keep(facade: &EntityFacade, hash: &str, document: &Value) {
    let outcome = crate::run::quietly(|| {
        facade.transactions_for(SNAPSHOT)?.run(TransactionAttribute::ForceNew, None, || {
            let found = facade.find(SNAPSHOT)?.eq("contentHash", MValue::string(hash)).disable_authz().one()?;
            if found.is_some() {
                return Ok(());
            }
            let mut row = facade.make_value(SNAPSHOT)?;
            row.set("contentHash", MValue::string(hash));
            row.set("documentJson", MValue::string(document::to_pretty(document)));
            row.set("createdDate", crate::run::stamp(jiff::Timestamp::now().as_millisecond()));
            row.create(facade)
        })
    });
    if let Err(error) = outcome {
        tracing::error!("the Constitution snapshot {hash} could not be stored: {error}");
    }
}

/// The stored document of `hash`.
///
/// # Errors
///
/// Returns an error when the row cannot be read.
pub fn load(facade: &EntityFacade, hash: &str) -> Result<Option<Arc<Value>>, Error> {
    static CACHE: OnceLock<Mutex<BTreeMap<String, Arc<Value>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    if let Some(found) = cache.lock().ok().and_then(|cache| cache.get(hash).cloned()) {
        return Ok(Some(found));
    }
    let row = facade.find(SNAPSHOT)?.eq("contentHash", MValue::string(hash)).disable_authz().one()?;
    let Some(text) = row.and_then(|row| text_of(&row, "documentJson")) else {
        return Ok(None);
    };
    let document = Arc::new(serde_json::from_str::<Value>(&text).map_err(|error| Error::new(error.to_string()))?);
    if let Ok(mut cache) = cache.lock() {
        if cache.len() > 64 {
            cache.clear();
        }
        cache.insert(hash.to_string(), Arc::clone(&document));
    }
    Ok(Some(document))
}

/// The prompt section of a run's Constitution, from its snapshot; `None` when it has none or it cannot be read.
pub fn section(facade: &EntityFacade, picked: &RunConstitution) -> Option<String> {
    let hash = picked.hash.as_deref()?;
    match load(facade, hash) {
        Ok(Some(document)) => Some(render::prompt_section(&document, &picked.locale).text),
        Ok(None) => {
            tracing::error!("the Constitution snapshot {hash} is missing, so the prompt has no Constitution");
            None
        }
        Err(error) => {
            tracing::error!("the Constitution snapshot {hash} could not be read: {error}");
            None
        }
    }
}
