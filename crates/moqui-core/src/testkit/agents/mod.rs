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

//! The evaluation harness of the agents (plans/LLM-A2A-Assist.md 9, `moqui test <component> --agents`): the tasks of a
//! component's `test/agent/*.agent.js` run as real runs of an agent, on a model of the conf (a live profile) or on the
//! model replies of earlier transcripts (a replay, with no model), and are checked deterministically.
//!
//! A spec file calls `agentSpec({agent, surface, user, tasks})` ([`spec`]). Each task gives the page the user starts on,
//! the user's messages (one or more turns), answers to the agent's questions, scripted approval answers, and checks:
//! the screen's path, parameters and form values, the answer, the tools called and not called, their results, the
//! writes held for approval, no write attempt, the questions, and a `verify(run)` function for the rest. A task run
//! makes a conversation of the spec's user with the start page as its screen state, then a run per turn
//! ([`crate::llm::run::start_run`]) with the drawer's `<context>` tail, waits for it, answers its questions, and reads
//! what it did back from its rows ([`observe`]). Everything goes to a transcript ([`transcript`]); the measures are in
//! [`metrics`].
//!
//! A replay plays a task's recording (`<dir>/<spec>/<task>-<n>.jsonl`, else `<task>-1.jsonl`) through the fake
//! provider and runs the tools live, so a change of a tool's output shows in the checks and as drift from the recorded
//! results. A live run whose endpoint does not answer is skipped, not failed ([`profile::live`]).
//!
//! A write pauses the run for its approval ([`crate::llm::approval`]); the task's scripted answers decide them in order
//! as its user would in the drawer ([`answer_approval`]): approve, reject with feedback, or edit, which hands the
//! write to the screen and then saves it there as the user, with the scripted values over the proposed ones, so the
//! hand-off's observer sees the user's own save. A task checks the approvals asked for (`held`) and what became of
//! them.

pub mod metrics;
pub mod observe;
pub mod profile;
pub mod spec;
pub mod transcript;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_lib::error::Error;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use crate::context::ExecutionContextFactory;
use crate::entity::EntityFacade;
use crate::llm::prompt::{build_tail, Surface, TailSpec};
use crate::llm::provider::fake;
use crate::llm::run::{self, in_tx, quietly, RunState, StartRequest};
use crate::llm::screen::ScreenState;
use crate::llm::text_of;
use crate::llm::tools::PauseKind;
use crate::testkit::SpecFile;

pub use metrics::{not_run, regressions, summarize, Summary, TaskMetrics, TaskOutcome, TaskResult};
pub use spec::ScriptedApproval;

/// A turn's limit on a live model, and on a replay, unless the spec or the task gives one.
pub const LIVE_TURN_TIMEOUT: Duration = Duration::from_secs(300);
pub const REPLAY_TURN_TIMEOUT: Duration = Duration::from_secs(60);

/// What the model is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// A model of the conf or the `LlmModel` rows, by id.
    Live { profile: String },
    /// The recorded replies under this directory.
    Replay { dir: PathBuf },
}

/// What [`evaluate`] runs.
#[derive(Clone, Debug)]
pub struct EvalOptions {
    pub mode: Mode,
    /// Runs of each task.
    pub runs: u32,
    /// The seed of the first run; run n sends `seed + n - 1` (Chat Completions models only).
    pub seed: Option<u64>,
    /// Only the tasks whose name contains this.
    pub task_filter: Option<String>,
    /// Where the transcripts go: `<dir>/<spec>/<task>-<n>.jsonl`.
    pub out_dir: PathBuf,
}

impl EvalOptions {
    /// The profile's name in reports and the baseline's file name: the model id, or the replay's ([`replay_label`]).
    pub fn profile_label(&self) -> String {
        self.mode.label()
    }
}

impl Mode {
    /// The profile's name in reports and the baseline's file name: the model id, or the replay's ([`replay_label`]).
    pub fn label(&self) -> String {
        match self {
            Mode::Live { profile } => profile.clone(),
            Mode::Replay { dir } => replay_label(dir),
        }
    }
}

