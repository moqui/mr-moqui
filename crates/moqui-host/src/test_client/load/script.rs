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

//! Load scripts (plans/LoadRunner.md section 4): a `<component>/test/load/<Name>.load.js` file with `plan({...})` and
//! `scenario(name, {...})` at its top level. The file is evaluated once to read its plan ([`read_plan`]) and once in
//! each user's engine ([`super::start`] with [`super::Source::Script`]). A scenario's settings are its defaults with
//! what the run overrides ([`settings`]).

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_devkit::testkit::{run_script_on_thread, ScriptFile, ScriptThread, SpecHost};
use serde_json::{json, Map, Value};

use super::pace::{Pause, MAX_SECONDS};
use super::MAX_USERS;

/// The settings a scenario runs with.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ScenarioSettings {
    pub(crate) name: String,
    pub(crate) users: u64,
    /// Seconds over which the users start, evenly.
    pub(crate) ramp_up: f64,
    /// Seconds the scenario repeats for.
    pub(crate) duration: Option<f64>,
    /// Iterations per user.
    pub(crate) iterations: Option<u64>,
    /// Between a user's iterations.
    pub(crate) think: Option<Pause>,
    /// Before each request.
    pub(crate) pause: Option<Pause>,
    /// Each iteration starts with a fresh `http` session.
    pub(crate) new_session: bool,
}

impl ScenarioSettings {
    /// The settings in `value` (`users`, `rampUp`, `duration`, `iterations`, `think`, `pause`, `newSession`). `who`
    /// starts an error: `load` or `load: scenario <name>`.
    ///
    /// # Errors
    ///
    /// When a setting is out of its range, or there is neither a duration nor iterations.
    pub(crate) fn from_json(name: &str, value: &Value, who: &str) -> Result<ScenarioSettings, String> {
        let field = |key: &str| value.get(key).unwrap_or(&Value::Null);
        let users = field("users").as_f64().unwrap_or(1.0);
        if !((1.0..=MAX_USERS as f64).contains(&users) && users.fract() == 0.0) {
            return Err(format!(
                "{who}: users is a whole number from 1 to {MAX_USERS}, not {users}"
            ));
        }
        let duration = field("duration")
            .as_f64()
            .filter(|seconds| *seconds > 0.0)
            .map(|seconds| seconds.min(MAX_SECONDS));
        let iterations = field("iterations")
            .as_f64()
            .filter(|count| *count >= 1.0)
            .map(|count| count as u64);
        if duration.is_none() && iterations.is_none() {
            return Err(format!("{who} needs a duration (seconds) or iterations (per user)"));
        }
        Ok(ScenarioSettings {
            name: name.to_string(),
            users: users as u64,
            ramp_up: field("rampUp").as_f64().unwrap_or(0.0).clamp(0.0, MAX_SECONDS),
            duration,
            iterations,
            think: Pause::parse(field("think"), &format!("{who}: think"))?,
            pause: Pause::parse(field("pause"), &format!("{who}: pause"))?,
            new_session: field("newSession").as_bool().unwrap_or(false),
        })
    }

    pub(crate) fn to_json(&self) -> Value {
        json!({
            "users": self.users, "rampUp": self.ramp_up, "duration": self.duration, "iterations": self.iterations,
            "think": self.think.as_ref().map(Pause::to_json), "pause": self.pause.as_ref().map(Pause::to_json),
            "newSession": self.new_session,
        })
    }

    /// Seconds before its users are told to stop: its duration and ramp and 120, or 600 with only iterations.
    pub(crate) fn timeout(&self) -> f64 {
        self.duration.map_or(600.0, |seconds| seconds + self.ramp_up + 120.0)
    }
}

/// A load script's file.
pub(crate) struct LoadScript {
    #[allow(dead_code)] // for `moqui test-load` and a run in a server (plans/LoadRunner.md L2, L3)
    pub(crate) path: PathBuf,
    /// The name errors give it: the component's directory name and the path within it.
    pub(crate) display: String,
    pub(crate) source: String,
}

