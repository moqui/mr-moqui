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

//! The writes of a canvas (plans/LLM-A2A-Assist.md 4.10, answered open question 13). A canvas never posts from the
//! page: an OpenUI Mutation (or a Vue SFC action, [`super::vue_sfc::action_request`]) names a transition of an `/apps`
//! screen, and the server evaluates its arguments from the stored program with the canvas's state
//! ([`super::openui::evaluate_mutation`]), holds it at the transition gate (a
//! render fenced with the gate `Hold`, which runs nothing and answers the held request with its hash), and runs that
//! exact request under the gate's `Approved` with the hash ([`execute`]). Two ways reach it:
//!
//! - **A click** on a Button the framework drew (`canvasAction` with the user's activation, from the browser session
//!   with its token): the click is the approval, so no card is asked for; an `AgentApproval` row records it, approved
//!   through the channel `canvas` and run at once. For a Vue SFC canvas that Button is the Save of the confirm the page
//!   draws outside the frame for a submit the frame made.
//! - **The agent proposes it** (`write_ui` with `run`): that is a write without a click, so it is an approval card like
//!   any other ([`crate::llm::approval::request`]); the run pauses until the user decides, and the approved request runs
//!   when it resumes.

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_lib::object::Value;
use serde_json::{json, Map, Value as Json};

use crate::context::ExecutionContextFactory;
use crate::llm::approval::{ChangeRow, TargetKind};
use crate::screen::{make_render, RenderOptions, RenderOrigin, TransitionGate};

/// A canvas Mutation evaluated and held at the gate: what the approval covers and what runs.
#[derive(Clone, Debug, PartialEq)]
pub struct HeldMutation {
    /// The canvas and the Mutation statement.
    pub artifact_id: String,
    pub version: i64,
    pub statement: String,
    pub method: String,
    pub path: String,
    /// The request parameters, each as its text.
    pub parameters: BTreeMap<String, String>,
    /// The gate's `heldWrite` (its `argsHash` is what `Approved` must match).
    pub held: Json,
}

impl HeldMutation {
    /// `<screenLocation>#<transition>`: the approval's target name.
    pub fn target_name(&self) -> String {
        format!(
            "{}#{}",
            self.held["screenLocation"].as_str().unwrap_or_default(),
            self.held["transition"].as_str().unwrap_or_default()
        )
    }

    pub fn args_hash(&self) -> String {
        self.held["argsHash"].as_str().unwrap_or_default().to_string()
    }

    /// The approval's arguments: exactly what runs, and what its permission is checked by.
    pub fn to_args(&self) -> Json {
        json!({"canvas": self.artifact_id, "version": self.version, "statement": self.statement, "method": self.method,
            "path": self.path, "parameters": self.parameters, "argsHash": self.args_hash(),
            "screenLocation": self.held["screenLocation"], "transition": self.held["transition"],
            "authzAction": self.held["authzAction"]})
    }

    pub fn of_args(args: &Json) -> Option<Self> {
        Some(Self {
            artifact_id: args["canvas"].as_str()?.to_string(),
            version: args["version"].as_i64().unwrap_or(0),
            statement: args["statement"].as_str()?.to_string(),
            method: args["method"].as_str().unwrap_or("POST").to_string(),
            path: args["path"].as_str()?.to_string(),
            parameters: args["parameters"]
                .as_object()
                .map(|map| map.iter().map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string())).collect())
                .unwrap_or_default(),
            held: json!({"argsHash": args["argsHash"], "screenLocation": args["screenLocation"],
                "transition": args["transition"], "authzAction": args["authzAction"]}),
        })
    }

    /// The card's rows: each parameter as proposed, every one of them (a secret-named one is never held, [`hold`]).
    pub fn changes(&self) -> Vec<ChangeRow> {
        self.parameters
            .iter()
            .map(|(name, value)| ChangeRow {
                field: name.clone(),
                label: name.clone(),
                current: String::new(),
                proposed: value.clone(),
            })
            .collect()
    }

    /// One line for the card and the model.
    pub fn summary(&self, title: &str) -> String {
        format!(
            "Run {} of canvas \"{title}\" ({} {})",
            self.statement,
            self.held["transition"].as_str().unwrap_or("a transition"),
            self.path
        )
    }

    pub fn target_kind(&self) -> TargetKind {
        TargetKind::Tool
    }
}