/// The profile of a replay set: `replay` for a directory named so (the recordings of the default profile), else
/// `replay-<name>` (`replay-small` for `replay-small/`, the JSON tool mode recordings of a small model), so each set
/// has its own baseline.
pub fn replay_label(dir: &Path) -> String {
    let name = dir.file_name().and_then(|name| name.to_str()).unwrap_or("replay");
    if name == "replay" || name.starts_with("replay-") {
        name.to_string()
    } else {
        format!("replay-{name}")
    }
}

/// What [`evaluate`] did.
#[derive(Debug, Default)]
pub struct Evaluation {
    pub results: Vec<TaskResult>,
    /// The whole run was skipped: why (the endpoint does not answer).
    pub skipped: Option<String>,
}

/// A spec of a component to run: its file and the component's name.
#[derive(Clone, Debug)]
pub struct AgentSpecFile {
    pub component: String,
    pub file: SpecFile,
}

/// Run the specs' tasks. Each spec runs in a fresh execution context of `factory`; `on_result` hears each task run as
/// it ends. A spec that does not load fails each of its tasks, and the others go on.
///
/// # Errors
///
/// Returns an error when the profile names no model or the model rows cannot be written; a live endpoint that does not
/// answer is not an error but [`Evaluation::skipped`].
pub fn evaluate(
    factory: &Arc<ExecutionContextFactory>,
    specs: &[AgentSpecFile],
    options: &EvalOptions,
    on_result: &mut dyn FnMut(&TaskResult),
) -> Result<Evaluation, Error> {
    let target = match &options.mode {
        Mode::Live { profile } => match profile::live(factory, profile)? {
            profile::Live::Ready(target) => Some(target),
            profile::Live::Skip(reason) => {
                return Ok(Evaluation {
                    results: Vec::new(),
                    skipped: Some(reason),
                })
            }
        },
        Mode::Replay { .. } => None,
    };
    let mut rows = profile::Rows::default();
    let mut evaluation = Evaluation::default();
    for spec_file in specs {
        let context = factory.new_context();
        let entered = context.enter();
        let outcome = run_spec(factory, spec_file, options, target.as_ref(), &mut rows, &mut |result| {
            on_result(result);
            evaluation.results.push(result.clone());
        });
        drop(entered);
        drop(context);
        if let Err(error) = outcome {
            rows.restore(factory);
            return Err(error);
        }
    }
    rows.restore(factory);
    Ok(evaluation)
}