impl LoadScript {
    /// The `.load.js` file at `relative` within `component_dir`: relative, with no `..`, and not out through a link,
    /// so a spec's `load({ script })` runs only a script of its own component.
    ///
    /// # Errors
    ///
    /// When the path leaves the component, is not a `.load.js` file, or does not read.
    pub(crate) fn read(component_dir: &Path, relative: &str) -> Result<LoadScript, String> {
        let path = Path::new(relative);
        if path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::Prefix(_)))
        {
            return Err(format!("load: {relative} is not a path within the component"));
        }
        if !relative.ends_with(".load.js") {
            return Err(format!("load: {relative} is not a load script (<Name>.load.js)"));
        }
        let root =
            std::fs::canonicalize(component_dir).map_err(|err| format!("load: {}: {err}", component_dir.display()))?;
        let full = std::fs::canonicalize(root.join(path)).map_err(|err| format!("load: {relative}: {err}"))?;
        if !full.starts_with(&root) {
            return Err(format!("load: {relative} is outside the component"));
        }
        let component = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        LoadScript::from_file(&full, &format!("{component}/{}", relative.trim_start_matches("./")))
    }

    /// The script at `path`, already checked by the caller to be one it may run, named `display` in errors.
    ///
    /// # Errors
    ///
    /// When the file does not read.
    pub(crate) fn from_file(path: &Path, display: &str) -> Result<LoadScript, String> {
        let source = std::fs::read_to_string(path).map_err(|err| format!("load: {display}: {err}"))?;
        Ok(LoadScript {
            path: path.to_path_buf(),
            display: display.to_string(),
            source,
        })
    }
}

/// What a script's top level declared.
#[derive(Clone, Debug, Default)]
pub(crate) struct Plan {
    /// `title`, `description`, `uses`, `params` (each `default` (none for a secret), `description`, `options`,
    /// `secret`, `required`, `type`), `thresholds`.
    pub(crate) plan: Value,
    /// The params as the script resolved them: the run's over the defaults, of the defaults' types. Secrets too.
    pub(crate) params: Value,
    /// Each scenario's defaults, in the file's order.
    pub(crate) scenarios: Vec<(String, Value)>,
}

impl Plan {
    pub(crate) fn thresholds(&self) -> &Value {
        self.plan.get("thresholds").unwrap_or(&Value::Null)
    }

