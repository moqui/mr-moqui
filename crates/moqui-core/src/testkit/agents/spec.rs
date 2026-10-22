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

//! An agent spec file in the script engine: the test prelude's helpers and the agent prelude (`prelude.js`), then the
//! file, whose `agentSpec({...})` registers the tasks. [`load`] reads the plan as JSON; the setup, cleanup and check
//! functions stay in the engine and are called by index ([`phase`], [`check`]).

use std::collections::BTreeMap;

use moqui_lib::object::to_plain_string;
use serde::Deserialize;
use serde_json::Value as Json;

use crate::testkit::SpecFile;

const PRELUDE: &str = include_str!("prelude.js");
const PRELUDE_NAME: &str = "moqui-agent-prelude.js";

/// What `agentSpec({...})` gave, as data.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Plan {
    #[serde(default)]
    pub errors: Vec<String>,
    pub spec: SpecPlan,
    pub tasks: Vec<TaskPlan>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SpecPlan {
    pub agent: String,
    /// drawer, chat, tui, job, a2a or mcp.
    pub surface: String,
    /// The username the tasks run as, unless a task names its own.
    pub user: String,
    /// Seconds a turn may take.
    pub timeout: Option<f64>,
    #[serde(default)]
    pub requires: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct TaskPlan {
    pub name: String,
    /// The task's file name: its name in lower case, words joined by `-`.
    pub slug: String,
    pub start: Option<StartPlan>,
    pub user: Option<String>,
    pub turns: Vec<TurnPlan>,
    #[serde(default)]
    pub approvals: Vec<ScriptedApproval>,
    /// Answers to the agent's questions, in order.
    #[serde(default)]
    pub answers: Vec<String>,
    pub timeout: Option<f64>,
    pub ignore: Option<String>,
    /// The writes the task expects held for approval, when its checks say.
    #[serde(rename = "expectHeld")]
    pub expect_held: Option<u32>,
}

/// The page the user is on when the task starts.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct StartPlan {
    pub path: String,
    #[serde(default)]
    pub parameters: BTreeMap<String, Json>,
    pub title: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct TurnPlan {
    pub say: String,
}

/// A scripted answer to an approval: approve, edit (new values), or reject (feedback).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct ScriptedApproval {
    /// approve, edit or reject.
    pub kind: String,
    #[serde(default)]
    pub values: BTreeMap<String, Json>,
    #[serde(default)]
    pub feedback: String,
}

/// Evaluate the spec file in the entered context's engine and read its plan. `Err` is why it did not load.
pub fn load(spec: &SpecFile) -> Result<Plan, String> {
    crate::script::clear_context_stack();
    crate::testkit::run::install_prelude().map_err(|err| format!("the test prelude did not load: {err}"))?;
    eval(PRELUDE, PRELUDE_NAME).map_err(|err| format!("the agent prelude did not load: {err}"))?;
    let file = serde_json::to_string(&spec.display).unwrap_or_default();
    eval(&format!("__agentEval.reset({file})"), PRELUDE_NAME)?;
    // The wrapper opens on the file's first line, so a line in an error is the file's.
    eval(&format!("(function(){{{}\n}})();", spec.source), &spec.display)?;
    let plan = eval("__agentEval.plan()", PRELUDE_NAME)?;
    let plan: Json = serde_json::from_str(&plan).map_err(|err| format!("the agent spec plan did not read: {err}"))?;
    let errors: Vec<String> = plan
        .get("errors")
        .and_then(|errors| serde_json::from_value(errors.clone()).ok())
        .unwrap_or_default();
    if !errors.is_empty() {
        return Err(format!("{}: {}", spec.display, errors.join("; ")));
    }
    serde_json::from_value(plan).map_err(|err| format!("{}: the agent spec plan did not read: {err}", spec.display))
}

/// Run the spec's `setup` or `cleanup` (`task` `None`), or a task's. `Err` is what it threw.
pub fn phase(task: Option<usize>, name: &str) -> Result<(), String> {
    let call = match task {
        Some(index) => format!("__agentEval.taskPhase({index}, {})", Json::from(name)),
        None => format!("__agentEval.specPhase({})", Json::from(name)),
    };
    let out: Json = serde_json::from_str(&eval(&call, PRELUDE_NAME)?).unwrap_or(Json::Null);
    match out.get("error").and_then(Json::as_str) {
        Some(error) => Err(error.to_string()),
        None => Ok(()),
    }
}

/// The task's checks over what its run did (`observed`, [`super::observe::Observed`] as JSON): the failures.
pub fn check(task: usize, observed: &Json) -> Vec<String> {
    let text = serde_json::to_string(&observed.to_string()).unwrap_or_default();
    match eval(&format!("__agentEval.check({task}, {text})"), PRELUDE_NAME) {
        Ok(out) => serde_json::from_str::<Json>(&out)
            .ok()
            .and_then(|out| out.get("failures").cloned())
            .and_then(|failures| serde_json::from_value(failures).ok())
            .unwrap_or_else(|| vec![format!("the checks answered {out}")]),
        Err(error) => vec![format!("the checks failed: {error}")],
    }
}

fn eval(source: &str, name: &str) -> Result<String, String> {
    crate::script::eval_named_value(source, name)
        .map(|value| to_plain_string(&value))
        .map_err(|err| err.to_string())
}