fn run_spec(
    factory: &Arc<ExecutionContextFactory>,
    spec_file: &AgentSpecFile,
    options: &EvalOptions,
    target: Option<&profile::Target>,
    rows: &mut profile::Rows,
    on_result: &mut dyn FnMut(&TaskResult),
) -> Result<(), Error> {
    let file = &spec_file.file;
    let fail_all = |reason: String, on_result: &mut dyn FnMut(&TaskResult)| {
        on_result(&TaskResult {
            spec: file.name.clone(),
            task: "(every task)".to_string(),
            slug: "load".to_string(),
            run: 1,
            outcome: TaskOutcome::Failed(vec![reason]),
            metrics: TaskMetrics::default(),
            notes: Vec::new(),
            transcript: None,
        });
    };
    let plan = match spec::load(file) {
        Ok(plan) => plan,
        Err(reason) => {
            fail_all(reason, on_result);
            reset_context();
            return Ok(());
        }
    };
    let surface = Surface::ALL
        .into_iter()
        .find(|surface| surface.as_str() == plan.spec.surface)
        .unwrap_or_default();
    let facade = factory
        .entity()
        .ok_or_else(|| Error::new("the entity facade is not installed"))?;
    let feature = agent_feature(&facade, &plan.spec.agent);
    let missing: Vec<&str> = plan
        .spec
        .requires
        .iter()
        .map(String::as_str)
        .filter(|feature| !crate::build_info::has_feature(feature))
        .collect();
    let spec_skip = (!missing.is_empty())
        .then(|| format!("requires {}, not built into this binary (moqui version)", missing.join(", ")));
    if spec_skip.is_none() {
        if let Some(target) = target {
            let reasoning = profile::feature_reasoning(factory, &feature);
            rows.bind(factory, &feature, &target.model_id, reasoning.as_str())?;
        }
        if let Err(error) = spec::phase(None, "setup") {
            fail_all(format!("the spec's setup failed: {error}"), on_result);
            let _ = spec::phase(None, "cleanup");
            reset_context();
            return Ok(());
        }
    }
    for (index, task) in plan.tasks.iter().enumerate() {
        if let Some(filter) = &options.task_filter {
            if !task.name.contains(filter.as_str()) {
                continue;
            }
        }
        for run in 1..=options.runs.max(1) {
            let mut result = TaskResult {
                spec: file.name.clone(),
                task: task.name.clone(),
                slug: task.slug.clone(),
                run,
                outcome: TaskOutcome::Passed,
                metrics: TaskMetrics::default(),
                notes: Vec::new(),
                transcript: None,
            };
            if let Some(reason) = &spec_skip {
                result.outcome = TaskOutcome::Skipped(reason.clone());
            } else if let Some(reason) = &task.ignore {
                result.outcome = TaskOutcome::Ignored(reason.clone());
            } else {
                let job = TaskJob {
                    factory,
                    facade: &facade,
                    spec: &plan.spec,
                    spec_name: &file.name,
                    index,
                    task,
                    run,
                    surface,
                    feature: &feature,
                    options,
                    target,
                };
                job.run(rows, &mut result);
            }
            on_result(&result);
            if spec_skip.is_some() || task.ignore.is_some() {
                break;
            }
        }
    }
    if spec_skip.is_none() {
        if let Err(error) = spec::phase(None, "cleanup") {
            tracing::error!("the cleanup of agent spec {} failed: {error}", file.display);
        }
    }
    reset_context();
    Ok(())
}

/// The model feature of the agent: its `featureId`, else `assist`.
fn agent_feature(facade: &EntityFacade, agent_id: &str) -> String {
    quietly(|| {
        facade
            .find("moqui.agent.AgentDefinition")
            .and_then(|find| find.eq("agentId", Value::string(agent_id)).disable_authz().one())
            .ok()
            .flatten()
            .and_then(|row| text_of(&row, "featureId"))
            .unwrap_or_else(|| "assist".to_string())
    })
}

/// One run of one task.
struct TaskJob<'a> {
    factory: &'a Arc<ExecutionContextFactory>,
    facade: &'a EntityFacade,
    spec: &'a spec::SpecPlan,
    spec_name: &'a str,
    index: usize,
    task: &'a spec::TaskPlan,
    run: u32,
    surface: Surface,
    feature: &'a str,
    options: &'a EvalOptions,
    target: Option<&'a profile::Target>,
}

