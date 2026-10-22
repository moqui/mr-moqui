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

//! Token estimates (plans/LLM-A2A-Assist.md 7.8). There is no tokenizer: a text is its characters divided by `k`, the
//! characters per token, which starts at [`DEFAULT_CHARS_PER_TOKEN`] (about [`CJK_CHARS_PER_TOKEN`] for CJK text) and is
//! calibrated per model from the prompt tokens the provider reports ([`observe`]). A model's `chars-per-token` (conf or
//! `LlmModel`) is where its calibration starts.
//!
//! The window ([`Window`]) is the model's context less the output, a reasoning reserve and a 3% margin; compaction
//! ([`super::window`]) works between its high and low watermarks. Counting a request is the last reported prompt tokens
//! plus the characters added since, divided by `k` ([`Estimator::since`]), so the estimate is only ever off by the new
//! part. The characters of a request ([`RequestChars::sent`]) are what is sent: message text, tool-call arguments and
//! names, the reasoning replayed to the model, the tools, the system text and the tail.

use std::collections::HashMap;
use std::sync::Mutex;

use moqui_lib::sync::lock;

use super::conf::ModelConf;
use super::provider::{ModelRequest, ReplayBag, ReplayItem, Role};
use super::registry::Target;

/// Characters per token of a model with no calibration yet.
pub const DEFAULT_CHARS_PER_TOKEN: f64 = 3.6;
/// Characters per token of CJK text.
pub const CJK_CHARS_PER_TOKEN: f64 = 1.5;
/// The weight of one observation in the running calibration.
const CALIBRATION_WEIGHT: f64 = 0.3;
/// The range a calibration stays in, so one odd report cannot run away with it.
const MIN_CHARS_PER_TOKEN: f64 = 1.0;
const MAX_CHARS_PER_TOKEN: f64 = 8.0;
/// A report on fewer characters than this says little about the ratio and is not used.
const MIN_OBSERVED_CHARS: usize = 400;

/// Whether `ch` is CJK: ideographs, kana, hangul and their punctuation and full-width forms.
fn is_cjk(ch: char) -> bool {
    matches!(u32::from(ch),
        0x3000..=0x303F | 0x3040..=0x30FF | 0x3100..=0x312F | 0x3130..=0x318F | 0x31F0..=0x31FF
        | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF
        | 0x20000..=0x2FA1F)
}

/// Characters per token for `text` under a calibration `k`: `k` for text with no CJK, [`CJK_CHARS_PER_TOKEN`] for
/// text that is all CJK, in proportion between.
pub fn chars_per_token(text: &str, k: f64) -> f64 {
    let mut total = 0_usize;
    let mut cjk = 0_usize;
    for ch in text.chars() {
        total += 1;
        if is_cjk(ch) {
            cjk += 1;
        }
    }
    if total == 0 || cjk == 0 {
        return k;
    }
    let share = cjk as f64 / total as f64;
    k * (1.0 - share) + CJK_CHARS_PER_TOKEN.min(k) * share
}

/// A token estimate for one model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Estimator {
    /// Characters per token of plain text.
    pub k: f64,
}

impl Default for Estimator {
    fn default() -> Self {
        Self {
            k: DEFAULT_CHARS_PER_TOKEN,
        }
    }
}

impl Estimator {
    /// The model's calibration in this process, else its `chars-per-token`, else the default.
    pub fn for_model(model: &ModelConf) -> Self {
        let start = model
            .chars_per_token
            .filter(|k| k.is_finite() && *k > 0.0)
            .unwrap_or(DEFAULT_CHARS_PER_TOKEN);
        let k = lock(calibrations()).get(&model.id).map_or(start, |calibration| calibration.k);
        Self { k }
    }

    /// The tokens of `text`, rounded up.
    pub fn tokens(&self, text: &str) -> u64 {
        let chars = text.chars().count();
        if chars == 0 {
            return 0;
        }
        (chars as f64 / chars_per_token(text, self.k)).ceil() as u64
    }

    /// The tokens of `chars` characters of plain text.
    pub fn tokens_of_chars(&self, chars: usize) -> u64 {
        (chars as f64 / self.k).ceil() as u64
    }

    /// The request's tokens when the provider last reported `reported_tokens` for `reported_chars` of it and it has
    /// `chars` now: the report plus the new characters' estimate. With no report, all of it is estimated.
    pub fn since(&self, reported: Option<(u64, usize)>, chars: usize) -> u64 {
        match reported {
            Some((tokens, at)) if chars >= at => tokens + self.tokens_of_chars(chars - at),
            _ => self.tokens_of_chars(chars),
        }
    }
}

/// A model's running calibration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Calibration {
    pub k: f64,
    pub observations: u32,
}

