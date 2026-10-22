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

//! User facts (plans/LLM-A2A-Assist.md 7.6): what the prefix of a new conversation says about its user. The framework
//! gives the user's name, username, id, locale and time zone ([`crate::llm::prompt::user_facts`]); the date and time
//! are not facts but the `now` line of each message's tail, since the prefix is frozen for the conversation. Components
//! add their own, such as the active organization and the organization's instructions, through a registration point
//! that keeps the framework free of their names:
//!
//! - a Rust provider ([`UserFactsProvider`]) submitted with [`submit_user_facts!`](crate::submit_user_facts), as tools
//!   are, from any linked crate;
//! - a service named by a `user-facts` element of the `llm-facade` conf (a component's `MoquiConf.xml` adds it), an
//!   implementation of the interface `org.moqui.agent.AgentServices.get#UserFacts`: in `userId` and `agentId`, out
//!   `factList` (maps of `label` and `value`) and `instructions` (the organization's instructions).
//!
//! Each provider runs in the context the prefix is built in (the user's, with the run's bookkeeping reads), under the
//! read fence; one that fails or tries to write is logged and left out, and the
//! others go on. A label already given keeps its first value; the section's size cap is the prompt's
//! ([`crate::llm::prompt::caps::USER_FACTS`]).

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};

use crate::context::ExecutionContextFactory;
use crate::entity::{fence, EntityFacade};

pub use inventory::submit;

/// The interface every `user-facts` service implements.
pub const FACTS_SERVICE: &str = "org.moqui.agent.AgentServices.get#UserFacts";

/// What the prefix says about a user.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserFacts {
    /// `(label, value)` lines, in the order given.
    pub facts: Vec<(String, String)>,
    /// The organization's instructions (plan 7.6: keyed by the user's active internal organization).
    pub org_instructions: String,
}

impl UserFacts {
    /// The value of a fact.
    pub fn get(&self, label: &str) -> Option<&str> {
        self.facts
            .iter()
            .find(|(name, _)| name == label)
            .map(|(_, value)| value.as_str())
    }

    /// Add what `other` says: a label not given yet, and instructions when there are none yet.
    fn add(&mut self, other: UserFacts) {
        for (label, value) in other.facts {
            let (label, value) = (label.trim().to_string(), value.trim().to_string());
            if label.is_empty() || value.is_empty() || self.get(&label).is_some() {
                continue;
            }
            self.facts.push((label, value));
        }
        if self.org_instructions.trim().is_empty() {
            self.org_instructions = other.org_instructions.trim().to_string();
        }
    }
}

/// A source of facts about users, registered with [`submit_user_facts!`](crate::submit_user_facts).
pub trait UserFactsProvider: Send + Sync {
    /// Its name, in the log.
    fn name(&self) -> &str;

    /// The facts about `user_id` for a conversation with `agent_id`; none is an empty [`UserFacts`].
    ///
    /// # Errors
    ///
    /// Returns an error when they cannot be read; the provider is then left out.
    fn facts(&self, facade: &EntityFacade, user_id: &str, agent_id: &str) -> Result<UserFacts, Error>;
}

/// A Rust provider, submitted with [`submit_user_facts!`](crate::submit_user_facts).
pub struct UserFactsRegistration {
    pub make: fn() -> Arc<dyn UserFactsProvider>,
}

inventory::collect!(UserFactsRegistration);

/// The `make` of a provider that is `Default`.
pub fn make_default<T: UserFactsProvider + Default + 'static>() -> Arc<dyn UserFactsProvider> {
    Arc::new(T::default())
}

/// Register a user-facts provider type (`UserFactsProvider + Default`) from any linked crate:
///
/// ```ignore
/// moqui_core::submit_user_facts!(ActiveOrganizationFacts);
/// ```
#[macro_export]
macro_rules! submit_user_facts {
    ($provider:ty) => {
        $crate::llm::facts::submit! {
            $crate::llm::facts::UserFactsRegistration { make: $crate::llm::facts::make_default::<$provider> }
        }
    };
}

/// Every registered Rust provider, sorted by name.
pub fn providers() -> &'static [Arc<dyn UserFactsProvider>] {
    static PROVIDERS: OnceLock<Vec<Arc<dyn UserFactsProvider>>> = OnceLock::new();
    PROVIDERS.get_or_init(|| {
        let mut providers: Vec<Arc<dyn UserFactsProvider>> = inventory::iter::<UserFactsRegistration>
            .into_iter()
            .map(|registration| (registration.make)())
            .collect();
        providers.sort_by(|left, right| left.name().cmp(right.name()));
        providers
    })
}

/// The facts about `user_id` for a new conversation with `agent_id`: the framework's, then each Rust provider's, then
/// each conf service's, each run fenced; one that fails is logged and left out.
pub fn collect(factory: &ExecutionContextFactory, facade: &EntityFacade, user_id: &str, agent_id: &str) -> UserFacts {
    let mut out = UserFacts {
        facts: super::prompt::user_facts(facade, user_id).unwrap_or_else(|error| {
            tracing::error!("the user facts of {user_id} could not be read: {error}");
            Vec::new()
        }),
        org_instructions: String::new(),
    };
    for provider in providers() {
        let label = format!("user facts {}", provider.name());
        if let Some(facts) = fenced(&label, || provider.facts(facade, user_id, agent_id)) {
            out.add(facts);
        }
    }
    let services = super::facade(factory).conf().user_fact_services.clone();
    for service in services {
        let label = format!("user facts {service}");
        if let Some(facts) = fenced(&label, || from_service(&service, user_id, agent_id)) {
            out.add(facts);
        }
    }
    out
}

/// `body` under the read fence: its facts, or `None` (logged) when it failed or tried to write.
fn fenced(label: &str, body: impl FnOnce() -> Result<UserFacts, Error>) -> Option<UserFacts> {
    let guard = fence::enter(label.to_string());
    let outcome = body();
    let violations = guard.violations();
    drop(guard);
    if !violations.is_empty() {
        let tried: Vec<String> = violations.iter().map(fence::Violation::describe).collect();
        tracing::error!("{label} tried {}, so its facts are left out", tried.join(", "));
        return None;
    }
    match outcome {
        Ok(facts) => Some(facts),
        Err(error) => {
            tracing::error!("{label} failed, so its facts are left out: {error}");
            None
        }
    }
}

/// The facts a `user-facts` service gives: its `factList` and `instructions`.
fn from_service(service: &str, user_id: &str, agent_id: &str) -> Result<UserFacts, Error> {
    let mut parameters = BTreeMap::new();
    parameters.insert("userId".to_string(), Value::string(user_id));
    parameters.insert("agentId".to_string(), Value::string(agent_id));
    let result =
        crate::service::call::sync_checked(service, &parameters, &crate::service::call::CallOptions::default())
            .map_err(|error| Error::new(error.message().to_string()))?;
    let mut facts = Vec::new();
    if let Some(Value::List(list)) = result.get("factList") {
        for item in list {
            if let Value::Map(map) = item {
                let text = |key: &str| map.get(key).map(to_plain_string).unwrap_or_default();
                facts.push((text("label"), text("value")));
            }
        }
    }
    Ok(UserFacts {
        facts,
        org_instructions: result
            .get("instructions")
            .filter(|value| !value.is_null())
            .map(to_plain_string)
            .unwrap_or_default(),
    })
}
