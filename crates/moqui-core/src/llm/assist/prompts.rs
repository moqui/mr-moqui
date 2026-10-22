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

//! Suggested prompts of the drawer (plan 4.11): the chips in the empty state and above the composer, for the page the
//! user is on. They come from the admin's `AgentSuggestedPrompt` rows (for an agent or all, a screen path pattern, an
//! entity) and from providers that components register, so the framework names none of their screens:
//!
//! - a Rust provider ([`PromptProvider`]) submitted with [`submit_assist_prompts!`](crate::submit_assist_prompts),
//!   from any linked crate, as user facts and tools are.
//!
//! - an app's `<shell-providers><assist-prompt label text send entity screen>` element ([`shell_providers`], read from
//!   the shell's providers of the render the call runs in: the apps the user may open, a screen they may view);
//!
//! and the same `<shell-providers>` carry the response actions of the answers (`<response-action label icon screen
//! text-parameter requires-record>`, plan 4.11), so the framework names none of the apps' screens.
//!
//! Each provider runs under the read fence; one that fails or tries to write is logged and left out.

use std::sync::{Arc, OnceLock};

use moqui_lib::error::Error;

use crate::entity::{fence, EntityFacade};

pub use inventory::submit;

/// What the page is: whose, for which agent, the screen path (`/apps/…`) and the record it is on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PromptQuery {
    pub user_id: String,
    pub agent_id: String,
    pub path: Option<String>,
    pub entity_name: Option<String>,
    pub record_id: Option<String>,
}

/// One suggested prompt: its chip label, the text it sends or puts in the composer, and whether it sends at once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SuggestedPrompt {
    pub label: String,
    pub text: String,
    pub send: bool,
}

/// A source of suggested prompts, registered with [`submit_assist_prompts!`](crate::submit_assist_prompts).
pub trait PromptProvider: Send + Sync {
    /// Its name, in the log and as the prompts' `source`.
    fn name(&self) -> &str;

    /// The prompts for the page; none is an empty list.
    ///
    /// # Errors
    ///
    /// Returns an error when they cannot be made; the provider is then left out.
    fn prompts(&self, facade: &EntityFacade, query: &PromptQuery) -> Result<Vec<SuggestedPrompt>, Error>;
}

/// A Rust provider, submitted with [`submit_assist_prompts!`](crate::submit_assist_prompts).
pub struct PromptRegistration {
    pub make: fn() -> Arc<dyn PromptProvider>,
}

inventory::collect!(PromptRegistration);

/// The `make` of a provider that is `Default`.
pub fn make_default<T: PromptProvider + Default + 'static>() -> Arc<dyn PromptProvider> {
    Arc::new(T::default())
}

/// Register a suggested-prompt provider type (`PromptProvider + Default`) from any linked crate:
///
/// ```ignore
/// moqui_core::submit_assist_prompts!(OrderPrompts);
/// ```
#[macro_export]
macro_rules! submit_assist_prompts {
    ($provider:ty) => {
        $crate::llm::assist::prompts::submit! {
            $crate::llm::assist::prompts::PromptRegistration {
                make: $crate::llm::assist::prompts::make_default::<$provider>,
            }
        }
    };
}

/// Every registered provider, sorted by name.
pub fn providers() -> &'static [Arc<dyn PromptProvider>] {
    static PROVIDERS: OnceLock<Vec<Arc<dyn PromptProvider>>> = OnceLock::new();
    PROVIDERS.get_or_init(|| {
        let mut providers: Vec<Arc<dyn PromptProvider>> = inventory::iter::<PromptRegistration>
            .into_iter()
            .map(|registration| (registration.make)())
            .collect();
        providers.sort_by(|left, right| left.name().cmp(right.name()));
        providers
    })
}

/// The prompts of every provider for `query`, each with its provider's name; one that fails or tries to write is
/// logged and left out.
pub fn from_providers(facade: &EntityFacade, query: &PromptQuery) -> Vec<(String, SuggestedPrompt)> {
    let mut out = Vec::new();
    for provider in providers() {
        let label = format!("suggested prompts {}", provider.name());
        let guard = fence::enter(label.clone());
        let outcome = provider.prompts(facade, query);
        let violations = guard.violations();
        drop(guard);
        if !violations.is_empty() {
            let tried: Vec<String> = violations.iter().map(fence::Violation::describe).collect();
            tracing::error!("{label} tried {}, so its prompts are left out", tried.join(", "));
            continue;
        }
        match outcome {
            Ok(prompts) => out.extend(
                prompts
                    .into_iter()
                    .filter(|prompt| !prompt.label.trim().is_empty() && !prompt.text.trim().is_empty())
                    .map(|prompt| (provider.name().to_string(), prompt)),
            ),
            Err(error) => tracing::error!("{label} failed, so its prompts are left out: {error}"),
        }
    }
    out
}

/// The apps' Assist providers for the user: `(app name, entry)` of their `assistPrompts` and `responseActions` as the
/// shell's providers give them (localized, with a `/qapps` url each).
#[derive(Clone, Debug, Default)]
pub struct ShellAssist {
    pub prompts: Vec<(String, serde_json::Value)>,
    pub actions: Vec<(String, serde_json::Value)>,
}

/// [`ShellAssist`] of the render in progress (the drawer's calls run in one); empty outside a render or when the
/// providers cannot be read (logged).
pub fn shell_providers() -> ShellAssist {
    let read = crate::screen::with_current_render(|state| crate::screen::shell::shell_providers(state, None));
    let value = match read {
        Some(Ok(value)) => moqui_lib::json::to_json(&value),
        Some(Err(error)) => {
            tracing::error!("the apps' Assist providers could not be read: {error}");
            return ShellAssist::default();
        }
        None => return ShellAssist::default(),
    };
    let mut out = ShellAssist::default();
    for app in value.get("apps").and_then(serde_json::Value::as_array).into_iter().flatten() {
        let name = app.get("name").and_then(serde_json::Value::as_str).unwrap_or_default().to_string();
        let providers = app.get("providers");
        let list = |key: &str| {
            providers
                .and_then(|providers| providers.get(key))
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        out.prompts.extend(list("assistPrompts").into_iter().map(|prompt| (name.clone(), prompt)));
        out.actions.extend(list("responseActions").into_iter().map(|action| (name.clone(), action)));
    }
    out
}
