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

//! The measures of plan 9: per task run (model and tool calls, wrong calls, repairs, invented references, writes held
//! against the expectation, prompt, completion and cached tokens, the prefix size, compactions, wall time, replay
//! drift), per task over its runs (pass@1, and whether every run passed), and for the suite (pass@1, pass^k, the
//! totals and the cache-hit ratio). A baseline (`test/agent/baseline-<profile>.json`) keeps each task's success and
//! tokens; a run with lower success, or 20% more tokens, than the baseline is a regression.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};

/// Tokens past the baseline by this share are a regression.
pub const TOKEN_GROWTH: f64 = 0.2;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TaskMetrics {
    pub model_calls: u64,
    pub tool_calls: u64,
    pub wrong_calls: u64,
    pub repairs: u64,
    pub invented: u64,
    pub held: u64,
    pub held_expected: Option<u64>,
    pub prompt_tokens: u64,
    pub cached_tokens: u64,
    pub completion_tokens: u64,
    pub prefix_chars: u64,
    pub compactions: u64,
    pub wall_ms: u64,
    /// Tool results of a replay that differ from the recording's.
    pub drift: u64,
}

impl TaskMetrics {
    pub fn tokens(&self) -> u64 {
        self.prompt_tokens + self.completion_tokens
    }

    pub fn add(&mut self, other: &TaskMetrics) {
        self.model_calls += other.model_calls;
        self.tool_calls += other.tool_calls;
        self.wrong_calls += other.wrong_calls;
        self.repairs += other.repairs;
        self.invented += other.invented;
        self.held += other.held;
        self.prompt_tokens += other.prompt_tokens;
        self.cached_tokens += other.cached_tokens;
        self.completion_tokens += other.completion_tokens;
        self.prefix_chars = self.prefix_chars.max(other.prefix_chars);
        self.compactions += other.compactions;
        self.wall_ms += other.wall_ms;
        self.drift += other.drift;
    }
}

/// How a task run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskOutcome {
    Passed,
    Failed(Vec<String>),
    /// Not run: why (the endpoint does not answer, a feature this build lacks).
    Skipped(String),
    Ignored(String),
}

impl TaskOutcome {
    pub fn word(&self) -> &'static str {
        match self {
            TaskOutcome::Passed => "passed",
            TaskOutcome::Failed(_) => "FAILED",
            TaskOutcome::Skipped(_) => "skipped",
            TaskOutcome::Ignored(_) => "ignored",
        }
    }
}

/// One run of one task.
#[derive(Clone, Debug)]
pub struct TaskResult {
    pub spec: String,
    pub task: String,
    pub slug: String,
    /// From 1.
    pub run: u32,
    pub outcome: TaskOutcome,
    pub metrics: TaskMetrics,
    /// What the run noticed beside the checks: replay drift, a question with no scripted answer.
    pub notes: Vec<String>,
    pub transcript: Option<PathBuf>,
}

impl TaskResult {
    pub fn key(&self) -> String {
        format!("{}/{}", self.spec, self.slug)
    }

    /// The console line of the run.
    pub fn line(&self) -> String {
        let m = &self.metrics;
        let mut line = format!(
            "{} #{} {}  ({} model, {} tool calls, {} wrong, {} tokens, {:.1}s)",
            self.key(),
            self.run,
            self.outcome.word(),
            m.model_calls,
            m.tool_calls,
            m.wrong_calls,
            group(m.tokens()),
            m.wall_ms as f64 / 1000.0
        );
        match &self.outcome {
            TaskOutcome::Skipped(reason) | TaskOutcome::Ignored(reason) => line.push_str(&format!(": {reason}")),
            _ => {}
        }
        line
    }
}

/// A task over its runs.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSummary {
    pub runs: u32,
    pub passes: u32,
    pub pass_at1: f64,
    /// Mean tokens of a run.
    pub tokens: u64,
    pub metrics: TaskMetrics,
}