    /// The kinds of load the script makes: `http` by default.
    pub(crate) fn uses(&self) -> Vec<String> {
        self.plan
            .get("uses")
            .and_then(Value::as_array)
            .map(|uses| uses.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_else(|| vec!["http".to_string()])
    }

    /// The resolved params without the secrets, for a run's record.
    pub(crate) fn public_params(&self) -> Value {
        let secret = |name: &str| {
            self.plan
                .get("params")
                .and_then(|params| params.get(name))
                .and_then(|param| param.get("secret"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        };
        let params: Map<String, Value> = self
            .params
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(name, _)| !secret(name))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        Value::Object(params)
    }
}

/// How long reading a plan may take: the file's top level only declares, so this is long.
const PLAN_DEADLINE: Duration = Duration::from_secs(30);

/// What a user's and the plan's engine evaluate last, whatever happened: close the default session the client
/// prelude opened, which `thread.end` does not when the file threw.
pub(crate) const CLOSE_CODE: &str = "__moquiLoad.close()";

/// What a user's and the plan's engine evaluate before the file: the run's params (and `env`, values for declared
/// params the run does not give), and the top level begun.
pub(crate) fn begin_code(params: &Value, env: &Value) -> String {
    begin_code_with(params, env, false)
}

/// [`begin_code`], `lenient` for a listing: a required param with no value is null rather than an error.
fn begin_code_with(params: &Value, env: &Value, lenient: bool) -> String {
    let object = |value: &Value| if value.is_object() { value.clone() } else { json!({}) };
    format!(
        "__moquiLoad.begin({}, {}, {{ lenient: {lenient} }})",
        object(params),
        object(env)
    )
}

/// Evaluate `script` on a thread of its own with the run's `params`, and `env` for the declared params they leave out
/// (`moqui test-load`'s MOQUI_TEST_<NAME> variables), and read what its top level declared.
///
/// # Errors
///
/// When the file does not evaluate (the error names its line), a param is missing or of the wrong type, or the
/// thread does not start.
pub(crate) fn read_plan(
    host: &Arc<dyn SpecHost>,
    script: &LoadScript,
    params: &Value,
    env: &Value,
) -> Result<Plan, String> {
    read_plan_with(host, script, params, env, false)
}

/// [`read_plan`] for a list of scripts, with no params given: a required param is null, not an error.
///
/// # Errors
///
/// As [`read_plan`], but for a missing required param.
pub(crate) fn read_plan_listed(host: &Arc<dyn SpecHost>, script: &LoadScript, env: &Value) -> Result<Plan, String> {
    read_plan_with(host, script, &json!({}), env, true)
}

fn read_plan_with(
    host: &Arc<dyn SpecHost>,
    script: &LoadScript,
    params: &Value,
    env: &Value,
    lenient: bool,
) -> Result<Plan, String> {
    let thread = ScriptThread {
        name: "moqui-load-plan".to_string(),
        before: begin_code_with(params, env, lenient),
        file: Some(ScriptFile {
            display: script.display.clone(),
            source: script.source.clone(),
        }),
        call: "() => __moquiLoad.describe()".to_string(),
        args_json: "[]".to_string(),
        after: CLOSE_CODE.to_string(),
        around: None,
    };
    let handle = run_script_on_thread(Arc::clone(host), thread)
        .map_err(|err| format!("load: {}: the thread did not start: {err}", script.display))?;
    let deadline = Instant::now() + PLAN_DEADLINE;
    while !handle.is_finished() {
        if Instant::now() >= deadline {
            // left to end on its own: a top level that does not end in this time is a script to fix
            return Err(format!(
                "load: {}: its top level did not end in {} seconds",
                script.display,
                PLAN_DEADLINE.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let json = handle
        .join()
        .unwrap_or_else(|_| Err("the thread panicked".to_string()))
        .map_err(|err| format!("load: {}: {err}", script.display))?;
    let described: Value =
        serde_json::from_str(&json).map_err(|err| format!("load: {}: the plan did not read: {err}", script.display))?;
    let scenarios = described
        .get("scenarios")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|scenario| {
            let name = scenario.get("name").and_then(Value::as_str)?;
            Some((name.to_string(), scenario.clone()))
        })
        .collect();
    Ok(Plan {
        plan: described.get("plan").cloned().unwrap_or(Value::Null),
        params: described.get("params").cloned().unwrap_or_else(|| json!({})),
        scenarios,
    })
}

/// The settings a run overrides.
const SETTINGS: [&str; 7] = [
    "users",
    "rampUp",
    "duration",
    "iterations",
    "think",
    "pause",
    "newSession",
];

/// The settings a run of `plan` uses: each scenario's defaults, with the run's `users`, `rampUp`, `duration`,
/// `iterations`, `think` and `pause` over them, and `scenarios.<name>` over those. With `scenarios`, only the
/// scenarios it names run.
///
/// # Errors
///
/// When the script has no scenario, `scenarios` names one it does not have, or a scenario's settings are out of range.
pub(crate) fn settings(plan: &Plan, run: &Value) -> Result<Vec<ScenarioSettings>, String> {
    if plan.scenarios.is_empty() {
        return Err("load: the load script declares no scenario(name, { run })".to_string());
    }
    // `scenarios: { name: { ...settings } }`, or `scenarios: ["name", ...]` to pick some with the script's settings
    let selected: Option<Map<String, Value>> = match run.get("scenarios") {
        Some(Value::Object(selected)) => Some(selected.clone()),
        Some(Value::Array(names)) => {
            let mut picked = Map::new();
            for name in names {
                let Some(name) = name.as_str() else {
                    return Err(format!("load: scenarios names scenarios, not {name}"));
                };
                picked.insert(name.to_string(), json!({}));
            }
            Some(picked)
        }
        Some(Value::Null) | None => None,
        Some(other) => {
            return Err(format!(
                "load: scenarios is {{ name: settings }} or [names], not {other}"
            ))
        }
    };
    if let Some(selected) = &selected {
        for name in selected.keys() {
            if !plan.scenarios.iter().any(|(known, _)| known == name) {
                let known: Vec<&str> = plan.scenarios.iter().map(|(known, _)| known.as_str()).collect();
                return Err(format!(
                    "load: the load script has no scenario {name} (it has {})",
                    known.join(", ")
                ));
            }
        }
    }
    let mut out = Vec::new();
    for (name, defaults) in &plan.scenarios {
        let own = match &selected {
            Some(selected) => match selected.get(name) {
                Some(own) => own,
                None => continue,
            },
            None => &Value::Null,
        };
        let mut merged = defaults.as_object().cloned().unwrap_or_default();
        for over in [run, own] {
            // a run's duration replaces the script's iterations, and its iterations the script's duration, unless it
            // gives both: `--users 50 --duration 300` runs for 300 seconds, not the script's 5 iterations
            let given = |key: &str| over.get(key).is_some_and(|value| !value.is_null());
            if given("duration") && !given("iterations") {
                merged.remove("iterations");
            }
            if given("iterations") && !given("duration") {
                merged.remove("duration");
            }
            for key in SETTINGS {
                match over.get(key) {
                    Some(Value::Null) | None => {}
                    Some(value) => {
                        merged.insert(key.to_string(), value.clone());
                    }
                }
            }
        }
        // in a scenario's own settings, null unsets the script's default
        for key in SETTINGS {
            if own.get(key).is_some_and(Value::is_null) {
                merged.remove(key);
            }
        }
        out.push(ScenarioSettings::from_json(
            name,
            &Value::Object(merged),
            &format!("load: scenario {name}"),
        )?);
    }
    Ok(out)
}