impl Calibration {
    pub fn new(k: f64) -> Self {
        Self { k, observations: 0 }
    }

    /// Fold in a report of `tokens` prompt tokens for `chars` characters (the first replaces the start, later ones move
    /// it by a weight). A report on too little text, or one with no tokens, is ignored. Out: whether it was used.
    pub fn observe(&mut self, chars: usize, tokens: u64) -> bool {
        if chars < MIN_OBSERVED_CHARS || tokens == 0 {
            return false;
        }
        let seen = (chars as f64 / tokens as f64).clamp(MIN_CHARS_PER_TOKEN, MAX_CHARS_PER_TOKEN);
        self.k = if self.observations == 0 {
            seen
        } else {
            self.k * (1.0 - CALIBRATION_WEIGHT) + seen * CALIBRATION_WEIGHT
        };
        self.observations = self.observations.saturating_add(1);
        true
    }
}

fn calibrations() -> &'static Mutex<HashMap<String, Calibration>> {
    static CALIBRATIONS: std::sync::OnceLock<Mutex<HashMap<String, Calibration>>> = std::sync::OnceLock::new();
    CALIBRATIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The calibration hook: the provider reported `prompt_tokens` for a request of `chars` characters to `model`. The
/// step loop calls it with each reply's usage; the calibration is per process and starts from the model's
/// `chars-per-token`. Text that is mostly CJK is left out, since its ratio is not the model's for plain text.
pub fn observe(model: &ModelConf, chars: usize, prompt_tokens: u64, cjk_share: f64) {
    if cjk_share > 0.2 {
        return;
    }
    let start = Estimator::for_model(model).k;
    let mut map = lock(calibrations());
    let calibration = map.entry(model.id.clone()).or_insert_with(|| Calibration::new(start));
    calibration.observe(chars, prompt_tokens);
}

/// Forget the calibration of `model_id` (an admin changed its `chars-per-token`), or of every model.
pub fn forget(model_id: Option<&str>) {
    let mut map = lock(calibrations());
    match model_id {
        Some(id) => {
            map.remove(id);
        }
        None => map.clear(),
    }
}

/// The share of CJK characters in `text`.
pub fn cjk_share(text: &str) -> f64 {
    let (total, cjk) = text
        .chars()
        .fold((0_usize, 0_usize), |(total, cjk), ch| (total + 1, cjk + usize::from(is_cjk(ch))));
    if total == 0 {
        0.0
    } else {
        cjk as f64 / total as f64
    }
}

/// The characters of a request by part, as sent: what the estimates count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RequestChars {
    pub system: usize,
    /// The tools as their JSON (name, description, parameters schema).
    pub tools: usize,
    /// Message text, tool-call arguments and the tool names, without the tail.
    pub history: usize,
    pub tail: usize,
    /// The reasoning sent back to the model with its messages (the replay bags `reasoning-replay` sends).
    pub reasoning: usize,
}

impl RequestChars {
    /// The characters of `request` as sent to `target`: [`Self::of`] with the replay bags the model gets back.
    pub fn sent(request: &ModelRequest, target: &Target) -> Self {
        let mut chars = Self::of(request);
        chars.reasoning = request.replays(target).into_iter().flatten().map(replay_chars).sum();
        chars
    }

    /// The characters of `request` without its replayed reasoning (which depends on the model it goes to).
    pub fn of(request: &ModelRequest) -> Self {
        let system = request.system_text().chars().count();
        let tools = request.tools.iter().map(tool_chars).sum();
        let history = request
            .history
            .iter()
            .map(|message| {
                let calls: usize = message
                    .tool_calls
                    .iter()
                    .map(|call| call.name.chars().count() + call.arguments.to_string().chars().count())
                    .sum();
                let name = if message.role == Role::Tool {
                    message.tool_name.as_deref().map_or(0, |name| name.chars().count())
                } else {
                    0
                };
                message.text.chars().count() + calls + name
            })
            .sum();
        let tail = request.tail.as_deref().map_or(0, |tail| tail.chars().count());
        Self {
            system,
            tools,
            history,
            tail,
            reasoning: 0,
        }
    }

    pub fn total(&self) -> usize {
        self.system + self.tools + self.history + self.tail + self.reasoning
    }
}

/// The characters of a replay bag as it goes back: thinking text and signatures, encrypted items, echoed reasoning.
pub fn replay_chars(bag: &ReplayBag) -> usize {
    bag.items
        .iter()
        .map(|item| match item {
            ReplayItem::Thinking { thinking, signature } => thinking.chars().count() + signature.chars().count(),
            ReplayItem::RedactedThinking { data } => data.chars().count(),
            ReplayItem::ResponsesReasoning { item } => item.to_string().chars().count(),
            ReplayItem::ReasoningContent { text } => text.chars().count(),
            ReplayItem::ToolCallExtra { fields, .. } | ReplayItem::MessageExtra { fields } => {
                serde_json::Value::Object(fields.clone()).to_string().chars().count()
            }
            ReplayItem::Fake { token } => token.chars().count(),
        })
        .sum()
}