/// The suite of a component over a run of the harness.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub profile: String,
    pub runs: u32,
    pub tasks: BTreeMap<String, TaskSummary>,
    /// Mean of the tasks' pass@1.
    pub pass_at1: f64,
    /// Share of the tasks that passed every run.
    pub pass_all: f64,
    pub tasks_passed: u32,
    pub tasks_run: u32,
    pub skipped: u32,
    pub approvals_as_expected: u32,
    pub approvals_expected: u32,
    pub cache_hit: f64,
    /// Mean tokens of a task run.
    pub tokens: u64,
    pub totals: TaskMetrics,
}

/// The summary of the task runs (skipped and ignored ones are counted apart).
pub fn summarize(profile: &str, runs: u32, results: &[TaskResult]) -> Summary {
    let mut summary = Summary {
        profile: profile.to_string(),
        runs,
        ..Summary::default()
    };
    let mut ran = 0u64;
    for result in results {
        match result.outcome {
            TaskOutcome::Skipped(_) | TaskOutcome::Ignored(_) => {
                summary.skipped += 1;
                continue;
            }
            _ => {}
        }
        ran += 1;
        let task = summary.tasks.entry(result.key()).or_default();
        task.runs += 1;
        if result.outcome == TaskOutcome::Passed {
            task.passes += 1;
        }
        task.metrics.add(&result.metrics);
        summary.totals.add(&result.metrics);
        if let Some(expected) = result.metrics.held_expected {
            summary.approvals_expected += 1;
            if expected == result.metrics.held {
                summary.approvals_as_expected += 1;
            }
        }
    }
    for task in summary.tasks.values_mut() {
        task.pass_at1 = f64::from(task.passes) / f64::from(task.runs.max(1));
        task.tokens = task.metrics.tokens() / u64::from(task.runs.max(1));
    }
    let count = summary.tasks.len();
    summary.tasks_run = u32::try_from(count).unwrap_or(u32::MAX);
    if count > 0 {
        summary.pass_at1 = summary.tasks.values().map(|task| task.pass_at1).sum::<f64>() / count as f64;
        summary.tasks_passed = u32::try_from(summary.tasks.values().filter(|task| task.passes == task.runs).count())
            .unwrap_or(u32::MAX);
        summary.pass_all = f64::from(summary.tasks_passed) / count as f64;
    }
    summary.cache_hit = if summary.totals.prompt_tokens == 0 {
        0.0
    } else {
        summary.totals.cached_tokens as f64 / summary.totals.prompt_tokens as f64
    };
    summary.tokens = summary.totals.tokens() / ran.max(1);
    summary
}

impl Summary {
    /// The console report.
    pub fn text(&self, component: &str) -> String {
        let t = &self.totals;
        let runs = u64::from(self.runs.max(1));
        let per = |n: u64| n as f64 / (u64::from(self.tasks_run.max(1)) * runs) as f64;
        let mut text = format!(
            "\nAgent evaluation of {component} ({}, {} run{} a task)\n",
            self.profile,
            self.runs,
            if self.runs == 1 { "" } else { "s" }
        );
        for (key, task) in &self.tasks {
            text.push_str(&format!(
                "  {key}  {}/{}  {} tokens a run\n",
                task.passes,
                task.runs,
                group(task.tokens)
            ));
        }
        text.push_str(&format!(
            "pass@1 {:.2}  pass^{} {:.2}  {} of {} tasks passed every run{}\n",
            self.pass_at1,
            self.runs,
            self.pass_all,
            self.tasks_passed,
            self.tasks_run,
            if self.skipped > 0 { format!(", {} skipped", self.skipped) } else { String::new() }
        ));
        text.push_str(&format!(
            "a run: {:.1} model calls, {:.1} tool calls; in all: {} wrong calls ({:.0}% of the tool calls), {} repairs, \
             {} invented references, approvals as expected {} of {}\n",
            per(t.model_calls),
            per(t.tool_calls),
            t.wrong_calls,
            self.wrong_call_share() * 100.0,
            t.repairs,
            t.invented,
            self.approvals_as_expected,
            self.approvals_expected
        ));
        text.push_str(&format!(
            "tokens: prompt {}, completion {}, cached {} ({:.0}% of prompt); prefix {} chars; {} compactions; wall {:.1}s{}\n",
            group(t.prompt_tokens),
            group(t.completion_tokens),
            group(t.cached_tokens),
            self.cache_hit * 100.0,
            group(t.prefix_chars),
            t.compactions,
            t.wall_ms as f64 / 1000.0,
            if t.drift > 0 { format!("; {} replay results changed", t.drift) } else { String::new() }
        ));
        text
    }