/// The request parameters of a Mutation's evaluated body: flat values as text, a list joined with commas; nested
/// objects as JSON text; null as an empty value, as a form posts an empty field, so every parameter the body names is
/// sent and shown.
pub fn parameters_of(body: &Json) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Json::Object(map) = body {
        for (key, value) in map {
            let text = match value {
                Json::Null => String::new(),
                Json::String(text) => text.clone(),
                Json::Bool(flag) => flag.to_string(),
                Json::Number(number) => number.to_string(),
                Json::Array(items) => items
                    .iter()
                    .map(|item| item.as_str().map_or_else(|| item.to_string(), str::to_string))
                    .collect::<Vec<_>>()
                    .join(","),
                Json::Object(_) => value.to_string(),
            };
            out.insert(key.clone(), text);
        }
    }
    out
}

/// Evaluate the Mutation `statement` of `canvas` with `state` (none: the state the page last posted) and hold it at the
/// gate as the current user from the agent's origin.
///
/// # Errors
///
/// What the model or the page reads: no such Mutation, a path that is not a transition of the apps, a request the
/// gate does not hold (it does not write, the user may not run it).
pub fn hold(
    factory: &Arc<ExecutionContextFactory>,
    agent: &crate::llm::run::AgentDef,
    run_id: Option<&str>,
    canvas: &super::Canvas,
    statement: &str,
    state: Option<&Json>,
) -> Result<HeldMutation, String> {
    let agent_id = agent.agent_id.as_str();
    let state = state.cloned().unwrap_or_else(|| canvas.data["state"].clone());
    let (method, raw_path, body) = match canvas.dialect.as_str() {
        "openui" => openui_request(factory, canvas, statement, &state)?,
        "vue-sfc" => super::vue_sfc::action_request(&canvas.program, statement, &state)?.ok_or_else(|| {
            let known: Vec<&str> = canvas.data["mutations"].as_array().into_iter().flatten().filter_map(Json::as_str).collect();
            format!(
                "canvas v{} has no action named {statement}{}",
                canvas.version,
                if known.is_empty() { String::new() } else { format!("; its actions are {}", known.join(", ")) }
            )
        })?,
        other => return Err(format!("a {other} canvas has no Mutations")),
    };
    if method == "GET" {
        return Err(format!("the Mutation {statement} reads with GET; a read is a Query"));
    }
    let path = crate::llm::tools::apps::app_path(&raw_path);
    let bare = path.split('?').next().unwrap_or_default().to_string();
    if !bare.starts_with("/apps/") {
        return Err(format!("{path} is not a transition of an /apps screen"));
    }
    if let Some(problem) = crate::llm::screen::path_problem(&bare) {
        return Err(format!("{bare}: {problem}"));
    }
    let parameters = parameters_of(&body);
    // a canvas never posts a secret: a password form is a screen's, and a secret-named value is never shown or kept
    if let Some(secret) = parameters.keys().find(|name| crate::llm::tools::data::secret_name(name)) {
        return Err(format!(
            "the Mutation {statement} sends {secret}, a secret; a canvas does not post secrets, so use the screen's own form"
        ));
    }
    let output = render(factory, agent_id, run_id, &path, &method, &parameters, TransitionGate::Hold)
        .map_err(|error| format!("{bare} could not be checked: {}", error.message()))?;
    let body: Json = serde_json::from_slice(&output.body).unwrap_or(Json::Null);
    match output.status {
        409 if body.get("heldWrite").is_some() => {
            let held = HeldMutation {
                artifact_id: canvas.artifact_id.clone(),
                version: canvas.version,
                statement: statement.to_string(),
                method,
                path,
                parameters,
                held: body["heldWrite"].clone(),
            };
            check_write(factory, &agent.role_groups(), &held)?;
            Ok(held)
        }
        200 => Err(format!(
            "{bare} does not write (it is read-only), so it is a Query, not a Mutation"
        )),
        401 | 403 => Err(format!("the user may not run {bare}")),
        404 => Err(format!("there is no transition at {bare}")),
        status => Err(format!(
            "{bare} answered {status}: {}",
            crate::llm::tools::format::cut(&output.body_text(), 200)
        )),
    }
}

