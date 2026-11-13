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
//! without the agents: its services report the feature missing, `ec.llm` is not there, a skill file is not synced,
//! and speech to text ([`transcribe`], for call captions, voicemail and recordings) reports `notBuiltIn`.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use moqui_lib::error::Error as LibError;

use crate::context::ExecutionContextFactory;
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
    /// The `ec.llm` paths of the host table (`crate::script::host`), bound on every script engine.
    pub script_paths: &'static [crate::script::host::Path],
    /// Speech to text through the model bound to the `transcribe` feature ([`transcribe`]).
    pub transcribe: fn(&ExecutionContextFactory, &SpeechRequest<'_>) -> Result<String, SpeechError>,
    /// The provider [`transcribe`] would use now ([`speech_provider`]).
    pub speech_provider: fn(&ExecutionContextFactory) -> Result<SpeechProvider, SpeechError>,
}
inventory::collect!(LlmHooks);

/// The counts of a skill sync, and one line for each file left out or row not written.
pub struct SkillsSynced {
    pub added: usize,
    pub changed: usize,
    pub retired: usize,
    pub problems: Vec<String>,
}

/// Audio to transcribe, for [`transcribe`]: the bytes of one file (WAV, Ogg, WebM, MP3 and the like) with its media
/// type, and what the caller knows of it. Live captions (plans/archive/Comms-Framework.md §6.4), voicemail and recordings use
/// this one seam.
#[derive(Debug, Clone, Default)]
pub struct SpeechRequest<'a> {
    pub audio: &'a [u8],
    /// `audio/wav`, `audio/ogg`, `audio/webm`, ...
    pub media_type: &'a str,
    /// An ISO 639-1 code; `None` lets the model tell.
    pub language: Option<&'a str>,
    /// Words to expect (names, terms), passed to the model as its prompt.
    pub prompt: Option<&'a str>,
    /// The user the call counts against (quotas, the `LlmCall` row): for a server job, the user who turned it on.
    pub user_id: Option<&'a str>,
    /// What asked, kept on the `LlmCall` row as its client (`captions:<callSessionId>`, `voicemail:<id>`).
    pub purpose: &'a str,
    /// The longest the provider may take; `None` is the provider's own limit.
    pub timeout: Option<std::time::Duration>,
}

/// Why audio was not transcribed. `code` is one of `notBuiltIn` (no agent runtime in this binary), `notConfigured`
/// (no model bound to `transcribe`, or the runtime is off), `invalid` (the audio), `quota`, `failed` (the provider) and
/// `timeout`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechError {
    pub code: &'static str,
    pub message: String,
}

impl SpeechError {
    #[must_use]
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        SpeechError {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SpeechError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

/// Who transcribes: what a consent notice names (`label`: the endpoint and model), and whether the audio stays on
/// this deployment's own network (`local`: an endpoint marked local).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechProvider {
    pub label: String,
    pub local: bool,
}

/// Speech to text through the agent runtime: the model bound to the `transcribe` feature on an OpenAI-compatible
/// endpoint (`/v1/audio/transcriptions`), with the user's quota checked and the call kept as an `LlmCall`.
///
/// # Errors
///
/// `notBuiltIn` in a binary without the agent runtime (the `llm` feature); the others as [`SpeechError`] says.
pub fn transcribe(factory: &ExecutionContextFactory, request: &SpeechRequest<'_>) -> Result<String, SpeechError> {
    match get() {
        Some(hooks) => (hooks.transcribe)(factory, request),
        None => Err(not_built_in()),
    }
}

/// The provider [`transcribe`] would use now, for a policy check and a notice before any audio goes out.
///
/// # Errors
///
/// As [`transcribe`], without calling the provider.
pub fn speech_provider(factory: &ExecutionContextFactory) -> Result<SpeechProvider, SpeechError> {
    match get() {
        Some(hooks) => (hooks.speech_provider)(factory),
        None => Err(not_built_in()),
    }
}

fn not_built_in() -> SpeechError {
    SpeechError::new(
        "notBuiltIn",
        "speech to text is not built in: this binary has no agent runtime (the llm feature)",
    )
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