/// The characters of a tool as sent: its name, description and parameters schema, as compact JSON.
pub fn tool_chars(tool: &super::provider::ToolSpec) -> usize {
    tool.name.chars().count() + tool.description.chars().count() + tool.schema().to_string().chars().count()
}

/// The token window of a model (7.8): `budget = context − max output − reasoning reserve − 3%`; compaction starts above
/// `high` and works down to `low`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub context: u64,
    pub budget: u64,
    pub high: u64,
    pub low: u64,
}

/// The reasoning reserve of a model whose reasoning is on, in tokens, when it gives none.
pub const REASONING_RESERVE: u64 = 1024;
/// The context window of a model that does not say: what a small local server often loads.
pub const DEFAULT_CONTEXT_WINDOW: u64 = 32_768;

impl Window {
    /// The window of `model`, with `reasoning` when its reasoning is on: its `context-window` (or what the server
    /// reported, passed as `served_context`), its watermarks or the defaults 85% and 55%.
    pub fn for_model(model: &ModelConf, served_context: Option<u64>, reasoning: bool) -> Self {
        let context = served_context
            .or(model.context_window.map(u64::from))
            .unwrap_or(DEFAULT_CONTEXT_WINDOW);
        let output = model.max_output.map_or(4096, u64::from);
        let reserve = if reasoning { REASONING_RESERVE } else { 0 };
        let budget = context.saturating_sub(output).saturating_sub(reserve) * 97 / 100;
        let high = u64::from(model.behavior.compact_high_percent.unwrap_or(85).min(100));
        let low = u64::from(model.behavior.compact_low_percent.unwrap_or(55).min(100));
        Self {
            context,
            budget,
            high: budget * high / 100,
            low: budget * low / 100,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::conf::ModelConf;

    #[test]
    fn an_estimate_divides_characters_and_counts_cjk_denser() {
        let estimator = Estimator::default();
        assert_eq!(estimator.tokens(""), 0);
        assert_eq!(estimator.tokens(&"a".repeat(36)), 10);
        let cjk = "数据".repeat(15);
        assert_eq!(estimator.tokens(&cjk), 20);
        let half = format!("{}{}", "a".repeat(30), "数".repeat(30));
        let tokens = estimator.tokens(&half);
        assert!(tokens > 9 && tokens < 40, "{tokens}");
        // the last report plus the new part
        assert_eq!(estimator.since(Some((1000, 4000)), 4360), 1100);
        assert_eq!(estimator.since(None, 360), 100);
    }

    #[test]
    fn a_calibration_starts_from_the_first_report_and_moves_by_a_weight() {
        let mut calibration = Calibration::new(DEFAULT_CHARS_PER_TOKEN);
        assert!(!calibration.observe(100, 50), "too little text");
        assert!(calibration.observe(4000, 1000));
        assert!((calibration.k - 4.0).abs() < 1e-9);
        assert!(calibration.observe(4000, 2000));
        assert!((calibration.k - (4.0 * 0.7 + 2.0 * 0.3)).abs() < 1e-9);
        assert!(calibration.observe(100_000, 1));
        assert!(calibration.k <= MAX_CHARS_PER_TOKEN);

        let mut model = ModelConf::new("tokens-calibrated", "local", "m");
        model.chars_per_token = Some(3.0);
        forget(Some("tokens-calibrated"));
        assert!((Estimator::for_model(&model).k - 3.0).abs() < 1e-9);
        observe(&model, 5000, 1000, 0.0);
        assert!((Estimator::for_model(&model).k - 5.0).abs() < 1e-9);
        observe(&model, 5000, 5000, 0.9);
        assert!((Estimator::for_model(&model).k - 5.0).abs() < 1e-9, "a CJK report is left out");
        forget(Some("tokens-calibrated"));
    }

    #[test]
    fn the_window_leaves_room_for_output_reasoning_and_a_margin() {
        let mut model = ModelConf::new("tokens-window", "local", "m");
        model.context_window = Some(32_768);
        model.max_output = Some(6144);
        let window = Window::for_model(&model, None, true);
        assert_eq!(window.budget, (32_768 - 6144 - REASONING_RESERVE) * 97 / 100);
        assert_eq!(window.high, window.budget * 85 / 100);
        assert_eq!(window.low, window.budget * 55 / 100);
        assert_eq!(Window::for_model(&model, Some(16_384), false).context, 16_384);
    }
}