/// The request of the OpenUI Mutation `statement` of `canvas`, evaluated from the stored program with `state`: its
/// method (upper case), its path and its body.
fn openui_request(
    factory: &Arc<ExecutionContextFactory>,
    canvas: &super::Canvas,
    statement: &str,
    state: &Json,
) -> Result<(String, String, Json), String> {
    let sources = super::openui::sources(factory).map_err(|error| {
        tracing::error!("OpenUI Lang is not available for a canvas write: {error}");
        "the canvas cannot run writes on this server now".to_string()
    })?;
    let evaluated = super::openui::evaluate_mutation(&sources, &canvas.program, statement, state)
        .map_err(|error| format!("the Mutation {statement} could not be evaluated: {error}"))?;
    let Some((tool, args)) = evaluated else {
        let known: Vec<&str> = canvas.data["mutations"].as_array().into_iter().flatten().filter_map(Json::as_str).collect();
        return Err(format!(
            "canvas v{} has no Mutation named {statement}{}",
            canvas.version,
            if known.is_empty() { String::new() } else { format!("; its Mutations are {}", known.join(", ")) }
        ));
    };
    if tool != "request" {
        return Err(format!("the Mutation {statement} names the tool {tool}; the only tool is request"));
    }
    let method = args["method"].as_str().unwrap_or("POST").to_ascii_uppercase();
    Ok((method, args["path"].as_str().unwrap_or_default().to_string(), args["body"].clone()))
}

/// Whether the user ∩ the agent may run `held`: the agent's role groups on the transition (`AT_XML_SCREEN_TRANS`
/// with the held write's action) and the user's own permission for the path (`path_permitted`: the screens, the
/// transition and its service), as the screen tool checks its held writes; the gate holds a write before the
/// transition's own check runs. Checked before an approval is asked, when it resumes, and for a click.
///
/// # Errors
///
/// Why it may not run.
pub fn check_write(factory: &Arc<ExecutionContextFactory>, groups: &[String], held: &HeldMutation) -> Result<(), String> {
    let transition = held.held["transition"].as_str().unwrap_or_default();
    let location = held.held["screenLocation"].as_str().unwrap_or_default();
    if !groups.is_empty() {
        let action = held.held["authzAction"].as_str().filter(|action| !action.is_empty()).unwrap_or("all");
        let name = format!("{location}/{transition}");
        match crate::user::group_permits(groups, "AT_XML_SCREEN_TRANS", &name, action) {
            Ok(true) => {}
            Ok(false) => return Err(format!("this agent's role does not permit {transition}")),
            Err(error) => {
                tracing::error!("whether the role group permits {name} could not be read: {error}");
                return Err(format!("{transition} could not be checked"));
            }
        }
    }
    let bare = held.path.split('?').next().unwrap_or_default();
    match crate::screen::path_permitted(factory, &crate::llm::screen::root_screen(factory), bare) {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!("the user is not allowed to run {transition}")),
        Err(error) => Err(format!("{transition} could not be checked ({error})")),
    }
}

