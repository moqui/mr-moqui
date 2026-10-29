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

//! Where the framework reaches the agent runtime (crates/moqui-llm, plans/LLM-A2A-Assist.md). That crate sits on top
//! of this one and submits one [`LlmHooks`]. A host built without it has none, and each place below acts as a build
//! without the agents: its services report the feature missing, `ec.llm` is not there, and a skill file is not
//! synced.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use moqui_lib::error::Error as LibError;

use crate::context::{Error, ExecutionContextFactory};
use crate::entity::EntityFacade;
use crate::screen::overlay::Overlay;
pub use crate::service::rust_services::RustServiceFn;

/// Submit the agent runtime's [`LlmHooks`]; moqui-llm does, once.
pub use inventory::submit;

/// What the framework calls in the agent runtime.
pub struct LlmHooks {
    /// Add the runtime's Rust services (`moqui_llm::…` locations) to the registry.
    pub register_services: fn(&mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String>,
    /// The overlay of a `canvas:<artifactId>` preview token (crate::screen::overlay::for_token).
    pub canvas_overlay: fn(&Arc<ExecutionContextFactory>, &str) -> Option<Overlay>,
    /// Sync the skill files into the database, on a dev instance's reload (crate::devtools::reload).
    pub sync_skills: fn(&ExecutionContextFactory, &EntityFacade) -> Result<SkillsSynced, LibError>,
    /// The `ec.llm` object of a script context.
    pub script_object: for<'js> fn(&rquickjs::Ctx<'js>) -> Result<rquickjs::Object<'js>, Error>,
}
inventory::collect!(LlmHooks);

/// The counts of a skill sync, and one line for each file left out or row not written.
pub struct SkillsSynced {
    pub added: usize,
    pub changed: usize,
    pub retired: usize,
    pub problems: Vec<String>,
}

/// The agent runtime's hooks, when this binary links it.
#[must_use]
pub fn get() -> Option<&'static LlmHooks> {
    static HOOKS: OnceLock<Option<&'static LlmHooks>> = OnceLock::new();
    *HOOKS.get_or_init(|| {
        let mut entries = inventory::iter::<LlmHooks>.into_iter();
        let first = entries.next();
        if entries.next().is_some() {
            tracing::error!("more than one agent runtime submitted LlmHooks; using the first");
        }
        first
    })
}

/// Whether this binary links the agent runtime.
#[must_use]
pub fn linked() -> bool {
    get().is_some()
}