impl TaskJob<'_> {
    fn run(&self, rows: &mut profile::Rows, result: &mut TaskResult) {
        let started = Instant::now();
        let mut failures = Vec::new();
        let transcript_path = self
            .options
            .out_dir
            .join(self.spec_name)
            .join(format!("{}-{}.jsonl", self.task.slug, self.run));
        // the model: a recording on the fake, or the live model with this run's seed
        let mut recording = None;
        let mut model = json!(null);
        let mut seed = None;
        if let Mode::Replay { dir } = &self.options.mode {
            // a replay set of a profile may record only some tasks (a JSON-mode set of a small model): the others are
            // skipped, and a baseline names what must run
            if !dir.join(self.spec_name).join(format!("{}-1.jsonl", self.task.slug)).is_file() {
                result.outcome = TaskOutcome::Skipped(format!("{} has no recording of it", dir.display()));
                return;
            }
        }
        match &self.options.mode {
            Mode::Replay { dir } => match find_recording(dir, self.spec_name, &self.task.slug, self.run)
                .and_then(|path| transcript::Recording::read(&path).map_err(|err| err.to_string()))
            {
                Ok(found) => {
                    model = found.model.clone();
                    if let Err(error) = rows.replay_model(self.factory, &found.model).and_then(|()| {
                        let reasoning = found.model.get("reasoningLevel").and_then(Json::as_str).unwrap_or("low");
                        rows.bind(self.factory, self.feature, profile::REPLAY_MODEL, reasoning)
                    }) {
                        failures.push(format!("the replay model could not be set: {error}"));
                    }
                    found.script(profile::REPLAY_MODEL);
                    recording = Some(found);
                }
                Err(error) => failures.push(error),
            },
            Mode::Live { .. } => {
                if let Some(target) = self.target {
                    model = target.settings.clone();
                    model["reasoningLevel"] = json!(profile::feature_reasoning(self.factory, self.feature).as_str());
                    if let (Some(base), Some(first)) = (&target.seed_base, self.options.seed) {
                        let value = first.saturating_add(u64::from(self.run - 1));
                        match rows.seed(self.factory, &target.model_id, base, value) {
                            Ok(()) => seed = Some(value),
                            Err(error) => failures.push(format!("the seed could not be set: {error}")),
                        }
                    }
                }
            }
        }
        if !failures.is_empty() {
            result.outcome = TaskOutcome::Failed(failures);
            return;
        }
        let mut out = transcript::Transcript::create(transcript_path);
        out.line(&json!({"type": "header", "spec": self.spec_name, "task": self.task.name, "run": self.run,
            "profile": self.options.profile_label(), "model": model, "seed": seed, "agent": self.spec.agent,
            "surface": self.surface.as_str(), "startedDate": jiff::Timestamp::now().to_string()}));
        result.transcript = Some(out.path().to_path_buf());

        if let Err(error) = spec::phase(Some(self.index), "setup") {
            result.outcome = TaskOutcome::Failed(vec![format!("the task's setup failed: {error}")]);
            let _ = spec::phase(Some(self.index), "cleanup");
            out.line(&json!({"type": "result", "passed": false, "failures": [format!("setup: {error}")]}));
            out.finish();
            return;
        }
        let turns = self.turns(&mut failures, &mut result.notes, &mut out);
        let observed = json!({"turns": turns.iter().map(observe::TurnSeen::to_check).collect::<Vec<_>>()});
        if failures.is_empty() || !turns.is_empty() {
            failures.extend(spec::check(self.index, &observed));
        }
        if let Err(error) = spec::phase(Some(self.index), "cleanup") {
            result.notes.push(format!("the task's cleanup failed: {error}"));
        }
        let mut drift = Vec::new();
        if let Some(recording) = &recording {
            let made = fake::requests(profile::REPLAY_MODEL).len();
            if made != recording.replies.len() {
                failures.push(format!(
                    "the run asked the model {made} time(s); the recording has {} repl{}",
                    recording.replies.len(),
                    if recording.replies.len() == 1 { "y" } else { "ies" }
                ));
            }
            let now: Vec<(String, String)> = turns
                .iter()
                .flat_map(|turn| turn.calls.iter().map(|call| (call.tool.clone(), call.result.clone())))
                .collect();
            drift = transcript::drift(&recording.results, &now);
            for line in &drift {
                result.notes.push(format!("replay drift: {line}"));
            }
            fake::clear(profile::REPLAY_MODEL);
        }
        result.metrics = self.metrics(&turns, drift.len(), started.elapsed());
        result.outcome = if failures.is_empty() {
            TaskOutcome::Passed
        } else {
            TaskOutcome::Failed(failures.clone())
        };
        out.line(&json!({"type": "result", "passed": failures.is_empty(), "failures": failures,
            "notes": result.notes, "metrics": result.metrics}));
        out.finish();
    }

    /// The task's turns: a conversation with the start page, a run per message, each read back.
    fn turns(
        &self,
        failures: &mut Vec<String>,
        notes: &mut Vec<String>,
        out: &mut transcript::Transcript,
    ) -> Vec<observe::TurnSeen> {
        let username = self.task.user.as_deref().unwrap_or(&self.spec.user);
        let Some(user_id) = user_id_of(self.facade, username) else {
            failures.push(format!("there is no user {username}"));
            return Vec::new();
        };
        let conversation_id = match self.conversation(&user_id) {
            Ok(id) => id,
            Err(error) => {
                failures.push(format!("the conversation could not be made: {error}"));
                return Vec::new();
            }
        };
        let timeout = self
            .task
            .timeout
            .or(self.spec.timeout)
            .filter(|seconds| *seconds > 0.0)
            .map(Duration::from_secs_f64)
            .unwrap_or(match self.options.mode {
                Mode::Live { .. } => LIVE_TURN_TIMEOUT,
                Mode::Replay { .. } => REPLAY_TURN_TIMEOUT,
            });
        let mut answers = self.task.answers.iter();
        let mut approvals = self.task.approvals.iter();
        let mut seen = Vec::new();
        for (at, turn) in self.task.turns.iter().enumerate() {
            let turn_started = Instant::now();
            let state = crate::llm::screen::load_state(self.facade, &conversation_id).ok().flatten();
            let request = StartRequest {
                user_id: user_id.clone(),
                agent_id: self.spec.agent.clone(),
                conversation_id: Some(conversation_id.clone()),
                surface: self.surface,
                text: turn.say.clone(),
                tail: Some(self.tail(state.as_ref())),
                ..StartRequest::default()
            };
            // the AT_LLM tarpit (30 runs a minute) is for people: a suite of 24 tasks, or --runs 3, starts more
            let caller = crate::user::current_user_id().ok().flatten().unwrap_or_default();
            self.factory
                .clear_tarpit_hits(&format!("{caller}@AT_LLM:agent:{}", self.spec.agent));
            let started = match run::start_run(self.factory, &request) {
                Ok(started) => started,
                Err(error) => {
                    failures.push(format!("turn {}: the run did not start: {error}", at + 1));
                    break;
                }
            };
            let ended = self.settle(&started.run_id, &user_id, timeout, &mut answers, &mut approvals, notes, out);
            let mut observed = match observe::observe(self.facade, &conversation_id, &started.run_id, &turn.say) {
                Ok(observed) => observed,
                Err(error) => {
                    failures.push(format!("turn {}: the run could not be read: {error}", at + 1));
                    break;
                }
            };
            observed.wall_ms = u64::try_from(turn_started.elapsed().as_millis()).unwrap_or(u64::MAX);
            out.turn(at, &observed);
            if let Err(why) = ended {
                failures.push(format!("turn {}: {why}", at + 1));
            } else if observed.state != observe::state_name(RunState::Completed) {
                failures.push(format!(
                    "turn {}: the run ended {}{}",
                    at + 1,
                    observed.state,
                    observed.error.as_ref().map(|error| format!(": {error}")).unwrap_or_default()
                ));
            }
            seen.push(observed);
            if !failures.is_empty() {
                break;
            }
        }
        if let Some(turn) = seen.last_mut() {
            turn.prefix_chars = observe::prefix_chars(self.facade, &conversation_id);
        }
        seen
    }

    /// Wait for the run to end, answering its questions and approvals from the script. `Err` says why it did not end
    /// by itself.
    #[allow(clippy::too_many_arguments)]
    fn settle(
        &self,
        run_id: &str,
        user_id: &str,
        timeout: Duration,
        answers: &mut std::slice::Iter<'_, String>,
        approvals: &mut std::slice::Iter<'_, ScriptedApproval>,
        notes: &mut Vec<String>,
        out: &mut transcript::Transcript,
    ) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let state = run::wait_for(self.factory, run_id, left).map_err(|err| err.to_string())?;
            if state.is_terminal() {
                return Ok(());
            }
            if state.is_paused() {
                let pause = quietly(|| run::load_run(self.facade, run_id))
                    .ok()
                    .flatten()
                    .and_then(|run| run.pause_kind);
                let answered = match pause {
                    Some(PauseKind::Question) => match answers.next() {
                        Some(answer) => {
                            out.line(&json!({"type": "question.answer", "text": answer}));
                            run::submit_input(self.factory, run_id, user_id, "answer", answer, None)
                                .map(|_| ())
                                .map_err(|err| format!("the scripted answer was not taken: {err}"))
                        }
                        None => Err("the agent asked a question the task has no scripted answer for".to_string()),
                    },
                    Some(PauseKind::Approval) => match approvals.next() {
                        Some(answer) => {
                            out.line(&json!({"type": "approval.answer", "kind": answer.kind, "values": answer.values,
                                "feedback": answer.feedback}));
                            match answer_approval(self.factory, run_id, user_id, answer) {
                                ApprovalApplied::Applied => Ok(()),
                                ApprovalApplied::Refused(why) => Err(format!(
                                    "the scripted approval answer {} was not applied: {why}",
                                    answer.kind
                                )),
                            }
                        }
                        None => Err("the run waits for an approval the task has no scripted answer for".to_string()),
                    },
                    other => Err(format!("the run paused for {other:?}, which the harness does not answer")),
                };
                if let Err(why) = answered {
                    notes.push(why.clone());
                    let _ = run::cancel_run(self.factory, run_id, Some(user_id));
                    let _ = run::wait_for(self.factory, run_id, Duration::from_secs(10));
                    return Err(why);
                }
                continue;
            }
            if Instant::now() >= deadline {
                let _ = run::cancel_run(self.factory, run_id, Some(user_id));
                let _ = run::wait_for(self.factory, run_id, Duration::from_secs(10));
                return Err(format!("the run did not end within {}s", timeout.as_secs()));
            }
        }
    }

    /// A conversation of the user with the agent, its screen state the start page.
    fn conversation(&self, user_id: &str) -> Result<String, Error> {
        let id = quietly(|| {
            in_tx(self.facade, || {
                let mut row = self.facade.make_value("moqui.agent.AgentConversation")?;
                row.set_sequenced_id_primary(self.facade)?;
                row.set("userId", Value::string(user_id));
                row.set("agentId", Value::string(&self.spec.agent));
                row.set("surfaceEnumId", Value::string(self.surface.enum_id()));
                row.set("statusId", Value::string("AgcActive"));
                row.set("title", Value::string(format!("Agent evaluation: {}", self.task.name)));
                row.set("lastMessageSeq", Value::I64(0));
                row.set("createdDate", run::stamp(run::now_millis()));
                row.create(self.facade)?;
                Ok(text_of(&row, "conversationId").unwrap_or_default())
            })
        })?;
        if let Some(start) = &self.task.start {
            let (path, mut parameters) = split_query(&start.path);
            for (name, value) in &start.parameters {
                parameters.insert(name.clone(), value.as_str().map_or_else(|| value.to_string(), str::to_string));
            }
            let state = ScreenState {
                path,
                parameters,
                values: BTreeMap::new(),
            };
            crate::llm::screen::store_state(self.facade, &id, crate::llm::screen::StateWriter::Owner(user_id), &state)?;
        }
        Ok(id)
    }

    /// The `<context>` tail the drawer sends: now, and the page the user is on (the screen state), with the start's
    /// title on the first turn.
    fn tail(&self, state: Option<&ScreenState>) -> String {
        let headless = !matches!(self.surface, Surface::Drawer | Surface::Tui);
        let page = state.filter(|state| !state.path.is_empty()).map(|state| {
            let title = self
                .task
                .start
                .as_ref()
                .filter(|start| split_query(&start.path).0 == state.path)
                .and_then(|start| start.title.clone())
                .unwrap_or_default();
            (state.url(), title)
        });
        build_tail(&TailSpec {
            now: Some(jiff::Zoned::now()),
            page,
            headless,
            ..TailSpec::default()
        })
    }

    fn metrics(&self, turns: &[observe::TurnSeen], drift: usize, wall: Duration) -> TaskMetrics {
        let mut metrics = TaskMetrics {
            held_expected: self.task.expect_held.map(u64::from),
            wall_ms: u64::try_from(wall.as_millis()).unwrap_or(u64::MAX),
            drift: drift as u64,
            ..TaskMetrics::default()
        };
        for turn in turns {
            metrics.model_calls += turn.replies.len() as u64;
            metrics.tool_calls += turn.calls.len() as u64;
            metrics.wrong_calls += turn.calls.iter().filter(|call| call.error).count() as u64;
            metrics.repairs += turn.calls.iter().filter(|call| call.repaired).count() as u64;
            metrics.invented += turn.invented.len() as u64;
            metrics.held += turn.held.len() as u64;
            metrics.compactions += u64::from(turn.compactions);
            metrics.prefix_chars = metrics.prefix_chars.max(turn.prefix_chars);
            // the call log has every call of the run, the side calls too; the steps only the turn's own
            let mut steps = observe::Usage::default();
            for reply in &turn.replies {
                steps.add(&reply.usage);
            }
            let spent = if turn.spent.input >= steps.input { turn.spent } else { steps };
            metrics.prompt_tokens += spent.input;
            metrics.cached_tokens += spent.cached;
            metrics.completion_tokens += spent.output;
        }
        metrics
    }
}