/// Whether the user ∩ the agent may read `path` (a screen, or a transition that answers JSON): the agent's role groups
/// on the screen (`AT_XML_SCREEN`) or the transition (`AT_XML_SCREEN_TRANS`), viewed, and the user's own permission.
///
/// # Errors
///
/// Why it may not be read.
pub fn check_read(factory: &Arc<ExecutionContextFactory>, groups: &[String], path: &str) -> Result<(), String> {
    let root = crate::llm::screen::root_screen(factory);
    let hit = crate::screen::locate(factory, &root, path).map_err(|_| format!("there is no screen at {path}"))?;
    if !groups.is_empty() {
        let (kind, name) = match &hit.transition {
            Some(transition) => ("AT_XML_SCREEN_TRANS", format!("{}/{transition}", hit.location)),
            None => ("AT_XML_SCREEN", hit.location.clone()),
        };
        match crate::user::group_permits(groups, kind, &name, "view") {
            Ok(true) => {}
            Ok(false) => return Err(format!("this agent's role does not permit reading {path}")),
            Err(error) => {
                tracing::error!("whether the role group permits {name} could not be read: {error}");
                return Err(format!("{path} could not be checked"));
            }
        }
    }
    match crate::screen::path_permitted(factory, &root, path) {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!("the user may not read {path}")),
        Err(_) => Err(format!("there is no screen at {path}")),
    }
}

/// What a canvas write came to: whether it saved, and the transition's answer (messages, errors, its screen).
#[derive(Clone, Debug, PartialEq)]
pub struct Executed {
    pub ok: bool,
    pub data: Json,
    pub error: Option<String>,
}

/// Run `held` as the current user under the gate's `Approved` for `approval_id`: the transition runs only when the
/// request hashes to the approved one.
pub fn execute(
    factory: &Arc<ExecutionContextFactory>,
    agent_id: &str,
    run_id: Option<&str>,
    approval_id: &str,
    held: &HeldMutation,
) -> Executed {
    let gate = TransitionGate::Approved {
        approval_id: approval_id.to_string(),
        args_hash: held.args_hash(),
    };
    let output = match render(factory, agent_id, run_id, &held.path, &held.method, &held.parameters, gate) {
        Ok(output) => output,
        Err(error) => {
            return Executed {
                ok: false,
                data: Json::Null,
                error: Some(error.message().to_string()),
            };
        }
    };
    let data: Json = serde_json::from_slice(&output.body).unwrap_or(Json::Null);
    let errors = response_errors(&data);
    if output.status == 409 {
        return Executed {
            ok: false,
            data,
            error: Some("the request is not the one approved, so it did not run".to_string()),
        };
    }
    let ok = output.status < 400 && errors.is_empty();
    Executed {
        ok,
        error: (!ok).then(|| {
            if errors.is_empty() {
                format!("the write answered {}", output.status)
            } else {
                errors.join("; ")
            }
        }),
        data,
    }
}

/// The error messages of a transition's JSON answer (it answers 200 even when its service refused the input).
pub fn response_errors(data: &Json) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for error in data["validationErrors"].as_array().into_iter().flatten() {
        let field = error["fieldPretty"].as_str().or_else(|| error["field"].as_str());
        let message = error["message"].as_str().unwrap_or("Validation error");
        out.push(match field {
            Some(field) => format!("{message} (for field {field})"),
            None => message.to_string(),
        });
    }
    match &data["errors"] {
        Json::Array(list) => out.extend(list.iter().filter_map(Json::as_str).map(str::to_string)),
        Json::String(text) if !text.is_empty() => out.push(text.clone()),
        _ => {}
    }
    for info in data["messageInfos"].as_array().into_iter().flatten() {
        let kind = info["type"].as_str().unwrap_or_default().to_ascii_lowercase();
        if kind == "danger" || kind == "error" {
            if let Some(message) = info["message"].as_str() {
                out.push(message.to_string());
            }
        }
    }
    out
}

/// The text of a write's outcome for the model: saved with the messages, or what refused it.
pub fn outcome_text(executed: &Executed, held: &HeldMutation) -> String {
    let messages: Vec<String> = executed.data["messageInfos"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|info| info["message"].as_str().map(str::to_string))
        .collect();
    if executed.ok {
        let mut text = format!("The user approved {} and it ran: saved.", held.statement);
        if !messages.is_empty() {
            text.push_str(&format!(" message: {}", messages.join("; ")));
        }
        text
    } else {
        format!(
            "The user approved {}, but it did not save: {}",
            held.statement,
            executed.error.clone().unwrap_or_default()
        )
    }
}