    /// The share of the tool calls that were wrong (an error result): the plan's check is at most 10%.
    pub fn wrong_call_share(&self) -> f64 {
        if self.totals.tool_calls == 0 {
            0.0
        } else {
            self.totals.wrong_calls as f64 / self.totals.tool_calls as f64
        }
    }

    /// The baseline file's content: each task's success and tokens.
    pub fn baseline(&self) -> Json {
        let tasks: serde_json::Map<String, Json> = self
            .tasks
            .iter()
            .map(|(key, task)| (key.clone(), json!({"passAt1": round(task.pass_at1), "tokens": task.tokens})))
            .collect();
        json!({"profile": self.profile, "runs": self.runs, "passAt1": round(self.pass_at1), "tokens": self.tokens,
            "wrongCalls": round(self.wrong_call_share()), "invented": self.totals.invented, "cacheHit": round(self.cache_hit),
            "tasks": tasks})
    }
}

/// The regressions of `summary` against `baseline` (a baseline file's JSON): a task, or the suite, with lower success,
/// or more than [`TOKEN_GROWTH`] more tokens. Tasks only one side has are left out.
pub fn regressions(summary: &Summary, baseline: &Json) -> Vec<String> {
    let mut found = Vec::new();
    let check = |name: &str, success: f64, tokens: u64, base: &Json, found: &mut Vec<String>| {
        if let Some(was) = base.get("passAt1").and_then(Json::as_f64) {
            if success + 1e-6 < was {
                found.push(format!("{name}: success {success:.2}, the baseline's {was:.2}"));
            }
        }
        if let Some(was) = base.get("tokens").and_then(Json::as_u64).filter(|was| *was > 0) {
            if tokens as f64 > was as f64 * (1.0 + TOKEN_GROWTH) {
                found.push(format!(
                    "{name}: {} tokens a run, the baseline's {} (+{:.0}%)",
                    group(tokens),
                    group(was),
                    (tokens as f64 / was as f64 - 1.0) * 100.0
                ));
            }
        }
    };
    let Some(tasks) = baseline.get("tasks").and_then(Json::as_object) else {
        return vec!["the baseline has no tasks".to_string()];
    };
    let mut compared = 0;
    for (key, task) in &summary.tasks {
        if let Some(base) = tasks.get(key.as_str()) {
            compared += 1;
            check(key, task.pass_at1, task.tokens, base, &mut found);
        }
    }
    // the suite's own figures only when the same tasks ran
    if compared == tasks.len() && compared == summary.tasks.len() {
        check("the suite", summary.pass_at1, summary.tokens, baseline, &mut found);
    }
    found
}

/// The tasks of `baseline` that `summary` did not run (skipped or missing): a set that lost a recording, or a task
/// renamed.
pub fn not_run(summary: &Summary, baseline: &Json) -> Vec<String> {
    let Some(tasks) = baseline.get("tasks").and_then(Json::as_object) else {
        return Vec::new();
    };
    tasks
        .keys()
        .filter(|key| !summary.tasks.contains_key(key.as_str()))
        .map(|key| format!("{key}: in the baseline, not run"))
        .collect()
}

fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// A number with thousands separated by commas.
pub fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}