/// What became of a scripted approval answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApprovalApplied {
    Applied,
    /// It could not be applied: why.
    Refused(String),
}

/// Answer the approval a run waits for with a scripted answer, as its user would in the drawer: approve; reject with
/// the feedback; or edit, which takes the write to the screen (Edit in screen) and saves it there as the user with the
/// scripted values over the proposed ones (by request name or by label), so the hand-off's observer sees the user's
/// own save. The values of a service or tool call's edit are its edited arguments instead.
pub fn answer_approval(
    factory: &Arc<ExecutionContextFactory>,
    run_id: &str,
    user_id: &str,
    answer: &ScriptedApproval,
) -> ApprovalApplied {
    use crate::llm::approval::{self, Channel, Decider, Decision, TargetKind};
    let refused = |why: String| ApprovalApplied::Refused(why);
    let Some(facade) = factory.entity() else {
        return refused("the entity facade is not installed".to_string());
    };
    let Some(approval_id) = quietly(|| run::load_run(&facade, run_id)).ok().flatten().and_then(|run| run.pause_ref)
    else {
        return refused("the run waits for no approval".to_string());
    };
    let Some(view) = quietly(|| approval::load(&facade, &approval_id)).ok().flatten() else {
        return refused(format!("there is no approval {approval_id}"));
    };
    let decider = Decider::scripted(user_id, Channel::Drawer);
    let decision = match answer.kind.as_str() {
        "approve" => Decision::Approve { arguments: None },
        "reject" => Decision::Reject {
            feedback: answer.feedback.clone(),
        },
        "edit" if view.target_kind != TargetKind::Screen => {
            let mut arguments = view.args.clone();
            for (name, value) in &answer.values {
                arguments[name] = value.clone();
            }
            Decision::Approve {
                arguments: Some(arguments),
            }
        }
        "edit" => Decision::Edit,
        other => return refused(format!("{other} is not a scripted approval answer")),
    };
    if let Err(error) = approval::decide(factory, &approval_id, &decider, &decision) {
        return refused(error.to_string());
    }
    if decision != Decision::Edit {
        return ApprovalApplied::Applied;
    }
    match save_in_screen(factory, user_id, &view, &answer.values) {
        Ok(()) => ApprovalApplied::Applied,
        Err(why) => refused(format!("the user's save in the screen did not run: {why}")),
    }
}