fn render(
    factory: &Arc<ExecutionContextFactory>,
    agent_id: &str,
    run_id: Option<&str>,
    path: &str,
    method: &str,
    parameters: &BTreeMap<String, String>,
    gate: TransitionGate,
) -> Result<crate::screen::RenderOutput, crate::context::Error> {
    let (bare, query) = path.split_once('?').unwrap_or((path, ""));
    let mut values: BTreeMap<String, Value> = BTreeMap::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        values.insert(crate::web::percent_decode(key), Value::string(crate::web::percent_decode(value)));
    }
    for (key, value) in parameters {
        values.insert(key.clone(), Value::string(value));
    }
    make_render(factory)
        .root_screen(crate::llm::screen::root_screen(factory))
        .screen_path(bare)
        .method(method)
        .parameters(values)
        .header("Accept", "application/json")
        .options(RenderOptions {
            origin: RenderOrigin::Agent {
                agent_id: agent_id.to_string(),
                run_id: run_id.map(str::to_string),
            },
            gate,
            write_fence: true,
        })
        .render()
}

/// The `AgentApproval` row of a click: the user's click on a framework-drawn Button is the approval (open question 13),
/// so the row is made Executing with the channel `canvas`, and [`settle`] records the outcome.
///
/// # Errors
///
/// When the row cannot be written.
pub fn record_click(
    facade: &crate::entity::EntityFacade,
    user_id: &str,
    agent_id: &str,
    canvas: &super::Canvas,
    held: &HeldMutation,
) -> Result<String, moqui_lib::error::Error> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| moqui_lib::error::Error::new(format!("random source failed: {error}")))?;
    let approval_id = moqui_lib::codec::hex_encode(&bytes);
    let args = held.to_args();
    let hash = crate::llm::approval::approval_hash(user_id, agent_id, "write_ui", held.target_kind(), &held.target_name(), &args);
    let now = crate::llm::run::now_millis();
    let snapshot = json!({"summary": held.summary(&canvas.title), "changes": held.changes(), "record": Map::new()});
    let mut row = facade.make_value("moqui.agent.AgentApproval")?;
    row.set("approvalId", Value::string(&approval_id));
    row.set("runId", Value::string(&canvas.run_id));
    row.set("userId", Value::string(user_id));
    row.set("agentId", Value::string(agent_id));
    row.set("classificationEnumId", Value::string(crate::llm::tools::Classification::Write.enum_id()));
    row.set("targetKind", Value::string(held.target_kind().as_str()));
    row.set("targetName", Value::string(held.target_name()));
    row.set("argumentsJson", Value::string(args.to_string()));
    row.set("argumentsHash", Value::string(&hash));
    row.set("snapshotJson", Value::string(snapshot.to_string()));
    row.set("statusId", Value::string(crate::llm::approval::ApprovalStatus::Executing.status_id()));
    row.set("decisionUserId", Value::string(user_id));
    row.set("decisionDate", crate::llm::run::stamp(now));
    row.set("decisionChannel", Value::string(crate::llm::approval::Channel::Canvas.as_str()));
    row.set("createdDate", crate::llm::run::stamp(now));
    row.set("expireDate", crate::llm::run::stamp(now));
    row.create(facade)?;
    Ok(approval_id)
}

/// Record what a click's write came to: Executed or Failed with its result.
///
/// # Errors
///
/// When the row cannot be written.
pub fn settle(facade: &crate::entity::EntityFacade, approval_id: &str, executed: &Executed) -> Result<(), moqui_lib::error::Error> {
    let status = if executed.ok {
        crate::llm::approval::ApprovalStatus::Executed
    } else {
        crate::llm::approval::ApprovalStatus::Failed
    };
    facade
        .find("moqui.agent.AgentApproval")?
        .eq("approvalId", Value::string(approval_id))
        .for_update(true)
        .disable_authz()
        .update_all([
            ("statusId".to_string(), Value::string(status.status_id())),
            (
                "resultJson".to_string(),
                Value::string(json!({"ok": executed.ok, "error": executed.error}).to_string()),
            ),
        ])
        .map(|_| ())
}