/// The user's own save of a handed-off screen write: the held request sent again as the user at the terminal, with
/// no gate, the scripted values over the proposed ones; on a thread of its own, as a request of its own.
fn save_in_screen(
    factory: &Arc<ExecutionContextFactory>,
    user_id: &str,
    view: &crate::llm::approval::ApprovalView,
    values: &BTreeMap<String, Json>,
) -> Result<(), String> {
    let write: crate::llm::screen::PendingWrite =
        serde_json::from_value(view.args.clone()).map_err(|error| format!("the held write does not read: {error}"))?;
    let changes = view.changes();
    let mut fields = write.fields.clone();
    for (name, value) in values {
        let field = changes
            .iter()
            .find(|change| change.field == *name || change.label.eq_ignore_ascii_case(name))
            .map_or(name.as_str(), |change| change.field.as_str());
        fields.insert(field.to_string(), value.as_str().map_or_else(|| value.to_string(), str::to_string));
    }
    let username = user_name_of(factory, user_id).ok_or_else(|| format!("there is no user {user_id}"))?;
    let (path, query) = split_query(&write.url);
    let mut parameters: BTreeMap<String, Value> =
        query.into_iter().map(|(name, value)| (name, Value::string(value))).collect();
    for (name, value) in fields {
        parameters.insert(name, Value::string(value));
    }
    let method = write.method.clone();
    std::thread::scope(|scope| {
        scope
            .spawn(|| -> Result<(), String> {
                let context = factory.new_context();
                let _entered = context.enter();
                {
                    let _authz = context.disable_authz();
                    crate::user::internal_login_user(&username).map_err(|error| error.to_string())?;
                }
                let output = crate::screen::make_render(factory)
                    .screen_path(path)
                    .method(method)
                    .parameters(parameters)
                    .origin(crate::screen::RenderOrigin::Terminal)
                    .header("Accept", "application/json")
                    .render()
                    .map_err(|error| error.to_string())?;
                if output.status >= 400 {
                    return Err(format!(
                        "the screen answered {}: {}",
                        output.status,
                        String::from_utf8_lossy(&output.body).chars().take(300).collect::<String>()
                    ));
                }
                Ok(())
            })
            .join()
            .unwrap_or_else(|_| Err("the save panicked".to_string()))
    })
}

fn user_name_of(factory: &Arc<ExecutionContextFactory>, user_id: &str) -> Option<String> {
    let facade = factory.entity()?;
    quietly(|| {
        facade
            .find("moqui.security.UserAccount")
            .and_then(|find| find.eq("userId", Value::string(user_id)).disable_authz().one())
            .ok()
            .flatten()
            .and_then(|row| text_of(&row, "username"))
    })
}

/// The recording of a task run: `<dir>/<spec>/<task>-<run>.jsonl`, else `<task>-1.jsonl`.
fn find_recording(dir: &Path, spec: &str, slug: &str, run: u32) -> Result<PathBuf, String> {
    let exact = dir.join(spec).join(format!("{slug}-{run}.jsonl"));
    if exact.is_file() {
        return Ok(exact);
    }
    let first = dir.join(spec).join(format!("{slug}-1.jsonl"));
    if first.is_file() {
        return Ok(first);
    }
    Err(format!("there is no recording {}", exact.display()))
}

fn user_id_of(facade: &EntityFacade, username: &str) -> Option<String> {
    quietly(|| {
        facade
            .find("moqui.security.UserAccount")
            .and_then(|find| find.eq("username", Value::string(username)).disable_authz().one())
            .ok()
            .flatten()
            .and_then(|row| text_of(&row, "userId"))
    })
}

/// A path and its query parameters.
fn split_query(url: &str) -> (String, BTreeMap<String, String>) {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let parameters = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(name), decode(value))
        })
        .collect();
    (path.to_string(), parameters)
}

fn decode(text: &str) -> String {
    crate::web::percent_decode(text)
}

/// What a spec leaves on the thread does not reach the next.
fn reset_context() {
    if let Err(err) = crate::user::logout() {
        tracing::debug!("logout after an agent spec: {err}");
    }
    crate::script::clear_context_stack();
}
